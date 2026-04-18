//! Request and response types for the `filetag-web` HTTP API.
//!
//! All response types derive [`serde::Serialize`]; all request types derive
//! [`serde::Deserialize`].  No numeric root IDs are exchanged between the
//! frontend and backend — all requests carry `dir: Option<String>` (absolute
//! filesystem path) and the backend resolves the root via `root_for_dir`.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// API response types
// ---------------------------------------------------------------------------

/// Database statistics returned by `GET /api/info`.
#[derive(Serialize)]
pub struct ApiInfo {
    /// Absolute path to the database root directory.
    pub root: String,
    /// Total number of indexed files.
    pub files: i64,
    /// Total number of distinct tags.
    pub tags: i64,
    /// Total number of file–tag assignments.
    pub assignments: i64,
    /// Sum of all indexed file sizes in bytes.
    pub total_size: i64,
}

/// A tag with its usage count, as returned by `GET /api/tags`.
#[derive(Serialize)]
pub struct ApiTag {
    /// Tag name, possibly containing `/` separators (e.g. `genre/rock`).
    pub name: String,
    /// Number of files that carry this tag.
    pub count: i64,
    /// Optional CSS colour string (e.g. `#ff0000`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// A directory listing as returned by `GET /api/files`.
#[derive(Serialize)]
pub struct ApiDirListing {
    /// The path that was listed (relative to the database root).
    pub path: String,
    /// Absolute filesystem path of the deepest database root for this directory.
    pub root_path: String,
    pub entries: Vec<ApiDirEntry>,
}

/// A single entry in an [`ApiDirListing`].
#[derive(Serialize)]
pub struct ApiDirEntry {
    pub name: String,
    pub is_dir: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtime: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag_count: Option<i64>,
    /// Set for virtual-root tile entries; absolute filesystem path of the database root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_path: Option<String>,
    /// False when the file is on a different filesystem than the database root.
    /// Tagging is not allowed in that case.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub covered: Option<bool>,
}

/// Full file detail as returned by `GET /api/file`.
#[derive(Serialize)]
pub struct ApiFileDetail {
    pub path: String,
    pub size: i64,
    pub file_id: Option<String>,
    pub mtime: i64,
    pub indexed_at: String,
    pub tags: Vec<ApiFileTag>,
    /// False when the file is on a different filesystem than the database root.
    pub covered: bool,
    /// Video duration in seconds (only set for video files).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
}

/// A tag attached to a file, optionally with a value.
#[derive(Serialize)]
pub struct ApiFileTag {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// Search result envelope returned by `GET /api/search`.
#[derive(Serialize)]
pub struct ApiSearchResult {
    pub query: String,
    pub results: Vec<ApiSearchEntry>,
}

/// A single file match within an [`ApiSearchResult`].
#[derive(Serialize)]
pub struct ApiSearchEntry {
    pub path: String,
    pub tags: Vec<ApiFileTag>,
}

/// A database root as listed by `GET /api/roots`.
#[derive(Serialize)]
pub struct ApiRoot {
    /// Index into the server's `roots` array; used as a `root` query parameter.
    pub id: usize,
    pub name: String,
    /// Absolute path to the root directory.
    pub path: String,
    pub sort_order: i64,
    /// False when this root is a subdirectory of another loaded root.
    /// Non-entry-point roots are not shown as top-level navigation tiles.
    pub entry_point: bool,
}

// ---------------------------------------------------------------------------
// API request types
// ---------------------------------------------------------------------------

/// Query params for `GET /api/files`.
///
/// `dir` is the absolute filesystem path of the directory to list.  When
/// absent the server returns the virtual root (entry-point tiles).
#[derive(Deserialize)]
pub struct FileListParams {
    /// Absolute filesystem path of the directory to list.
    pub dir: Option<String>,
    #[serde(default)]
    pub show_hidden: bool,
}

/// Query params for `GET /api/search`.
#[derive(Deserialize)]
pub struct SearchParams {
    pub q: String,
    /// Absolute filesystem path of the currently browsed directory.
    /// The backend resolves the active root from this path.
    pub dir: Option<String>,
}

/// Query params for `GET /api/file`.
#[derive(Deserialize)]
pub struct FileDetailParams {
    pub path: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
}

/// Body for `POST /api/tag` and `POST /api/untag`.
#[derive(Deserialize)]
pub struct TagRequest {
    pub path: String,
    pub tags: Vec<String>,
    /// Absolute filesystem path of the currently browsed directory.
    /// The backend resolves the entry-point root from this path.
    pub dir: Option<String>,
}

/// Generic query param carrying the current browsing directory.
/// The backend resolves the active (deepest) root from this path.
#[derive(Deserialize, Default)]
pub struct DirParam {
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
}

/// Body for `POST /api/rename-db`.
#[derive(Deserialize)]
pub struct RenameDbRequest {
    /// Absolute filesystem path of the database root directory to rename.
    pub dir: String,
    pub name: String,
}

/// Body for `POST /api/reorder-roots`.
#[derive(Deserialize)]
pub struct ReorderRootsRequest {
    /// Root directory paths in the desired new order (first element = sort position 0).
    pub order: Vec<String>,
}

/// Body for `POST /api/tag-color`.
#[derive(Deserialize)]
pub struct TagColorRequest {
    pub name: String,
    /// `None` clears the colour.
    pub color: Option<String>,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
}

/// Body for `POST /api/rename-tag`.
#[derive(Deserialize)]
pub struct RenameTagRequest {
    pub name: String,
    pub new_name: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
}

/// Body for `POST /api/delete-tag`.
#[derive(Deserialize)]
pub struct DeleteTagRequest {
    pub name: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
}

/// Body for `POST /api/cache/clear`.
///
/// The active root is always determined from the `dir` query parameter.
/// - `paths` present: clear cache for exactly those file paths.  
/// - `all` = true: wipe the entire cache directory of the active root.  
/// - Neither: enumerate the directory named by `dir` and clear its entries.
#[derive(Deserialize, Default)]
pub struct CacheClearBody {
    pub paths: Option<Vec<String>>,
    /// When `true`, wipe the entire `.filetag/cache/` directory of the active root.
    pub all: Option<bool>,
}

/// Query params for `GET /api/settings`.
#[derive(Deserialize, Default)]
pub struct SettingsParams {
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
}

/// Body for `POST /api/settings`.
#[derive(Deserialize)]
pub struct SettingsBody {
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
    /// Minimum number of trickplay sprites for a video.
    pub sprite_min: Option<u32>,
    /// Maximum number of trickplay sprites for a video.
    pub sprite_max: Option<u32>,
}
