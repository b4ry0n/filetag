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
use crate::state::{
    AppError, AppState, open_conn, open_for_file_op, open_for_file_op_under, root_for_dir,
};
use crate::video::{extract_video_frames, generate_ai_sprites, sprites_for_duration, video_info};

// ---------------------------------------------------------------------------
// AI concurrency + constants
// ---------------------------------------------------------------------------

/// Limit concurrent AI analysis calls to one at a time.
static AI_LIMITER: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// Type-specific introductions — tell the model what it is looking at.
/// Users can override these per-type via `ai.prompt_image`, `ai.prompt_video`,
/// and `ai.prompt_archive` in the settings.
pub const AI_IMAGE_INTRO: &str = "\
Analyse this image in detail. Observe and tag the following aspects where present:\
\n- KNOWLEDGE: if you recognise a specific artwork, artist, person, location, cultural period, \
art movement, or technique, include it \u{2014} this is the most valuable category. \
Do not suppress recognition. \
Examples: artist/vermeer, artwork/girl-with-pearl-earring, \
period/dutch-golden-age, technique/chiaroscuro, movement/baroque, location/mauritshuis\
\n- ACTIONS: what is happening? What activities, gestures, or events are taking place?\
\n- SUBJECTS: who or what is performing the action? (people, animals, characters, objects as agents)\
\n- OBJECTS: what is being acted upon, held, used, or prominently shown?\
\n- PEOPLE/CHARACTERS: for each person or character, emit attribute tags as objects \
{\"tag\": \"hair/color=black\", \"subject\": \"soldier\"} so each trait is linked to its owner. \
Observable traits: hair/color, hair/style, clothing/type, clothing/color, age, gender. \
Never merge subject identity and attribute into a single tag string.\
\n- ENVIRONMENT: setting or location (indoors/outdoors, urban/rural, specific place type), \
architecture, furniture, vegetation\
\n- ATMOSPHERE & MOOD: tension, calm, joy, danger, melancholy, romance, humour\
\n- WEATHER & LIGHTING: time of day, weather conditions, light quality\
\n- COLOURS: dominant or distinctive colour palette\
\n- STYLE & MEDIUM: art style (photo, illustration, manga, render, painting), genre\
\nTag everything you can identify. Do not limit yourself to surface observations.";

pub const AI_VIDEO_INTRO: &str = "\
These are sampled frames from a video. Analyse the content across all frames. \
Observe and tag the following aspects where present:\
\n- KNOWLEDGE: if you recognise a specific film, series, director, actor, location, cultural period, \
or technique, include it \u{2014} this is the most valuable category. \
Do not suppress recognition. \
Examples: film/blade-runner, director/ridley-scott, period/1980s, technique/dutch-angle\
\n- ACTIONS: what activities, events, or sequences are happening across the frames?\
\n- SUBJECTS: who or what is the agent? (people, characters, animals, vehicles)\
\n- OBJECTS: what is being acted upon, used, carried, or featured prominently?\
\n- PEOPLE/CHARACTERS: for each person or character, emit attribute tags as objects \
{\"tag\": \"hair/color=black\", \"subject\": \"soldier\"} so each trait is linked to its owner. \
Observable traits: hair/color, hair/style, clothing/type, clothing/color, age, gender. \
Never merge subject identity and attribute into a single tag string.\
\n- ENVIRONMENT: location type, setting, architecture, landscape\
\n- ATMOSPHERE & MOOD: tone of the content (tense, playful, dramatic, calm, action-packed)\
\n- WEATHER & TIME OF DAY: if determinable from the frames\
\n- COLOURS: dominant or striking colour palette\
\n- STYLE & GENRE: animation, live-action, documentary, fiction, genre\
\nTag everything you can identify. \
Do NOT tag credits, title cards, captions, or on-screen text. \
Do NOT tag the contact-sheet format itself (no contact_sheet, movie_still, filmstrip, screenshot, collage).";

pub const AI_VIDEO_FULL_INTRO: &str = "\
Analyse this video. Observe and tag: actions and events, subjects performing them, \
objects being used or featured, visible traits of people or characters (hair, clothing, age), \
setting and environment, mood and atmosphere, dominant colours, and genre or style.";

pub const AI_ARCHIVE_INTRO: &str = "\
Analyse this archive's file listing and any sample images. \
Determine: the primary content type and genre, the main subjects (characters, people, topics), \
recurring visual themes, actions or events depicted, art style or medium, \
and any structural patterns in the filenames that reveal series, volumes, or authors.";

/// Default output-format instruction appended to every prompt.
/// Users can override this via `ai.output_format` in the settings.
pub const AI_OUTPUT_FORMAT: &str = "\
Output ONLY a JSON array of short descriptive tags (English). \
Return at most 25 tags. Use the tag types below \u{2014} always as SEPARATE entries in the array.\
\n\
Array elements are either a plain string (tag without subject) or an object \
{\"tag\": \"...\", \"subject\": \"...\"} (tag linked to a specific person/entity in the file). \
Use the object form ONLY when a tag describes a specific individual, not the file as a whole.\
\n\
TAG TYPES:\
\n\
1. Knowledge & context \u{2014} HIGHEST PRIORITY. Tag anything you can identify from world knowledge: \
specific artworks, artists, films, books, people, locations, cultural periods, art movements, \
techniques, styles. Use a hierarchical path or key=value.\
   \"artist/vermeer\", \"artwork/girl-with-pearl-earring\", \"period/dutch-golden-age\", \
\"technique/chiaroscuro\", \"movement/baroque\", \"location/mauritshuis\", \
\"film/blade-runner\", \"country=netherlands\", \"year=1665\"\
\n\
2. Action tags \u{2014} what is happening. Use a hierarchical path, no value.\
   \"action/fighting\", \"action/running\", \"action/cooking\"\
\n\
3. Physical attribute tags \u{2014} observable traits of a SPECIFIC person/entity. \
   Use a {\"tag\", \"subject\"} object so the attribute is linked to the right subject.\
   {\"tag\": \"hair/color=blue\", \"subject\": \"soldier\"}, {\"tag\": \"clothing/type=uniform\", \"subject\": \"soldier\"}\
   RULE: one entry per attribute per subject. Never merge subject and attribute into one tag.\
\n\
4. Object tags \u{2014} prominent objects being used, held, or featured.\
   \"object/sword\", \"object/pearl-earring\", \"object/book\"\
\n\
5. Environment tags \u{2014} setting or location.\
   \"environment/urban\", \"environment/forest\", \"environment/interior/studio\"\
\n\
6. Mood / atmosphere.\
   \"mood/tense\", \"mood/calm\", \"mood/intimate\"\
\n\
7. Genre / style / medium.\
   \"genre/portrait\", \"genre/manga\", \"style/oil-painting\", \"style/photograph\"\
\n\
8. Typed metadata \u{2014} use key=value string.\
   \"year=1665\", \"country=netherlands\", \"lang=en\"\
\n\
Avoid vague or overly broad tags. Do not repeat the filename.\n\n\
Good (Girl with a Pearl Earring by Vermeer): \
[\"artist/vermeer\", \"artwork/girl-with-pearl-earring\", \"period/dutch-golden-age\", \
\"technique/chiaroscuro\", \"movement/baroque\", \"style/oil-painting\", \"genre/portrait\", \
\"mood/intimate\", \"year=1665\", \"country=netherlands\", \
{\"tag\": \"clothing/type=turban\", \"subject\": \"girl\"}, {\"tag\": \"object/pearl-earring\", \"subject\": \"girl\"}]\n\
Bad: [\"subject=soldier\", \"blue-haired-soldier\", \"subject/soldier\"] \u{2014} wrong format or merged\n\n\
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
    "mp4", "mov", "avi", "mkv", "wmv", "m4v", "webm", "flv", "mpg", "mpeg", "m2ts", "mts", "ts",
    "3gp", "f4v",
];

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
    /// Number of videos that fell back from full-video mode to sprite mode.
    pub fallback_count: usize,
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
    /// `"sprite"` (default) = contact-sheet JPEG; `"full"` = raw video bytes.
    video_mode: String,
    /// Maximum video file size in MiB for `video_mode = "full"`.
    /// Files larger than this fall back to sprite mode automatically.
    video_max_mb: u64,
    /// Maximum number of sampled frames packed into one AI sprite sheet.
    /// Lower values produce more sheets with higher per-frame detail.
    video_sheet_max_frames: usize,
    /// Frame selection strategy for video analysis: `interval` or `scene`.
    video_frame_selection: String,
    /// Free-text description of the collection (e.g. "family photos and videos" or "bird photography").
    /// Injected into every prompt so the model has collection context.
    subject: Option<String>,
    /// User override for the image-type intro sentence.
    prompt_image: Option<String>,
    /// User override for the video-type intro sentence.
    prompt_video: Option<String>,
    /// User override for the archive-type intro sentence.
    prompt_archive: Option<String>,
    /// User override for the output format instruction.
    output_format: Option<String>,
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
    // Full video mode is currently disabled; always use sprite mode.
    let video_mode = "sprite".to_string();
    let video_max_mb = db::get_setting(conn, "ai.video_max_mb")
        .ok()
        .flatten()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(50);
    let video_sheet_max_frames = db::get_setting(conn, "ai.video_sheet_max_frames")
        .ok()
        .flatten()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(16)
        .clamp(1, 256);
    let video_frame_selection_raw = db::get_setting(conn, "ai.video_frame_selection")
        .ok()
        .flatten()
        .unwrap_or_else(|| "interval".to_string());
    let video_frame_selection = if video_frame_selection_raw == "scene" {
        "scene".to_string()
    } else {
        "interval".to_string()
    };
    let subject = db::get_setting(conn, "ai.subject")
        .ok()
        .flatten()
        .filter(|s| !s.is_empty());
    // Backward compat: if the per-type key is absent, fall back to the legacy
    // ai.prompt key (which previously acted as the image prompt).
    let legacy_prompt = db::get_setting(conn, "ai.prompt")
        .ok()
        .flatten()
        .filter(|s| !s.is_empty());
    let prompt_image = db::get_setting(conn, "ai.prompt_image")
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
        .or_else(|| legacy_prompt.clone());
    let prompt_video = db::get_setting(conn, "ai.prompt_video")
        .ok()
        .flatten()
        .filter(|s| !s.is_empty());
    let prompt_archive = db::get_setting(conn, "ai.prompt_archive")
        .ok()
        .flatten()
        .filter(|s| !s.is_empty());
    let output_format = db::get_setting(conn, "ai.output_format")
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
        video_mode,
        video_max_mb,
        video_sheet_max_frames,
        video_frame_selection,
        subject,
        prompt_image,
        prompt_video,
        prompt_archive,
        output_format,
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
    match ext.as_str() {
        "arw" | "cr2" | "cr3" | "nef" | "orf" | "rw2" | "dng" | "raf" | "pef" | "srw" | "raw"
        | "3fr" | "x3f" | "rwl" | "iiq" | "mef" | "mos" => {
            if let Some(cache) = raw_cache_path(abs_path, root) {
                if !cache.exists() {
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
            } else {
                return None;
            }
        }
        _ => {
            source_path = abs_path.to_path_buf();
        }
    }

    let path_layer = format!("{}[0]", source_path.display());
    // Clone voor spawn_blocking zodat source_path bruikbaar blijft
    let source_path_block = source_path.clone();
    let path_layer_block = path_layer.clone();
    let jpeg_result = tokio::task::spawn_blocking(move || {
        for cmd in &["magick", "convert"] {
            let out = std::process::Command::new(cmd)
                .arg(&path_layer_block)
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
                .output();
            if let Ok(out) = out
                && out.status.success()
                && out.stdout.starts_with(&[0xFF, 0xD8])
            {
                return Some(out.stdout);
            }
        }
        // ffmpeg fallback
        let out = std::process::Command::new("nice")
            .args(["-n", "10", "ffmpeg", "-i"])
            .arg(&source_path_block)
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
            .output();
        if let Ok(out) = out
            && out.status.success()
            && out.stdout.starts_with(&[0xFF, 0xD8])
        {
            return Some(out.stdout);
        }
        None
    })
    .await
    .ok()
    .flatten();
    if let Some(jpeg) = jpeg_result {
        return Some(jpeg);
    }
    tokio::fs::read(&source_path).await.ok()
}

/// Prepare a JPEG from raw bytes (e.g. an archive entry) for AI analysis.
async fn ai_prepare_jpeg_from_bytes(bytes: Vec<u8>, ext: &str) -> Option<Vec<u8>> {
    // Verplaats blocking external tool call naar spawn_blocking
    let bytes_clone = bytes.clone();
    let ext_lc = ext.to_lowercase();
    let jpeg_result = tokio::task::spawn_blocking(move || {
        let mut cmd = std::process::Command::new("magick");
        cmd.args([
            "-",
            "-auto-orient",
            "-strip",
            "-resize",
            "800x800>",
            "jpeg:-",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
        if let Ok(mut child) = cmd.spawn() {
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                let _ = stdin.write_all(&bytes_clone);
            }
            if let Ok(out) = child.wait_with_output()
                && out.status.success()
                && !out.stdout.is_empty()
            {
                return Some(out.stdout);
            }
        }
        None
    })
    .await
    .ok()
    .flatten();
    if let Some(jpeg) = jpeg_result {
        return Some(jpeg);
    }
    if matches!(
        ext_lc.as_str(),
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

/// Read the response body and then check the HTTP status.  Unlike
/// `error_for_status()`, this preserves the body so the server's error message
/// is included in the returned error.
async fn response_text(resp: reqwest::Response) -> anyhow::Result<String> {
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        let detail = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| {
                v["error"]["message"]
                    .as_str()
                    .or_else(|| v["error"].as_str())
                    .or_else(|| v["message"].as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| text.chars().take(300).collect());
        anyhow::bail!("{status}: {detail}");
    }
    Ok(text)
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
        let raw = response_text(req.send().await?).await?;
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
        let raw = response_text(req.send().await?).await?;
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
fn filter_bare_kv_keys(tags: Vec<AiTagEntry>, kv_keys: &[String], prefix: &str) -> Vec<AiTagEntry> {
    if kv_keys.is_empty() {
        return tags;
    }
    tags.into_iter()
        .filter(|entry| {
            // Strip prefix to get the raw tag part.
            let raw = if prefix.is_empty() {
                entry.raw.as_str()
            } else {
                entry.raw.strip_prefix(prefix).unwrap_or(entry.raw.as_str())
            };
            // A bare kv key has no `=` and matches a known key name.
            if raw.contains('=') {
                return true; // has a value, keep it
            }
            !kv_keys.iter().any(|k| k == raw)
        })
        .collect()
}

/// Assemble the full prompt sent to the VLM.
///
/// The prompt has four layers, in order:
/// 1. An optional data prefix supplied by the caller (e.g. archive file listing).
/// 2. The type-specific intro (user-overridable; describes what the model is looking at).
/// 3. An optional collection subject from the database settings.
/// 4. The fixed output-format instruction (JSON array, examples, /no_think).
///
/// After the fixed section, dynamic per-file context is appended when present:
/// existing tags and known key=value keys.
fn build_full_prompt(
    intro: &str,
    config: &AiConfig,
    existing_tags: &[String],
    kv_keys: &[String],
    source_path: Option<&Path>,
    data_prefix: Option<&str>,
) -> String {
    let mut parts: Vec<String> = Vec::new();

    if let Some(prefix) = data_prefix.filter(|s| !s.is_empty()) {
        parts.push(prefix.to_string());
    }

    parts.push(intro.to_string());

    if let Some(s) = config.subject.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("Collection context: {s}"));
    }

    if let Some(path) = source_path
        && let Some(name) = path.file_name().and_then(|n| n.to_str())
        && !name.is_empty()
    {
        parts.push(format!(
            "Filename hint: \"{name}\". Treat the filename only as a weak hint, not as ground truth. Use it only when it supports the visible content or other clear evidence. Ignore misleading scanner names, hashes, sequence numbers, release names, or other arbitrary filename fragments."
        ));
    }

    let output_fmt = config
        .output_format
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(AI_OUTPUT_FORMAT);
    parts.push(output_fmt.to_string());

    if !existing_tags.is_empty() {
        let list = existing_tags
            .iter()
            .map(|t| format!("\"{t}\""))
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!(
            "The file already has these tags: [{list}]. Only suggest additional tags that complement these; do not repeat them."
        ));
    }

    if !kv_keys.is_empty() {
        let keys_list = kv_keys
            .iter()
            .map(|k| format!("\"{k}\""))
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!(
            "The following key=value tag keys are already in use in this collection: [{keys_list}]. \
For each key where you can determine a specific value for this file, include it as a \"key=value\" entry in your output. \
Do NOT output a bare key without a value (e.g. output \"name=alice\", never just \"name\"). \
If you cannot determine a value for a key, omit that key entirely."
        ));
    }

    parts.join("\n\n")
}

async fn analyse_image(
    config: &AiConfig,
    source_path: &Path,
    jpeg_bytes: &[u8],
    existing_tags: &[String],
    kv_keys: &[String],
) -> anyhow::Result<(String, Vec<AiTagEntry>)> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(jpeg_bytes);
    let intro = config.prompt_image.as_deref().unwrap_or(AI_IMAGE_INTRO);
    let prompt = build_full_prompt(
        intro,
        config,
        existing_tags,
        kv_keys,
        Some(source_path),
        None,
    );
    let raw = vlm_call(config, &prompt, Some(&b64)).await?;
    let tags = parse_ai_tags(&raw, &config.tag_prefix)?;
    let tags = filter_bare_kv_keys(tags, kv_keys, &config.tag_prefix);
    Ok((raw, tags))
}

/// Maximum number of sample images to extract from an archive for AI analysis.
const ARCHIVE_SAMPLE_COUNT: usize = 4;

/// Analyse a video by generating a dedicated AI sprite sheet and sending it
/// as a single image to the VLM.  Uses a separate cache key from the trickplay
/// sprites so frame count can differ independently.
async fn analyse_video_sprite(
    config: &AiConfig,
    abs: &Path,
    root: &Path,
    existing_tags: &[String],
    kv_keys: &[String],
    n_frames: Option<u32>,
) -> anyhow::Result<(String, Vec<AiTagEntry>)> {
    let info = video_info(abs)
        .await
        .ok_or_else(|| anyhow::anyhow!("cannot read video metadata"))?;

    let n = n_frames
        .map(|v| (v as usize).clamp(2, 256))
        .unwrap_or_else(|| sprites_for_duration(info.duration, 8, 64));
    let use_scene_select = config.video_frame_selection == "scene";

    // Keep each image compact (multi-sheet) so the model retains more per-frame detail.
    let sprite_paths = generate_ai_sprites(
        abs,
        root,
        n,
        info.duration,
        config.video_sheet_max_frames,
        use_scene_select,
    )
    .await?;
    let mut sprite_b64 = Vec::with_capacity(sprite_paths.len());
    for sprite_path in &sprite_paths {
        let sprite_bytes = tokio::fs::read(sprite_path).await?;
        sprite_b64.push(base64::engine::general_purpose::STANDARD.encode(&sprite_bytes));
    }
    let b64_refs: Vec<&str> = sprite_b64.iter().map(|s| s.as_str()).collect();

    let intro = config.prompt_video.as_deref().unwrap_or(AI_VIDEO_INTRO);
    let prompt = build_full_prompt(intro, config, existing_tags, kv_keys, Some(abs), None);
    let raw = vlm_call_multi(config, &prompt, &b64_refs).await?;
    let tags = parse_ai_tags(&raw, &config.tag_prefix)?;
    let tags = filter_bare_kv_keys(tags, kv_keys, &config.tag_prefix);
    Ok((raw, tags))
}

/// Analyse a video by extracting individual frames and sending them as
/// separate JPEG images to the VLM (like OpenWebUI does).  This is compatible
/// with any vision model that accepts multiple images; it does NOT send raw
/// video bytes.
///
/// If frame extraction fails, the caller falls back to sprite mode.
async fn analyse_video_full(
    config: &AiConfig,
    abs: &Path,
    existing_tags: &[String],
    kv_keys: &[String],
    n_frames: Option<u32>,
) -> anyhow::Result<(String, Vec<AiTagEntry>)> {
    let info = video_info(abs)
        .await
        .ok_or_else(|| anyhow::anyhow!("cannot read video metadata"))?;

    let n = n_frames
        .map(|v| (v as usize).clamp(2, 256))
        .unwrap_or_else(|| sprites_for_duration(info.duration, 8, 24));
    let use_scene_select = config.video_frame_selection == "scene";

    let frames = extract_video_frames(abs, n, info.duration, use_scene_select).await?;
    let b64_frames: Vec<String> = frames
        .iter()
        .map(|b| base64::engine::general_purpose::STANDARD.encode(b))
        .collect();
    let b64_refs: Vec<&str> = b64_frames.iter().map(|s| s.as_str()).collect();

    let intro = config
        .prompt_video
        .as_deref()
        .unwrap_or(AI_VIDEO_FULL_INTRO);
    let prompt = build_full_prompt(intro, config, existing_tags, kv_keys, Some(abs), None);
    let raw = vlm_call_multi(config, &prompt, &b64_refs).await?;
    let tags = parse_ai_tags(&raw, &config.tag_prefix)?;
    let tags = filter_bare_kv_keys(tags, kv_keys, &config.tag_prefix);
    Ok((raw, tags))
}

/// Dispatch video analysis to the configured mode (`sprite` or `full`).
///
/// When `video_mode` is `"full"`, the raw video is sent to the VLM.  If the
/// video exceeds the size limit or the API call fails, the function falls back
/// to sprite mode automatically.
///
/// Returns `(raw_response, tags, warning)`.  `warning` is `Some` when a
/// fallback occurred and describes why.
async fn analyse_video(
    config: &AiConfig,
    abs: &Path,
    root: &Path,
    existing_tags: &[String],
    kv_keys: &[String],
    n_frames: Option<u32>,
) -> anyhow::Result<(String, Vec<AiTagEntry>, Option<String>)> {
    if config.video_mode == "full" {
        let max_bytes = config.video_max_mb.saturating_mul(1024 * 1024);
        if let Ok(meta) = std::fs::metadata(abs)
            && meta.len() > max_bytes
        {
            let warning = format!(
                "video is {} MiB, exceeds full-mode limit ({} MiB); fell back to sprite mode",
                meta.len().div_ceil(1024 * 1024),
                config.video_max_mb
            );
            eprintln!("[filetag-web] {warning}");
            let (raw, tags) =
                analyse_video_sprite(config, abs, root, existing_tags, kv_keys, n_frames).await?;
            return Ok((raw, tags, Some(warning)));
        }
        match analyse_video_full(config, abs, existing_tags, kv_keys, n_frames).await {
            Ok((raw, tags)) => return Ok((raw, tags, None)),
            Err(e) => {
                let warning = format!(
                    "full-video analysis failed ({}); fell back to sprite mode",
                    e
                );
                eprintln!("[filetag-web] {warning}");
                let (raw, tags) =
                    analyse_video_sprite(config, abs, root, existing_tags, kv_keys, n_frames)
                        .await?;
                return Ok((raw, tags, Some(warning)));
            }
        }
    }
    let (raw, tags) =
        analyse_video_sprite(config, abs, root, existing_tags, kv_keys, n_frames).await?;
    Ok((raw, tags, None))
}

/// Analyse an archive by inspecting its contents listing and sample images.
async fn analyse_archive(
    config: &AiConfig,
    archive_abs: &Path,
    existing_tags: &[String],
    kv_keys: &[String],
) -> anyhow::Result<(String, Vec<AiTagEntry>)> {
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

    // Build the prompt — the archive listing is passed as a data prefix so
    // the model sees it before the type intro and collection context.
    let data_prefix = format!(
        "This archive contains {} files ({} images).\n\nFile listing:\n{}",
        all_entries.len(),
        image_names.len(),
        listing,
    );
    let intro = config.prompt_archive.as_deref().unwrap_or(AI_ARCHIVE_INTRO);
    let prompt = build_full_prompt(
        intro,
        config,
        existing_tags,
        kv_keys,
        Some(archive_abs),
        Some(&data_prefix),
    );

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

/// Tags describing the sprite-sheet or image format rather than the content.
/// These are never useful as file-content tags and are filtered out unconditionally.
const SPRITE_META_BLOCKLIST: &[&str] = &[
    // Sprite/format meta-tags
    "contact_sheet",
    "movie_still",
    "film_frame",
    "video_frame",
    "screenshot",
    "collage",
    "composite",
    "sprite_sheet",
    "storyboard",
    "montage",
    "still_frame",
    "frame_grab",
    "filmstrip",
    "thumbnail_grid",
    // Generic scene-setting words that add no value
    "interior",
    "exterior",
    "indoors",
    "outdoors",
    "lighting",
    "camera",
    "sitting",
    "standing",
    "walking",
    "talking",
    "looking",
    // Credits / text-overlay artefacts
    "credits",
    "end_credits",
    "opening_credits",
    "title_card",
    "caption",
    "text_overlay",
    "subtitles",
];

/// A single AI-generated tag, optionally linked to a subject within the file.
///
/// The `raw` field is the full prefixed tag string (e.g. `"ai/hair/color=blue"`).
/// The `subject` field identifies which entity in the file this tag describes
/// (e.g. `Some("soldier")`); `None` means the tag applies to the file as a whole.
#[derive(Debug, Clone)]
pub struct AiTagEntry {
    /// Full prefixed tag string, e.g. `"ai/hair/color=blue"`.
    pub raw: String,
    /// Subject this tag applies to within the file, e.g. `"soldier"`.
    pub subject: Option<String>,
}

impl AiTagEntry {
    /// Serialise to a human-readable string for API responses and logging.
    pub fn display(&self) -> String {
        match &self.subject {
            Some(s) if !s.is_empty() => format!("{} [{}]", self.raw, s),
            _ => self.raw.clone(),
        }
    }
}

fn parse_ai_tags(text: &str, _prefix: &str) -> anyhow::Result<Vec<AiTagEntry>> {
    let trimmed = text.trim();

    // Try to find the last valid JSON array in the response, which may contain
    // either plain strings or {"tag": "...", "subject": "..."} objects.
    let mut json_array: Option<Vec<serde_json::Value>> = None;
    let mut search_from = trimmed.len();
    while search_from > 0 {
        if let Some(end_off) = trimmed[..search_from].rfind(']') {
            if let Some(start_off) = trimmed[..end_off].rfind('[') {
                let candidate = &trimmed[start_off..=end_off];
                if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(candidate)
                    && !arr.is_empty()
                {
                    json_array = Some(arr);
                    break;
                }
            }
            search_from = end_off;
        } else {
            break;
        }
    }

    // Build raw (tag_str, subject) pairs from the parsed array.
    let raw_pairs: Vec<(String, Option<String>)> = if let Some(arr) = json_array {
        arr.into_iter()
            .filter_map(|v| match v {
                serde_json::Value::String(s) => Some((s, None)),
                serde_json::Value::Object(obj) => {
                    let tag = obj
                        .get("tag")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())?;
                    let subject = obj
                        .get("subject")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string());
                    Some((tag, subject))
                }
                _ => None,
            })
            .collect()
    } else {
        // Fallback: plain text line/comma splitting (no subject info).
        trimmed
            .replace(['[', ']', '"'], "")
            .split([',', '\n'])
            .map(|s| {
                (
                    s.trim()
                        .trim_start_matches(['-', '*', '\u{2022}'])
                        .trim()
                        .to_string(),
                    None,
                )
            })
            .filter(|(s, _)| tag_candidate_ok(s))
            .collect()
    };

    let mut seen = std::collections::HashSet::new();
    let tags: Vec<AiTagEntry> = raw_pairs
        .into_iter()
        .filter_map(|(t, subject)| {
            let clean = t
                .trim()
                .trim_matches(|c: char| {
                    c.is_ascii_punctuation() && c != '/' && c != '=' && c != '-' && c != '_'
                })
                .to_string();
            let raw = format!("ai/{clean}");
            let tag_part = &raw[3..]; // skip "ai/"
            if tag_candidate_ok(tag_part)
                && !SPRITE_META_BLOCKLIST.contains(&tag_part)
                && seen.insert(raw.clone())
            {
                Some(AiTagEntry { raw, subject })
            } else {
                None
            }
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
    tags: &[AiTagEntry],
    _prefix: &str,
) -> anyhow::Result<()> {
    let file_rec = if rel_path.contains("::") {
        db::get_or_index_archive_entry(conn, rel_path)?
    } else {
        db::get_or_index_file(conn, rel_path, root)?
    };
    let existing = db::tags_for_file(conn, file_rec.id)?;

    let ai_prefix = "ai/";
    let existing_names: std::collections::HashSet<String> = existing
        .iter()
        .filter(|(name, _)| !name.starts_with(ai_prefix))
        .map(|(name, _)| name.to_lowercase())
        .collect();

    // Remove old AI tags (always ai/ prefix).
    for (name, value) in &existing {
        if name.starts_with(ai_prefix)
            && let Ok(tag_id) = db::get_or_create_tag(conn, name)
        {
            let _ = db::remove_tag(conn, file_rec.id, tag_id, value.as_deref(), None);
        }
    }

    for entry in tags {
        let tag_str = &entry.raw;
        let (name, value) = if let Some(eq) = tag_str.find('=') {
            (
                tag_str[..eq].to_string(),
                Some(tag_str[eq + 1..].to_string()),
            )
        } else {
            (tag_str.clone(), None)
        };
        let bare = if let Some(stripped) = name.strip_prefix(ai_prefix) {
            stripped.to_string()
        } else {
            name.clone()
        };
        if existing_names.contains(&bare) {
            continue;
        }
        let tag_id = db::get_or_create_tag(conn, &name)?;
        db::apply_tag(
            conn,
            file_rec.id,
            tag_id,
            value.as_deref(),
            entry.subject.as_deref(),
        )?;
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
            let _ = db::remove_tag(conn, file_rec.id, tag_id, value.as_deref(), None);
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
        db::tags_for_file_with_subject(conn, rec.id)
            .unwrap_or_default()
            .into_iter()
            .filter(|(name, _, _)| !name.starts_with(tag_prefix))
            .map(|(name, value, subject)| {
                let tag_str = match value.as_deref().unwrap_or("") {
                    "" => name,
                    v => format!("{name}={v}"),
                };
                if subject.is_empty() {
                    tag_str
                } else {
                    format!("{tag_str} @{subject}")
                }
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

/// Batch helper: return `None` when the file is already marked as analysed.
/// Otherwise return the same tuple as `fetch_existing_tags`.
fn fetch_existing_tags_unless_marked(
    conn: &Connection,
    root: &Path,
    rel: &str,
    tag_prefix: &str,
    marker: &str,
) -> Option<(Vec<String>, Vec<String>)> {
    let rec_result = if rel.contains("::") {
        db::get_or_index_archive_entry(conn, rel)
    } else {
        db::get_or_index_file(conn, rel, root)
    };

    if let Ok(rec) = rec_result
        && let Ok(all_tags) = db::tags_for_file(conn, rec.id)
        && all_tags.iter().any(|(name, _)| name == marker)
    {
        return None;
    }

    Some(fetch_existing_tags(conn, root, rel, tag_prefix))
}

/// Shared per-file analysis dispatch used by both single and batch handlers.
async fn analyse_one_path(
    config: &AiConfig,
    effective_root: &Path,
    rel: &str,
    existing_tags: &[String],
    kv_keys: &[String],
    n_frames: Option<u32>,
) -> anyhow::Result<(String, Vec<AiTagEntry>, Option<String>)> {
    let ext = rel.rsplit('.').next().unwrap_or("").to_lowercase();
    let is_archive = ARCHIVE_EXTS.contains(&ext.as_str());
    let is_video = AI_VIDEO_EXTS.contains(&ext.as_str());

    if is_archive {
        let abs = effective_root.join(rel);
        let (raw, tags) = analyse_archive(config, &abs, existing_tags, kv_keys).await?;
        Ok((raw, tags, None))
    } else if is_video {
        let abs = effective_root.join(rel);
        analyse_video(
            config,
            &abs,
            effective_root,
            existing_tags,
            kv_keys,
            n_frames,
        )
        .await
    } else {
        let jpeg = prepare_jpeg_for_analysis(effective_root, rel)
            .await
            .ok_or_else(|| anyhow::anyhow!("Could not prepare image for analysis"))?;
        let abs = effective_root.join(rel);
        let (raw, tags) = analyse_image(config, &abs, &jpeg, existing_tags, kv_keys).await?;
        Ok((raw, tags, None))
    }
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
    /// Number of frames to sample from a video for AI analysis.
    /// Defaults to `sprites_for_duration` if not specified.
    n_frames: Option<u32>,
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

    let _permit = AI_LIMITER
        .acquire()
        .await
        .map_err(|e| AppError(anyhow::anyhow!("AI limiter error: {e}")))?;

    let (existing_tags, kv_keys) =
        fetch_existing_tags(&conn, &effective_root, &rel, &config.tag_prefix);

    let (raw_response, tags, warning) = analyse_one_path(
        &config,
        &effective_root,
        &rel,
        &existing_tags,
        &kv_keys,
        body.n_frames,
    )
    .await
    .map_err(AppError)?;

    let applied = if !body.dry_run && !tags.is_empty() {
        apply_ai_tags(&conn, &effective_root, &rel, &tags, &config.tag_prefix).map_err(AppError)?;
        true
    } else {
        false
    };

    Ok(Json(serde_json::json!({
        "tags": tags.iter().map(|e| e.display()).collect::<Vec<_>>(),
        "applied": applied,
        "raw": if body.dry_run { raw_response } else { String::new() },
        "warning": warning,
    })))
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
            fallback_count: 0,
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

            let (existing_tags, kv_keys) = match fetch_existing_tags_unless_marked(
                &conn,
                &effective_root,
                &eff_rel,
                &config.tag_prefix,
                &marker,
            ) {
                Some(v) => v,
                None => continue,
            };

            let _permit = match AI_LIMITER.acquire().await {
                Ok(p) => p,
                Err(_) => break,
            };

            let (_raw, tags, warning) = match analyse_one_path(
                &config,
                &effective_root,
                &eff_rel,
                &existing_tags,
                &kv_keys,
                None,
            )
            .await
            {
                Ok(v) => v,
                Err(_) => continue,
            };

            if warning.is_some() {
                let mut prog = state2.ai_progress.lock().unwrap();
                prog.fallback_count += 1;
            }

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
                    db::apply_tag(&conn2, rec.id, tid, None, None)?;
                    Ok(())
                })();
            }
        }

        let fallback_count = {
            let prog = state2.ai_progress.lock().unwrap();
            prog.fallback_count
        };
        let mut prog = state2.ai_progress.lock().unwrap();
        *prog = AiProgress {
            running: false,
            done: total,
            total,
            current: None,
            fallback_count,
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
    /// Free-text description of the collection.
    subject: Option<String>,
    /// Type-specific intro override for images.
    prompt_image: Option<String>,
    /// Type-specific intro override for video.
    prompt_video: Option<String>,
    /// Type-specific intro override for archives.
    prompt_archive: Option<String>,
    /// User override for the output format instruction.
    output_format: Option<String>,
    /// Legacy single-prompt field — treated as `prompt_image` when
    /// `prompt_image` is not present in the same request.
    prompt: Option<String>,
    tag_prefix: Option<String>,
    max_tokens: Option<u32>,
    format: Option<String>,
    /// `"sprite"` (default) or `"full"` (raw video bytes with sprite fallback).
    video_mode: Option<String>,
    /// Maximum video file size in MiB before falling back to sprite mode.
    video_max_mb: Option<u64>,
    /// Maximum number of sampled frames per AI sprite sheet.
    video_sheet_max_frames: Option<u32>,
    /// `interval` (default) or `scene`.
    video_frame_selection: Option<String>,
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
        // Only http/https endpoints are accepted to prevent SSRF via other schemes.
        let scheme = v.split(':').next().unwrap_or("").to_ascii_lowercase();
        if scheme != "http" && scheme != "https" {
            return Err(AppError(anyhow::anyhow!(
                "endpoint must use http:// or https://"
            )));
        }
        db::set_setting(&conn, "ai.endpoint", v).map_err(AppError)?;
    }
    if let Some(v) = &body.model {
        db::set_setting(&conn, "ai.model", v).map_err(AppError)?;
    }
    if let Some(v) = &body.api_key {
        db::set_setting(&conn, "ai.api_key", v).map_err(AppError)?;
    }
    if let Some(v) = &body.subject {
        db::set_setting(&conn, "ai.subject", v).map_err(AppError)?;
    }
    if let Some(v) = &body.prompt_image {
        db::set_setting(&conn, "ai.prompt_image", v).map_err(AppError)?;
    }
    if let Some(v) = &body.prompt_video {
        db::set_setting(&conn, "ai.prompt_video", v).map_err(AppError)?;
    }
    if let Some(v) = &body.prompt_archive {
        db::set_setting(&conn, "ai.prompt_archive", v).map_err(AppError)?;
    }
    if let Some(v) = &body.output_format {
        db::set_setting(&conn, "ai.output_format", v).map_err(AppError)?;
    }
    // Legacy: old clients send `prompt` (treated as prompt_image when the
    // new per-type field is absent).
    if body.prompt_image.is_none()
        && body.prompt_video.is_none()
        && body.prompt_archive.is_none()
        && let Some(v) = &body.prompt
    {
        db::set_setting(&conn, "ai.prompt_image", v).map_err(AppError)?;
    }
    if let Some(v) = &body.tag_prefix {
        // Reject tag prefixes containing path-traversal sequences.
        if v.contains("../") || v.contains("..\\") || v.starts_with('/') {
            return Err(AppError(anyhow::anyhow!("invalid tag prefix")));
        }
        db::set_setting(&conn, "ai.tag_prefix", v).map_err(AppError)?;
    }
    if let Some(v) = body.max_tokens {
        db::set_setting(&conn, "ai.max_tokens", &v.to_string()).map_err(AppError)?;
    }
    if let Some(v) = &body.format {
        if v != "openai" && v != "ollama" {
            return Err(AppError(anyhow::anyhow!(
                "format must be 'openai' or 'ollama'"
            )));
        }
        db::set_setting(&conn, "ai.format", v).map_err(AppError)?;
    }
    if let Some(v) = &body.video_mode {
        // Full video mode is currently disabled; always store sprite.
        let _ = v;
        db::set_setting(&conn, "ai.video_mode", "sprite").map_err(AppError)?;
    }
    if let Some(v) = body.video_max_mb {
        db::set_setting(&conn, "ai.video_max_mb", &v.to_string()).map_err(AppError)?;
    }
    if let Some(v) = body.video_sheet_max_frames {
        let clamped = v.clamp(1, 256);
        db::set_setting(&conn, "ai.video_sheet_max_frames", &clamped.to_string())
            .map_err(AppError)?;
    }
    if let Some(v) = &body.video_frame_selection {
        if v != "interval" && v != "scene" {
            return Err(AppError(anyhow::anyhow!(
                "video_frame_selection must be 'interval' or 'scene'"
            )));
        }
        db::set_setting(&conn, "ai.video_frame_selection", v).map_err(AppError)?;
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
    let video_mode_raw = g("ai.video_mode");
    let video_mode = if video_mode_raw.is_empty() {
        "sprite".to_string()
    } else {
        video_mode_raw
    };
    let video_max_mb = g("ai.video_max_mb").parse::<u64>().unwrap_or(50);
    let video_sheet_max_frames = g("ai.video_sheet_max_frames")
        .parse::<u32>()
        .unwrap_or(16)
        .clamp(1, 256);
    let video_frame_selection_raw = g("ai.video_frame_selection");
    let video_frame_selection = if video_frame_selection_raw == "scene" {
        "scene".to_string()
    } else {
        "interval".to_string()
    };
    // Backward compat: if ai.prompt_image is unset, fall back to legacy ai.prompt.
    let prompt_image_raw = g("ai.prompt_image");
    let prompt_image = if prompt_image_raw.is_empty() {
        g("ai.prompt")
    } else {
        prompt_image_raw
    };

    Ok(Json(serde_json::json!({
        "endpoint": g("ai.endpoint"),
        "model": g("ai.model"),
        "api_key": api_key_masked,
        "subject": g("ai.subject"),
        "prompt_image": prompt_image,
        "prompt_video": g("ai.prompt_video"),
        "prompt_archive": g("ai.prompt_archive"),
        "tag_prefix": tag_prefix,
        "max_tokens": max_tokens,
        "format": format,
        "video_mode": video_mode,
        "video_max_mb": video_max_mb,
        "video_sheet_max_frames": video_sheet_max_frames,
        "video_frame_selection": video_frame_selection,
        "enabled": g("ai.enabled") != "0",
        "default_prompt_image": AI_IMAGE_INTRO,
        "default_prompt_video": AI_VIDEO_INTRO,
        "default_prompt_video_full": AI_VIDEO_FULL_INTRO,
        "default_prompt_archive": AI_ARCHIVE_INTRO,
        "output_format": g("ai.output_format"),
        "default_output_format": AI_OUTPUT_FORMAT,
    })))
}

// ---------------------------------------------------------------------------
// Multi-file common-traits analysis
// ---------------------------------------------------------------------------

/// Maximum number of images to include in one common-traits analysis call.
const COMMON_TRAITS_MAX_IMAGES: usize = 8;

#[derive(Deserialize)]
pub(crate) struct AiAnalyseCommonRequest {
    paths: Vec<String>,
    dir: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

/// Analyse multiple images in a single VLM call and return only the tags that
/// describe characteristics shared across **all** of them.
///
/// Tags are applied to every path in `paths` (not just the analysable subset)
/// so that, for example, a mixed selection of images and other files all
/// receive the shared context tags.
pub async fn api_ai_analyse_common(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AiAnalyseCommonRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if body.paths.is_empty() {
        return Ok(Json(serde_json::json!({ "tags": [], "applied_count": 0 })));
    }

    let db_root = root_for_dir(&state, Path::new(body.dir.as_deref().unwrap_or("")))
        .ok_or_else(|| AppError(anyhow::anyhow!("no database found for this path")))?;

    let config = {
        let root_conn = open_conn(db_root).map_err(AppError)?;
        load_ai_config(&root_conn).ok_or_else(|| {
            AppError(anyhow::anyhow!(
                "AI not configured — set endpoint in settings"
            ))
        })?
    };

    // Collect analysable paths (images, archives, videos); cap at COMMON_TRAITS_MAX_IMAGES.
    // Videos use the ffmpeg first-frame path inside prepare_jpeg_for_analysis.
    let analysable: Vec<&str> = body
        .paths
        .iter()
        .filter(|p| {
            let ext = p.rsplit('.').next().unwrap_or("").to_lowercase();
            AI_IMAGE_EXTS.contains(&ext.as_str())
                || ARCHIVE_EXTS.contains(&ext.as_str())
                || AI_VIDEO_EXTS.contains(&ext.as_str())
        })
        .map(|s| s.as_str())
        .take(COMMON_TRAITS_MAX_IMAGES)
        .collect();

    if analysable.is_empty() {
        return Err(AppError(anyhow::anyhow!(
            "No analysable images in the selection"
        )));
    }

    let _permit = AI_LIMITER
        .acquire()
        .await
        .map_err(|e| AppError(anyhow::anyhow!("AI limiter error: {e}")))?;

    // Prepare JPEG bytes for each analysable path.
    let mut b64_images: Vec<String> = Vec::new();
    for rel_path in &analysable {
        if let Ok((_conn, eff_root, eff_rel)) = open_for_file_op(db_root, rel_path)
            && let Some(bytes) = prepare_jpeg_for_analysis(&eff_root, &eff_rel).await
        {
            b64_images.push(base64::engine::general_purpose::STANDARD.encode(&bytes));
        }
    }

    if b64_images.is_empty() {
        return Err(AppError(anyhow::anyhow!(
            "Could not prepare any images for analysis"
        )));
    }

    // Build the common-traits prompt.
    let n = b64_images.len();
    let intro = format!(
        "You are looking at {n} images. Identify ONLY characteristics that are \
shared across ALL of them. Return ONLY tags describing attributes present in \
every single image. Ignore features unique to any individual image."
    );

    let output_fmt = config
        .output_format
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(AI_OUTPUT_FORMAT);

    let mut prompt_parts = vec![intro];
    if let Some(s) = config.subject.as_deref().filter(|s| !s.is_empty()) {
        prompt_parts.push(format!("Collection context: {s}"));
    }
    prompt_parts.push(output_fmt.to_string());
    let prompt = prompt_parts.join("\n\n");

    let b64_refs: Vec<&str> = b64_images.iter().map(|s| s.as_str()).collect();
    let raw = vlm_call_multi(&config, &prompt, &b64_refs)
        .await
        .map_err(AppError)?;

    let tags = parse_ai_tags(&raw, &config.tag_prefix).map_err(AppError)?;

    // Apply tags to every path in the original selection.
    // First remove any existing ai/ tags so the result reflects only common traits.
    let mut applied_count = 0usize;
    if !body.dry_run && !tags.is_empty() {
        for rel_path in &body.paths {
            if let Ok((conn, eff_root, eff_rel)) = open_for_file_op(db_root, rel_path) {
                let _ = remove_prefixed_tags(&conn, &eff_root, &eff_rel, &config.tag_prefix);
                if apply_ai_tags(&conn, &eff_root, &eff_rel, &tags, &config.tag_prefix).is_ok() {
                    applied_count += 1;
                }
            }
        }
    }

    Ok(Json(serde_json::json!({
        "tags": tags.iter().map(|e| e.display()).collect::<Vec<_>>(),
        "applied_count": applied_count,
        "raw": if body.dry_run { raw } else { String::new() },
    })))
}

// ---------------------------------------------------------------------------
// Chat
// ---------------------------------------------------------------------------

/// A single turn in the chat conversation.
#[derive(serde::Deserialize, serde::Serialize, Clone)]
pub struct AiChatMessage {
    pub role: String, // "user" | "assistant"
    pub content: String,
}

/// Request body for `POST /api/ai/chat`.
#[derive(serde::Deserialize)]
pub struct AiChatRequest {
    /// Absolute path of the current directory (used to resolve the correct DB root).
    pub dir: Option<String>,
    /// Absolute paths of the files being discussed.  Images are encoded and
    /// sent with the first user message; other file types are skipped.
    pub files: Vec<String>,
    /// Full conversation history so far (NOT including the placeholder reply).
    pub messages: Vec<AiChatMessage>,
    /// Override frame count for video analysis (None = auto based on duration).
    pub n_frames: Option<u32>,
}

/// Response for `POST /api/ai/chat`.
#[derive(serde::Serialize)]
pub struct AiChatResponse {
    pub reply: String,
}

/// Send a multi-turn conversation to the VLM.
///
/// `b64_images` are attached to the *first* user message so the model can
/// refer back to them in subsequent turns without re-sending the pixels.
async fn vlm_chat_with_history(
    config: &AiConfig,
    messages: &[AiChatMessage],
    b64_images: &[String],
) -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()?;

    let raw = if config.format == "ollama" {
        let url = format!("{}/api/chat", config.endpoint.trim_end_matches('/'));
        let api_messages: Vec<serde_json::Value> = messages
            .iter()
            .enumerate()
            .map(|(i, m)| {
                if i == 0 && !b64_images.is_empty() {
                    serde_json::json!({
                        "role": m.role,
                        "content": m.content,
                        "images": b64_images,
                    })
                } else {
                    serde_json::json!({ "role": m.role, "content": m.content })
                }
            })
            .collect();
        let body = serde_json::json!({
            "model": config.model,
            "stream": false,
            "messages": api_messages,
            "options": { "num_predict": config.max_tokens },
        });
        let mut req = client.post(&url).json(&body);
        if let Some(key) = &config.api_key
            && !key.is_empty()
        {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        let raw = response_text(req.send().await?).await?;
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
        let api_messages: Vec<serde_json::Value> = messages
            .iter()
            .enumerate()
            .map(|(i, m)| {
                if i == 0 && !b64_images.is_empty() {
                    let mut parts = vec![serde_json::json!({"type": "text", "text": m.content})];
                    for b64 in b64_images {
                        let data_uri = format!("data:image/jpeg;base64,{b64}");
                        parts.push(serde_json::json!({
                            "type": "image_url",
                            "image_url": { "url": data_uri }
                        }));
                    }
                    serde_json::json!({ "role": m.role, "content": parts })
                } else {
                    serde_json::json!({ "role": m.role, "content": m.content })
                }
            })
            .collect();
        let body = serde_json::json!({
            "model": config.model,
            "max_tokens": config.max_tokens,
            "messages": api_messages,
        });
        let mut req = client.post(&url).json(&body);
        if let Some(key) = &config.api_key
            && !key.is_empty()
        {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        let raw = response_text(req.send().await?).await?;
        let resp: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
        let content = &resp["choices"][0]["message"]["content"];
        if let Some(s) = content.as_str() {
            s.to_string()
        } else {
            // Structured content array (some providers return this)
            content
                .as_array()
                .and_then(|arr| {
                    arr.iter()
                        .filter_map(|p| p["text"].as_str())
                        .next()
                        .map(|s| s.to_string())
                })
                .unwrap_or_default()
        }
    };

    Ok(strip_think_blocks(&raw).to_string())
}

/// `POST /api/ai/chat` — multi-turn chat about one or more selected files.
pub async fn api_ai_chat(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AiChatRequest>,
) -> Result<Json<AiChatResponse>, AppError> {
    if req.messages.is_empty() {
        return Err(AppError(anyhow::anyhow!("messages must not be empty")));
    }
    for msg in &req.messages {
        if msg.role != "user" && msg.role != "assistant" {
            return Err(AppError(anyhow::anyhow!("invalid role: {}", msg.role)));
        }
    }

    let dir = req.dir.as_deref().unwrap_or("");
    let root = root_for_dir(&state, std::path::Path::new(dir))
        .ok_or_else(|| AppError(anyhow::anyhow!("no database found for directory")))?;
    let conn = open_conn(root)?;
    let config =
        load_ai_config(&conn).ok_or_else(|| AppError(anyhow::anyhow!("AI is not configured")))?;

    // Prepare visual context for the first user message.
    // - Images: resize + encode as JPEG (up to 4 files).
    // - Videos: generate an AI sprite sheet and encode each sheet (up to 2 videos).
    // - Other file types: skip (their filenames appear in the user's message text).
    let mut b64_images: Vec<String> = Vec::new();
    let mut image_slots = 0usize;
    let mut video_slots = 0usize;
    let mut video_context = String::new();
    for file_path in &req.files {
        let abs = root.root.join(file_path);
        let ext = abs
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        if file_path.contains("::") && image_slots < 4 {
            // Virtual archive entry path (e.g. "comics/book.cbz::page001.jpg").
            // Must be checked before the plain-image branch because the virtual path
            // has an image extension but the file does not exist on disk.
            if let Some((archive_rel, entry_name)) = file_path.split_once("::") {
                let archive_abs = root.root.join(archive_rel);
                let ename = entry_name.to_string();
                let arc = archive_abs.clone();
                let entry_result =
                    tokio::task::spawn_blocking(move || archive_read_entry(&arc, &ename))
                        .await
                        .ok()
                        .and_then(|r| r.ok());
                if let Some((bytes, _)) = entry_result {
                    let entry_ext = entry_name.rsplit('.').next().unwrap_or("").to_lowercase();
                    if let Some(jpeg) = ai_prepare_jpeg_from_bytes(bytes, &entry_ext).await
                        && jpeg.starts_with(&[0xFF, 0xD8])
                    {
                        let label = format!(
                            "{}::{}",
                            archive_abs
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or(archive_rel),
                            entry_name
                        );
                        video_context.push_str(&format!("[Image: \"{label}\"]\n"));
                        b64_images.push(base64::engine::general_purpose::STANDARD.encode(&jpeg));
                        image_slots += 1;
                    }
                }
            }
        } else if AI_IMAGE_EXTS.contains(&ext.as_str()) && image_slots < 4 {
            if let Some(bytes) = ai_prepare_jpeg(&abs, &root.root).await {
                // Only accept actual JPEG output — not raw bytes of non-image files.
                if bytes.starts_with(&[0xFF, 0xD8]) {
                    let file_name = abs
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(file_path.as_str());
                    video_context.push_str(&format!("[Image: \"{file_name}\"]\n"));
                    b64_images.push(base64::engine::general_purpose::STANDARD.encode(&bytes));
                    image_slots += 1;
                }
            }
        } else if AI_VIDEO_EXTS.contains(&ext.as_str()) && video_slots < 2 {
            // Use the same sprite-sheet approach as the AI tagging pipeline.
            // A contact sheet (grid of frames) lets the model perceive the video
            // as a whole rather than treating each frame as an independent image.
            if let Some(info) = video_info(&abs).await {
                let n = req
                    .n_frames
                    .map(|v| (v as usize).clamp(2, 256))
                    .unwrap_or_else(|| sprites_for_duration(info.duration, 8, 64));
                let use_scene = config.video_frame_selection == "scene";
                let file_name = abs
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(file_path.as_str());
                let mins = (info.duration / 60.0) as u64;
                let secs = (info.duration % 60.0) as u64;
                let duration_str = format!("{mins}:{secs:02}");

                // Mirror the analyse_video logic: use full-frame mode when configured,
                // with sprite fallback on failure.
                let frames_b64: Option<Vec<String>> = if config.video_mode == "full" {
                    match extract_video_frames(&abs, n, info.duration, use_scene).await {
                        Ok(frames) => {
                            let encoded: Vec<String> = frames
                                .iter()
                                .map(|b| base64::engine::general_purpose::STANDARD.encode(b))
                                .collect();
                            if encoded.is_empty() {
                                None
                            } else {
                                Some(encoded)
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "[filetag-web] chat: full frame extraction failed for {file_name}: {e}; trying sprites"
                            );
                            match generate_ai_sprites(
                                &abs,
                                &root.root,
                                n,
                                info.duration,
                                config.video_sheet_max_frames,
                                use_scene,
                            )
                            .await
                            {
                                Ok(paths) => {
                                    let mut enc = Vec::new();
                                    for p in &paths {
                                        if let Ok(b) = tokio::fs::read(p).await {
                                            enc.push(
                                                base64::engine::general_purpose::STANDARD
                                                    .encode(&b),
                                            );
                                        }
                                    }
                                    if enc.is_empty() { None } else { Some(enc) }
                                }
                                Err(e2) => {
                                    eprintln!(
                                        "[filetag-web] chat: sprite fallback also failed for {file_name}: {e2}"
                                    );
                                    None
                                }
                            }
                        }
                    }
                } else {
                    match generate_ai_sprites(
                        &abs,
                        &root.root,
                        n,
                        info.duration,
                        config.video_sheet_max_frames,
                        use_scene,
                    )
                    .await
                    {
                        Ok(paths) => {
                            let mut enc = Vec::new();
                            for p in &paths {
                                if let Ok(b) = tokio::fs::read(p).await {
                                    enc.push(base64::engine::general_purpose::STANDARD.encode(&b));
                                }
                            }
                            if enc.is_empty() { None } else { Some(enc) }
                        }
                        Err(e) => {
                            eprintln!(
                                "[filetag-web] chat: sprite generation failed for {file_name}: {e}; trying individual frames"
                            );
                            match extract_video_frames(&abs, n, info.duration, use_scene).await {
                                Ok(frames) => {
                                    let encoded: Vec<String> = frames
                                        .iter()
                                        .map(|b| {
                                            base64::engine::general_purpose::STANDARD.encode(b)
                                        })
                                        .collect();
                                    if encoded.is_empty() {
                                        None
                                    } else {
                                        Some(encoded)
                                    }
                                }
                                Err(e2) => {
                                    eprintln!(
                                        "[filetag-web] chat: frame fallback also failed for {file_name}: {e2}"
                                    );
                                    None
                                }
                            }
                        }
                    }
                };

                if let Some(frames) = frames_b64 {
                    video_context.push_str(&format!(
                        "[Video context: the following image(s) are sampled frames from \"{file_name}\" ({duration_str}), presented as a contact sheet. Use these to answer questions about the video content. Do NOT tag or comment on the contact-sheet format itself.]\n"
                    ));
                    b64_images.extend(frames);
                    video_slots += 1;
                }
            }
        }
        // All other file types: pass filename only so the model can use it as context.
        else if ARCHIVE_EXTS.contains(&ext.as_str()) && image_slots < 4 {
            // Archive file: extract a file listing + sample images, like analyse_archive.
            let arc = abs.clone();
            let file_name = abs
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(file_path.as_str())
                .to_string();
            if let Ok((all_entries, image_names)) = tokio::task::spawn_blocking(move || {
                let entries = archive_list_entries_raw(&arc)?;
                let images = archive_image_entries(&arc)?;
                anyhow::Ok((entries, images))
            })
            .await
            .unwrap_or(Err(anyhow::anyhow!("spawn_blocking failed")))
            {
                let listing = build_archive_listing(&all_entries);
                let sample_names = pick_samples(&image_names, ARCHIVE_SAMPLE_COUNT);
                let mut sample_b64: Vec<String> = Vec::new();
                for name in &sample_names {
                    if image_slots + sample_b64.len() >= 4 {
                        break;
                    }
                    let arc2 = abs.clone();
                    let ename = name.clone();
                    let entry_result =
                        tokio::task::spawn_blocking(move || archive_read_entry(&arc2, &ename))
                            .await
                            .ok()
                            .and_then(|r| r.ok());
                    if let Some((bytes, _)) = entry_result {
                        let entry_ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
                        if let Some(jpeg) = ai_prepare_jpeg_from_bytes(bytes, &entry_ext).await {
                            sample_b64
                                .push(base64::engine::general_purpose::STANDARD.encode(&jpeg));
                        }
                    }
                }
                video_context.push_str(&format!(
                    "[Archive \"{file_name}\": {} files, {} images. File listing:\n{listing}]\n",
                    all_entries.len(),
                    image_names.len(),
                ));
                image_slots += sample_b64.len();
                b64_images.extend(sample_b64);
            }
        } else {
            let file_name = abs
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(file_path.as_str());
            video_context.push_str(&format!("[File: \"{file_name}\"]\n"));
        }
    }

    // Text files: read content and prepend to the first user message as context.
    // Each file is quoted with its filename so the model knows what it is reading.
    // Limited to 32 KiB per file, up to 4 files total.
    const TEXT_EXTS: &[&str] = &[
        "txt",
        "rst",
        "csv",
        "tsv",
        "log",
        "ini",
        "cfg",
        "conf",
        "json",
        "yaml",
        "yml",
        "toml",
        "xml",
        "html",
        "htm",
        "css",
        "js",
        "ts",
        "jsx",
        "tsx",
        "py",
        "rb",
        "rs",
        "go",
        "java",
        "c",
        "cpp",
        "h",
        "hpp",
        "sh",
        "bash",
        "zsh",
        "fish",
        "sql",
        "diff",
        "patch",
        "gitignore",
        "env",
        "md",
        "markdown",
    ];
    const MAX_TEXT_BYTES: usize = 32 * 1024;
    const MAX_TEXT_FILES: usize = 4;

    let mut text_context = String::new();
    let mut text_file_count = 0usize;
    for file_path in &req.files {
        if text_file_count >= MAX_TEXT_FILES {
            break;
        }
        let abs = root.root.join(file_path);
        let ext = abs
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        if !TEXT_EXTS.contains(&ext.as_str()) {
            continue;
        }

        let raw = match tokio::fs::read(&abs).await {
            Ok(b) => b,
            Err(_) => continue,
        };
        // Reject if it doesn't look like valid UTF-8 text.
        let text = match std::str::from_utf8(&raw) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let file_name = abs
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(file_path.as_str());
        let snippet = if text.len() > MAX_TEXT_BYTES {
            format!(
                "```{file_name}\n{}\n… (truncated)\n```",
                &text[..MAX_TEXT_BYTES]
            )
        } else {
            format!("```{file_name}\n{text}\n```")
        };
        text_context.push_str(&snippet);
        text_context.push('\n');
        text_file_count += 1;
    }

    // Prepend video and/or text context to the first user message so the model
    // has full context from the start of the conversation.
    let mut messages = req.messages.clone();
    let mut prefix = video_context.clone();
    if !text_context.is_empty() {
        prefix.push_str("The following file(s) are provided as context:\n\n");
        prefix.push_str(&text_context);
        prefix.push('\n');
    }
    if !prefix.is_empty() {
        let first = &messages[0];
        messages[0] = AiChatMessage {
            role: first.role.clone(),
            content: format!("{prefix}{}", first.content),
        };
    }

    let reply = vlm_chat_with_history(&config, &messages, &b64_images)
        .await
        .map_err(AppError)?;

    Ok(Json(AiChatResponse { reply }))
}

// ---------------------------------------------------------------------------
// Prompt Wizard — fully automatic, two-pass optimisation
// ---------------------------------------------------------------------------

/// Request for `POST /api/ai/prompt-wizard`.
#[derive(serde::Deserialize)]
pub struct PromptWizardRequest {
    pub dir: Option<String>,
    /// The user's collection description and tagging goals (from settings).
    pub goals: String,
    /// Pass 2 only: draft prompts to review and improve.
    pub draft: Option<WizardPrompts>,
}

/// Response for `POST /api/ai/prompt-wizard`.
#[derive(serde::Serialize)]
pub struct PromptWizardResponse {
    pub prompts: WizardPrompts,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct WizardPrompts {
    pub prompt_image: String,
    pub prompt_video: String,
    pub prompt_archive: String,
    pub subject: String,
    /// Optimised output-format instruction produced by the wizard.
    #[serde(default)]
    pub output_format: String,
}

/// Extract a JSON object from a model reply, stripping markdown code fences.
fn extract_json_object(text: &str) -> String {
    // Strip markdown code fences (```json ... ``` or ``` ... ```)
    let inner = if let Some(start) = text.find("```json") {
        let after = &text[start + 7..];
        after.find("```").map(|e| &after[..e]).unwrap_or(after)
    } else if let Some(start) = text.find("```") {
        let after = &text[start + 3..];
        after.find("```").map(|e| &after[..e]).unwrap_or(after)
    } else {
        text
    };
    // Find outermost { … }
    let start = inner.find('{').unwrap_or(0);
    let end = inner.rfind('}').map(|i| i + 1).unwrap_or(inner.len());
    inner[start..end].to_string()
}

const WIZARD_PASS1: &str = r#"You are an expert at writing prompts for filetag, an AI-powered file tagging system.

## filetag's tag system

filetag supports three tag styles:
1. Hierarchical paths — "/" separates levels (e.g. genre/rock, topic/nature/birds, hair/color=black)
2. Key=value pairs — typed attributes (e.g. year=1979, rating=5, country=japan, person=alice)
3. Plain tags — simple descriptors (e.g. black-and-white, documentary, illustrated)

All AI-generated tags are automatically prefixed with "<TAG_PREFIX>".
Write tags WITHOUT the prefix (e.g. "genre/rock" not "<TAG_PREFIX>genre/rock").

## How the full analysis prompt is assembled (at file analysis time)

The system builds a full prompt from these layers, in order:
  1. Type-specific intro   ← what you write as prompt_image / prompt_video / prompt_archive
  2. Collection context    ← the subject description
  3. Filename hint         ← added automatically
  4. Output format         ← what you write as output_format
  5. Existing tags         ← "The file already has these tags: [tag1, tag2, ...]" (auto-appended)
  6. KV-keys in use        ← "The following key=value keys are in use: [key1, key2]" (auto-appended)

Layers 5 and 6 are DYNAMIC: they reflect the actual tags on the specific file being analysed,
and the kv-keys that have been used across the collection at that point in time.

## Consistency requirement

Your output_format must explicitly instruct the model to:
- Follow the EXACT same hierarchical path structure as shown in layer 6 (kv-keys)
- Use the EXACT same key names already in use (e.g. if "hair/color" is listed, use "hair/color=brown",
  not "hair-color=brown" or "brown-hair")
- Only suggest tags that ADD to layer 5 (existing tags); never repeat them
- If a kv-key is listed but the value cannot be determined, omit that key entirely

## User's collection description and goals

"<GOALS>"

## Your task

Write collection-specific prompts for each file type. Each prompt (layers 1) should:
1. Describe what the model is looking at (image / sampled video frames / archive listing)
2. Mention the hierarchical path conventions for this collection (e.g. "genre/X, topic/X")
3. Name the key=value fields most relevant for this collection type
4. Be specific and concise — it is prepended to media before the model sees it

The output_format (layer 4) should:
- List the key=value fields the user cares about
- Explicitly reference layers 5 and 6: tell the model to follow existing tag structure
- Keep the JSON array format and /no_think directive
- Cap at 10 tags

Respond with ONLY valid JSON (no markdown fences, no explanation):
{
  "subject": "<1-3 sentence collection description>",
  "prompt_image": "<image intro instruction>",
  "prompt_video": "<video intro instruction>",
  "prompt_archive": "<archive intro instruction>",
  "output_format": "<full output format instruction, referencing consistency with existing tags/kv-keys>"
}"#;

const WIZARD_PASS2: &str = r#"You are reviewing AI tagging prompts for the filetag system. Critically improve them.

## filetag tag system

- Hierarchical paths with "/" (genre/rock, topic/nature/birds, hair/color=black)
- Key=value pairs for typed attributes (year=2024, rating=5, country=japan, person=alice)
- Plain tags for non-hierarchical descriptors
- AI prefix "<TAG_PREFIX>" is added automatically — write tags WITHOUT the prefix

## Dynamic context at analysis time

At analysis time the system automatically appends to every prompt:
- Layer 5: "The file already has these tags: [...]" — skip these in output
- Layer 6: "The following key=value keys are in use: [key1, key2, ...]" — use EXACT same key names

The output_format must explicitly tell the model:
- Use the exact key names shown in layer 6 (e.g. "hair/color=brown" when "hair/color" is listed)
- Suggest only tags not already in layer 5
- Omit a kv-key entirely if its value cannot be determined

Collection goals: "<GOALS>"

## Draft prompts to review

subject:        "<SUBJECT>"
prompt_image:   "<IMAGE>"
prompt_video:   "<VIDEO>"
prompt_archive: "<ARCHIVE>"
output_format:  "<OUTPUT_FORMAT>"

## Review checklist

1. Does each type prompt guide the model to use hierarchical paths (genre/X, topic/X)?
2. Does it name the specific kv-fields relevant for this collection?
3. Is it concrete about this collection's subjects/genres/attributes?
4. Does output_format explicitly reference consistency with layers 5 and 6?
5. Does output_format include JSON array, /no_think, 10-tag cap?
6. Is anything vague, redundant, or missing?

Produce a refined final version. Respond with ONLY valid JSON (no markdown):
{
  "subject": "...",
  "prompt_image": "...",
  "prompt_video": "...",
  "prompt_archive": "...",
  "output_format": "..."
}"#;

/// `POST /api/ai/prompt-wizard` — one automated pass of prompt optimisation.
/// Call twice: first without `draft` (pass 1), then with the result as `draft` (pass 2).
pub async fn api_ai_prompt_wizard(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PromptWizardRequest>,
) -> Result<Json<PromptWizardResponse>, AppError> {
    let dir = req.dir.as_deref().unwrap_or("");
    let root = root_for_dir(&state, std::path::Path::new(dir))
        .ok_or_else(|| AppError(anyhow::anyhow!("no database found for directory")))?;
    let conn = open_conn(root)?;
    let mut config =
        load_ai_config(&conn).ok_or_else(|| AppError(anyhow::anyhow!("AI is not configured")))?;
    // Wizard outputs structured JSON prompts that are much longer than tag arrays.
    // Ensure a generous token budget regardless of the per-analysis setting.
    config.max_tokens = config.max_tokens.max(4096);

    let tag_prefix = config.tag_prefix.clone();
    let goals = req.goals.trim().to_string();
    let goals_section = if goals.is_empty() {
        "No specific instructions provided — write general-purpose prompts suitable for a mixed file collection.".to_string()
    } else {
        goals.clone()
    };

    let prompt = match &req.draft {
        None => WIZARD_PASS1
            .replace("<TAG_PREFIX>", &tag_prefix)
            .replace("<GOALS>", &goals_section),
        Some(d) => WIZARD_PASS2
            .replace("<TAG_PREFIX>", &tag_prefix)
            .replace("<GOALS>", &goals_section)
            .replace("<SUBJECT>", &d.subject)
            .replace("<IMAGE>", &d.prompt_image)
            .replace("<VIDEO>", &d.prompt_video)
            .replace("<ARCHIVE>", &d.prompt_archive)
            .replace("<OUTPUT_FORMAT>", &d.output_format),
    };

    let raw = vlm_call(&config, &prompt, None).await.map_err(AppError)?;

    let json_str = extract_json_object(&raw);
    let prompts: WizardPrompts = serde_json::from_str(&json_str).map_err(|e| {
        AppError(anyhow::anyhow!(
            "AI did not return valid JSON: {e}\nResponse was: {raw}"
        ))
    })?;

    Ok(Json(PromptWizardResponse { prompts }))
}
