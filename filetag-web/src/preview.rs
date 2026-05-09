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

use crate::extract::{heic_extract_jpeg_thumbnail, raw_embedded_jpeg, raw_tiff_orientation};
use crate::saliency::SalientPoint;
use crate::state::Features;
use crate::state::{AppState, THUMB_LIMITER, load_features_for, resolve_preview, root_for_dir};
use crate::types::DirParam;
use crate::video::{orient_to_vf_prefix, video_thumb_strip};

// ---------------------------------------------------------------------------
// WebP encoding helpers
// ---------------------------------------------------------------------------

/// Encode a `DynamicImage` as lossy WebP at the given quality (0–100).
/// Uses libwebp via the `webp` crate. Returns `None` on encoding failure.
fn encode_lossy_webp(img: &image::DynamicImage, quality: f32) -> Option<Vec<u8>> {
    let rgb = img.to_rgb8();
    let mem = webp::Encoder::from_rgb(rgb.as_raw(), rgb.width(), rgb.height()).encode(quality);
    let bytes: Vec<u8> = mem.to_vec();
    bytes.starts_with(b"RIFF").then_some(bytes)
}

/// Public wrapper for use by `archive.rs`.
pub fn encode_lossy_webp_pub(img: &image::DynamicImage, quality: f32) -> Option<Vec<u8>> {
    encode_lossy_webp(img, quality)
}

/// Encode an `RgbaImage` as **lossy** WebP at quality 80, preserving the alpha channel.
/// `image::ImageFormat::WebP` discards alpha; this helper uses the `webp` crate
/// directly (`WebPEncodeRGBA`) so transparency is retained.
/// Resize-to-fill for grid tiles with orientation-aware gravity.
///
/// Portrait source image into a square or landscape panel: crop from the top
/// ("North" gravity) so that subjects near the top of the frame — faces,
/// heads — are preserved.  All other combinations use centre crop.
fn smart_fill_tile(
    img: &image::DynamicImage,
    pw: u32,
    ph: u32,
    salient: Option<SalientPoint>,
) -> image::RgbaImage {
    if let Some(sp) = salient {
        return salient_crop(img, pw, ph, sp.cx, sp.cy);
    }
    if img.height() > img.width() {
        // Portrait source: crop from top (North gravity) regardless of panel shape,
        // so subjects near the top of the frame (faces, heads) are preserved.
        let scale = pw as f32 / img.width() as f32;
        let new_h = ((img.height() as f32) * scale).ceil() as u32;
        let scaled = img.resize_exact(pw, new_h.max(ph), image::imageops::FilterType::Lanczos3);
        image::imageops::crop_imm(&scaled.to_rgba8(), 0, 0, pw, ph).to_image()
    } else {
        img.resize_to_fill(pw, ph, image::imageops::FilterType::Lanczos3)
            .to_rgba8()
    }
}

/// Crop `img` to `pw`×`ph` centred on the normalised focus point (`cx`, `cy`).
/// Scales the image to cover the panel, then shifts the crop window so the
/// salient point is as close to the centre as possible.
fn salient_crop(img: &image::DynamicImage, pw: u32, ph: u32, cx: f32, cy: f32) -> image::RgbaImage {
    // Scale so the panel is fully covered.
    let scale = (pw as f32 / img.width() as f32).max(ph as f32 / img.height() as f32);
    let sw = (img.width() as f32 * scale).round() as u32;
    let sh = (img.height() as f32 * scale).round() as u32;
    let scaled = img.resize_exact(
        sw.max(pw),
        sh.max(ph),
        image::imageops::FilterType::Lanczos3,
    );
    let sw = scaled.width();
    let sh = scaled.height();

    // Salient pixel in scaled space.
    let px = (cx * sw as f32) as i64;
    let py = (cy * sh as f32) as i64;

    // Place the salient point at 38 % from the top of the panel (rather than
    // dead-centre at 50 %) so there is a little breathing room above the head.
    let anchor_y = (ph as f32 * 0.38) as i64;

    // Crop origin — anchor salient point at 38 % from top, clamped to valid range.
    let ox = (px - pw as i64 / 2).clamp(0, (sw - pw) as i64) as u32;
    let oy = (py - anchor_y).clamp(0, (sh - ph) as i64) as u32;
    image::imageops::crop_imm(&scaled.to_rgba8(), ox, oy, pw, ph).to_image()
}

/// Thumbnail bytes + optional salient point; used as return type for archive
/// spawn_blocking closures to keep the type signature manageable.
type ThumbWithSalient = (Vec<u8>, Option<(f32, f32)>);

fn encode_lossy_webp_rgba(canvas: &image::RgbaImage) -> Option<Vec<u8>> {
    let (w, h) = canvas.dimensions();
    let mem = webp::Encoder::from_rgba(canvas.as_raw(), w, h).encode(80.0);
    let bytes: Vec<u8> = mem.to_vec();
    bytes.starts_with(b"RIFF").then_some(bytes)
}

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
        // TIFF: browsers cannot render this natively; convert to JPEG on the fly
        // (cached under .filetag/cache/tiff-preview/).
        "tiff" | "tif" => {
            let cache = file_cache_path(&abs, &cache_root, "tiff-preview", "preview.jpg");
            if let Some(Ok(data)) = cache.as_ref().map(std::fs::read) {
                return ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response();
            }
            let path2 = abs.clone();
            let result = tokio::task::spawn_blocking(move || -> Option<Vec<u8>> {
                let data = std::fs::read(&path2).ok()?;
                let img = image::load_from_memory(&data).ok()?;
                // Downscale to max 2560 px on the longest side for web display.
                // Full-resolution TIFFs can be tens of MB; 2560 px is enough for
                // any screen and keeps the JPEG well under 1 MB.
                const MAX_PX: u32 = 2560;
                let img = if img.width() > MAX_PX || img.height() > MAX_PX {
                    img.resize(MAX_PX, MAX_PX, image::imageops::FilterType::Lanczos3)
                } else {
                    img
                };
                let rgb = img.to_rgb8();
                let mut out = Vec::new();
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
                    .encode_image(&rgb)
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

            match result {
                Some(data) => {
                    if let Some(p) = cache {
                        let _ = tokio::fs::write(p, &data).await;
                    }
                    ([(header::CONTENT_TYPE, "image/jpeg")], data).into_response()
                }
                None => StatusCode::NO_CONTENT.into_response(),
            }
        }
        "arw" | "cr2" | "cr3" | "nef" | "orf" | "rw2" | "dng" | "raf" | "pef" | "srw" | "raw"
        | "3fr" | "x3f" | "rwl" | "iiq" | "mef" | "mos" | "psd" | "psb" | "xcf" | "ai" | "eps" => {
            preview_raw(&abs, &cache_root, features).await
        }
        "heic" | "heif" => preview_heic(&abs, &cache_root, features).await,
        // Formats browsers cannot decode natively: transcode to mp4 via ffmpeg
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
    file_cache_path(abs, root, "thumbs", "thumb.webp")
}

/// Return the sidecar path for a cached salient point, keyed by the same
/// mtime + size as the corresponding thumbnail.
/// Stored in `<root>/.filetag/cache/thumbs/` with a `.sp` suffix.
fn salient_cache_path(abs: &Path, root: &Path) -> Option<PathBuf> {
    file_cache_path(abs, root, "thumbs", "thumb.sp")
}

/// Read a cached salient point from disk.
///
/// - `Some(Some((cx, cy)))` — a salient point was previously detected.
/// - `Some(None)`           — detection ran but found nothing; don't retry.
/// - `None`                 — not computed yet.
fn read_salient_cache(sp_path: &Path) -> Option<Option<(f32, f32)>> {
    let text = std::fs::read_to_string(sp_path).ok()?;
    let text = text.trim();
    if text == "-" {
        return Some(None);
    }
    let mut parts = text.splitn(2, ',');
    let cx: f32 = parts.next()?.parse().ok()?;
    let cy: f32 = parts.next()?.parse().ok()?;
    Some(Some((cx, cy)))
}

/// Public wrapper for use by `archive.rs`.
pub fn read_salient_cache_pub(sp_path: &Path) -> Option<Option<(f32, f32)>> {
    read_salient_cache(sp_path)
}

/// Persist a salient point (or the "nothing detected" sentinel) to disk.
fn write_salient_cache(sp_path: &Path, salient: Option<(f32, f32)>) {
    let text = match salient {
        Some((cx, cy)) => format!("{cx},{cy}"),
        None => "-".to_string(),
    };
    let _ = std::fs::write(sp_path, text);
}

/// Public wrapper for use by `archive.rs`.
pub fn write_salient_cache_pub(sp_path: &Path, salient: Option<(f32, f32)>) {
    write_salient_cache(sp_path, salient);
}

/// Attach `X-Salient-Cx` / `X-Salient-Cy` headers to an existing response.
fn attach_salient_headers(mut resp: Response, salient: Option<(f32, f32)>) -> Response {
    if let Some((cx, cy)) = salient {
        let hdrs = resp.headers_mut();
        if let Ok(v) = axum::http::HeaderValue::try_from(format!("{cx:.4}")) {
            hdrs.insert(axum::http::HeaderName::from_static("x-salient-cx"), v);
        }
        if let Ok(v) = axum::http::HeaderValue::try_from(format!("{cy:.4}")) {
            hdrs.insert(axum::http::HeaderName::from_static("x-salient-cy"), v);
        }
    }
    resp
}

/// Public wrapper for use by `archive.rs`.
pub fn attach_salient_headers_pub(resp: Response, salient: Option<(f32, f32)>) -> Response {
    attach_salient_headers(resp, salient)
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

        // Use the outer TIFF IFD0 orientation (tag 0x0112) as the authoritative
        // rotation, because embedded preview JPEGs in RAW files (e.g. Sony ARW)
        // are often stored in native sensor orientation without their own EXIF
        // orientation tag. Fall back to the JPEG EXIF only when IFD0 says 1.
        let tiff_orient = raw_tiff_orientation(&data);
        let jpeg_orient = jpeg_exif_orientation(&jpeg_bytes);
        let orient = if tiff_orient != 1 {
            tiff_orient
        } else {
            jpeg_orient
        };
        let img = image::load_from_memory(&jpeg_bytes).ok()?;
        let img = apply_exif_orientation(img, orient);
        let img = img.resize(400, 400, image::imageops::FilterType::Lanczos3);

        encode_lossy_webp(&img, 80.0)
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

        // Encode as lossy WebP
        encode_lossy_webp(&img, 80.0)
    })
    .await
    .ok()?
}

/// Generate a small WebP thumbnail for any image file.
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
            encode_lossy_webp(&img, 80.0)
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
                    "webp:-",
                ])
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .output()
                .await
                && out.status.success()
                && out.stdout.starts_with(b"RIFF")
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
            // Re-encode ffmpeg JPEG as lossy WebP.
            let jpeg = out.stdout;
            if let Some(webp) = tokio::task::spawn_blocking(move || {
                let img = image::load_from_memory(&jpeg).ok()?;
                encode_lossy_webp(&img, 80.0)
            })
            .await
            .ok()
            .flatten()
            {
                return Some(webp);
            }
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
    if !data.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    // Re-encode JPEG from sips as lossy WebP.
    tokio::task::spawn_blocking(move || {
        let img = image::load_from_memory(&data).ok()?;
        encode_lossy_webp(&img, 80.0)
    })
    .await
    .ok()
    .flatten()
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
/// Temp files are written under `<root>/.filetag/cache/tmp/` per data-isolation rules.
/// Returns `None` immediately when `features.pdf` is disabled.
pub async fn pdf_thumb_jpeg(path: &Path, root: &Path, features: Features) -> Option<Vec<u8>> {
    if !features.pdf {
        return None;
    }
    let tmp_dir = root.join(".filetag").join("cache").join("tmp");
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
            // Re-encode pdftoppm JPEG as lossy WebP.
            if let Some(webp) = tokio::task::spawn_blocking(move || {
                let img = image::load_from_memory(&data).ok()?;
                encode_lossy_webp(&img, 80.0)
            })
            .await
            .ok()
            .flatten()
            {
                return Some(webp);
            }
        }
    }
    let _ = tokio::fs::remove_file(&expected).await;

    // Fallback: ImageMagick (requires Ghostscript for PDF rasterisation)
    image_thumb_jpeg(path, features).await
}

// ---------------------------------------------------------------------------
// Thumb handler (main thumbnail endpoint)
// ---------------------------------------------------------------------------

/// Generate and cache a thumbnail for a single entry inside an archive.
///
/// Cache key is derived from the zip file's mtime + size + entry name so
/// thumbnails are invalidated automatically when the archive changes.
async fn thumb_archive_entry(
    zip_abs: &Path,
    entry_name: &str,
    root: &Path,
    features: Features,
) -> Response {
    // Build stable cache paths: <root>/.filetag/cache/thumbs/<mtime>_<size>_<slug>.thumb.{webp,sp}
    let (cache_path, sp_path): (Option<PathBuf>, Option<PathBuf>) = (|| {
        let meta = std::fs::metadata(zip_abs).ok()?;
        let mtime = meta
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs();
        let size = meta.len();
        // Sanitise entry name for use as a filename: replace path separators.
        let slug = entry_name.replace(['/', '\\', ':'], "_");
        let dir = root.join(".filetag").join("cache").join("thumbs");
        std::fs::create_dir_all(&dir).ok()?;
        Some((
            dir.join(format!("{mtime}_{size}_{slug}.thumb.webp")),
            dir.join(format!("{mtime}_{size}_{slug}.thumb.sp")),
        ))
    })()
    .unzip();

    // Serve from cache if available.
    if let Some(ref p) = cache_path
        && let Ok(data) = tokio::fs::read(p).await
    {
        let salient = if features.saliency_pose {
            let cached_sp = sp_path.as_ref().and_then(|s| read_salient_cache(s));
            if cached_sp.is_none()
                && crate::saliency::pose_model_ready()
                && let Some(sp) = sp_path.clone()
            {
                let data2 = data.clone();
                tokio::spawn(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        let img = image::load_from_memory(&data2).ok()?;
                        crate::saliency::detect_salient_point(&img, false)
                    })
                    .await
                    .ok()
                    .flatten();
                    write_salient_cache(&sp, result.map(|s| (s.cx, s.cy)));
                });
            }
            cached_sp.flatten()
        } else {
            None
        };
        let resp = ([(header::CONTENT_TYPE, "image/webp")], data).into_response();
        return attach_salient_headers(resp, salient);
    }

    let _permit = match THUMB_LIMITER.try_acquire() {
        Ok(p) => p,
        Err(_) => {
            return (StatusCode::ACCEPTED, "thumbnail queue full").into_response();
        }
    };

    let zip_abs = zip_abs.to_path_buf();
    let entry_name = entry_name.to_string();
    let use_saliency = features.saliency_pose && crate::saliency::pose_model_ready();

    let result = tokio::task::spawn_blocking(move || -> Option<ThumbWithSalient> {
        let (bytes, _) = crate::archive::archive_read_entry(&zip_abs, &entry_name).ok()?;
        let img = image::load_from_memory(&bytes).ok()?;
        // Respect EXIF orientation for JPEG entries.
        let orient = if entry_name.to_lowercase().ends_with(".jpg")
            || entry_name.to_lowercase().ends_with(".jpeg")
        {
            jpeg_exif_orientation(&bytes)
        } else {
            1
        };
        let img = apply_exif_orientation(img, orient);
        // Run salient detection on the full image before downscaling.
        let salient = if use_saliency {
            crate::saliency::detect_salient_point(&img, false).map(|s| (s.cx, s.cy))
        } else {
            None
        };
        let img = img.resize(400, 400, image::imageops::FilterType::Lanczos3);
        Some((encode_lossy_webp(&img, 80.0)?, salient))
    })
    .await
    .ok()
    .flatten();

    match result {
        Some((data, salient)) => {
            if let Some(ref p) = cache_path {
                let _ = tokio::fs::write(p, &data).await;
            }
            if let Some(ref sp) = sp_path {
                write_salient_cache(sp, salient);
            }
            let resp = ([(header::CONTENT_TYPE, "image/webp")], data).into_response();
            attach_salient_headers(resp, salient)
        }
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

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

    // Virtual archive entry path (e.g. `archive.cbz::subdir/image.jpg`).
    // The `::` separator is not a filesystem path separator, so we must handle
    // it before calling `resolve_preview` (which would fail to canonicalize it).
    if let Some(sep) = rel_path.find("::") {
        let zip_rel = &rel_path[..sep];
        let entry_name = &rel_path[sep + 2..];
        let zip_abs = match crate::state::preview_safe_path(&db_root.root, zip_rel) {
            Some(p) => p,
            None => return (StatusCode::BAD_REQUEST, "Invalid archive path").into_response(),
        };
        let features = load_features_for(&state, &db_root.root);
        return thumb_archive_entry(&zip_abs, entry_name, &db_root.root, features).await;
    }

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
            if let Some(cache) = thumb_cache_path(&abs, &cache_root) {
                let sp_path = salient_cache_path(&abs, &cache_root);
                if let Ok(data) = tokio::fs::read(&cache).await {
                    // Already cached — attach salient headers if available.
                    let salient = if features.saliency_pose {
                        let cached_sp = sp_path.as_ref().and_then(|p| read_salient_cache(p));
                        if cached_sp.is_none()
                            && crate::saliency::pose_model_ready()
                            && let Some(sp) = sp_path.clone()
                        {
                            let data2 = data.clone();
                            tokio::spawn(async move {
                                let result = tokio::task::spawn_blocking(move || {
                                    let img = image::load_from_memory(&data2).ok()?;
                                    crate::saliency::detect_salient_point(&img, false)
                                })
                                .await
                                .ok()
                                .flatten();
                                write_salient_cache(&sp, result.map(|s| (s.cx, s.cy)));
                            });
                        }
                        cached_sp.flatten()
                    } else {
                        None
                    };
                    let resp = ([(header::CONTENT_TYPE, "image/webp")], data).into_response();
                    return attach_salient_headers(resp, salient);
                }
                let _permit = match THUMB_LIMITER.try_acquire() {
                    Ok(p) => p,
                    Err(_) => {
                        return (StatusCode::ACCEPTED, "thumbnail queue full").into_response();
                    }
                };
                let abs2 = abs.clone();
                let root = cache_root.clone();
                let use_saliency = features.saliency_pose && crate::saliency::pose_model_ready();
                // Fetch cover image bytes; run salient detection before downscaling.
                let cover =
                    tokio::task::spawn_blocking(move || crate::archive::archive_cover_image(&abs2))
                        .await;
                if let Ok(Ok(img_bytes)) = cover {
                    let salient = if use_saliency {
                        let img_bytes2 = img_bytes.clone();
                        tokio::task::spawn_blocking(move || {
                            let img = image::load_from_memory(&img_bytes2).ok()?;
                            crate::saliency::detect_salient_point(&img, false)
                        })
                        .await
                        .ok()
                        .flatten()
                        .map(|s| (s.cx, s.cy))
                    } else {
                        None
                    };
                    if let Some(data) = thumb_from_raw_bytes(&img_bytes, &root, features).await {
                        let _ = tokio::fs::write(&cache, &data).await;
                        if let Some(ref sp) = sp_path {
                            write_salient_cache(sp, salient);
                        }
                        let resp = ([(header::CONTENT_TYPE, "image/webp")], data).into_response();
                        return attach_salient_headers(resp, salient);
                    }
                }
            }
            StatusCode::NO_CONTENT.into_response()
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
            let root = cache_root.clone();
            thumb_cached(&abs, &cache_root, move |abs| {
                Box::pin(async move {
                    // raw_thumb_rust reads TIFF IFD0 orientation (tag 0x0112) correctly.
                    // Embedded JPEG previews in TIFF-family RAW files (ARW, NEF, CR2, …)
                    // are stored in native sensor orientation without their own EXIF tag,
                    // so we must take the orientation from the outer TIFF container.
                    if let Some(data) = raw_thumb_rust(abs).await {
                        return Some(data);
                    }
                    // Fallback for formats not in RAW_THUMB_EXTS (PSD, XCF, CR3, EPS, …)
                    if let Some(full_jpeg) = raw_extract_jpeg(abs, features).await {
                        thumb_from_raw_bytes(&full_jpeg, &root, features).await
                    } else {
                        None
                    }
                })
            })
            .await
        }

        // PDF
        "pdf" => {
            let root = cache_root.clone();
            thumb_cached(&abs, &cache_root, move |abs| {
                Box::pin(async move { pdf_thumb_jpeg(abs, &root, features).await })
            })
            .await
        }

        // Regular images (JPEG, PNG, WEBP, …)
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tiff" | "tif" | "avif" => {
            if let Some(cache) = thumb_cache_path(&abs, &cache_root) {
                let sp_path = salient_cache_path(&abs, &cache_root);
                if let Ok(data) = tokio::fs::read(&cache).await {
                    // Thumbnail already cached.  Read the salient sidecar when
                    // saliency is enabled; if not yet computed, schedule a
                    // background task so the next request will have the header.
                    let salient = if features.saliency_pose {
                        let cached_sp = sp_path.as_ref().and_then(|p| read_salient_cache(p));
                        if cached_sp.is_none() && crate::saliency::pose_model_ready() {
                            // Not computed yet — kick off a background task.
                            if let Some(sp) = sp_path.clone() {
                                let abs2 = abs.clone();
                                tokio::spawn(async move {
                                    let result = tokio::task::spawn_blocking(move || {
                                        let d = std::fs::read(&abs2).ok()?;
                                        let img = image::load_from_memory(&d).ok()?;
                                        crate::saliency::detect_salient_point(&img, false)
                                    })
                                    .await
                                    .ok()
                                    .flatten();
                                    write_salient_cache(&sp, result.map(|s| (s.cx, s.cy)));
                                });
                            }
                        }
                        cached_sp.flatten()
                    } else {
                        None
                    };
                    let resp = ([(header::CONTENT_TYPE, "image/webp")], data).into_response();
                    return attach_salient_headers(resp, salient);
                }
                let _permit = match THUMB_LIMITER.try_acquire() {
                    Ok(p) => p,
                    Err(_) => {
                        return (StatusCode::ACCEPTED, "thumbnail queue full").into_response();
                    }
                };
                if let Some(data) = image_thumb_jpeg(&abs, features).await {
                    let _ = tokio::fs::write(&cache, &data).await;
                    // Compute salient point in the same request so the first
                    // response already carries the correct header.
                    let salient = if features.saliency_pose && crate::saliency::pose_model_ready() {
                        let abs2 = abs.clone();
                        let result = tokio::task::spawn_blocking(move || {
                            let d = std::fs::read(&abs2).ok()?;
                            let img = image::load_from_memory(&d).ok()?;
                            crate::saliency::detect_salient_point(&img, false)
                        })
                        .await
                        .ok()
                        .flatten();
                        if let Some(sp) = sp_path {
                            write_salient_cache(&sp, result.map(|s| (s.cx, s.cy)));
                        }
                        result.map(|s| (s.cx, s.cy))
                    } else {
                        None
                    };
                    let resp = ([(header::CONTENT_TYPE, "image/webp")], data).into_response();
                    return attach_salient_headers(resp, salient);
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
/// callback, write cache, serve WebP.
async fn thumb_cached<F>(abs: &Path, root: &Path, generate: F) -> Response
where
    F: FnOnce(&Path) -> Pin<Box<dyn Future<Output = Option<Vec<u8>>> + Send + '_>>,
{
    if let Some(cache) = thumb_cache_path(abs, root) {
        if let Ok(data) = tokio::fs::read(&cache).await {
            return ([(header::CONTENT_TYPE, "image/webp")], data).into_response();
        }
        let _permit = match THUMB_LIMITER.try_acquire() {
            Ok(p) => p,
            Err(_) => {
                return (StatusCode::ACCEPTED, "thumbnail queue full").into_response();
            }
        };
        if let Some(data) = generate(abs).await {
            let _ = tokio::fs::write(&cache, &data).await;
            return ([(header::CONTENT_TYPE, "image/webp")], data).into_response();
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

/// Convert raw image bytes (e.g. from an archive or RAW extraction) into a
/// thumbnail JPEG by writing to a temp file and calling `image_thumb_jpeg`.
/// Falls back to the raw bytes if resizing fails.
async fn thumb_from_raw_bytes(
    raw_bytes: &[u8],
    _cache_root: &Path,
    _features: Features,
) -> Option<Vec<u8>> {
    let raw_bytes = raw_bytes.to_vec();
    tokio::task::spawn_blocking(move || -> Option<Vec<u8>> {
        let img = image::load_from_memory(&raw_bytes).ok()?;
        let img = img.resize(400, 400, image::imageops::FilterType::Lanczos3);
        encode_lossy_webp(&img, 80.0)
    })
    .await
    .ok()
    .flatten()
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
const DIR_PDF_EXTS: &[&str] = &["pdf"];
const DIR_ARCHIVE_EXTS: &[&str] = &["zip", "cbz", "rar", "cbr", "7z", "cb7"];

/// List previewable files in `dir`, sorted by path.
///
/// Direct children are scanned first, then subdirectories up to a small bounded
/// depth so folders that only contain album/chapter subfolders still get a
/// preview without walking very large trees indefinitely.
fn list_previewable_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_file() && !is_ignored_dir_preview_file(&path) && is_dir_preview_candidate(&path)
        {
            files.push(path);
        }
    }
    // Sorteer: gewone bestanden/archieven vóór directories (voor batch preview)
    files.sort_by(|a, b| {
        let a_is_dir = a.is_dir();
        let b_is_dir = b.is_dir();
        match (a_is_dir, b_is_dir) {
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            _ => a.cmp(b),
        }
    });
    files
}

// Recursieve variant verwijderd: alleen directe kinderen worden nu meegenomen

fn is_dir_preview_candidate(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    DIR_IMAGE_EXTS.contains(&ext.as_str())
        || DIR_VIDEO_EXTS.contains(&ext.as_str())
        || DIR_PDF_EXTS.contains(&ext.as_str())
        || DIR_ARCHIVE_EXTS.contains(&ext.as_str())
}

fn is_ignored_dir_preview_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| name == ".DS_Store" || name.starts_with("._"))
}

/// Well-known cover image stems, checked case-insensitively.
const COVER_STEMS: &[&str] = &[
    "cover",
    "folder",
    "poster",
    "thumb",
    "thumbnail",
    "front",
    "albumart",
    "artwork",
    "album",
];
const COVER_IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "avif", "gif", "bmp"];

/// Find a cover image in `dir` by checking well-known stems and extensions
/// (case-insensitive).  Plain names are tried before hidden (dot-prefixed) ones.
///
/// Also matches numeric prefixes used to force sort order in other applications,
/// e.g. `00_cover.jpg`, `01-folder.png`, `0cover.jpg`.
fn find_cover_image(dir: &Path) -> Option<PathBuf> {
    let entries: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().collect();

    /// Returns true when `name` (already lowercased, extension stripped) matches
    /// `stem`, optionally preceded by leading digits and an optional separator
    /// character (`_`, `-`, ` `).
    fn stem_matches(name_stem: &str, stem: &str) -> bool {
        if name_stem == stem {
            return true;
        }
        // Strip leading ASCII digits, then an optional single separator.
        let after_digits = name_stem.trim_start_matches(|c: char| c.is_ascii_digit());
        if after_digits == name_stem {
            return false; // no leading digits
        }
        let after_sep = after_digits
            .strip_prefix(['_', '-', ' '])
            .unwrap_or(after_digits);
        after_sep == stem
    }

    for hidden in [false, true] {
        for stem in COVER_STEMS {
            for ext in COVER_IMAGE_EXTS {
                for entry in &entries {
                    let Some(fname) = entry.file_name().into_string().ok() else {
                        continue;
                    };
                    let lower = fname.to_lowercase();
                    // Check extension matches.
                    let Some(name_stem) = lower.strip_suffix(&format!(".{ext}")) else {
                        continue;
                    };
                    // Check hidden/plain prefix.
                    let candidate_stem = if hidden {
                        name_stem.strip_prefix('.')?
                    } else {
                        if name_stem.starts_with('.') {
                            continue;
                        }
                        name_stem
                    };
                    if stem_matches(candidate_stem, stem) && entry.path().is_file() {
                        return Some(entry.path());
                    }
                }
            }
        }
    }
    None
}

/// Build a 240×240 "cover frame" for a directory preview.
///
/// The cover image is scaled to fill the full 240×240 canvas (square crop),
/// then a blurred+darkened version is used as background and the
/// aspect-ratio-preserved image is centred on top.  This avoids
/// transparent letterbox bars showing the card background colour.
async fn build_cover_frame(cover: &Path, output: &Path) -> bool {
    // --- ImageMagick path ---
    // Layer 0: blurred+darkened full-bleed background.
    // Layer 1: aspect-ratio-fit foreground, centred.
    for cmd_name in &["magick", "convert"] {
        let ok = tokio::process::Command::new(cmd_name)
            .arg(cover)
            // background layer: fill 240×240, blur, darken
            .args([
                "-resize", "240x240^", "-gravity", "Center", "-extent", "240x240",
            ])
            .args(["-blur", "0x8", "-brightness-contrast", "-25"])
            .arg("(")
            .arg(cover)
            // foreground layer: fit in 220×220, preserve aspect ratio
            .args([
                "-resize",
                "220x220",
                "-background",
                "none",
                "-gravity",
                "Center",
                "-extent",
                "220x220",
            ])
            .arg(")")
            .args(["-gravity", "Center", "-composite"])
            .arg(output)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false);
        if ok
            && output.exists()
            && let Ok(bytes) = std::fs::read(output)
            && (image::load_from_memory_with_format(&bytes, image::ImageFormat::WebP).is_ok()
                || bytes.starts_with(b"\x89PNG"))
        {
            return true;
        }
        if output.exists() {
            let _ = std::fs::remove_file(output);
        }
    }

    // --- Rust fallback ---
    let cover = cover.to_path_buf();
    let output = output.to_path_buf();
    tokio::task::spawn_blocking(move || build_cover_frame_rust(&cover, &output))
        .await
        .unwrap_or(false)
}

fn build_cover_frame_rust(cover: &Path, output: &Path) -> bool {
    let Ok(data) = std::fs::read(cover) else {
        return false;
    };
    let Ok(img) = image::load_from_memory(&data) else {
        return false;
    };

    const CANVAS: u32 = 240;

    // Background layer: full-bleed square crop, blurred and darkened.
    let bg_base = img.resize_to_fill(CANVAS, CANVAS, image::imageops::FilterType::Lanczos3);
    let bg_blurred = image::imageops::blur(&bg_base.to_rgba8(), 8.0);
    // Darken by multiplying each channel by ~0.55
    let bg_dark: image::RgbaImage = image::RgbaImage::from_fn(CANVAS, CANVAS, |x, y| {
        let p = bg_blurred.get_pixel(x, y);
        image::Rgba([
            (p[0] as f32 * 0.55) as u8,
            (p[1] as f32 * 0.55) as u8,
            (p[2] as f32 * 0.55) as u8,
            p[3],
        ])
    });

    // Foreground layer: aspect-ratio fit, centred.
    let scaled = img.resize(220, 220, image::imageops::FilterType::Lanczos3);
    let sw = scaled.width();
    let sh = scaled.height();
    let ox = (CANVAS - sw) / 2;
    let oy = (CANVAS - sh) / 2;

    let mut canvas = bg_dark;
    image::imageops::overlay(&mut canvas, &scaled.to_rgba8(), ox as i64, oy as i64);

    let Some(bytes) = encode_lossy_webp_rgba(&canvas) else {
        return false;
    };
    std::fs::write(output, bytes).is_ok()
}

/// Return candidate indices in expanding even samples. This keeps the first
/// frames representative, but still falls back to more files when early
/// candidates cannot actually produce thumbnails.
fn preview_candidate_order(len: usize, target: usize) -> Vec<usize> {
    if len == 0 || target == 0 {
        return Vec::new();
    }
    let mut order = Vec::new();
    let mut sample = target.min(len).max(1);
    loop {
        for i in 0..sample {
            let idx = (i * len) / sample;
            if !order.contains(&idx) {
                order.push(idx);
            }
        }
        if sample >= len {
            break;
        }
        sample = (sample * 2).min(len);
    }
    order
}

/// Generate a small JPEG for a single directory item (image, video, or PDF).
/// Target dimensions: 120 × 120 px, square-cropped when `preserve_aspect` is
/// `false` (crop style).  When `preserve_aspect` is `true` (fit/scattered
/// style), the full image is returned un-cropped so the collage builder can
/// apply area-normalisation while keeping the original proportions.
async fn dir_item_jpeg(
    path: &Path,
    root: &Path,
    features: Features,
    preserve_aspect: bool,
) -> Option<Vec<u8>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if ext == "pdf" {
        return pdf_thumb_jpeg(path, root, features).await;
    }

    if DIR_ARCHIVE_EXTS.contains(&ext.as_str()) {
        let path = path.to_path_buf();
        let bytes =
            tokio::task::spawn_blocking(move || crate::archive::archive_cover_image(&path).ok())
                .await
                .ok()
                .flatten()?;
        // Resize the cover: square-crop for "crop" style, fit-in-box for
        // "fit"/"scattered" styles so the collage builder can preserve the
        // aspect ratio.
        let img = image::load_from_memory(&bytes).ok()?;
        let thumb = if preserve_aspect {
            img.resize(240, 240, image::imageops::FilterType::Lanczos3)
        } else {
            img.resize_to_fill(120, 120, image::imageops::FilterType::Lanczos3)
        };
        let mut out = std::io::Cursor::new(Vec::new());
        thumb.write_to(&mut out, image::ImageFormat::Jpeg).ok()?;
        return Some(out.into_inner());
    }

    if DIR_VIDEO_EXTS.contains(&ext.as_str()) {
        if !features.video {
            return None;
        }
        // Seek to an interesting frame rather than frame 1 (which is often a
        // title card or black leader).  Strategy: 15% into the video, clamped
        // to [5 s, 300 s].  For videos shorter than 5 s we use the midpoint.
        let seek_secs: f64 = if let Some(info) = crate::video::video_info(path).await {
            let dur = info.duration;
            if dur <= 5.0 {
                dur * 0.5
            } else {
                (dur * 0.15).clamp(5.0, 300.0)
            }
        } else {
            0.0
        };

        // Extract the first decodable video frame.  For the crop style the
        // frame is square-cropped to 120×120; for fit/scattered styles the
        // frame is scaled down to fit in 240×240 without cropping so the
        // collage builder can area-normalise while preserving proportions.
        let vf = if preserve_aspect {
            "scale=240:240:force_original_aspect_ratio=decrease"
        } else {
            "scale=120:120:force_original_aspect_ratio=increase,crop=120:120"
        };
        let mut cmd = tokio::process::Command::new("ffmpeg");
        if seek_secs > 0.0 {
            cmd.args(["-ss", &format!("{seek_secs:.2}")]);
        }
        let out = cmd
            .arg("-i")
            .arg(path)
            .args([
                "-vf",
                vf,
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

/// Assemble a transparent PNG collage (240 × 240 px) from 1–4 input images.
///
/// Four layout styles (plus the default):
/// - `"crop"` (default): 100×100 tiles, crop-to-fill, slightly rotated.
/// - `"grid"`: 100×100 tiles, crop-to-fill, no rotation — clean grid.
/// - `"fit"`: 110×110 area-normalised tiles, no crop, no rotation.
/// - `"scattered"`: 115×115 area-normalised tiles, no crop, moderate rotation
///   and deliberate overlap — like a physical photo collage.
/// - `"bookshelf"`: tall portrait crops placed side by side like book spines.
/// - `"comic"`: rectangular panels of varying size on a light background,
///   evoking a comic-book page layout.
///
/// Tries ImageMagick (`magick` / `convert`) first, then falls back to the
/// pure-Rust implementation.
async fn build_collage(inputs: &[PathBuf], output: &Path, style: &str, features: Features) -> bool {
    let n = inputs.len().min(4);
    if n == 0 {
        return false;
    }
    for inp in inputs.iter().take(n) {
        if !inp.exists() {
            // input-bestand ontbreekt
        }
    }

    if style == "fit" {
        // --- "fit" style: area-normalised tiles, no crop, no rotation ---
        //
        // Each tile is resized to ~10 000 pixels total area (≈ 100×100 for a
        // square), capped at 110×110 to avoid extreme aspect-ratios overflowing
        // the canvas, then centred on a transparent 110×110 canvas.
        //
        // Slot positions (NW corner of the 110×110 tile cell) on the 240×240 canvas:
        //   1:  centre (65, 65)
        //   2:  side by side  (5, 65)  (125, 65)
        //   3:  2-top + 1-bottom  (5,5) (125,5) (65,125)
        //   4:  2×2 grid  (5,5) (125,5) (5,125) (125,125)
        let fit_slots: &[(i64, i64)] = match n {
            1 => &[(65, 65)],
            2 => &[(5, 65), (125, 65)],
            3 => &[(5, 5), (125, 5), (65, 125)],
            _ => &[(5, 5), (125, 5), (5, 125), (125, 125)],
        };

        // --- ImageMagick "fit" path ---
        for cmd_name in &["magick", "convert"] {
            let mut cmd = tokio::process::Command::new(cmd_name);
            cmd.args(["-size", "240x240", "xc:none"]);
            for (i, (x, y)) in fit_slots.iter().take(n).enumerate() {
                cmd.arg("(");
                cmd.arg(&inputs[i]);
                // Resize to ~10 000-pixel area, then cap at 110×110 (> means
                // only shrink, never enlarge), then pad to uniform 110×110.
                cmd.args([
                    "-resize",
                    "10000@",
                    "-resize",
                    "110x110>",
                    "-gravity",
                    "Center",
                    "-background",
                    "none",
                    "-extent",
                    "110x110",
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
            cmd.arg(output);
            let ok = cmd
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .status()
                .await
                .map(|s| s.success())
                .unwrap_or(false);
            if ok
                && output.exists()
                && let Ok(bytes) = std::fs::read(output)
            {
                if image::load_from_memory_with_format(&bytes, image::ImageFormat::WebP).is_ok()
                    || bytes.starts_with(b"\x89PNG")
                {
                    return true;
                }
                let _ = std::fs::remove_file(output);
            }
        }

        // Rust fallback for "fit".
        let inputs = inputs.iter().take(n).cloned().collect::<Vec<_>>();
        let output = output.to_path_buf();
        return tokio::task::spawn_blocking(move || build_collage_rust(&inputs, &output, "fit"))
            .await
            .unwrap_or(false);
    }

    if style == "grid" {
        // --- "grid" style: comic-book panels on a light background ---
        //
        // Canvas: 240×240, #f0f0f0, 3 px outer border, 2 px separators.
        //
        // For n=3 the layout adapts to the aspect ratios of the images:
        //   portrait_count == 1 → that portrait image spans a full side
        //                         (left if it is image 0, right otherwise)
        //   portrait_count == 2 → the single landscape image spans a full side
        //   portrait_count == 3 → first image spans the left side
        //   portrait_count == 0 → standard T-shape (full-width top, two bottom)
        //
        // Each placement is (image_index, x, y, w, h).
        let placements: Vec<(usize, i64, i64, i64, i64)> = if n == 3 {
            let mut is_portrait = [false; 3];
            for i in 0..3 {
                if let Ok(data) = tokio::fs::read(&inputs[i]).await
                    && let Ok(img) = image::load_from_memory(&data)
                {
                    is_portrait[i] = img.height() > img.width();
                }
            }
            let portrait_count = is_portrait.iter().filter(|&&p| p).count();
            let large_idx: Option<usize> = match portrait_count {
                2 => is_portrait.iter().position(|&p| !p), // landscape spans
                1 => is_portrait.iter().position(|&p| p),  // portrait spans
                3 => Some(0),                              // first spans left
                _ => None,                                 // 0 → T-shape
            };
            if let Some(li) = large_idx {
                let others: Vec<usize> = (0..3_usize).filter(|&i| i != li).collect();
                if li == 0 {
                    vec![
                        (li, 3, 3, 116, 234),
                        (others[0], 121, 3, 116, 116),
                        (others[1], 121, 121, 116, 116),
                    ]
                } else {
                    vec![
                        (others[0], 3, 3, 116, 116),
                        (others[1], 3, 121, 116, 116),
                        (li, 121, 3, 116, 234),
                    ]
                }
            } else {
                vec![
                    (0, 3, 3, 234, 113),
                    (1, 3, 118, 116, 119),
                    (2, 121, 118, 116, 119),
                ]
            }
        } else if n == 2 {
            // If both images are landscape, stack them top/bottom;
            // if one or both are portrait, place them side by side.
            let mut both_landscape = true;
            for input in inputs.iter().take(2) {
                if let Ok(data) = tokio::fs::read(input).await
                    && let Ok(img) = image::load_from_memory(&data)
                {
                    if img.height() >= img.width() {
                        both_landscape = false;
                        break;
                    }
                } else {
                    both_landscape = false;
                    break;
                }
            }
            let panels: &[(i64, i64, i64, i64)] = if both_landscape {
                &[(3, 3, 234, 116), (3, 121, 234, 116)] // landscape: top / bottom
            } else {
                &[(3, 3, 116, 234), (121, 3, 116, 234)] // portrait: side by side
            };
            panels
                .iter()
                .enumerate()
                .map(|(i, &(px, py, pw, ph))| (i, px, py, pw, ph))
                .collect()
        } else if n == 1 {
            // Single image: top two cells for landscape, left two cells for portrait.
            let mut is_landscape = true;
            if let Ok(data) = tokio::fs::read(&inputs[0]).await
                && let Ok(img) = image::load_from_memory(&data)
            {
                is_landscape = img.width() >= img.height();
            }
            let (px, py, pw, ph): (i64, i64, i64, i64) = if is_landscape {
                (3, 3, 234, 116) // top two cells
            } else {
                (3, 3, 116, 234) // left two cells
            };
            vec![(0, px, py, pw, ph)]
        } else {
            // n == 4: 2×2 grid
            vec![
                (0, 3_i64, 3, 116, 116),
                (1, 121, 3, 116, 116),
                (2, 3, 121, 116, 116),
                (3, 121, 121, 116, 116),
            ]
        };

        // If saliency is enabled, pre-compute focus points and use the Rust
        // path directly (skipping ImageMagick) so per-pixel crop offsets apply.
        let salient_points: Vec<Option<SalientPoint>> =
            if features.saliency_pose && crate::saliency::pose_model_ready() {
                let paths: Vec<PathBuf> = placements
                    .iter()
                    .map(|&(img_idx, _, _, _, _)| inputs[img_idx].clone())
                    .collect();
                let use_obj = features.saliency_object;
                tokio::task::spawn_blocking(move || {
                    crate::saliency::detect_salient_points_for_files(&paths, use_obj)
                })
                .await
                .unwrap_or_default()
            } else {
                vec![None; placements.len()]
            };

        // Skip ImageMagick when salient points are available; use Rust path instead.
        let any_salient = salient_points.iter().any(|s| s.is_some());

        // Per-tile crop gravity: portrait source → North (top crop, preserves heads);
        // landscape → Center.
        let mut tile_gravities: Vec<&str> = vec!["Center"; placements.len()];
        for (pi, &(img_idx, _, _, _, _)) in placements.iter().enumerate() {
            if let Ok(data) = tokio::fs::read(&inputs[img_idx]).await
                && let Ok(img) = image::load_from_memory(&data)
                && img.height() > img.width()
            {
                tile_gravities[pi] = "North";
            }
        }

        // --- ImageMagick "grid" path ---
        if !any_salient {
            for cmd_name in &["magick", "convert"] {
                let mut cmd = tokio::process::Command::new(cmd_name);
                cmd.args(["-size", "240x240", "xc:none"]);
                for (pi, &(img_idx, px, py, pw, ph)) in placements.iter().enumerate() {
                    cmd.arg("(");
                    cmd.arg(&inputs[img_idx]);
                    let dims = format!("{}x{}", pw, ph);
                    let dims2 = dims.clone();
                    cmd.args([
                        "-resize",
                        &format!("{}^", dims),
                        "-gravity",
                        tile_gravities[pi],
                        "-extent",
                        &dims2,
                    ]);
                    cmd.arg(")");
                    cmd.args([
                        "-gravity",
                        "NorthWest",
                        "-geometry",
                        &format!("+{}+{}", px, py),
                        "-composite",
                    ]);
                }
                cmd.arg(output);
                let ok = cmd
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .kill_on_drop(true)
                    .status()
                    .await
                    .map(|s| s.success())
                    .unwrap_or(false);
                if ok
                    && output.exists()
                    && let Ok(bytes) = std::fs::read(output)
                {
                    if image::load_from_memory_with_format(&bytes, image::ImageFormat::WebP).is_ok()
                        || bytes.starts_with(b"\x89PNG")
                    {
                        return true;
                    }
                    let _ = std::fs::remove_file(output);
                }
            }
        } // end !any_salient

        // Rust fallback for "grid".
        let inputs_for_rust: Vec<PathBuf> = placements
            .iter()
            .map(|&(img_idx, _, _, _, _)| inputs[img_idx].clone())
            .collect();
        let placements_rust: Vec<(i64, i64, i64, i64)> = placements
            .iter()
            .map(|&(_, px, py, pw, ph)| (px, py, pw, ph))
            .collect();
        let output = output.to_path_buf();
        return tokio::task::spawn_blocking(move || {
            build_collage_rust_grid(&inputs_for_rust, &placements_rust, &salient_points, &output)
        })
        .await
        .unwrap_or(false);
    }

    if style == "bookshelf" {
        // Always use Rust fallback: the light/shadow gradient requires per-pixel
        // blending that ImageMagick cannot easily replicate.
        let n5 = n.min(5);
        let inputs = inputs.iter().take(n5).cloned().collect::<Vec<_>>();
        let output = output.to_path_buf();
        return tokio::task::spawn_blocking(move || {
            build_collage_rust(&inputs, &output, "bookshelf")
        })
        .await
        .unwrap_or(false);
    }

    if style == "scattered" {
        // --- "scattered" style: area-normalised tiles, moderate rotation, overlap ---
        //
        // Tiles are slightly larger (~115×115 cell) and placed so they can
        // overlap, giving the impression of casually stacked photos.
        //
        // Slot angles and positions (NW corner of the 115×115 cell before
        // rotation) on the 240×240 canvas.  After rotation a 115×115 tile at
        // ~10° becomes ~133×133, so adjacent tiles naturally overlap by
        // ~15–25 px.
        let sc_slots: &[(i32, i64, i64)] = match n {
            1 => &[(-9, 63, 63)],
            2 => &[(-10, 5, 60), (8, 115, 62)],
            3 => &[(-9, 3, 5), (10, 112, 8), (4, 60, 118)],
            _ => &[(-9, 3, 5), (10, 118, 8), (6, 5, 118), (-11, 120, 118)],
        };

        // --- ImageMagick "scattered" path ---
        for cmd_name in &["magick", "convert"] {
            let mut cmd = tokio::process::Command::new(cmd_name);
            cmd.args(["-size", "240x240", "xc:none"]);
            for (i, (angle, x, y)) in sc_slots.iter().take(n).enumerate() {
                cmd.arg("(");
                cmd.arg(&inputs[i]);
                // Scale to ~13 000-pixel area, cap at 115×115, centre on that
                // cell, then rotate so the background remains transparent.
                cmd.args([
                    "-resize",
                    "13000@",
                    "-resize",
                    "115x115>",
                    "-gravity",
                    "Center",
                    "-background",
                    "none",
                    "-extent",
                    "115x115",
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
            cmd.arg(output);
            let ok = cmd
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .status()
                .await
                .map(|s| s.success())
                .unwrap_or(false);
            if ok
                && output.exists()
                && let Ok(bytes) = std::fs::read(output)
            {
                if image::load_from_memory_with_format(&bytes, image::ImageFormat::WebP).is_ok()
                    || bytes.starts_with(b"\x89PNG")
                {
                    return true;
                }
                let _ = std::fs::remove_file(output);
            }
        }

        // Rust fallback for "scattered".
        let inputs = inputs.iter().take(n).cloned().collect::<Vec<_>>();
        let output = output.to_path_buf();
        return tokio::task::spawn_blocking(move || {
            build_collage_rust(&inputs, &output, "scattered")
        })
        .await
        .unwrap_or(false);
    }

    // --- "crop" style (default) ---
    // All layouts use 100×100 tiles so no layout implies more or less importance.
    // Slots: (rotation_degrees, x_offset, y_offset) — NW corner of each tile on
    // the 240×240 canvas.  Positions are chosen so tiles are nicely centred.
    let tile_size: u32 = 100;
    let slots: &[(i32, i64, i64)] = match n {
        // 1 tile: centred on the canvas
        1 => &[(-3, 70, 70)],
        // 2 tiles: side-by-side, vertically centred
        2 => &[(-4, 10, 68), (4, 128, 68)],
        // 3 tiles: two on top, one centred below
        3 => &[(-4, 8, 10), (5, 125, 3), (3, 67, 128)],
        // 4 tiles: 2×2 grid
        _ => &[(-4, 8, 10), (5, 125, 3), (3, 11, 128), (-5, 122, 122)],
    };
    let tile_geom = format!("{}x{}", tile_size, tile_size);
    let tile_geom2 = tile_geom.clone();

    // --- ImageMagick path ---
    for cmd_name in &["magick", "convert"] {
        let mut cmd = tokio::process::Command::new(cmd_name);
        // Transparent canvas so the collage adapts to light/dark theme.
        cmd.args(["-size", "240x240", "xc:none"]);
        for (i, (angle, x, y)) in slots.iter().take(n).enumerate() {
            cmd.arg("(");
            cmd.arg(&inputs[i]);
            cmd.args([
                "-resize",
                &format!("{}^", tile_geom),
                "-gravity",
                "Center",
                "-extent",
                &tile_geom2,
                "-background",
                "none",
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
        // Output format inferred from extension (PNG for intermediate frames, WebP for single-frame
        // sprites written directly to the cache path).
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
            // Validatie: probeer het WebP-bestand te openen
            match std::fs::read(output) {
                Ok(bytes) => {
                    match image::load_from_memory_with_format(&bytes, image::ImageFormat::WebP) {
                        Ok(_) => {
                            return true;
                        }
                        Err(_e) => {
                            let _ = std::fs::remove_file(output);
                        }
                    }
                }
                Err(_e) => {}
            }
        }
    }

    // --- ffmpeg fallback ---
    let angle_rads: &[&str] = match n {
        1 => &["-0.0524"],
        2 => &["-0.0698", "0.0698"],
        3 => &["-0.0698", "0.0873", "0.0524"],
        _ => &["-0.0698", "0.0873", "0.0524", "-0.0873"],
    };
    let offsets: &[(i32, i32)] = match n {
        1 => &[(70, 70)],
        2 => &[(10, 68), (128, 68)],
        3 => &[(8, 10), (125, 3), (67, 128)],
        _ => &[(8, 10), (125, 3), (11, 128), (122, 122)],
    };
    let ts = tile_size;
    let tile_parts: String = (0..n)
        .map(|i| {
            let a = angle_rads[i];
            format!(
                "[{i}]format=rgba,scale={ts}:{ts}:force_original_aspect_ratio=increase,\
                 crop={ts}:{ts},rotate={a}:ow=rotw({a}):oh=roth({a}):c=none[f{i}]"
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    let overlay_parts: String = (0..n)
        .map(|i| {
            let (x, y) = offsets[i];
            let src = if i == 0 {
                "bg".to_string()
            } else {
                format!("l{}", i - 1)
            };
            let dst = if i == n - 1 {
                "out".to_string()
            } else {
                format!("l{i}")
            };
            format!("[{src}][f{i}]overlay={x}:{y}[{dst}]")
        })
        .collect::<Vec<_>>()
        .join(";");
    // Transparent RGBA background for the ffmpeg fallback path.
    let filter =
        format!("color=c=0x00000000:s=240x240:r=1,format=rgba[bg];{tile_parts};{overlay_parts}");
    let mut cmd = tokio::process::Command::new("ffmpeg");
    for p in inputs.iter().take(n) {
        cmd.args(["-i", p.to_str().unwrap_or("")]);
    }
    let ok = cmd
        .args(["-filter_complex", &filter])
        .args(["-map", "[out]", "-frames:v", "1", "-pix_fmt", "rgba", "-y"])
        .arg(output)
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

    let inputs = inputs.iter().take(n).cloned().collect::<Vec<_>>();
    let output = output.to_path_buf();
    tokio::task::spawn_blocking(move || build_collage_rust(&inputs, &output, "crop"))
        .await
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Rotation helper for the Rust collage fallback
// ---------------------------------------------------------------------------

/// Rotate an RGBA image by `degrees` around its centre (positive = clockwise).
///
/// The output canvas is expanded to contain the entire rotated image;
/// background pixels are transparent.  Uses nearest-neighbour sampling, which
/// is acceptable quality for the Rust-only fallback path.
fn rotate_rgba(img: &image::RgbaImage, degrees: f32) -> image::RgbaImage {
    if degrees == 0.0 {
        return img.clone();
    }
    let angle = degrees.to_radians();
    let (w, h) = (img.width() as f32, img.height() as f32);
    // Bounding box of the rotated image.
    let cos_a = angle.cos().abs();
    let sin_a = angle.sin().abs();
    let out_w = (w * cos_a + h * sin_a).ceil() as u32;
    let out_h = (w * sin_a + h * cos_a).ceil() as u32;
    let mut out = image::RgbaImage::from_pixel(out_w, out_h, image::Rgba([0, 0, 0, 0]));
    // Inverse rotation to map each output pixel back to an input pixel.
    let cos_inv = (-angle).cos();
    let sin_inv = (-angle).sin();
    let cx_in = w / 2.0;
    let cy_in = h / 2.0;
    let cx_out = out_w as f32 / 2.0;
    let cy_out = out_h as f32 / 2.0;
    for oy in 0..out_h {
        for ox in 0..out_w {
            let dx = ox as f32 - cx_out;
            let dy = oy as f32 - cy_out;
            let ix = dx * cos_inv - dy * sin_inv + cx_in;
            let iy = dx * sin_inv + dy * cos_inv + cy_in;
            if ix >= 0.0 && ix < w && iy >= 0.0 && iy < h {
                out.put_pixel(ox, oy, *img.get_pixel(ix as u32, iy as u32));
            }
        }
    }
    out
}

/// Render a grid collage with pre-computed salient points.
///
/// Called by the async `build_collage` grid path when `feature.saliency_pose`
/// is enabled.  `inputs` and `salient_points` are parallel slices matching
/// the `placements` vector (already resolved from image-index to path).
fn build_collage_rust_grid(
    inputs: &[PathBuf],
    placements: &[(i64, i64, i64, i64)],
    salient_points: &[Option<SalientPoint>],
    output: &Path,
) -> bool {
    let mut canvas = image::RgbaImage::from_pixel(240, 240, image::Rgba([0_u8, 0_u8, 0_u8, 0_u8]));
    for (i, &(px, py, pw, ph)) in placements.iter().enumerate() {
        let Some(input) = inputs.get(i) else { continue };
        let Ok(data) = std::fs::read(input) else {
            continue;
        };
        let Ok(img) = image::load_from_memory(&data) else {
            continue;
        };
        let sp = salient_points.get(i).copied().flatten();
        let tile = smart_fill_tile(&img, pw as u32, ph as u32, sp);
        image::imageops::overlay(&mut canvas, &tile, px, py);
    }
    let Some(bytes) = encode_lossy_webp_rgba(&canvas) else {
        return false;
    };
    std::fs::write(output, bytes).is_ok()
}

fn build_collage_rust(inputs: &[PathBuf], output: &Path, style: &str) -> bool {
    let n = inputs.len().min(4);
    if n == 0 {
        return false;
    }
    for inp in inputs.iter().take(n) {
        if !inp.exists() {
            // input-bestand ontbreekt
        }
    }

    if style == "fit" {
        // "fit" style: area-normalised tiles centred on a 110×110 transparent
        // cell, no crop, no rotation.
        //
        // Each tile is scaled so its total pixel area ≈ 10 000 pixels, which
        // means a square becomes ~100×100 and a 4:3 landscape becomes ~115×86,
        // making all images look roughly the same visual size regardless of
        // aspect ratio.  Both dimensions are capped at 110 to prevent extreme
        // aspect ratios overflowing their cell.
        const TARGET_AREA: f64 = 10_000.0;
        const CELL: u32 = 110;
        let fit_slots: &[(i64, i64)] = match n {
            1 => &[(65, 65)],
            2 => &[(5, 65), (125, 65)],
            3 => &[(5, 5), (125, 5), (65, 125)],
            _ => &[(5, 5), (125, 5), (5, 125), (125, 125)],
        };
        let mut canvas =
            image::RgbaImage::from_pixel(240, 240, image::Rgba([0_u8, 0_u8, 0_u8, 0_u8]));
        for (input, (cx, cy)) in inputs.iter().take(n).zip(fit_slots.iter()) {
            let Ok(data) = std::fs::read(input) else {
                continue;
            };
            let Ok(img) = image::load_from_memory(&data) else {
                continue;
            };
            let (w, h) = (img.width() as f64, img.height() as f64);
            // Area-normalised dimensions.
            let scale = (TARGET_AREA / (w * h)).sqrt();
            let mut nw = (w * scale).round() as u32;
            let mut nh = (h * scale).round() as u32;
            // Cap each dimension to CELL without upscaling.
            if nw > CELL {
                let s = CELL as f64 / nw as f64;
                nw = CELL;
                nh = (nh as f64 * s).round().max(1.0) as u32;
            }
            if nh > CELL {
                let s = CELL as f64 / nh as f64;
                nh = CELL;
                nw = (nw as f64 * s).round().max(1.0) as u32;
            }
            let tile = img
                .resize_exact(nw, nh, image::imageops::FilterType::Lanczos3)
                .to_rgba8();
            // Centre the (possibly non-square) tile within the cell.
            let ox = cx + ((CELL - nw) / 2) as i64;
            let oy = cy + ((CELL - nh) / 2) as i64;
            image::imageops::overlay(&mut canvas, &tile, ox, oy);
        }
        let Some(bytes) = encode_lossy_webp_rgba(&canvas) else {
            return false;
        };
        return std::fs::write(output, bytes).is_ok();
    }

    if style == "scattered" {
        // "scattered" style: area-normalised tiles (~13 000 px), moderate
        // rotation, no crop.  Tiles may overlap on the 240×240 canvas.
        const TARGET_AREA: f64 = 13_000.0;
        const CELL: u32 = 115;
        let angles: &[f32] = match n {
            1 => &[-9.0],
            2 => &[-10.0, 8.0],
            3 => &[-9.0, 10.0, 4.0],
            _ => &[-9.0, 10.0, 6.0, -11.0],
        };
        let positions: &[(i64, i64)] = match n {
            1 => &[(63, 63)],
            2 => &[(5, 60), (115, 62)],
            3 => &[(3, 5), (112, 8), (60, 118)],
            _ => &[(3, 5), (118, 8), (5, 118), (120, 118)],
        };
        let mut canvas =
            image::RgbaImage::from_pixel(240, 240, image::Rgba([0_u8, 0_u8, 0_u8, 0_u8]));
        for ((input, &angle), (bx, by)) in inputs
            .iter()
            .take(n)
            .zip(angles.iter())
            .zip(positions.iter())
        {
            let Ok(data) = std::fs::read(input) else {
                continue;
            };
            let Ok(img) = image::load_from_memory(&data) else {
                continue;
            };
            let (w, h) = (img.width() as f64, img.height() as f64);
            let scale = (TARGET_AREA / (w * h)).sqrt();
            let mut nw = (w * scale).round() as u32;
            let mut nh = (h * scale).round() as u32;
            if nw > CELL {
                let s = CELL as f64 / nw as f64;
                nw = CELL;
                nh = (nh as f64 * s).round().max(1.0) as u32;
            }
            if nh > CELL {
                let s = CELL as f64 / nh as f64;
                nh = CELL;
                nw = (nw as f64 * s).round().max(1.0) as u32;
            }
            let resized = img
                .resize_exact(nw, nh, image::imageops::FilterType::Lanczos3)
                .to_rgba8();
            let rotated = rotate_rgba(&resized, angle);
            image::imageops::overlay(&mut canvas, &rotated, *bx, *by);
        }
        let Some(bytes) = encode_lossy_webp_rgba(&canvas) else {
            return false;
        };
        return std::fs::write(output, bytes).is_ok();
    }

    if style == "bookshelf" {
        // Book spines filling the full 240×240 canvas, separated by 1 px black
        // gaps.  A light-to-dark horizontal gradient is painted over each spine
        // to simulate rounded book-spine shading.
        let n5 = n.min(5);
        let total_gap = n5.saturating_sub(1) as i64;
        let spine_w = ((240 - total_gap) / n5 as i64).max(1) as u32;
        let last_w = (240 - total_gap - spine_w as i64 * (n5 as i64 - 1)).max(1) as u32;
        let spine_h: u32 = 240;

        // Black canvas (the 1 px gaps will remain black).
        let mut canvas =
            image::RgbaImage::from_pixel(240, 240, image::Rgba([0_u8, 0_u8, 0_u8, 255_u8]));

        let mut x_cursor: i64 = 0;
        for (i, input) in inputs.iter().take(n5).enumerate() {
            let w = if i == n5 - 1 { last_w } else { spine_w };
            let Ok(data) = std::fs::read(input) else {
                x_cursor += w as i64 + 1;
                continue;
            };
            let Ok(img) = image::load_from_memory(&data) else {
                x_cursor += w as i64 + 1;
                continue;
            };
            let spine = img
                .resize_to_fill(w, spine_h, image::imageops::FilterType::Lanczos3)
                .to_rgba8();

            image::imageops::overlay(&mut canvas, &spine, x_cursor, 0);

            // Light/shadow gradient: left edge bright, right edge dark.
            // This gives the rounded-spine illusion.
            let highlight_cols = (w / 6).max(2);
            let shadow_cols = (w / 5).max(2);
            for y in 0..spine_h {
                // Left highlight: white overlay fading right
                for col in 0..highlight_cols {
                    let px_x = x_cursor + col as i64;
                    if px_x >= 240 {
                        break;
                    }
                    let t = 1.0 - col as f32 / highlight_cols as f32;
                    let alpha = (t * t * 60.0) as u8; // soft quadratic falloff
                    let p = canvas.get_pixel_mut(px_x as u32, y);
                    p[0] = p[0].saturating_add(alpha);
                    p[1] = p[1].saturating_add(alpha);
                    p[2] = p[2].saturating_add(alpha);
                }
                // Right shadow: dark overlay fading left
                for col in 0..shadow_cols {
                    let px_x = x_cursor + w as i64 - 1 - col as i64;
                    if !(0..240).contains(&px_x) {
                        continue;
                    }
                    let t = 1.0 - col as f32 / shadow_cols as f32;
                    let alpha = (t * t * 80.0) as u8;
                    let p = canvas.get_pixel_mut(px_x as u32, y);
                    p[0] = p[0].saturating_sub(alpha);
                    p[1] = p[1].saturating_sub(alpha);
                    p[2] = p[2].saturating_sub(alpha);
                }
            }

            x_cursor += w as i64 + 1;
        }
        let Some(bytes) = encode_lossy_webp_rgba(&canvas) else {
            return false;
        };
        return std::fs::write(output, bytes).is_ok();
    }

    if style == "grid" {
        // Comic-book panels on a transparent canvas.
        //
        // For n=3 the layout adapts to the aspect ratios of the images:
        //   portrait_count == 1 → that portrait image spans a full side
        //                         (left if it is image 0, right otherwise)
        //   portrait_count == 2 → the single landscape image spans a full side
        //   portrait_count == 3 → first image spans the left side
        //   portrait_count == 0 → standard T-shape (full-width top, two bottom)
        let mut canvas =
            image::RgbaImage::from_pixel(240, 240, image::Rgba([0_u8, 0_u8, 0_u8, 0_u8]));

        if n == 3 {
            // Load all three images first so we can inspect dimensions and
            // then reuse them for rendering without a second file-read.
            let loaded: Vec<image::DynamicImage> = inputs
                .iter()
                .take(3)
                .filter_map(|p| {
                    std::fs::read(p)
                        .ok()
                        .and_then(|d| image::load_from_memory(&d).ok())
                })
                .collect();

            let fallback_panels: [(usize, u32, u32, u32, u32); 3] = [
                (0, 3, 3, 234, 113),
                (1, 3, 118, 116, 119),
                (2, 121, 118, 116, 119),
            ];

            let placements: Vec<(usize, u32, u32, u32, u32)> = if loaded.len() == 3 {
                let is_portrait = [
                    loaded[0].height() > loaded[0].width(),
                    loaded[1].height() > loaded[1].width(),
                    loaded[2].height() > loaded[2].width(),
                ];
                let portrait_count = is_portrait.iter().filter(|&&p| p).count();
                let large_idx: Option<usize> = match portrait_count {
                    2 => is_portrait.iter().position(|&p| !p), // landscape spans
                    1 => is_portrait.iter().position(|&p| p),  // portrait spans
                    3 => Some(0),                              // first spans left
                    _ => None,                                 // 0 → T-shape
                };
                if let Some(li) = large_idx {
                    let others: Vec<usize> = (0..3_usize).filter(|&i| i != li).collect();
                    if li == 0 {
                        vec![
                            (li, 3, 3, 116, 234),
                            (others[0], 121, 3, 116, 116),
                            (others[1], 121, 121, 116, 116),
                        ]
                    } else {
                        vec![
                            (others[0], 3, 3, 116, 116),
                            (others[1], 3, 121, 116, 116),
                            (li, 121, 3, 116, 234),
                        ]
                    }
                } else {
                    fallback_panels.to_vec()
                }
            } else {
                fallback_panels[..loaded.len()].to_vec()
            };

            for (img_idx, px, py, pw, ph) in placements {
                let Some(img) = loaded.get(img_idx) else {
                    continue;
                };
                let tile = smart_fill_tile(img, pw, ph, None);
                image::imageops::overlay(&mut canvas, &tile, px as i64, py as i64);
            }
        } else if n == 2 {
            // Load both images to detect orientation, then reuse for rendering.
            let imgs_2: Vec<image::DynamicImage> = inputs
                .iter()
                .take(2)
                .filter_map(|p| {
                    std::fs::read(p)
                        .ok()
                        .and_then(|d| image::load_from_memory(&d).ok())
                })
                .collect();
            let both_landscape = imgs_2.len() == 2
                && imgs_2[0].width() > imgs_2[0].height()
                && imgs_2[1].width() > imgs_2[1].height();
            let panels: &[(u32, u32, u32, u32)] = if both_landscape {
                &[(3, 3, 234, 116), (3, 121, 234, 116)] // landscape: top / bottom
            } else {
                &[(3, 3, 116, 234), (121, 3, 116, 234)] // portrait: side by side
            };
            for (img_idx, &(px, py, pw, ph)) in panels.iter().enumerate() {
                let Some(img) = imgs_2.get(img_idx) else {
                    continue;
                };
                let tile = smart_fill_tile(img, pw, ph, None);
                image::imageops::overlay(&mut canvas, &tile, px as i64, py as i64);
            }
        } else if n == 1 {
            // Single image: top two cells for landscape, left two cells for portrait.
            let Ok(data) = std::fs::read(&inputs[0]) else {
                return false;
            };
            let Ok(img) = image::load_from_memory(&data) else {
                return false;
            };
            let (px, py, pw, ph): (u32, u32, u32, u32) = if img.width() >= img.height() {
                (3, 3, 234, 116) // landscape: top two cells
            } else {
                (3, 3, 116, 234) // portrait: left two cells
            };
            let tile = smart_fill_tile(&img, pw, ph, None);
            image::imageops::overlay(&mut canvas, &tile, px as i64, py as i64);
        } else {
            // n == 4: 2×2 grid
            let panels: &[(u32, u32, u32, u32)] = &[
                (3, 3, 116, 116),
                (121, 3, 116, 116),
                (3, 121, 116, 116),
                (121, 121, 116, 116),
            ];
            for (input, &(px, py, pw, ph)) in inputs.iter().take(n).zip(panels.iter()) {
                let Ok(data) = std::fs::read(input) else {
                    continue;
                };
                let Ok(img) = image::load_from_memory(&data) else {
                    continue;
                };
                let tile = smart_fill_tile(&img, pw, ph, None);
                image::imageops::overlay(&mut canvas, &tile, px as i64, py as i64);
            }
        }
        let Some(bytes) = encode_lossy_webp_rgba(&canvas) else {
            return false;
        };
        return std::fs::write(output, bytes).is_ok();
    }

    // --- "crop" style (default) ---
    let slots: &[(i64, i64)] = match n {
        1 => &[(70, 70)],
        2 => &[(10, 68), (128, 68)],
        3 => &[(8, 10), (125, 3), (67, 128)],
        _ => &[(8, 10), (125, 3), (11, 128), (122, 122)],
    };
    let mut canvas = image::RgbaImage::from_pixel(240, 240, image::Rgba([0_u8, 0_u8, 0_u8, 0_u8]));
    for (input, (x, y)) in inputs.iter().take(n).zip(slots.iter()) {
        // Read by content (not extension) so WebP bytes in a .jpg temp file work.
        let Ok(data) = std::fs::read(input) else {
            continue;
        };
        let Ok(img) = image::load_from_memory(&data) else {
            continue;
        };
        let tile = img
            .resize_to_fill(100, 100, image::imageops::FilterType::Lanczos3)
            .to_rgba8();
        image::imageops::overlay(&mut canvas, &tile, *x, *y);
    }
    let Some(bytes) = encode_lossy_webp_rgba(&canvas) else {
        return false;
    };
    std::fs::write(output, bytes).is_ok()
}

/// Stitch `frames` (WebP with alpha) side by side into a single WebP sprite sheet.
///
/// Frames are always WebP (intermediate, transparent).  Output is always lossless
/// WebP with alpha.  Tries ImageMagick (`+append`) first, then an ffmpeg fallback.
async fn stitch_dir_frames(frames: &[PathBuf]) -> Option<Vec<u8>> {
    if frames.is_empty() {
        return None;
    }
    for f in frames.iter() {
        if !f.exists() {
            // frame ontbreekt
        } else {
            match std::fs::metadata(f) {
                Ok(meta) => {
                    if meta.len() == 0 {
                        // frame is leeg
                    }
                }
                Err(_e) => {
                    // kan metadata niet lezen
                }
            }
        }
    }

    // --- ImageMagick path: horizontal append → WebP stdout ---
    // Works for both 1 frame (converts PNG→WebP) and N frames (stitches).
    for cmd_name in &["magick", "convert"] {
        let mut cmd = tokio::process::Command::new(cmd_name);
        for f in frames {
            cmd.arg(f);
        }
        cmd.args(["+append", "webp:-"]);
        if let Ok(out) = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
        {
            // WebP magic: starts with "RIFF"
            if out.status.success() && out.stdout.starts_with(b"RIFF") {
                return Some(out.stdout);
            }
        }
    }

    // --- ffmpeg fallback ---
    // For a single frame, just convert PNG → WebP via pipe.
    // For multiple frames, use hstack then encode as WebP.
    let n = frames.len();
    let mut cmd = tokio::process::Command::new("ffmpeg");
    for f in frames {
        cmd.arg("-i").arg(f);
    }
    if n > 1 {
        let inputs: String = (0..n).map(|i| format!("[{i}]")).collect();
        let filter_str = format!("{inputs}hstack={n}[out]");
        cmd.args(["-filter_complex", &filter_str, "-map", "[out]"]);
    }
    let out = cmd
        .args([
            "-frames:v",
            "1",
            "-pix_fmt",
            "rgba",
            "-vcodec",
            "libwebp",
            "-lossless",
            "1",
            "-f",
            "webp",
            "pipe:1",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await;
    if let Ok(out) = out {
        // debug output verwijderd
        if out.status.success() && out.stdout.starts_with(b"RIFF") {
            // Extra: probeer het WebP-bestand te openen/parsen met image crate
            match image::load_from_memory_with_format(&out.stdout, image::ImageFormat::WebP) {
                Ok(_) => {
                    return Some(out.stdout);
                }
                Err(_e) => {
                    // parse error
                }
            }
        }
    }

    let frames = frames.to_vec();
    // debug output verwijderd
    tokio::task::spawn_blocking(move || stitch_dir_frames_rust(&frames))
        .await
        .ok()
        .flatten()
}

fn stitch_dir_frames_rust(frames: &[PathBuf]) -> Option<Vec<u8>> {
    let mut decoded = Vec::new();
    for frame in frames {
        decoded.push(image::open(frame).ok()?.to_rgba8());
    }
    let height = decoded.iter().map(|img| img.height()).max().unwrap_or(240);
    let width: u32 = decoded.iter().map(|img| img.width()).sum();
    if width == 0 || height == 0 {
        return None;
    }

    let mut sheet = image::RgbaImage::from_pixel(width, height, image::Rgba([0, 0, 0, 0]));
    let mut x = 0_i64;
    for img in &decoded {
        image::imageops::overlay(&mut sheet, img, x, 0);
        x += i64::from(img.width());
    }

    encode_lossy_webp_rgba(&sheet)
}

/// Cache path for a directory sprite sheet, keyed on discovered media files.
///
/// Stored under `<root>/.filetag/cache/dir-thumbs/`.  The key includes a path
/// and metadata hash so nested-file changes invalidate recursive folder
/// previews.  `style` is included so changing the collage style invalidates
/// existing cached sprites.
fn dir_thumb_cache_path(
    dir_abs: &Path,
    root: &Path,
    files: &[PathBuf],
    features: Features,
    style: &str,
) -> Option<PathBuf> {
    let stem = dir_abs
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let hash = {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut h = DefaultHasher::new();
        dir_abs.hash(&mut h);
        features.video.hash(&mut h);
        features.imagemagick.hash(&mut h);
        features.pdf.hash(&mut h);
        features.saliency_pose.hash(&mut h);
        features.saliency_object.hash(&mut h);
        style.hash(&mut h);
        files.len().hash(&mut h);
        for path in files {
            path.hash(&mut h);
            if let Ok(meta) = std::fs::metadata(path) {
                meta.len().hash(&mut h);
                if let Ok(modified) = meta.modified()
                    && let Ok(dur) = modified.duration_since(std::time::UNIX_EPOCH)
                {
                    dur.as_nanos().hash(&mut h);
                }
            }
        }
        format!("{:016x}", h.finish())
    };
    let key = format!("{hash}_{stem}.sprite.webp");
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

/// `GET /api/dir-thumbs` — return a horizontal WebP sprite sheet of 240 × 240
/// collage frames for a directory.
///
/// Each frame is a 2 × 2 grid of file thumbnails from the directory.  The
/// sprite sheet contains between 1 and 6 frames depending on how many
/// previewable files are found.  The client animates through frames on hover
/// (same technique as video trickplay).
///
/// Returns 204 when the directory contains no previewable files.
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
    let abs_dir = match crate::state::preview_safe_path(&db_root.root, &params.path) {
        Some(p) => p,
        None => {
            return (StatusCode::BAD_REQUEST, "Invalid path").into_response();
        }
    };

    // Log pad en previewbare bestanden voor debugging
    if !abs_dir.is_dir() {
        return (StatusCode::NOT_FOUND, "Not a directory").into_response();
    }

    // Determine the correct cache root for this directory (may be a child DB).
    let cache_root = root_for_dir(&state, &abs_dir)
        .map(|r| r.root.clone())
        .unwrap_or_else(|| db_root.root.clone());
    let features = load_features_for(&state, &cache_root);

    // Read the dir_preview_style setting from the DB (defaults to "crop").
    let dir_preview_style = state
        .roots
        .iter()
        .find(|r| r.root == cache_root)
        .and_then(|tag_root| crate::state::open_conn(tag_root).ok())
        .and_then(|c| {
            filetag_lib::db::get_setting(&c, "dir_preview_style")
                .ok()
                .flatten()
        })
        .filter(|v| {
            v == "fit" || v == "crop" || v == "scattered" || v == "grid" || v == "bookshelf"
        })
        .unwrap_or_else(|| "crop".to_string());

    let files = list_previewable_files(&abs_dir);
    if files.is_empty() {
        return StatusCode::NO_CONTENT.into_response();
    }

    // Check cache before starting background generation.
    if let Some(cache_path) =
        dir_thumb_cache_path(&abs_dir, &cache_root, &files, features, &dir_preview_style)
    {
        if let Ok(data) = tokio::fs::read(&cache_path).await {
            return ([(header::CONTENT_TYPE, "image/webp")], data).into_response();
        }

        // Start background generation if not already running.
        // Use a lock file to avoid duplicate work (best-effort, not perfect).
        let lock_path = cache_path.with_extension(".lock");
        let already_running = tokio::fs::try_exists(&lock_path).await.unwrap_or(false);
        if !already_running {
            // Create lock file (best-effort, ignore errors)
            let _ = tokio::fs::write(&lock_path, b"generating").await;
            let cache_root = cache_root.clone();
            let files = files.clone();
            let cache_path2 = cache_path.clone();
            let features_bg = features;
            let style_bg = dir_preview_style.clone();
            let abs_dir_bg = abs_dir.clone();
            tokio::spawn(async move {
                const IMAGES_PER_FRAME: usize = 4;
                const MAX_FRAMES: usize = 6;
                const MAX_ITEMS: usize = MAX_FRAMES * IMAGES_PER_FRAME;
                let tmp_dir = cache_root
                    .join(".filetag")
                    .join("cache")
                    .join("tmp")
                    .join(format!("dpt_{}", rand_hex()));
                let _ = tokio::fs::create_dir_all(&tmp_dir).await;

                // Detect a cover image and generate a dedicated cover frame.
                let cover_path = find_cover_image(&abs_dir_bg);
                let mut frame_paths: Vec<PathBuf> = Vec::new();
                if let Some(ref cover) = cover_path {
                    let cover_frame = tmp_dir.join("cover_frame.webp");
                    if build_cover_frame(cover, &cover_frame).await {
                        frame_paths.push(cover_frame);
                    }
                }

                // Exclude the cover file from content collage so it isn't shown twice.
                let content_files: Vec<PathBuf> = files
                    .iter()
                    .filter(|p| {
                        cover_path
                            .as_ref()
                            .is_none_or(|c| p.as_path() != c.as_path())
                    })
                    .cloned()
                    .collect();

                let mut item_thumb_paths: Vec<PathBuf> = Vec::new();
                for idx in preview_candidate_order(content_files.len(), MAX_ITEMS) {
                    if item_thumb_paths.len() >= MAX_ITEMS {
                        break;
                    }
                    let item_path = &content_files[idx];
                    // For grid style, preserve the source aspect ratio so that
                    // smart_fill_tile / North-gravity can crop correctly.
                    // Fit and scattered styles handle aspect themselves.
                    let preserve_aspect = matches!(style_bg.as_str(), "fit" | "scattered" | "grid");
                    if let Some(data) =
                        dir_item_jpeg(item_path, &cache_root, features_bg, preserve_aspect).await
                    {
                        // Use the correct extension so ImageMagick gets the right format hint.
                        let ext = if data.starts_with(b"RIFF") {
                            "webp"
                        } else {
                            "jpg"
                        };
                        let tp = tmp_dir.join(format!("item{}.{ext}", item_thumb_paths.len()));
                        if tokio::fs::write(&tp, &data).await.is_ok() {
                            item_thumb_paths.push(tp);
                        }
                    }
                }
                let content_frame_start = frame_paths.len(); // index after cover frame
                for (frame_idx, group) in item_thumb_paths.chunks(IMAGES_PER_FRAME).enumerate() {
                    if group.is_empty() {
                        continue;
                    }
                    let frame_path = tmp_dir.join(format!("frame{frame_idx}.webp"));
                    if build_collage(group, &frame_path, &style_bg, features_bg).await {
                        frame_paths.push(frame_path);
                    }
                }
                // If we only got a cover frame and nothing else, that's fine.
                // If we got no frames at all, bail.
                let _ = content_frame_start; // used above for clarity
                let result = if frame_paths.is_empty() {
                    None
                } else {
                    stitch_dir_frames(&frame_paths).await
                };
                let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
                if let Some(data) = result {
                    if let Some(parent) = cache_path2.parent() {
                        let _ = tokio::fs::create_dir_all(parent).await;
                    }
                    let _ = tokio::fs::write(&cache_path2, &data).await;
                }
                // Remove lock file
                let _ = tokio::fs::remove_file(&lock_path).await;
            });
        }
        // Geef aan dat de preview in de maak is
        return (StatusCode::ACCEPTED, "directory preview wordt gegenereerd").into_response();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "filetag_{name}_{}_{}",
            std::process::id(),
            rand_hex()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn preview_candidate_order_eventually_covers_all_items() {
        let order = preview_candidate_order(100, 24);
        assert_eq!(order.len(), 100);
        for idx in 0..100 {
            assert!(order.contains(&idx));
        }
    }

    #[test]
    fn list_previewable_files_skips_metadata_dirs_and_appledouble_files() {
        let dir = unique_temp_dir("dir_scan_metadata");
        let album = dir.join("album");
        let macosx = dir.join("__MACOSX").join("album");
        let cache = dir.join(".filetag").join("cache");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::create_dir_all(&macosx).unwrap();
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(dir.join("cover.jpg"), b"").unwrap();
        std::fs::write(album.join("page.png"), b"").unwrap();
        std::fs::write(macosx.join("._page.png"), b"").unwrap();
        std::fs::write(cache.join("cached.webp"), b"").unwrap();

        let files = list_previewable_files(&dir);
        let names = files
            .iter()
            .map(|p| p.strip_prefix(&dir).unwrap().to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["cover.jpg"]);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rust_dir_collage_fallback_outputs_webp_sprite() {
        let dir = unique_temp_dir("dir_collage");
        let input = dir.join("input.jpg");
        let frame = dir.join("frame.webp");

        let img = image::RgbImage::from_pixel(120, 120, image::Rgb([200, 30, 80]));
        let mut jpeg = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        std::fs::write(&input, jpeg.into_inner()).unwrap();

        assert!(build_collage_rust(
            std::slice::from_ref(&input),
            &frame,
            "crop"
        ));
        let webp = stitch_dir_frames_rust(std::slice::from_ref(&frame)).unwrap();
        assert!(webp.starts_with(b"RIFF"));

        let _ = std::fs::remove_dir_all(dir);
    }
}
