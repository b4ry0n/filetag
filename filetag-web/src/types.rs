//! Request and response types for the `filetag-web` HTTP API.
//!
//! All response types derive [`serde::Serialize`]; all request types derive
//! [`serde::Deserialize`].  Requests may carry either `dir: Option<String>`
//! (absolute filesystem path, used by the web frontend) or `root_id: Option<usize>`
//! (index returned by `GET /api/roots`, preferred by native clients).  The
//! backend resolves the root via `root_from_dir_or_id`, which checks `root_id`
//! first and falls back to `dir`.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// API response types
// ---------------------------------------------------------------------------

/// Database statistics returned by `GET /api/info`.
#[derive(Serialize)]
pub struct ApiInfo {
    /// Numeric root ID (index from `GET /api/roots`).
    pub root_id: usize,
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
    /// Numeric root ID (index from `GET /api/roots`).
    pub root_id: Option<usize>,
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
    /// Numeric root ID for virtual-root tile entries (index from `GET /api/roots`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_id: Option<usize>,
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
    /// Numeric root ID (index from `GET /api/roots`).
    pub root_id: usize,
    /// Absolute path of the database root that owns this file.
    pub root_path: String,
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
    /// Numeric root ID (index from `GET /api/roots`).
    pub root_id: usize,
    /// Absolute path of the database root that owns this file.
    pub root_path: String,
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
    /// Absolute filesystem path of the directory to list (legacy; prefer `root_id` + `path`).
    pub dir: Option<String>,
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
    /// Root-relative subdirectory path, used with `root_id` (no leading slash).
    pub path: Option<String>,
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
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
}

/// Query params for `GET /api/file`.
#[derive(Deserialize)]
pub struct FileDetailParams {
    pub path: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
}

/// Body for `POST /api/comic/import-metadata`.
#[derive(Deserialize)]
pub struct ComicImportRequest {
    /// Relative path of the comic archive within the database root.
    pub path: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
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
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
}

/// Body for `POST /api/tag-bulk` and `POST /api/untag-bulk`.
///
/// Applies or removes tags across multiple files in a single SQLite transaction
/// per database root.  This is dramatically faster than issuing one request per
/// file because it reduces the number of disk fsyncs from O(n) to O(k), where
/// k is the number of distinct database roots in the selection.
#[derive(Deserialize)]
pub struct BulkTagRequest {
    pub paths: Vec<String>,
    pub tags: Vec<String>,
    /// Optional subject group for the operation.
    pub subject: Option<String>,
    /// Absolute filesystem path of the currently browsed directory.
    /// Used to resolve the entry-point database root.
    pub dir: Option<String>,
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
}

/// Generic query param carrying the current browsing directory.
/// The backend resolves the active (deepest) root from this path.
#[derive(Deserialize, Default)]
pub struct DirParam {
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
}

/// Body for `POST /api/files-tags` — fetch tags for multiple paths in one
/// request.
#[derive(Deserialize)]
pub struct FilesTagsRequest {
    pub paths: Vec<String>,
    /// Absolute filesystem path of the currently browsed directory.
    /// Used to resolve the entry-point database root.
    pub dir: Option<String>,
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
}

/// Body for `POST /api/rename-db`.
#[derive(Deserialize)]
pub struct RenameDbRequest {
    /// Absolute filesystem path of the database root directory to rename.
    pub dir: String,
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
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
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
}

/// Body for `POST /api/rename-tag`.
#[derive(Deserialize)]
pub struct RenameTagRequest {
    pub name: String,
    pub new_name: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
}

/// Body for `POST /api/delete-tag`.
#[derive(Deserialize)]
pub struct DeleteTagRequest {
    pub name: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
}

/// Body for `POST /api/rename-subject`.
#[derive(Deserialize)]
pub struct RenameSubjectRequest {
    pub name: String,
    pub new_name: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
}

/// Body for `POST /api/create-subject`.
#[derive(Deserialize)]
pub struct CreateSubjectRequest {
    pub name: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
}

/// Body for `POST /api/delete-subject`.
#[derive(Deserialize)]
pub struct DeleteSubjectRequest {
    pub name: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
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
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
}

/// Body for `POST /api/clone-subject`.
#[derive(Deserialize)]
pub struct CloneSubjectRequest {
    pub name: String,
    pub new_name: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
}

/// Query parameters for `GET /api/subject/props`.
#[derive(Deserialize)]
pub struct SubjectPropsParams {
    /// Subject name to query properties for.
    pub name: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
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
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
}

/// Minimal request body carrying only a directory.
#[derive(Deserialize, Default)]
pub struct DirBody {
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
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
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
}

/// Body for `POST /api/settings`.
#[derive(Deserialize)]
pub struct SettingsBody {
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
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
    /// Enable YOLOv8n-pose salient-point detection for grid thumbnails.
    pub feature_saliency_pose: Option<bool>,
    /// Enable YOLOv8n object-detection fallback for non-person images.
    pub feature_saliency_object: Option<bool>,
    /// Directory collage layout style: `"crop"` (default) or `"fit"`.
    pub dir_preview_style: Option<String>,
    /// Tile hover preview mode: `"sprite"` (default) or `"webm"`.
    pub tile_preview_mode: Option<String>,
    /// WebM tile preview clip length in seconds (default 8, range 2–120).
    pub vtile_duration: Option<u32>,
    /// When true, serve the longest already-cached clip (>= vtile_duration) instead
    /// of transcoding a new clip at the exact configured duration.
    pub vtile_use_longest: Option<bool>,
}

// ---------------------------------------------------------------------------
// Synonym API
// ---------------------------------------------------------------------------

/// Body for `POST /api/synonym/add` — link two tag names as synonyms
/// (symmetric; no canonical direction).
#[derive(Deserialize)]
pub struct AddSynonymRequest {
    /// First tag name.
    pub name: String,
    /// Second tag name to link with `name`.
    pub other: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
}

/// Body for `POST /api/synonym/remove` — remove a tag from its synonym group.
#[derive(Deserialize)]
pub struct RemoveSynonymRequest {
    /// Tag name to remove from its group.
    pub name: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
}

/// Body for `POST /api/synonym/attr` — set an attribute on a tag name.
#[derive(Deserialize)]
pub struct SetTagAttrRequest {
    /// Tag name to set the attribute on.
    pub name: String,
    /// Attribute key (e.g. `"lang"`).
    pub key: String,
    /// Attribute value (e.g. `"nl"`).
    pub value: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
}

/// Body for `POST /api/synonym/attr-remove` — remove an attribute from a tag name.
#[derive(Deserialize)]
pub struct RemoveTagAttrRequest {
    /// Tag name to remove the attribute from.
    pub name: String,
    /// Attribute key to remove.
    pub key: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
}

/// Body for `POST /api/display-context` — set the global display context.
#[derive(Deserialize)]
pub struct SetDisplayContextRequest {
    /// Key→value map used to select preferred display names from synonym groups.
    pub context: std::collections::HashMap<String, String>,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
    /// Root ID (from `GET /api/roots`) as alternative to `dir`.
    pub root_id: Option<usize>,
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
    /// Original image width in pixels (full-resolution, before any downscaling).
    /// The frontend uses this to map stored pixel coordinates onto the displayed image.
    pub image_width: u32,
    /// Original image height in pixels (full-resolution, before any downscaling).
    pub image_height: u32,
}

/// Body for `POST /api/face/analyse`.
#[derive(Deserialize)]
pub struct FaceAnalyseRequest {
    /// Absolute filesystem path of the file to analyse.
    pub path: String,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
    /// Root ID from `GET /api/roots` (native client alternative to `dir`).
    pub root_id: Option<usize>,
}

/// Body for `POST /api/face/analyse-batch`.
#[derive(Deserialize)]
pub struct FaceAnalyseBatchRequest {
    /// Absolute filesystem path of the directory to process.
    pub dir: String,
    /// When `true`, process files in subdirectories recursively.
    #[serde(default)]
    pub recursive: bool,
    /// Optional explicit list of absolute file paths to process instead of
    /// scanning the directory.  When present, `recursive` is ignored.
    pub paths: Option<Vec<String>>,
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
    /// Root ID from `GET /api/roots` (native client alternative to `dir`).
    pub root_id: Option<usize>,
}

/// Body for `POST /api/face/cluster`.
#[derive(Deserialize)]
pub struct FaceClusterRequest {
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
    /// Root ID from `GET /api/roots` (native client alternative to `dir`).
    pub root_id: Option<usize>,
}

/// Body for `POST /api/face/delete`.
#[derive(Deserialize)]
pub struct FaceDeleteRequest {
    /// One or more detection IDs to permanently remove.
    pub detection_ids: Vec<i64>,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
    /// Root ID from `GET /api/roots` (native client alternative to `dir`).
    pub root_id: Option<usize>,
}

/// Query params for `GET /api/face/suggest`.
#[derive(Debug, Deserialize, Default)]
pub struct FaceSuggestParams {
    pub detection_id: i64,
    pub dir: Option<String>,
    pub root_id: Option<usize>,
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
    /// When true, large images are split into overlapping tiles during detection.
    /// Increases accuracy for small faces in crowd shots at the cost of much
    /// longer analysis time. Disabled by default.
    pub tiling_enabled: bool,
    /// `true` when both ONNX model files are present on disk.
    pub models_ready: bool,
}

/// Body for `POST /api/tag-dir-recursive` — tag all files in a directory tree.
#[derive(Debug, Clone, Deserialize)]
pub struct TagDirRecursiveRequest {
    /// Relative path of the directory to tag (from the database root).
    pub path: String,
    pub tags: Vec<String>,
    /// Optional subject group for the tags being added.
    pub subject: Option<String>,
    /// When `true`, entries inside archives (ZIP/RAR/7z and comic variants)
    /// found anywhere in the tree are also tagged.
    #[serde(default)]
    pub include_archives: bool,
    /// Absolute filesystem path of the currently browsed directory (root
    /// resolution — same semantics as `dir` in other tag requests).
    pub dir: Option<String>,
    /// Index into the roots list returned by `GET /api/roots`. Takes priority
    /// over `dir` when provided.
    pub root_id: Option<usize>,
}

/// Body for `POST /api/fs/rename`.
#[derive(Deserialize)]
pub struct FsRenameRequest {
    /// Absolute filesystem path of the file or directory to rename (legacy; prefer `root_id` + `rel_path`).
    pub path: Option<String>,
    /// Root ID (from `GET /api/roots`). Used together with `rel_path`.
    pub root_id: Option<usize>,
    /// Path of the item relative to its database root. Used with `root_id`.
    pub rel_path: Option<String>,
    /// New filename (basename only — must not contain path separators).
    pub new_name: String,
}

/// Body for `POST /api/fs/move`.
#[derive(Deserialize)]
pub struct FsMoveRequest {
    /// Absolute filesystem path of the file or directory to move (legacy; prefer `root_id` + `rel_path`).
    pub path: Option<String>,
    /// Absolute filesystem path of the destination directory (legacy; prefer `dest_root_id` + `dest_rel_dir`).
    pub dest_dir: Option<String>,
    /// Root ID of the source item.
    pub root_id: Option<usize>,
    /// Path of the source item relative to its database root.
    pub rel_path: Option<String>,
    /// Root ID of the destination directory (may differ from `root_id` for cross-root moves).
    pub dest_root_id: Option<usize>,
    /// Destination directory path relative to its database root.
    pub dest_rel_dir: Option<String>,
}

/// Body for `POST /api/fs/delete`.
#[derive(Deserialize)]
pub struct FsDeleteRequest {
    /// Absolute filesystem path of the file or directory to delete (legacy; prefer `root_id` + `rel_path`).
    pub path: Option<String>,
    /// Root ID (from `GET /api/roots`). Used together with `rel_path`.
    pub root_id: Option<usize>,
    /// Path of the item relative to its database root. Used with `root_id`.
    pub rel_path: Option<String>,
}

/// Body for `POST /api/fs/copy`.
#[derive(Deserialize)]
pub struct FsCopyRequest {
    /// Absolute filesystem path of the source file (legacy; prefer `root_id` + `rel_path`).
    pub path: Option<String>,
    /// Absolute filesystem path of the destination directory (legacy; prefer `dest_root_id` + `dest_rel_dir`).
    pub dest_dir: Option<String>,
    /// Root ID of the source file.
    pub root_id: Option<usize>,
    /// Path of the source file relative to its database root.
    pub rel_path: Option<String>,
    /// Root ID of the destination directory.
    pub dest_root_id: Option<usize>,
    /// Destination directory path relative to its database root.
    pub dest_rel_dir: Option<String>,
    /// New filename for the copy.  Defaults to `"Copy of <name>"` when absent.
    pub new_name: Option<String>,
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
    pub tiling_enabled: Option<bool>,
    /// Absolute filesystem path of the currently browsed directory.
    pub dir: Option<String>,
    /// Index into the roots list returned by `GET /api/roots`.
    pub root_id: Option<usize>,
}
