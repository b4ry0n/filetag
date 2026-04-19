//! AI/VLM image and archive analysis for `filetag-web`.
//!
//! Handlers accept a file path, send it to a configured OpenAI-compatible or
//! Ollama endpoint, and apply the returned tags to the database.  All AI
//! configuration is stored per-root in the `settings` table.

use std::path::Path;
use std::sync::Arc;

use axum::{
    extract::{Query, State},
    response::Json,
};
use base64::Engine;
use filetag_lib::db;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::archive::{archive_image_entries, archive_list_entries_raw, archive_read_entry};
use crate::preview::{raw_cache_path, raw_extract_jpeg};
use crate::state::Features;
use crate::video::video_info;
use crate::state::{
    AppError, AppState, open_conn, open_for_file_op, open_for_file_op_under, root_for_dir,
};

// ---------------------------------------------------------------------------
// AI concurrency + constants
// ---------------------------------------------------------------------------

/// Limit concurrent AI analysis calls to one at a time.
static AI_LIMITER: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

const AI_DEFAULT_PROMPT: &str = "\
Look at this image. Output ONLY a JSON array of short descriptive tags (English, lowercase). \
Tags can be plain strings or key=value pairs when a specific attribute value matters.\n\n\
Good: [\"dog\", \"beach\", \"sunny\", \"color=blue\", \"year=2023\"]\n\
Bad: any text outside the JSON array\n\n\
/no_think";

/// File extensions recognised as still images for AI analysis.
pub const AI_IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "bmp", "avif", "tiff", "tif", "heic", "heif", "arw",
    "cr2", "cr3", "nef", "orf", "rw2", "dng", "raf", "pef", "srw", "raw", "3fr", "x3f", "rwl",
    "iiq", "mef", "mos",
];

/// Archive extensions that can be analysed by sampling their image entries.
pub const ARCHIVE_EXTS: &[&str] = &["zip", "cbz", "rar", "cbr", "7z", "cb7"];

/// Video extensions that can be analysed by sampling frames.
pub const AI_VIDEO_EXTS: &[&str] = &[
    "mp4", "mov", "avi", "mkv", "wmv", "m4v", "webm", "flv", "mpg", "mpeg",
    "m2ts", "mts", "ts", "3gp", "f4v",
];

/// Number of frames sampled from a video for AI analysis.
const VIDEO_FRAME_COUNT: usize = 6;

const AI_VIDEO_PROMPT: &str = "\
You are analysing a video file. \
You are given a series of frames sampled from throughout the video. \
Based on these frames, output ONLY a JSON array of short descriptive tags \
(English, lowercase) that describe the video as a whole. \
Tags can be plain strings or key=value pairs when a specific attribute value matters.\n\
\n\
Good: [\"action\", \"outdoor\", \"sport\", \"location=beach\"]\n\
Bad: any text outside the JSON array\n\
\n\
/no_think";

const AI_ARCHIVE_PROMPT: &str = "\
You are analysing the contents of an archive file. \
You are given a listing of all filenames inside and a few sample images extracted from it. \
Based on the filenames and the sample images, output ONLY a JSON array of short descriptive tags \
(English, lowercase) that describe the archive as a whole.\
Tags can be plain strings or key=value pairs when a specific attribute value matters.\n\
\n\
Good: [\"manga\", \"action\", \"black and white\", \"language=japanese\"]\n\
Bad: any text outside the JSON array\n\
\n\
/no_think";

// ---------------------------------------------------------------------------
// Progress tracking
// ---------------------------------------------------------------------------

/// Progress snapshot for the running (or most recently completed) AI batch job.
#[derive(Default, Clone, Serialize)]
pub struct AiProgress {
    /// `true` while a batch is actively running.
    pub running: bool,
    /// Number of files processed so far.
    pub done: usize,
    /// Total number of files in the batch.
    pub total: usize,
    /// Relative path of the file currently being analysed.
    pub current: Option<String>,
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

struct AiConfig {
    endpoint: String,
    model: String,
    api_key: Option<String>,
    tag_prefix: String,
    max_tokens: u32,
    format: String,
    prompt: Option<String>,
}

fn load_ai_config(conn: &Connection) -> Option<AiConfig> {
    let endpoint = db::get_setting(conn, "ai.endpoint").ok().flatten()?;
    if endpoint.is_empty() {
        return None;
    }
    // If explicitly disabled, return None (endpoint is preserved).
    if db::get_setting(conn, "ai.enabled")
        .ok()
        .flatten()
        .as_deref()
        == Some("0")
    {
        return None;
    }
    let model = db::get_setting(conn, "ai.model")
        .ok()
        .flatten()
        .unwrap_or_default();
    let api_key = db::get_setting(conn, "ai.api_key").ok().flatten();
    let tag_prefix = db::get_setting(conn, "ai.tag_prefix")
        .ok()
        .flatten()
        .unwrap_or_else(|| "ai/".to_string());
    let max_tokens = db::get_setting(conn, "ai.max_tokens")
        .ok()
        .flatten()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(512);
    let format = db::get_setting(conn, "ai.format")
        .ok()
        .flatten()
        .unwrap_or_else(|| "openai".to_string());
    let prompt = db::get_setting(conn, "ai.prompt")
        .ok()
        .flatten()
        .filter(|s| !s.is_empty());
    Some(AiConfig {
        endpoint,
        model,
        api_key,
        tag_prefix,
        max_tokens,
        format,
        prompt,
    })
}

// ---------------------------------------------------------------------------
// Image preparation
// ---------------------------------------------------------------------------

/// Prepare a JPEG suitable for AI analysis (max 800px, stripped metadata).
async fn ai_prepare_jpeg(abs_path: &Path, root: &Path) -> Option<Vec<u8>> {
    let ext = abs_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let source_path;
    let _tmp_data: Option<Vec<u8>>;

    match ext.as_str() {
        "arw" | "cr2" | "cr3" | "nef" | "orf" | "rw2" | "dng" | "raf" | "pef" | "srw" | "raw"
        | "3fr" | "x3f" | "rwl" | "iiq" | "mef" | "mos" => {
            if let Some(cache) = raw_cache_path(abs_path, root) {
                if !cache.exists() {
                    // AI analysis implies external tool availability for RAW extraction.
                    let feats = Features {
                        imagemagick: true,
                        ..Features::default()
                    };
                    if let Some(data) = raw_extract_jpeg(abs_path, feats).await {
                        let _ = tokio::fs::write(&cache, &data).await;
                    } else {
                        return None;
                    }
                }
                source_path = cache;
                _tmp_data = None;
            } else {
                return None;
            }
        }
        _ => {
            source_path = abs_path.to_path_buf();
            _tmp_data = None;
        }
    }

    let path_layer = format!("{}[0]", source_path.display());
    for cmd in &["magick", "convert"] {
        if let Ok(out) = tokio::process::Command::new(cmd)
            .arg(&path_layer)
            .args([
                "-auto-orient",
                "-strip",
                "-resize",
                "800x800>",
                "-quality",
                "85",
                "jpg:-",
            ])
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            && out.status.success()
            && out.stdout.starts_with(&[0xFF, 0xD8])
        {
            return Some(out.stdout);
        }
    }

    // ffmpeg fallback
    if let Ok(out) = tokio::process::Command::new("nice")
        .args(["-n", "10", "ffmpeg", "-i"])
        .arg(&source_path)
        .args([
            "-vf",
            "scale='if(gt(iw,ih),800,-2)':'if(gt(iw,ih),-2,800)':flags=lanczos",
            "-vframes",
            "1",
            "-map_metadata",
            "-1",
            "-f",
            "image2pipe",
            "-vcodec",
            "mjpeg",
            "-q:v",
            "4",
            "pipe:1",
        ])
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        && out.status.success()
        && out.stdout.starts_with(&[0xFF, 0xD8])
    {
        return Some(out.stdout);
    }

    tokio::fs::read(&source_path).await.ok()
}

/// Prepare a JPEG from raw bytes (e.g. an archive entry) for AI analysis.
async fn ai_prepare_jpeg_from_bytes(bytes: Vec<u8>, ext: &str) -> Option<Vec<u8>> {
    if let Ok(mut child) = tokio::process::Command::new("magick")
        .args([
            "-",
            "-auto-orient",
            "-strip",
            "-resize",
            "800x800>",
            "jpeg:-",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        && let Some(mut stdin) = child.stdin.take()
    {
        use tokio::io::AsyncWriteExt;
        let bytes_for_stdin = bytes.clone();
        let write_handle = tokio::spawn(async move {
            let _ = stdin.write_all(&bytes_for_stdin).await;
        });
        if let Ok(out) = child.wait_with_output().await {
            let _ = write_handle.await;
            if out.status.success() && !out.stdout.is_empty() {
                return Some(out.stdout);
            }
        } else {
            write_handle.abort();
        }
    }

    let e = ext.to_lowercase();
    if matches!(
        e.as_str(),
        "jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp" | "avif"
    ) {
        Some(bytes)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// VLM / LLM API calls
// ---------------------------------------------------------------------------

/// Strip `<think>...</think>` blocks from reasoning model output.
fn strip_think_blocks(text: &str) -> &str {
    let t = text.trim();
    if let Some(end) = t.rfind("</think>") {
        t[end + "</think>".len()..].trim()
    } else {
        t
    }
}

/// Make a single VLM/LLM API call and return the assistant message content.
async fn vlm_call(
    config: &AiConfig,
    prompt: &str,
    b64_image: Option<&str>,
) -> anyhow::Result<String> {
    let images: Vec<&str> = b64_image.into_iter().collect();
    vlm_call_multi(config, prompt, &images).await
}

/// Make a VLM/LLM API call with zero or more base64-encoded images.
async fn vlm_call_multi(
    config: &AiConfig,
    prompt: &str,
    b64_images: &[&str],
) -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?;

    let raw = if config.format == "ollama" {
        let url = format!("{}/api/chat", config.endpoint.trim_end_matches('/'));
        let msg = if b64_images.is_empty() {
            serde_json::json!({ "role": "user", "content": prompt })
        } else {
            serde_json::json!({ "role": "user", "content": prompt, "images": b64_images })
        };
        let body = serde_json::json!({
            "model": config.model,
            "stream": false,
            "messages": [msg],
            "options": { "num_predict": config.max_tokens }
        });
        let mut req = client.post(&url).json(&body);
        if let Some(key) = &config.api_key
            && !key.is_empty()
        {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        let raw = req.send().await?.error_for_status()?.text().await?;
        let resp: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
        resp["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string()
    } else {
        // OpenAI-compatible
        let url = format!(
            "{}/v1/chat/completions",
            config.endpoint.trim_end_matches('/')
        );
        let mut content_parts = vec![serde_json::json!({"type": "text", "text": prompt})];
        for b64 in b64_images {
            let data_uri = format!("data:image/jpeg;base64,{b64}");
            content_parts
                .push(serde_json::json!({"type": "image_url", "image_url": {"url": data_uri}}));
        }
        let body = serde_json::json!({
            "model": config.model,
            "max_tokens": config.max_tokens,
            "messages": [{"role": "user", "content": content_parts}]
        });
        let mut req = client.post(&url).json(&body);
        if let Some(key) = &config.api_key
            && !key.is_empty()
        {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        let raw = req.send().await?.error_for_status()?.text().await?;
        let resp: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
        let content_str = resp["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("");
        let reasoning = resp["choices"][0]["message"]["reasoning_content"]
            .as_str()
            .unwrap_or("");
        let raw_text = if !content_str.is_empty() {
            content_str
        } else {
            reasoning
        };
        raw_text.to_string()
    };

    Ok(strip_think_blocks(&raw).to_string())
}

// ---------------------------------------------------------------------------
// Tag parsing + application
// ---------------------------------------------------------------------------

/// Remove any tags that are bare kv keys (i.e. equal to a known key without a
/// `=value` part).  Models sometimes emit just `name` instead of `name=alice`
/// even when instructed otherwise; this is a safety net.
fn filter_bare_kv_keys(tags: Vec<String>, kv_keys: &[String], prefix: &str) -> Vec<String> {
    if kv_keys.is_empty() {
        return tags;
    }
    tags.into_iter()
        .filter(|t| {
            // Strip prefix to get the raw tag part.
            let raw = if prefix.is_empty() {
                t.as_str()
            } else {
                t.strip_prefix(prefix).unwrap_or(t.as_str())
            };
            // A bare kv key has no `=` and matches a known key name.
            if raw.contains('=') {
                return true; // has a value, keep it
            }
            !kv_keys.iter().any(|k| k == raw)
        })
        .collect()
}

fn build_ai_prompt(base_prompt: &str, existing_tags: &[String], kv_keys: &[String]) -> String {
    let mut prefix_lines: Vec<String> = Vec::new();

    if !existing_tags.is_empty() {
        let list = existing_tags
            .iter()
            .map(|t| format!("\"{t}\""))
            .collect::<Vec<_>>()
            .join(", ");
        prefix_lines.push(format!(
            "The file already has these tags: [{list}]. Only suggest additional tags that complement these; do not repeat them."
        ));
    }

    if !kv_keys.is_empty() {
        let keys_list = kv_keys
            .iter()
            .map(|k| format!("\"{k}\""))
            .collect::<Vec<_>>()
            .join(", ");
        prefix_lines.push(format!(
            "The following key=value tag keys are already in use in this collection: [{keys_list}]. \
For each key where you can determine a specific value for this file, include it as a \"key=value\" entry in your output. \
Do NOT output a bare key without a value (e.g. output \"name=alice\", never just \"name\"). \
If you cannot determine a value for a key, omit that key entirely."
        ));
    }

    if prefix_lines.is_empty() {
        return base_prompt.to_string();
    }
    format!(
        "{prefix}\n\n{base_prompt}",
        prefix = prefix_lines.join("\n")
    )
}

async fn analyse_image(
    config: &AiConfig,
    jpeg_bytes: &[u8],
    existing_tags: &[String],
    kv_keys: &[String],
) -> anyhow::Result<(String, Vec<String>)> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(jpeg_bytes);
    let base = config.prompt.as_deref().unwrap_or(AI_DEFAULT_PROMPT);
    let prompt = build_ai_prompt(base, existing_tags, kv_keys);
    let raw = vlm_call(config, &prompt, Some(&b64)).await?;
    let tags = parse_ai_tags(&raw, &config.tag_prefix)?;
    let tags = filter_bare_kv_keys(tags, kv_keys, &config.tag_prefix);
    Ok((raw, tags))
}

/// Maximum number of sample images to extract from an archive for AI analysis.
const ARCHIVE_SAMPLE_COUNT: usize = 4;

/// Analyse a video by extracting evenly-spaced frames with ffmpeg and sending
/// them to the VLM.  Uses `ffprobe` (via [`video_info`]) to determine the
/// duration so that frames are spread across the full length of the video.
async fn analyse_video(
    config: &AiConfig,
    abs: &Path,
    existing_tags: &[String],
    kv_keys: &[String],
) -> anyhow::Result<(String, Vec<String>)> {
    // Determine video duration so we can spread frames evenly.
    let duration = video_info(abs)
        .await
        .map(|i| i.duration)
        .unwrap_or(0.0);

    let n = VIDEO_FRAME_COUNT;
    // Seek positions: evenly spaced, avoiding the very first and last second
    // to skip black leader/credits.
    let offsets: Vec<f64> = if duration > 2.0 {
        (0..n)
            .map(|i| 1.0 + (duration - 2.0) * (i as f64) / ((n - 1) as f64))
            .collect()
    } else {
        // Very short clip: just use equal fractions.
        (0..n)
            .map(|i| duration * (i as f64 + 0.5) / n as f64)
            .collect()
    };

    let mut sample_b64: Vec<String> = Vec::new();
    for ss in &offsets {
        let out = tokio::process::Command::new("nice")
            .args(["-n", "10", "ffmpeg", "-ss", &format!("{ss:.3}"), "-i"])
            .arg(abs)
            .args([
                "-vf",
                "scale='if(gt(iw,ih),800,-2)':'if(gt(iw,ih),-2,800)':flags=lanczos",
                "-vframes",
                "1",
                "-map_metadata",
                "-1",
                "-f",
                "image2pipe",
                "-vcodec",
                "mjpeg",
                "-q:v",
                "4",
                "pipe:1",
            ])
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .output()
            .await;
        if let Ok(o) = out {
            if o.status.success() && o.stdout.starts_with(&[0xFF, 0xD8]) {
                sample_b64.push(base64::engine::general_purpose::STANDARD.encode(&o.stdout));
            }
        }
    }

    if sample_b64.is_empty() {
        anyhow::bail!("could not extract any frames from video");
    }

    let base_prompt = format!(
        "This video is {:.0} seconds long. {} frames have been sampled from it.\n\n{}",
        duration,
        sample_b64.len(),
        AI_VIDEO_PROMPT,
    );
    let prompt = build_ai_prompt(&base_prompt, existing_tags, kv_keys);
    let b64_refs: Vec<&str> = sample_b64.iter().map(|s| s.as_str()).collect();
    let raw = vlm_call_multi(config, &prompt, &b64_refs).await?;
    let tags = parse_ai_tags(&raw, &config.tag_prefix)?;
    let tags = filter_bare_kv_keys(tags, kv_keys, &config.tag_prefix);
    Ok((raw, tags))
}

/// Analyse an archive by inspecting its contents listing and sample images.
async fn analyse_archive(
    config: &AiConfig,
    archive_abs: &Path,
    existing_tags: &[String],
    kv_keys: &[String],
) -> anyhow::Result<(String, Vec<String>)> {
    let arc = archive_abs.to_path_buf();

    // Gather entry listing and image entries in a blocking task.
    let (all_entries, image_names) = {
        let arc2 = arc.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let entries = archive_list_entries_raw(&arc2)?;
            let images = archive_image_entries(&arc2)?;
            Ok((entries, images))
        })
        .await??
    };

    if all_entries.is_empty() {
        anyhow::bail!("archive is empty");
    }

    // Build a truncated file listing for the prompt.
    let listing = build_archive_listing(&all_entries);

    // Pick evenly-spaced sample images.
    let sample_names = pick_samples(&image_names, ARCHIVE_SAMPLE_COUNT);

    // Extract and prepare JPEG bytes for each sample.
    let mut sample_b64: Vec<String> = Vec::new();
    for name in &sample_names {
        let arc3 = arc.clone();
        let ename = name.clone();
        let entry_result = tokio::task::spawn_blocking(move || archive_read_entry(&arc3, &ename))
            .await
            .ok()
            .and_then(|r| r.ok());
        if let Some((bytes, _mime)) = entry_result {
            let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
            if let Some(jpeg) = ai_prepare_jpeg_from_bytes(bytes, &ext).await {
                sample_b64.push(base64::engine::general_purpose::STANDARD.encode(&jpeg));
            }
        }
    }

    // Build the prompt.
    let base_prompt = format!(
        "This archive contains {} files ({} images).\n\nFile listing:\n{}\n\n{}",
        all_entries.len(),
        image_names.len(),
        listing,
        AI_ARCHIVE_PROMPT,
    );
    let prompt = build_ai_prompt(&base_prompt, existing_tags, kv_keys);

    let b64_refs: Vec<&str> = sample_b64.iter().map(|s| s.as_str()).collect();
    let raw = vlm_call_multi(config, &prompt, &b64_refs).await?;
    let tags = parse_ai_tags(&raw, &config.tag_prefix)?;
    let tags = filter_bare_kv_keys(tags, kv_keys, &config.tag_prefix);
    Ok((raw, tags))
}

/// Build a compact textual listing of archive entries (truncated to ~80 entries).
fn build_archive_listing(entries: &[(String, u64, bool)]) -> String {
    let max_shown = 80;
    let mut lines: Vec<String> = entries
        .iter()
        .take(max_shown)
        .map(|(name, size, _)| {
            if *size > 0 {
                format!("- {} ({})", name, format_size_compact(*size))
            } else {
                format!("- {}", name)
            }
        })
        .collect();
    if entries.len() > max_shown {
        lines.push(format!("  … and {} more files", entries.len() - max_shown));
    }
    lines.join("\n")
}

fn format_size_compact(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Pick up to `n` evenly-spaced items from a list.
fn pick_samples(items: &[String], n: usize) -> Vec<String> {
    if items.len() <= n {
        return items.to_vec();
    }
    let step = items.len() as f64 / n as f64;
    (0..n)
        .map(|i| items[(i as f64 * step) as usize].clone())
        .collect()
}

fn parse_ai_tags(text: &str, prefix: &str) -> anyhow::Result<Vec<String>> {
    let trimmed = text.trim();

    let mut raw_tags: Option<Vec<String>> = None;
    let bytes = trimmed.as_bytes();
    let mut search_from = trimmed.len();
    while let Some(end_off) = trimmed[..search_from].rfind(']') {
        if let Some(start_off) = trimmed[..end_off].rfind('[') {
            let candidate = &trimmed[start_off..=end_off];
            if let Ok(arr) = serde_json::from_str::<Vec<String>>(candidate)
                && !arr.is_empty()
            {
                raw_tags = Some(arr);
                break;
            }
        }
        if end_off == 0 {
            break;
        }
        search_from = end_off;
    }
    let _ = bytes;

    let raw_tags: Vec<String> = raw_tags.unwrap_or_else(|| {
        trimmed
            .replace(['[', ']', '"'], "")
            .split([',', '\n'])
            .map(|s| {
                s.trim()
                    .trim_start_matches(['-', '*', '•'])
                    .trim()
                    .to_string()
            })
            .filter(|s| tag_candidate_ok(s))
            .collect()
    });

    let mut seen = std::collections::HashSet::new();
    let tags: Vec<String> = raw_tags
        .into_iter()
        .map(|t| {
            // Strip leading/trailing punctuation that the model sometimes appends
            // (e.g. trailing dot, comma, exclamation mark).  Characters that are
            // legitimate *inside* a tag (hyphen, underscore, slash, equals sign,
            // period as a decimal separator like "5.1") are left intact when they
            // occur in the middle of the string.
            let clean = t
                .trim()
                .trim_matches(|c: char| {
                    c.is_ascii_punctuation() && c != '/' && c != '=' && c != '-' && c != '_'
                })
                .to_lowercase();
            if prefix.is_empty() {
                clean
            } else {
                format!("{prefix}{clean}")
            }
        })
        .filter(|t| {
            let tag_part = if prefix.is_empty() {
                t.as_str()
            } else {
                &t[prefix.len()..]
            };
            tag_candidate_ok(tag_part) && seen.insert(t.clone())
        })
        .collect();

    Ok(tags)
}

/// Return true if `s` looks like a real short tag rather than a reasoning fragment.
fn tag_candidate_ok(s: &str) -> bool {
    if s.is_empty() || s.len() > 50 {
        return false;
    }
    if s.contains(':') || s.contains('*') || s.contains('(') || s.contains(')') {
        return false;
    }
    // Reject tags that still contain sentence-level punctuation that has no
    // business being inside a tag name.
    if s.contains('.')
        || s.contains('!')
        || s.contains('?')
        || s.contains(';')
        || s.contains(',')
        || s.contains('"')
        || s.contains('\'')
        || s.contains('`')
    {
        return false;
    }
    let first = s.chars().next().unwrap_or(' ');
    if (first.is_ascii_alphanumeric()) && s.chars().nth(1) == Some('.') {
        return false;
    }
    s.split_whitespace().count() <= 4
}

/// Apply AI-generated tags to a file, removing any previous AI tags first.
fn apply_ai_tags(
    conn: &Connection,
    root: &Path,
    rel_path: &str,
    tags: &[String],
    prefix: &str,
) -> anyhow::Result<()> {
    let file_rec = if rel_path.contains("::") {
        db::get_or_index_archive_entry(conn, rel_path)?
    } else {
        db::get_or_index_file(conn, rel_path, root)?
    };
    let existing = db::tags_for_file(conn, file_rec.id)?;

    let existing_names: std::collections::HashSet<String> = existing
        .iter()
        .filter(|(name, _)| !name.starts_with(prefix))
        .map(|(name, _)| name.to_lowercase())
        .collect();

    if !prefix.is_empty() {
        for (name, value) in &existing {
            if name.starts_with(prefix)
                && let Ok(tag_id) = db::get_or_create_tag(conn, name)
            {
                let _ = db::remove_tag(conn, file_rec.id, tag_id, value.as_deref());
            }
        }
    }

    for tag_str in tags {
        let (name, value) = if let Some(eq) = tag_str.find('=') {
            (
                tag_str[..eq].to_string(),
                Some(tag_str[eq + 1..].to_string()),
            )
        } else {
            (tag_str.clone(), None)
        };
        let bare = if !prefix.is_empty() && name.starts_with(prefix) {
            name[prefix.len()..].to_string()
        } else {
            name.clone()
        };
        if existing_names.contains(&bare) {
            continue;
        }
        let tag_id = db::get_or_create_tag(conn, &name)?;
        db::apply_tag(conn, file_rec.id, tag_id, value.as_deref())?;
    }

    Ok(())
}

/// Remove all tags whose name starts with `prefix` from a file.
fn remove_prefixed_tags(
    conn: &Connection,
    root: &Path,
    rel_path: &str,
    prefix: &str,
) -> anyhow::Result<()> {
    let file_rec = if rel_path.contains("::") {
        db::get_or_index_archive_entry(conn, rel_path)?
    } else {
        db::get_or_index_file(conn, rel_path, root)?
    };
    let existing = db::tags_for_file(conn, file_rec.id)?;
    for (name, value) in &existing {
        if name.starts_with(prefix)
            && let Ok(tag_id) = db::get_or_create_tag(conn, name)
        {
            let _ = db::remove_tag(conn, file_rec.id, tag_id, value.as_deref());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared helper: fetch existing non-AI tags for a file + kv-keys used in DB
// ---------------------------------------------------------------------------

/// Returns `(existing_tags, kv_keys)` where:
/// - `existing_tags` = non-ai/ tags already on this specific file
/// - `kv_keys`       = all key names that have at least one value anywhere in
///   the database, excluding the ai/ prefix
fn fetch_existing_tags(
    conn: &Connection,
    root: &Path,
    rel: &str,
    tag_prefix: &str,
) -> (Vec<String>, Vec<String>) {
    let rec_result = if rel.contains("::") {
        db::get_or_index_archive_entry(conn, rel)
    } else {
        db::get_or_index_file(conn, rel, root)
    };
    let existing_tags: Vec<String> = if let Ok(rec) = rec_result {
        db::tags_for_file(conn, rec.id)
            .unwrap_or_default()
            .into_iter()
            .filter(|(name, _)| !name.starts_with(tag_prefix))
            .map(|(name, value)| match value.as_deref().unwrap_or("") {
                "" => name,
                v => format!("{name}={v}"),
            })
            .collect()
    } else {
        vec![]
    };

    // Collect all kv-keys (tag names that have at least one non-empty value)
    // from the whole DB, excluding the ai/ prefix so the signal stays clean.
    let kv_keys: Vec<String> = db::all_tags(conn)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(name, _count, _color, has_values)| {
            if name.starts_with(tag_prefix) || !has_values {
                None
            } else {
                Some(name)
            }
        })
        .collect();

    (existing_tags, kv_keys)
}

/// Prepare JPEG bytes for AI analysis from either a plain file or archive entry.
async fn prepare_jpeg_for_analysis(effective_root: &Path, rel: &str) -> Option<Vec<u8>> {
    if let Some(sep) = rel.find("::") {
        let archive_abs = effective_root.join(&rel[..sep]);
        let entry_name = rel[sep + 2..].to_string();
        let ext = entry_name.rsplit('.').next().unwrap_or("").to_lowercase();
        let (bytes, _mime) = {
            let arc = archive_abs.clone();
            let ename = entry_name.clone();
            tokio::task::spawn_blocking(move || archive_read_entry(&arc, &ename))
                .await
                .ok()?
                .ok()?
        };
        ai_prepare_jpeg_from_bytes(bytes, &ext).await
    } else {
        let abs = effective_root.join(rel);
        ai_prepare_jpeg(&abs, effective_root).await
    }
}

// ---------------------------------------------------------------------------
// API endpoints
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(crate) struct AiClearTagsRequest {
    paths: Vec<String>,
    dir: Option<String>,
    #[serde(default)]
    prefix: Option<String>,
}

/// Remove all tags whose name starts with `prefix` from the listed files.
/// Used to clear previously applied AI tags before re-analysing.
pub async fn api_ai_clear_tags(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AiClearTagsRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_for_dir(
        &state,
        std::path::Path::new(body.dir.as_deref().unwrap_or("")),
    )
    .ok_or_else(|| AppError(anyhow::anyhow!("dir is required")))?;
    let prefix = body.prefix.as_deref().unwrap_or("ai/");
    let mut cleared = 0usize;
    for path in &body.paths {
        let (conn, effective_root, rel) = open_for_file_op(db_root, path).map_err(AppError)?;
        remove_prefixed_tags(&conn, &effective_root, &rel, prefix).map_err(AppError)?;
        cleared += 1;
    }
    Ok(Json(serde_json::json!({ "cleared": cleared })))
}

#[derive(Deserialize)]
pub(crate) struct AiAnalyseRequest {
    path: String,
    dir: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

/// Analyse a single image (or archive) with the configured VLM, optionally apply tags.
pub async fn api_ai_analyse(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AiAnalyseRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = match root_for_dir(&state, Path::new(body.dir.as_deref().unwrap_or(""))) {
        Some(r) => r,
        None => return Err(AppError(anyhow::anyhow!("no database found for this path"))),
    };
    let config = {
        let root_conn = open_conn(db_root).map_err(AppError)?;
        load_ai_config(&root_conn).ok_or_else(|| {
            AppError(anyhow::anyhow!(
                "AI not configured — set endpoint in settings"
            ))
        })?
    };
    let (conn, effective_root, rel) = open_for_file_op(db_root, &body.path).map_err(AppError)?;

    // Detect archive or video files.
    let ext = body.path.rsplit('.').next().unwrap_or("").to_lowercase();
    let is_archive = ARCHIVE_EXTS.contains(&ext.as_str());
    let is_video   = AI_VIDEO_EXTS.contains(&ext.as_str());

    let _permit = AI_LIMITER
        .acquire()
        .await
        .map_err(|e| AppError(anyhow::anyhow!("AI limiter error: {e}")))?;

    let (existing_tags, kv_keys) =
        fetch_existing_tags(&conn, &effective_root, &rel, &config.tag_prefix);

    let (raw_response, tags) = if is_archive {
        let abs = effective_root.join(&rel);
        analyse_archive(&config, &abs, &existing_tags, &kv_keys)
            .await
            .map_err(AppError)?
    } else if is_video {
        let abs = effective_root.join(&rel);
        analyse_video(&config, &abs, &existing_tags, &kv_keys)
            .await
            .map_err(AppError)?
    } else {
        let jpeg = prepare_jpeg_for_analysis(&effective_root, &rel)
            .await
            .ok_or_else(|| AppError(anyhow::anyhow!("Could not prepare image for analysis")))?;
        analyse_image(&config, &jpeg, &existing_tags, &kv_keys)
            .await
            .map_err(AppError)?
    };

    let applied = if !body.dry_run && !tags.is_empty() {
        apply_ai_tags(&conn, &effective_root, &rel, &tags, &config.tag_prefix).map_err(AppError)?;
        true
    } else {
        false
    };

    Ok(Json(
        serde_json::json!({ "tags": tags, "applied": applied, "raw": if body.dry_run { raw_response } else { String::new() } }),
    ))
}

#[derive(Deserialize)]
pub(crate) struct AiBatchRequest {
    paths: Vec<String>,
    dir: Option<String>,
}

/// Queue AI analysis for a batch of images (background task).
pub async fn api_ai_analyse_batch(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AiBatchRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_for_dir(
        &state,
        std::path::Path::new(body.dir.as_deref().unwrap_or("")),
    )
    .ok_or_else(|| AppError(anyhow::anyhow!("dir is required")))?;
    let root = db_root.root.clone();
    let batch_config = {
        let root_conn = open_conn(db_root).map_err(AppError)?;
        load_ai_config(&root_conn).ok_or_else(|| {
            AppError(anyhow::anyhow!(
                "AI not configured — set endpoint in settings"
            ))
        })?
    };

    let paths: Vec<String> = body
        .paths
        .into_iter()
        .filter(|p| {
            let ext = p.rsplit('.').next().unwrap_or("").to_lowercase();
            AI_IMAGE_EXTS.contains(&ext.as_str())
                || ARCHIVE_EXTS.contains(&ext.as_str())
                || AI_VIDEO_EXTS.contains(&ext.as_str())
        })
        .collect();

    let total = paths.len();

    {
        let mut prog = state.ai_progress.lock().unwrap();
        *prog = AiProgress {
            running: true,
            done: 0,
            total,
            current: None,
        };
    }

    let state2 = Arc::clone(&state);

    tokio::spawn(async move {
        let config = batch_config;
        let marker = format!("{}analysed", config.tag_prefix);

        for (i, rel_path) in paths.iter().enumerate() {
            {
                let mut prog = state2.ai_progress.lock().unwrap();
                prog.current = Some(rel_path.clone());
                prog.done = i;
            }

            let (conn, effective_root, eff_rel) = match open_for_file_op_under(&root, rel_path) {
                Ok(t) => t,
                Err(_) => continue,
            };

            // Check marker + fetch existing tags and kv-keys
            let (existing_tags, kv_keys): (Vec<String>, Vec<String>) = {
                let rec_result = if eff_rel.contains("::") {
                    db::get_or_index_archive_entry(&conn, &eff_rel)
                } else {
                    db::get_or_index_file(&conn, &eff_rel, &effective_root)
                };
                let existing: Vec<String> = match rec_result {
                    Ok(rec) => match db::tags_for_file(&conn, rec.id) {
                        Ok(all_tags) => {
                            if all_tags.iter().any(|(n, _)| n == &marker) {
                                continue;
                            }
                            all_tags
                                .into_iter()
                                .filter(|(name, _)| !name.starts_with(&config.tag_prefix))
                                .map(|(name, value)| match value.as_deref().unwrap_or("") {
                                    "" => name,
                                    v => format!("{name}={v}"),
                                })
                                .collect()
                        }
                        Err(_) => vec![],
                    },
                    Err(_) => vec![],
                };
                let kv: Vec<String> = db::all_tags(&conn)
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|(name, _, _, has_values)| {
                        if name.starts_with(&config.tag_prefix) || !has_values {
                            None
                        } else {
                            Some(name)
                        }
                    })
                    .collect();
                (existing, kv)
            };

            let ext = rel_path.rsplit('.').next().unwrap_or("").to_lowercase();
            let is_archive = ARCHIVE_EXTS.contains(&ext.as_str());
            let is_video   = AI_VIDEO_EXTS.contains(&ext.as_str());

            let _permit = match AI_LIMITER.acquire().await {
                Ok(p) => p,
                Err(_) => break,
            };

            let (_raw, tags) = if is_archive {
                let abs = effective_root.join(&eff_rel);
                match analyse_archive(&config, &abs, &existing_tags, &kv_keys).await {
                    Ok(t) => t,
                    Err(_) => continue,
                }
            } else if is_video {
                let abs = effective_root.join(&eff_rel);
                match analyse_video(&config, &abs, &existing_tags, &kv_keys).await {
                    Ok(t) => t,
                    Err(_) => continue,
                }
            } else {
                let jpeg = match prepare_jpeg_for_analysis(&effective_root, &eff_rel).await {
                    Some(j) => j,
                    None => continue,
                };
                match analyse_image(&config, &jpeg, &existing_tags, &kv_keys).await {
                    Ok(t) => t,
                    Err(_) => continue,
                }
            };

            if !tags.is_empty()
                && let Ok((conn2, eff_root2, eff_rel2)) = open_for_file_op_under(&root, rel_path)
            {
                let _ = apply_ai_tags(&conn2, &eff_root2, &eff_rel2, &tags, &config.tag_prefix);
                let _ = (|| -> anyhow::Result<()> {
                    let rec = if eff_rel2.contains("::") {
                        db::get_or_index_archive_entry(&conn2, &eff_rel2)?
                    } else {
                        db::get_or_index_file(&conn2, &eff_rel2, &eff_root2)?
                    };
                    let tid = db::get_or_create_tag(&conn2, &marker)?;
                    db::apply_tag(&conn2, rec.id, tid, None)?;
                    Ok(())
                })();
            }
        }

        let mut prog = state2.ai_progress.lock().unwrap();
        *prog = AiProgress {
            running: false,
            done: total,
            total,
            current: None,
        };
    });

    Ok(Json(serde_json::json!({ "queued": total })))
}

/// Return current AI batch progress.
/// Return current AI batch progress as a JSON snapshot of [`AiProgress`].
pub async fn api_ai_status(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let prog = state.ai_progress.lock().unwrap().clone();
    Json(serde_json::json!(prog))
}

#[derive(Deserialize)]
pub(crate) struct AiConfigRequest {
    endpoint: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
    prompt: Option<String>,
    tag_prefix: Option<String>,
    max_tokens: Option<u32>,
    format: Option<String>,
    enabled: Option<bool>,
    dir: Option<String>,
}

/// Save AI configuration to the database settings table.
pub async fn api_ai_config_set(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AiConfigRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_for_dir(
        &state,
        std::path::Path::new(body.dir.as_deref().unwrap_or("")),
    )
    .ok_or_else(|| AppError(anyhow::anyhow!("dir is required")))?;
    let conn = open_conn(db_root).map_err(AppError)?;

    if let Some(v) = &body.endpoint {
        db::set_setting(&conn, "ai.endpoint", v).map_err(AppError)?;
    }
    if let Some(v) = &body.model {
        db::set_setting(&conn, "ai.model", v).map_err(AppError)?;
    }
    if let Some(v) = &body.api_key {
        db::set_setting(&conn, "ai.api_key", v).map_err(AppError)?;
    }
    if let Some(v) = &body.prompt {
        db::set_setting(&conn, "ai.prompt", v).map_err(AppError)?;
    }
    if let Some(v) = &body.tag_prefix {
        db::set_setting(&conn, "ai.tag_prefix", v).map_err(AppError)?;
    }
    if let Some(v) = body.max_tokens {
        db::set_setting(&conn, "ai.max_tokens", &v.to_string()).map_err(AppError)?;
    }
    if let Some(v) = &body.format {
        db::set_setting(&conn, "ai.format", v).map_err(AppError)?;
    }
    if let Some(v) = body.enabled {
        db::set_setting(&conn, "ai.enabled", if v { "1" } else { "0" }).map_err(AppError)?;
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
pub(crate) struct AiConfigQuery {
    dir: Option<String>,
}

/// Read AI configuration from the database settings table.
/// The `api_key` value is masked before returning.
pub async fn api_ai_config_get(
    Query(params): Query<AiConfigQuery>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_for_dir(
        &state,
        std::path::Path::new(params.dir.as_deref().unwrap_or("")),
    )
    .ok_or_else(|| AppError(anyhow::anyhow!("dir is required")))?;
    let conn = open_conn(db_root).map_err(AppError)?;

    let g = |key: &str| -> String {
        db::get_setting(&conn, key)
            .ok()
            .flatten()
            .unwrap_or_default()
    };

    let api_key_raw = g("ai.api_key");
    let api_key_masked = if api_key_raw.is_empty() {
        String::new()
    } else if api_key_raw.len() <= 8 {
        "****".to_string()
    } else {
        format!(
            "{}…{}",
            &api_key_raw[..4],
            &api_key_raw[api_key_raw.len() - 4..]
        )
    };

    let tag_prefix_raw = g("ai.tag_prefix");
    let tag_prefix = if tag_prefix_raw.is_empty() {
        "ai/".to_string()
    } else {
        tag_prefix_raw
    };
    let max_tokens = g("ai.max_tokens").parse::<u32>().unwrap_or(512);
    let format_raw = g("ai.format");
    let format = if format_raw.is_empty() {
        "openai".to_string()
    } else {
        format_raw
    };

    Ok(Json(serde_json::json!({
        "endpoint": g("ai.endpoint"),
        "model": g("ai.model"),
        "api_key": api_key_masked,
        "prompt": g("ai.prompt"),
        "tag_prefix": tag_prefix,
        "max_tokens": max_tokens,
        "format": format,
        "enabled": g("ai.enabled") != "0",
        "default_prompt": AI_DEFAULT_PROMPT,
    })))
}
