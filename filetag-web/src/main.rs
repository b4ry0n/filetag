use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use axum::{
    Router,
    extract::{Path as AxumPath, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
};
use clap::Parser;
use filetag_lib::{db, query};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "filetag-web", about = "Web interface for filetag", version)]
struct Args {
    /// Database root directory (default: current directory)
    path: Option<PathBuf>,

    /// Port to listen on
    #[arg(short, long, default_value_t = 3000)]
    port: u16,

    /// Address to bind to
    #[arg(short, long, default_value = "127.0.0.1")]
    bind: String,
}

// ---------------------------------------------------------------------------
// State and error handling
// ---------------------------------------------------------------------------

struct AppState {
    db_path: PathBuf,
    root: PathBuf,
}

struct AppError(anyhow::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.0.to_string() });
        (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
    }
}

impl From<anyhow::Error> for AppError {
    fn from(err: anyhow::Error) -> Self {
        Self(err)
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(err: rusqlite::Error) -> Self {
        Self(err.into())
    }
}

fn open_conn(state: &AppState) -> anyhow::Result<Connection> {
    let conn = Connection::open(&state.db_path).context("opening database")?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )?;
    Ok(conn)
}

/// Resolve a relative path under `root`, rejecting directory traversal.
fn safe_path(root: &Path, rel: &str) -> anyhow::Result<PathBuf> {
    let joined = root.join(rel);
    let canonical = std::fs::canonicalize(&joined)
        .with_context(|| format!("resolving {}", joined.display()))?;
    anyhow::ensure!(canonical.starts_with(root), "path escapes database root");
    Ok(canonical)
}

// ---------------------------------------------------------------------------
// API types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ApiInfo {
    root: String,
    files: i64,
    tags: i64,
    assignments: i64,
    total_size: i64,
}

#[derive(Serialize)]
struct ApiTag {
    name: String,
    count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<String>,
}

#[derive(Serialize)]
struct ApiDirListing {
    path: String,
    entries: Vec<ApiDirEntry>,
}

#[derive(Serialize)]
struct ApiDirEntry {
    name: String,
    is_dir: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mtime: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tag_count: Option<i64>,
}

#[derive(Serialize)]
struct ApiFileDetail {
    path: String,
    size: i64,
    file_id: Option<String>,
    mtime: i64,
    indexed_at: String,
    tags: Vec<ApiFileTag>,
}

#[derive(Serialize)]
struct ApiFileTag {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<String>,
}

#[derive(Serialize)]
struct ApiSearchResult {
    query: String,
    results: Vec<ApiSearchEntry>,
}

#[derive(Serialize)]
struct ApiSearchEntry {
    path: String,
    tags: Vec<ApiFileTag>,
}

#[derive(Deserialize)]
struct FileListParams {
    #[serde(default)]
    path: String,
    #[serde(default)]
    show_hidden: bool,
}

#[derive(Deserialize)]
struct SearchParams {
    q: String,
}

#[derive(Deserialize)]
struct FileDetailParams {
    path: String,
}

#[derive(Deserialize)]
struct TagRequest {
    path: String,
    tags: Vec<String>,
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// File preview handler
// ---------------------------------------------------------------------------

/// Serve a file for preview, converting RAW / HEIC formats server-side.
async fn preview_handler(
    AxumPath(rel_path): AxumPath<String>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let abs = match preview_safe_path(&state.root, &rel_path) {
        Some(p) => p,
        None => return (StatusCode::BAD_REQUEST, "Invalid path").into_response(),
    };

    let ext = abs
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "arw" | "cr2" | "cr3" | "nef" | "orf" | "rw2" | "dng" | "raf" | "pef" | "srw"
        | "raw" | "3fr" | "x3f" | "rwl" | "iiq" | "mef" | "mos"
        | "psd" | "psb" | "xcf" | "ai" | "eps" => {
            preview_raw(&abs, &state.root).await
        }
        "heic" | "heif" => preview_heic(&abs, &state.root).await,
        _ => serve_file_bytes(&abs).await,
    }
}

/// Sanitise a URL path component so it cannot escape `root`.
/// Unlike `safe_path`, this does not require the file to exist first.
fn preview_safe_path(root: &Path, rel: &str) -> Option<PathBuf> {
    use std::path::Component;
    let mut result = root.to_path_buf();
    for component in std::path::Path::new(rel.trim_start_matches('/')).components() {
        match component {
            Component::Normal(name) => result.push(name),
            Component::CurDir => {}
            _ => return None,
        }
    }
    // Re-canonicalise to catch symlinks that escape root
    match std::fs::canonicalize(&result) {
        Ok(canonical) if canonical.starts_with(root) => Some(canonical),
        Ok(_) => None,
        // File may not exist yet (e.g. wrong path) – just reject
        Err(_) => None,
    }
}

fn mime_for_ext(ext: &str) -> &'static str {
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
        "ts" => "video/mp2t",
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

async fn serve_file_bytes(path: &Path) -> Response {
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

/// Return the cache path for a RAW preview JPEG, keyed by mtime + size.
/// Stored in <root>/.filetag/cache/raw/.
fn raw_cache_path(abs: &Path, root: &Path) -> Option<PathBuf> {
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
    let key = format!("{mtime}_{size}_{stem}.prev.jpg");
    let dir = root.join(".filetag").join("cache").join("raw");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(key))
}

/// Try to extract a JPEG preview from a RAW file using available tools.
/// Attempt order: dcraw -e -c → exiftool → ffmpeg → ImageMagick.
/// Result is cached in <root>/.filetag/cache/raw/ keyed by mtime+size.
async fn preview_raw(path: &Path, root: &Path) -> Response {
    // Serve from cache if available
    if let Some(cache) = raw_cache_path(path, root) {
        if let Ok(data) = tokio::fs::read(&cache).await {
            return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
        }

        // Cache miss: run the tool chain, then persist the result
        let jpeg = raw_extract_jpeg(path).await;
        if let Some(data) = jpeg {
            let _ = tokio::fs::write(&cache, &data).await;
            return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
        }
    } else {
        // Could not determine cache path (e.g. no metadata); try without cache
        if let Some(data) = raw_extract_jpeg(path).await {
            return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
        }
    }

    (
        StatusCode::UNPROCESSABLE_ENTITY,
        "RAW preview unavailable — install dcraw, exiftool, ffmpeg, or ImageMagick",
    )
        .into_response()
}

/// Inner extraction logic for `preview_raw`: tries tools in order and returns
/// the first JPEG bytes found, or `None` if all tools fail.
async fn raw_extract_jpeg(path: &Path) -> Option<Vec<u8>> {
    // dcraw: extract embedded thumbnail to stdout
    if let Ok(out) = tokio::process::Command::new("dcraw")
        .args(["-e", "-c"])
        .arg(path)
        .output()
        .await
    {
        if out.status.success() && out.stdout.starts_with(&[0xFF, 0xD8]) {
            return Some(out.stdout);
        }
    }

    // exiftool: extract PreviewImage or ThumbnailImage
    for tag in &["-PreviewImage", "-ThumbnailImage", "-JpgFromRaw"] {
        if let Ok(out) = tokio::process::Command::new("exiftool")
            .args(["-b", tag])
            .arg(path)
            .output()
            .await
        {
            if out.status.success() && out.stdout.starts_with(&[0xFF, 0xD8]) {
                return Some(out.stdout);
            }
        }
    }

    // ffmpeg: decode first frame to JPEG
    if let Ok(out) = tokio::process::Command::new("ffmpeg")
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
        .output()
        .await
    {
        if out.status.success() && !out.stdout.is_empty() {
            return Some(out.stdout);
        }
    }

    // ImageMagick 7 (magick) or 6 (convert): composite/layered formats
    let path_layer = format!("{}[0]", path.display());
    for cmd in &["magick", "convert"] {
        if let Ok(out) = tokio::process::Command::new(cmd)
            .arg(&path_layer)
            .args(["-flatten", "-quality", "85", "jpg:-"])
            .output()
            .await
        {
            if out.status.success() && out.stdout.starts_with(&[0xFF, 0xD8]) {
                return Some(out.stdout);
            }
        }
    }

    None
}

/// Convert HEIC/HEIF to JPEG for browser display.
/// Attempt order: sips (macOS) → ffmpeg → ImageMagick convert
async fn preview_heic(path: &Path, root: &Path) -> Response {
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

    // Serve from cache if fresh
    if tmp.exists() {
        if let Ok(data) = tokio::fs::read(&tmp).await {
            return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
        }
    }

    // sips (macOS built-in)
    if let Ok(out) = tokio::process::Command::new("sips")
        .args(["-s", "format", "jpeg", "-Z", "1600"])
        .arg(path)
        .arg("--out")
        .arg(&tmp)
        .output()
        .await
    {
        if out.status.success() {
            if let Ok(data) = tokio::fs::read(&tmp).await {
                return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
            }
        }
    }

    // ffmpeg
    if let Ok(out) = tokio::process::Command::new("ffmpeg")
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
        .output()
        .await
    {
        if out.status.success() && !out.stdout.is_empty() {
            let _ = tokio::fs::write(&tmp, &out.stdout).await;
            return ([(header::CONTENT_TYPE, "image/jpeg")], out.stdout).into_response();
        }
    }

    // ImageMagick convert (with -auto-orient to respect EXIF orientation)
    if let Ok(out) = tokio::process::Command::new("convert")
        .arg(path)
        .args(["-auto-orient"])
        .arg(&tmp)
        .output()
        .await
    {
        if out.status.success() {
            if let Ok(data) = tokio::fs::read(&tmp).await {
                return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
            }
        }
    }

    (
        StatusCode::UNPROCESSABLE_ENTITY,
        "HEIC preview unavailable — install sips (macOS), ffmpeg, or ImageMagick",
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Image thumbnail (resize to max 400 px wide/tall via ffmpeg or sips/magick)
// ---------------------------------------------------------------------------

/// Generate a small JPEG thumbnail for any image file (JPEG, PNG, GIF, WEBP,
/// TIFF, …). Returns raw JPEG bytes or `None` if all tools fail.
/// Target: max 400 px on the longest side, quality 80.
/// ImageMagick is tried first because its `-auto-orient` flag reliably applies
/// EXIF orientation. ffmpeg is used as a fallback (it applies rotation in most
/// modern versions but is not guaranteed for all EXIF Orientation values).
async fn image_thumb_jpeg(path: &Path) -> Option<Vec<u8>> {
    // ImageMagick 7 (magick) or 6 (convert):
    // -auto-orient reads the EXIF Orientation tag and physically rotates the
    // image before resizing, so the thumbnail always has the correct orientation.
    let path_layer = format!("{}[0]", path.display());
    for cmd in &["magick", "convert"] {
        if let Ok(out) = tokio::process::Command::new(cmd)
            .arg(&path_layer)
            .args(["-auto-orient", "-resize", "400x400>", "-quality", "80", "jpg:-"])
            .stderr(std::process::Stdio::null())
            .output()
            .await
        {
            if out.status.success() && out.stdout.starts_with(&[0xFF, 0xD8]) {
                return Some(out.stdout);
            }
        }
    }

    // ffmpeg fallback: scale to fit 400×400, pipe JPEG to stdout.
    // Modern ffmpeg (4.x+) applies EXIF rotation automatically for most cases.
    if let Ok(out) = tokio::process::Command::new("ffmpeg")
        .args(["-i"])
        .arg(path)
        .args([
            "-vf",
            "scale='if(gt(iw,ih),400,-2)':'if(gt(iw,ih),-2,400)':flags=lanczos",
            "-vframes", "1",
            "-f", "image2pipe",
            "-vcodec", "mjpeg",
            "-q:v", "5",
            "pipe:1",
        ])
        .stderr(std::process::Stdio::null())
        .output()
        .await
    {
        if out.status.success() && out.stdout.starts_with(&[0xFF, 0xD8]) {
            return Some(out.stdout);
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Video thumbnail strip (2×2 contact sheet via ffmpeg)
// ---------------------------------------------------------------------------

/// Return a cache path for this file's thumbnail, keyed by mtime + size.
/// All cache files are stored under <root>/.filetag/cache/thumbs/.
fn thumb_cache_path(abs: &Path, root: &Path) -> Option<PathBuf> {
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
    let key = format!("{mtime}_{size}_{stem}.thumb.jpg");
    let dir = root.join(".filetag").join("cache").join("thumbs");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(key))
}

/// Get video duration in seconds via ffprobe.
async fn video_duration(path: &Path) -> Option<f64> {
    let out = tokio::process::Command::new("ffprobe")
        .args([
            "-v", "error",
            "-show_entries", "format=duration",
            "-of", "csv=p=0",
        ])
        .arg(path)
        .output()
        .await
        .ok()?;
    if out.status.success() {
        std::str::from_utf8(&out.stdout)
            .ok()?
            .trim()
            .parse::<f64>()
            .ok()
    } else {
        None
    }
}

/// Generate a 2×2 JPEG contact-sheet thumbnail for a video file.
/// Uses ffmpeg fast-seek (-ss before -i) for each of 4 frames.
async fn video_thumb_strip(path: &Path, root: &Path) -> Response {
    // Serve from cache if available
    if let Some(cache) = thumb_cache_path(path, root) {
        if let Ok(data) = tokio::fs::read(&cache).await {
            return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
        }

        // Determine seek positions
        let dur = video_duration(path).await;
        let positions: [f64; 4] = match dur {
            Some(d) if d > 0.5 => [d * 0.08, d * 0.33, d * 0.62, d * 0.87],
            // Fallback: fixed offsets; ffmpeg will just grab what's available
            _ => [2.0, 10.0, 30.0, 60.0],
        };

        let mut cmd = tokio::process::Command::new("ffmpeg");
        for t in &positions {
            cmd.args(["-ss", &format!("{t:.2}"), "-i"]).arg(path);
        }
        // Build a 2×2 grid: top row = frames 0+1, bottom row = 2+3
        cmd.args([
            "-filter_complex",
            "[0:v]scale=240:-2,setsar=1[a];\
             [1:v]scale=240:-2,setsar=1[b];\
             [2:v]scale=240:-2,setsar=1[c];\
             [3:v]scale=240:-2,setsar=1[d];\
             [a][b]hstack[top];\
             [c][d]hstack[bot];\
             [top][bot]vstack",
            "-frames:v", "1",
            "-f", "image2",
            "-vcodec", "mjpeg",
            "-q:v", "5",
        ])
        .arg(&cache)
        .stderr(std::process::Stdio::null());

        if let Ok(status) = cmd.status().await {
            if status.success() {
                if let Ok(data) = tokio::fs::read(&cache).await {
                    return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
                }
            }
        }

        // 4-input hstack failed (e.g. video shorter than all seek points).
        // Fall back to single frame at ~10% or 5 s.
        let ss = match dur {
            Some(d) if d > 1.0 => format!("{:.2}", d * 0.1),
            _ => "5".to_string(),
        };
        let out = tokio::process::Command::new("ffmpeg")
            .args(["-ss", &ss, "-i"])
            .arg(path)
            .args([
                "-vframes", "1",
                "-vf", "scale=480:-2",
                "-f", "image2",
                "-vcodec", "mjpeg",
                "-q:v", "5",
            ])
            .arg(&cache)
            .stderr(std::process::Stdio::null())
            .status()
            .await;

        if out.map(|s| s.success()).unwrap_or(false) {
            if let Ok(data) = tokio::fs::read(&cache).await {
                return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
            }
        }
    }

    (
        StatusCode::UNPROCESSABLE_ENTITY,
        "Video thumbnail unavailable — install ffmpeg",
    )
        .into_response()
}

/// Thumbnail endpoint — generates a JPEG thumbnail for any previewable file.
/// For video: returns a 2×2 contact-sheet. For others: delegates to preview_handler.
async fn thumb_handler(
    AxumPath(rel_path): AxumPath<String>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let abs = match preview_safe_path(&state.root, &rel_path) {
        Some(p) => p,
        None => return (StatusCode::BAD_REQUEST, "Invalid path").into_response(),
    };

    let ext = abs
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        // Video: 2×2 contact-sheet
        "mp4" | "webm" | "mov" | "avi" | "mkv" | "wmv" | "flv" | "m4v" | "ts" | "3gp"
        | "f4v" => video_thumb_strip(&abs, &state.root).await,

        // HEIC/HEIF: full-res conversion is already cached; thumbnail via image_thumb_jpeg
        "heic" | "heif" => {
            if let Some(cache) = thumb_cache_path(&abs, &state.root) {
                if let Ok(data) = tokio::fs::read(&cache).await {
                    return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
                }
                // Convert to JPEG first, then resize
                let full = preview_heic(&abs, &state.root).await;
                // preview_heic returns a Response; we can't easily re-use its bytes here,
                // so we call image_thumb_jpeg on the original path after HEIC cache is warm.
                if let Some(data) = image_thumb_jpeg(&abs).await {
                    let _ = tokio::fs::write(&cache, &data).await;
                    return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
                }
                drop(full);
            }
            (StatusCode::UNPROCESSABLE_ENTITY, "Thumbnail unavailable").into_response()
        }

        // RAW / PSD / layered: use raw_extract_jpeg then resize
        "arw" | "cr2" | "cr3" | "nef" | "orf" | "rw2" | "dng" | "raf" | "pef" | "srw"
        | "raw" | "3fr" | "x3f" | "rwl" | "iiq" | "mef" | "mos"
        | "psd" | "psb" | "xcf" | "ai" | "eps" => {
            if let Some(cache) = thumb_cache_path(&abs, &state.root) {
                if let Ok(data) = tokio::fs::read(&cache).await {
                    return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
                }
                // Get the full preview JPEG, then downscale it
                if let Some(full_jpeg) = raw_extract_jpeg(&abs).await {
                    // Write full preview to a temp path, resize it
                    let tmp = cache.with_extension("tmp.jpg");
                    if tokio::fs::write(&tmp, &full_jpeg).await.is_ok() {
                        if let Some(small) = image_thumb_jpeg(&tmp).await {
                            let _ = tokio::fs::remove_file(&tmp).await;
                            let _ = tokio::fs::write(&cache, &small).await;
                            return ([(header::CONTENT_TYPE, "image/jpeg")], small).into_response();
                        }
                        let _ = tokio::fs::remove_file(&tmp).await;
                    }
                    // Fallback: serve the full preview if resizing failed
                    let _ = tokio::fs::write(&cache, &full_jpeg).await;
                    return ([(header::CONTENT_TYPE, "image/jpeg")], full_jpeg).into_response();
                }
            }
            (StatusCode::UNPROCESSABLE_ENTITY, "Thumbnail unavailable").into_response()
        }

        // Regular images (JPEG, PNG, WEBP, …): resize to thumbnail
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tiff" | "tif" | "avif" => {
            if let Some(cache) = thumb_cache_path(&abs, &state.root) {
                if let Ok(data) = tokio::fs::read(&cache).await {
                    return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
                }
                if let Some(data) = image_thumb_jpeg(&abs).await {
                    let _ = tokio::fs::write(&cache, &data).await;
                    return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
                }
            }
            // Cache unavailable or tool missing: serve the original
            serve_file_bytes(&abs).await
        }

        // Everything else: fall through to preview handler
        _ => preview_handler(AxumPath(rel_path), State(state)).await,
    }
}

// ---------------------------------------------------------------------------
// Static files (embedded)
// ---------------------------------------------------------------------------

async fn index_html() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_str!("../static/index.html"),
    )
}

async fn style_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../static/style.css"),
    )
}

async fn app_js() -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        include_str!("../static/app.js"),
    )
}

async fn favicon() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "image/svg+xml")],
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><text y=".9em" font-size="90">🏷</text></svg>"#,
    )
}

// ---------------------------------------------------------------------------
// API handlers
// ---------------------------------------------------------------------------

async fn api_info(State(state): State<Arc<AppState>>) -> Result<Json<ApiInfo>, AppError> {
    let conn = open_conn(&state)?;
    let files: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
    let tags: i64 = conn.query_row("SELECT COUNT(*) FROM tags", [], |r| r.get(0))?;
    let assignments: i64 = conn.query_row("SELECT COUNT(*) FROM file_tags", [], |r| r.get(0))?;
    let total_size: i64 =
        conn.query_row("SELECT COALESCE(SUM(size), 0) FROM files", [], |r| r.get(0))?;

    Ok(Json(ApiInfo {
        root: state.root.display().to_string(),
        files,
        tags,
        assignments,
        total_size,
    }))
}

/// Delete all cached thumbnails and preview files from `.filetag/cache/`.
async fn api_cache_clear(State(state): State<Arc<AppState>>) -> Response {
    let cache_dir = state.root.join(".filetag").join("cache");
    let mut removed: u64 = 0;
    for sub in &["raw", "thumbs"] {
        let dir = cache_dir.join(sub);
        if dir.exists() {
            if let Ok(mut rd) = tokio::fs::read_dir(&dir).await {
                while let Ok(Some(entry)) = rd.next_entry().await {
                    if tokio::fs::remove_file(entry.path()).await.is_ok() {
                        removed += 1;
                    }
                }
            }
        }
    }
    // Loose HEIC cache files are stored directly under cache/
    if cache_dir.exists() {
        if let Ok(mut rd) = tokio::fs::read_dir(&cache_dir).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                if entry.path().is_file() {
                    if tokio::fs::remove_file(entry.path()).await.is_ok() {
                        removed += 1;
                    }
                }
            }
        }
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({ "removed": removed })),
    )
        .into_response()
}

async fn api_tags(State(state): State<Arc<AppState>>) -> Result<Json<Vec<ApiTag>>, AppError> {
    let conn = open_conn(&state)?;
    let tags = db::all_tags(&conn).map_err(AppError)?;
    Ok(Json(
        tags.into_iter()
            .map(|(name, count, color)| ApiTag { name, count, color })
            .collect(),
    ))
}

async fn api_files(
    State(state): State<Arc<AppState>>,
    Query(params): Query<FileListParams>,
) -> Result<Json<ApiDirListing>, AppError> {
    let dir = if params.path.is_empty() {
        state.root.clone()
    } else {
        safe_path(&state.root, &params.path)?
    };

    let prefix = if params.path.is_empty() {
        String::new()
    } else {
        format!("{}/", params.path.trim_end_matches('/'))
    };

    let conn = open_conn(&state)?;
    let mut tag_stmt = conn.prepare_cached(
        "SELECT COUNT(*) FROM file_tags ft \
         JOIN files f ON f.id = ft.file_id WHERE f.path = ?1",
    )?;

    let mut dirs = Vec::new();
    let mut files = Vec::new();

    let rd =
        std::fs::read_dir(&dir).with_context(|| format!("reading directory {}", dir.display()))?;

    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".filetag" {
            continue;
        }
        if !params.show_hidden && name.starts_with('.') {
            continue;
        }

        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };

        if meta.is_dir() {
            let child_count = std::fs::read_dir(entry.path())
                .map(|rd| rd.flatten().count() as i64)
                .unwrap_or(0);
            dirs.push(ApiDirEntry {
                name,
                is_dir: true,
                size: None,
                mtime: None,
                file_count: Some(child_count),
                tag_count: None,
            });
        } else if meta.is_file() {
            let rel_path = format!("{}{}", prefix, name);
            let size = meta.len() as i64;
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0);

            let tag_count: i64 = tag_stmt
                .query_row(rusqlite::params![&rel_path], |r| r.get(0))
                .unwrap_or(0);

            files.push(ApiDirEntry {
                name,
                is_dir: false,
                size: Some(size),
                mtime: Some(mtime),
                file_count: None,
                tag_count: Some(tag_count),
            });
        }
    }

    dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    dirs.extend(files);

    Ok(Json(ApiDirListing {
        path: params.path,
        entries: dirs,
    }))
}

async fn api_search(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchParams>,
) -> Result<Json<ApiSearchResult>, AppError> {
    let conn = open_conn(&state)?;
    let expr = query::parse(&params.q).map_err(AppError)?;
    let results = query::execute_with_tags(&conn, &expr).map_err(AppError)?;

    Ok(Json(ApiSearchResult {
        query: params.q,
        results: results
            .into_iter()
            .map(|(path, tags)| ApiSearchEntry {
                path,
                tags: tags
                    .into_iter()
                    .map(|(name, value)| ApiFileTag { name, value })
                    .collect(),
            })
            .collect(),
    }))
}

async fn api_file_detail(
    State(state): State<Arc<AppState>>,
    Query(params): Query<FileDetailParams>,
) -> Result<Json<ApiFileDetail>, AppError> {
    let conn = open_conn(&state)?;

    if let Some(record) = db::file_by_path(&conn, &params.path).map_err(AppError)? {
        let tags = db::tags_for_file(&conn, record.id).map_err(AppError)?;
        let indexed_at: String = conn.query_row(
            "SELECT indexed_at FROM files WHERE id = ?1",
            rusqlite::params![record.id],
            |r| r.get(0),
        )?;

        Ok(Json(ApiFileDetail {
            path: params.path,
            size: record.size,
            file_id: record.file_id,
            mtime: record.mtime_ns,
            indexed_at,
            tags: tags
                .into_iter()
                .map(|(name, value)| ApiFileTag { name, value })
                .collect(),
        }))
    } else {
        // File not yet indexed: return filesystem metadata
        let abs = safe_path(&state.root, &params.path)?;
        let meta = std::fs::metadata(&abs).with_context(|| format!("reading {}", abs.display()))?;
        let size = meta.len() as i64;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);

        Ok(Json(ApiFileDetail {
            path: params.path,
            size,
            file_id: None,
            mtime,
            indexed_at: String::new(),
            tags: vec![],
        }))
    }
}

async fn api_tag(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TagRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    safe_path(&state.root, &body.path)?;
    let conn = open_conn(&state)?;
    // Auto-index the file if not yet in the database
    let record = db::get_or_index_file(&conn, &body.path, &state.root).map_err(AppError)?;

    let mut added = 0i64;
    for tag_str in &body.tags {
        let (name, value) = parse_tag(tag_str);
        let tag_id = db::get_or_create_tag(&conn, &name).map_err(AppError)?;
        db::apply_tag(&conn, record.id, tag_id, value.as_deref()).map_err(AppError)?;
        added += 1;
    }

    Ok(Json(serde_json::json!({ "added": added })))
}

async fn api_untag(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TagRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let conn = open_conn(&state)?;
    let record = db::file_by_path(&conn, &body.path)
        .map_err(AppError)?
        .ok_or_else(|| AppError(anyhow::anyhow!("file not found: {}", body.path)))?;

    let mut removed = 0i64;
    for tag_str in &body.tags {
        let (name, value) = parse_tag(tag_str);
        if let Ok(tag_id) = conn.query_row(
            "SELECT id FROM tags WHERE name = ?1",
            rusqlite::params![&name],
            |r| r.get::<_, i64>(0),
        ) && db::remove_tag(&conn, record.id, tag_id, value.as_deref()).map_err(AppError)?
        {
            removed += 1;
        }
    }

    Ok(Json(serde_json::json!({ "removed": removed })))
}

/// Parse "genre/rock" -> ("genre/rock", None), "year=2024" -> ("year", Some("2024"))
fn parse_tag(s: &str) -> (String, Option<String>) {
    if let Some(eq) = s.find('=') {
        (s[..eq].to_string(), Some(s[eq + 1..].to_string()))
    } else {
        (s.to_string(), None)
    }
}

// --- Tag color ---

#[derive(Deserialize)]
struct TagColorRequest {
    name: String,
    color: Option<String>,
}

async fn api_tag_color(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TagColorRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let conn = open_conn(&state)?;
    let ok = db::set_tag_color(&conn, &body.name, body.color.as_deref()).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "ok": ok })))
}

// --- Delete tag ---

#[derive(Deserialize)]
struct DeleteTagRequest {
    name: String,
}

async fn api_delete_tag(
    State(state): State<Arc<AppState>>,
    Json(body): Json<DeleteTagRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let conn = open_conn(&state)?;
    let deleted = db::delete_tag(&conn, &body.name).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "deleted": deleted })))
}

// ---------------------------------------------------------------------------
// Static files
// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let root = args.path.unwrap_or_else(|| ".".into());
    let root =
        std::fs::canonicalize(&root).with_context(|| format!("resolving {}", root.display()))?;

    // Verify database exists
    let (conn, root) = db::find_and_open(&root)?;
    drop(conn);

    let db_path = root.join(".filetag").join("db.sqlite3");
    let state = Arc::new(AppState {
        db_path,
        root: root.clone(),
    });

    let app = Router::new()
        .route("/", get(index_html))
        .route("/style.css", get(style_css))
        .route("/app.js", get(app_js))
        .route("/favicon.svg", get(favicon))
        .route("/api/info", get(api_info))
        .route("/api/cache/clear", post(api_cache_clear))
        .route("/api/tags", get(api_tags))
        .route("/api/files", get(api_files))
        .route("/api/search", get(api_search))
        .route("/api/file", get(api_file_detail))
        .route("/api/tag", post(api_tag))
        .route("/api/untag", post(api_untag))
        .route("/api/tag-color", post(api_tag_color))
        .route("/api/delete-tag", post(api_delete_tag))
        .route("/preview/*path", get(preview_handler))
        .route("/thumb/*path", get(thumb_handler))
        .with_state(state);

    let addr = format!("{}:{}", args.bind, args.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding to {}", addr))?;

    println!("filetag-web serving {} at http://{}", root.display(), addr);
    axum::serve(listener, app).await?;

    Ok(())
}
