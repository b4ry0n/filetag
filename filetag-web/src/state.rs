//! Shared application state and helpers for `filetag-web`.
//!
//! [`AppState`] is the Axum extractor state, holding one [`TagRoot`] per
//! loaded database.  All database opens for file operations go through
//! [`open_for_file_op`] — the one sanctioned entry point that routes to the
//! correct child database.

use std::path::{Path, PathBuf};

use anyhow::Context;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use filetag_lib::db;

use crate::auth::SessionStore;
use filetag_lib::db::TagRoot;
use rusqlite::Connection;

use crate::ai::AiProgress;
use crate::face::{FaceProgress, ModelDownloadProgress};

// ---------------------------------------------------------------------------
// Feature flags
// ---------------------------------------------------------------------------

/// Feature flags that govern the use of external tools and optional capabilities.
///
/// All flags default to `false` (fully portable, no external tools required).
/// Operators enable them per database root via the Settings panel.
#[derive(Clone, Copy, Debug, Default)]
pub struct Features {
    /// Enable ffmpeg for video transcoding, HLS streaming, trickplay sprites,
    /// and video thumbnails. Also enables ffmpeg as an image-preview fallback.
    pub video: bool,
    /// Enable ImageMagick (`magick`/`convert`) and `sips` (macOS) for HEIC,
    /// PSD, XCF, and other exotic image formats. Also enables `dcraw` as a
    /// RAW-extraction fallback.
    pub imagemagick: bool,
    /// Enable PDF thumbnail generation via `pdftoppm` (poppler) or ImageMagick
    /// + Ghostscript.
    pub pdf: bool,
}

/// Load feature flags from the per-root settings table.
pub fn load_features(conn: &Connection) -> Features {
    let get = |key: &str| -> bool {
        db::get_setting(conn, key)
            .ok()
            .flatten()
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    };
    Features {
        video: get("feature.video"),
        imagemagick: get("feature.imagemagick"),
        pdf: get("feature.pdf"),
    }
}

/// Load feature flags for the database root at `root_path`.
/// Returns `Features::default()` (all off) when the root is not found or the
/// settings table cannot be read.
pub fn load_features_for(state: &AppState, root_path: &Path) -> Features {
    let Some(tag_root) = state.roots.iter().find(|r| r.root == root_path) else {
        return Features::default();
    };
    match open_conn(tag_root) {
        Ok(conn) => load_features(&conn),
        Err(_) => Features::default(),
    }
}

// ---------------------------------------------------------------------------
// Concurrency limiters
// ---------------------------------------------------------------------------

/// Limit concurrent heavy thumbnail/extraction operations to prevent spawning
/// too many ffmpeg/ffprobe/unrar processes at once when browsing directories
/// with many large media files.
pub static THUMB_LIMITER: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// Separate semaphore for video sprite generation.  Sprite builds can run up
/// to 4 in parallel without saturating the CPU, and must not block the
/// thumbnail queue (which has only 1 permit).
pub static VTHUMB_LIMITER: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

/// Semaphore for full video transcoding (serve_transcoded_mp4 slow path).
/// Kept separate from THUMB_LIMITER so that a long-running transcode (e.g.
/// a video playing in picture-in-picture mode) does not block thumbnail
/// generation for the rest of the session.
pub static TRANSCODE_LIMITER: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

// ---------------------------------------------------------------------------
// Application state
// ---------------------------------------------------------------------------

/// Global Axum application state.
pub struct AppState {
    /// All database roots loaded at startup, indexed by their position.
    pub roots: Vec<TagRoot>,
    /// Progress information for the current AI batch job (if any).
    pub ai_progress: std::sync::Mutex<AiProgress>,
    /// Progress information for the current face-analysis batch (if any).
    pub face_progress: std::sync::Mutex<FaceProgress>,
    /// Progress for the model download (if active).
    pub model_download: std::sync::Mutex<ModelDownloadProgress>,
    /// Session store for optional password authentication.
    pub sessions: SessionStore,
}

/// Returns true when `abs_path` is covered by any loaded database root.
///
/// A file is covered when there is a loaded `TagRoot` that:
///   1. resides on the same filesystem as the file (volume/device match), AND
///   2. whose root directory is an ancestor of `abs_path`.
///
/// This correctly handles mounted volumes that have their own database: even if
/// the file appears inside the directory tree of a parent root, the mount's own
/// `TagRoot` makes it covered. When volume information is unavailable the volume
/// check is skipped and the path-ancestor check alone decides.
pub fn file_is_covered(state: &AppState, abs_path: &Path) -> bool {
    let file_vol = db::volume_id(abs_path);
    state.roots.iter().any(|root| {
        let vol_match = match (file_vol, root.dev) {
            (Some(fv), Some(rv)) => fv == rv,
            _ => true, // volume unknown → skip volume check
        };
        vol_match && abs_path.starts_with(&root.root)
    })
}

/// Wraps any roots that share the same display name by appending ` 1`, ` 2`,
/// … to disambiguate them.  Unique names are returned unchanged.
pub fn resolve_names(names: Vec<String>) -> Vec<String> {
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

// ---------------------------------------------------------------------------
// Error handling
// ---------------------------------------------------------------------------

/// [`anyhow::Error`] wrapper that converts to a JSON response.
pub struct AppError(pub anyhow::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let msg = self.0.to_string();
        let lower = msg.to_ascii_lowercase();
        let status = if lower.contains("not found") || lower.contains("no database loaded") {
            StatusCode::NOT_FOUND
        } else if lower.contains("dir ")
            || lower.contains("invalid ")
            || lower.contains("unknown root")
            || lower.contains("not within any loaded database root")
            || lower.contains("must ")
            || lower.contains("parameter is required")
        {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        let body = serde_json::json!({ "error": msg });
        (status, Json(body)).into_response()
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

// ---------------------------------------------------------------------------
// Database connection helpers
// ---------------------------------------------------------------------------

/// Open a connection to a known database root. Sets WAL mode, foreign keys,
/// and a generous busy timeout. Suitable for settings/config reads and any
/// operation that targets the root DB itself.
pub fn open_conn(db_root: &TagRoot) -> anyhow::Result<Connection> {
    let conn = Connection::open(&db_root.db_path).context("opening database")?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )?;
    Ok(conn)
}

/// The ONE sanctioned way to open a database for any operation that reads or
/// writes file records or tags.
///
/// Walks up from the file's directory to find the most specific database
/// (including child databases), so tags are always written to the correct DB.
///
/// Returns `(conn, effective_root, effective_rel_path)`.
///
/// For archive entries (`"some/archive.zip::entry.jpg"`), routing is based on
/// the archive file; `effective_rel` includes the `::entry` suffix relative to
/// the found database root.
///
/// Never call `Connection::open` or `open_conn` directly for file operations —
/// use this function instead.
pub fn open_for_file_op(
    db_root: &TagRoot,
    path: &str,
) -> anyhow::Result<(Connection, PathBuf, String)> {
    open_for_file_op_under(&db_root.root, path)
}

/// Same as `open_for_file_op` but takes a raw root path.  Used by background
/// worker tasks that capture root by value rather than holding a `&TagRoot`.
pub fn open_for_file_op_under(
    root: &Path,
    path: &str,
) -> anyhow::Result<(Connection, PathBuf, String)> {
    // For archive entries, route on the archive file itself.
    let fs_path = if let Some(zip_part) = path.split_once("::").map(|(z, _)| z) {
        preview_safe_path(root, zip_part)
            .ok_or_else(|| anyhow::anyhow!("invalid path '{}': escapes root", zip_part))?
    } else {
        safe_path(root, path)?
    };

    let start = fs_path.parent().unwrap_or(&fs_path);
    let (conn, effective_root) = db::find_and_open(start)?;

    // Compute the path relative to the found (child) database root.
    let effective_rel = if let Some(entry) = path.split_once("::").map(|(_, e)| e) {
        let zip_rel = db::relative_to_root(&fs_path, &effective_root)?;
        format!("{}::{}", zip_rel, entry)
    } else {
        db::relative_to_root(&fs_path, &effective_root)?
    };

    Ok((conn, effective_root, effective_rel))
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Return the deepest `TagRoot` whose root path contains `abs`.
///
/// This is the single source of truth for determining which database root owns
/// a given path. All API handlers that need to access `.filetag/` data call
/// this function. No other root-resolution functions exist.
pub fn root_for_dir<'a>(state: &'a AppState, abs: &Path) -> Option<&'a TagRoot> {
    let abs_path = abs.canonicalize().unwrap_or_else(|_| abs.to_path_buf());
    // Containment check (pure path operation) is performed before any
    // filesystem access so that volume_id() is never called on an
    // unvalidated user-supplied path (CWE-022 / path injection).
    state
        .roots
        .iter()
        .filter(|r| {
            abs_path.starts_with(&r.root) && {
                match (db::volume_id(&abs_path), r.dev) {
                    (Some(av), Some(rv)) => av == rv,
                    _ => true,
                }
            }
        })
        .max_by_key(|r| r.root.as_os_str().len())
}

/// Resolve a relative path under `root`, rejecting directory traversal.
pub fn safe_path(root: &Path, rel: &str) -> anyhow::Result<PathBuf> {
    preview_safe_path(root, rel)
        .ok_or_else(|| anyhow::anyhow!("invalid path '{}': escapes root or does not exist", rel))
}

/// Validate a relative path under a root and return both the absolute path
/// and the correct owning root for cache/preview purposes.
///
/// Handlers that need to write or read cache artefacts MUST use this function
/// (or `root_for_dir` directly) so the correct `.filetag/cache/` directory is
/// always selected.
pub fn resolve_preview(
    state: &AppState,
    root: &Path,
    rel_path: &str,
) -> Option<(PathBuf, PathBuf)> {
    let abs = preview_safe_path(root, rel_path)?;
    let effective_root = root_for_dir(state, &abs)
        .map(|r| r.root.clone())
        .unwrap_or_else(|| root.to_path_buf());
    Some((abs, effective_root))
}

/// Sanitise a URL path component so it cannot escape `root`.
/// Unlike `safe_path`, this does not require the file to exist first.
///
/// Symlinks that resolve to a path outside `root` are intentional (the user
/// created them) and are therefore allowed.  Path traversal via `..` is
/// already prevented by the component filter below, so removing the
/// `starts_with(root)` restriction is safe.
pub fn preview_safe_path(root: &Path, rel: &str) -> Option<PathBuf> {
    use std::path::Component;
    let mut result = root.to_path_buf();
    for component in std::path::Path::new(rel.trim_start_matches('/')).components() {
        match component {
            Component::Normal(name) => result.push(name),
            Component::CurDir => {}
            _ => return None,
        }
    }
    // Canonicalise to resolve symlinks.  Symlinks pointing outside the root
    // are allowed; path traversal via `..` is blocked by the filter above.
    std::fs::canonicalize(&result).ok()
}

pub use filetag_lib::parse_tag;

/// Best-effort terminal column width.  Falls back to 80 when unavailable.
/// Reads the `COLUMNS` environment variable (set by most interactive shells).
pub fn terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(80)
}
