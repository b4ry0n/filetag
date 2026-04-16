use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    body::{Body, Bytes},
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Json, Response},
};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::state::{AppState, THUMB_LIMITER, resolve_preview, root_at};
use crate::types::RootParam;

// ---------------------------------------------------------------------------
// Video cache eviction
// ---------------------------------------------------------------------------

/// Maximum total size (bytes) of the video transcode cache per database root.
/// When exceeded, the oldest cached files are removed until below this limit.
const VIDEO_CACHE_MAX_BYTES: u64 = 10 * 1024 * 1024 * 1024; // 10 GiB

/// Evict oldest video cache files until total size is below `max_bytes`.
async fn evict_video_cache(video_dir: PathBuf, max_bytes: u64) {
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
// File preview handler
// ---------------------------------------------------------------------------

/// Serve a file for preview, converting RAW / HEIC formats server-side.
pub async fn preview_handler(
    AxumPath(rel_path): AxumPath<String>,
    Query(rp): Query<RootParam>,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let db_root = match root_at(&state, rp.root) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "Unknown root").into_response(),
    };
    let (abs, cache_root) = match resolve_preview(&state, &db_root.root, &rel_path) {
        Some(t) => t,
        None => return (StatusCode::BAD_REQUEST, "Invalid path").into_response(),
    };

    let ext = abs
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "arw" | "cr2" | "cr3" | "nef" | "orf" | "rw2" | "dng" | "raf" | "pef" | "srw" | "raw"
        | "3fr" | "x3f" | "rwl" | "iiq" | "mef" | "mos" | "psd" | "psb" | "xcf" | "ai" | "eps" => {
            preview_raw(&abs, &cache_root).await
        }
        "heic" | "heif" => preview_heic(&abs, &cache_root).await,
        // Formats browsers cannot decode natively: transcode to mp4 via ffmpeg
        "avi" | "wmv" | "mkv" | "flv" | "mpg" | "mpeg" | "3gp" | "f4v" | "m4v" => {
            serve_transcoded_mp4(&abs, &cache_root, &headers).await
        }
        _ => serve_file_range(&abs, &headers).await,
    }
}

// ---------------------------------------------------------------------------
// MIME type mapping
// ---------------------------------------------------------------------------

pub fn mime_for_ext(ext: &str) -> &'static str {
    match ext {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "tiff" | "tif" => "image/tiff",
        "avif" => "image/avif",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        "mkv" => "video/x-matroska",
        "wmv" => "video/x-ms-wmv",
        "flv" => "video/x-flv",
        "ts" => "text/plain; charset=utf-8",
        "mpg" | "mpeg" => "video/mpeg",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        "opus" => "audio/opus",
        "wav" => "audio/wav",
        "aac" => "audio/aac",
        "m4a" => "audio/mp4",
        "pdf" => "application/pdf",
        "json" => "application/json",
        "xml" => "application/xml",
        "html" | "htm" => "text/html; charset=utf-8",
        _ => "text/plain; charset=utf-8",
    }
}

// ---------------------------------------------------------------------------
// File serving
// ---------------------------------------------------------------------------

pub async fn serve_file_bytes(path: &Path) -> Response {
    match tokio::fs::read(path).await {
        Ok(data) => {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            let ct = mime_for_ext(&ext);
            ([(header::CONTENT_TYPE, ct)], data).into_response()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (StatusCode::NOT_FOUND, "File not found").into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Serve a file with HTTP Range request support (required for video/audio seeking).
pub async fn serve_file_range(path: &Path, headers: &HeaderMap) -> Response {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    let meta = match tokio::fs::metadata(path).await {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (StatusCode::NOT_FOUND, "File not found").into_response();
        }
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let total = meta.len();

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let ct = mime_for_ext(&ext);

    if let Some(range_str) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) {
        let Some((start, end)) = parse_byte_range(range_str, total) else {
            return axum::http::Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{total}"))
                .body(Body::empty())
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
        };

        let length = end - start + 1;
        let mut file = match tokio::fs::File::open(path).await {
            Ok(f) => f,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        let mut buf = vec![0u8; length as usize];
        if file.read_exact(&mut buf).await.is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }

        return axum::http::Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_TYPE, ct)
            .header(header::ACCEPT_RANGES, "bytes")
            .header(
                header::CONTENT_RANGE,
                format!("bytes {start}-{end}/{total}"),
            )
            .header(header::CONTENT_LENGTH, length)
            .body(Body::from(buf))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    }

    // No Range header: return full file and advertise range support.
    match tokio::fs::read(path).await {
        Ok(data) => axum::http::Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, ct)
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_LENGTH, total)
            .body(Body::from(data))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (StatusCode::NOT_FOUND, "File not found").into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Parse a `bytes=<start>-[<end>]` range header value.
/// Returns `(start, end)` as inclusive byte offsets clamped to `total - 1`.
fn parse_byte_range(s: &str, total: u64) -> Option<(u64, u64)> {
    let s = s.strip_prefix("bytes=")?;
    let mut parts = s.splitn(2, '-');
    let start_str = parts.next()?;
    let end_str = parts.next().unwrap_or("");
    let start: u64 = start_str.parse().ok()?;
    let end: u64 = if end_str.is_empty() {
        total.saturating_sub(1)
    } else {
        end_str.parse().ok()?
    };
    if start >= total {
        return None;
    }
    Some((start, end.min(total - 1)))
}

// ---------------------------------------------------------------------------
// Video transcoding
// ---------------------------------------------------------------------------

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
/// is unavailable on the first play; from cache it works via Range requests.
async fn serve_transcoded_mp4(path: &Path, root: &Path, headers: &HeaderMap) -> Response {
    let cache_path = match file_cache_path(path, root, "video", "v7.mp4") {
        Some(p) => p,
        None => return serve_file_range(path, headers).await,
    };

    // Cached copy exists: serve with full Range/seek support.
    if cache_path.exists() {
        return serve_file_range(&cache_path, headers).await;
    }

    // Acquire concurrency permit.
    let permit = match THUMB_LIMITER.acquire().await {
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
// Cache path helpers
// ---------------------------------------------------------------------------

/// Return a cache path for a derived preview file, keyed by mtime + size.
/// Files are stored under `<root>/.filetag/cache/<subdir>/`.
pub fn file_cache_path(abs: &Path, root: &Path, subdir: &str, suffix: &str) -> Option<PathBuf> {
    let meta = std::fs::metadata(abs).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let size = meta.len();
    let stem = abs
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let key = format!("{mtime}_{size}_{stem}.{suffix}");
    let dir = root.join(".filetag").join("cache").join(subdir);
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(key))
}

/// Return the cache path for a RAW preview JPEG, keyed by mtime + size.
/// Stored in `<root>/.filetag/cache/raw/`.
pub fn raw_cache_path(abs: &Path, root: &Path) -> Option<PathBuf> {
    file_cache_path(abs, root, "raw", "prev.jpg")
}

/// Return the cache path for a file thumbnail, keyed by mtime + size.
/// Stored in `<root>/.filetag/cache/thumbs/`.
pub fn thumb_cache_path(abs: &Path, root: &Path) -> Option<PathBuf> {
    file_cache_path(abs, root, "thumbs", "thumb.jpg")
}

// ---------------------------------------------------------------------------
// RAW / PSD / layered format preview
// ---------------------------------------------------------------------------

/// Try to extract a JPEG preview from a RAW file using available tools.
/// Attempt order: dcraw -e -c → exiftool → ffmpeg → ImageMagick.
/// Result is cached in <root>/.filetag/cache/raw/ keyed by mtime+size.
async fn preview_raw(path: &Path, root: &Path) -> Response {
    if let Some(cache) = raw_cache_path(path, root) {
        if let Ok(data) = tokio::fs::read(&cache).await {
            return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
        }

        let jpeg = raw_extract_jpeg(path).await;
        if let Some(data) = jpeg {
            let _ = tokio::fs::write(&cache, &data).await;
            return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
        }
    } else if let Some(data) = raw_extract_jpeg(path).await {
        return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
    }

    (
        StatusCode::UNPROCESSABLE_ENTITY,
        "RAW preview unavailable — install dcraw, exiftool, ffmpeg, or ImageMagick",
    )
        .into_response()
}

/// Inner extraction logic for `preview_raw`: tries tools in order and returns
/// the first JPEG bytes found, or `None` if all tools fail.
pub async fn raw_extract_jpeg(path: &Path) -> Option<Vec<u8>> {
    // dcraw: extract embedded thumbnail to stdout
    if let Ok(out) = tokio::process::Command::new("dcraw")
        .args(["-e", "-c"])
        .arg(path)
        .kill_on_drop(true)
        .output()
        .await
        && out.status.success()
        && out.stdout.starts_with(&[0xFF, 0xD8])
    {
        return Some(out.stdout);
    }

    // exiftool: extract PreviewImage or ThumbnailImage
    for tag in &["-PreviewImage", "-ThumbnailImage", "-JpgFromRaw"] {
        if let Ok(out) = tokio::process::Command::new("exiftool")
            .args(["-b", tag])
            .arg(path)
            .kill_on_drop(true)
            .output()
            .await
            && out.status.success()
            && out.stdout.starts_with(&[0xFF, 0xD8])
        {
            return Some(out.stdout);
        }
    }

    // ffmpeg: decode first frame to JPEG
    if let Ok(out) = tokio::process::Command::new("nice")
        .args(["-n", "10", "ffmpeg"])
        .arg("-i")
        .arg(path)
        .args([
            "-vframes",
            "1",
            "-f",
            "image2pipe",
            "-vcodec",
            "mjpeg",
            "pipe:1",
        ])
        .kill_on_drop(true)
        .output()
        .await
        && out.status.success()
        && !out.stdout.is_empty()
    {
        return Some(out.stdout);
    }

    // ImageMagick 7 (magick) or 6 (convert): composite/layered formats
    let path_layer = format!("{}[0]", path.display());
    for cmd in &["magick", "convert"] {
        if let Ok(out) = tokio::process::Command::new(cmd)
            .arg(&path_layer)
            .args(["-flatten", "-quality", "85", "jpg:-"])
            .kill_on_drop(true)
            .output()
            .await
            && out.status.success()
            && out.stdout.starts_with(&[0xFF, 0xD8])
        {
            return Some(out.stdout);
        }
    }

    None
}

// ---------------------------------------------------------------------------
// HEIC/HEIF preview
// ---------------------------------------------------------------------------

/// Convert HEIC/HEIF to JPEG for browser display.
/// Attempt order: sips (macOS) → ffmpeg → ImageMagick convert
pub async fn preview_heic(path: &Path, root: &Path) -> Response {
    let cache_dir = root.join(".filetag").join("cache");
    let _ = std::fs::create_dir_all(&cache_dir);
    let tmp = cache_dir.join(format!(
        "heic_{}_{}.jpg",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        std::fs::metadata(path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0),
    ));

    if tmp.exists()
        && let Ok(data) = tokio::fs::read(&tmp).await
    {
        return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
    }

    // sips (macOS built-in)
    if let Ok(out) = tokio::process::Command::new("sips")
        .args(["-s", "format", "jpeg", "-Z", "1600"])
        .arg(path)
        .arg("--out")
        .arg(&tmp)
        .kill_on_drop(true)
        .output()
        .await
        && out.status.success()
        && let Ok(data) = tokio::fs::read(&tmp).await
    {
        return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
    }

    // ffmpeg
    if let Ok(out) = tokio::process::Command::new("nice")
        .args(["-n", "10", "ffmpeg"])
        .arg("-i")
        .arg(path)
        .args([
            "-vframes",
            "1",
            "-f",
            "image2pipe",
            "-vcodec",
            "mjpeg",
            "pipe:1",
        ])
        .kill_on_drop(true)
        .output()
        .await
        && out.status.success()
        && !out.stdout.is_empty()
    {
        let _ = tokio::fs::write(&tmp, &out.stdout).await;
        return ([(header::CONTENT_TYPE, "image/jpeg")], out.stdout).into_response();
    }

    // ImageMagick convert
    if let Ok(out) = tokio::process::Command::new("convert")
        .arg(path)
        .args(["-auto-orient"])
        .arg(&tmp)
        .kill_on_drop(true)
        .output()
        .await
        && out.status.success()
        && let Ok(data) = tokio::fs::read(&tmp).await
    {
        return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
    }

    (
        StatusCode::UNPROCESSABLE_ENTITY,
        "HEIC preview unavailable — install sips (macOS), ffmpeg, or ImageMagick",
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Image thumbnail (resize to max 400 px)
// ---------------------------------------------------------------------------

/// Generate a small JPEG thumbnail for any image file.
/// Target: max 400 px on the longest side, quality 80.
pub async fn image_thumb_jpeg(path: &Path) -> Option<Vec<u8>> {
    let path_layer = format!("{}[0]", path.display());
    for cmd in &["magick", "convert"] {
        if let Ok(out) = tokio::process::Command::new(cmd)
            .arg(&path_layer)
            .args([
                "-auto-orient",
                "-strip",
                "-resize",
                "400x400>",
                "-quality",
                "80",
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
        .args(["-n", "10", "ffmpeg"])
        .args(["-i"])
        .arg(path)
        .args([
            "-vf",
            "scale='if(gt(iw,ih),400,-2)':'if(gt(iw,ih),-2,400)':flags=lanczos",
            "-vframes",
            "1",
            "-map_metadata",
            "-1",
            "-f",
            "image2pipe",
            "-vcodec",
            "mjpeg",
            "-q:v",
            "5",
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

    None
}

// ---------------------------------------------------------------------------
// PDF thumbnail
// ---------------------------------------------------------------------------

/// Generate a JPEG thumbnail for a PDF by rasterising the first page.
/// Tries pdftoppm first (poppler-utils), then ImageMagick+Ghostscript.
/// Temp files are written under `<root>/.filetag/tmp/` per data-isolation rules.
pub async fn pdf_thumb_jpeg(path: &Path, root: &Path) -> Option<Vec<u8>> {
    let tmp_dir = root.join(".filetag").join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp_prefix = tmp_dir.join(format!("pdft_{}", stem));
    let expected = tmp_dir.join(format!("pdft_{}.jpg", stem));

    let status = tokio::process::Command::new("pdftoppm")
        .args([
            "-jpeg",
            "-singlefile",
            "-f",
            "1",
            "-l",
            "1",
            "-scale-to",
            "400",
        ])
        .arg(path)
        .arg(&tmp_prefix)
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .status()
        .await;

    if status.map(|s| s.success()).unwrap_or(false)
        && let Ok(data) = tokio::fs::read(&expected).await
    {
        let _ = tokio::fs::remove_file(&expected).await;
        if data.starts_with(&[0xFF, 0xD8]) {
            return Some(data);
        }
    }
    let _ = tokio::fs::remove_file(&expected).await;

    // Fallback: ImageMagick (requires Ghostscript for PDF rasterisation)
    image_thumb_jpeg(path).await
}

// ---------------------------------------------------------------------------
// Video trickplay thumbnails
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct VThumbsParams {
    path: String,
    root: Option<usize>,
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
    let db_root = match root_at(&state, params.root) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "Unknown root").into_response(),
    };
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

    if !cache_path.exists() {
        let _permit = match THUMB_LIMITER.try_acquire() {
            Ok(p) => p,
            Err(_) => {
                return (StatusCode::SERVICE_UNAVAILABLE, "thumbnail queue full").into_response();
            }
        };

        if !cache_path.exists() {
            let info = if let Some(i) = prefetched_info {
                i
            } else {
                match video_info(&abs).await {
                    Some(i) => i,
                    None => {
                        return (
                            StatusCode::UNPROCESSABLE_ENTITY,
                            "Cannot read video metadata",
                        )
                            .into_response();
                    }
                }
            };

            if let Some(parent) = cache_path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }

            let positions: Vec<f64> = (0..n)
                .map(|i| info.duration * (i as f64 + 0.5) / n as f64)
                .collect();

            let mut cmd = tokio::process::Command::new("nice");
            cmd.args(["-n", "10", "ffmpeg"]);
            for t in &positions {
                cmd.args(["-ss", &format!("{t:.2}"), "-i"]).arg(&abs);
            }

            let scale_parts: Vec<String> = (0..n)
                .map(|i| format!("[{i}:v]scale=320:-2,setsar=1[f{i}]"))
                .collect();
            let hstack_inputs: String = (0..n).map(|i| format!("[f{i}]")).collect();
            let filter = format!("{};{hstack_inputs}hstack={n}[out]", scale_parts.join(";"));

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

            if !ok {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Video trickplay unavailable — install ffmpeg",
                )
                    .into_response();
            }
        }
    }

    match tokio::fs::read(&cache_path).await {
        Ok(data) => ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Read failed").into_response(),
    }
}

/// Number of trickplay sprites for a given video duration: 1 per 30 s,
/// clamped to [min_n, max_n].
fn sprites_for_duration(duration_secs: f64, min_n: usize, max_n: usize) -> usize {
    let n = (duration_secs / 30.0).round() as usize;
    n.clamp(min_n, max_n)
}

// ---------------------------------------------------------------------------
// Video trickplay pre-generation
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct PregenParams {
    root: Option<usize>,
}

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
    let db_root = match root_at(&state, params.root) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "Unknown root").into_response(),
    };
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
            let _permit = THUMB_LIMITER.acquire().await;
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

// ---------------------------------------------------------------------------
// Video info + contact-sheet thumbnail
// ---------------------------------------------------------------------------

pub struct VideoInfo {
    pub duration: f64,
    /// Codec name of the first video stream as reported by ffprobe (e.g. "h264", "hevc").
    pub video_codec: String,
    /// Codec name of the first audio stream as reported by ffprobe (e.g. "aac", "ac3").
    pub audio_codec: String,
}

impl VideoInfo {
    /// Returns the ffmpeg `-c:v` argument: "copy" when the codec can be placed
    /// directly in an MP4 container (H.264, HEVC, MPEG-4, VP9).
    pub fn video_arg(&self) -> &'static str {
        match self.video_codec.as_str() {
            "h264" | "hevc" | "mpeg4" | "vp9" | "av1" => "copy",
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
// Thumb handler (main thumbnail endpoint)
// ---------------------------------------------------------------------------

/// Thumbnail endpoint — generates a JPEG thumbnail for any previewable file.
pub async fn thumb_handler(
    AxumPath(rel_path): AxumPath<String>,
    Query(rp): Query<RootParam>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let db_root = match root_at(&state, rp.root) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "Unknown root").into_response(),
    };
    let (abs, cache_root) = match resolve_preview(&state, &db_root.root, &rel_path) {
        Some(t) => t,
        None => return (StatusCode::BAD_REQUEST, "Invalid path").into_response(),
    };

    let ext = abs
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        // ZIP/CBZ/RAR/CBR/7z/CB7: thumbnail = first image page, resized
        "zip" | "cbz" | "rar" | "cbr" | "7z" | "cb7" => {
            thumb_cached(&abs, &cache_root, |abs| {
                Box::pin(async move {
                    let abs2 = abs.to_path_buf();
                    let result = tokio::task::spawn_blocking(move || {
                        crate::archive::archive_cover_image(&abs2)
                    })
                    .await;
                    if let Ok(Ok(img_bytes)) = result {
                        thumb_from_raw_bytes(&img_bytes, abs).await
                    } else {
                        None
                    }
                })
            })
            .await
        }

        // Video: single-frame thumbnail
        "mp4" | "webm" | "mov" | "avi" | "mkv" | "wmv" | "flv" | "m4v" | "ts" | "3gp" | "f4v"
        | "mpg" | "mpeg" | "m2v" | "m2ts" | "mts" | "mxf" | "rm" | "rmvb" | "divx" | "vob"
        | "ogv" | "ogg" | "dv" | "asf" | "amv" | "mpe" | "m1v" | "mpv" | "qt" => {
            video_thumb_strip(&abs, &cache_root).await
        }

        // HEIC/HEIF
        "heic" | "heif" => {
            thumb_cached(&abs, &cache_root, |abs| {
                Box::pin(async move { image_thumb_jpeg(abs).await })
            })
            .await
        }

        // RAW / PSD / layered
        "arw" | "cr2" | "cr3" | "nef" | "orf" | "rw2" | "dng" | "raf" | "pef" | "srw" | "raw"
        | "3fr" | "x3f" | "rwl" | "iiq" | "mef" | "mos" | "psd" | "psb" | "xcf" | "ai" | "eps" => {
            thumb_cached(&abs, &cache_root, |abs| {
                Box::pin(async move {
                    if let Some(full_jpeg) = raw_extract_jpeg(abs).await {
                        thumb_from_raw_bytes(&full_jpeg, abs).await
                    } else {
                        None
                    }
                })
            })
            .await
        }

        // PDF
        "pdf" => {
            thumb_cached(&abs, &cache_root, |abs| {
                Box::pin(async move { pdf_thumb_jpeg(abs, abs.parent().unwrap_or(abs)).await })
            })
            .await
        }

        // Regular images (JPEG, PNG, WEBP, …)
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tiff" | "tif" | "avif" => {
            if let Some(cache) = thumb_cache_path(&abs, &cache_root) {
                if let Ok(data) = tokio::fs::read(&cache).await {
                    return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
                }
                let _permit = match THUMB_LIMITER.try_acquire() {
                    Ok(p) => p,
                    Err(_) => {
                        return (StatusCode::SERVICE_UNAVAILABLE, "thumbnail queue full")
                            .into_response();
                    }
                };
                if let Some(data) = image_thumb_jpeg(&abs).await {
                    let _ = tokio::fs::write(&cache, &data).await;
                    return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
                }
            }
            // Cache unavailable or tool missing: serve the original
            serve_file_bytes(&abs).await
        }

        // Everything else: fall through to preview handler
        _ => {
            preview_handler(
                AxumPath(rel_path),
                Query(rp),
                State(state),
                HeaderMap::new(),
            )
            .await
        }
    }
}

// ---------------------------------------------------------------------------
// Cached thumbnail helper (deduplicates the cache-check-generate-serve pattern)
// ---------------------------------------------------------------------------

use std::future::Future;
use std::pin::Pin;

/// General-purpose cached thumbnail: check cache, acquire permit, run `generate`
/// callback, write cache, serve JPEG.
async fn thumb_cached<F>(abs: &Path, root: &Path, generate: F) -> Response
where
    F: FnOnce(&Path) -> Pin<Box<dyn Future<Output = Option<Vec<u8>>> + Send + '_>>,
{
    if let Some(cache) = thumb_cache_path(abs, root) {
        if let Ok(data) = tokio::fs::read(&cache).await {
            return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
        }
        let _permit = match THUMB_LIMITER.try_acquire() {
            Ok(p) => p,
            Err(_) => {
                return (StatusCode::SERVICE_UNAVAILABLE, "thumbnail queue full").into_response();
            }
        };
        if let Some(data) = generate(abs).await {
            let _ = tokio::fs::write(&cache, &data).await;
            return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
        }
    }
    (StatusCode::UNPROCESSABLE_ENTITY, "Thumbnail unavailable").into_response()
}

/// Convert raw image bytes (e.g. from an archive or RAW extraction) into a
/// thumbnail JPEG by writing to a temp file and calling `image_thumb_jpeg`.
/// Falls back to the raw bytes if resizing fails.
async fn thumb_from_raw_bytes(raw_bytes: &[u8], abs: &Path) -> Option<Vec<u8>> {
    let root = abs.parent()?;
    let tmp_dir = root.join(".filetag").join("tmp");
    let _ = tokio::fs::create_dir_all(&tmp_dir).await;
    let tmp = tmp_dir.join("thumb_src.jpg");
    if tokio::fs::write(&tmp, raw_bytes).await.is_ok() {
        if let Some(small) = image_thumb_jpeg(&tmp).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Some(small);
        }
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    // Fallback: return the raw bytes unchanged
    Some(raw_bytes.to_vec())
}
