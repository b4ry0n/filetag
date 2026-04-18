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
use filetag_lib::db::TagRoot;
use rusqlite::Connection;

use crate::ai::AiProgress;

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

// ---------------------------------------------------------------------------
// Application state
// ---------------------------------------------------------------------------

/// Global Axum application state.
pub struct AppState {
    /// All database roots loaded at startup, indexed by their position.
    pub roots: Vec<TagRoot>,
    /// Progress information for the current AI batch job (if any).
    pub ai_progress: std::sync::Mutex<AiProgress>,
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

/// [`anyhow::Error`] wrapper that converts to an HTTP 500 JSON response.
pub struct AppError(pub anyhow::Error);

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
    state
        .roots
        .iter()
        .filter(|r| abs.starts_with(&r.root))
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
    // Re-canonicalise to catch symlinks that escape root
    match std::fs::canonicalize(&result) {
        Ok(canonical) if canonical.starts_with(root) => Some(canonical),
        Ok(_) => None,
        // File may not exist yet (e.g. wrong path) – just reject
        Err(_) => None,
    }
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
