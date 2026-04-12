use std::io::Read;
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

/// Limit concurrent heavy thumbnail/extraction operations to prevent spawning
/// too many ffmpeg/ffprobe/unrar processes at once when browsing directories
/// with many large media files.
static THUMB_LIMITER: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

struct DbRoot {
    name: String,
    db_path: PathBuf,
    root: PathBuf,
}

struct AppState {
    roots: Vec<DbRoot>,
}

fn root_at(state: &AppState, id: Option<usize>) -> anyhow::Result<&DbRoot> {
    let idx = id.unwrap_or(0);
    state
        .roots
        .get(idx)
        .ok_or_else(|| anyhow::anyhow!("root {} not found", idx))
}

fn resolve_names(names: Vec<String>) -> Vec<String> {
    use std::collections::HashMap;
    let mut counts: HashMap<String, usize> = HashMap::new();
    for name in &names {
        *counts.entry(name.clone()).or_insert(0) += 1;
    }
    let mut seen: HashMap<String, usize> = HashMap::new();
    names
        .into_iter()
        .map(|name| {
            if counts[&name] == 1 {
                name
            } else {
                let n = seen.entry(name.clone()).or_insert(0);
                *n += 1;
                format!("{} {}", name, *n)
            }
        })
        .collect()
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

fn open_conn(db_root: &DbRoot) -> anyhow::Result<Connection> {
    let conn = Connection::open(&db_root.db_path).context("opening database")?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )?;
    Ok(conn)
}

/// Resolve a relative path under `root`, rejecting directory traversal.
fn safe_path(root: &Path, rel: &str) -> anyhow::Result<PathBuf> {
    preview_safe_path(root, rel)
        .ok_or_else(|| anyhow::anyhow!("invalid path '{}': escapes root or does not exist", rel))
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
    /// Set for virtual-root entries; identifies which database root to enter.
    #[serde(skip_serializing_if = "Option::is_none")]
    root_id: Option<usize>,
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
    root: Option<usize>,
    #[serde(default)]
    path: String,
    #[serde(default)]
    show_hidden: bool,
}

#[derive(Deserialize)]
struct SearchParams {
    q: String,
    root: Option<usize>,
}

#[derive(Deserialize)]
struct FileDetailParams {
    path: String,
    root: Option<usize>,
}

#[derive(Deserialize)]
struct TagRequest {
    path: String,
    tags: Vec<String>,
    root_id: Option<usize>,
}

#[derive(Deserialize, Default)]
struct RootParam {
    root: Option<usize>,
}

#[derive(Serialize)]
struct ApiRoot {
    id: usize,
    name: String,
    path: String,
}

#[derive(Deserialize)]
struct RenameDbRequest {
    root_id: usize,
    name: String,
}

// ---------------------------------------------------------------------------
// File preview handler
// ---------------------------------------------------------------------------

/// Serve a file for preview, converting RAW / HEIC formats server-side.
async fn preview_handler(
    AxumPath(rel_path): AxumPath<String>,
    Query(rp): Query<RootParam>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let db_root = match root_at(&state, rp.root) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "Unknown root").into_response(),
    };
    let abs = match preview_safe_path(&db_root.root, &rel_path) {
        Some(p) => p,
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
            preview_raw(&abs, &db_root.root).await
        }
        "heic" | "heif" => preview_heic(&abs, &db_root.root).await,
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

/// Return a cache path for a derived preview file, keyed by mtime + size.
/// Files are stored under `<root>/.filetag/cache/<subdir>/`.
fn file_cache_path(abs: &Path, root: &Path, subdir: &str, suffix: &str) -> Option<PathBuf> {
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
fn raw_cache_path(abs: &Path, root: &Path) -> Option<PathBuf> {
    file_cache_path(abs, root, "raw", "prev.jpg")
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
            .output()
            .await
            && out.status.success()
            && out.stdout.starts_with(&[0xFF, 0xD8])
        {
            return Some(out.stdout);
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
        .output()
        .await
        && out.status.success()
        && let Ok(data) = tokio::fs::read(&tmp).await
    {
        return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
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
        && out.status.success()
        && !out.stdout.is_empty()
    {
        let _ = tokio::fs::write(&tmp, &out.stdout).await;
        return ([(header::CONTENT_TYPE, "image/jpeg")], out.stdout).into_response();
    }

    // ImageMagick convert (with -auto-orient to respect EXIF orientation)
    if let Ok(out) = tokio::process::Command::new("convert")
        .arg(path)
        .args(["-auto-orient"])
        .arg(&tmp)
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
            // -auto-orient physically rotates the pixels according to the EXIF
            // Orientation tag; -strip then removes all metadata from the output
            // so browsers cannot re-apply the orientation a second time.
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
            .output()
            .await
            && out.status.success()
            && out.stdout.starts_with(&[0xFF, 0xD8])
        {
            return Some(out.stdout);
        }
    }

    // ffmpeg fallback.
    // -map_metadata -1 strips all metadata (including the EXIF Orientation tag)
    // from the output JPEG, so the already-rotated pixels are not rotated again
    // by the browser.
    if let Ok(out) = tokio::process::Command::new("ffmpeg")
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
// Video thumbnail strip (2×2 contact sheet via ffmpeg)
// ---------------------------------------------------------------------------

/// Generate a JPEG thumbnail for a PDF by rasterising the first page.
/// Tries pdftoppm first (poppler-utils), then ImageMagick+Ghostscript.
/// Temp files are written under `<root>/.filetag/tmp/` per data-isolation rules.
async fn pdf_thumb_jpeg(path: &Path, root: &Path) -> Option<Vec<u8>> {
    let tmp_dir = root.join(".filetag").join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    // pdftoppm appends `.jpg` to the prefix when using -singlefile -jpeg
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

/// Return the cache path for a file thumbnail, keyed by mtime + size.
/// Stored in `<root>/.filetag/cache/thumbs/`.
fn thumb_cache_path(abs: &Path, root: &Path) -> Option<PathBuf> {
    file_cache_path(abs, root, "thumbs", "thumb.jpg")
}

/// Get video duration in seconds via ffprobe.
async fn video_duration(path: &Path) -> Option<f64> {
    let out = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
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

        // Acquire a concurrency permit so we don't spawn dozens of ffmpeg processes
        // in parallel when browsing directories with many large video files.
        let _permit = THUMB_LIMITER.acquire().await.ok();

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
            "-frames:v",
            "1",
            "-f",
            "image2",
            "-vcodec",
            "mjpeg",
            "-q:v",
            "5",
        ])
        .arg(&cache)
        .stderr(std::process::Stdio::null());

        if let Ok(status) = cmd.status().await
            && status.success()
            && let Ok(data) = tokio::fs::read(&cache).await
        {
            return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
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
                "-vframes",
                "1",
                "-vf",
                "scale=480:-2",
                "-f",
                "image2",
                "-vcodec",
                "mjpeg",
                "-q:v",
                "5",
            ])
            .arg(&cache)
            .stderr(std::process::Stdio::null())
            .status()
            .await;

        if out.map(|s| s.success()).unwrap_or(false)
            && let Ok(data) = tokio::fs::read(&cache).await
        {
            return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
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

    let ext = abs
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        // ZIP/CBZ/RAR/CBR/7z/CB7: thumbnail = first image page, resized
        "zip" | "cbz" | "rar" | "cbr" | "7z" | "cb7" => {
            if let Some(cache) = thumb_cache_path(&abs, &root) {
                if let Ok(data) = tokio::fs::read(&cache).await {
                    return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
                }
                let _permit = THUMB_LIMITER.acquire().await.ok();
                let result = tokio::task::spawn_blocking(move || archive_cover_image(&abs)).await;
                if let Ok(Ok(img_bytes)) = result {
                    // Write to a temp file so image_thumb_jpeg can read it
                    let tmp = cache.with_extension("archive_src.jpg");
                    if tokio::fs::write(&tmp, &img_bytes).await.is_ok() {
                        if let Some(small) = image_thumb_jpeg(&tmp).await {
                            let _ = tokio::fs::remove_file(&tmp).await;
                            let _ = tokio::fs::write(&cache, &small).await;
                            return ([(header::CONTENT_TYPE, "image/jpeg")], small).into_response();
                        }
                        let _ = tokio::fs::remove_file(&tmp).await;
                        // fallback: serve the raw first page unresized
                        let _ = tokio::fs::write(&cache, &img_bytes).await;
                        return ([(header::CONTENT_TYPE, "image/jpeg")], img_bytes).into_response();
                    }
                }
            }
            (StatusCode::UNPROCESSABLE_ENTITY, "No images in archive").into_response()
        }

        // Video: 2×2 contact-sheet
        "mp4" | "webm" | "mov" | "avi" | "mkv" | "wmv" | "flv" | "m4v" | "ts" | "3gp" | "f4v" => {
            video_thumb_strip(&abs, &root).await
        }

        // HEIC/HEIF: full-res conversion is already cached; thumbnail via image_thumb_jpeg
        "heic" | "heif" => {
            if let Some(cache) = thumb_cache_path(&abs, &root) {
                if let Ok(data) = tokio::fs::read(&cache).await {
                    return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
                }
                let _permit = THUMB_LIMITER.acquire().await.ok();
                // Convert to JPEG first, then resize
                let full = preview_heic(&abs, &root).await;
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
        "arw" | "cr2" | "cr3" | "nef" | "orf" | "rw2" | "dng" | "raf" | "pef" | "srw" | "raw"
        | "3fr" | "x3f" | "rwl" | "iiq" | "mef" | "mos" | "psd" | "psb" | "xcf" | "ai" | "eps" => {
            if let Some(cache) = thumb_cache_path(&abs, &root) {
                if let Ok(data) = tokio::fs::read(&cache).await {
                    return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
                }
                let _permit = THUMB_LIMITER.acquire().await.ok();
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

        // PDF: rasterise first page via pdftoppm or ImageMagick+Ghostscript
        "pdf" => {
            if let Some(cache) = thumb_cache_path(&abs, &root) {
                if let Ok(data) = tokio::fs::read(&cache).await {
                    return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
                }
                let _permit = THUMB_LIMITER.acquire().await.ok();
                if let Some(data) = pdf_thumb_jpeg(&abs, &root).await {
                    let _ = tokio::fs::write(&cache, &data).await;
                    return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
                }
            }
            (StatusCode::UNPROCESSABLE_ENTITY,
             "PDF thumbnail unavailable — install pdftoppm (poppler-utils) or ImageMagick+Ghostscript")
                .into_response()
        }

        // Regular images (JPEG, PNG, WEBP, …): resize to thumbnail
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tiff" | "tif" | "avif" => {
            if let Some(cache) = thumb_cache_path(&abs, &root) {
                if let Ok(data) = tokio::fs::read(&cache).await {
                    return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
                }
                let _permit = THUMB_LIMITER.acquire().await.ok();
                if let Some(data) = image_thumb_jpeg(&abs).await {
                    let _ = tokio::fs::write(&cache, &data).await;
                    return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
                }
            }
            // Cache unavailable or tool missing: serve the original
            serve_file_bytes(&abs).await
        }

        // Everything else: fall through to preview handler
        _ => preview_handler(AxumPath(rel_path), Query(rp), State(state)).await,
    }
}

// ---------------------------------------------------------------------------
// ZIP / CBZ comic viewer
// ---------------------------------------------------------------------------

/// Image extensions that count as comic pages inside a ZIP.
const ZIP_IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "bmp", "tiff", "tif", "avif",
];

fn is_zip_image(name: &str) -> bool {
    // Skip macOS AppleDouble resource fork entries (stored under __MACOSX/ or
    // with a ._-prefixed filename).  These carry a .jpg/.png extension but are
    // not valid images and would break thumbnails and the comic viewer.
    if name.starts_with("__MACOSX/") {
        return false;
    }
    let basename = name.rsplit('/').next().unwrap_or(name);
    if basename.starts_with("._") {
        return false;
    }

    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    ZIP_IMAGE_EXTS.contains(&ext.as_str())
}

/// Collect and sort image entry names from a ZIP file.
fn zip_image_entries(path: &Path) -> anyhow::Result<Vec<String>> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut names: Vec<String> = (0..archive.len())
        .filter_map(|i| {
            let entry = archive.by_index(i).ok()?;
            let name = entry.name().to_owned();
            if !entry.is_dir() && is_zip_image(&name) {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    names.sort_by(|a, b| natord(a, b));
    Ok(names)
}

/// Minimal natural-order string comparison for consistent page sorting.
fn natord(a: &str, b: &str) -> std::cmp::Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(ac), Some(bc)) if ac.is_ascii_digit() && bc.is_ascii_digit() => {
                let na: u64 = std::iter::from_fn(|| ai.next_if(|c| c.is_ascii_digit()))
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0);
                let nb: u64 = std::iter::from_fn(|| bi.next_if(|c| c.is_ascii_digit()))
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0);
                match na.cmp(&nb) {
                    std::cmp::Ordering::Equal => {}
                    ord => return ord,
                }
            }
            (Some(ac), Some(bc)) => {
                let al = ac.to_lowercase().next().unwrap();
                let bl = bc.to_lowercase().next().unwrap();
                if al != bl {
                    return al.cmp(&bl);
                }
                ai.next();
                bi.next();
            }
        }
    }
}

/// Extract raw bytes of a single image entry from a ZIP.
fn zip_read_entry(zip_path: &Path, entry_name: &str) -> anyhow::Result<(Vec<u8>, &'static str)> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut entry = archive
        .by_name(entry_name)
        .map_err(|_| anyhow::anyhow!("entry not found: {}", entry_name))?;
    let ext = entry_name.rsplit('.').next().unwrap_or("").to_lowercase();
    let mime = mime_for_ext(&ext);
    let mut buf = Vec::new();
    entry.read_to_end(&mut buf)?;
    Ok((buf, mime))
}

/// List all (name, unpacked_size, is_image) entries from a ZIP, sorted naturally.
fn zip_list_entries_raw(path: &Path) -> anyhow::Result<Vec<(String, u64, bool)>> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut entries: Vec<(String, u64, bool)> = Vec::new();
    for i in 0..archive.len() {
        if let Ok(entry) = archive.by_index(i)
            && !entry.is_dir()
        {
            let name = entry.name().to_owned();
            if name.starts_with("__MACOSX/") {
                continue;
            }
            let basename = name.rsplit('/').next().unwrap_or(&name);
            if basename.starts_with("._") {
                continue;
            }
            let size = entry.size();
            let is_im = is_zip_image(&name);
            entries.push((name, size, is_im));
        }
    }
    entries.sort_by(|a, b| natord(&a.0, &b.0));
    Ok(entries)
}

// ---------------------------------------------------------------------------
// RAR / CBR comic viewer
// ---------------------------------------------------------------------------

fn rar_image_entries(path: &Path) -> anyhow::Result<Vec<String>> {
    let archive = unrar::Archive::new(path).open_for_listing()?;
    let mut names: Vec<String> = archive
        .filter_map(|e| e.ok())
        .filter(|e| e.is_file())
        .map(|e| e.filename.to_string_lossy().replace('\\', "/"))
        .filter(|name| is_zip_image(name))
        .collect();
    names.sort_by(|a, b| natord(a, b));
    Ok(names)
}

fn rar_read_entry(rar_path: &Path, entry_name: &str) -> anyhow::Result<(Vec<u8>, &'static str)> {
    let mut archive = unrar::Archive::new(rar_path).open_for_processing()?;
    while let Some(header) = archive.read_header()? {
        if header.entry().filename.to_string_lossy().replace('\\', "/") == entry_name {
            let (data, _rest) = header.read()?;
            let ext = entry_name.rsplit('.').next().unwrap_or("").to_lowercase();
            return Ok((data, mime_for_ext(&ext)));
        }
        archive = header.skip()?;
    }
    anyhow::bail!("entry not found: {entry_name}")
}

fn rar_list_entries_raw(path: &Path) -> anyhow::Result<Vec<(String, u64, bool)>> {
    let archive = unrar::Archive::new(path).open_for_listing()?;
    let mut entries: Vec<(String, u64, bool)> = archive
        .filter_map(|e| e.ok())
        .filter(|e| e.is_file())
        .map(|e| {
            let name = e.filename.to_string_lossy().replace('\\', "/");
            let size = e.unpacked_size;
            let is_im = is_zip_image(&name);
            (name, size, is_im)
        })
        .collect();
    entries.sort_by(|a, b| natord(&a.0, &b.0));
    Ok(entries)
}

// ---------------------------------------------------------------------------
// 7z / CB7 comic viewer
// ---------------------------------------------------------------------------

fn sevenz_image_entries(path: &Path) -> anyhow::Result<Vec<String>> {
    let sz = sevenz_rust::SevenZReader::open(path, sevenz_rust::Password::empty())?;
    let mut names: Vec<String> = sz
        .archive()
        .files
        .iter()
        .filter(|e| !e.is_directory() && e.has_stream())
        .map(|e| e.name().replace('\\', "/"))
        .filter(|name| is_zip_image(name))
        .collect();
    names.sort_by(|a, b| natord(a, b));
    Ok(names)
}

fn sevenz_read_entry(path: &Path, entry_name: &str) -> anyhow::Result<(Vec<u8>, &'static str)> {
    let target = entry_name.replace('\\', "/");
    let mut found: Option<Vec<u8>> = None;
    let mut read_err: Option<std::io::Error> = None;
    let mut sz = sevenz_rust::SevenZReader::open(path, sevenz_rust::Password::empty())?;
    sz.for_each_entries(|entry, reader| {
        if !entry.is_directory() && entry.name().replace('\\', "/") == target {
            let mut buf = Vec::new();
            match reader.read_to_end(&mut buf) {
                Ok(_) => found = Some(buf),
                Err(e) => read_err = Some(e),
            }
            Ok(false)
        } else {
            Ok(true)
        }
    })?;
    if let Some(e) = read_err {
        return Err(anyhow::anyhow!("read error: {e}"));
    }
    let data = found.ok_or_else(|| anyhow::anyhow!("entry not found: {entry_name}"))?;
    let ext = entry_name.rsplit('.').next().unwrap_or("").to_lowercase();
    Ok((data, mime_for_ext(&ext)))
}

fn sevenz_list_entries_raw(path: &Path) -> anyhow::Result<Vec<(String, u64, bool)>> {
    let sz = sevenz_rust::SevenZReader::open(path, sevenz_rust::Password::empty())?;
    let mut entries: Vec<(String, u64, bool)> = sz
        .archive()
        .files
        .iter()
        .filter(|e| !e.is_directory() && e.has_stream())
        .map(|e| {
            let name = e.name().replace('\\', "/");
            let size = e.size();
            let is_im = is_zip_image(&name);
            (name, size, is_im)
        })
        .collect();
    entries.sort_by(|a, b| natord(&a.0, &b.0));
    Ok(entries)
}

// ---------------------------------------------------------------------------
// Archive format dispatcher (ZIP + RAR + 7z)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Cover image extraction (optimised for thumbnail use)
// ---------------------------------------------------------------------------

/// Extract the cover image bytes from an archive, optimised for speed.
///
/// - ZIP/CBZ: finds the natural-sort minimum image in one metadata-only pass,
///   then decompresses only that entry.
/// - RAR/CBR: processes entries sequentially and stops at the first image found
///   (avoids a full listing pass before extraction).
/// - 7z/CB7: reads metadata-only header to find natural-sort minimum, then
///   decompresses only that entry.
fn archive_cover_image(path: &Path) -> anyhow::Result<Vec<u8>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "zip" | "cbz" => zip_cover_image(path),
        "rar" | "cbr" => rar_cover_image(path),
        "7z" | "cb7" => sevenz_cover_image(path),
        e => anyhow::bail!("unsupported archive format: {e}"),
    }
}

fn zip_cover_image(path: &Path) -> anyhow::Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    // One pass over central directory (header only, no decompression) to find
    // the natural-sort minimum image name.
    let mut best: Option<String> = None;
    for i in 0..archive.len() {
        let entry = archive.by_index_raw(i)?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_owned();
        if !is_zip_image(&name) {
            continue;
        }
        if best
            .as_ref()
            .map(|b| natord(&name, b) == std::cmp::Ordering::Less)
            .unwrap_or(true)
        {
            best = Some(name);
        }
    }
    let first = best.ok_or_else(|| anyhow::anyhow!("no images in archive"))?;
    let (data, _mime) = zip_read_entry(path, &first)?;
    Ok(data)
}

fn rar_cover_image(path: &Path) -> anyhow::Result<Vec<u8>> {
    // Sequential processing: stop and extract the first image found.
    // This avoids a separate listing pass followed by another extraction pass.
    let mut archive = unrar::Archive::new(path).open_for_processing()?;
    while let Some(header) = archive.read_header()? {
        let name = header.entry().filename.to_string_lossy().replace('\\', "/");
        let basename = name.rsplit('/').next().unwrap_or(&name);
        if !basename.starts_with("._") && is_zip_image(&name) {
            let (data, _rest) = header.read()?;
            return Ok(data);
        }
        archive = header.skip()?;
    }
    anyhow::bail!("no images in archive")
}

fn sevenz_cover_image(path: &Path) -> anyhow::Result<Vec<u8>> {
    // The 7z header (metadata only, at end of file) lists all entries without
    // decompressing data. Find the natural-sort minimum image, then extract it.
    let sz = sevenz_rust::SevenZReader::open(path, sevenz_rust::Password::empty())?;
    let mut best: Option<String> = None;
    for e in sz.archive().files.iter() {
        if e.is_directory() || !e.has_stream() {
            continue;
        }
        let name = e.name().replace('\\', "/");
        if !is_zip_image(&name) {
            continue;
        }
        if best
            .as_ref()
            .map(|b| natord(&name, b) == std::cmp::Ordering::Less)
            .unwrap_or(true)
        {
            best = Some(name);
        }
    }
    let first = best.ok_or_else(|| anyhow::anyhow!("no images in archive"))?;
    let (data, _mime) = sevenz_read_entry(path, &first)?;
    Ok(data)
}

fn archive_image_entries(path: &Path) -> anyhow::Result<Vec<String>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "zip" | "cbz" => zip_image_entries(path),
        "rar" | "cbr" => rar_image_entries(path),
        "7z" | "cb7" => sevenz_image_entries(path),
        e => anyhow::bail!("unsupported archive format: {e}"),
    }
}

fn archive_read_entry(path: &Path, entry_name: &str) -> anyhow::Result<(Vec<u8>, &'static str)> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "zip" | "cbz" => zip_read_entry(path, entry_name),
        "rar" | "cbr" => rar_read_entry(path, entry_name),
        "7z" | "cb7" => sevenz_read_entry(path, entry_name),
        e => anyhow::bail!("unsupported archive format: {e}"),
    }
}

fn archive_list_entries_raw(path: &Path) -> anyhow::Result<Vec<(String, u64, bool)>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "zip" | "cbz" => zip_list_entries_raw(path),
        "rar" | "cbr" => rar_list_entries_raw(path),
        "7z" | "cb7" => sevenz_list_entries_raw(path),
        e => anyhow::bail!("unsupported archive format: {e}"),
    }
}

// --- API: list pages ---

// --- API: list image files in a directory ---

/// Image extensions shown in the directory viewer (raster + common camera formats).
const DIR_IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "bmp", "avif", "tiff", "tif", "heic", "heif", "arw",
    "cr2", "cr3", "nef", "orf", "rw2", "dng", "raf", "pef", "srw", "raw", "3fr", "x3f", "rwl",
    "iiq", "mef", "mos",
];

fn is_dir_image(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    DIR_IMAGE_EXTS.contains(&ext.as_str())
}

#[derive(Deserialize)]
struct DirImagesParams {
    path: String,
    root: Option<usize>,
}

#[derive(Serialize)]
struct DirImagesResponse {
    images: Vec<String>,
}

async fn api_dir_images(
    State(state): State<Arc<AppState>>,
    Query(params): Query<DirImagesParams>,
) -> Response {
    let db_root = match root_at(&state, params.root) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "Unknown root").into_response(),
    };
    let dir_abs = match preview_safe_path(&db_root.root, &params.path) {
        Some(p) => p,
        None => return (StatusCode::BAD_REQUEST, "Invalid path").into_response(),
    };
    if !dir_abs.is_dir() {
        return (StatusCode::BAD_REQUEST, "Not a directory").into_response();
    }
    let root = db_root.root.clone();
    match tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<String>> {
        let mut images: Vec<String> = std::fs::read_dir(&dir_abs)?
            .filter_map(|e| {
                let e = e.ok()?;
                let ft = e.file_type().ok()?;
                if !ft.is_file() {
                    return None;
                }
                let name = e.file_name().to_string_lossy().into_owned();
                if !is_dir_image(&name) {
                    return None;
                }
                // Return path relative to root
                let abs = e.path();
                let rel = abs
                    .strip_prefix(&root)
                    .ok()
                    .map(|p| p.to_string_lossy().into_owned())?;
                Some(rel)
            })
            .collect();
        images.sort_by_key(|a| a.to_lowercase());
        Ok(images)
    })
    .await
    {
        Ok(Ok(images)) => (StatusCode::OK, Json(DirImagesResponse { images })).into_response(),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not list directory",
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ZipListParams {
    path: String,
    root: Option<usize>,
}

#[derive(Serialize)]
struct ZipPagesResponse {
    pages: Vec<String>,
    count: usize,
}

async fn api_zip_pages(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ZipListParams>,
) -> Response {
    let db_root = match root_at(&state, params.root) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "Unknown root").into_response(),
    };
    let abs = match preview_safe_path(&db_root.root, &params.path) {
        Some(p) => p,
        None => return (StatusCode::BAD_REQUEST, "Invalid path").into_response(),
    };
    match tokio::task::spawn_blocking(move || archive_image_entries(&abs)).await {
        Ok(Ok(pages)) => {
            let count = pages.len();
            (StatusCode::OK, Json(ZipPagesResponse { pages, count })).into_response()
        }
        _ => (StatusCode::UNPROCESSABLE_ENTITY, "Cannot read archive").into_response(),
    }
}

// --- API: serve single page from ZIP ---

#[derive(Deserialize)]
struct ZipPageParams {
    path: String,
    page: usize,
    root: Option<usize>,
}

async fn api_zip_page(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ZipPageParams>,
) -> Response {
    let db_root = match root_at(&state, params.root) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "Unknown root").into_response(),
    };
    let abs = match preview_safe_path(&db_root.root, &params.path) {
        Some(p) => p,
        None => return (StatusCode::BAD_REQUEST, "Invalid path").into_response(),
    };
    let page_idx = params.page;
    let result = tokio::task::spawn_blocking(move || {
        let pages = archive_image_entries(&abs)?;
        let name = pages
            .into_iter()
            .nth(page_idx)
            .ok_or_else(|| anyhow::anyhow!("page out of range"))?;
        archive_read_entry(&abs, &name)
    })
    .await;
    match result {
        Ok(Ok((data, mime))) => ([(header::CONTENT_TYPE, mime)], data).into_response(),
        Ok(Err(e)) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "task error").into_response(),
    }
}

// --- API: serve resized thumbnail for a single page inside a ZIP ---

/// Returns a cache path for a specific ZIP page thumbnail.
/// Key: `<mtime>_<size>_<stem>_p<page>.thumb.jpg`
fn zip_page_thumb_cache_path(abs: &Path, root: &Path, page: usize) -> Option<PathBuf> {
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
    let key = format!("{mtime}_{size}_{stem}_p{page}.thumb.jpg");
    let dir = root.join(".filetag").join("cache").join("zip-pages");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(key))
}

#[derive(Deserialize)]
struct ZipThumbParams {
    path: String,
    page: usize,
    root: Option<usize>,
}

async fn api_zip_thumb(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ZipThumbParams>,
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
    let page_idx = params.page;

    // Serve from cache if available
    if let Some(cache) = zip_page_thumb_cache_path(&abs, &root, page_idx) {
        if let Ok(data) = tokio::fs::read(&cache).await {
            return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
        }

        let _permit = THUMB_LIMITER.acquire().await.ok();

        // Extract the page, write to temp, resize, cache, serve
        let result = tokio::task::spawn_blocking(move || {
            let pages = archive_image_entries(&abs)?;
            let name = pages
                .into_iter()
                .nth(page_idx)
                .ok_or_else(|| anyhow::anyhow!("page out of range"))?;
            archive_read_entry(&abs, &name)
        })
        .await;

        if let Ok(Ok((img_bytes, _mime))) = result {
            let tmp = root
                .join(".filetag")
                .join("tmp")
                .join(format!("zp_{page_idx}.jpg"));
            let _ = tokio::fs::create_dir_all(tmp.parent().unwrap()).await;
            if tokio::fs::write(&tmp, &img_bytes).await.is_ok() {
                if let Some(small) = image_thumb_jpeg(&tmp).await {
                    let _ = tokio::fs::remove_file(&tmp).await;
                    let _ = tokio::fs::write(&cache, &small).await;
                    return ([(header::CONTENT_TYPE, "image/jpeg")], small).into_response();
                }
                let _ = tokio::fs::remove_file(&tmp).await;
            }
            // Fallback: serve raw bytes uncached
            return ([(header::CONTENT_TYPE, "image/jpeg")], img_bytes).into_response();
        }
        return (StatusCode::NOT_FOUND, "Page not found").into_response();
    }
    (StatusCode::INTERNAL_SERVER_ERROR, "Cache unavailable").into_response()
}

// ---------------------------------------------------------------------------
// Static files (embedded)
// ---------------------------------------------------------------------------

/// Ensure a virtual zip-entry record exists in the `files` table and return its id.
/// The DB path for an entry is `zip_rel::entry_name` (the `::` separator is never
/// valid in real filesystem paths, so it uniquely marks virtual entries).
fn ensure_zip_entry_record(conn: &rusqlite::Connection, db_path: &str) -> anyhow::Result<i64> {
    if let Ok(id) = conn.query_row(
        "SELECT id FROM files WHERE path = ?1",
        rusqlite::params![db_path],
        |r| r.get::<_, i64>(0),
    ) {
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO files (path, file_id, size, mtime_ns, indexed_at) \
         VALUES (?1, NULL, 0, 0, datetime('now'))",
        rusqlite::params![db_path],
    )?;
    Ok(conn.last_insert_rowid())
}

#[derive(Serialize)]
struct ZipEntry {
    name: String,
    size: u64,
    is_image: bool,
    image_index: Option<usize>,
    tag_count: i64,
}

#[derive(Serialize)]
struct ZipEntriesResponse {
    zip_path: String,
    entries: Vec<ZipEntry>,
}

async fn api_zip_entries(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ZipListParams>,
) -> Response {
    let db_root = match root_at(&state, params.root) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "Unknown root").into_response(),
    };
    let abs = match preview_safe_path(&db_root.root, &params.path) {
        Some(p) => p,
        None => return (StatusCode::BAD_REQUEST, "Invalid path").into_response(),
    };

    // Enumerate all entries in a blocking thread
    let raw: Vec<(String, u64, bool)> =
        match tokio::task::spawn_blocking(move || archive_list_entries_raw(&abs)).await {
            Ok(Ok(v)) => v,
            _ => return (StatusCode::UNPROCESSABLE_ENTITY, "Cannot read archive").into_response(),
        };

    // Query tag counts for all entries of this archive in a single SQL query
    // rather than one query per entry.
    let conn = match open_conn(db_root) {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let prefix_like = format!(
        "{}::{}",
        params.path.replace('%', "\\%").replace('_', "\\_"),
        '%'
    );
    let tag_map: std::collections::HashMap<String, i64> = {
        let mut stmt = match conn.prepare(
            "SELECT f.path, COUNT(*) FROM file_tags ft \
             JOIN files f ON f.id = ft.file_id \
             WHERE f.path LIKE ?1 ESCAPE '\\' \
             GROUP BY f.path",
        ) {
            Ok(s) => s,
            Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
        };
        stmt.query_map(rusqlite::params![prefix_like], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    };

    let mut image_counter = 0usize;
    let mut entries = Vec::with_capacity(raw.len());
    for (name, size, is_image) in raw {
        let image_index = is_image.then(|| {
            let i = image_counter;
            image_counter += 1;
            i
        });
        let db_path = format!("{}::{}", params.path, name);
        let tag_count = tag_map.get(&db_path).copied().unwrap_or(0);
        entries.push(ZipEntry {
            name,
            size,
            is_image,
            image_index,
            tag_count,
        });
    }

    (
        StatusCode::OK,
        Json(ZipEntriesResponse {
            zip_path: params.path,
            entries,
        }),
    )
        .into_response()
}

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

async fn api_roots(State(state): State<Arc<AppState>>) -> Json<Vec<ApiRoot>> {
    Json(
        state
            .roots
            .iter()
            .enumerate()
            .map(|(id, r)| ApiRoot {
                id,
                name: r.name.clone(),
                path: r.root.display().to_string(),
            })
            .collect(),
    )
}

async fn api_rename_db(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RenameDbRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_at(&state, Some(body.root_id))?;
    let conn = open_conn(db_root)?;
    db::set_setting(&conn, "name", &body.name)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn api_info(
    State(state): State<Arc<AppState>>,
    Query(rp): Query<RootParam>,
) -> Result<Json<ApiInfo>, AppError> {
    let db_root = root_at(&state, rp.root)?;
    let conn = open_conn(db_root)?;
    let files: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
    let tags: i64 = conn.query_row("SELECT COUNT(*) FROM tags", [], |r| r.get(0))?;
    let assignments: i64 = conn.query_row("SELECT COUNT(*) FROM file_tags", [], |r| r.get(0))?;
    let total_size: i64 =
        conn.query_row("SELECT COALESCE(SUM(size), 0) FROM files", [], |r| r.get(0))?;

    Ok(Json(ApiInfo {
        root: db_root.root.display().to_string(),
        files,
        tags,
        assignments,
        total_size,
    }))
}

/// Delete all cached thumbnails and preview files from `.filetag/cache/`.
/// Body for `POST /api/cache/clear`. If `paths` is `Some`, only those files'
/// cache entries are removed. If `None` (or missing), the entire cache is cleared.
#[derive(serde::Deserialize, Default)]
struct CacheClearBody {
    paths: Option<Vec<String>>,
}

/// Delete cache entries for a single file (all variants: thumb, raw preview, HEIC).
fn remove_cache_for_path(abs: &Path, root: &Path) -> u64 {
    let mut removed = 0u64;
    // Thumb (video contact-sheet OR image thumbnail)
    if let Some(p) = thumb_cache_path(abs, root)
        && std::fs::remove_file(&p).is_ok()
    {
        removed += 1;
    }
    // RAW/PSD preview
    if let Some(p) = raw_cache_path(abs, root)
        && std::fs::remove_file(&p).is_ok()
    {
        removed += 1;
    }
    // HEIC loose cache file (named heic_<name>_<mtime>.jpg)
    let cache_dir = root.join(".filetag").join("cache");
    if let Some(stem) = abs.file_name().map(|n| n.to_string_lossy().into_owned()) {
        let mtime = std::fs::metadata(abs)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let heic_path = cache_dir.join(format!("heic_{}_{}.jpg", stem, mtime));
        if std::fs::remove_file(&heic_path).is_ok() {
            removed += 1;
        }
    }
    removed
}

async fn api_cache_clear(
    State(state): State<Arc<AppState>>,
    Query(rp): Query<RootParam>,
    body: Option<axum::extract::Json<CacheClearBody>>,
) -> Response {
    let db_root = match root_at(&state, rp.root) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "Unknown root").into_response(),
    };
    let root = db_root.root.clone();
    let paths = body.and_then(|b| b.paths.clone());

    let removed = if let Some(rel_paths) = paths {
        // Clear only the specified files
        let mut n = 0u64;
        for rel in rel_paths {
            if let Some(abs) = preview_safe_path(&root, &rel) {
                n += remove_cache_for_path(&abs, &root);
            }
        }
        n
    } else {
        // Clear the entire cache for this root
        let cache_dir = root.join(".filetag").join("cache");
        let mut n = 0u64;
        for sub in &["raw", "thumbs"] {
            let dir = cache_dir.join(sub);
            if dir.exists()
                && let Ok(mut rd) = tokio::fs::read_dir(&dir).await
            {
                while let Ok(Some(entry)) = rd.next_entry().await {
                    if tokio::fs::remove_file(entry.path()).await.is_ok() {
                        n += 1;
                    }
                }
            }
        }
        // Loose HEIC cache files directly under cache/
        if cache_dir.exists()
            && let Ok(mut rd) = tokio::fs::read_dir(&cache_dir).await
        {
            while let Ok(Some(entry)) = rd.next_entry().await {
                if entry.path().is_file() && tokio::fs::remove_file(entry.path()).await.is_ok() {
                    n += 1;
                }
            }
        }
        n
    };

    (
        StatusCode::OK,
        Json(serde_json::json!({ "removed": removed })),
    )
        .into_response()
}

async fn api_tags(
    State(state): State<Arc<AppState>>,
    Query(rp): Query<RootParam>,
) -> Result<Json<Vec<ApiTag>>, AppError> {
    let db_root = root_at(&state, rp.root)?;
    let conn = open_conn(db_root)?;
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
    // Virtual root: no root_id specified and no path — list all roots as directories
    if params.root.is_none() && params.path.is_empty() && state.roots.len() > 1 {
        let entries = state
            .roots
            .iter()
            .enumerate()
            .map(|(id, r)| ApiDirEntry {
                name: r.name.clone(),
                is_dir: true,
                size: None,
                mtime: None,
                file_count: None,
                tag_count: None,
                root_id: Some(id),
            })
            .collect();
        return Ok(Json(ApiDirListing {
            path: String::new(),
            entries,
        }));
    }

    let db_root = root_at(&state, params.root)?;
    let dir = if params.path.is_empty() {
        db_root.root.clone()
    } else {
        safe_path(&db_root.root, &params.path)?
    };

    let prefix = if params.path.is_empty() {
        String::new()
    } else {
        format!("{}/", params.path.trim_end_matches('/'))
    };

    let conn = open_conn(db_root)?;
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
                root_id: None,
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
                root_id: None,
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
    let db_root = root_at(&state, params.root)?;
    let conn = open_conn(db_root)?;
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
    let db_root = root_at(&state, params.root)?;
    let conn = open_conn(db_root)?;

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
    } else if params.path.contains("::") {
        // Virtual zip entry not yet in DB — return empty detail
        Ok(Json(ApiFileDetail {
            path: params.path,
            size: 0,
            file_id: None,
            mtime: 0,
            indexed_at: String::new(),
            tags: vec![],
        }))
    } else {
        // File not yet indexed: return filesystem metadata
        let abs = safe_path(&db_root.root, &params.path)?;
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
    let db_root = root_at(&state, body.root_id)?;
    let conn = open_conn(db_root)?;
    let file_id = if body.path.contains("::") {
        // Virtual zip entry — no filesystem check
        ensure_zip_entry_record(&conn, &body.path).map_err(AppError)?
    } else {
        safe_path(&db_root.root, &body.path)?;
        // Auto-index the file if not yet in the database
        db::get_or_index_file(&conn, &body.path, &db_root.root)
            .map_err(AppError)?
            .id
    };

    let mut added = 0i64;
    for tag_str in &body.tags {
        let (name, value) = parse_tag(tag_str);
        let tag_id = db::get_or_create_tag(&conn, &name).map_err(AppError)?;
        db::apply_tag(&conn, file_id, tag_id, value.as_deref()).map_err(AppError)?;
        added += 1;
    }

    Ok(Json(serde_json::json!({ "added": added })))
}

async fn api_untag(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TagRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_at(&state, body.root_id)?;
    let conn = open_conn(db_root)?;
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
    // Tag colors are per-database; default to root 0
    let db_root = root_at(&state, None)?;
    let conn = open_conn(db_root)?;
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
    let db_root = root_at(&state, None)?;
    let conn = open_conn(db_root)?;
    let deleted = db::delete_tag(&conn, &body.name).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "deleted": deleted })))
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let root = args.path.unwrap_or_else(|| ".".into());
    let root =
        std::fs::canonicalize(&root).with_context(|| format!("resolving {}", root.display()))?;

    // Open primary database and collect all linked databases.
    let (conn, root) = db::find_and_open(&root)?;
    let all_dbs = db::collect_all_databases(conn, root.clone())?;

    // Build named roots (name comes from the `settings` table key "name", or
    // falls back to the last path component of the root directory).
    let raw_names: Vec<String> = all_dbs
        .iter()
        .map(|db| {
            let conn_tmp = Connection::open(db.root.join(".filetag").join("db.sqlite3")).ok();
            conn_tmp
                .as_ref()
                .and_then(|c| db::get_setting(c, "name").ok().flatten())
                .unwrap_or_else(|| {
                    db.root
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| db.root.display().to_string())
                })
        })
        .collect();
    let names = resolve_names(raw_names);

    let roots: Vec<DbRoot> = all_dbs
        .into_iter()
        .zip(names)
        .map(|(open_db, name)| DbRoot {
            name,
            db_path: open_db.root.join(".filetag").join("db.sqlite3"),
            root: open_db.root,
        })
        .collect();

    if roots.is_empty() {
        anyhow::bail!("no databases found");
    }

    let primary_root = roots[0].root.display().to_string();
    let n_roots = roots.len();
    let state = Arc::new(AppState { roots });

    let app = Router::new()
        .route("/", get(index_html))
        .route("/style.css", get(style_css))
        .route("/app.js", get(app_js))
        .route("/favicon.svg", get(favicon))
        .route("/api/roots", get(api_roots))
        .route("/api/db/rename", post(api_rename_db))
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
        .route("/api/zip/pages", get(api_zip_pages))
        .route("/api/zip/page", get(api_zip_page))
        .route("/api/zip/thumb", get(api_zip_thumb))
        .route("/api/zip/entries", get(api_zip_entries))
        .route("/api/dir/images", get(api_dir_images))
        .route("/preview/*path", get(preview_handler))
        .route("/thumb/*path", get(thumb_handler))
        .with_state(state);

    let addr = format!("{}:{}", args.bind, args.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding to {}", addr))?;

    if n_roots > 1 {
        println!(
            "filetag-web serving {} databases (primary: {}) at http://{}",
            n_roots, primary_root, addr
        );
    } else {
        println!("filetag-web serving {} at http://{}", primary_root, addr);
    }
    axum::serve(listener, app).await?;

    Ok(())
}
