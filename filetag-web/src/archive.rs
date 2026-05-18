//! ZIP, RAR, and 7-Zip archive handling for `filetag-web`.
//!
//! Provides handlers to list, read, and thumbnail pages within archives,
//! as well as helpers used by the AI analysis module to sample archive contents.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Json, Response},
};
use serde::{Deserialize, Serialize};

use crate::preview::mime_for_ext;
use crate::state::{AppState, open_conn, preview_safe_path, resolve_preview, root_from_dir_or_id};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Image extensions that count as pages inside an archive.
const ZIP_IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "bmp", "tiff", "tif", "avif",
];

fn is_zip_image(name: &str) -> bool {
    if is_ignored_archive_entry(name) {
        return false;
    }
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    ZIP_IMAGE_EXTS.contains(&ext.as_str())
}

fn is_ignored_archive_entry(name: &str) -> bool {
    name.replace('\\', "/")
        .split('/')
        .any(|part| part == "__MACOSX" || part == ".DS_Store" || part.starts_with("._"))
}

fn is_decodable_image(data: &[u8]) -> bool {
    image::load_from_memory(data).is_ok()
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

// ---------------------------------------------------------------------------
// ZIP / CBZ
// ---------------------------------------------------------------------------

fn zip_image_entries(path: &Path) -> anyhow::Result<Vec<String>> {
    let file = std::fs::File::open(path)?;
    let archive = zip::ZipArchive::new(file)?;
    // Use file_names() instead of by_index(): file_names() reads from the
    // already-loaded central directory and does zero per-entry I/O.  With
    // by_index() every call would seek to the local file header, causing one
    // NFS round-trip per page — very slow for large archives on a NAS.
    let mut names: Vec<String> = archive
        .file_names()
        .filter(|name| is_zip_image(name))
        .map(|s| s.to_owned())
        .collect();
    names.sort_by(|a, b| natord(a, b));
    Ok(names)
}

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

fn zip_list_entries_raw(path: &Path) -> anyhow::Result<Vec<(String, u64, bool)>> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    // by_index() seeks to each local file header (I/O per entry), but this
    // function is always called from spawn_blocking so the async executor is
    // not stalled.  We need by_index() here because file_names() does not
    // expose the uncompressed size — it is in ZipFileData but pub(crate).
    let mut entries: Vec<(String, u64, bool)> = Vec::new();
    for i in 0..archive.len() {
        if let Ok(entry) = archive.by_index(i)
            && !entry.is_dir()
        {
            let name = entry.name().to_owned();
            if is_ignored_archive_entry(&name) {
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

fn zip_cover_image(path: &Path) -> anyhow::Result<Vec<u8>> {
    for name in zip_image_entries(path)? {
        let (data, _mime) = zip_read_entry(path, &name)?;
        if is_decodable_image(&data) {
            return Ok(data);
        }
    }
    anyhow::bail!("no decodable images in archive")
}

// ---------------------------------------------------------------------------
// RAR / CBR
// ---------------------------------------------------------------------------

#[cfg(feature = "rar")]
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

#[cfg(not(feature = "rar"))]
fn rar_image_entries(_path: &Path) -> anyhow::Result<Vec<String>> {
    anyhow::bail!("RAR support not compiled in (enable the `rar` feature)")
}

#[cfg(feature = "rar")]
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

#[cfg(not(feature = "rar"))]
fn rar_read_entry(_rar_path: &Path, _entry_name: &str) -> anyhow::Result<(Vec<u8>, &'static str)> {
    anyhow::bail!("RAR support not compiled in (enable the `rar` feature)")
}

#[cfg(feature = "rar")]
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
        .filter(|(name, _, _)| !is_ignored_archive_entry(name))
        .collect();
    entries.sort_by(|a, b| natord(&a.0, &b.0));
    Ok(entries)
}

#[cfg(not(feature = "rar"))]
fn rar_list_entries_raw(_path: &Path) -> anyhow::Result<Vec<(String, u64, bool)>> {
    anyhow::bail!("RAR support not compiled in (enable the `rar` feature)")
}

#[cfg(feature = "rar")]
fn rar_cover_image(path: &Path) -> anyhow::Result<Vec<u8>> {
    for name in rar_image_entries(path)? {
        if let Ok((data, _mime)) = rar_read_entry(path, &name)
            && is_decodable_image(&data)
        {
            return Ok(data);
        }
    }
    anyhow::bail!("no decodable images in archive")
}

#[cfg(not(feature = "rar"))]
fn rar_cover_image(_path: &Path) -> anyhow::Result<Vec<u8>> {
    anyhow::bail!("RAR support not compiled in (enable the `rar` feature)")
}

// ---------------------------------------------------------------------------
// 7z / CB7
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
        .filter(|(name, _, _)| !is_ignored_archive_entry(name))
        .collect();
    entries.sort_by(|a, b| natord(&a.0, &b.0));
    Ok(entries)
}

fn sevenz_cover_image(path: &Path) -> anyhow::Result<Vec<u8>> {
    for name in sevenz_image_entries(path)? {
        if let Ok((data, _mime)) = sevenz_read_entry(path, &name)
            && is_decodable_image(&data)
        {
            return Ok(data);
        }
    }
    anyhow::bail!("no decodable images in archive")
}

// ---------------------------------------------------------------------------
// Format dispatchers
// ---------------------------------------------------------------------------

/// Extract the cover image (first image entry) from an archive as raw bytes.
pub fn archive_cover_image(path: &Path) -> anyhow::Result<Vec<u8>> {
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

/// Return a sorted list of image-entry names inside an archive.
pub fn archive_image_entries(path: &Path) -> anyhow::Result<Vec<String>> {
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

/// Read a named entry from an archive and return its raw bytes and MIME type.
pub fn archive_read_entry(
    path: &Path,
    entry_name: &str,
) -> anyhow::Result<(Vec<u8>, &'static str)> {
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

/// Return all entries in an archive as `(name, compressed_size, is_image)` tuples.
pub fn archive_list_entries_raw(path: &Path) -> anyhow::Result<Vec<(String, u64, bool)>> {
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

// ---------------------------------------------------------------------------
// ComicInfo.xml support
// ---------------------------------------------------------------------------

/// Read the raw bytes of `ComicInfo.xml` from a comic archive, if present.
///
/// The filename is matched case-insensitively and may be in a subdirectory.
/// Returns `Ok(None)` when the archive contains no ComicInfo entry.
pub fn archive_read_comic_info(path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    let entries = archive_list_entries_raw(path)?;
    let entry_name = entries
        .iter()
        .find(|(name, _, _)| {
            let lc = name.to_lowercase();
            lc == "comicinfo.xml" || lc.ends_with("/comicinfo.xml")
        })
        .map(|(name, _, _)| name.clone());

    match entry_name {
        None => Ok(None),
        Some(name) => {
            let (bytes, _mime) = archive_read_entry(path, &name)?;
            Ok(Some(bytes))
        }
    }
}

/// Extract a text element from a flat XML string.
///
/// Matches `<Tag>content</Tag>` and `<Tag attrs...>content</Tag>`.  Returns
/// `None` for self-closing elements, missing elements, or empty content.
fn xml_element<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let close_tag = format!("</{tag}>");

    // Match `<Tag>content</Tag>` (exact, no attributes)
    let exact_open = format!("<{tag}>");
    if let Some(start) = xml.find(&exact_open) {
        let content_start = start + exact_open.len();
        let end_off = xml[content_start..].find(&close_tag)?;
        let text = xml[content_start..content_start + end_off].trim();
        return if text.is_empty() { None } else { Some(text) };
    }

    // Match `<Tag ...>content</Tag>` (with attributes)
    let attr_open = format!("<{tag} ");
    if let Some(start) = xml.find(&attr_open) {
        let after = start + attr_open.len();
        let gt_off = xml[after..].find('>')?;
        let before_gt = &xml[after..after + gt_off];
        if before_gt.ends_with('/') {
            return None; // Self-closing
        }
        let content_start = after + gt_off + 1;
        let end_off = xml[content_start..].find(&close_tag)?;
        let text = xml[content_start..content_start + end_off].trim();
        return if text.is_empty() { None } else { Some(text) };
    }

    None
}

/// Unescape the five predefined XML entities.
fn xml_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

/// Parse a ComicInfo.xml byte string and return `(tag_name, value)` pairs
/// ready to be applied with `db::apply_tag`.
///
/// Tag mapping:
/// * `Series`       → `comic/series`        with value
/// * `Number`       → `comic/number`        with value
/// * `Volume`       → `comic/volume`        with value
/// * `Title`        → `comic/title`         with value (skipped when identical to Series)
/// * `Year`         → `comic/year`          with value
/// * `Publisher`    → `comic/publisher`     with value
/// * `LanguageISO`  → `comic/language`      with value
/// * `Format`       → `comic/format`        with value
/// * `AgeRating`    → `comic/age-rating`    with value
/// * `Writer`       → `comic/writer`        with value  (comma-list → multiple)
/// * `Penciller`    → `comic/penciller`     with value  (comma-list → multiple)
/// * `Inker`        → `comic/inker`         with value  (comma-list → multiple)
/// * `Colorist`     → `comic/colorist`      with value  (comma-list → multiple)
/// * `CoverArtist`  → `comic/cover-artist`  with value  (comma-list → multiple)
/// * `Genre`        → `comic/genre`         with value  (comma-list → multiple)
/// * `Tags`         → `comic/tags/VALUE`    no value    (comma-list → multiple flat tags)
/// * `Manga`        → `comic/manga`         no value    (only when "Yes" / "YesAndRightToLeft")
/// * `BlackAndWhite`→ `comic/black-and-white` no value  (only when "Yes")
pub fn parse_comic_info_tags(xml_bytes: &[u8]) -> Vec<(String, String)> {
    let xml = std::str::from_utf8(xml_bytes)
        .unwrap_or("")
        .replace('\r', "");

    let split_csv = |s: &str| -> Vec<String> {
        s.split(',')
            .map(|p| p.trim().to_owned())
            .filter(|p| !p.is_empty())
            .collect()
    };

    let mut tags: Vec<(String, String)> = Vec::new();

    // Single-value fields
    for (xml_tag, ft_tag) in [
        ("Series", "comic/series"),
        ("Number", "comic/number"),
        ("Volume", "comic/volume"),
        ("Year", "comic/year"),
        ("Publisher", "comic/publisher"),
        ("LanguageISO", "comic/language"),
        ("Format", "comic/format"),
        ("AgeRating", "comic/age-rating"),
    ] {
        if let Some(v) = xml_element(&xml, xml_tag) {
            tags.push((ft_tag.to_owned(), xml_unescape(v)));
        }
    }

    // Title: skip when identical to Series (avoids duplicate information)
    let series_val = xml_element(&xml, "Series").unwrap_or("").to_owned();
    if let Some(v) = xml_element(&xml, "Title")
        && v != series_val
    {
        tags.push(("comic/title".to_owned(), xml_unescape(v)));
    }

    // Comma-list creator fields
    for (xml_tag, ft_tag) in [
        ("Writer", "comic/writer"),
        ("Penciller", "comic/penciller"),
        ("Inker", "comic/inker"),
        ("Colorist", "comic/colorist"),
        ("CoverArtist", "comic/cover-artist"),
    ] {
        if let Some(v) = xml_element(&xml, xml_tag) {
            for val in split_csv(v) {
                tags.push((ft_tag.to_owned(), xml_unescape(&val)));
            }
        }
    }

    // Genre: comma-list with value
    if let Some(v) = xml_element(&xml, "Genre") {
        for val in split_csv(v) {
            tags.push(("comic/genre".to_owned(), xml_unescape(&val)));
        }
    }

    // Tags: comma-list → flat tags under comic/tags/ (no value)
    if let Some(v) = xml_element(&xml, "Tags") {
        for val in split_csv(v) {
            let val = xml_unescape(&val);
            if !val.is_empty() {
                tags.push((format!("comic/tags/{}", val), String::new()));
            }
        }
    }

    // Boolean flags
    if let Some(v) = xml_element(&xml, "Manga")
        && (v == "Yes" || v == "YesAndRightToLeft")
    {
        tags.push(("comic/manga".to_owned(), String::new()));
    }
    if let Some(v) = xml_element(&xml, "BlackAndWhite")
        && v == "Yes"
    {
        tags.push(("comic/black-and-white".to_owned(), String::new()));
    }

    tags
}

// ---------------------------------------------------------------------------
// Directory image listing
// ---------------------------------------------------------------------------

/// Image extensions shown in the directory viewer.
const DIR_IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "bmp", "avif", "tiff", "tif", "heic", "heif", "arw",
    "cr2", "cr3", "nef", "orf", "rw2", "dng", "raf", "pef", "srw", "raw", "3fr", "x3f", "rwl",
    "iiq", "mef", "mos",
];

fn is_dir_image(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    DIR_IMAGE_EXTS.contains(&ext.as_str())
}

/// Query params for `GET /api/dir-images`.
#[derive(Deserialize)]
pub struct DirImagesParams {
    path: String,
    dir: Option<String>,
    root_id: Option<usize>,
}

#[derive(Serialize)]
struct DirImagesResponse {
    images: Vec<String>,
}

/// `GET /api/dir-images` — return all image file paths under a directory.
pub async fn api_dir_images(
    State(state): State<Arc<AppState>>,
    Query(params): Query<DirImagesParams>,
) -> Response {
    let db_root = match root_from_dir_or_id(&state, params.dir.as_deref(), params.root_id) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "Unknown root or missing dir").into_response(),
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
// Archive API handlers
// ---------------------------------------------------------------------------

/// Query params for `GET /api/zip/pages` and `GET /api/zip/entries`.
#[derive(Deserialize)]
pub struct ZipListParams {
    pub path: String,
    pub dir: Option<String>,
    pub root_id: Option<usize>,
}

#[derive(Serialize)]
struct ZipPagesResponse {
    pages: Vec<String>,
    count: usize,
}

/// `GET /api/zip/pages` — list the image-entry names in an archive.
pub async fn api_zip_pages(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ZipListParams>,
) -> Response {
    let db_root = match root_from_dir_or_id(&state, params.dir.as_deref(), params.root_id) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "Unknown root or missing dir").into_response(),
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

/// Query params for `GET /api/zip/page`.
#[derive(Deserialize)]
pub struct ZipPageParams {
    path: String,
    page: usize,
    dir: Option<String>,
    root_id: Option<usize>,
}

/// `GET /api/zip/page` — serve a single page from an archive by index.
pub async fn api_zip_page(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ZipPageParams>,
) -> Response {
    let db_root = match root_from_dir_or_id(&state, params.dir.as_deref(), params.root_id) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "Unknown root or missing dir").into_response(),
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

/// Both cache paths for a single ZIP page thumbnail, computed in one blocking call.
struct ZipThumbPaths {
    thumb: PathBuf,
    salient: PathBuf,
}

/// Compute both cache paths for a ZIP page thumbnail.
///
/// This is a **blocking** function (calls `std::fs::metadata` and
/// `std::fs::create_dir_all`) and must be called from `spawn_blocking`.
/// It is a single function so that the filesystem metadata is read only once
/// and the cache directory is created only once per request.
fn zip_page_cache_paths(abs: &Path, root: &Path, page: usize) -> Option<ZipThumbPaths> {
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
    let dir = root.join(".filetag").join("cache").join("zip-pages");
    std::fs::create_dir_all(&dir).ok()?;
    let base = format!("{mtime}_{size}_{stem}_p{page}");
    Some(ZipThumbPaths {
        thumb: dir.join(format!("{base}.thumb.webp")),
        salient: dir.join(format!("{base}.thumb.sp")),
    })
}

/// Query params for `GET /api/zip/thumb`.
#[derive(Deserialize)]
pub struct ZipThumbParams {
    path: String,
    page: usize,
    dir: Option<String>,
    root_id: Option<usize>,
    /// Priority hint: `"high"` uses the dedicated high-priority semaphore.
    priority: Option<String>,
}

/// `GET /api/zip/thumb` — return a JPEG thumbnail for an archive page.
pub async fn api_zip_thumb(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ZipThumbParams>,
) -> Response {
    let db_root = match root_from_dir_or_id(&state, params.dir.as_deref(), params.root_id) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "Unknown root or missing dir").into_response(),
    };
    let features = crate::state::load_features_for(&state, &db_root.root);
    let (abs, cache_root) = match resolve_preview(&state, &db_root.root, &params.path) {
        Some(t) => t,
        None => return (StatusCode::BAD_REQUEST, "Invalid path").into_response(),
    };
    let page_idx = params.page;

    // Compute cache paths in spawn_blocking: zip_page_cache_paths calls
    // std::fs::metadata (stat the archive) and std::fs::create_dir_all, both
    // blocking syscalls that may take hundreds of milliseconds over NFS.
    // Previously these were called directly in the async handler, stalling the
    // executor thread on every thumbnail request even when the cache was warm.
    let abs2 = abs.clone();
    let cache_root2 = cache_root.clone();
    let paths =
        tokio::task::spawn_blocking(move || zip_page_cache_paths(&abs2, &cache_root2, page_idx))
            .await
            .ok()
            .flatten();

    let Some(ZipThumbPaths {
        thumb: cache,
        salient: sp_path,
    }) = paths
    else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Cache unavailable").into_response();
    };

    if let Ok(data) = tokio::fs::read(&cache).await {
        // Already cached — attach salient headers if sidecar exists.
        let salient = if features.saliency_pose {
            let cached_sp = crate::preview::read_salient_cache_pub(&sp_path);
            if cached_sp.is_none() && crate::saliency::pose_model_ready() {
                let sp = sp_path.clone();
                let data2 = data.clone();
                tokio::spawn(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        let img = image::load_from_memory(&data2).ok()?;
                        crate::saliency::detect_salient_point(&img, false)
                    })
                    .await
                    .ok()
                    .flatten();
                    crate::preview::write_salient_cache_pub(&sp, result.map(|s| (s.cx, s.cy)));
                });
            }
            cached_sp.flatten()
        } else {
            None
        };
        let resp = ([(header::CONTENT_TYPE, "image/webp")], data).into_response();
        return crate::preview::attach_salient_headers_pub(resp, salient);
    }

    let _permit = match crate::state::thumb_semaphore(params.priority.as_deref())
        .acquire()
        .await
    {
        Ok(p) => p,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "thumbnail limiter closed",
            )
                .into_response();
        }
    };

    let abs2 = abs.clone();
    let result = tokio::task::spawn_blocking(move || {
        let pages = archive_image_entries(&abs2)?;
        let name = pages
            .into_iter()
            .nth(page_idx)
            .ok_or_else(|| anyhow::anyhow!("page out of range"))?;
        archive_read_entry(&abs2, &name)
    })
    .await;

    if let Ok(Ok((img_bytes, mime))) = result {
        // Run salient detection on full-resolution bytes before downscaling.
        let salient = if features.saliency_pose && crate::saliency::pose_model_ready() {
            let bytes2 = img_bytes.clone();
            tokio::task::spawn_blocking(move || {
                let img = image::load_from_memory(&bytes2).ok()?;
                crate::saliency::detect_salient_point(&img, false)
            })
            .await
            .ok()
            .flatten()
            .map(|s| (s.cx, s.cy))
        } else {
            None
        };
        let small = {
            let bytes = img_bytes.clone();
            tokio::task::spawn_blocking(move || -> Option<Vec<u8>> {
                let orient = if bytes.starts_with(&[0xFF, 0xD8]) {
                    crate::preview::jpeg_exif_orientation(&bytes)
                } else {
                    1
                };
                let img = image::load_from_memory(&bytes).ok()?;
                let img = crate::preview::apply_exif_orientation(img, orient);
                let img = img.resize(400, 400, image::imageops::FilterType::Lanczos3);
                crate::preview::encode_lossy_webp_pub(&img, 80.0)
            })
            .await
            .ok()
            .flatten()
        };
        if let Some(small) = small {
            let _ = tokio::fs::write(&cache, &small).await;
            crate::preview::write_salient_cache_pub(&sp_path, salient);
            let resp = ([(header::CONTENT_TYPE, "image/webp")], small).into_response();
            return crate::preview::attach_salient_headers_pub(resp, salient);
        }
        return ([(header::CONTENT_TYPE, mime)], img_bytes).into_response();
    }
    (StatusCode::NOT_FOUND, "Page not found").into_response()
}

// ---------------------------------------------------------------------------
// ZIP entry record + entries listing
// ---------------------------------------------------------------------------

/// Ensure a virtual zip-entry record exists in the `files` table and return its id.
pub fn ensure_zip_entry_record(conn: &rusqlite::Connection, db_path: &str) -> anyhow::Result<i64> {
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

/// `GET /api/zip/entries` — list all entries in an archive with tag counts.
pub async fn api_zip_entries(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ZipListParams>,
) -> Response {
    let db_root = match root_from_dir_or_id(&state, params.dir.as_deref(), params.root_id) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "Unknown root or missing dir").into_response(),
    };
    let abs = match preview_safe_path(&db_root.root, &params.path) {
        Some(p) => p,
        None => return (StatusCode::BAD_REQUEST, "Invalid path").into_response(),
    };

    let raw: Vec<(String, u64, bool)> =
        match tokio::task::spawn_blocking(move || archive_list_entries_raw(&abs)).await {
            Ok(Ok(v)) => v,
            _ => return (StatusCode::UNPROCESSABLE_ENTITY, "Cannot read archive").into_response(),
        };

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn unique_temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "filetag_archive_{name}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn test_jpeg() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(8, 8, image::Rgb([20, 80, 140]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut out, image::ImageFormat::Jpeg)
            .unwrap();
        out.into_inner()
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        for (name, data) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn zip_entries_ignore_macosx_metadata_images() {
        let dir = unique_temp_dir("metadata");
        let path = dir.join("sample.cbz");
        let jpeg = test_jpeg();
        write_zip(
            &path,
            &[
                ("pages/001.jpg", &jpeg),
                ("__MACOSX/pages/._001.jpg", b"not an image"),
                ("pages/.DS_Store", b"noise"),
            ],
        );

        assert_eq!(zip_image_entries(&path).unwrap(), vec!["pages/001.jpg"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn zip_cover_skips_invalid_image_candidates() {
        let dir = unique_temp_dir("invalid_cover");
        let path = dir.join("sample.cbz");
        let jpeg = test_jpeg();
        write_zip(
            &path,
            &[("pages/000.jpg", b"not an image"), ("pages/001.jpg", &jpeg)],
        );

        let cover = archive_cover_image(&path).unwrap();
        let decoded = image::load_from_memory(&cover).unwrap();
        assert_eq!(decoded.width(), 8);
        let _ = std::fs::remove_dir_all(dir);
    }
}
