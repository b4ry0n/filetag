//! Core CRUD API handlers and embedded static-file serving for `filetag-web`.
//!
//! All JSON-returning handlers follow the pattern:
//! - Resolve the active root via [`root_from_dir`] (from the `dir` query / body field).
//! - For file operations, open the correct child database via [`open_for_file_op`].
//! - Return `Result<Json<…>, AppError>` so errors become HTTP 500 responses.

use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use axum::{
    extract::{Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Json, Response},
};
use filetag_lib::{db, query};

use crate::archive::ensure_zip_entry_record;
use crate::state::{
    AppError, AppState, file_is_covered, load_features_for, open_conn, open_for_file_op, parse_tag,
    resolve_preview, root_for_dir, safe_path,
};
use crate::types::*;
use crate::video::video_info;
use filetag_lib::db::TagRoot;

// ---------------------------------------------------------------------------
// Root resolution from `dir` parameter
// ---------------------------------------------------------------------------

/// Resolve the active database root from an absolute filesystem path.
///
/// Returns the deepest `TagRoot` whose root directory contains `dir`. This is
/// the one canonical root-resolution function used by all API handlers.
///
/// Returns `AppError` (HTTP 400) when `dir` is absent or not within any loaded root.
fn root_from_dir<'a>(state: &'a AppState, dir: Option<&str>) -> Result<&'a TagRoot, AppError> {
    let d = dir.ok_or_else(|| {
        AppError(anyhow::anyhow!(
            "dir parameter is required — navigate into a database first"
        ))
    })?;
    root_for_dir(state, Path::new(d)).ok_or_else(|| {
        AppError(anyhow::anyhow!(
            "path '{}' is not within any loaded database root",
            d
        ))
    })
}

// ---------------------------------------------------------------------------
// Static file handlers (embedded)
// ---------------------------------------------------------------------------

/// Serve the single-page app entry point (embedded `index.html`).
pub async fn index_html() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        include_str!("../static/index.html"),
    )
}

macro_rules! css_handler {
    ($name:ident, $path:literal) => {
        pub async fn $name() -> impl IntoResponse {
            (
                [
                    (header::CONTENT_TYPE, "text/css; charset=utf-8"),
                    (header::CACHE_CONTROL, "no-store"),
                ],
                include_str!($path),
            )
        }
    };
}

css_handler!(css_base, "../static/css/base.css");
css_handler!(css_layout, "../static/css/layout.css");
css_handler!(css_toolbar, "../static/css/toolbar.css");
css_handler!(css_cards, "../static/css/cards.css");
css_handler!(css_detail, "../static/css/detail.css");
css_handler!(css_viewer, "../static/css/viewer.css");
css_handler!(css_chat, "../static/css/chat.css");

macro_rules! js_handler {
    ($name:ident, $path:literal) => {
        pub async fn $name() -> impl IntoResponse {
            (
                [
                    (
                        header::CONTENT_TYPE,
                        "application/javascript; charset=utf-8",
                    ),
                    (header::CACHE_CONTROL, "no-store"),
                ],
                include_str!($path),
            )
        }
    };
}

js_handler!(js_i18n, "../static/js/i18n.js");
js_handler!(js_utils, "../static/js/utils.js");
js_handler!(js_state, "../static/js/state.js");
js_handler!(js_tags, "../static/js/tags.js");
js_handler!(js_render, "../static/js/render.js");
js_handler!(js_detail, "../static/js/detail.js");
js_handler!(js_actions, "../static/js/actions.js");
js_handler!(js_lightbox, "../static/js/lightbox.js");
js_handler!(js_viewer, "../static/js/viewer.js");
js_handler!(js_main, "../static/js/main.js");
js_handler!(js_chat, "../static/js/chat.js");

/// Serve the embedded `favicon.ico`.
pub async fn favicon() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "image/svg+xml")],
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><text y=".9em" font-size="90">🏷</text></svg>"#,
    )
}

// ---------------------------------------------------------------------------
// Roots
// ---------------------------------------------------------------------------

/// `GET /api/auth/status` — returns whether password authentication is enabled.
pub async fn api_auth_status(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "auth": state.sessions.is_enabled() }))
}

// ---------------------------------------------------------------------------

/// `GET /api/roots` — list all loaded database roots.
pub async fn api_roots(State(state): State<Arc<AppState>>) -> Json<Vec<ApiRoot>> {
    let mut entries: Vec<ApiRoot> = state
        .roots
        .iter()
        .enumerate()
        .map(|(id, r)| {
            let sort_order = open_conn(r)
                .ok()
                .and_then(|c| db::get_setting(&c, "sort_order").ok().flatten())
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(id as i64);
            ApiRoot {
                id,
                name: r.name.clone(),
                path: r.root.display().to_string(),
                sort_order,
                entry_point: r.entry_point,
            }
        })
        .collect();
    entries.sort_by_key(|r| r.sort_order);
    Json(entries)
}

/// `POST /api/reorder-roots` — persist a new sort order for the root tiles.
pub async fn api_reorder_roots(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ReorderRootsRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    for (position, root_path) in body.order.iter().enumerate() {
        let db_root = root_from_dir(&state, Some(root_path.as_str()))?;
        let conn = open_conn(db_root)?;
        db::set_setting(&conn, "sort_order", &position.to_string())?;
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// `POST /api/rename-db` — update the display name of a database root.
pub async fn api_rename_db(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RenameDbRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, Some(body.dir.as_str()))?;
    let conn = open_conn(db_root)?;
    db::set_setting(&conn, "name", &body.name)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Info
// ---------------------------------------------------------------------------

/// `GET /api/info` — database statistics (file count, tag count, total size).
pub async fn api_info(
    State(state): State<Arc<AppState>>,
    Query(rp): Query<DirParam>,
) -> Result<Json<ApiInfo>, AppError> {
    let db_root = root_from_dir(&state, rp.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    let files: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
    let tags: i64 = conn.query_row("SELECT COUNT(*) FROM tags", [], |r| r.get(0))?;
    let assignments: i64 = conn.query_row("SELECT COUNT(*) FROM file_tags", [], |r| r.get(0))?;
    let total_size: i64 =
        conn.query_row("SELECT COALESCE(SUM(size), 0) FROM files", [], |r| r.get(0))?;

    Ok(Json(ApiInfo {
        root: db_root.root.display().to_string(),
        files,
        tags,
        assignments,
        total_size,
    }))
}

// ---------------------------------------------------------------------------
// Cache clearing
// ---------------------------------------------------------------------------

/// Delete cache entries for a single file (all variants: thumb, raw preview, HEIC).
fn remove_cache_for_path(abs: &Path, root: &Path) -> u64 {
    let mut removed = 0u64;

    let meta = match std::fs::metadata(abs) {
        Ok(m) => m,
        Err(_) => return 0,
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let size = meta.len();
    let stem = abs
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let pfx = format!("{mtime}_{size}_{stem}");

    let cache_dir = root.join(".filetag").join("cache");

    // Walk every subdirectory of the cache dir and remove any file whose name
    // starts with the key prefix. This covers thumbs, raw, vthumbs, video, and
    // any future subdirectory without maintaining a hardcoded list.
    if let Ok(subdirs) = std::fs::read_dir(&cache_dir) {
        for sd in subdirs.flatten() {
            if sd.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let sd_name = sd.file_name();
                if sd_name == "hls2" {
                    // hls2 stores a subdirectory per file (named by prefix), not flat files.
                    let hls_dir = sd.path().join(&pfx);
                    if hls_dir.exists() {
                        if let Ok(rd) = std::fs::read_dir(&hls_dir) {
                            for entry in rd.flatten() {
                                if std::fs::remove_file(entry.path()).is_ok() {
                                    removed += 1;
                                }
                            }
                        }
                        let _ = std::fs::remove_dir(&hls_dir);
                    }
                } else if let Ok(rd) = std::fs::read_dir(sd.path()) {
                    for entry in rd.flatten() {
                        if entry
                            .file_name()
                            .to_string_lossy()
                            .starts_with(pfx.as_str())
                            && std::fs::remove_file(entry.path()).is_ok()
                        {
                            removed += 1;
                        }
                    }
                }
            }
        }
    }

    removed
}

/// `POST /api/cache/clear` — delete cached thumbnails and preview files.
///
/// The active root is always determined from the `dir` query parameter.
/// Three modes (controlled by the request body):
/// - `all: true` — wipe the entire `.filetag/cache/` directory of the active root.
/// - `paths: [...]` — clear cache for exactly those file paths.
/// - no body (or empty body) — enumerate `dir` and clear the entries on the page.
pub async fn api_cache_clear(
    State(state): State<Arc<AppState>>,
    Query(rp): Query<DirParam>,
    body: Option<axum::extract::Json<CacheClearBody>>,
) -> Response {
    let db_root = match root_from_dir(&state, rp.dir.as_deref()) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, e.0.to_string()).into_response(),
    };

    let body = body.map(|b| b.0).unwrap_or_default();

    let removed = if body.all.unwrap_or(false) {
        // Wipe the entire cache of the active root. root_from_dir already
        // returned the deepest root containing the current dir, so this always
        // clears exactly the right cache directory.
        let cache_dir = db_root.root.join(".filetag").join("cache");
        if cache_dir.exists()
            && let Err(e) = tokio::fs::remove_dir_all(&cache_dir).await
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to clear cache: {e}"),
            )
                .into_response();
        }
        1u64
    } else {
        // Determine the list of file paths to clear.
        let anchor = &db_root.root;
        let rel_paths: Vec<String> = if let Some(paths) = body.paths {
            // Caller supplied an explicit list (search-mode page clear).
            paths
        } else {
            // Browse-mode page clear: enumerate the current directory on disk.
            let abs_dir = Path::new(rp.dir.as_deref().unwrap_or(""));
            match std::fs::read_dir(abs_dir) {
                Ok(rd) => rd
                    .flatten()
                    .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                    .filter_map(|e| {
                        e.path()
                            .strip_prefix(anchor)
                            .ok()
                            .map(|p| p.to_string_lossy().into_owned())
                    })
                    .collect(),
                Err(_) => vec![],
            }
        };
        let mut n = 0u64;
        for rel in rel_paths {
            if let Some((abs, cr)) = resolve_preview(&state, anchor, &rel) {
                n += remove_cache_for_path(&abs, &cr);
            }
        }
        n
    };

    (
        StatusCode::OK,
        Json(serde_json::json!({ "removed": removed })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Cache management helpers
// ---------------------------------------------------------------------------

/// Recursively compute the total size and file count of a directory.
fn dir_size_and_count(dir: &Path) -> (u64, u64) {
    let mut size = 0u64;
    let mut count = 0u64;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let (s, c) = dir_size_and_count(&entry.path());
                size += s;
                count += c;
            } else if let Ok(meta) = entry.metadata() {
                size += meta.len();
                count += 1;
            }
        }
    }
    (size, count)
}

/// `GET /api/cache/info` — return size breakdown of the active root's cache.
pub async fn api_cache_info(
    State(state): State<Arc<AppState>>,
    Query(rp): Query<DirParam>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, rp.dir.as_deref())?;
    let cache_dir = db_root.root.join(".filetag").join("cache");

    let mut subdirs = vec![];
    let mut total_size = 0u64;

    if let Ok(rd) = std::fs::read_dir(&cache_dir) {
        let mut entries: Vec<_> = rd
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .collect();
        entries.sort_by_key(|e| e.file_name());

        for sd in entries {
            let name = sd.file_name().to_string_lossy().into_owned();
            let (size, count) = dir_size_and_count(&sd.path());
            total_size += size;
            subdirs.push(serde_json::json!({ "name": name, "size": size, "count": count }));
        }
    }

    Ok(Json(
        serde_json::json!({ "subdirs": subdirs, "total": total_size }),
    ))
}

/// `POST /api/cache/prune` — remove cache files whose source no longer exists on disk.
///
/// Enumerates all indexed file paths, computes the expected cache key prefix for
/// each file that still exists, then deletes any cache entry whose name does not
/// start with a live prefix.
pub async fn api_cache_prune(
    State(state): State<Arc<AppState>>,
    Query(rp): Query<DirParam>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, rp.dir.as_deref())?;
    let root = &db_root.root;
    let cache_dir = root.join(".filetag").join("cache");

    // Build the set of live cache key prefixes from every indexed file that still
    // exists on disk.
    let conn = open_conn(db_root).map_err(AppError)?;
    let live_prefixes: std::collections::HashSet<String> = {
        let mut stmt = conn
            .prepare("SELECT path FROM files")
            .map_err(|e| AppError(e.into()))?;
        let rel_paths: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .map_err(|e| AppError(e.into()))?
            .flatten()
            .collect();

        let mut prefixes = std::collections::HashSet::new();
        for rel in rel_paths {
            let abs = root.join(&rel);
            if let Ok(meta) = std::fs::metadata(&abs) {
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let size = meta.len();
                let stem = abs
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                prefixes.insert(format!("{mtime}_{size}_{stem}"));
            }
        }
        prefixes
    };

    let mut removed = 0u64;
    let mut freed = 0u64;

    if let Ok(subdirs) = std::fs::read_dir(&cache_dir) {
        for sd in subdirs.flatten() {
            if !sd.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            if sd.file_name() == "hls2" {
                // hls2 uses one subdirectory per source file named by its key prefix.
                if let Ok(rd) = std::fs::read_dir(sd.path()) {
                    for entry in rd.flatten() {
                        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                            continue;
                        }
                        let prefix = entry.file_name().to_string_lossy().into_owned();
                        if !live_prefixes.contains(&prefix) {
                            if let Ok(rd2) = std::fs::read_dir(entry.path()) {
                                for f in rd2.flatten() {
                                    if let Ok(m) = f.metadata() {
                                        freed += m.len();
                                    }
                                    if std::fs::remove_file(f.path()).is_ok() {
                                        removed += 1;
                                    }
                                }
                            }
                            let _ = std::fs::remove_dir(entry.path());
                        }
                    }
                }
            } else if let Ok(rd) = std::fs::read_dir(sd.path()) {
                for entry in rd.flatten() {
                    let fname = entry.file_name().to_string_lossy().into_owned();
                    let is_live = live_prefixes.iter().any(|p| fname.starts_with(p.as_str()));
                    if !is_live {
                        if let Ok(m) = entry.metadata() {
                            freed += m.len();
                        }
                        if std::fs::remove_file(entry.path()).is_ok() {
                            removed += 1;
                        }
                    }
                }
            }
        }
    }

    Ok(Json(
        serde_json::json!({ "removed": removed, "freed": freed }),
    ))
}

/// `POST /api/cache/clear-subdir` — wipe a single named cache subdirectory.
///
/// Only the subdirectories listed in `ALLOWED_SUBDIRS` may be cleared.
pub async fn api_cache_clear_subdir(
    State(state): State<Arc<AppState>>,
    Query(rp): Query<DirParam>,
    Json(body): Json<CacheClearSubdirBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    const ALLOWED_SUBDIRS: &[&str] = &["thumbs", "raw", "vthumbs", "ai_sprites", "hls2", "video"];
    if !ALLOWED_SUBDIRS.contains(&body.subdir.as_str()) {
        return Err(AppError(anyhow::anyhow!(
            "unknown cache subdirectory '{}'",
            body.subdir
        )));
    }
    let db_root = root_from_dir(&state, rp.dir.as_deref())?;
    let subdir = db_root
        .root
        .join(".filetag")
        .join("cache")
        .join(&body.subdir);
    if subdir.exists() {
        tokio::fs::remove_dir_all(&subdir)
            .await
            .map_err(|e| AppError(e.into()))?;
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Tags list
// ---------------------------------------------------------------------------

/// `GET /api/tags` — list all known tags with usage counts, colours and synonyms.
pub async fn api_tags(
    State(state): State<Arc<AppState>>,
    Query(rp): Query<DirParam>,
) -> Result<Json<Vec<ApiTag>>, AppError> {
    let db_root = root_from_dir(&state, rp.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    let tags = db::all_tags(&conn).map_err(AppError)?;
    let result: Result<Vec<ApiTag>, AppError> = tags
        .into_iter()
        .map(|(name, count, color, has_values)| {
            let synonyms = db::synonyms_for_tag(&conn, &name).map_err(AppError)?;
            Ok(ApiTag {
                name,
                count,
                color,
                synonyms,
                has_values,
            })
        })
        .collect();
    Ok(Json(result?))
}

/// `GET /api/tag-values` — list all distinct values for a given k/v tag.
pub async fn api_tag_values(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TagValuesParams>,
) -> Result<Json<Vec<ApiTagValue>>, AppError> {
    let db_root = root_from_dir(&state, params.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    let values = db::tag_values(&conn, &params.name).map_err(AppError)?;
    Ok(Json(
        values
            .into_iter()
            .map(|(value, count)| ApiTagValue { value, count })
            .collect(),
    ))
}

/// `GET /api/subjects` — list all distinct subjects with file counts.
pub async fn api_subjects(
    State(state): State<Arc<AppState>>,
    Query(rp): Query<DirParam>,
) -> Result<Json<Vec<ApiSubject>>, AppError> {
    let db_root = root_from_dir(&state, rp.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    let rows = db::all_subjects(&conn).map_err(AppError)?;
    Ok(Json(
        rows.into_iter()
            .map(|(name, count)| ApiSubject { name, count })
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// Synonym management
// ---------------------------------------------------------------------------

/// `POST /api/synonym/add` — register an alias as a synonym for a tag.
pub async fn api_add_synonym(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AddSynonymRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, body.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    db::add_synonym(&conn, &body.alias, &body.canonical).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// `POST /api/synonym/remove` — remove a registered synonym alias.
pub async fn api_remove_synonym(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RemoveSynonymRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, body.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    let removed = db::remove_synonym(&conn, &body.alias).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "ok": removed })))
}

// ---------------------------------------------------------------------------
// File listing
// ---------------------------------------------------------------------------

/// `GET /api/files` — list directory contents with per-entry metadata.
pub async fn api_files(
    State(state): State<Arc<AppState>>,
    Query(params): Query<FileListParams>,
) -> Result<Json<ApiDirListing>, AppError> {
    // Virtual root: only when there are multiple entry-point roots and no dir
    // parameter has been provided yet.
    let entry_point_roots: Vec<(usize, &TagRoot)> = state
        .roots
        .iter()
        .enumerate()
        .filter(|(_, r)| r.entry_point)
        .collect();
    if params.dir.is_none() {
        let mut ordered: Vec<(usize, &TagRoot, i64)> = entry_point_roots
            .iter()
            .map(|&(id, r)| {
                let order = open_conn(r)
                    .ok()
                    .and_then(|c| db::get_setting(&c, "sort_order").ok().flatten())
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(id as i64);
                (id, r, order)
            })
            .collect();
        ordered.sort_by_key(|&(_, _, o)| o);
        let entries = ordered
            .iter()
            .map(|&(_id, r, _)| ApiDirEntry {
                name: r.name.clone(),
                is_dir: true,
                size: None,
                mtime: None,
                file_count: None,
                tag_count: None,
                root_path: Some(r.root.display().to_string()),
                covered: None,
            })
            .collect();
        return Ok(Json(ApiDirListing {
            path: String::new(),
            root_path: String::new(),
            entries,
        }));
    }

    let db_root = root_from_dir(&state, params.dir.as_deref())?;
    let abs_dir = std::path::Path::new(params.dir.as_deref().unwrap_or(""));

    // Path relative to the deepest root — used for breadcrumb in JS and for
    // tag-count queries; this matches how paths are stored in the DB.
    let db_rel: String = abs_dir
        .strip_prefix(&db_root.root)
        .unwrap_or(std::path::Path::new(""))
        .to_string_lossy()
        .into_owned();

    let prefix = if db_rel.is_empty() {
        String::new()
    } else {
        format!("{}/", db_rel.trim_end_matches('/'))
    };

    let conn = open_conn(db_root)?;
    let mut tag_stmt = conn.prepare_cached(
        "SELECT COUNT(*) FROM file_tags ft \
         JOIN files f ON f.id = ft.file_id WHERE f.path = ?1",
    )?;

    let mut dirs = Vec::new();
    let mut files = Vec::new();

    let rd = std::fs::read_dir(abs_dir)
        .with_context(|| format!("reading directory {}", abs_dir.display()))?;

    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".filetag" {
            continue;
        }
        if !params.show_hidden && name.starts_with('.') {
            continue;
        }

        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };

        if meta.is_dir() {
            let child_count = std::fs::read_dir(entry.path())
                .map(|rd| rd.flatten().count() as i64)
                .unwrap_or(0);
            let dir_rel_path = format!("{}{}", prefix, name);
            let dir_tag_count: i64 = tag_stmt
                .query_row(rusqlite::params![&dir_rel_path], |r| r.get(0))
                .unwrap_or(0);
            dirs.push(ApiDirEntry {
                name,
                is_dir: true,
                size: None,
                mtime: None,
                file_count: Some(child_count),
                tag_count: if dir_tag_count > 0 {
                    Some(dir_tag_count)
                } else {
                    None
                },
                root_path: None,
                covered: None,
            });
        } else if meta.is_file() {
            let rel_path = format!("{}{}", prefix, name);
            let size = meta.len() as i64;
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0);

            let tag_count: i64 = tag_stmt
                .query_row(rusqlite::params![&rel_path], |r| r.get(0))
                .unwrap_or(0);

            files.push(ApiDirEntry {
                name,
                is_dir: false,
                size: Some(size),
                mtime: Some(mtime),
                file_count: None,
                tag_count: Some(tag_count),
                root_path: None,
                covered: Some(file_is_covered(&state, &entry.path())),
            });
        }
    }

    dirs.sort_by_key(|a| a.name.to_lowercase());
    files.sort_by_key(|a| a.name.to_lowercase());
    dirs.extend(files);

    Ok(Json(ApiDirListing {
        path: db_rel,
        root_path: db_root.root.display().to_string(),
        entries: dirs,
    }))
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

/// `GET /api/search` — execute a tag query and return matching file paths.
pub async fn api_search(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchParams>,
) -> Result<Json<ApiSearchResult>, AppError> {
    let db_root = root_from_dir(&state, params.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    let expr = query::parse(&params.q).map_err(AppError)?;
    let results = query::execute_with_tags(&conn, &expr).map_err(AppError)?;

    Ok(Json(ApiSearchResult {
        query: params.q,
        results: results
            .into_iter()
            .map(|(path, tags)| ApiSearchEntry {
                path,
                tags: tags
                    .into_iter()
                    .map(|(name, value)| ApiFileTag {
                        name,
                        value,
                        subject: None,
                        implicit: false,
                    })
                    .collect(),
            })
            .collect(),
    }))
}

// ---------------------------------------------------------------------------
// File detail
// ---------------------------------------------------------------------------

/// `GET /api/file` — return full metadata and tags for a single file.
pub async fn api_file_detail(
    State(state): State<Arc<AppState>>,
    Query(params): Query<FileDetailParams>,
) -> Result<Json<ApiFileDetail>, AppError> {
    let db_root = root_from_dir(&state, params.dir.as_deref())?;

    let is_zip = params.path.contains("::");
    let fs_path = if is_zip {
        let zip_part = params.path.split_once("::").unwrap().0;
        db_root.root.join(zip_part)
    } else {
        safe_path(&db_root.root, &params.path)?;
        db_root.root.join(&params.path)
    };

    // Probe video duration in the background while we do the DB lookup.
    let is_video = !is_zip
        && fs_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| {
                matches!(
                    e.to_ascii_lowercase().as_str(),
                    "mp4"
                        | "webm"
                        | "mkv"
                        | "avi"
                        | "mov"
                        | "wmv"
                        | "flv"
                        | "m4v"
                        | "3gp"
                        | "f4v"
                        | "mpg"
                        | "mpeg"
                        | "m2v"
                        | "m2ts"
                        | "mts"
                        | "ogv"
                        | "vob"
                        | "mxf"
                        | "divx"
                        | "qt"
                )
            })
            .unwrap_or(false);
    let duration = if is_video {
        let features = load_features_for(&state, &db_root.root);
        if features.video {
            video_info(&fs_path).await.map(|i| i.duration)
        } else {
            None
        }
    } else {
        None
    };

    let start = fs_path.parent().unwrap_or(&fs_path);

    let db_lookup = db::find_and_open(start).ok().and_then(|(conn, eff_root)| {
        let eff_rel = if is_zip {
            let zip_rel = db::relative_to_root(&fs_path, &eff_root).ok()?;
            let entry = params.path.split_once("::").unwrap().1;
            Some(format!("{}::{}", zip_rel, entry))
        } else {
            db::relative_to_root(&fs_path, &eff_root).ok()
        };
        eff_rel.map(|r| (conn, r))
    });

    if let Some((conn, effective_rel)) = db_lookup
        && let Some(record) = db::file_by_path(&conn, &effective_rel).map_err(AppError)?
    {
        let tags = db::tags_for_file_with_subject(&conn, record.id).map_err(AppError)?;
        let implicit_tags = db::subject_props_for_file(&conn, record.id).map_err(AppError)?;
        let indexed_at: String = conn.query_row(
            "SELECT indexed_at FROM files WHERE id = ?1",
            rusqlite::params![record.id],
            |r| r.get(0),
        )?;

        let mut all_tags: Vec<ApiFileTag> = tags
            .into_iter()
            .map(|(name, value, subject)| ApiFileTag {
                name,
                value,
                subject: if subject.is_empty() {
                    None
                } else {
                    Some(subject)
                },
                implicit: false,
            })
            .collect();
        for (subject, name, value) in implicit_tags {
            all_tags.push(ApiFileTag {
                name,
                value: if value.is_empty() { None } else { Some(value) },
                subject: Some(subject),
                implicit: true,
            });
        }

        return Ok(Json(ApiFileDetail {
            path: params.path,
            size: record.size,
            file_id: record.file_id,
            mtime: record.mtime_ns,
            indexed_at,
            covered: true,
            tags: all_tags,
            duration,
        }));
    }

    if is_zip {
        return Ok(Json(ApiFileDetail {
            path: params.path,
            size: 0,
            file_id: None,
            mtime: 0,
            indexed_at: String::new(),
            covered: true,
            tags: vec![],
            duration: None,
        }));
    }

    let meta =
        std::fs::metadata(&fs_path).with_context(|| format!("reading {}", fs_path.display()))?;
    let size = meta.len() as i64;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);

    Ok(Json(ApiFileDetail {
        path: params.path,
        size,
        file_id: None,
        mtime,
        indexed_at: String::new(),
        covered: file_is_covered(&state, &fs_path),
        tags: vec![],
        duration,
    }))
}

// ---------------------------------------------------------------------------
// Tag / Untag (now using open_for_file_op)
// ---------------------------------------------------------------------------

/// `POST /api/tag` — apply one or more tags to a file.
///
/// Routes through [`open_for_file_op`] so the tag is written to the correct
/// child database when nested databases are loaded.
pub async fn api_tag(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TagRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, body.dir.as_deref())?;
    let (conn, effective_root, effective_rel) =
        open_for_file_op(db_root, &body.path).map_err(AppError)?;

    let file_id = if body.path.contains("::") {
        ensure_zip_entry_record(&conn, &effective_rel).map_err(AppError)?
    } else {
        db::get_or_index_file(&conn, &effective_rel, &effective_root)
            .map_err(AppError)?
            .id
    };

    let mut added = 0i64;
    for tag_str in &body.tags {
        let (name, value) = parse_tag(tag_str);
        let tag_id = db::get_or_create_tag(&conn, &name).map_err(AppError)?;
        db::apply_tag(
            &conn,
            file_id,
            tag_id,
            value.as_deref(),
            body.subject.as_deref(),
        )
        .map_err(AppError)?;
        added += 1;
    }

    Ok(Json(serde_json::json!({ "added": added })))
}

/// `POST /api/untag` — remove one or more tags from a file.
pub async fn api_untag(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TagRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, body.dir.as_deref())?;
    let (conn, _effective_root, effective_rel) =
        open_for_file_op(db_root, &body.path).map_err(AppError)?;

    let record = db::file_by_path(&conn, &effective_rel)
        .map_err(AppError)?
        .ok_or_else(|| AppError(anyhow::anyhow!("file not found: {}", body.path)))?;

    let mut removed = 0i64;
    for tag_str in &body.tags {
        let (name, value) = parse_tag(tag_str);
        if let Ok(tag_id) = conn.query_row(
            "SELECT id FROM tags WHERE name = ?1",
            rusqlite::params![&name],
            |r| r.get::<_, i64>(0),
        ) && db::remove_tag(
            &conn,
            record.id,
            tag_id,
            value.as_deref(),
            body.subject.as_deref(),
        )
        .map_err(AppError)?
        {
            removed += 1;
        }
    }

    Ok(Json(serde_json::json!({ "removed": removed })))
}

// ---------------------------------------------------------------------------
// Tag color + delete
// ---------------------------------------------------------------------------

/// `POST /api/rename-tag` — rename a tag across all files in the database.
pub async fn api_rename_tag(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RenameTagRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, body.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    let outcome = db::rename_tag(&conn, &body.name, &body.new_name).map_err(AppError)?;
    let ok = !matches!(outcome, db::RenameOutcome::NotFound);
    let merged = matches!(outcome, db::RenameOutcome::Merged { .. });
    Ok(Json(serde_json::json!({ "ok": ok, "merged": merged })))
}

/// `POST /api/tag-color` — set or clear the display colour for a tag.
pub async fn api_tag_color(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TagColorRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, body.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    let ok = db::set_tag_color(&conn, &body.name, body.color.as_deref()).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "ok": ok })))
}

/// `POST /api/delete-tag` — permanently delete a tag and all its assignments.
pub async fn api_delete_tag(
    State(state): State<Arc<AppState>>,
    Json(body): Json<DeleteTagRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, body.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    let deleted = db::delete_tag(&conn, &body.name).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "deleted": deleted })))
}

pub async fn api_prune_tags(
    State(state): State<Arc<AppState>>,
    Json(body): Json<DirBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, body.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    let removed = db::prune_unused_tags(&conn).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "removed": removed })))
}

/// `POST /api/create-subject` — create an empty subject entity.
pub async fn api_create_subject(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateSubjectRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, body.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    let created = db::create_subject(&conn, &body.name).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "created": created })))
}

/// `POST /api/rename-subject` — rename a subject label across all file-tag assignments.
pub async fn api_rename_subject(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RenameSubjectRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, body.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    let updated = db::rename_subject(&conn, &body.name, &body.new_name).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "updated": updated })))
}

/// `POST /api/delete-subject` — remove a subject label by clearing it on all file-tag assignments.
pub async fn api_delete_subject(
    State(state): State<Arc<AppState>>,
    Json(body): Json<DeleteSubjectRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, body.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    let updated = db::delete_subject(&conn, &body.name).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "updated": updated })))
}

/// `POST /api/assign-subject` — assign a file to a subject by moving its
/// unassigned file-tag rows (subject = '') to the given subject name.
pub async fn api_assign_subject(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AssignSubjectRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, body.dir.as_deref())?;
    let (conn, effective_root, effective_rel) =
        open_for_file_op(db_root, &body.path).map_err(AppError)?;
    let file_id = if body.path.contains("::") {
        ensure_zip_entry_record(&conn, &effective_rel).map_err(AppError)?
    } else {
        db::get_or_index_file(&conn, &effective_rel, &effective_root)
            .map_err(AppError)?
            .id
    };
    let updated = db::assign_file_to_subject(&conn, file_id, &body.subject).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "updated": updated })))
}

/// `POST /api/clone-subject` — copy all file-tag assignments from one subject to another.
pub async fn api_clone_subject(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CloneSubjectRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, body.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    let inserted = db::clone_subject(&conn, &body.name, &body.new_name).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "inserted": inserted })))
}

/// `GET /api/subject/props` — list entity properties of a subject.
pub async fn api_subject_props(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubjectPropsParams>,
) -> Result<Json<Vec<serde_json::Value>>, AppError> {
    let db_root = root_from_dir(&state, params.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    let rows = db::get_subject_props(&conn, &params.name).map_err(AppError)?;
    Ok(Json(
        rows.into_iter()
            .map(|(tag, value)| serde_json::json!({ "tag": tag, "value": value }))
            .collect(),
    ))
}

/// `POST /api/subject/set-prop` — add a property to a subject entity.
pub async fn api_subject_set_prop(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SubjectPropRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, body.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    let inserted =
        db::set_subject_prop(&conn, &body.subject, &body.tag, &body.value).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "inserted": inserted })))
}

/// `POST /api/subject/remove-prop` — remove a property from a subject entity.
pub async fn api_subject_remove_prop(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SubjectPropRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, body.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    let value_opt = if body.value.is_empty() {
        None
    } else {
        Some(body.value.as_str())
    };
    let removed =
        db::remove_subject_prop(&conn, &body.subject, &body.tag, value_opt).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "removed": removed })))
}

/// `GET /api/settings` — read per-root settings (trickplay counts + feature flags).
pub async fn api_settings_get(
    Query(params): Query<SettingsParams>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, params.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    let sprite_min: u32 = db::get_setting(&conn, "sprite_min")
        .map_err(AppError)?
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    let sprite_max: u32 = db::get_setting(&conn, "sprite_max")
        .map_err(AppError)?
        .and_then(|v| v.parse().ok())
        .unwrap_or(16);
    let bool_setting = |key: &str| -> bool {
        db::get_setting(&conn, key)
            .ok()
            .flatten()
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    };
    Ok(Json(serde_json::json!({
        "sprite_min": sprite_min,
        "sprite_max": sprite_max,
        "feature_video": bool_setting("feature.video"),
        "feature_imagemagick": bool_setting("feature.imagemagick"),
        "feature_pdf": bool_setting("feature.pdf"),
    })))
}

/// `POST /api/settings` — persist per-root settings.
pub async fn api_settings_set(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SettingsBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, body.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    if let Some(v) = body.sprite_min {
        db::set_setting(&conn, "sprite_min", &v.to_string()).map_err(AppError)?;
    }
    if let Some(v) = body.sprite_max {
        db::set_setting(&conn, "sprite_max", &v.to_string()).map_err(AppError)?;
    }
    let bool_to_str = |b: bool| if b { "1" } else { "0" };
    if let Some(v) = body.feature_video {
        db::set_setting(&conn, "feature.video", bool_to_str(v)).map_err(AppError)?;
    }
    if let Some(v) = body.feature_imagemagick {
        db::set_setting(&conn, "feature.imagemagick", bool_to_str(v)).map_err(AppError)?;
    }
    if let Some(v) = body.feature_pdf {
        db::set_setting(&conn, "feature.pdf", bool_to_str(v)).map_err(AppError)?;
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}
