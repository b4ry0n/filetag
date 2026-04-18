//! File preview, thumbnail generation, and directory thumbnail sprites.
//!
//! All cache artefacts are written under `<root>/.filetag/cache/` so the
//! data-isolation invariant is maintained.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use crate::extract::{heic_extract_jpeg_thumbnail, raw_embedded_jpeg};
use crate::state::Features;
use crate::state::{AppState, THUMB_LIMITER, load_features_for, resolve_preview, root_for_dir};
use crate::types::DirParam;
use crate::video::{orient_to_vf_prefix, serve_transcoded_mp4, video_thumb_strip};

// ---------------------------------------------------------------------------
// File preview handler
// ---------------------------------------------------------------------------

/// Serve a file for preview, converting RAW / HEIC formats server-side.
pub async fn preview_handler(
    AxumPath(rel_path): AxumPath<String>,
    Query(rp): Query<DirParam>,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let db_root = match root_for_dir(
        &state,
        std::path::Path::new(rp.dir.as_deref().unwrap_or("")),
    ) {
        Some(r) => r,
        None => return (StatusCode::BAD_REQUEST, "Unknown root or missing dir").into_response(),
    };
    let (abs, cache_root) = match resolve_preview(&state, &db_root.root, &rel_path) {
        Some(t) => t,
        None => return (StatusCode::BAD_REQUEST, "Invalid path").into_response(),
    };
    let features = load_features_for(&state, &db_root.root);

    let ext = abs
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "arw" | "cr2" | "cr3" | "nef" | "orf" | "rw2" | "dng" | "raf" | "pef" | "srw" | "raw"
        | "3fr" | "x3f" | "rwl" | "iiq" | "mef" | "mos" | "psd" | "psb" | "xcf" | "ai" | "eps" => {
            preview_raw(&abs, &cache_root, features).await
        }
        "heic" | "heif" => preview_heic(&abs, &cache_root, features).await,
        // Formats browsers cannot decode natively: transcode to mp4 via ffmpeg
        "avi" | "wmv" | "mkv" | "flv" | "mpg" | "mpeg" | "3gp" | "f4v" | "m4v" => {
            if features.video {
                serve_transcoded_mp4(&abs, &cache_root, &headers).await
            } else {
                serve_file_range(&abs, &headers).await
            }
        }
        _ => serve_file_range(&abs, &headers).await,
    }
}

// ---------------------------------------------------------------------------
// MIME type mapping
// ---------------------------------------------------------------------------

/// Return the MIME type string for a given (lowercase) file extension.
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

/// Serve the raw bytes of a file, setting appropriate `Content-Type` headers.
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

/// RAW extensions for which pure-Rust embedded JPEG extraction is attempted.
const RAW_THUMB_EXTS: &[&str] = &[
    "arw", "cr2", "nef", "orf", "dng", "rw2", "pef", "srw", "raf", "raw", "3fr", "erf", "mef",
    "mos", "rwl", "nrw", "kdc",
];

/// Generate a JPEG thumbnail for a RAW file by extracting the embedded preview
/// and resizing it with the `image` crate. Returns `None` when the embedded
/// preview cannot be found or the extension is not in `RAW_THUMB_EXTS`.
async fn raw_thumb_rust(path: &Path) -> Option<Vec<u8>> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    if !RAW_THUMB_EXTS.contains(&ext.as_str()) {
        return None;
    }
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || -> Option<Vec<u8>> {
        let data = std::fs::read(&path).ok()?;
        let jpeg_bytes = raw_embedded_jpeg(&data)?;

        // The embedded preview carries its own EXIF orientation.
        let orient = jpeg_exif_orientation(&jpeg_bytes);
        let img = image::load_from_memory(&jpeg_bytes).ok()?;
        let img = apply_exif_orientation(img, orient);
        let img = img.resize(400, 400, image::imageops::FilterType::Lanczos3);

        let rgb = img.to_rgb8();
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80)
            .encode_image(&rgb)
            .ok()?;
        if out.starts_with(&[0xFF, 0xD8]) {
            Some(out)
        } else {
            None
        }
    })
    .await
    .ok()?
}

// ---------------------------------------------------------------------------
// RAW / PSD / layered format preview
// ---------------------------------------------------------------------------

/// Try to extract a JPEG preview from a RAW file.
/// Attempt order: pure-Rust (TIFF/RAF) → dcraw (imagemagick) → ffmpeg (video) → ImageMagick.
/// Result is cached in `<root>/.filetag/cache/raw/` keyed by mtime+size.
async fn preview_raw(path: &Path, root: &Path, features: Features) -> Response {
    if let Some(cache) = raw_cache_path(path, root) {
        if let Ok(data) = tokio::fs::read(&cache).await {
            return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
        }

        let jpeg = raw_extract_jpeg(path, features).await;
        if let Some(data) = jpeg {
            let _ = tokio::fs::write(&cache, &data).await;
            return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
        }
    } else if let Some(data) = raw_extract_jpeg(path, features).await {
        return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}

/// Inner extraction logic for `preview_raw`: tries extraction methods in order.
/// Pure-Rust paths always run; external tools are gated by `features`.
pub async fn raw_extract_jpeg(path: &Path, features: Features) -> Option<Vec<u8>> {
    // Pure-Rust: parse embedded JPEG from TIFF header or RAF container.
    // Handles NEF, CR2, ARW, ORF, DNG, PEF, SRW, RW2, RAF without any
    // external tools.
    if let Ok(data) = tokio::fs::read(path).await
        && let Some(jpeg) = raw_embedded_jpeg(&data)
    {
        return Some(jpeg);
    }

    // dcraw: extract embedded thumbnail to stdout (fallback for exotic formats)
    if features.imagemagick
        && let Ok(out) = tokio::process::Command::new("dcraw")
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

    // ffmpeg: decode first frame to JPEG
    if features.video
        && let Ok(out) = tokio::process::Command::new("nice")
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
    if features.imagemagick {
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
    }

    None
}

// ---------------------------------------------------------------------------
// HEIC/HEIF preview
// ---------------------------------------------------------------------------

/// Convert HEIC/HEIF to JPEG for browser display.
/// Attempt order: pure-Rust ISOBMFF thumbnail → sips/magick (imagemagick) → ffmpeg (video).
/// Returns 422 with an explanatory message when nothing works.
pub async fn preview_heic(path: &Path, root: &Path, features: Features) -> Response {
    // Pure-Rust: extract embedded JPEG thumbnail from ISOBMFF container.
    // Works without any external tools; yields the thumbnail image stored
    // alongside the primary HEVC item (typically 240–480 px on iPhone files).
    if let Ok(data) = tokio::fs::read(path).await
        && let Some(jpeg) = heic_extract_jpeg_thumbnail(&data)
    {
        return ([(header::CONTENT_TYPE, "image/jpeg")], jpeg).into_response();
    }

    if !features.imagemagick && !features.video {
        return StatusCode::NO_CONTENT.into_response();
    }
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

    // sips (macOS built-in) — requires imagemagick feature (it's a system image tool)
    if features.imagemagick
        && let Ok(out) = tokio::process::Command::new("sips")
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
    if features.video
        && let Ok(out) = tokio::process::Command::new("nice")
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
    if features.imagemagick
        && let Ok(out) = tokio::process::Command::new("convert")
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

    StatusCode::NO_CONTENT.into_response()
}

// ---------------------------------------------------------------------------
// EXIF orientation helpers
// ---------------------------------------------------------------------------

/// Read the EXIF Orientation tag from raw JPEG bytes.
/// Returns the orientation value (1–8), or 1 (normal) if absent or unreadable.
fn jpeg_exif_orientation(data: &[u8]) -> u8 {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return 1;
    }
    let mut pos = 2;
    while pos + 3 < data.len() {
        if data[pos] != 0xFF {
            return 1;
        }
        let marker = data[pos + 1];
        if marker == 0xDA {
            return 1; // start of scan — no APP1 before image data
        }
        let seg_len = ((data[pos + 2] as usize) << 8) | data[pos + 3] as usize;
        if seg_len < 2 || pos + 2 + seg_len > data.len() {
            return 1;
        }
        if marker == 0xE1 {
            let app1 = &data[pos + 4..pos + 2 + seg_len];
            if app1.starts_with(b"Exif\0\0") && app1.len() >= 14 {
                return parse_tiff_orientation(&app1[6..]);
            }
        }
        pos += 2 + seg_len;
    }
    1
}

fn parse_tiff_orientation(tiff: &[u8]) -> u8 {
    if tiff.len() < 8 {
        return 1;
    }
    let le = &tiff[0..2] == b"II";
    let u16_at = |off: usize| -> u16 {
        if off + 2 > tiff.len() {
            return 0;
        }
        if le {
            u16::from_le_bytes([tiff[off], tiff[off + 1]])
        } else {
            u16::from_be_bytes([tiff[off], tiff[off + 1]])
        }
    };
    let u32_at = |off: usize| -> u32 {
        if off + 4 > tiff.len() {
            return 0;
        }
        if le {
            u32::from_le_bytes([tiff[off], tiff[off + 1], tiff[off + 2], tiff[off + 3]])
        } else {
            u32::from_be_bytes([tiff[off], tiff[off + 1], tiff[off + 2], tiff[off + 3]])
        }
    };
    let ifd0 = u32_at(4) as usize;
    if ifd0 + 2 > tiff.len() {
        return 1;
    }
    let nentries = u16_at(ifd0) as usize;
    for i in 0..nentries {
        let e = ifd0 + 2 + i * 12;
        if e + 12 > tiff.len() {
            break;
        }
        if u16_at(e) == 0x0112 {
            // Orientation tag: type SHORT (3), count 1; value in bytes 8–9.
            let v = u16_at(e + 8) as u8;
            return if (1..=8).contains(&v) { v } else { 1 };
        }
    }
    1
}

// ---------------------------------------------------------------------------
// Image thumbnail (resize to max 400 px)
// ---------------------------------------------------------------------------

/// Rotate/flip a `DynamicImage` to match its EXIF orientation tag so that the
/// resulting image is always in "normal" (orientation 1) display order.
fn apply_exif_orientation(img: image::DynamicImage, orient: u8) -> image::DynamicImage {
    match orient {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate90().flipv(),
        8 => img.rotate270(),
        _ => img,
    }
}

/// Extensions handled by the pure-Rust path (`image` crate).
const RUST_THUMB_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "tiff", "tif", "bmp", "gif", "ico",
];

/// Generate a JPEG thumbnail using the pure-Rust `image` crate.
/// Returns `None` for unsupported extensions or on decode failure so the
/// caller can fall back to ImageMagick / ffmpeg.
async fn image_thumb_rust(path: &Path) -> Option<Vec<u8>> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    if !RUST_THUMB_EXTS.contains(&ext.as_str()) {
        return None;
    }
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || -> Option<Vec<u8>> {
        let data = std::fs::read(&path).ok()?;

        // EXIF orientation (JPEG only — PNG/WebP rarely carry EXIF rotation)
        let orient = if matches!(ext.as_str(), "jpg" | "jpeg") {
            jpeg_exif_orientation(&data)
        } else {
            1
        };

        let img = image::load_from_memory(&data).ok()?;
        let img = apply_exif_orientation(img, orient);

        // Resize: fit within 400×400 preserving aspect ratio
        let img = img.resize(400, 400, image::imageops::FilterType::Lanczos3);

        // Encode as JPEG quality 80
        let rgb = img.to_rgb8();
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80)
            .encode_image(&rgb)
            .ok()?;

        if out.starts_with(&[0xFF, 0xD8]) {
            Some(out)
        } else {
            None
        }
    })
    .await
    .ok()?
}

/// Generate a small JPEG thumbnail for any image file.
/// Target: max 400 px on the longest side, quality 80.
///
/// Priority:
/// 1. Pure-Rust path (`image` crate) — fast, no system dependencies, correct EXIF orientation.
/// 2. Pure-Rust RAW path — extracts embedded JPEG from TIFF/RAF containers.
/// 3. ImageMagick (`magick` / `convert`) — handles HEIC and other exotic formats (if enabled).
/// 4. ffmpeg — last resort with manual EXIF orientation correction (if enabled).
pub async fn image_thumb_jpeg(path: &Path, features: Features) -> Option<Vec<u8>> {
    // Fast pure-Rust path for common formats
    if let Some(data) = image_thumb_rust(path).await {
        return Some(data);
    }

    // Pure-Rust RAW path: extract embedded JPEG preview and resize
    if let Some(data) = raw_thumb_rust(path).await {
        return Some(data);
    }

    // Pure-Rust HEIC/HEIF path: extract JPEG thumbnail from ISOBMFF container
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if (ext == "heic" || ext == "heif")
        && let Ok(raw) = tokio::fs::read(path).await
        && let Some(jpeg) = heic_extract_jpeg_thumbnail(&raw)
    {
        // Resize the extracted thumbnail to the standard thumb size.
        let resized = tokio::task::spawn_blocking(move || -> Option<Vec<u8>> {
            let img = image::load_from_memory(&jpeg).ok()?;
            let img = img.resize(400, 400, image::imageops::FilterType::Lanczos3);
            let mut out = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80)
                .encode_image(&img.to_rgb8())
                .ok()?;
            if out.starts_with(&[0xFF, 0xD8]) {
                Some(out)
            } else {
                None
            }
        })
        .await
        .ok()
        .flatten();
        if resized.is_some() {
            return resized;
        }
    }

    if features.imagemagick {
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
    }

    // ffmpeg fallback — read EXIF orientation from the first 64 KiB of the
    // file so we can correct the rotation that ImageMagick would have handled
    // with -auto-orient.
    if features.video {
        let orient = {
            let mut buf = vec![0u8; 65536];
            let n = std::fs::File::open(path)
                .and_then(|mut f| std::io::Read::read(&mut f, &mut buf))
                .unwrap_or(0);
            jpeg_exif_orientation(&buf[..n])
        };
        let vf = format!(
            "{}scale='if(gt(iw,ih),400,-2)':'if(gt(iw,ih),-2,400)':flags=lanczos",
            orient_to_vf_prefix(orient)
        );
        if let Ok(out) = tokio::process::Command::new("nice")
            .args(["-n", "10", "ffmpeg"])
            .args(["-i"])
            .arg(path)
            .args(["-vf"])
            .arg(&vf)
            .args([
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
    }

    None
}

// ---------------------------------------------------------------------------
// sips thumbnail helper (macOS)
// ---------------------------------------------------------------------------

/// Use sips (macOS built-in) to convert `path` to a JPEG thumbnail.
/// `root` is the database root; the temp file goes under `<root>/.filetag/cache/tmp/`.
/// Returns `None` on non-macOS or when sips fails.
#[cfg(target_os = "macos")]
pub async fn sips_thumb_jpeg(path: &Path, root: &Path) -> Option<Vec<u8>> {
    let tmp_dir = root.join(".filetag").join("cache").join("tmp");
    let _ = tokio::fs::create_dir_all(&tmp_dir).await;
    let stem = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = tmp_dir.join(format!("sips_{stem}.jpg"));
    let status = tokio::process::Command::new("sips")
        .args(["-s", "format", "jpeg", "-Z", "400"])
        .arg(path)
        .args(["--out"])
        .arg(&tmp)
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .status()
        .await
        .ok()?;
    if !status.success() {
        let _ = tokio::fs::remove_file(&tmp).await;
        return None;
    }
    let data = tokio::fs::read(&tmp).await.ok()?;
    let _ = tokio::fs::remove_file(&tmp).await;
    if data.starts_with(&[0xFF, 0xD8]) {
        Some(data)
    } else {
        None
    }
}

#[cfg(not(target_os = "macos"))]
pub async fn sips_thumb_jpeg(_path: &Path, _root: &Path) -> Option<Vec<u8>> {
    None
}

// ---------------------------------------------------------------------------
// PDF thumbnail
// ---------------------------------------------------------------------------

/// Generate a JPEG thumbnail for a PDF by rasterising the first page.
/// Tries pdftoppm first (poppler-utils), then ImageMagick+Ghostscript.
/// Temp files are written under `<root>/.filetag/tmp/` per data-isolation rules.
/// Returns `None` immediately when `features.pdf` is disabled.
pub async fn pdf_thumb_jpeg(path: &Path, root: &Path, features: Features) -> Option<Vec<u8>> {
    if !features.pdf {
        return None;
    }
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
    image_thumb_jpeg(path, features).await
}

// ---------------------------------------------------------------------------
// Thumb handler (main thumbnail endpoint)
// ---------------------------------------------------------------------------

/// Thumbnail endpoint — generates a JPEG thumbnail for any previewable file.
pub async fn thumb_handler(
    AxumPath(rel_path): AxumPath<String>,
    Query(rp): Query<DirParam>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let db_root = match root_for_dir(
        &state,
        std::path::Path::new(rp.dir.as_deref().unwrap_or("")),
    ) {
        Some(r) => r,
        None => return (StatusCode::BAD_REQUEST, "Unknown root or missing dir").into_response(),
    };
    let (abs, cache_root) = match resolve_preview(&state, &db_root.root, &rel_path) {
        Some(t) => t,
        None => return (StatusCode::BAD_REQUEST, "Invalid path").into_response(),
    };
    let features = load_features_for(&state, &db_root.root);

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
                        thumb_from_raw_bytes(&img_bytes, abs, features).await
                    } else {
                        None
                    }
                })
            })
            .await
        }

        // Video: single-frame thumbnail (requires video feature)
        "mp4" | "webm" | "mov" | "avi" | "mkv" | "wmv" | "flv" | "m4v" | "ts" | "3gp" | "f4v"
        | "mpg" | "mpeg" | "m2v" | "m2ts" | "mts" | "mxf" | "rm" | "rmvb" | "divx" | "vob"
        | "ogv" | "ogg" | "dv" | "asf" | "amv" | "mpe" | "m1v" | "mpv" | "qt" => {
            if features.video {
                video_thumb_strip(&abs, &cache_root).await
            } else {
                StatusCode::NO_CONTENT.into_response()
            }
        }

        // HEIC/HEIF
        "heic" | "heif" => {
            let root = cache_root.clone();
            thumb_cached(&abs, &cache_root, |abs| {
                Box::pin(async move {
                    if let Some(data) = image_thumb_jpeg(abs, features).await {
                        return Some(data);
                    }
                    // sips fallback: handles HEVC-encoded HEIC (dynamic wallpapers)
                    // that the pure-Rust extractor and ImageMagick miss.
                    if features.imagemagick
                        && let Some(data) = sips_thumb_jpeg(abs, &root).await
                    {
                        return Some(data);
                    }
                    None
                })
            })
            .await
        }

        // RAW / PSD / layered
        "arw" | "cr2" | "cr3" | "nef" | "orf" | "rw2" | "dng" | "raf" | "pef" | "srw" | "raw"
        | "3fr" | "x3f" | "rwl" | "iiq" | "mef" | "mos" | "psd" | "psb" | "xcf" | "ai" | "eps" => {
            thumb_cached(&abs, &cache_root, |abs| {
                Box::pin(async move {
                    if let Some(full_jpeg) = raw_extract_jpeg(abs, features).await {
                        thumb_from_raw_bytes(&full_jpeg, abs, features).await
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
                Box::pin(
                    async move { pdf_thumb_jpeg(abs, abs.parent().unwrap_or(abs), features).await },
                )
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
                if let Some(data) = image_thumb_jpeg(&abs, features).await {
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
    StatusCode::NO_CONTENT.into_response()
}

/// Convert raw image bytes (e.g. from an archive or RAW extraction) into a
/// thumbnail JPEG by writing to a temp file and calling `image_thumb_jpeg`.
/// Falls back to the raw bytes if resizing fails.
async fn thumb_from_raw_bytes(raw_bytes: &[u8], abs: &Path, features: Features) -> Option<Vec<u8>> {
    let root = abs.parent()?;
    let tmp_dir = root.join(".filetag").join("tmp");
    let _ = tokio::fs::create_dir_all(&tmp_dir).await;
    let tmp = tmp_dir.join("thumb_src.jpg");
    if tokio::fs::write(&tmp, raw_bytes).await.is_ok() {
        if let Some(small) = image_thumb_jpeg(&tmp, features).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Some(small);
        }
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    // Fallback: return the raw bytes unchanged
    Some(raw_bytes.to_vec())
}

// ---------------------------------------------------------------------------
// Directory trickplay thumbnails
// ---------------------------------------------------------------------------

/// Extensions that can yield a preview image for a directory collage frame.
const DIR_IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "bmp", "tiff", "tif", "avif", "heic", "heif", "arw",
    "cr2", "cr3", "nef", "orf", "rw2", "dng", "raf", "pef", "srw", "raw", "psd", "psb",
];
const DIR_VIDEO_EXTS: &[&str] = &[
    "mp4", "mov", "avi", "mkv", "wmv", "m4v", "webm", "flv", "mpg", "mpeg", "m2ts", "mts", "ts",
    "3gp",
];

/// List previewable files (flat, no recursion) in `dir`, sorted by name.
fn list_previewable_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            DIR_IMAGE_EXTS.contains(&ext.as_str()) || DIR_VIDEO_EXTS.contains(&ext.as_str())
        })
        .collect();
    files.sort();
    files
}

/// Pick `n` items evenly spread across `items`.
fn pick_evenly<T: Clone>(items: &[T], n: usize) -> Vec<T> {
    if n == 0 || items.is_empty() {
        return Vec::new();
    }
    if items.len() <= n {
        return items.to_vec();
    }
    (0..n)
        .map(|i| {
            let idx = (i * items.len()) / n;
            items[idx].clone()
        })
        .collect()
}

/// Generate a small JPEG for a single directory item (image or video first frame).
/// Target dimensions: 120 × 120 px, square-cropped.
async fn dir_item_jpeg(path: &Path, features: Features) -> Option<Vec<u8>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if DIR_VIDEO_EXTS.contains(&ext.as_str()) {
        if !features.video {
            return None;
        }
        // Extract the first decodable video frame, square-cropped to 120×120.
        let out = tokio::process::Command::new("ffmpeg")
            .arg("-i")
            .arg(path)
            .args([
                "-vf",
                "scale=120:120:force_original_aspect_ratio=increase,crop=120:120",
                "-vframes",
                "1",
                "-q:v",
                "6",
                "-f",
                "image2pipe",
                "-vcodec",
                "mjpeg",
                "-map_metadata",
                "-1",
                "pipe:1",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            .ok()?;
        if out.status.success() && out.stdout.starts_with(&[0xFF, 0xD8]) {
            return Some(out.stdout);
        }
        return None;
    }

    // Images: delegate to the shared thumbnail function (handles RAW, HEIC, etc.).
    image_thumb_jpeg(path, features).await
}

/// Assemble a 2 × 2 JPEG collage (240 × 240 px) from four input images.
///
/// Assembles a 2 × 2 corkboard-style collage (240 × 240 px, white background,
/// tiles slightly rotated and irregularly offset) from four input images.
///
/// Tries ImageMagick `magick` (v7) or `convert` (v6) first, then falls back
/// to an ffmpeg filter graph.
async fn build_2x2_montage(inputs: &[PathBuf; 4], output: &Path) -> bool {
    // Each slot: (rotation_degrees, x_offset, y_offset).
    // Tiles are scaled/cropped to 100×100 before rotation; the rotation
    // enlarges the bounding box by ~4–9 px per side depending on angle.
    // Offsets position the NW corner of each rotated bounding box on the
    // 240×240 canvas, leaving ~10–15 px of breathing room between tiles.
    const SLOTS: [(i32, i64, i64); 4] = [
        (-4, 8, 10),    // top-left,     −4°
        (5, 125, 3),    // top-right,    +5°
        (3, 11, 128),   // bottom-left,  +3°
        (-5, 122, 122), // bottom-right, −5°
    ];

    // ImageMagick compositing: v7 uses `magick`, v6 uses `convert`.
    for cmd_name in &["magick", "convert"] {
        let mut cmd = tokio::process::Command::new(cmd_name);
        // Start with a white canvas.
        cmd.args(["-size", "240x240", "xc:#fefefe"]);
        for (i, (angle, x, y)) in SLOTS.iter().enumerate() {
            cmd.arg("(");
            cmd.arg(&inputs[i]);
            cmd.args([
                "-resize",
                "100x100^",
                "-gravity",
                "Center",
                "-extent",
                "100x100",
                "-background",
                "white",
                "-rotate",
                &angle.to_string(),
            ]);
            cmd.arg(")");
            cmd.args([
                "-gravity",
                "NorthWest",
                "-geometry",
                &format!("+{}+{}", x, y),
                "-composite",
            ]);
        }
        cmd.args(["-quality", "85"]);
        cmd.arg(output);
        let ok = cmd
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false);
        if ok && output.exists() {
            return true;
        }
    }

    // ffmpeg fallback: white canvas, rotated tiles, same positions.
    // Angles in radians; ffmpeg rotate filter uses radians.
    let angle_rads = ["-0.0698", "0.0873", "0.0524", "-0.0873"];
    let offsets = [(8i32, 10i32), (125, 3), (11, 128), (122, 122)];
    let tile_parts: String = (0..4usize)
        .map(|i| {
            let a = angle_rads[i];
            format!(
                "[{i}]scale=100:100:force_original_aspect_ratio=increase,\
                 crop=100:100,rotate={a}:ow=rotw({a}):oh=roth({a}):c=white[f{i}]"
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    let overlay_parts: String = (0..4usize)
        .map(|i| {
            let (x, y) = offsets[i];
            let src = if i == 0 {
                "bg".to_string()
            } else {
                format!("l{}", i - 1)
            };
            let dst = if i == 3 {
                "out".to_string()
            } else {
                format!("l{i}")
            };
            format!("[{src}][f{i}]overlay={x}:{y}[{dst}]")
        })
        .collect::<Vec<_>>()
        .join(";");
    let filter = format!("color=white:size=240x240[bg];{tile_parts};{overlay_parts}");
    let ok = tokio::process::Command::new("ffmpeg")
        .args(["-i", inputs[0].to_str().unwrap_or("")])
        .args(["-i", inputs[1].to_str().unwrap_or("")])
        .args(["-i", inputs[2].to_str().unwrap_or("")])
        .args(["-i", inputs[3].to_str().unwrap_or("")])
        .args(["-filter_complex", &filter])
        .args(["-map", "[out]", "-frames:v", "1", "-q:v", "5", "-y"])
        .arg(output)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);
    ok && output.exists()
}

/// Stitch `frames` side by side into a single JPEG sprite sheet.
///
/// Returns the JPEG bytes of the combined image, or `None` if all tools fail.
async fn stitch_dir_frames(frames: &[PathBuf]) -> Option<Vec<u8>> {
    if frames.is_empty() {
        return None;
    }
    if frames.len() == 1 {
        return tokio::fs::read(&frames[0]).await.ok();
    }
    let n = frames.len();
    let inputs: String = (0..n).map(|i| format!("[{i}]")).collect();
    let filter = format!("{inputs}hstack={n}[out]");
    let mut cmd = tokio::process::Command::new("ffmpeg");
    for f in frames {
        cmd.arg("-i").arg(f);
    }
    let out = cmd
        .args([
            "-filter_complex",
            &filter,
            "-map",
            "[out]",
            "-frames:v",
            "1",
            "-q:v",
            "4",
            "-f",
            "image2pipe",
            "-vcodec",
            "mjpeg",
            "pipe:1",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    if out.status.success() && out.stdout.starts_with(&[0xFF, 0xD8]) {
        Some(out.stdout)
    } else {
        None
    }
}

/// Cache path for a directory sprite sheet, keyed on the directory's mtime.
///
/// Stored under `<root>/.filetag/cache/dir-thumbs/`.  The key includes a path
/// hash so two directories with the same basename and mtime do not collide.
fn dir_thumb_cache_path(dir_abs: &Path, root: &Path) -> Option<PathBuf> {
    let mtime = std::fs::metadata(dir_abs)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let stem = dir_abs
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Short path hash to disambiguate same-name directories.
    let hash = {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut h = DefaultHasher::new();
        dir_abs.hash(&mut h);
        format!("{:016x}", h.finish())
    };
    let key = format!("{mtime}_{hash}_{stem}.sprite.jpg");
    let dir = root.join(".filetag").join("cache").join("dir-thumbs");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(key))
}

/// Query parameters for `GET /api/dir-thumbs`.
#[derive(Deserialize)]
pub struct DirThumbsParams {
    path: String,
    dir: Option<String>,
}

/// `GET /api/dir-thumbs` — return a horizontal JPEG sprite sheet of 240 × 240
/// collage frames for a directory.
///
/// Each frame is a 2 × 2 grid of file thumbnails from the directory.  The
/// sprite sheet contains between 1 and 6 frames depending on how many
/// previewable files are found.  The client animates through frames on hover
/// (same technique as video trickplay).
///
/// Returns 204 when the directory contains fewer than 4 previewable files.
pub async fn api_dir_thumbs(
    Query(params): Query<DirThumbsParams>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let db_root = match root_for_dir(
        &state,
        std::path::Path::new(params.dir.as_deref().unwrap_or("")),
    ) {
        Some(r) => r,
        None => return (StatusCode::BAD_REQUEST, "Unknown root or missing dir").into_response(),
    };
    let features = load_features_for(&state, &db_root.root);

    let abs_dir = match crate::state::preview_safe_path(&db_root.root, &params.path) {
        Some(p) => p,
        None => return (StatusCode::BAD_REQUEST, "Invalid path").into_response(),
    };

    if !abs_dir.is_dir() {
        return (StatusCode::NOT_FOUND, "Not a directory").into_response();
    }

    // Determine the correct cache root for this directory (may be a child DB).
    let cache_root = root_for_dir(&state, &abs_dir)
        .map(|r| r.root.clone())
        .unwrap_or_else(|| db_root.root.clone());

    // Check cache before acquiring the permit.
    if let Some(cache_path) = dir_thumb_cache_path(&abs_dir, &cache_root) {
        if let Ok(data) = tokio::fs::read(&cache_path).await {
            return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
        }

        // --- Build the sprite sheet ---
        let files = list_previewable_files(&abs_dir);
        if files.len() < 4 {
            return StatusCode::NO_CONTENT.into_response();
        }

        let _permit = match THUMB_LIMITER.try_acquire() {
            Ok(p) => p,
            Err(_) => {
                return (StatusCode::SERVICE_UNAVAILABLE, "thumbnail queue full").into_response();
            }
        };

        // Double-check after acquiring the permit.
        if let Ok(data) = tokio::fs::read(&cache_path).await {
            return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
        }

        const IMAGES_PER_FRAME: usize = 4;
        const MAX_FRAMES: usize = 6;
        let selected = pick_evenly(&files, (MAX_FRAMES * IMAGES_PER_FRAME).min(files.len()));

        // Temp directory for intermediate thumbnail + frame files.
        let tmp_dir = cache_root
            .join(".filetag")
            .join("tmp")
            .join(format!("dpt_{}", rand_hex()));
        if tokio::fs::create_dir_all(&tmp_dir).await.is_err() {
            return (StatusCode::INTERNAL_SERVER_ERROR, "tmp dir failed").into_response();
        }

        let mut frame_paths: Vec<PathBuf> = Vec::new();
        for (frame_idx, group) in selected.chunks(IMAGES_PER_FRAME).enumerate() {
            if group.len() < IMAGES_PER_FRAME {
                break; // skip the last incomplete group
            }
            // Generate per-item thumbnails in parallel (bounded by outer permit).
            let mut thumb_paths: Vec<PathBuf> = Vec::new();
            for (item_idx, item_path) in group.iter().enumerate() {
                if let Some(jpeg) = dir_item_jpeg(item_path, features).await {
                    let tp = tmp_dir.join(format!("t{frame_idx}_{item_idx}.jpg"));
                    if tokio::fs::write(&tp, &jpeg).await.is_ok() {
                        thumb_paths.push(tp);
                    }
                }
            }
            if thumb_paths.len() < IMAGES_PER_FRAME {
                continue; // not enough thumbs for a full frame
            }
            let frame_path = tmp_dir.join(format!("frame{frame_idx}.jpg"));
            let inputs: [PathBuf; 4] = [
                thumb_paths[0].clone(),
                thumb_paths[1].clone(),
                thumb_paths[2].clone(),
                thumb_paths[3].clone(),
            ];
            if build_2x2_montage(&inputs, &frame_path).await {
                frame_paths.push(frame_path);
            }
        }

        let result = if frame_paths.is_empty() {
            None
        } else {
            stitch_dir_frames(&frame_paths).await
        };

        let _ = tokio::fs::remove_dir_all(&tmp_dir).await;

        if let Some(data) = result {
            if let Some(parent) = cache_path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let _ = tokio::fs::write(&cache_path, &data).await;
            return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
        }

        return (StatusCode::UNPROCESSABLE_ENTITY, "No previewable files").into_response();
    }

    (StatusCode::INTERNAL_SERVER_ERROR, "cache path unavailable").into_response()
}

/// Return a short hex string based on the current time, used to make temp
/// directory names unique enough to avoid collisions between concurrent requests.
fn rand_hex() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    format!("{t:08x}")
}
