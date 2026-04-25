//! Video transcoding, trickplay sprites, and thumbnail generation.
//!
//! All cache artefacts are written under `<root>/.filetag/cache/` so the
//! data-isolation invariant is maintained.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    body::{Body, Bytes},
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Json, Response},
};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::preview::{file_cache_path, serve_file_range};
use crate::state::{
    AppState, THUMB_LIMITER, TRANSCODE_LIMITER, VTHUMB_LIMITER, load_features_for, resolve_preview,
    root_for_dir,
};

// ---------------------------------------------------------------------------
// Video cache eviction
// ---------------------------------------------------------------------------

/// Maximum total size (bytes) of the video transcode cache per database root.
/// When exceeded, the oldest cached files are removed until below this limit.
const VIDEO_CACHE_MAX_BYTES: u64 = 10 * 1024 * 1024 * 1024; // 10 GiB

/// Evict oldest video cache files until total size is below `max_bytes`.
pub(crate) async fn evict_video_cache(video_dir: PathBuf, max_bytes: u64) {
    let Ok(mut rd) = tokio::fs::read_dir(&video_dir).await else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
    while let Ok(Some(entry)) = rd.next_entry().await {
        let p = entry.path();
        // Only count fully written files; skip .tmp and .staging intermediates.
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != "mp4" {
            continue;
        }
        if let Ok(meta) = tokio::fs::metadata(&p).await {
            let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            files.push((mtime, meta.len(), p));
        }
    }
    let total: u64 = files.iter().map(|(_, sz, _)| *sz).sum();
    if total <= max_bytes {
        return;
    }
    // Oldest access time first.
    files.sort_unstable_by_key(|(mtime, _, _)| *mtime);
    let mut remaining = total;
    for (_, sz, path) in files {
        if remaining <= max_bytes {
            break;
        }
        if tokio::fs::remove_file(&path).await.is_ok() {
            remaining = remaining.saturating_sub(sz);
        }
    }
}

// ---------------------------------------------------------------------------
// Video info
// ---------------------------------------------------------------------------

/// Codec and duration information for a video file.
pub struct VideoInfo {
    /// Total duration in seconds.
    pub duration: f64,
    /// Codec name of the first video stream as reported by ffprobe (e.g. "h264", "hevc").
    pub video_codec: String,
    /// Codec name of the first audio stream as reported by ffprobe (e.g. "aac", "ac3").
    pub audio_codec: String,
}

impl VideoInfo {
    /// Returns the ffmpeg `-c:v` argument: "copy" when the codec can be placed
    /// directly in an MP4 container and decoded by browsers (H.264, HEVC, AV1).
    /// MPEG-4 part 2 (DivX/Xvid) and VP9 are excluded: not reliably decoded
    /// by browsers in an MP4 container.
    pub fn video_arg(&self) -> &'static str {
        match self.video_codec.as_str() {
            "h264" | "hevc" | "av1" => "copy",
            _ => "libx264",
        }
    }

    /// Returns the ffmpeg `-c:a` argument: "copy" when the audio is already AAC,
    /// otherwise transcode to AAC.
    pub fn audio_arg(&self) -> &'static str {
        if self.audio_codec == "aac" {
            "copy"
        } else {
            "aac"
        }
    }
}

/// Get video duration and codec information via a single ffprobe call.
pub async fn video_info(path: &Path) -> Option<VideoInfo> {
    let out = tokio::process::Command::new("nice")
        .args(["-n", "10", "ffprobe"])
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration:stream=codec_type,codec_name",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = std::str::from_utf8(&out.stdout).ok()?;

    // ffprobe csv output with p=0 (no section names).
    // The column order depends on the ffprobe version and platform:
    //   "h264,video"  or "video,h264"  — video stream line
    //   "aac,audio"   or "audio,aac"   — audio stream line
    //   "1234.567"                     — format duration line
    // We determine which column is codec_type by checking for "video"/"audio".
    let mut video_codec = String::new();
    let mut audio_codec = String::new();
    let mut duration = 0f64;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(d) = line.parse::<f64>() {
            if d > 0.0 {
                duration = d;
            }
        } else if let Some(comma) = line.find(',') {
            let left = &line[..comma];
            let right = &line[comma + 1..];
            // Identify which field is codec_type and which is codec_name.
            let (kind, codec) = if left == "video" || left == "audio" {
                (left, right)
            } else if right == "video" || right == "audio" {
                (right, left)
            } else {
                continue;
            };
            if kind == "video" && video_codec.is_empty() {
                video_codec = codec.to_owned();
            } else if kind == "audio" && audio_codec.is_empty() {
                audio_codec = codec.to_owned();
            }
        }
    }
    if duration <= 0.0 {
        return None;
    }
    Some(VideoInfo {
        duration,
        video_codec,
        audio_codec,
    })
}

// ---------------------------------------------------------------------------
// Video transcoding
// ---------------------------------------------------------------------------

/// Map an EXIF Orientation value to the ffmpeg `-vf` prefix needed to
/// correct the rotation before scaling. Returns an empty string for
/// orientation 1 (normal) so the scale filter can be used unmodified.
pub fn orient_to_vf_prefix(orient: u8) -> &'static str {
    match orient {
        2 => "hflip,",
        3 => "hflip,vflip,",
        4 => "vflip,",
        5 => "transpose=0,vflip,",
        6 => "transpose=1,",
        7 => "transpose=3,",
        8 => "transpose=2,",
        _ => "",
    }
}

/// Transcode (or remux) a video to MP4 and stream it to the client.
///
/// When a cached copy already exists the response uses full Range support so
/// the browser can seek freely. On the first play we spawn a single ffmpeg
/// process that outputs fragmented MP4 (`frag_keyframe+empty_moov`) to stdout.
/// The output is streamed directly to the browser via chunked transfer encoding
/// while simultaneously being written to a cache file.  This means playback
/// starts within seconds (no need to wait for the full transcode) and
/// subsequent plays are served instantly from cache.
///
/// Fast path (H.264 source): container-only remux, completes in seconds.
/// Slow path (HEVC/other): fMP4 streaming while writing to cache. Seeking
/// is disabled on first play only.
pub async fn serve_transcoded_mp4(path: &Path, root: &Path, headers: &HeaderMap) -> Response {
    let cache_path = match file_cache_path(path, root, "video", "v7.mp4") {
        Some(p) => p,
        None => return serve_file_range(path, headers).await,
    };

    // Cached copy exists: serve with full Range/seek support.
    if cache_path.exists() {
        return serve_file_range(&cache_path, headers).await;
    }

    // Acquire concurrency permit.
    let permit = match TRANSCODE_LIMITER.acquire().await {
        Ok(p) => p,
        Err(_) => return (StatusCode::SERVICE_UNAVAILABLE, "transcode queue full").into_response(),
    };

    // Re-check after acquiring permit.
    if cache_path.exists() {
        drop(permit);
        return serve_file_range(&cache_path, headers).await;
    }

    // Probe codecs; fall back to full transcode if ffprobe fails.
    let info = video_info(path).await;
    let c_video = info.as_ref().map(|i| i.video_arg()).unwrap_or("libx264");
    let c_audio = info.as_ref().map(|i| i.audio_arg()).unwrap_or("aac");

    let tmp = cache_path.with_extension("tmp.mp4");
    if let Some(parent) = cache_path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }

    if c_video == "copy" {
        // Fast path: no video encoding, just repackage the container.
        // ffmpeg finishes in a few seconds even for large files; we wait for
        // it and then serve the seekable result with Range support.
        let mut cmd = tokio::process::Command::new("nice");
        cmd.args(["-n", "10", "ffmpeg", "-y"])
            .arg("-i")
            .arg(path)
            .args([
                "-map", "0:v:0", "-map", "0:a:0?", "-c:v", "copy", "-c:a", c_audio,
            ]);
        if c_audio != "copy" {
            cmd.args(["-b:a", "128k"]);
        }
        let ok = cmd
            .args(["-sn", "-movflags", "+faststart"])
            .arg(&tmp)
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false);

        drop(permit);

        if ok {
            let _ = tokio::fs::rename(&tmp, &cache_path).await;
            if let Some(dir) = cache_path.parent() {
                let dir = dir.to_path_buf();
                tokio::spawn(evict_video_cache(dir, VIDEO_CACHE_MAX_BYTES));
            }
            return serve_file_range(&cache_path, headers).await;
        } else {
            let _ = tokio::fs::remove_file(&tmp).await;
            return serve_file_range(path, headers).await;
        }
    }

    // Slow path: video re-encoding needed (e.g. HEVC → H.264).
    // Stream fragmented MP4 to the browser while simultaneously writing to
    // cache. Seeking is not available on the first play; subsequent plays
    // are served from the seekable cache file.
    let mut cmd = tokio::process::Command::new("nice");
    cmd.args(["-n", "10", "ffmpeg", "-y"])
        .arg("-i")
        .arg(path)
        .args(["-map", "0:v:0", "-map", "0:a:0?"])
        .args([
            "-c:v", c_video, "-preset", "fast", "-crf", "23", "-threads", "2",
        ])
        .args(["-c:a", c_audio]);
    if c_audio != "copy" {
        cmd.args(["-b:a", "128k"]);
    }
    cmd.args([
        "-sn",
        "-movflags",
        "frag_keyframe+empty_moov+default_base_moof",
    ])
    .args(["-f", "mp4", "pipe:1"])
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::null())
    .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => {
            drop(permit);
            return serve_file_range(path, headers).await;
        }
    };

    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            drop(permit);
            return serve_file_range(path, headers).await;
        }
    };

    let cache_final = cache_path.clone();
    let tmp_clone = tmp.clone();

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(32);

    tokio::spawn(async move {
        let mut stdout = stdout;
        let mut file = tokio::fs::File::create(&tmp_clone).await.ok();
        let mut buf = vec![0u8; 64 * 1024];

        loop {
            match stdout.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = Bytes::copy_from_slice(&buf[..n]);
                    if let Some(ref mut f) = file {
                        let _ = f.write_all(&chunk).await;
                    }
                    if tx.send(Ok(chunk)).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    break;
                }
            }
        }

        if let Some(mut f) = file.take() {
            let _ = f.flush().await;
        }

        drop(stdout);
        let ok = child.wait().await.map(|s| s.success()).unwrap_or(false);
        drop(permit);

        if ok {
            // Post-process the fragmented MP4 into a seekable file with
            // faststart so that the second (and subsequent) plays have a
            // full seekable timeline with correct duration.
            let staging = tmp_clone.with_extension("staging.mp4");
            if tokio::fs::rename(&tmp_clone, &staging).await.is_ok() {
                let ok2 = tokio::process::Command::new("ffmpeg")
                    .args(["-y", "-i"])
                    .arg(&staging)
                    .args(["-c", "copy", "-movflags", "+faststart"])
                    .arg(&cache_final)
                    .stderr(std::process::Stdio::null())
                    .kill_on_drop(true)
                    .status()
                    .await
                    .map(|s| s.success())
                    .unwrap_or(false);
                let _ = tokio::fs::remove_file(&staging).await;
                if ok2 {
                    if let Some(dir) = cache_final.parent() {
                        evict_video_cache(dir.to_path_buf(), VIDEO_CACHE_MAX_BYTES).await;
                    }
                } else {
                    let _ = tokio::fs::remove_file(&cache_final).await;
                }
            } else {
                let _ = tokio::fs::remove_file(&tmp_clone).await;
            }
        } else {
            let _ = tokio::fs::remove_file(&tmp_clone).await;
        }
    });

    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "video/mp4")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

// ---------------------------------------------------------------------------
// Video contact-sheet thumbnail
// ---------------------------------------------------------------------------

/// Generate a JPEG contact-sheet thumbnail for a video file.
pub async fn video_thumb_strip(path: &Path, root: &Path) -> Response {
    let cache = match file_cache_path(path, root, "thumbs", "vthumb.jpg") {
        Some(p) => p,
        None => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "cache path unavailable").into_response();
        }
    };

    if let Ok(data) = tokio::fs::read(&cache).await {
        return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
    }

    let _permit = match THUMB_LIMITER.try_acquire() {
        Ok(p) => p,
        Err(_) => {
            return (StatusCode::SERVICE_UNAVAILABLE, "thumbnail queue full").into_response();
        }
    };

    if let Some(parent) = cache.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }

    let n_sprite: f64 = 8.0;
    let info = video_info(path).await;
    let ss = info
        .as_ref()
        .map(|i| format!("{:.2}", i.duration / (2.0 * n_sprite)))
        .unwrap_or_else(|| "5".to_string());

    let ok = tokio::process::Command::new("nice")
        .args(["-n", "10", "ffmpeg", "-ss", &ss, "-i"])
        .arg(path)
        .args(["-vframes", "1", "-vf", "scale=480:-2", "-q:v", "5", "-y"])
        .arg(&cache)
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);

    if ok && let Ok(data) = tokio::fs::read(&cache).await {
        return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
    }

    (
        StatusCode::UNPROCESSABLE_ENTITY,
        "Video thumbnail unavailable — install ffmpeg",
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Trickplay sprite generation
// ---------------------------------------------------------------------------

/// Generate (or reuse from cache) a trickplay sprite sheet for `abs`.
///
/// Returns the path to the cached JPEG on success.  The file is guaranteed to
/// exist when `Ok` is returned.  The frame count `n` and video `duration_secs`
/// must already be known by the caller (via [`video_info`] + [`sprites_for_duration`]).
pub async fn generate_sprite_cached(
    abs: &Path,
    root: &Path,
    n: usize,
    duration_secs: f64,
) -> anyhow::Result<PathBuf> {
    let cache_path = file_cache_path(abs, root, "vthumbs", &format!("sprite{n}x1.jpg"))
        .ok_or_else(|| anyhow::anyhow!("cannot compute cache path for {}", abs.display()))?;

    if cache_path.exists() {
        return Ok(cache_path);
    }

    if let Some(parent) = cache_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let positions: Vec<f64> = (0..n)
        .map(|i| duration_secs * (i as f64 + 0.5) / n as f64)
        .collect();

    let scale_parts: Vec<String> = (0..n)
        .map(|i| format!("[{i}:v]scale=320:-2,setsar=1[f{i}]"))
        .collect();
    let hstack_inputs: String = (0..n).map(|i| format!("[f{i}]")).collect();
    let filter = format!("{};{hstack_inputs}hstack={n}[out]", scale_parts.join(";"));

    let mut cmd = tokio::process::Command::new("nice");
    cmd.args(["-n", "10", "ffmpeg"]);
    for t in &positions {
        cmd.args(["-ss", &format!("{t:.2}"), "-i"]).arg(abs);
    }
    let ok = cmd
        .args([
            "-filter_complex",
            &filter,
            "-map",
            "[out]",
            "-frames:v",
            "1",
            "-q:v",
            "4",
            "-y",
        ])
        .arg(&cache_path)
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);

    if !ok || !cache_path.exists() {
        anyhow::bail!("ffmpeg could not generate sprite sheet — is ffmpeg installed?");
    }

    Ok(cache_path)
}

/// Extract `n` evenly-spaced frames from `abs` and return them as individual
/// JPEG byte vectors.  Uses ffmpeg; returns an error when ffmpeg is unavailable.
pub async fn extract_video_frames(
    abs: &Path,
    n: usize,
    duration_secs: f64,
    use_scene_select: bool,
) -> anyhow::Result<Vec<Vec<u8>>> {
    // Hard upper bound on the number of frames.  The truncate() below makes
    // the bound visible to static analysis tools that do not follow clamp().
    let n = n.clamp(1, 256);
    let mut positions = if use_scene_select {
        match scene_positions(abs, n, duration_secs).await {
            Ok(v) if !v.is_empty() => v,
            _ => interval_positions(n, duration_secs),
        }
    } else {
        interval_positions(n, duration_secs)
    };
    positions.truncate(256); // explicit bound so the allocation below is provably bounded

    let mut frames = Vec::with_capacity(positions.len());

    for t in positions {
        let output = tokio::process::Command::new("nice")
            .args(["-n", "10", "ffmpeg", "-ss", &format!("{t:.2}"), "-i"])
            .arg(abs)
            .args([
                "-frames:v",
                "1",
                "-vf",
                "scale=512:-2",
                "-q:v",
                "5",
                "-f",
                "image2pipe",
                "-vcodec",
                "mjpeg",
                "pipe:1",
                "-loglevel",
                "error",
            ])
            .output()
            .await?;
        if output.status.success() && !output.stdout.is_empty() {
            frames.push(output.stdout);
        }
    }

    if frames.is_empty() {
        anyhow::bail!("ffmpeg could not extract frames — is ffmpeg installed?");
    }

    Ok(frames)
}

fn interval_positions(n: usize, duration_secs: f64) -> Vec<f64> {
    (0..n)
        .map(|i| duration_secs * (i as f64 + 0.5) / n as f64)
        .collect()
}

fn parse_pts_time(line: &str) -> Option<f64> {
    let key = "pts_time:";
    let pos = line.find(key)? + key.len();
    let tail = &line[pos..];
    let tok = tail.split_whitespace().next()?;
    tok.parse::<f64>().ok()
}

fn pick_evenly(items: &[f64], n: usize) -> Vec<f64> {
    if items.len() <= n {
        return items.to_vec();
    }
    let step = items.len() as f64 / n as f64;
    (0..n)
        .map(|i| {
            let idx = (i as f64 * step).floor() as usize;
            items[idx.min(items.len() - 1)]
        })
        .collect()
}

fn build_sprite_filter(chunk_n: usize) -> (String, usize, usize) {
    let cols = (chunk_n as f64).sqrt().ceil().max(1.0) as usize;
    let rows = chunk_n.div_ceil(cols);

    let scale_parts: Vec<String> = (0..chunk_n)
        .map(|i| format!("[{i}:v]scale=320:-2,setsar=1[f{i}]"))
        .collect();

    let mut filter_parts = scale_parts;
    for row in 0..rows {
        let start = row * cols;
        let end = ((row + 1) * cols).min(chunk_n);
        if start >= end {
            continue;
        }
        let row_len = end - start;
        if row_len == cols {
            // Full row — no padding needed.
            let row_inputs: String = (start..end).map(|i| format!("[f{i}]")).collect();
            filter_parts.push(format!("{row_inputs}hstack={row_len}[r{row}]"));
        } else {
            // Partial last row: pad to full row width so vstack heights match.
            let pad_w = cols * 320;
            if row_len == 1 {
                filter_parts.push(format!("[f{start}]pad={pad_w}:ih:0:0[r{row}]"));
            } else {
                let row_inputs: String = (start..end).map(|i| format!("[f{i}]")).collect();
                filter_parts.push(format!(
                    "{row_inputs}hstack={row_len},pad={pad_w}:ih:0:0[r{row}]"
                ));
            }
        }
    }
    if rows == 1 {
        filter_parts.push("[r0]null[out]".to_string());
    } else {
        let row_inputs: String = (0..rows).map(|i| format!("[r{i}]")).collect();
        filter_parts.push(format!("{row_inputs}vstack={rows}[out]"));
    }

    (filter_parts.join(";"), cols, rows)
}

async fn run_sprite_sheet_ffmpeg(
    abs: &Path,
    times: &[f64],
    cache_path: &Path,
    filter: &str,
) -> anyhow::Result<std::process::Output> {
    let mut cmd = tokio::process::Command::new("nice");
    cmd.args(["-n", "10", "ffmpeg"]);
    for t in times {
        cmd.args(["-ss", &format!("{t:.2}"), "-i"]).arg(abs);
    }
    let out = cmd
        .args([
            "-filter_complex",
            filter,
            "-map",
            "[out]",
            "-frames:v",
            "1",
            "-q:v",
            "4",
            "-y",
        ])
        .arg(cache_path)
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await?;
    Ok(out)
}

fn looks_like_empty_output(stderr: &str) -> bool {
    stderr.contains("received no packets")
        || stderr.contains("Nothing was written into output file")
}

async fn seek_has_video_frame(abs: &Path, t: f64) -> bool {
    tokio::process::Command::new("nice")
        .args(["-n", "10", "ffmpeg", "-ss", &format!("{t:.2}"), "-i"])
        .arg(abs)
        .args(["-frames:v", "1", "-f", "null", "-", "-loglevel", "error"])
        .stderr(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .kill_on_drop(true)
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

async fn scene_positions(abs: &Path, n: usize, duration_secs: f64) -> anyhow::Result<Vec<f64>> {
    if n == 0 {
        return Ok(vec![]);
    }

    let out = tokio::process::Command::new("nice")
        .args(["-n", "10", "ffmpeg", "-i"])
        .arg(abs)
        .args([
            "-vf",
            "select='gt(scene,0.35)',showinfo",
            "-f",
            "null",
            "-",
            "-loglevel",
            "info",
        ])
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await?;

    if !out.status.success() {
        anyhow::bail!("ffmpeg scene detection failed");
    }

    let stderr = String::from_utf8_lossy(&out.stderr);
    let mut cuts: Vec<f64> = stderr.lines().filter_map(parse_pts_time).collect();
    cuts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    cuts.dedup_by(|a, b| (*a - *b).abs() < 0.001);

    // Build scene segments [0..cut1], [cut1..cut2], ... [last_cut..duration]
    // and sample at the midpoint of each segment so transition frames are avoided.
    let mut bounds: Vec<f64> = Vec::with_capacity(cuts.len() + 2);
    bounds.push(0.0);
    for c in cuts {
        if c > 0.0 && c < duration_secs {
            bounds.push(c);
        }
    }
    bounds.push(duration_secs);

    let mut mids: Vec<f64> = Vec::new();
    for w in bounds.windows(2) {
        let start = w[0];
        let end = w[1];
        if end > start {
            mids.push(start + (end - start) * 0.5);
        }
    }

    if mids.is_empty() {
        return Ok(vec![]);
    }

    Ok(pick_evenly(&mids, n))
}

/// Generate one or more AI analysis sprite sheets for `abs`.
///
/// Frames are sampled evenly across the full duration, then split into chunks
/// (`max_frames_per_sheet`) so each output image stays compact for VLM input.
/// Each sheet uses a near-square tile grid.
pub async fn generate_ai_sprites(
    abs: &Path,
    root: &Path,
    n: usize,
    duration_secs: f64,
    max_frames_per_sheet: usize,
    use_scene_select: bool,
) -> anyhow::Result<Vec<PathBuf>> {
    if n == 0 {
        anyhow::bail!("cannot generate AI sprites with zero frames");
    }

    let max_per_sheet = max_frames_per_sheet.clamp(1, 256);
    let positions = if use_scene_select {
        match scene_positions(abs, n, duration_secs).await {
            Ok(v) if !v.is_empty() => v,
            _ => interval_positions(n, duration_secs),
        }
    } else {
        interval_positions(n, duration_secs)
    };
    let mode_suffix = if use_scene_select {
        "scene"
    } else {
        "interval"
    };

    let mut out_paths = Vec::new();

    for (sheet_idx, chunk) in positions.chunks(max_per_sheet).enumerate() {
        let chunk_n = chunk.len();
        let (filter, cols, rows) = build_sprite_filter(chunk_n);

        let cache_path = file_cache_path(
            abs,
            root,
            "ai_sprites",
            &format!(
                "ai{n}_{mode_suffix}_s{:02}_{chunk_n}_{cols}x{rows}.jpg",
                sheet_idx + 1
            ),
        )
        .ok_or_else(|| {
            anyhow::anyhow!("cannot compute AI sprite cache path for {}", abs.display())
        })?;

        if cache_path.exists() {
            out_paths.push(cache_path);
            continue;
        }

        if let Some(parent) = cache_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let out = run_sprite_sheet_ffmpeg(abs, chunk, &cache_path, &filter).await?;

        if !out.status.success() || !cache_path.exists() {
            let stderr = String::from_utf8_lossy(&out.stderr);

            // Older/odd files can fail on specific seek positions. Retry with
            // only timestamps that actually return a frame.
            if looks_like_empty_output(&stderr) {
                let mut valid_times = Vec::new();
                for t in chunk {
                    if seek_has_video_frame(abs, *t).await {
                        valid_times.push(*t);
                    }
                }

                if !valid_times.is_empty() && valid_times.len() < chunk.len() {
                    let (retry_filter, _, _) = build_sprite_filter(valid_times.len());
                    let retry =
                        run_sprite_sheet_ffmpeg(abs, &valid_times, &cache_path, &retry_filter)
                            .await?;
                    if retry.status.success() && cache_path.exists() {
                        out_paths.push(cache_path);
                        continue;
                    }
                    let retry_stderr = String::from_utf8_lossy(&retry.stderr);
                    anyhow::bail!(
                        "ffmpeg sprite retry failed ({} of {} frames usable). first error: {} | retry error: {}",
                        valid_times.len(),
                        chunk.len(),
                        stderr.trim(),
                        retry_stderr.trim()
                    );
                }
            }

            anyhow::bail!(
                "ffmpeg could not generate AI sprite sheet (filter: {}): {}",
                filter,
                stderr.trim()
            );
        }

        out_paths.push(cache_path);
    }

    if out_paths.is_empty() {
        anyhow::bail!("ffmpeg could not generate AI sprite sheet — is ffmpeg installed?");
    }

    Ok(out_paths)
}

// ---------------------------------------------------------------------------
// Video trickplay thumbnails
// ---------------------------------------------------------------------------

/// Query params for `GET /api/vthumbs`.
#[derive(Deserialize)]
pub struct VThumbsParams {
    path: String,
    dir: Option<String>,
    #[serde(default)]
    n: Option<usize>,
    /// Client-configured max sprites (default 16). Lower = faster generation.
    #[serde(default)]
    max_n: Option<usize>,
    /// Client-configured min sprites (default 8).
    #[serde(default)]
    min_n: Option<usize>,
}

/// Return a horizontal sprite sheet (JPEG, N×1 grid) of evenly-spaced frames.
pub async fn api_vthumbs(
    Query(params): Query<VThumbsParams>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let db_root = match root_for_dir(
        &state,
        std::path::Path::new(params.dir.as_deref().unwrap_or("")),
    ) {
        Some(r) => r,
        None => return (StatusCode::BAD_REQUEST, "Unknown root or missing dir").into_response(),
    };

    if !load_features_for(&state, &db_root.root).video {
        return (StatusCode::NOT_IMPLEMENTED, "Video feature not enabled").into_response();
    }

    let (abs, cache_root) = match resolve_preview(&state, &db_root.root, &params.path) {
        Some(t) => t,
        None => return (StatusCode::BAD_REQUEST, "Invalid path").into_response(),
    };

    // When n is not explicit, video_info is needed to compute it from duration
    // (1 sprite per 30 s, min/max configurable, defaults 8/16).
    // Prefetch here so the cache filename is stable; the result is reused
    // below if the sprite needs to be built.
    let min_n = params.min_n.unwrap_or(8).clamp(2, 64);
    let max_n = params.max_n.unwrap_or(16).clamp(min_n, 64);
    let (n, prefetched_info) = if let Some(explicit) = params.n {
        (explicit.clamp(min_n, max_n), None)
    } else {
        let info = video_info(&abs).await;
        let dur = info.as_ref().map(|i| i.duration).unwrap_or(0.0);
        (sprites_for_duration(dur, min_n, max_n), info)
    };

    let cache_path =
        match file_cache_path(&abs, &cache_root, "vthumbs", &format!("sprite{n}x1.jpg")) {
            Some(p) => p,
            None => return (StatusCode::INTERNAL_SERVER_ERROR, "Cache path error").into_response(),
        };

    let cache_path = if !cache_path.exists() {
        // Use the dedicated vthumb semaphore (4 permits) so sprite generation
        // does not block the shared thumbnail queue (1 permit).
        let _permit = match VTHUMB_LIMITER.try_acquire() {
            Ok(p) => p,
            Err(_) => {
                return (StatusCode::SERVICE_UNAVAILABLE, "vthumb queue full").into_response();
            }
        };

        if cache_path.exists() {
            cache_path
        } else {
            let dur = if let Some(ref i) = prefetched_info {
                i.duration
            } else {
                match video_info(&abs).await {
                    Some(i) => i.duration,
                    None => {
                        return (
                            StatusCode::UNPROCESSABLE_ENTITY,
                            "Cannot read video metadata",
                        )
                            .into_response();
                    }
                }
            };

            match generate_sprite_cached(&abs, &cache_root, n, dur).await {
                Ok(p) => p,
                Err(_) => {
                    return (
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "Video trickplay unavailable — install ffmpeg",
                    )
                        .into_response();
                }
            }
        }
    } else {
        cache_path
    };

    match tokio::fs::read(&cache_path).await {
        Ok(data) => ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Read failed").into_response(),
    }
}

/// Number of trickplay sprites for a given video duration: 1 per 30 s,
/// clamped to [min_n, max_n].
pub fn sprites_for_duration(duration_secs: f64, min_n: usize, max_n: usize) -> usize {
    let n = (duration_secs / 30.0).round() as usize;
    n.clamp(min_n, max_n)
}

// ---------------------------------------------------------------------------
// Video trickplay pre-generation
// ---------------------------------------------------------------------------

/// Query params for `POST /api/vthumbs-pregen`.
#[derive(Deserialize)]
pub struct PregenParams {
    dir: Option<String>,
}

/// Request body for `POST /api/vthumbs-pregen`.
#[derive(Deserialize)]
pub struct PregenBody {
    paths: Vec<String>,
}

/// Generate trickplay sprites for a list of video paths in the background.
pub async fn api_vthumbs_pregen(
    Query(params): Query<PregenParams>,
    State(state): State<Arc<AppState>>,
    axum::extract::Json(body): axum::extract::Json<PregenBody>,
) -> Response {
    let db_root = match root_for_dir(
        &state,
        std::path::Path::new(params.dir.as_deref().unwrap_or("")),
    ) {
        Some(r) => r,
        None => return (StatusCode::BAD_REQUEST, "Unknown root or missing dir").into_response(),
    };

    if !load_features_for(&state, &db_root.root).video {
        return (StatusCode::NOT_IMPLEMENTED, "Video feature not enabled").into_response();
    }

    let root = db_root.root.clone();
    let queued = body.paths.len();
    let state_clone = state.clone();

    tokio::spawn(async move {
        for rel_path in body.paths {
            let (abs, cache_root) = match resolve_preview(&state_clone, &root, &rel_path) {
                Some(t) => t,
                None => continue,
            };
            // Determine n from duration before checking the cache, because n
            // determines the cache filename.
            let info = match video_info(&abs).await {
                Some(i) => i,
                None => continue,
            };
            let n = sprites_for_duration(info.duration, 8, 16);
            let cache_path =
                match file_cache_path(&abs, &cache_root, "vthumbs", &format!("sprite{n}x1.jpg")) {
                    Some(p) => p,
                    None => continue,
                };
            if cache_path.exists() {
                continue;
            }
            let _permit = VTHUMB_LIMITER.acquire().await;
            if cache_path.exists() {
                continue;
            }
            if let Some(parent) = cache_path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let positions: Vec<f64> = (0..n)
                .map(|i| info.duration * (i as f64 + 0.5) / n as f64)
                .collect();
            let mut cmd = tokio::process::Command::new("nice");
            cmd.args(["-n", "15", "ffmpeg"]);
            for t in &positions {
                cmd.args(["-ss", &format!("{t:.2}"), "-i"]).arg(&abs);
            }
            let scale_parts: Vec<String> = (0..n)
                .map(|i| format!("[{i}:v]scale=320:-2,setsar=1[f{i}]"))
                .collect();
            let hstack_inputs: String = (0..n).map(|i| format!("[f{i}]")).collect();
            let filter = format!("{};{hstack_inputs}hstack={n}[out]", scale_parts.join(";"));
            let _ = cmd
                .args([
                    "-filter_complex",
                    &filter,
                    "-map",
                    "[out]",
                    "-frames:v",
                    "1",
                    "-q:v",
                    "4",
                    "-y",
                ])
                .arg(&cache_path)
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .status()
                .await;
        }
    });

    Json(serde_json::json!({ "queued": queued })).into_response()
}
