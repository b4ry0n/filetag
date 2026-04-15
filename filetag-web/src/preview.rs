use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Json, Response},
};
use serde::Deserialize;

use crate::state::{AppState, THUMB_LIMITER, cache_root_for_file, preview_safe_path, root_at};
use crate::types::RootParam;

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
    let abs = match preview_safe_path(&db_root.root, &rel_path) {
        Some(p) => p,
        None => return (StatusCode::BAD_REQUEST, "Invalid path").into_response(),
    };
    let cache_root = cache_root_for_file(&state, &abs)
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| db_root.root.clone());

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

/// Transcode a video file to H.264/AAC mp4 via ffmpeg and stream it immediately
/// to the client as a fragmented mp4. The output is simultaneously written to a
/// cache file under `<root>/.filetag/cache/video/` so subsequent requests are
/// served instantly with full Range support.
async fn serve_transcoded_mp4(path: &Path, root: &Path, headers: &HeaderMap) -> Response {
    // Cache-key suffix "v4.mp4": regular (non-fragmented) MP4 with `-movflags
    // +faststart`. The complete `moov` atom (including total duration) sits at the
    // beginning of the file so browsers immediately know the total run-time and full
    // byte-range seeking works from the first request.
    //
    // When the source video stream is already H.264 (the common case for MKV films)
    // ffmpeg uses `-c:v copy` and only re-encodes the audio track if it is not AAC.
    // Remuxing is near-instant even for feature-length films.
    let cache_path = match file_cache_path(path, root, "video", "v4.mp4") {
        Some(p) => p,
        None => return serve_file_range(path, headers).await,
    };

    // If a cached transcode already exists, serve it directly with Range support.
    if cache_path.exists() {
        return serve_file_range(&cache_path, headers).await;
    }

    // Acquire the shared concurrency permit to avoid too many ffmpeg processes.
    let permit = match THUMB_LIMITER.acquire().await {
        Ok(p) => p,
        Err(_) => return (StatusCode::SERVICE_UNAVAILABLE, "transcode queue full").into_response(),
    };

    // Re-check after acquiring permit (another task may have finished it).
    if cache_path.exists() {
        drop(permit);
        return serve_file_range(&cache_path, headers).await;
    }

    let tmp = cache_path.with_extension("tmp.mp4");

    // Probe codecs; fall back to full transcode if ffprobe fails.
    let info = video_info(path).await;
    let c_video = info.as_ref().map(|i| i.video_arg()).unwrap_or("libx264");
    let c_audio = info.as_ref().map(|i| i.audio_arg()).unwrap_or("aac");

    // Build ffmpeg argument list. Extra quality flags are only added when
    // transcoding (copy mode ignores them but they cause no harm if present).
    let args: Vec<&str> = vec!["-n", "10", "ffmpeg", "-y", "-i"];
    // path inserted at call site below
    let mut extra: Vec<&str> = vec![
        "-c:v",
        c_video,
        "-c:a",
        c_audio,
        "-movflags",
        "+faststart",
        "-f",
        "mp4",
    ];
    if c_video != "copy" {
        extra.splice(2..2, ["-preset", "fast", "-crf", "23"]);
    }
    if c_audio != "copy" {
        extra.splice(extra.len() - 4..extra.len() - 4, ["-b:a", "128k"]);
    }
    // Suppress incompatible subtitle streams that ffmpeg cannot mux into mp4
    // (e.g. ASS/SSA from MKV). Using -sn is safe for all inputs.
    extra.push("-sn");
    let _ = &args; // consumed below via Command builder

    let status = tokio::process::Command::new("nice")
        .args(["-n", "10", "ffmpeg", "-y"])
        .arg("-i")
        .arg(path)
        .args(&extra)
        .arg(&tmp)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .status()
        .await;

    drop(permit);

    match status {
        Ok(s) if s.success() => {
            if tokio::fs::rename(&tmp, &cache_path).await.is_ok() {
                serve_file_range(&cache_path, headers).await
            } else {
                let _ = tokio::fs::remove_file(&tmp).await;
                serve_file_range(path, headers).await
            }
        }
        _ => {
            let _ = tokio::fs::remove_file(&tmp).await;
            serve_file_range(path, headers).await
        }
    }
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
    let root = db_root.root.clone();
    let abs = match preview_safe_path(&root, &params.path) {
        Some(p) => p,
        None => return (StatusCode::BAD_REQUEST, "Invalid path").into_response(),
    };
    let cache_root = cache_root_for_file(&state, &abs)
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| root.clone());

    let n = params.n.unwrap_or(8).clamp(2, 16);

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
            let info = match video_info(&abs).await {
                Some(i) => i,
                None => {
                    return (
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "Cannot read video metadata",
                    )
                        .into_response();
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
    let n = 8usize;
    let queued = body.paths.len();
    let state_clone = state.clone();

    tokio::spawn(async move {
        for rel_path in body.paths {
            let abs = match preview_safe_path(&root, &rel_path) {
                Some(p) => p,
                None => continue,
            };
            let cache_root = cache_root_for_file(&state_clone, &abs)
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| root.clone());
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
            let info = match video_info(&abs).await {
                Some(i) => i,
                None => continue,
            };
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
    /// Returns the ffmpeg `-c:v` argument: "copy" when the video is already H.264,
    /// otherwise a libx264 transcode is needed.
    pub fn video_arg(&self) -> &'static str {
        if self.video_codec == "h264" {
            "copy"
        } else {
            "libx264"
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

    // ffprobe csv output: one line per stream ("stream,<codec_type>,<codec_name>")
    // followed by one line for the format ("<duration>").
    let mut video_codec = String::new();
    let mut audio_codec = String::new();
    let mut duration = 0f64;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("stream,") {
            let mut parts = rest.splitn(2, ',');
            let kind = parts.next().unwrap_or("");
            let codec = parts.next().unwrap_or("");
            match kind {
                "video" if video_codec.is_empty() => video_codec = codec.to_owned(),
                "audio" if audio_codec.is_empty() => audio_codec = codec.to_owned(),
                _ => {}
            }
        } else if let Ok(d) = line.parse::<f64>()
            && d > 0.0
        {
            duration = d;
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
    let root = db_root.root.clone();
    let abs = match preview_safe_path(&root, &rel_path) {
        Some(p) => p,
        None => return (StatusCode::BAD_REQUEST, "Invalid path").into_response(),
    };
    // Use the most specific DB root that contains this file so cache files are
    // written to the correct `.filetag/cache/` directory even when a child
    // database root is nested under the requested root.
    let cache_root = cache_root_for_file(&state, &abs)
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| root.clone());

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

// ---------------------------------------------------------------------------
// HLS streaming (on-demand segmented transcoding)
// ---------------------------------------------------------------------------

/// Target duration for each HLS segment in seconds.
const HLS_SEG_DURATION: f64 = 6.0;

#[derive(Deserialize)]
pub struct HlsParams {
    pub root: Option<usize>,
    pub seg: Option<u32>,
}

/// Return the cache directory for HLS segments, keyed by source file mtime + size.
/// Stored under `<root>/.filetag/cache/hls/`.
fn hls_cache_dir(abs: &Path, root: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(abs).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let size = meta.len();
    let name = abs
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = root
        .join(".filetag")
        .join("cache")
        .join("hls")
        .join(format!("{mtime}_{size}_{name}"));
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Percent-encode a path string for use in an HLS playlist URL.
/// Preserves '/' separators and unreserved characters; encodes everything else.
fn percent_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// HLS handler: returns either a media playlist (`seg` absent) or a single MPEG-TS
/// segment (`seg=N`). Segments are transcoded on demand with per-file caching.
pub async fn hls_handler(
    AxumPath(rel_path): AxumPath<String>,
    Query(params): Query<HlsParams>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let db_root = match root_at(&state, params.root) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "Unknown root").into_response(),
    };
    let abs = match preview_safe_path(&db_root.root, &rel_path) {
        Some(p) => p,
        None => return (StatusCode::BAD_REQUEST, "Invalid path").into_response(),
    };
    if !abs.exists() {
        return (StatusCode::NOT_FOUND, "File not found").into_response();
    }

    if let Some(seg_idx) = params.seg {
        hls_segment(&abs, &db_root.root, seg_idx).await
    } else {
        hls_playlist(&abs, &rel_path, params.root).await
    }
}

/// Build and return an HLS VOD media playlist (M3U8) for the given file.
async fn hls_playlist(abs: &Path, rel_path: &str, root_idx: Option<usize>) -> Response {
    let info = match video_info(abs).await {
        Some(i) => i,
        None => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                "Cannot determine video duration",
            )
                .into_response();
        }
    };

    let n_segs = (info.duration / HLS_SEG_DURATION).ceil() as u32;
    // EXT-X-TARGETDURATION must be >= all EXTINF values (ceiling per HLS spec §4.3.3.1).
    let target_dur = (HLS_SEG_DURATION.ceil() as u32) + 1;

    let root_prefix = root_idx.map(|r| format!("root={r}&")).unwrap_or_default();
    let seg_url_base = format!("/hls/{}?{}seg=", percent_encode_path(rel_path), root_prefix,);

    let mut m3u8 = format!(
        "#EXTM3U\n\
         #EXT-X-VERSION:3\n\
         #EXT-X-TARGETDURATION:{target_dur}\n\
         #EXT-X-MEDIA-SEQUENCE:0\n\
         #EXT-X-PLAYLIST-TYPE:VOD\n",
    );

    for i in 0..n_segs {
        let start = i as f64 * HLS_SEG_DURATION;
        let seg_dur = (info.duration - start).min(HLS_SEG_DURATION);
        m3u8.push_str(&format!("#EXTINF:{seg_dur:.3},\n{seg_url_base}{i}\n"));
    }
    m3u8.push_str("#EXT-X-ENDLIST\n");

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from(m3u8))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Serve (and if necessary transcode) a single HLS MPEG-TS segment.
///
/// Video codec strategy for MPEG-TS:
/// - H.264: stream-copy (fast, universally supported in hls.js/MSE)
/// - Other: transcode to H.264 (MSE in Chrome/Firefox only supports H.264)
///
/// Audio: always AAC (copy if source is already AAC, else transcode).
async fn hls_segment(abs: &Path, root: &Path, seg_idx: u32) -> Response {
    let cache_dir = match hls_cache_dir(abs, root) {
        Some(d) => d,
        None => return (StatusCode::INTERNAL_SERVER_ERROR, "HLS cache error").into_response(),
    };
    let seg_path = cache_dir.join(format!("seg{seg_idx}.ts"));

    // Fast path: segment already cached.
    if seg_path.exists() {
        return serve_ts_file(&seg_path).await;
    }

    let permit = match THUMB_LIMITER.acquire().await {
        Ok(p) => p,
        Err(_) => return (StatusCode::SERVICE_UNAVAILABLE, "transcode queue full").into_response(),
    };

    // Re-check after acquiring permit (another task may have finished it).
    if seg_path.exists() {
        drop(permit);
        return serve_ts_file(&seg_path).await;
    }

    let start = seg_idx as f64 * HLS_SEG_DURATION;

    let info = video_info(abs).await;
    let c_video = info
        .as_ref()
        .map(|i| {
            if i.video_codec == "h264" {
                "copy"
            } else {
                "libx264"
            }
        })
        .unwrap_or("libx264");
    let c_audio = info
        .as_ref()
        .map(|i| {
            if i.audio_codec == "aac" {
                "copy"
            } else {
                "aac"
            }
        })
        .unwrap_or("aac");

    let tmp_seg = seg_path.with_extension("tmp.ts");

    let mut cmd = tokio::process::Command::new("nice");
    cmd.args(["-n", "10", "ffmpeg", "-y"])
        // Input seeking before -i: fast for copy mode, seeks to nearest keyframe.
        .args(["-ss", &format!("{start:.3}")])
        .arg("-i")
        .arg(abs)
        .args(["-t", &format!("{HLS_SEG_DURATION:.3}")])
        .args(["-c:v", c_video]);
    if c_video != "copy" {
        cmd.args(["-preset", "fast", "-crf", "23"]);
    }
    cmd.args(["-c:a", c_audio]);
    if c_audio != "copy" {
        cmd.args(["-b:a", "128k"]);
    }
    // -sn: drop subtitle streams (cannot mux into MPEG-TS without special handling)
    cmd.args(["-sn", "-f", "mpegts"])
        .arg(&tmp_seg)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);

    let status = cmd.status().await;
    drop(permit);

    match status {
        Ok(s) if s.success() => {
            if tokio::fs::rename(&tmp_seg, &seg_path).await.is_err() {
                let _ = tokio::fs::remove_file(&tmp_seg).await;
                return (StatusCode::INTERNAL_SERVER_ERROR, "Segment rename failed")
                    .into_response();
            }
            serve_ts_file(&seg_path).await
        }
        _ => {
            let _ = tokio::fs::remove_file(&tmp_seg).await;
            (StatusCode::UNPROCESSABLE_ENTITY, "Segment transcode failed").into_response()
        }
    }
}

async fn serve_ts_file(path: &Path) -> Response {
    match tokio::fs::read(path).await {
        Ok(data) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "video/mp2t")
            // Segments are immutable once written; cache aggressively.
            .header(header::CACHE_CONTROL, "max-age=31536000, immutable")
            .body(Body::from(data))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Segment read failed").into_response(),
    }
}
