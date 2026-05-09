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
js_handler!(js_face, "../static/js/face.js");
js_handler!(js_prompt_wizard, "../static/js/prompt-wizard.js");
js_handler!(js_select, "../static/js/select.js");
css_handler!(css_face, "../static/css/face.css");
css_handler!(css_mobile, "../static/css/mobile.css");

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
    const ALLOWED_SUBDIRS: &[&str] = &[
        "thumbs",
        "raw",
        "vthumbs",
        "ai_sprites",
        "hls2",
        "video",
        "dir-thumbs",
    ];
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
// Database maintenance
// ---------------------------------------------------------------------------

/// `POST /api/db/purge-missing` — remove `files` records whose path no longer
/// exists on disk.
///
/// Scans every record in the `files` table of the active root, resolves the
/// absolute path, and deletes the row (cascading to `file_tags`) when the file
/// is absent from the filesystem.
///
/// Returns `{ removed: usize, vacuum: bool }`.  A SQLite `VACUUM` is run after
/// removal to reclaim space.
pub async fn api_db_purge_missing(
    State(state): State<Arc<AppState>>,
    Query(rp): Query<DirParam>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, rp.dir.as_deref())?;
    let root = db_root.root.clone();
    let db_path = db_root.db_path.clone();

    // Collect all file paths from DB (synchronous — not across await).
    let rel_paths: Vec<(i64, String)> = {
        let conn = open_conn(db_root).map_err(AppError)?;
        let mut stmt = conn
            .prepare("SELECT id, path FROM files")
            .map_err(|e| AppError(e.into()))?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| AppError(e.into()))?
            .flatten()
            .collect()
    };

    // Check existence and collect IDs to delete (pure I/O, no DB conn held).
    let missing_ids: Vec<i64> = rel_paths
        .into_iter()
        .filter(|(_, rel)| !root.join(rel).exists())
        .map(|(id, _)| id)
        .collect();

    let removed = missing_ids.len();

    if !missing_ids.is_empty() {
        let conn = rusqlite::Connection::open(&db_path).map_err(|e| AppError(e.into()))?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")
            .map_err(|e| AppError(e.into()))?;
        for id in missing_ids {
            conn.execute("DELETE FROM files WHERE id = ?1", [id])
                .map_err(|e| AppError(e.into()))?;
        }
        conn.execute_batch("VACUUM;")
            .map_err(|e| AppError(e.into()))?;
    }

    Ok(Json(
        serde_json::json!({ "removed": removed, "vacuumed": removed > 0 }),
    ))
}

/// `POST /api/db/purge-unused-tags` — delete tags that have no associated files.
///
/// A tag is considered unused when it has no rows in `file_tags`.  The tag
/// record itself is removed; `file_tags` has a foreign key back to `tags`, so
/// any stale rows there are also gone (via CASCADE).
///
/// Returns `{ removed: usize }`.
pub async fn api_db_purge_unused_tags(
    State(state): State<Arc<AppState>>,
    Query(rp): Query<DirParam>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, rp.dir.as_deref())?;
    let conn = open_conn(db_root).map_err(AppError)?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")
        .map_err(|e| AppError(e.into()))?;
    let removed = conn
        .execute(
            "DELETE FROM tags WHERE id NOT IN (SELECT DISTINCT tag_id FROM file_tags)",
            [],
        )
        .map_err(|e| AppError(e.into()))?;
    Ok(Json(serde_json::json!({ "removed": removed })))
}

/// `POST /api/db/purge-orphan-file-tags` — remove `file_tags` rows that have
/// no corresponding entry in `files` or `tags`.
///
/// Under normal operation ON DELETE CASCADE prevents this, but legacy imports
/// or manual edits can leave stale rows.
///
/// Returns `{ removed: usize }`.
pub async fn api_db_purge_orphan_file_tags(
    State(state): State<Arc<AppState>>,
    Query(rp): Query<DirParam>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, rp.dir.as_deref())?;
    let conn = open_conn(db_root).map_err(AppError)?;
    let removed = conn
        .execute(
            "DELETE FROM file_tags \
             WHERE file_id NOT IN (SELECT id FROM files) \
                OR tag_id  NOT IN (SELECT id FROM tags)",
            [],
        )
        .map_err(|e| AppError(e.into()))?;
    Ok(Json(serde_json::json!({ "removed": removed })))
}

/// `POST /api/db/vacuum` — run SQLite VACUUM to compact the database file.
///
/// Returns `{ ok: true }`.
pub async fn api_db_vacuum(
    State(state): State<Arc<AppState>>,
    Query(rp): Query<DirParam>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, rp.dir.as_deref())?;
    let conn = open_conn(db_root).map_err(AppError)?;
    conn.execute_batch("VACUUM;")
        .map_err(|e| AppError(e.into()))?;
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

/// `POST /api/synonym/add` — link two tag names as synonyms (symmetric).
pub async fn api_add_synonym(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AddSynonymRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, body.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    db::link_synonyms(&conn, &[body.name.as_str(), body.other.as_str()]).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// `POST /api/synonym/remove` — remove a tag from its synonym group.
pub async fn api_remove_synonym(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RemoveSynonymRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, body.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    let removed = db::remove_synonym(&conn, &body.name).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "ok": removed })))
}

/// `POST /api/synonym/attr` — set an attribute on a tag name.
pub async fn api_set_tag_attr(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SetTagAttrRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, body.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    db::set_tag_attr(&conn, &body.name, &body.key, &body.value).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// `POST /api/synonym/attr-remove` — remove an attribute from a tag name.
pub async fn api_remove_tag_attr(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RemoveTagAttrRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, body.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    let removed = db::remove_tag_attr(&conn, &body.name, &body.key).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "ok": removed })))
}

/// `GET /api/display-context` — return the current display context.
pub async fn api_get_display_context(
    State(state): State<Arc<AppState>>,
    Query(rp): Query<DirParam>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, rp.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    let ctx = db::get_display_context(&conn).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "context": ctx })))
}

/// `POST /api/display-context` — set the display context.
pub async fn api_set_display_context(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SetDisplayContextRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, body.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    db::set_display_context(&conn, &body.context).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "ok": true })))
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
                is_symlink: None,
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

    // Pre-compute whether the current directory itself is covered by a database
    // root.  All non-symlink files in this directory share the same device and
    // are under the same root, so they can reuse this single result instead of
    // each calling file_is_covered() (which does a stat() syscall per file —
    // extremely expensive on NFS/SMB network shares with thousands of entries).
    let dir_covered = file_is_covered(&state, abs_dir);

    // Phase 1: collect raw entries from the filesystem in a single read_dir
    // pass, without any per-entry DB queries.
    struct RawDir {
        name: String,
        is_symlink: bool,
        db_path: String,
    }
    struct RawFile {
        name: String,
        is_symlink: bool,
        db_path: String,
        covered_path: std::path::PathBuf,
        /// True when `covered_path` is a symlink target that may lie outside the
        /// current root; in that case `file_is_covered` must be called individually.
        /// For regular (non-symlink) files this is always false: they inherit
        /// `dir_covered` and never need a per-file stat() call.
        check_covered: bool,
        size: Option<i64>,
        mtime: Option<i64>,
    }

    let mut raw_dirs: Vec<RawDir> = Vec::new();
    let mut raw_files: Vec<RawFile> = Vec::new();

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

        // Determine whether the entry is a symlink.  `DirEntry::file_type()`
        // uses the `d_type` field from the kernel's readdir buffer on Linux/macOS
        // (O(1), no extra syscall on most filesystems) and falls back to
        // `lstat()` only when `d_type` is unavailable (e.g. some NFS mounts).
        let entry_ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        let is_symlink = entry_ft.is_symlink();

        // For symlinks we need two stats: lstat (to confirm it's a symlink) and
        // stat (to follow the link to the target).  For regular files and
        // directories we only need one stat via entry.metadata() — this avoids
        // a second syscall/network round-trip per entry on NFS/SMB shares.
        let (link_meta, target_meta) = if is_symlink {
            let lm = match std::fs::symlink_metadata(entry.path()) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let tm = std::fs::metadata(entry.path()).ok(); // follow the link
            (lm, tm)
        } else {
            let m = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            // Non-symlink: target == entry, so target_meta is not needed separately.
            (m, None)
        };

        // Determine effective kind from the target (or entry itself for non-symlinks).
        // `link_meta` covers non-symlinks and is used as fallback for broken symlinks.
        let effective_meta = target_meta.as_ref().unwrap_or(&link_meta);

        // Determine effective kind from the target.  Broken symlinks are shown
        // as files (type inferred from the link name's extension).
        let effective_is_dir = effective_meta.is_dir() && !(is_symlink && target_meta.is_none());
        let effective_is_file = effective_meta.is_file() || (is_symlink && target_meta.is_none());

        if effective_is_dir {
            // For a symlinked directory use the canonical path for tag-count
            // queries so that tags on the real directory are reflected.
            let dir_db_path = if is_symlink {
                entry
                    .path()
                    .canonicalize()
                    .ok()
                    .and_then(|c| {
                        c.strip_prefix(&db_root.root)
                            .ok()
                            .map(|r| r.to_string_lossy().into_owned())
                    })
                    .unwrap_or_else(|| format!("{}{}", prefix, name))
            } else {
                format!("{}{}", prefix, name)
            };
            raw_dirs.push(RawDir {
                name,
                is_symlink,
                db_path: dir_db_path,
            });
        } else if effective_is_file {
            let rel_path = format!("{}{}", prefix, name);

            // Size and mtime come from the effective metadata (target for symlinks,
            // or entry itself for regular files).
            let size = Some(effective_meta.len() as i64);
            let mtime = effective_meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i64);

            // For symlinks, resolve to the canonical path for DB lookups.
            // Tags are stored under the real file's path because all write
            // operations canonicalise before hitting the DB.
            let (db_lookup_path, covered_path, check_covered) = if is_symlink {
                let canonical_opt = entry.path().canonicalize().ok();
                let canon_rel = canonical_opt
                    .as_deref()
                    .and_then(|c| c.strip_prefix(&db_root.root).ok())
                    .map(|r| r.to_string_lossy().into_owned())
                    .unwrap_or_else(|| rel_path.clone());
                let canon_abs = canonical_opt.unwrap_or_else(|| entry.path());
                // Symlink targets may lie outside the database root or even on a
                // different filesystem, so they need individual coverage checks.
                (canon_rel, canon_abs, true)
            } else {
                // Regular files in this directory always have the same device/root
                // as abs_dir; reuse dir_covered — no per-file stat() call needed.
                (rel_path.clone(), entry.path(), false)
            };

            raw_files.push(RawFile {
                name,
                is_symlink,
                db_path: db_lookup_path,
                covered_path,
                check_covered,
                size,
                mtime,
            });
        }
    }

    // Phase 2: single batch query for all tag counts (replaces N+1 queries).
    // Use unwrap_or_default so that a transient DB error (e.g. SQLITE_BUSY on a
    // network share) silently falls back to showing 0 tags rather than returning
    // HTTP 500 and leaving the page blank — matching the old per-entry behaviour.
    let all_db_paths: Vec<String> = raw_dirs
        .iter()
        .map(|d| d.db_path.clone())
        .chain(raw_files.iter().map(|f| f.db_path.clone()))
        .collect();
    let tag_counts = batch_tag_counts(&conn, &all_db_paths).unwrap_or_default();

    // Phase 3: build ApiDirEntry structs from the collected data.
    let mut dirs: Vec<ApiDirEntry> = raw_dirs
        .into_iter()
        .map(|d| {
            let tc = tag_counts.get(&d.db_path).copied().unwrap_or(0);
            ApiDirEntry {
                name: d.name,
                is_dir: true,
                size: None,
                mtime: None,
                file_count: None,
                tag_count: if tc > 0 { Some(tc) } else { None },
                root_path: None,
                covered: None,
                is_symlink: if d.is_symlink { Some(true) } else { None },
            }
        })
        .collect();
    let mut files: Vec<ApiDirEntry> = raw_files
        .into_iter()
        .map(|f| {
            let tc = tag_counts.get(&f.db_path).copied().unwrap_or(0);
            // Avoid per-file stat() calls on network shares: regular files in
            // this directory inherit dir_covered (same device/root as abs_dir).
            // Only symlinks whose target might lie outside the root need an
            // individual file_is_covered() check.
            let covered = if f.check_covered {
                file_is_covered(&state, &f.covered_path)
            } else {
                dir_covered
            };
            ApiDirEntry {
                name: f.name,
                is_dir: false,
                size: f.size,
                mtime: f.mtime,
                file_count: None,
                tag_count: Some(tc),
                root_path: None,
                covered: Some(covered),
                is_symlink: if f.is_symlink { Some(true) } else { None },
            }
        })
        .collect();

    dirs.sort_by_key(|a| a.name.to_lowercase());
    files.sort_by_key(|a| a.name.to_lowercase());
    let mut entries = Vec::with_capacity(dirs.len() + files.len());
    entries.extend(dirs);
    entries.extend(files);

    Ok(Json(ApiDirListing {
        path: db_rel,
        root_path: db_root.root.display().to_string(),
        entries,
    }))
}

/// Batch-query tag counts for a slice of relative DB paths in a single SQL
/// round-trip.  Returns a map from path to count; paths absent from the DB
/// (zero tags) are simply missing from the map.
fn batch_tag_counts(
    conn: &rusqlite::Connection,
    paths: &[String],
) -> rusqlite::Result<std::collections::HashMap<String, i64>> {
    // SQLite's maximum variable count is 999; stay safely below it.
    const CHUNK: usize = 900;
    let mut map = std::collections::HashMap::with_capacity(paths.len());
    for chunk in paths.chunks(CHUNK) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT f.path, COUNT(ft.tag_id) FROM files f \
             LEFT JOIN file_tags ft ON f.id = ft.file_id \
             WHERE f.path IN ({placeholders}) GROUP BY f.path"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows.flatten() {
            map.insert(row.0, row.1);
        }
    }
    Ok(map)
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
// Filesystem search
// ---------------------------------------------------------------------------

/// `GET /api/fs-search?q=pattern&dir=...` — search files on the filesystem by
/// filename pattern, regardless of whether they are indexed in the database.
///
/// `q` is matched case-insensitively against each file's name:
///   - If `q` contains `*` or `?` it is treated as a glob pattern.
///   - Otherwise every file whose name contains `q` as a substring matches.
///
/// Returns at most 2 000 results to keep the response fast.
pub async fn api_fs_search(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchParams>,
) -> Result<Json<ApiSearchResult>, AppError> {
    let db_root = root_from_dir(&state, params.dir.as_deref())?;
    let root = db_root.root.clone();
    let pattern = params.q.to_lowercase();
    let is_glob = pattern.contains('*') || pattern.contains('?');
    let filetag_dir = root.join(".filetag");

    const MAX_RESULTS: usize = 2000;

    let results: Vec<ApiSearchEntry> = walkdir::WalkDir::new(&root)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| {
            // Skip .filetag dir and any dotfile / dotdir at any level.
            let p = e.path();
            if p.starts_with(&filetag_dir) {
                return false;
            }
            if let Some(name) = p.file_name().and_then(|n| n.to_str())
                && name.starts_with('.')
            {
                return false;
            }
            true
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if is_glob {
                glob_match(&pattern, &name)
            } else {
                name.contains(pattern.as_str())
            }
        })
        .take(MAX_RESULTS)
        .filter_map(|e| {
            e.path().strip_prefix(&root).ok().map(|rel| ApiSearchEntry {
                path: rel.to_string_lossy().into_owned(),
                tags: vec![],
            })
        })
        .collect();

    Ok(Json(ApiSearchResult {
        query: params.q,
        results,
    }))
}

/// Minimal glob matcher supporting `*` (any sequence) and `?` (any single character).
/// Both `pattern` and `text` must already be lowercased.
fn glob_match(pattern: &str, text: &str) -> bool {
    let p = pattern.as_bytes();
    let t = text.as_bytes();
    let mut pi = 0usize;
    let mut ti = 0usize;
    let mut star_pi = usize::MAX;
    let mut star_ti = 0usize;
    loop {
        if ti < t.len() {
            if pi < p.len() && (p[pi] == b'?' || p[pi] == t[ti]) {
                pi += 1;
                ti += 1;
                continue;
            }
            if pi < p.len() && p[pi] == b'*' {
                star_pi = pi;
                star_ti = ti;
                pi += 1;
                continue;
            }
            if star_pi != usize::MAX {
                pi = star_pi + 1;
                star_ti += 1;
                ti = star_ti;
                continue;
            }
            return false;
        }
        // Consume trailing `*` wildcards.
        while pi < p.len() && p[pi] == b'*' {
            pi += 1;
        }
        return pi == p.len();
    }
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

    // Auto-create the subject if it does not exist yet.
    if let Some(ref s) = body.subject
        && !s.is_empty()
    {
        db::create_subject(&conn, s).map_err(AppError)?;
    }

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

/// `POST /api/assign-subject` — assign a file to a subject by adding a
/// same-named subject-scoped tag, or by reassigning an existing bare row when
/// `mode = "reassign"`.
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
    let updated = match body.mode.as_deref() {
        Some("reassign") => {
            db::reassign_file_tag_to_subject(&conn, file_id, &body.subject, &body.subject)
                .map_err(AppError)?
        }
        _ => db::assign_file_to_subject(&conn, file_id, &body.subject).map_err(AppError)?,
    };
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

/// `GET /api/subject/tags` — list file-level tags assigned under a subject.
pub async fn api_subject_tags(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubjectPropsParams>,
) -> Result<Json<Vec<serde_json::Value>>, AppError> {
    let db_root = root_from_dir(&state, params.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    let rows = db::subject_file_tags(&conn, &params.name).map_err(AppError)?;
    Ok(Json(
        rows.into_iter()
            .map(|(value, count)| serde_json::json!({ "value": value, "count": count }))
            .collect(),
    ))
}

/// `POST /api/subject/add-tag` — add a tag to all files in a subject.
pub async fn api_subject_add_tag(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SubjectPropRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, body.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    let inserted =
        db::add_tag_to_subject_files(&conn, &body.subject, &body.tag).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "inserted": inserted })))
}

/// `POST /api/subject/remove-tag` — remove a tag from all files in a subject.
pub async fn api_subject_remove_tag(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SubjectPropRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir(&state, body.dir.as_deref())?;
    let conn = open_conn(db_root)?;
    let removed =
        db::remove_tag_from_subject_files(&conn, &body.subject, &body.tag).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "removed": removed })))
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
    // Detecteer of ImageMagick (magick/convert) en ffmpeg geïnstalleerd zijn
    fn tool_installed(names: &[&str]) -> bool {
        let mut found = false;
        for &n in names {
            if which::which(n).is_ok() {
                found = true;
            }
        }
        found
    }
    let imagemagick_installed = tool_installed(&["magick", "convert"]);
    let ffmpeg_installed = tool_installed(&["ffmpeg"]);
    let dir_preview_style: String = db::get_setting(&conn, "dir_preview_style")
        .map_err(AppError)?
        .filter(|v| {
            v == "fit" || v == "crop" || v == "scattered" || v == "grid" || v == "bookshelf"
        })
        .unwrap_or_else(|| "crop".to_string());
    Ok(Json(serde_json::json!({
        "sprite_min": sprite_min,
        "sprite_max": sprite_max,
        "feature_video": bool_setting("feature.video"),
        "feature_imagemagick": bool_setting("feature.imagemagick"),
        "feature_pdf": bool_setting("feature.pdf"),
        "feature_saliency_pose": bool_setting("feature.saliency_pose"),
        "feature_saliency_object": bool_setting("feature.saliency_object"),
        "saliency_pose_ready": crate::saliency::pose_model_ready(),
        "saliency_object_ready": crate::saliency::object_model_ready(),
        "dir_preview_style": dir_preview_style,
        "imagemagick_installed": imagemagick_installed,
        "ffmpeg_installed": ffmpeg_installed
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
    if let Some(v) = body.feature_saliency_pose {
        db::set_setting(&conn, "feature.saliency_pose", bool_to_str(v)).map_err(AppError)?;
    }
    if let Some(v) = body.feature_saliency_object {
        db::set_setting(&conn, "feature.saliency_object", bool_to_str(v)).map_err(AppError)?;
    }
    if let Some(v) = body.dir_preview_style {
        // Whitelist: only persist known values.
        if v == "crop" || v == "fit" || v == "scattered" || v == "grid" || v == "bookshelf" {
            db::set_setting(&conn, "dir_preview_style", &v).map_err(AppError)?;
        }
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}
