use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio_util::bytes::Bytes;

use anyhow::Context;
use axum::{
    Router,
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, StatusCode, header},
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

    /// Do not automatically include ancestor databases (stop at the current root)
    #[arg(long)]
    no_parents: bool,
}

// ---------------------------------------------------------------------------
// State and error handling
// ---------------------------------------------------------------------------

/// Limit concurrent heavy thumbnail/extraction operations to prevent spawning
/// too many ffmpeg/ffprobe/unrar processes at once when browsing directories
/// with many large media files.
static THUMB_LIMITER: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

struct DbRoot {
    name: String,
    db_path: PathBuf,
    root: PathBuf,
    /// Device ID of the root directory (Unix only). Used to detect filesystem
    /// boundary crossings when showing/tagging files.
    #[cfg(unix)]
    dev: Option<u64>,
    /// True when no other loaded root is a strict ancestor of this one.
    /// Entry-point roots are shown as top-level navigation tiles.
    entry_point: bool,
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

/// Returns true when `abs_path` is covered by any loaded database root.
///
/// A file is covered when there is a loaded `DbRoot` that:
///   1. resides on the same filesystem as the file (`st_dev` match), AND
///   2. whose root directory is an ancestor of `abs_path`.
///
/// This correctly handles mounted volumes that have their own database: even if
/// the file appears inside the directory tree of a parent root, the mount's own
/// DbRoot makes it covered. On non-Unix platforms all files are considered covered.
#[cfg(unix)]
fn file_is_covered(state: &AppState, meta: &std::fs::Metadata, abs_path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let file_dev = meta.dev();
    state
        .roots
        .iter()
        .any(|root| root.dev.is_none_or(|d| d == file_dev) && abs_path.starts_with(&root.root))
}

#[cfg(not(unix))]
fn file_is_covered(_state: &AppState, _meta: &std::fs::Metadata, _abs_path: &Path) -> bool {
    true
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
    /// False when the file is on a different filesystem than the database root.
    /// Tagging is not allowed in that case.
    #[serde(skip_serializing_if = "Option::is_none")]
    covered: Option<bool>,
}

#[derive(Serialize)]
struct ApiFileDetail {
    path: String,
    size: i64,
    file_id: Option<String>,
    mtime: i64,
    indexed_at: String,
    tags: Vec<ApiFileTag>,
    /// False when the file is on a different filesystem than the database root.
    covered: bool,
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
    sort_order: i64,
    /// False when this root is a subdirectory of another loaded root.
    /// Non-entry-point roots are not shown as top-level navigation tiles.
    entry_point: bool,
}

#[derive(Deserialize)]
struct RenameDbRequest {
    root_id: usize,
    name: String,
}

#[derive(Deserialize)]
struct ReorderRootsRequest {
    /// Root IDs in the desired new order (first element = sort position 0).
    order: Vec<usize>,
}

// ---------------------------------------------------------------------------
// File preview handler
// ---------------------------------------------------------------------------

/// Serve a file for preview, converting RAW / HEIC formats server-side.
async fn preview_handler(
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
        // Formats browsers cannot decode natively: transcode to mp4 via ffmpeg
        "avi" | "wmv" | "mkv" | "flv" | "mpg" | "mpeg" | "ts" | "3gp" | "f4v" | "m4v" => {
            serve_transcoded_mp4(&abs, &db_root.root, &headers).await
        }
        _ => serve_file_range(&abs, &headers).await,
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

/// Serve a file with HTTP Range request support (required for video/audio seeking).
async fn serve_file_range(path: &Path, headers: &HeaderMap) -> Response {
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

/// Transcode a video file to H.264/AAC mp4 via ffmpeg and stream it immediately
/// to the client as a fragmented mp4. The output is simultaneously written to a
/// cache file under `<root>/.filetag/cache/video/` so subsequent requests are
/// served instantly with full Range support.
async fn serve_transcoded_mp4(path: &Path, root: &Path, headers: &HeaderMap) -> Response {
    let cache_path = match file_cache_path(path, root, "video", "mp4") {
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

    // Spawn ffmpeg writing a fragmented mp4 to stdout. Fragmented mp4 is playable
    // from the first byte, so the browser starts instantly without waiting for the
    // full transcode to finish.
    let mut child = match tokio::process::Command::new("nice")
        .args(["-n", "10", "ffmpeg"])
        .arg("-i")
        .arg(path)
        .args([
            "-c:v",
            "libx264",
            "-preset",
            "fast",
            "-crf",
            "23",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            // Fragmented mp4: playable from byte 0, no need to wait for moov atom.
            "-movflags",
            "frag_keyframe+empty_moov+default_base_moof",
            "-f",
            "mp4",
            "pipe:1",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return serve_file_range(path, headers).await,
    };

    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => return serve_file_range(path, headers).await,
    };

    let tmp_clone = tmp.clone();
    let cache_clone = cache_path.clone();

    // Use a channel to bridge the background reader task and the HTTP response stream.
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(16);

    tokio::spawn(async move {
        // Hold the permit and keep the child alive for the duration of the stream.
        let _permit = permit;
        let _child = child;

        let mut reader = tokio::io::BufReader::new(stdout);
        let mut cache_file = tokio::fs::File::create(&tmp_clone).await.ok();
        let mut buf = vec![0u8; 64 * 1024];
        let mut ok = true;

        loop {
            match reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = Bytes::copy_from_slice(&buf[..n]);
                    if let Some(ref mut f) = cache_file
                        && f.write_all(&chunk).await.is_err()
                    {
                        ok = false;
                        cache_file = None;
                    }
                    if tx.send(Ok::<Bytes, std::io::Error>(chunk)).await.is_err() {
                        // Client disconnected; abort caching too.
                        ok = false;
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    ok = false;
                    break;
                }
            }
        }

        // Flush and persist the cache file only if the full stream was sent.
        if ok {
            if let Some(mut f) = cache_file {
                let _ = f.flush().await;
                let _ = tokio::fs::rename(&tmp_clone, &cache_clone).await;
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

    // ImageMagick convert (with -auto-orient to respect EXIF orientation)
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
            .kill_on_drop(true)
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

/// Return the cache path for a file thumbnail, keyed by mtime + size.
/// Stored in `<root>/.filetag/cache/thumbs/`.
fn thumb_cache_path(abs: &Path, root: &Path) -> Option<PathBuf> {
    file_cache_path(abs, root, "thumbs", "thumb.jpg")
}

// ---------------------------------------------------------------------------
// Video trickplay thumbnails
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct VThumbsParams {
    path: String,
    root: Option<usize>,
    /// Number of frames (default 8, max 16).
    #[serde(default)]
    n: Option<usize>,
}

/// Return a horizontal sprite sheet (JPEG, N×1 grid) of evenly-spaced frames.
/// One ffmpeg call using `fps=N/duration,scale=320:-2,tile=Nx1`.
/// Cached as a single file in `.filetag/cache/vthumbs/`.
async fn api_vthumbs(
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

    let n = params.n.unwrap_or(8).clamp(2, 16);

    let cache_path = match file_cache_path(&abs, &root, "vthumbs", &format!("sprite{n}x1.jpg")) {
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

        // Re-check now that the permit is held (another task may have generated it).
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

            // Use N separate fast-seek inputs rather than fps+tile, which would
            // require decoding the whole video.  Each -ss before -i is a fast seek.
            // Frame positions: evenly spaced, centred in each N-th slice.
            let positions: Vec<f64> = (0..n)
                .map(|i| info.duration * (i as f64 + 0.5) / n as f64)
                .collect();

            let mut cmd = tokio::process::Command::new("nice");
            cmd.args(["-n", "10", "ffmpeg"]);
            for t in &positions {
                cmd.args(["-ss", &format!("{t:.2}"), "-i"]).arg(&abs);
            }

            // Scale each input, then hstack into a single row.
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
struct PregenParams {
    root: Option<usize>,
}

#[derive(Deserialize)]
struct PregenBody {
    paths: Vec<String>,
}

/// Generate trickplay sprites for a list of video paths in the background.
/// Returns immediately with `{"queued": N}`.  Each path is processed
/// sequentially using the same THUMB_LIMITER semaphore as regular requests.
async fn api_vthumbs_pregen(
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

    // Spawn a background task that works through the list.
    tokio::spawn(async move {
        for rel_path in body.paths {
            let abs = match preview_safe_path(&root, &rel_path) {
                Some(p) => p,
                None => continue,
            };
            let cache_path =
                match file_cache_path(&abs, &root, "vthumbs", &format!("sprite{n}x1.jpg")) {
                    Some(p) => p,
                    None => continue,
                };
            if cache_path.exists() {
                continue; // already done
            }
            // Acquire the semaphore (blocks if another thumb is running).
            let _permit = THUMB_LIMITER.acquire().await;
            if cache_path.exists() {
                continue; // generated while we waited
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

struct VideoInfo {
    duration: f64,
}

/// Get video duration (seconds) via a single ffprobe call.
async fn video_info(path: &Path) -> Option<VideoInfo> {
    let out = tokio::process::Command::new("nice")
        .args(["-n", "10", "ffprobe"])
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
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
    let duration = text.trim().parse::<f64>().ok()?;
    if duration <= 0.0 {
        return None;
    }
    Some(VideoInfo { duration })
}

/// Generate a JPEG contact-sheet thumbnail for a video file.
/// Landscape videos: 2 columns × 3 rows (6 frames).
/// Portrait videos:  3 columns × 2 rows (6 frames).
async fn video_thumb_strip(path: &Path, root: &Path) -> Response {
    // Use "vthumb.jpg" so old contact-sheet caches ("thumb.jpg") are not served.
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

    // Single frame at the same position as sprite frame 0: duration / (2 * N).
    // This ensures the static thumbnail matches what you see when hovering at frame 0.
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
                let _permit = match THUMB_LIMITER.try_acquire() {
                    Ok(p) => p,
                    Err(_) => {
                        return (StatusCode::SERVICE_UNAVAILABLE, "thumbnail queue full")
                            .into_response();
                    }
                };
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
        "mp4" | "webm" | "mov" | "avi" | "mkv" | "wmv" | "flv" | "m4v" | "ts" | "3gp" | "f4v"
        | "mpg" | "mpeg" | "m2v" | "m2ts" | "mts" | "mxf" | "rm" | "rmvb" | "divx" | "vob"
        | "ogv" | "ogg" | "dv" | "asf" | "amv" | "mpe" | "m1v" | "mpv" | "qt" => {
            video_thumb_strip(&abs, &root).await
        }

        // HEIC/HEIF: full-res conversion is already cached; thumbnail via image_thumb_jpeg
        "heic" | "heif" => {
            if let Some(cache) = thumb_cache_path(&abs, &root) {
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
                let _permit = match THUMB_LIMITER.try_acquire() {
                    Ok(p) => p,
                    Err(_) => {
                        return (StatusCode::SERVICE_UNAVAILABLE, "thumbnail queue full")
                            .into_response();
                    }
                };
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
                let _permit = match THUMB_LIMITER.try_acquire() {
                    Ok(p) => p,
                    Err(_) => {
                        return (StatusCode::SERVICE_UNAVAILABLE, "thumbnail queue full")
                            .into_response();
                    }
                };
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

        let _permit = match THUMB_LIMITER.try_acquire() {
            Ok(p) => p,
            Err(_) => {
                return (StatusCode::SERVICE_UNAVAILABLE, "thumbnail queue full").into_response();
            }
        };

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
    // Read sort_order from each root's settings. Fall back to Vec index so
    // roots without an explicit order keep their original position.
    let mut entries: Vec<ApiRoot> = state
        .roots
        .iter()
        .enumerate()
        .map(|(id, r)| {
            let sort_order = Connection::open(&r.db_path)
                .ok()
                .and_then(|c| db::get_setting(&c, "sort_order").ok().flatten())
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(id as i64);
            ApiRoot {
                id,
                name: r.name.clone(),
                path: r.root.display().to_string(),
                sort_order,
                entry_point: r.entry_point,
            }
        })
        .collect();
    entries.sort_by_key(|r| r.sort_order);
    Json(entries)
}

async fn api_reorder_roots(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ReorderRootsRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    for (position, &root_id) in body.order.iter().enumerate() {
        let db_root = root_at(&state, Some(root_id))?;
        let conn = open_conn(db_root)?;
        db::set_setting(&conn, "sort_order", &position.to_string())?;
    }
    Ok(Json(serde_json::json!({ "ok": true })))
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
    // Virtual root: only when there are multiple entry-point roots and no root
    // has been explicitly selected yet.
    let entry_point_roots: Vec<(usize, &DbRoot)> = state
        .roots
        .iter()
        .enumerate()
        .filter(|(_, r)| r.entry_point)
        .collect();
    if params.root.is_none() && params.path.is_empty() && entry_point_roots.len() > 1 {
        // Sort by the persisted sort_order so drag-and-drop reordering is reflected.
        let mut ordered: Vec<(usize, &DbRoot, i64)> = entry_point_roots
            .iter()
            .map(|&(id, r)| {
                let order = Connection::open(&r.db_path)
                    .ok()
                    .and_then(|c| db::get_setting(&c, "sort_order").ok().flatten())
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(id as i64);
                (id, r, order)
            })
            .collect();
        ordered.sort_by_key(|&(_, _, o)| o);
        let entries = ordered
            .iter()
            .map(|&(id, r, _)| ApiDirEntry {
                name: r.name.clone(),
                is_dir: true,
                size: None,
                mtime: None,
                file_count: None,
                tag_count: None,
                root_id: Some(id),
                covered: None,
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
                covered: None,
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
                covered: Some(file_is_covered(&state, &meta, &entry.path())),
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

    // Determine the filesystem path used to locate the database.  For zip
    // entries ("archive.zip::entry") use the zip file itself; for regular
    // files use the file directly.  The path relative to the found database
    // root may differ from params.path when the file lives inside a child
    // database (e.g. params.path = "child/Music/song.mp3" while the child DB
    // stores it as "Music/song.mp3").
    let is_zip = params.path.contains("::");
    let fs_path = if is_zip {
        let zip_part = params.path.split_once("::").unwrap().0;
        db_root.root.join(zip_part)
    } else {
        safe_path(&db_root.root, &params.path)?;
        db_root.root.join(&params.path)
    };

    let start = fs_path.parent().unwrap_or(&fs_path);

    // Walk up from the file's directory to find the most specific database.
    // This correctly handles child databases: a file under child/ is found in
    // child/.filetag/db.sqlite3, not in the parent database.
    let db_lookup = db::find_and_open(start).ok().and_then(|(conn, eff_root)| {
        let eff_rel = if is_zip {
            let zip_rel = db::relative_to_root(&fs_path, &eff_root).ok()?;
            let entry = params.path.split_once("::").unwrap().1;
            Some(format!("{}::{}", zip_rel, entry))
        } else {
            db::relative_to_root(&fs_path, &eff_root).ok()
        };
        eff_rel.map(|r| (conn, r))
    });

    if let Some((conn, effective_rel)) = db_lookup
        && let Some(record) = db::file_by_path(&conn, &effective_rel).map_err(AppError)?
    {
        let tags = db::tags_for_file(&conn, record.id).map_err(AppError)?;
        let indexed_at: String = conn.query_row(
            "SELECT indexed_at FROM files WHERE id = ?1",
            rusqlite::params![record.id],
            |r| r.get(0),
        )?;

        return Ok(Json(ApiFileDetail {
            path: params.path,
            size: record.size,
            file_id: record.file_id,
            mtime: record.mtime_ns,
            indexed_at,
            covered: true, // already indexed — passed device check at index time
            tags: tags
                .into_iter()
                .map(|(name, value)| ApiFileTag { name, value })
                .collect(),
        }));
    }

    // Not found in any database.
    if is_zip {
        // Virtual zip entry not yet in DB — return empty detail
        return Ok(Json(ApiFileDetail {
            path: params.path,
            size: 0,
            file_id: None,
            mtime: 0,
            indexed_at: String::new(),
            covered: true, // zip entries are always under the db root
            tags: vec![],
        }));
    }

    // Regular file not yet indexed: return filesystem metadata.
    let meta =
        std::fs::metadata(&fs_path).with_context(|| format!("reading {}", fs_path.display()))?;
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
        covered: file_is_covered(&state, &meta, &fs_path),
        tags: vec![],
    }))
}

async fn api_tag(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TagRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_at(&state, body.root_id)?;

    // Find the most specific database for this path. For zip entries
    // ("some/archive.zip::entry") the zip file itself determines the db.
    let fs_path = if let Some(zip_path) = body.path.split_once("::").map(|(z, _)| z) {
        db_root.root.join(zip_path)
    } else {
        safe_path(&db_root.root, &body.path)?;
        db_root.root.join(&body.path)
    };
    let start = fs_path.parent().unwrap_or(&fs_path);
    let (conn, effective_root) = db::find_and_open(start).map_err(AppError)?;

    let file_id = if body.path.contains("::") {
        // Store the entry with its full virtual path relative to the effective root.
        let zip_abs = db_root.root.join(body.path.split_once("::").unwrap().0);
        let zip_rel = db::relative_to_root(&zip_abs, &effective_root).map_err(AppError)?;
        let entry_name = body.path.split_once("::").unwrap().1;
        let virtual_path = format!("{}::{}", zip_rel, entry_name);
        ensure_zip_entry_record(&conn, &virtual_path).map_err(AppError)?
    } else {
        let rel = db::relative_to_root(&fs_path, &effective_root).map_err(AppError)?;
        db::get_or_index_file(&conn, &rel, &effective_root)
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

    // Same routing logic as api_tag.
    let fs_path = if let Some(zip_path) = body.path.split_once("::").map(|(z, _)| z) {
        db_root.root.join(zip_path)
    } else {
        safe_path(&db_root.root, &body.path)?;
        db_root.root.join(&body.path)
    };
    let start = fs_path.parent().unwrap_or(&fs_path);
    let (conn, effective_root) = db::find_and_open(start).map_err(AppError)?;

    let effective_rel = if body.path.contains("::") {
        let zip_abs = db_root.root.join(body.path.split_once("::").unwrap().0);
        let zip_rel = db::relative_to_root(&zip_abs, &effective_root).map_err(AppError)?;
        let entry_name = body.path.split_once("::").unwrap().1;
        format!("{}::{}", zip_rel, entry_name)
    } else {
        db::relative_to_root(&fs_path, &effective_root).map_err(AppError)?
    };

    let record = db::file_by_path(&conn, &effective_rel)
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
// ---------------------------------------------------------------------------
// Terminal helpers
// ---------------------------------------------------------------------------

/// Best-effort terminal column width.  Falls back to 80 when unavailable.
/// Reads the `COLUMNS` environment variable (set by most interactive shells).
fn terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(80)
}

// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let root = args.path.unwrap_or_else(|| ".".into());
    let root =
        std::fs::canonicalize(&root).with_context(|| format!("resolving {}", root.display()))?;

    // Open primary database and collect all explicitly linked databases
    // (including automatic ancestor discovery).
    let (conn, root) = db::find_and_open(&root)?;
    let mut all_dbs = db::collect_all_databases(conn, root.clone(), !args.no_parents)?;

    // filetag-web intentionally goes further than the CLI: it also discovers
    // any nested databases by recursively scanning the filesystem under each
    // loaded root. This ensures that browsing, tagging, and searching are
    // always consistent — every sub-tree with its own `.filetag/` database is
    // included, whether or not it was explicitly registered with `filetag db add`.
    //
    // The CLI does not do this because it operates from a specific working
    // directory and only follows explicit links; unexpected databases in
    // sibling or cousin directories would be surprising there. In the web
    // interface the user sees the full directory tree, so every database that
    // is visible should also be searchable.
    {
        // Build the visited set from already-loaded roots so we do not open
        // the same database twice.
        let mut visited: std::collections::HashSet<std::path::PathBuf> = all_dbs
            .iter()
            .filter_map(|db| std::fs::canonicalize(&db.root).ok())
            .collect();

        // Scan under each already-loaded root (ancestors + explicit links).
        // Collect roots first to avoid borrow conflict.
        let scan_roots: Vec<std::path::PathBuf> =
            all_dbs.iter().map(|db| db.root.clone()).collect();

        // Show progress on stderr while scanning; overwrite the same line with \r.
        // The path is truncated to fit within the terminal width (default 80) so
        // the line never wraps — a wrapped line cannot be erased with \r\x1b[K.
        use std::io::Write as _;
        let term_width = terminal_width();
        let mut on_dir = |dir: &std::path::Path| {
            let prefix = "Scanning ";
            let suffix = "...";
            let budget = term_width.saturating_sub(prefix.len() + suffix.len() + 1);
            let path_str = dir.display().to_string();
            let display = if path_str.len() > budget {
                // Keep the tail of the path so the deepest component is visible.
                // Find a valid UTF-8 boundary at or after the target byte offset.
                let tail_bytes = budget.saturating_sub(1);
                let start = path_str.len().saturating_sub(tail_bytes);
                let start = path_str
                    .char_indices()
                    .map(|(i, _)| i)
                    .find(|&i| i >= start)
                    .unwrap_or(path_str.len());
                format!("…{}", &path_str[start..])
            } else {
                path_str
            };
            eprint!("\r\x1b[K{prefix}{display}{suffix}");
            let _ = std::io::stderr().flush();
        };

        for scan_root in &scan_roots {
            let found = db::scan_for_databases(scan_root, &mut visited, 10, &mut on_dir);
            all_dbs.extend(found);
        }

        // Clear the progress line.
        eprint!("\r\x1b[K");
        let _ = std::io::stderr().flush();
    }

    // Sort so that ancestor (shorter path) databases come first. This ensures
    // that when the user launches from a child directory, the topmost ancestor
    // database is the primary root (index 0) in the web interface.
    all_dbs.sort_by_key(|db| db.root.components().count());

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
        .map(|(open_db, name)| {
            #[cfg(unix)]
            let dev = {
                use std::os::unix::fs::MetadataExt;
                std::fs::metadata(&open_db.root).ok().map(|m| m.dev())
            };
            DbRoot {
                name,
                db_path: open_db.root.join(".filetag").join("db.sqlite3"),
                #[cfg(unix)]
                dev,
                entry_point: true, // filled in below
                root: open_db.root,
            }
        })
        .collect();

    // Mark entry points: a root is an entry point only if no other loaded root
    // is a strict ancestor of it. Roots that are subdirectories of another root
    // are still kept for DB routing and tag operations, but are not shown as
    // separate top-level navigation tiles.
    let roots: Vec<DbRoot> = {
        let paths: Vec<PathBuf> = roots.iter().map(|r| r.root.clone()).collect();
        roots
            .into_iter()
            .map(|mut r| {
                let has_ancestor = paths
                    .iter()
                    .any(|other| other != &r.root && r.root.starts_with(other));
                r.entry_point = !has_ancestor;
                r
            })
            .collect()
    };

    if roots.is_empty() {
        anyhow::bail!("no databases found");
    }

    let state = Arc::new(AppState { roots });

    let app = Router::new()
        .route("/", get(index_html))
        .route("/style.css", get(style_css))
        .route("/app.js", get(app_js))
        .route("/favicon.svg", get(favicon))
        .route("/api/roots", get(api_roots))
        .route("/api/roots/reorder", post(api_reorder_roots))
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
        .route("/api/vthumbs", get(api_vthumbs))
        .route("/api/vthumbs/pregenerate", post(api_vthumbs_pregen))
        .with_state(state.clone());

    let addr = format!("{}:{}", args.bind, args.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding to {}", addr))?;

    // Build parent index: for each root, find the closest ancestor root.
    let n = state.roots.len();
    let mut parent_idx: Vec<Option<usize>> = vec![None; n];
    for (i, entry) in parent_idx.iter_mut().enumerate().skip(1) {
        let mut best: Option<usize> = None;
        let mut best_depth = 0usize;
        for j in 0..i {
            let comp = state.roots[j].root.components().count();
            if state.roots[i].root.starts_with(&state.roots[j].root) && comp > best_depth {
                best_depth = comp;
                best = Some(j);
            }
        }
        *entry = best;
    }
    let top_level_count = parent_idx.iter().filter(|p| p.is_none()).count();

    println!("filetag-web at http://{}", addr);
    for i in 0..n {
        // Build ancestor chain from topmost ancestor down to direct parent.
        let mut chain: Vec<usize> = Vec::new();
        let mut cur = i;
        while let Some(p) = parent_idx[cur] {
            chain.push(p);
            cur = p;
        }
        chain.reverse();
        let depth = chain.len();

        // Continuation characters from root down to (but not including) the direct parent.
        let mut prefix = String::new();
        let cont_end = depth.saturating_sub(1);
        for &anc in &chain[..cont_end] {
            let anc_is_last = (anc + 1..n).all(|j| parent_idx[j] != parent_idx[anc]);
            if anc_is_last {
                prefix.push_str("   ");
            } else {
                prefix.push_str("│  ");
            }
        }

        let is_last = (i + 1..n).all(|j| parent_idx[j] != parent_idx[i]);
        let connector = if depth == 0 && top_level_count == 1 {
            ""
        } else if is_last {
            "└─ "
        } else {
            "├─ "
        };

        let label = format!(
            "{} ({})",
            state.roots[i].name,
            state.roots[i].root.display()
        );
        println!("  {}{}{}", prefix, connector, label);
    }
    axum::serve(listener, app).await?;

    Ok(())
}
