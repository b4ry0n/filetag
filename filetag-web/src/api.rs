use std::path::{Path, PathBuf};
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
    preview_safe_path, resolve_preview, root_for_dir, root_from_dir_or_id, safe_path,
};
use crate::types::*;
use crate::video::video_info;
use filetag_lib::db::TagRoot;

// ---------------------------------------------------------------------------
// Root resolution from `dir` parameter
// ---------------------------------------------------------------------------

/// Convenience wrapper: resolve root from `dir` only (no root_id).
fn root_from_dir<'a>(state: &'a AppState, dir: Option<&str>) -> Result<&'a TagRoot, AppError> {
    root_from_dir_or_id(state, dir, None)
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
js_handler!(js_filetree, "../static/js/filetree.js");
js_handler!(js_prompt_wizard, "../static/js/prompt-wizard.js");
js_handler!(js_select, "../static/js/select.js");
css_handler!(css_face, "../static/css/face.css");
css_handler!(css_mobile, "../static/css/mobile.css");
css_handler!(css_jobs, "../static/css/jobs.css");
js_handler!(js_jobs, "../static/js/jobs.js");

/// Serve the embedded `favicon.ico`.
pub async fn favicon() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "image/svg+xml")],
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><text y=".9em" font-size="90">🏷</text></svg>"#,
    )
}

/// Serve the embedded OpenAPI specification.
pub async fn openapi_yaml() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "application/yaml; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        include_str!("../static/openapi.yaml"),
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
    let db_root = root_from_dir_or_id(&state, Some(body.dir.as_str()), body.root_id)?;
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
    let db_root = root_from_dir_or_id(&state, rp.dir.as_deref(), rp.root_id)?;
    let conn = open_conn(db_root)?;
    let files: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
    let tags: i64 = conn.query_row("SELECT COUNT(*) FROM tags", [], |r| r.get(0))?;
    let assignments: i64 = conn.query_row("SELECT COUNT(*) FROM file_tags", [], |r| r.get(0))?;
    let total_size: i64 =
        conn.query_row("SELECT COALESCE(SUM(size), 0) FROM files", [], |r| r.get(0))?;

    Ok(Json(ApiInfo {
        root_id: state
            .roots
            .iter()
            .position(|r| r.root == db_root.root)
            .unwrap_or(0),
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
    let db_root = match root_from_dir_or_id(&state, rp.dir.as_deref(), rp.root_id) {
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
    let db_root = root_from_dir_or_id(&state, rp.dir.as_deref(), rp.root_id)?;
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
    let db_root = root_from_dir_or_id(&state, rp.dir.as_deref(), rp.root_id)?;
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
    let db_root = root_from_dir_or_id(&state, rp.dir.as_deref(), rp.root_id)?;
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
    let db_root = root_from_dir_or_id(&state, rp.dir.as_deref(), rp.root_id)?;
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
    let db_root = root_from_dir_or_id(&state, rp.dir.as_deref(), rp.root_id)?;
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
    let db_root = root_from_dir_or_id(&state, rp.dir.as_deref(), rp.root_id)?;
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
    let db_root = root_from_dir_or_id(&state, rp.dir.as_deref(), rp.root_id)?;
    let conn = open_conn(db_root).map_err(AppError)?;
    conn.execute_batch("VACUUM;")
        .map_err(|e| AppError(e.into()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Tags list
// ---------------------------------------------------------------------------

/// `GET /api/tags` — list all known tags with usage counts, colours and synonyms.
pub async fn api_tags(State(state): State<Arc<AppState>>) -> Result<Json<Vec<ApiTag>>, AppError> {
    use std::collections::HashMap;
    // Merge tags across all loaded roots: (count, color, has_values, synonyms).
    let mut merged: HashMap<String, (i64, Option<String>, bool, Vec<String>)> = HashMap::new();
    for root in &state.roots {
        let Ok(conn) = open_conn(root) else { continue };
        let Ok(tags) = db::all_tags(&conn) else {
            continue;
        };
        for (name, count, color, has_values) in tags {
            let entry = merged
                .entry(name.clone())
                .or_insert((0, None, false, vec![]));
            entry.0 += count;
            if entry.1.is_none() && color.is_some() {
                entry.1 = color;
            }
            if has_values {
                entry.2 = true;
            }
            if entry.3.is_empty()
                && let Ok(syns) = db::synonyms_for_tag(&conn, &name)
                && !syns.is_empty()
            {
                entry.3 = syns;
            }
        }
    }
    let mut result: Vec<ApiTag> = merged
        .into_iter()
        .map(|(name, (count, color, has_values, synonyms))| ApiTag {
            name,
            count,
            color,
            synonyms,
            has_values,
        })
        .collect();
    result.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(result))
}

/// `GET /api/tag-values` — list all distinct values for a given k/v tag.
/// Merges results across all loaded roots, just like `api_tags` does, so that
/// values from child / sibling databases are included regardless of which
/// directory is currently browsed.
pub async fn api_tag_values(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TagValuesParams>,
) -> Result<Json<Vec<ApiTagValue>>, AppError> {
    use std::collections::HashMap;
    let mut merged: HashMap<String, i64> = HashMap::new();
    for root in &state.roots {
        let Ok(conn) = open_conn(root) else { continue };
        let Ok(values) = db::tag_values(&conn, &params.name) else {
            continue;
        };
        for (value, count) in values {
            *merged.entry(value).or_insert(0) += count;
        }
    }
    let mut result: Vec<ApiTagValue> = merged
        .into_iter()
        .map(|(value, count)| ApiTagValue { value, count })
        .collect();
    result.sort_by(|a, b| b.count.cmp(&a.count).then(a.value.cmp(&b.value)));
    Ok(Json(result))
}

/// `GET /api/subjects` — list all distinct subjects with file counts.
/// Merges results across all loaded roots, just like `api_tags` does.
pub async fn api_subjects(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<ApiSubject>>, AppError> {
    use std::collections::HashMap;
    let mut merged: HashMap<String, i64> = HashMap::new();
    for root in &state.roots {
        let Ok(conn) = open_conn(root) else { continue };
        let Ok(rows) = db::all_subjects(&conn) else {
            continue;
        };
        for (name, count) in rows {
            *merged.entry(name).or_insert(0) += count;
        }
    }
    let mut result: Vec<ApiSubject> = merged
        .into_iter()
        .map(|(name, count)| ApiSubject { name, count })
        .collect();
    result.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// Synonym management
// ---------------------------------------------------------------------------

/// `POST /api/synonym/add` — link two tag names as synonyms (symmetric).
pub async fn api_add_synonym(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AddSynonymRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;
    let conn = open_conn(db_root)?;
    db::link_synonyms(&conn, &[body.name.as_str(), body.other.as_str()]).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// `POST /api/synonym/remove` — remove a tag from its synonym group.
pub async fn api_remove_synonym(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RemoveSynonymRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;
    let conn = open_conn(db_root)?;
    let removed = db::remove_synonym(&conn, &body.name).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "ok": removed })))
}

/// `POST /api/synonym/attr` — set an attribute on a tag name.
pub async fn api_set_tag_attr(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SetTagAttrRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;
    let conn = open_conn(db_root)?;
    db::set_tag_attr(&conn, &body.name, &body.key, &body.value).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// `POST /api/synonym/attr-remove` — remove an attribute from a tag name.
pub async fn api_remove_tag_attr(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RemoveTagAttrRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;
    let conn = open_conn(db_root)?;
    let removed = db::remove_tag_attr(&conn, &body.name, &body.key).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "ok": removed })))
}

/// `GET /api/display-context` — return the current display context.
pub async fn api_get_display_context(
    State(state): State<Arc<AppState>>,
    Query(rp): Query<DirParam>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir_or_id(&state, rp.dir.as_deref(), rp.root_id)?;
    let conn = open_conn(db_root)?;
    let ctx = db::get_display_context(&conn).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "context": ctx })))
}

/// `POST /api/display-context` — set the display context.
pub async fn api_set_display_context(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SetDisplayContextRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;
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
    if params.dir.is_none() && params.root_id.is_none() {
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
            .map(|&(id, r, _)| ApiDirEntry {
                name: r.name.clone(),
                is_dir: true,
                size: None,
                mtime: None,
                file_count: None,
                tag_count: None,
                root_path: Some(r.root.display().to_string()),
                root_id: Some(id),
                covered: None,
                is_symlink: None,
            })
            .collect();
        return Ok(Json(ApiDirListing {
            path: String::new(),
            root_id: None,
            root_path: String::new(),
            entries,
        }));
    }

    let db_root = root_from_dir_or_id(&state, params.dir.as_deref(), params.root_id)?;
    // When root_id is provided without a dir, browse the root directory itself.
    let root_owned = db_root.root.clone();
    let abs_dir: &std::path::Path = match params.dir.as_deref() {
        Some(d) => std::path::Path::new(d),
        None => &root_owned,
    };
    let root_id_val = state
        .roots
        .iter()
        .position(|r| r.root == db_root.root)
        .unwrap_or(0);

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

    // Pre-compute covered status for the current directory.  Regular files
    // inherit this value; only symlinks whose target may lie outside the root
    // need an individual file_is_covered() check.
    let dir_covered = file_is_covered(&state, abs_dir);

    // Phase 1: collect raw entries from the filesystem in a single read_dir
    // pass, without any per-entry DB queries.
    struct RawDir {
        name: String,
        is_symlink: bool,
        db_path: String,
        /// Absolute path used for counting direct children (see Phase 3).
        abs_path: std::path::PathBuf,
    }
    struct RawFile {
        name: String,
        is_symlink: bool,
        db_path: String,
        covered_path: std::path::PathBuf,
        /// True when `covered_path` is a symlink target that may lie outside
        /// the current root; regular files always use `dir_covered`.
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

        // symlink_metadata() gives us type, size and mtime without following
        // symlinks, so we can detect them reliably.
        let lmeta = match entry.path().symlink_metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };

        if lmeta.file_type().is_symlink() {
            // Symlinks: one extra stat() to follow the link.
            let tm = std::fs::metadata(entry.path()).ok();
            let target_is_dir = tm.as_ref().is_some_and(|m| m.is_dir());
            let target_is_file = tm.as_ref().is_some_and(|m| m.is_file()) || tm.is_none();

            if target_is_dir {
                let db_path = entry
                    .path()
                    .canonicalize()
                    .ok()
                    .and_then(|c| {
                        c.strip_prefix(&db_root.root)
                            .ok()
                            .map(|r| r.to_string_lossy().into_owned())
                    })
                    .unwrap_or_else(|| format!("{}{}", prefix, name));
                raw_dirs.push(RawDir {
                    abs_path: entry.path(),
                    name,
                    is_symlink: true,
                    db_path,
                });
            } else if target_is_file {
                let rel_path = format!("{}{}", prefix, name);
                let size = tm.as_ref().map(|m| m.len() as i64);
                let mtime = tm
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos() as i64);
                let canonical_opt = entry.path().canonicalize().ok();
                let canon_rel = canonical_opt
                    .as_deref()
                    .and_then(|c| c.strip_prefix(&db_root.root).ok())
                    .map(|r| r.to_string_lossy().into_owned())
                    .unwrap_or_else(|| rel_path.clone());
                let canon_abs = canonical_opt.unwrap_or_else(|| entry.path());
                raw_files.push(RawFile {
                    name,
                    is_symlink: true,
                    db_path: canon_rel,
                    covered_path: canon_abs,
                    check_covered: true,
                    size,
                    mtime,
                });
            }
        } else if lmeta.is_dir() {
            raw_dirs.push(RawDir {
                db_path: format!("{}{}", prefix, name),
                abs_path: entry.path(),
                name,
                is_symlink: false,
            });
        } else if lmeta.is_file() {
            let size = Some(lmeta.len() as i64);
            let mtime = lmeta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i64);
            let rel = format!("{}{}", prefix, name);
            raw_files.push(RawFile {
                name,
                is_symlink: false,
                db_path: rel,
                covered_path: entry.path(),
                check_covered: false,
                size,
                mtime,
            });
        }
    }

    // Phase 2: single batch query for all tag counts.
    let all_db_paths: Vec<String> = raw_dirs
        .iter()
        .map(|d| d.db_path.clone())
        .chain(raw_files.iter().map(|f| f.db_path.clone()))
        .collect();
    let tag_counts = batch_tag_counts(&conn, &all_db_paths)?;

    // Phase 3: build ApiDirEntry structs from the collected data.
    // Count direct (non-hidden) children for each subdirectory.
    let mut dirs: Vec<ApiDirEntry> = raw_dirs
        .into_iter()
        .map(|d| {
            let tc = tag_counts.get(&d.db_path).copied().unwrap_or(0);
            let file_count = std::fs::read_dir(&d.abs_path).ok().map(|rd| {
                rd.flatten()
                    .filter(|e| {
                        !e.file_name().to_string_lossy().starts_with('.')
                            && e.file_name().to_string_lossy() != ".filetag"
                    })
                    .count() as i64
            });
            ApiDirEntry {
                name: d.name,
                is_dir: true,
                size: None,
                mtime: None,
                file_count,
                tag_count: if tc > 0 { Some(tc) } else { None },
                root_path: None,
                root_id: None,
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
            // Symlinks may point outside the current root, so check individually.
            // Regular files inherit dir_covered (same filesystem as abs_dir).
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
                root_id: None,
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
        root_id: Some(root_id_val),
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
    let expr = query::parse(&params.q).map_err(AppError)?;
    let mut all_results: Vec<ApiSearchEntry> = Vec::new();
    for (idx, root) in state.roots.iter().enumerate() {
        let Ok(conn) = open_conn(root) else { continue };
        let Ok(results) = query::execute_with_tags(&conn, &expr) else {
            continue;
        };
        let root_str = root.root.display().to_string();
        for (path, tags) in results {
            all_results.push(ApiSearchEntry {
                path,
                root_id: idx,
                root_path: root_str.clone(),
                tags: tags
                    .into_iter()
                    .map(|(name, value)| ApiFileTag {
                        name,
                        value,
                        subject: None,
                        implicit: false,
                    })
                    .collect(),
            });
        }
    }
    Ok(Json(ApiSearchResult {
        query: params.q,
        results: all_results,
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
    let db_root = root_from_dir_or_id(&state, params.dir.as_deref(), params.root_id)?;
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
            let root_str = root.display().to_string();
            let root_id = state.roots.iter().position(|r| r.root == root).unwrap_or(0);
            e.path().strip_prefix(&root).ok().map(|rel| ApiSearchEntry {
                path: rel.to_string_lossy().into_owned(),
                root_id,
                root_path: root_str,
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
    let db_root = root_from_dir_or_id(&state, params.dir.as_deref(), params.root_id)?;

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
        eff_rel.map(|r| (conn, r, eff_root))
    });

    if let Some((conn, effective_rel, eff_root)) = db_lookup
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
            root_id: state
                .roots
                .iter()
                .position(|r| r.root == eff_root)
                .unwrap_or(0),
            root_path: eff_root.display().to_string(),
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
            root_id: state
                .roots
                .iter()
                .position(|r| r.root == db_root.root)
                .unwrap_or(0),
            root_path: db_root.root.display().to_string(),
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
        root_id: state
            .roots
            .iter()
            .position(|r| r.root == db_root.root)
            .unwrap_or(0),
        root_path: db_root.root.display().to_string(),
        tags: vec![],
        duration,
    }))
}

/// `POST /api/files-tags` — return tags for multiple files in one request.
///
/// Groups paths by database root and uses two SQL queries per root
/// (`SELECT … WHERE path IN (…)` then `SELECT … WHERE file_id IN (…)`)
/// instead of one round-trip per file.  Much faster for large multi-file
/// selections.  Returns `{ "files": { "<path>": [ {name, value, subject}, … ] } }`.
/// Paths that are not yet indexed return an empty tag array.
pub async fn api_files_tags(
    State(state): State<Arc<AppState>>,
    Json(body): Json<FilesTagsRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    use std::collections::HashMap;
    use std::path::PathBuf;

    if body.paths.is_empty() {
        return Ok(Json(serde_json::json!({ "files": {} })));
    }

    let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;

    // Group paths by their effective database root.
    let mut by_root: HashMap<PathBuf, Vec<(String, String)>> = HashMap::new();
    for path in &body.paths {
        let fs_path = if let Some(zip_part) = path.split_once("::").map(|(z, _)| z) {
            match preview_safe_path(&db_root.root, zip_part) {
                Some(p) => p,
                None => continue,
            }
        } else {
            match safe_path(&db_root.root, path) {
                Ok(p) => p,
                Err(_) => continue,
            }
        };
        let start = fs_path.parent().unwrap_or(&fs_path);
        let effective_root = match db::find_root(start) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let effective_rel = if let Some(entry) = path.split_once("::").map(|(_, e)| e) {
            match db::relative_to_root(&fs_path, &effective_root) {
                Ok(zip_rel) => format!("{}::{}", zip_rel, entry),
                Err(_) => continue,
            }
        } else {
            match db::relative_to_root(&fs_path, &effective_root) {
                Ok(r) => r,
                Err(_) => continue,
            }
        };
        by_root
            .entry(effective_root)
            .or_default()
            .push((path.clone(), effective_rel));
    }

    // Result: original path → Vec<tag objects>.
    let mut result: HashMap<String, Vec<serde_json::Value>> =
        body.paths.iter().map(|p| (p.clone(), Vec::new())).collect();

    for (effective_root, path_pairs) in by_root {
        let (conn, _) = match db::find_and_open_fast(&effective_root) {
            Ok(c) => c,
            Err(_) => continue,
        };

        // Build effective_rel → original_path lookup.
        let rel_to_orig: HashMap<String, String> = path_pairs
            .iter()
            .map(|(o, r)| (r.clone(), o.clone()))
            .collect();

        let effective_rels: Vec<&str> = path_pairs.iter().map(|(_, r)| r.as_str()).collect();

        // Query files in chunks to stay under SQLite variable limit (999).
        const CHUNK: usize = 400;
        let mut id_to_orig: HashMap<i64, String> = HashMap::new();

        for chunk in effective_rels.chunks(CHUNK) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT id, path FROM files WHERE path IN ({})",
                placeholders
            );
            let rows: Vec<(i64, String)> = conn
                .prepare(&sql)
                .map_err(|e| AppError(e.into()))?
                .query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| AppError(e.into()))?
                .filter_map(|r| r.ok())
                .collect();

            for (file_id, eff_rel) in rows {
                if let Some(orig) = rel_to_orig.get(&eff_rel) {
                    id_to_orig.insert(file_id, orig.clone());
                }
            }
        }

        if id_to_orig.is_empty() {
            continue;
        }

        // Fetch all tags for all file IDs in one query (also chunked).
        let ids: Vec<i64> = id_to_orig.keys().copied().collect();
        for chunk in ids.chunks(CHUNK) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT ft.file_id, t.name, ft.value, ft.subject
                 FROM file_tags ft
                 JOIN tags t ON t.id = ft.tag_id
                 WHERE ft.file_id IN ({})
                 ORDER BY ft.file_id, ft.subject, t.name, ft.value",
                placeholders
            );
            let rows: Vec<(i64, String, String, String)> = conn
                .prepare(&sql)
                .map_err(|e| AppError(e.into()))?
                .query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(|e| AppError(e.into()))?
                .filter_map(|r| r.ok())
                .collect();

            for (file_id, name, value, subject) in rows {
                if let Some(orig) = id_to_orig.get(&file_id) {
                    let tags = result.entry(orig.clone()).or_default();
                    tags.push(serde_json::json!({
                        "name": name,
                        "value": if value.is_empty() { serde_json::Value::Null } else { value.into() },
                        "subject": if subject.is_empty() { serde_json::Value::Null } else { subject.into() },
                    }));
                }
            }
        }
    }

    Ok(Json(serde_json::json!({ "files": result })))
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
    let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;
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
    let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;
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
// Bulk tag / untag
// ---------------------------------------------------------------------------

/// `POST /api/tag-bulk` — apply tags to multiple files in one request.
///
/// All files belonging to the same database root are processed in a single
/// SQLite transaction, reducing I/O from O(n) fsyncs to O(k) fsyncs where k
/// is the number of distinct database roots in the selection.  Tag IDs are
/// resolved once per database root rather than once per file.
pub async fn api_tag_bulk(
    State(state): State<Arc<AppState>>,
    Json(body): Json<BulkTagRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    use std::collections::HashMap;
    use std::path::PathBuf;

    if body.paths.is_empty() {
        return Ok(Json(serde_json::json!({ "added": 0 })));
    }

    let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;

    // Group paths by their effective database root (handles child-DB routing).
    // Use find_root — a pure path-walk — to avoid opening N connections.
    let mut by_root: HashMap<PathBuf, Vec<(String, String)>> = HashMap::new();
    for path in &body.paths {
        let fs_path = if let Some(zip_part) = path.split_once("::").map(|(z, _)| z) {
            preview_safe_path(&db_root.root, zip_part)
                .ok_or_else(|| AppError(anyhow::anyhow!("invalid path '{}': escapes root", path)))?
        } else {
            safe_path(&db_root.root, path).map_err(AppError)?
        };
        let start = fs_path.parent().unwrap_or(&fs_path);
        let effective_root = db::find_root(start).map_err(AppError)?;
        let effective_rel = if let Some(entry) = path.split_once("::").map(|(_, e)| e) {
            let zip_rel = db::relative_to_root(&fs_path, &effective_root).map_err(AppError)?;
            format!("{}::{}", zip_rel, entry)
        } else {
            db::relative_to_root(&fs_path, &effective_root).map_err(AppError)?
        };
        by_root
            .entry(effective_root)
            .or_default()
            .push((path.clone(), effective_rel));
    }

    let mut total_added = 0i64;
    for (effective_root, path_pairs) in by_root {
        // One connection per DB root (find_and_open_fast skips migrations).
        let (conn, _) = db::find_and_open_fast(&effective_root).map_err(AppError)?;

        if let Some(ref s) = body.subject
            && !s.is_empty()
        {
            db::create_subject(&conn, s).map_err(AppError)?;
        }

        // Resolve tag IDs once per root — not once per file.
        let mut tag_specs: Vec<(i64, Option<String>)> = Vec::new();
        for tag_str in &body.tags {
            let (name, value) = parse_tag(tag_str);
            let tag_id = db::get_or_create_tag(&conn, &name).map_err(AppError)?;
            tag_specs.push((tag_id, value));
        }

        // Single transaction for all files in this root.
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AppError(e.into()))?;
        for (path, effective_rel) in &path_pairs {
            let file_id = if path.contains("::") {
                ensure_zip_entry_record(&tx, effective_rel).map_err(AppError)?
            } else {
                db::get_or_index_file(&tx, effective_rel, &effective_root)
                    .map_err(AppError)?
                    .id
            };
            for (tag_id, value) in &tag_specs {
                db::apply_tag(
                    &tx,
                    file_id,
                    *tag_id,
                    value.as_deref(),
                    body.subject.as_deref(),
                )
                .map_err(AppError)?;
                total_added += 1;
            }
        }
        tx.commit().map_err(|e| AppError(e.into()))?;
    }

    Ok(Json(serde_json::json!({ "added": total_added })))
}

/// `POST /api/untag-bulk` — remove tags from multiple files in one request.
///
/// Like [`api_tag_bulk`] but for removal.  Files are grouped by database root
/// and each group is processed in a single transaction.
pub async fn api_untag_bulk(
    State(state): State<Arc<AppState>>,
    Json(body): Json<BulkTagRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    use std::collections::HashMap;
    use std::path::PathBuf;

    if body.paths.is_empty() {
        return Ok(Json(serde_json::json!({ "removed": 0 })));
    }

    let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;

    let mut by_root: HashMap<PathBuf, Vec<(String, String)>> = HashMap::new();
    for path in &body.paths {
        let fs_path = if let Some(zip_part) = path.split_once("::").map(|(z, _)| z) {
            preview_safe_path(&db_root.root, zip_part)
                .ok_or_else(|| AppError(anyhow::anyhow!("invalid path '{}': escapes root", path)))?
        } else {
            safe_path(&db_root.root, path).map_err(AppError)?
        };
        let start = fs_path.parent().unwrap_or(&fs_path);
        let effective_root = db::find_root(start).map_err(AppError)?;
        let effective_rel = if let Some(entry) = path.split_once("::").map(|(_, e)| e) {
            let zip_rel = db::relative_to_root(&fs_path, &effective_root).map_err(AppError)?;
            format!("{}::{}", zip_rel, entry)
        } else {
            db::relative_to_root(&fs_path, &effective_root).map_err(AppError)?
        };
        by_root
            .entry(effective_root)
            .or_default()
            .push((path.clone(), effective_rel));
    }

    let mut total_removed = 0i64;
    for (effective_root, path_pairs) in by_root {
        let (conn, _) = db::find_and_open_fast(&effective_root).map_err(AppError)?;

        let mut tag_specs: Vec<(i64, Option<String>)> = Vec::new();
        for tag_str in &body.tags {
            let (name, value) = parse_tag(tag_str);
            if let Ok(tag_id) = conn.query_row(
                "SELECT id FROM tags WHERE name = ?1",
                rusqlite::params![&name],
                |r| r.get::<_, i64>(0),
            ) {
                tag_specs.push((tag_id, value));
            }
        }

        if tag_specs.is_empty() {
            continue;
        }

        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AppError(e.into()))?;
        for (_, effective_rel) in &path_pairs {
            if let Ok(Some(record)) = db::file_by_path(&tx, effective_rel) {
                for (tag_id, value) in &tag_specs {
                    if db::remove_tag(
                        &tx,
                        record.id,
                        *tag_id,
                        value.as_deref(),
                        body.subject.as_deref(),
                    )
                    .map_err(AppError)?
                    {
                        total_removed += 1;
                    }
                }
            }
        }
        tx.commit().map_err(|e| AppError(e.into()))?;
    }

    Ok(Json(serde_json::json!({ "removed": total_removed })))
}

// ---------------------------------------------------------------------------
// Background job store
// ---------------------------------------------------------------------------

/// `GET /api/jobs` — list all background jobs plus synthetic entries for the
/// existing AI, face-scan, and pHash progress mechanisms.
/// Collect all job entries — native store jobs plus synthetic progress entries
/// for AI analysis, face scanning, similarity indexing, and model downloads —
/// and return them as a JSON object `{"jobs": [...]}`.  Used by both the
/// regular polling endpoint and the SSE stream.
fn build_jobs_json(state: &AppState) -> serde_json::Value {
    let mut jobs: Vec<serde_json::Value> = Vec::new();

    // 1. Native jobs registered in the job store.
    {
        let store = state.jobs.lock().unwrap();
        for job in store.list() {
            jobs.push(serde_json::to_value(job).unwrap_or_default());
        }
    }

    // 2. AI batch progress (synthetic entry — visible only while running or
    //    shortly after completion).
    {
        let prog = state.ai_progress.lock().unwrap().clone();
        if prog.running || prog.done > 0 {
            jobs.push(serde_json::json!({
                "id": "__ai",
                "kind": "ai-batch",
                "label": "AI analyse",
                "status": if prog.running { "running" } else { "done" },
                "done": prog.done,
                "total": prog.total,
                "current": prog.current,
                "created_ms": 0,
                "updated_ms": 0,
            }));
        }
    }

    // 3. Face-scan progress.
    {
        let prog = state.face_progress.lock().unwrap().clone();
        if prog.running || prog.done > 0 {
            jobs.push(serde_json::json!({
                "id": "__face",
                "kind": "face-scan",
                "label": "Gezichtsherkenning",
                "status": if prog.running { "running" } else { "done" },
                "done": prog.done,
                "total": prog.total,
                "current": prog.current,
                "created_ms": 0,
                "updated_ms": 0,
            }));
        }
    }

    // 4. pHash / similarity-index progress.
    {
        let prog = state.phash_progress.lock().unwrap().clone();
        if prog.running || prog.done > 0 {
            let status = if prog.running {
                "running"
            } else if prog.cancelled {
                "failed"
            } else {
                "done"
            };
            let mut entry = serde_json::json!({
                "id": "__phash",
                "kind": "similarity",
                "label": "Gelijkheidsindex",
                "status": status,
                "done": prog.done,
                "total": prog.total,
                "current": prog.current,
                "created_ms": 0,
                "updated_ms": 0,
            });
            if prog.cancelled {
                entry["error"] = serde_json::json!("Gestopt");
            }
            jobs.push(entry);
        }
    }

    // 5. Model download (face) — shown only while active.
    {
        let prog = state.model_download.lock().unwrap().clone();
        if prog.active {
            let total = prog.bytes_total.unwrap_or(0);
            jobs.push(serde_json::json!({
                "id": "__model-dl",
                "kind": "download",
                "label": format!("Model downloaden ({})", prog.phase),
                "status": "running",
                "done": prog.bytes_done,
                "total": total,
                "created_ms": 0,
                "updated_ms": 0,
            }));
        }
    }

    serde_json::json!({ "jobs": jobs })
}

pub async fn api_jobs(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(build_jobs_json(&state))
}

/// `GET /api/jobs/stream` — Server-Sent Events stream for live job updates.
///
/// Sends an initial snapshot immediately, then pushes a fresh snapshot
/// whenever a native job changes *or* every 2 seconds (heartbeat for
/// synthetic jobs such as AI-analysis progress that update outside the
/// `JobStore`).
pub async fn api_jobs_stream(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use std::convert::Infallible;
    use tokio_stream::{
        StreamExt as _,
        wrappers::{BroadcastStream, IntervalStream},
    };

    // Subscribe BEFORE taking the snapshot so no update is lost between the
    // snapshot and the subscription.
    let rx = state.jobs.lock().unwrap().subscribe();

    // Native-job change notifications.
    let native = BroadcastStream::new(rx)
        .filter_map(|r: Result<(), _>| r.ok()) // drop Lagged errors silently
        .map(|_: ()| ());

    // Heartbeat: fires every 2 s to propagate synthetic-job updates
    // (AI progress, face-scan, pHash, model download) that bypass the store.
    // Use interval_at to skip the immediate first tick — the initial snapshot
    // already covers the current state.
    let tick = IntervalStream::new(tokio::time::interval_at(
        tokio::time::Instant::now() + std::time::Duration::from_secs(2),
        std::time::Duration::from_secs(2),
    ))
    .map(|_: tokio::time::Instant| ());

    // Merge: any native change or 2-second heartbeat triggers a push.
    let trigger = native.merge(tick);

    // Initial snapshot delivered without waiting for a trigger.
    let initial_json = serde_json::to_string(&build_jobs_json(&state)).unwrap_or_default();
    let initial = tokio_stream::iter(std::iter::once(Ok::<Event, Infallible>(
        Event::default().data(initial_json),
    )));

    let state_s = state;
    let updates = trigger.map(move |_| {
        let json = serde_json::to_string(&build_jobs_json(&state_s)).unwrap_or_default();
        Ok::<Event, Infallible>(Event::default().data(json))
    });

    Sse::new(initial.chain(updates))
        .keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)))
}

/// `DELETE /api/jobs/:id` — dismiss a specific completed or failed job.
pub async fn api_jobs_dismiss(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    // Synthetic jobs (__ai, __face, etc.) cannot be dismissed from the store;
    // they disappear automatically when their underlying progress resets.
    if !id.starts_with("__") {
        state.jobs.lock().unwrap().dismiss(&id);
    }
    Json(serde_json::json!({ "ok": true }))
}

/// `DELETE /api/jobs` — dismiss all completed and failed jobs.
pub async fn api_jobs_dismiss_all(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    state.jobs.lock().unwrap().dismiss_finished();
    Json(serde_json::json!({ "ok": true }))
}

/// `POST /api/jobs/:id/cancel` — request cancellation of a running job.
///
/// The background task polls `JobStore::is_cancelled` between work items and
/// stops voluntarily.  The job remains in the store (as `Running`) until the
/// task naturally exits and calls `finish` or `fail`.
pub async fn api_jobs_cancel(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    if !id.starts_with("__") {
        state.jobs.lock().unwrap().cancel(&id);
    }
    Json(serde_json::json!({ "ok": true }))
}

// ---------------------------------------------------------------------------
// Recursive directory tagging
// ---------------------------------------------------------------------------

/// `POST /api/tag-dir-recursive` — tag all files in a directory tree.
///
/// The operation is dispatched as a background job and returns a `job_id`
/// immediately.  Callers should poll `GET /api/jobs` to track progress.
pub async fn api_tag_dir_recursive(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TagDirRecursiveRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;
    // Validate the requested directory exists and is inside the root.
    let _ = safe_path(&db_root.root, &body.path).map_err(AppError)?;

    let dir_name = body
        .path
        .trim_end_matches('/')
        .split('/')
        .next_back()
        .unwrap_or(&body.path)
        .to_string();
    let tags_label = body.tags.join(", ");
    let archives_suffix = if body.include_archives {
        " + archieven"
    } else {
        ""
    };
    let label = format!("Tag '{dir_name}': {tags_label}{archives_suffix}");

    let job_id = state.jobs.lock().unwrap().submit("tag-dir", label);

    let state2 = Arc::clone(&state);
    let body2 = body.clone();
    let job_id2 = job_id.clone();
    tokio::spawn(async move {
        let result = do_tag_dir_recursive(state2.clone(), body2, job_id2.clone()).await;
        match result {
            Ok(_) => state2.jobs.lock().unwrap().finish(&job_id2),
            Err(e) => state2.jobs.lock().unwrap().fail(&job_id2, e.to_string()),
        }
    });

    Ok(Json(serde_json::json!({ "job_id": job_id })))
}

/// Background worker for [`api_tag_dir_recursive`].
async fn do_tag_dir_recursive(
    state: Arc<AppState>,
    body: TagDirRecursiveRequest,
    job_id: String,
) -> anyhow::Result<()> {
    // Extract root info before any await points (avoids holding a non-Send ref).
    let (root_path, dir_abs) = {
        let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)
            .map_err(|e| anyhow::anyhow!("{:?}", e.0))?;
        let dir_abs = safe_path(&db_root.root, &body.path).map_err(|e| anyhow::anyhow!("{e}"))?;
        (db_root.root.clone(), dir_abs)
    };

    state
        .jobs
        .lock()
        .unwrap()
        .progress(&job_id, 0, Some("Bestanden verzamelen\u{2026}"));

    // Run all blocking work (walkdir + DB) in the spawn_blocking thread pool.
    // rusqlite::Connection is !Send, so it must not cross await points.
    let jobs_store = Arc::clone(&state.jobs);
    let job_id2 = job_id.clone();

    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        use std::collections::HashMap;
        use std::path::PathBuf;

        let include_archives = body.include_archives;
        let filetag_dir = root_path.join(".filetag");

        // Phase 1 — collect all relative paths (+ archive entries).
        let mut paths: Vec<String> = Vec::new();
        for entry in walkdir::WalkDir::new(&dir_abs)
            .follow_links(true)
            .into_iter()
            .filter_entry(|e| {
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
        {
            let abs = entry.path();
            let rel = db::relative_to_root(abs, &root_path)?;
            paths.push(rel.clone());

            if include_archives && entry.file_type().is_file() {
                let ext = abs
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if matches!(ext.as_str(), "zip" | "cbz" | "rar" | "cbr" | "7z" | "cb7")
                    && let Ok(entries) = crate::archive::archive_list_entries_raw(abs)
                {
                    for (name, _, _) in entries {
                        paths.push(format!("{rel}::{name}"));
                    }
                }
            }
        }

        let total = paths.len() as u64;
        {
            let mut store = jobs_store.lock().unwrap();
            store.start(&job_id2, total);
            store.progress(&job_id2, 0, None);
        }
        if paths.is_empty() {
            return Ok(());
        }

        // Phase 2 — group by effective DB root (child DB routing).
        let mut by_root: HashMap<PathBuf, Vec<(String, String)>> = HashMap::new();
        for path in &paths {
            let fs_path = if let Some(zip_part) = path.split_once("::").map(|(z, _)| z) {
                preview_safe_path(&root_path, zip_part)
                    .ok_or_else(|| anyhow::anyhow!("path '{}' escapes root", path))?
            } else {
                safe_path(&root_path, path)?
            };
            let start = fs_path.parent().unwrap_or(&fs_path);
            let effective_root = db::find_root(start)?;
            let effective_rel = if let Some(entry) = path.split_once("::").map(|(_, e)| e) {
                let zip_rel = db::relative_to_root(&fs_path, &effective_root)?;
                format!("{zip_rel}::{entry}")
            } else {
                db::relative_to_root(&fs_path, &effective_root)?
            };
            by_root
                .entry(effective_root)
                .or_default()
                .push((path.clone(), effective_rel));
        }

        // Phase 3 — tag in chunked transactions, updating progress after each chunk.
        let mut done: u64 = 0;
        const CHUNK: usize = 50;

        for (effective_root, path_pairs) in by_root {
            let (conn, _) = db::find_and_open_fast(&effective_root)?;

            if let Some(ref s) = body.subject
                && !s.is_empty()
            {
                db::create_subject(&conn, s)?;
            }

            let mut tag_specs: Vec<(i64, Option<String>)> = Vec::new();
            for tag_str in &body.tags {
                let (name, value) = parse_tag(tag_str);
                let tag_id = db::get_or_create_tag(&conn, &name)?;
                tag_specs.push((tag_id, value));
            }

            for chunk in path_pairs.chunks(CHUNK) {
                let tx = conn
                    .unchecked_transaction()
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                for (path, effective_rel) in chunk {
                    let file_id = if path.contains("::") {
                        ensure_zip_entry_record(&tx, effective_rel)?
                    } else {
                        db::get_or_index_file(&tx, effective_rel, &effective_root)?.id
                    };
                    for (tag_id, value) in &tag_specs {
                        db::apply_tag(
                            &tx,
                            file_id,
                            *tag_id,
                            value.as_deref(),
                            body.subject.as_deref(),
                        )?;
                    }
                    done += 1;
                }
                tx.commit().map_err(|e| anyhow::anyhow!("{e}"))?;
                jobs_store.lock().unwrap().progress(&job_id2, done, None);
            }
        }

        Ok(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("task join: {e}"))?
}

// ---------------------------------------------------------------------------
// Tag color + delete
// ---------------------------------------------------------------------------

/// `POST /api/rename-tag` — rename a tag across all files in the database.
///
/// The rename is applied to the primary database (determined by `dir`) and to
/// all linked databases (children + ancestors), so that tags spread across
/// multiple linked databases are all updated in one call.
pub async fn api_rename_tag(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RenameTagRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;
    let conn = open_conn(db_root)?;
    // Apply to all linked databases (the primary is included in the result).
    let all_dbs =
        db::collect_all_databases(conn, db_root.root.to_path_buf(), true).unwrap_or_default();
    let mut any_ok = false;
    let mut any_merged = false;
    for db in &all_dbs {
        match db::rename_tag(&db.conn, &body.name, &body.new_name) {
            Ok(db::RenameOutcome::NotFound) | Err(_) => {}
            Ok(db::RenameOutcome::Renamed) => any_ok = true,
            Ok(db::RenameOutcome::Merged { .. }) => {
                any_ok = true;
                any_merged = true;
            }
        }
    }
    Ok(Json(
        serde_json::json!({ "ok": any_ok, "merged": any_merged }),
    ))
}

// ---------------------------------------------------------------------------
// Comic metadata import
// ---------------------------------------------------------------------------

/// `POST /api/comic/import-metadata` — read `ComicInfo.xml` from a comic
/// archive and apply the metadata fields as tags.
///
/// Only archives that contain a `ComicInfo.xml` entry are supported.  Fields
/// are mapped to a `comic/` tag hierarchy; see [`archive::parse_comic_info_tags`]
/// for the full mapping.  Existing tags are not removed; duplicate entries are
/// silently ignored (`INSERT OR IGNORE`).
///
/// Returns `{ imported: usize, tags: [{name, value}] }`.
pub async fn api_comic_import_metadata(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ComicImportRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir_or_id(&state, req.dir.as_deref(), req.root_id)?;
    let (conn, effective_root, effective_rel) =
        open_for_file_op(db_root, &req.path).map_err(AppError)?;

    let abs_path = effective_root.join(&effective_rel);

    // Read ComicInfo.xml from the archive (blocking I/O).
    let xml_bytes =
        tokio::task::spawn_blocking(move || crate::archive::archive_read_comic_info(&abs_path))
            .await
            .map_err(|e| AppError(anyhow::anyhow!("task panicked: {e}")))?
            .map_err(AppError)?;

    let xml_bytes = xml_bytes
        .ok_or_else(|| AppError(anyhow::anyhow!("No ComicInfo.xml found in this archive")))?;

    let tag_pairs = crate::archive::parse_comic_info_tags(&xml_bytes);

    // Index the file if not yet present, then apply tags.
    let file_id = db::get_or_index_file(&conn, &effective_rel, &effective_root)
        .map_err(AppError)?
        .id;

    let mut result_tags: Vec<serde_json::Value> = Vec::new();
    for (tag_name, value) in &tag_pairs {
        let tag_id = db::get_or_create_tag(&conn, tag_name).map_err(AppError)?;
        let val = if value.is_empty() {
            None
        } else {
            Some(value.as_str())
        };
        db::apply_tag(&conn, file_id, tag_id, val, None).map_err(AppError)?;
        result_tags.push(serde_json::json!({ "name": tag_name, "value": value }));
    }

    Ok(Json(
        serde_json::json!({ "imported": result_tags.len(), "tags": result_tags }),
    ))
}

/// `POST /api/tag-color` — set or clear the display colour for a tag.
pub async fn api_tag_color(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TagColorRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;
    let conn = open_conn(db_root)?;
    let ok = db::set_tag_color(&conn, &body.name, body.color.as_deref()).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "ok": ok })))
}

/// `POST /api/delete-tag` — permanently delete a tag and all its assignments.
pub async fn api_delete_tag(
    State(state): State<Arc<AppState>>,
    Json(body): Json<DeleteTagRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;
    let conn = open_conn(db_root)?;
    let deleted = db::delete_tag(&conn, &body.name).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "deleted": deleted })))
}

pub async fn api_prune_tags(
    State(state): State<Arc<AppState>>,
    Json(body): Json<DirBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut removed: usize = 0;
    if body.dir.is_some() {
        // Single root: prune only the root that owns this path.
        let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;
        let conn = open_conn(db_root)?;
        removed += db::prune_unused_tags(&conn).map_err(AppError)?;
    } else {
        // No dir: prune every loaded root.
        for root in &state.roots {
            if let Ok(conn) = open_conn(root) {
                removed += db::prune_unused_tags(&conn).unwrap_or(0);
            }
        }
    }
    Ok(Json(serde_json::json!({ "removed": removed })))
}

/// `POST /api/create-subject` — create an empty subject entity.
pub async fn api_create_subject(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateSubjectRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;
    let conn = open_conn(db_root)?;
    let created = db::create_subject(&conn, &body.name).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "created": created })))
}

/// `POST /api/rename-subject` — rename a subject label across all file-tag assignments.
pub async fn api_rename_subject(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RenameSubjectRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;
    let conn = open_conn(db_root)?;
    let updated = db::rename_subject(&conn, &body.name, &body.new_name).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "updated": updated })))
}

/// `POST /api/delete-subject` — remove a subject label by clearing it on all file-tag assignments.
pub async fn api_delete_subject(
    State(state): State<Arc<AppState>>,
    Json(body): Json<DeleteSubjectRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;
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
    let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;
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
    let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;
    let conn = open_conn(db_root)?;
    let inserted = db::clone_subject(&conn, &body.name, &body.new_name).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "inserted": inserted })))
}

/// `GET /api/subject/props` — list entity properties of a subject.
pub async fn api_subject_props(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubjectPropsParams>,
) -> Result<Json<Vec<serde_json::Value>>, AppError> {
    let db_root = root_from_dir_or_id(&state, params.dir.as_deref(), params.root_id)?;
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
    let db_root = root_from_dir_or_id(&state, params.dir.as_deref(), params.root_id)?;
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
    let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;
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
    let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;
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
    let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;
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
    let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;
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
    let db_root = root_from_dir_or_id(&state, params.dir.as_deref(), params.root_id)?;
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
    let tile_preview_mode: String = db::get_setting(&conn, "tile_preview_mode")
        .map_err(AppError)?
        .filter(|v| v == "sprite" || v == "webm" || v == "webm-seek" || v == "autoplay")
        .unwrap_or_else(|| "sprite".to_string());
    let vtile_duration: u32 = db::get_setting(&conn, "vtile_duration")
        .map_err(AppError)?
        .and_then(|v| v.parse().ok())
        .unwrap_or(8u32)
        .clamp(0, 120);
    let vtile_use_longest: bool = db::get_setting(&conn, "vtile_use_longest")
        .map_err(AppError)?
        .map(|v| v == "1")
        .unwrap_or(false);
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
        "tile_preview_mode": tile_preview_mode,
        "vtile_duration": vtile_duration,
        "vtile_use_longest": vtile_use_longest,
        "imagemagick_installed": imagemagick_installed,
        "ffmpeg_installed": ffmpeg_installed
    })))
}

/// `POST /api/settings` — persist per-root settings.
pub async fn api_settings_set(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SettingsBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_from_dir_or_id(&state, body.dir.as_deref(), body.root_id)?;
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
    if let Some(v) = body.tile_preview_mode
        && (v == "sprite" || v == "webm" || v == "webm-seek" || v == "autoplay")
    {
        db::set_setting(&conn, "tile_preview_mode", &v).map_err(AppError)?;
    }
    if let Some(v) = body.vtile_duration {
        let clamped = v.clamp(0, 120);
        db::set_setting(&conn, "vtile_duration", &clamped.to_string()).map_err(AppError)?;
    }
    if let Some(v) = body.vtile_use_longest {
        db::set_setting(&conn, "vtile_use_longest", if v { "1" } else { "0" }).map_err(AppError)?;
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Filesystem operations (rename, move, delete, copy)
// ---------------------------------------------------------------------------

/// Validate a filename component: non-empty, no path separators, not "."/"..".
fn validate_filename(name: &str) -> anyhow::Result<()> {
    if name.is_empty() {
        anyhow::bail!("filename is empty");
    }
    if name == "." || name == ".." {
        anyhow::bail!("'.' and '..' are not valid filenames");
    }
    if name.contains('/') || name.contains('\\') {
        anyhow::bail!("filename must not contain path separators");
    }
    Ok(())
}

/// Compute the relative path of `abs` under `eff_root` from a canonicalised
/// parent directory.  Does not require `abs` to exist on disk.
fn rel_under_root(canon_parent: &Path, name: &str, eff_root: &Path) -> String {
    let parent_rel = canon_parent
        .strip_prefix(eff_root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    if parent_rel.is_empty() {
        name.to_string()
    } else {
        format!("{}/{}", parent_rel, name)
    }
}

/// POST /api/fs/rename — rename a file or directory in place.
pub async fn api_fs_rename(
    State(state): State<Arc<AppState>>,
    Json(body): Json<FsRenameRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    validate_filename(&body.new_name).map_err(AppError)?;

    let abs_path = PathBuf::from(&body.path);
    if !abs_path.exists() {
        return Err(AppError(anyhow::anyhow!("path does not exist")));
    }

    // Ensure the path is inside a known root.
    root_for_dir(&state, &abs_path).ok_or_else(|| {
        AppError(anyhow::anyhow!(
            "path is not within any known database root"
        ))
    })?;

    let is_dir = abs_path.is_dir();
    let parent = abs_path
        .parent()
        .ok_or_else(|| AppError(anyhow::anyhow!("path has no parent")))?;

    // Open the nearest child DB from the parent directory (always exists).
    let (conn, eff_root) = db::find_and_open_fast(parent).map_err(AppError)?;
    let old_rel = db::relative_to_root(&abs_path, &eff_root).map_err(AppError)?;
    if old_rel == ".filetag" || old_rel.starts_with(".filetag/") {
        return Err(AppError(anyhow::anyhow!(
            "cannot operate on .filetag directory"
        )));
    }

    let new_abs = parent.join(&body.new_name);
    if new_abs.exists() {
        return Err(AppError(anyhow::anyhow!(
            "a file named {:?} already exists",
            body.new_name
        )));
    }

    // Compute new relative path without filesystem access (new_abs doesn't exist yet).
    let canon_parent = parent.canonicalize().map_err(|e| AppError(e.into()))?;
    let new_rel = rel_under_root(&canon_parent, &body.new_name, &eff_root);

    // Perform the rename.
    std::fs::rename(&abs_path, &new_abs)
        .context("rename failed")
        .map_err(AppError)?;

    // Update DB records.
    if is_dir {
        let _ = db::rename_dir_paths(&conn, &old_rel, &new_rel);
    } else {
        let _ = db::rename_file_path(&conn, &old_rel, &new_rel);
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/fs/move — move a file or directory (cross-root allowed).
pub async fn api_fs_move(
    State(state): State<Arc<AppState>>,
    Json(body): Json<FsMoveRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let abs_src = PathBuf::from(&body.path);
    if !abs_src.exists() {
        return Err(AppError(anyhow::anyhow!("source path does not exist")));
    }

    let dest_dir = PathBuf::from(&body.dest_dir);
    if !dest_dir.is_dir() {
        return Err(AppError(anyhow::anyhow!("destination is not a directory")));
    }

    // Both must be within a known root (cross-root moves are allowed).
    root_for_dir(&state, &abs_src).ok_or_else(|| {
        AppError(anyhow::anyhow!(
            "source is not within any known database root"
        ))
    })?;
    root_for_dir(&state, &dest_dir).ok_or_else(|| {
        AppError(anyhow::anyhow!(
            "destination is not within any known database root"
        ))
    })?;

    let name = abs_src
        .file_name()
        .ok_or_else(|| AppError(anyhow::anyhow!("source has no filename")))?;
    let abs_dest = dest_dir.join(name);
    if abs_dest.exists() {
        return Err(AppError(anyhow::anyhow!(
            "a file or directory named {:?} already exists in the destination",
            name
        )));
    }

    let is_src_dir = abs_src.is_dir();

    // Prevent moving a directory into itself.
    if is_src_dir {
        let canon_src = abs_src.canonicalize().map_err(|e| AppError(e.into()))?;
        let canon_dest = dest_dir.canonicalize().map_err(|e| AppError(e.into()))?;
        if canon_dest.starts_with(&canon_src) {
            return Err(AppError(anyhow::anyhow!(
                "cannot move a directory into itself"
            )));
        }
    }

    // Open source child DB and compute relative paths — before the move.
    let src_parent = abs_src
        .parent()
        .ok_or_else(|| AppError(anyhow::anyhow!("source has no parent")))?;
    let (src_conn, src_eff_root) = db::find_and_open_fast(src_parent).map_err(AppError)?;
    let old_rel = db::relative_to_root(&abs_src, &src_eff_root).map_err(AppError)?;
    if old_rel == ".filetag" || old_rel.starts_with(".filetag/") {
        return Err(AppError(anyhow::anyhow!(
            "cannot operate on .filetag directory"
        )));
    }

    // Open destination child DB.
    let (dest_conn, dest_eff_root) = db::find_and_open_fast(&dest_dir).map_err(AppError)?;
    let canon_dest_dir = dest_dir.canonicalize().map_err(|e| AppError(e.into()))?;
    let name_str = name.to_string_lossy().into_owned();
    let new_rel = rel_under_root(&canon_dest_dir, &name_str, &dest_eff_root);

    let same_db = src_eff_root == dest_eff_root;

    // For cross-root directory moves, snapshot tags before the filesystem op.
    let dir_snapshot: Vec<db::FileWithTags> = if !same_db && is_src_dir {
        db::files_under_prefix(&src_conn, &old_rel).unwrap_or_default()
    } else {
        Vec::new()
    };

    // For cross-root file moves, snapshot tags before the filesystem op.
    let file_tags: Vec<(String, Option<String>, String)> = if !same_db && !is_src_dir {
        if let Ok(Some(fid)) = db::file_id_by_path(&src_conn, &old_rel) {
            db::tags_for_file_with_subject(&src_conn, fid).unwrap_or_default()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    // Perform the filesystem move.  Fall back to copy+delete for files when
    // rename fails (e.g. cross-device).
    let move_err = std::fs::rename(&abs_src, &abs_dest).err();
    if let Some(e) = move_err {
        if !is_src_dir {
            // Cross-device or other rename failure: copy then delete.
            std::fs::copy(&abs_src, &abs_dest)
                .context("cross-device move (copy phase) failed")
                .map_err(AppError)?;
            std::fs::remove_file(&abs_src)
                .context("cross-device move (delete phase) failed")
                .map_err(AppError)?;
        } else {
            return Err(AppError(
                anyhow::Error::new(e).context("directory move failed"),
            ));
        }
    }

    // Update database records.
    if same_db {
        // Same child DB: update paths in place.
        if is_src_dir {
            let _ = db::rename_dir_paths(&src_conn, &old_rel, &new_rel);
        } else {
            let _ = db::rename_file_path(&src_conn, &old_rel, &new_rel);
        }
    } else if is_src_dir {
        // Cross-root directory: delete old records and re-index with tags.
        let _ = db::delete_dir_paths(&src_conn, &old_rel);
        let old_prefix = format!("{}/", old_rel);
        for fwt in &dir_snapshot {
            // Translate old rel_path to new rel_path.
            let suffix = fwt
                .rel_path
                .strip_prefix(&old_prefix)
                .unwrap_or(&fwt.rel_path);
            let dest_file_rel = format!("{}/{}", new_rel, suffix);
            let _ = db::get_or_index_file(&dest_conn, &dest_file_rel, &dest_eff_root);
            if let Ok(Some(dest_fid)) = db::file_id_by_path(&dest_conn, &dest_file_rel) {
                for (tag_name, value, subject) in &fwt.tags {
                    if let Ok(tag_id) = db::get_or_create_tag(&dest_conn, tag_name) {
                        let _ = db::apply_tag(
                            &dest_conn,
                            dest_fid,
                            tag_id,
                            if value.is_empty() {
                                None
                            } else {
                                Some(value.as_str())
                            },
                            if subject.is_empty() {
                                None
                            } else {
                                Some(subject.as_str())
                            },
                        );
                    }
                }
            }
        }
    } else {
        // Cross-root file: delete from src, index + apply tags in dest.
        let _ = db::delete_file_by_path(&src_conn, &old_rel);
        let _ = db::get_or_index_file(&dest_conn, &new_rel, &dest_eff_root);
        if let Ok(Some(dest_fid)) = db::file_id_by_path(&dest_conn, &new_rel) {
            for (tag_name, value, subject) in &file_tags {
                if let Ok(tag_id) = db::get_or_create_tag(&dest_conn, tag_name) {
                    let _ = db::apply_tag(
                        &dest_conn,
                        dest_fid,
                        tag_id,
                        value.as_deref(),
                        if subject.is_empty() {
                            None
                        } else {
                            Some(subject.as_str())
                        },
                    );
                }
            }
        }
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/fs/delete — delete a file or directory.
pub async fn api_fs_delete(
    State(state): State<Arc<AppState>>,
    Json(body): Json<FsDeleteRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let abs_path = PathBuf::from(&body.path);
    if !abs_path.exists() {
        return Err(AppError(anyhow::anyhow!("path does not exist")));
    }

    // Ensure the path is inside a known root.
    root_for_dir(&state, &abs_path).ok_or_else(|| {
        AppError(anyhow::anyhow!(
            "path is not within any known database root"
        ))
    })?;

    let parent = abs_path
        .parent()
        .ok_or_else(|| AppError(anyhow::anyhow!("path has no parent")))?;
    let (conn, eff_root) = db::find_and_open_fast(parent).map_err(AppError)?;
    let rel = db::relative_to_root(&abs_path, &eff_root).map_err(AppError)?;
    if rel == ".filetag" || rel.starts_with(".filetag/") {
        return Err(AppError(anyhow::anyhow!(
            "cannot delete .filetag directory"
        )));
    }

    let is_dir = abs_path.is_dir();
    if is_dir {
        std::fs::remove_dir_all(&abs_path)
            .context("delete failed")
            .map_err(AppError)?;
        let _ = db::delete_dir_paths(&conn, &rel);
    } else {
        std::fs::remove_file(&abs_path)
            .context("delete failed")
            .map_err(AppError)?;
        let _ = db::delete_file_by_path(&conn, &rel);
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/fs/copy — copy a file (tags are copied to the destination).
pub async fn api_fs_copy(
    State(state): State<Arc<AppState>>,
    Json(body): Json<FsCopyRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let abs_src = PathBuf::from(&body.path);
    if !abs_src.is_file() {
        return Err(AppError(anyhow::anyhow!("source must be an existing file")));
    }

    let src_parent = abs_src
        .parent()
        .ok_or_else(|| AppError(anyhow::anyhow!("source has no parent")))?;
    root_for_dir(&state, &abs_src).ok_or_else(|| {
        AppError(anyhow::anyhow!(
            "source is not within any known database root"
        ))
    })?;

    let dest_dir_path = body
        .dest_dir
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| src_parent.to_path_buf());
    if !dest_dir_path.is_dir() {
        return Err(AppError(anyhow::anyhow!(
            "destination directory does not exist"
        )));
    }

    // Destination must be within a known root (cross-root copies are allowed).
    root_for_dir(&state, &dest_dir_path).ok_or_else(|| {
        AppError(anyhow::anyhow!(
            "destination is not within any known database root"
        ))
    })?;

    // Determine destination filename.
    let dest_name: String = if let Some(ref n) = body.new_name {
        validate_filename(n).map_err(AppError)?;
        n.clone()
    } else {
        let stem = abs_src
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ext = abs_src
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        format!("Copy of {}{}", stem, ext)
    };

    let abs_dest = dest_dir_path.join(&dest_name);
    if abs_dest.exists() {
        return Err(AppError(anyhow::anyhow!(
            "destination file {:?} already exists",
            dest_name
        )));
    }

    // Copy the file.
    std::fs::copy(&abs_src, &abs_dest)
        .context("copy failed")
        .map_err(AppError)?;

    // Copy tags from source to the new file.
    let (src_conn, src_eff_root) = db::find_and_open_fast(src_parent).map_err(AppError)?;
    let src_rel = db::relative_to_root(&abs_src, &src_eff_root).map_err(AppError)?;
    let canon_dest_dir = dest_dir_path
        .canonicalize()
        .map_err(|e| AppError(e.into()))?;

    // Open destination DB (may differ from source DB for cross-root copies).
    let (dest_conn, dest_eff_root) = db::find_and_open_fast(&dest_dir_path).map_err(AppError)?;
    let dest_rel = rel_under_root(&canon_dest_dir, &dest_name, &dest_eff_root);

    if let Ok(Some(src_file_id)) = db::file_id_by_path(&src_conn, &src_rel)
        && let Ok(tags) = db::tags_for_file_with_subject(&src_conn, src_file_id)
        && !tags.is_empty()
    {
        let _ = db::get_or_index_file(&dest_conn, &dest_rel, &dest_eff_root);
        if let Ok(Some(dest_file_id)) = db::file_id_by_path(&dest_conn, &dest_rel) {
            for (tag_name, value, subject) in tags {
                if let Ok(tag_id) = db::get_or_create_tag(&dest_conn, &tag_name) {
                    let _ = db::apply_tag(
                        &dest_conn,
                        dest_file_id,
                        tag_id,
                        value.as_deref(),
                        if subject.is_empty() {
                            None
                        } else {
                            Some(subject.as_str())
                        },
                    );
                }
            }
        }
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}
