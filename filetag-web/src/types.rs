use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// API response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct ApiInfo {
    pub root: String,
    pub files: i64,
    pub tags: i64,
    pub assignments: i64,
    pub total_size: i64,
}

#[derive(Serialize)]
pub struct ApiTag {
    pub name: String,
    pub count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Serialize)]
pub struct ApiDirListing {
    pub path: String,
    pub entries: Vec<ApiDirEntry>,
}

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

#[derive(Serialize)]
pub struct ApiFileTag {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Serialize)]
pub struct ApiSearchResult {
    pub query: String,
    pub results: Vec<ApiSearchEntry>,
}

#[derive(Serialize)]
pub struct ApiSearchEntry {
    pub path: String,
    pub tags: Vec<ApiFileTag>,
}

#[derive(Serialize)]
pub struct ApiRoot {
    pub id: usize,
    pub name: String,
    pub path: String,
    pub sort_order: i64,
    /// False when this root is a subdirectory of another loaded root.
    /// Non-entry-point roots are not shown as top-level navigation tiles.
    pub entry_point: bool,
}

// ---------------------------------------------------------------------------
// API request types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct FileListParams {
    pub root: Option<usize>,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub show_hidden: bool,
}

#[derive(Deserialize)]
pub struct SearchParams {
    pub q: String,
    pub root: Option<usize>,
}

#[derive(Deserialize)]
pub struct FileDetailParams {
    pub path: String,
    pub root: Option<usize>,
}

#[derive(Deserialize)]
pub struct TagRequest {
    pub path: String,
    pub tags: Vec<String>,
    pub root_id: Option<usize>,
}

#[derive(Deserialize, Default)]
pub struct RootParam {
    pub root: Option<usize>,
}

#[derive(Deserialize)]
pub struct RenameDbRequest {
    pub root_id: usize,
    pub name: String,
}

#[derive(Deserialize)]
pub struct ReorderRootsRequest {
    /// Root IDs in the desired new order (first element = sort position 0).
    pub order: Vec<usize>,
}

#[derive(Deserialize)]
pub struct TagColorRequest {
    pub name: String,
    pub color: Option<String>,
    pub root_id: Option<usize>,
}

#[derive(Deserialize)]
pub struct RenameTagRequest {
    pub name: String,
    pub new_name: String,
    pub root_id: Option<usize>,
}

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
}
