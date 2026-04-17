//! Request and response types for the `filetag-web` HTTP API.
//!
//! All response types derive [`serde::Serialize`]; all request types derive
//! [`serde::Deserialize`].  Query-parameter structs use `pub root: Option<usize>`;
//! JSON body structs use `pub root_id: Option<usize>`.

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
    /// Set for virtual-root entries; identifies which database root to enter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_id: Option<usize>,
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
#[derive(Deserialize)]
pub struct FileListParams {
    pub root: Option<usize>,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub show_hidden: bool,
}

/// Query params for `GET /api/search`.
#[derive(Deserialize)]
pub struct SearchParams {
    pub q: String,
    pub root: Option<usize>,
}

/// Query params for `GET /api/file`.
#[derive(Deserialize)]
pub struct FileDetailParams {
    pub path: String,
    pub root: Option<usize>,
}

/// Body for `POST /api/tag` and `POST /api/untag`.
#[derive(Deserialize)]
pub struct TagRequest {
    pub path: String,
    pub tags: Vec<String>,
    pub root_id: Option<usize>,
}

/// Generic query param carrying only a `root` index.
#[derive(Deserialize, Default)]
pub struct RootParam {
    pub root: Option<usize>,
}

/// Body for `POST /api/rename-db`.
#[derive(Deserialize)]
pub struct RenameDbRequest {
    pub root_id: usize,
    pub name: String,
}

/// Body for `POST /api/reorder-roots`.
#[derive(Deserialize)]
pub struct ReorderRootsRequest {
    /// Root IDs in the desired new order (first element = sort position 0).
    pub order: Vec<usize>,
}

/// Body for `POST /api/tag-color`.
#[derive(Deserialize)]
pub struct TagColorRequest {
    pub name: String,
    /// `None` clears the colour.
    pub color: Option<String>,
    pub root_id: Option<usize>,
}

/// Body for `POST /api/rename-tag`.
#[derive(Deserialize)]
pub struct RenameTagRequest {
    pub name: String,
    pub new_name: String,
    pub root_id: Option<usize>,
}

/// Body for `POST /api/delete-tag`.
#[derive(Deserialize)]
pub struct DeleteTagRequest {
    pub name: String,
    pub root_id: Option<usize>,
}

/// Body for `POST /api/cache/clear`. If `paths` is `Some`, only those files'
/// cache entries are removed. If `None` (or missing), the entire cache is cleared.
#[derive(Deserialize, Default)]
pub struct CacheClearBody {
    pub paths: Option<Vec<String>>,
    pub dir: Option<String>,
}

/// Query params for `GET /api/settings`.
#[derive(Deserialize)]
pub struct SettingsParams {
    pub root: Option<usize>,
}

/// Body for `POST /api/settings`.
#[derive(Deserialize)]
pub struct SettingsBody {
    pub root_id: Option<usize>,
    /// Minimum number of trickplay sprites for a video.
    pub sprite_min: Option<u32>,
    /// Maximum number of trickplay sprites for a video.
    pub sprite_max: Option<u32>,
}
