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
    /// Registered synonyms (aliases) for this tag.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub synonyms: Vec<String>,
    /// True when at least one file-tag assignment carries a non-empty value
    /// (i.e. this tag is used in `key=value` style).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub has_values: bool,
}

/// Query parameters for `GET /api/tag-values`.
#[derive(Deserialize)]
pub struct TagValuesParams {
    /// Tag name to query values for.
    pub name: String,
    /// Optional absolute directory path for root resolution.
    pub dir: Option<String>,
}

/// A single value variant for a k/v tag, returned by `GET /api/tag-values`.
#[derive(Serialize)]
pub struct ApiTagValue {
    /// The concrete value string (e.g. `"2024"`, `"Miles Davis"`).
    pub value: String,
    /// Number of files carrying this tag with this value.
    pub count: i64,
}

/// A subject as returned by `GET /api/subjects`.
#[derive(Serialize)]
pub struct ApiSubject {
    /// The subject label (e.g. `"person/alice"` or `"car-1"`).
    pub name: String,
    /// Number of distinct files that carry at least one tag under this subject.
    pub count: i64,
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
    /// True when this entry is a symbolic link.  All operations act on the
    /// link target; the link itself never receives its own tags or thumbnails.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_symlink: Option<bool>,
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

/// A tag attached to a file, optionally with a value and a subject group.
#[derive(Serialize)]
pub struct ApiFileTag {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Subject group; absent when the tag was applied without a subject.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// True when the tag comes from the subject entity (implicit), not directly from the file.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub implicit: bool,
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
    /// Optional subject group for the tags being added/removed.
    pub subject: Option<String>,
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

/// Body for `POST /api/rename-subject`.
#[derive(Deserialize)]
pub struct RenameSubjectRequest {
    pub name: String,
    pub new_name: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
}

/// Body for `POST /api/create-subject`.
#[derive(Deserialize)]
pub struct CreateSubjectRequest {
    pub name: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
}

/// Body for `POST /api/delete-subject`.
#[derive(Deserialize)]
pub struct DeleteSubjectRequest {
    pub name: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
}

/// Body for `POST /api/assign-subject` — assign a file to a subject.
#[derive(Deserialize)]
pub struct AssignSubjectRequest {
    /// Absolute file path.
    pub path: String,
    /// Subject name to assign.
    pub subject: String,
    /// Conflict handling for an existing bare tag with the subject name.
    /// Supported values: `"add"` (default) or `"reassign"`.
    pub mode: Option<String>,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
}

/// Body for `POST /api/clone-subject`.
#[derive(Deserialize)]
pub struct CloneSubjectRequest {
    pub name: String,
    pub new_name: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
}

/// Query parameters for `GET /api/subject/props`.
#[derive(Deserialize)]
pub struct SubjectPropsParams {
    /// Subject name to query properties for.
    pub name: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
}

/// Body for `POST /api/subject/set-prop` and `POST /api/subject/remove-prop`.
#[derive(Deserialize)]
pub struct SubjectPropRequest {
    pub subject: String,
    pub tag: String,
    #[serde(default)]
    pub value: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
}

/// Minimal request body carrying only a directory.
#[derive(Deserialize, Default)]
pub struct DirBody {
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

/// Body for `POST /api/cache/clear-subdir`.
#[derive(Deserialize)]
pub struct CacheClearSubdirBody {
    /// The cache subdirectory to remove (e.g. "vthumbs", "ai_sprites").
    pub subdir: String,
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
    /// Enable ffmpeg video features.
    pub feature_video: Option<bool>,
    /// Enable ImageMagick / sips for exotic image formats.
    pub feature_imagemagick: Option<bool>,
    /// Enable PDF thumbnail generation.
    pub feature_pdf: Option<bool>,
    /// Directory collage layout style: `"crop"` (default) or `"fit"`.
    pub dir_preview_style: Option<String>,
}

// ---------------------------------------------------------------------------
// Synonym API
// ---------------------------------------------------------------------------

/// Body for `POST /api/synonym/add`.
#[derive(Deserialize)]
pub struct AddSynonymRequest {
    /// The alias to register.
    pub alias: String,
    /// The canonical tag name the alias maps to.
    pub canonical: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
}

/// Body for `POST /api/synonym/remove`.
#[derive(Deserialize)]
pub struct RemoveSynonymRequest {
    /// The alias to remove.
    pub alias: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
}

// ---------------------------------------------------------------------------
// Face detection API
// ---------------------------------------------------------------------------

/// A single face detection as returned by the API.
#[derive(Serialize, Clone)]
pub struct ApiFaceDetection {
    /// Detection primary key.
    pub id: i64,
    /// Bounding box in pixels (origin = top-left of image).
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// Detector confidence (0.0–1.0).
    pub confidence: f32,
    /// Assigned subject name, or `null` if not yet identified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_name: Option<String>,
}

/// Response body for face-analysis and face-detections endpoints.
#[derive(Serialize)]
pub struct ApiFaceResult {
    /// Relative path of the analysed file (relative to database root).
    pub path: String,
    /// All detections found in this file.
    pub detections: Vec<ApiFaceDetection>,
}

/// Body for `POST /api/face/analyse`.
#[derive(Deserialize)]
pub struct FaceAnalyseRequest {
    /// Absolute filesystem path of the file to analyse.
    pub path: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
}

/// Body for `POST /api/face/analyse-batch`.
#[derive(Deserialize)]
pub struct FaceAnalyseBatchRequest {
    /// Absolute filesystem path of the directory to process.
    pub dir: String,
    /// When `true`, process files in subdirectories recursively.
    #[serde(default)]
    pub recursive: bool,
}

/// Body for `POST /api/face/assign`.
#[derive(Deserialize)]
pub struct FaceAssignRequest {
    /// Detection ID to assign.
    pub detection_id: i64,
    /// Subject name to assign (e.g. `"person/alice"`).  Pass `null` to clear.
    pub subject_name: Option<String>,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
}

/// Body for `POST /api/face/cluster`.
#[derive(Deserialize)]
pub struct FaceClusterRequest {
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
}

/// Body for `POST /api/face/delete`.
#[derive(Deserialize)]
pub struct FaceDeleteRequest {
    /// One or more detection IDs to permanently remove.
    pub detection_ids: Vec<i64>,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
}

/// Query params for `GET /api/face/suggest`.
#[derive(Debug, Deserialize, Default)]
pub struct FaceSuggestParams {
    pub detection_id: i64,
    pub dir: Option<String>,
}

/// Current face-analysis batch progress, returned by `GET /api/face/status`.
#[derive(Default, Clone, Serialize)]
pub struct FaceProgressResponse {
    /// `true` while a batch is actively running.
    pub running: bool,
    /// Number of files processed so far.
    pub done: usize,
    /// Total number of files in the batch.
    pub total: usize,
    /// Relative path of the file currently being processed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
}

/// Face analysis configuration returned by `GET /api/face/config`.
#[derive(Serialize)]
pub struct FaceConfigResponse {
    pub enabled: bool,
    pub confidence: f32,
    pub cluster_distance: f32,
    pub min_face_px: u32,
    pub tag_prefix: String,
    pub auto_match_threshold: f32,
    /// `true` when both ONNX model files are present on disk.
    pub models_ready: bool,
}

/// Body for `POST /api/face/config`.
#[derive(Deserialize)]
pub struct FaceConfigRequest {
    pub enabled: Option<bool>,
    pub confidence: Option<f32>,
    pub cluster_distance: Option<f32>,
    pub min_face_px: Option<u32>,
    pub tag_prefix: Option<String>,
    pub auto_match_threshold: Option<f32>,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
}
