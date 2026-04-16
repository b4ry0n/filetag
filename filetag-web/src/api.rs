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
use crate::preview::{file_cache_path, raw_cache_path, thumb_cache_path, video_info};
use crate::state::{
    AppError, AppState, DbRoot, file_is_covered, open_conn, open_for_file_op, parse_tag,
    resolve_preview, root_at, safe_path,
};
use crate::types::*;

// ---------------------------------------------------------------------------
// Static file handlers (embedded)
// ---------------------------------------------------------------------------

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

js_handler!(js_utils, "../static/js/utils.js");
js_handler!(js_state, "../static/js/state.js");
js_handler!(js_tags, "../static/js/tags.js");
js_handler!(js_render, "../static/js/render.js");
js_handler!(js_detail, "../static/js/detail.js");
js_handler!(js_actions, "../static/js/actions.js");
js_handler!(js_lightbox, "../static/js/lightbox.js");
js_handler!(js_viewer, "../static/js/viewer.js");
js_handler!(js_main, "../static/js/main.js");

pub async fn favicon() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "image/svg+xml")],
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><text y=".9em" font-size="90">🏷</text></svg>"#,
    )
}

// ---------------------------------------------------------------------------
// Roots
// ---------------------------------------------------------------------------

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

pub async fn api_reorder_roots(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ReorderRootsRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    for (position, &root_id) in body.order.iter().enumerate() {
        let db_root = root_at(&state, Some(root_id))?;
        let conn = open_conn(db_root)?;
        db::set_setting(&conn, "sort_order", &position.to_string())?;
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn api_rename_db(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RenameDbRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_at(&state, Some(body.root_id))?;
    let conn = open_conn(db_root)?;
    db::set_setting(&conn, "name", &body.name)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Info
// ---------------------------------------------------------------------------

pub async fn api_info(
    State(state): State<Arc<AppState>>,
    Query(rp): Query<RootParam>,
) -> Result<Json<ApiInfo>, AppError> {
    let db_root = root_at(&state, rp.root)?;
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

    // thumbs, raw preview
    for p in [thumb_cache_path(abs, root), raw_cache_path(abs, root)]
        .into_iter()
        .flatten()
    {
        if std::fs::remove_file(&p).is_ok() {
            removed += 1;
        }
    }

    // transcoded video (v4.mp4)
    if let Some(p) = file_cache_path(abs, root, "video", "v4.mp4")
        && std::fs::remove_file(&p).is_ok()
    {
        removed += 1;
    }

    // Build the cache key prefix "{mtime}_{size}_{stem}" used by all file_cache_path entries.
    let prefix: Option<String> = (|| {
        let meta = std::fs::metadata(abs).ok()?;
        let mtime = meta
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs();
        let size = meta.len();
        let stem = abs.file_name()?.to_string_lossy().into_owned();
        Some(format!("{mtime}_{size}_{stem}"))
    })();

    let cache_dir = root.join(".filetag").join("cache");

    if let Some(ref pfx) = prefix {
        // vthumbs: sprite files named "{prefix}.sprite{n}x1.jpg" for various n values.
        // Scan the subdir for files that start with the key prefix so we catch all n values.
        for subdir in &["vthumbs"] {
            let dir = cache_dir.join(subdir);
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for entry in rd.flatten() {
                    let name = entry.file_name();
                    if name.to_string_lossy().starts_with(pfx.as_str())
                        && std::fs::remove_file(entry.path()).is_ok()
                    {
                        removed += 1;
                    }
                }
            }
        }

        // hls: the segment directory is named "{prefix}" inside the hls2 subdir.
        let hls_dir = cache_dir.join("hls2").join(pfx);
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

        // HEIC preview: named "heic_{stem}_{mtime}.jpg" (legacy flat layout, no size).
        let stem = abs
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mtime = pfx.split('_').next().unwrap_or("0");
        let heic_path = cache_dir.join(format!("heic_{}_{}.jpg", stem, mtime));
        if std::fs::remove_file(&heic_path).is_ok() {
            removed += 1;
        }
    }

    removed
}

pub async fn api_cache_clear(
    State(state): State<Arc<AppState>>,
    Query(rp): Query<RootParam>,
    body: Option<axum::extract::Json<CacheClearBody>>,
) -> Response {
    let db_root = match root_at(&state, rp.root) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "Unknown root").into_response(),
    };
    let root = db_root.root.clone();
    let paths = body.and_then(|b| b.paths.clone());

    let removed = if let Some(rel_paths) = paths {
        let mut n = 0u64;
        for rel in rel_paths {
            if let Some((abs, cr)) = resolve_preview(&state, &db_root.root, &rel) {
                n += remove_cache_for_path(&abs, &cr);
            }
        }
        n
    } else {
        // Drop the entire cache directory — the simplest and most complete approach.
        let cache_dir = root.join(".filetag").join("cache");
        let existed = cache_dir.exists();
        let _ = tokio::fs::remove_dir_all(&cache_dir).await;
        if existed { 1 } else { 0 }
    };

    (
        StatusCode::OK,
        Json(serde_json::json!({ "removed": removed })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Tags list
// ---------------------------------------------------------------------------

pub async fn api_tags(
    State(state): State<Arc<AppState>>,
    Query(rp): Query<RootParam>,
) -> Result<Json<Vec<ApiTag>>, AppError> {
    let db_root = root_at(&state, rp.root)?;
    let conn = open_conn(db_root)?;
    let tags = db::all_tags(&conn).map_err(AppError)?;
    Ok(Json(
        tags.into_iter()
            .map(|(name, count, color)| ApiTag { name, count, color })
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// File listing
// ---------------------------------------------------------------------------

pub async fn api_files(
    State(state): State<Arc<AppState>>,
    Query(params): Query<FileListParams>,
) -> Result<Json<ApiDirListing>, AppError> {
    // Virtual root: only when there are multiple entry-point roots and no root
    // has been explicitly selected yet.
    let entry_point_roots: Vec<(usize, &DbRoot)> = state
        .roots
        .iter()
        .enumerate()
        .filter(|(_, r)| r.entry_point)
        .collect();
    if params.root.is_none() && params.path.is_empty() && entry_point_roots.len() > 1 {
        let mut ordered: Vec<(usize, &DbRoot, i64)> = entry_point_roots
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
                root_id: Some(id),
                covered: None,
            })
            .collect();
        return Ok(Json(ApiDirListing {
            path: String::new(),
            entries,
        }));
    }

    let db_root = root_at(&state, params.root)?;
    let dir = if params.path.is_empty() {
        db_root.root.clone()
    } else {
        safe_path(&db_root.root, &params.path)?
    };

    let prefix = if params.path.is_empty() {
        String::new()
    } else {
        format!("{}/", params.path.trim_end_matches('/'))
    };

    let conn = open_conn(db_root)?;
    let mut tag_stmt = conn.prepare_cached(
        "SELECT COUNT(*) FROM file_tags ft \
         JOIN files f ON f.id = ft.file_id WHERE f.path = ?1",
    )?;

    let mut dirs = Vec::new();
    let mut files = Vec::new();

    let rd =
        std::fs::read_dir(&dir).with_context(|| format!("reading directory {}", dir.display()))?;

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
            dirs.push(ApiDirEntry {
                name,
                is_dir: true,
                size: None,
                mtime: None,
                file_count: Some(child_count),
                tag_count: None,
                root_id: None,
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
                root_id: None,
                covered: Some(file_is_covered(&state, &meta, &entry.path())),
            });
        }
    }

    dirs.sort_by_key(|a| a.name.to_lowercase());
    files.sort_by_key(|a| a.name.to_lowercase());
    dirs.extend(files);

    Ok(Json(ApiDirListing {
        path: params.path,
        entries: dirs,
    }))
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

pub async fn api_search(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchParams>,
) -> Result<Json<ApiSearchResult>, AppError> {
    let db_root = root_at(&state, params.root)?;
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
                    .map(|(name, value)| ApiFileTag { name, value })
                    .collect(),
            })
            .collect(),
    }))
}

// ---------------------------------------------------------------------------
// File detail
// ---------------------------------------------------------------------------

pub async fn api_file_detail(
    State(state): State<Arc<AppState>>,
    Query(params): Query<FileDetailParams>,
) -> Result<Json<ApiFileDetail>, AppError> {
    let db_root = root_at(&state, params.root)?;

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
        video_info(&fs_path).await.map(|i| i.duration)
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
        let tags = db::tags_for_file(&conn, record.id).map_err(AppError)?;
        let indexed_at: String = conn.query_row(
            "SELECT indexed_at FROM files WHERE id = ?1",
            rusqlite::params![record.id],
            |r| r.get(0),
        )?;

        return Ok(Json(ApiFileDetail {
            path: params.path,
            size: record.size,
            file_id: record.file_id,
            mtime: record.mtime_ns,
            indexed_at,
            covered: true,
            tags: tags
                .into_iter()
                .map(|(name, value)| ApiFileTag { name, value })
                .collect(),
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
        covered: file_is_covered(&state, &meta, &fs_path),
        tags: vec![],
        duration,
    }))
}

// ---------------------------------------------------------------------------
// Tag / Untag (now using open_for_file_op)
// ---------------------------------------------------------------------------

pub async fn api_tag(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TagRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_at(&state, body.root_id)?;
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
        db::apply_tag(&conn, file_id, tag_id, value.as_deref()).map_err(AppError)?;
        added += 1;
    }

    Ok(Json(serde_json::json!({ "added": added })))
}

pub async fn api_untag(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TagRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_at(&state, body.root_id)?;
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
        ) && db::remove_tag(&conn, record.id, tag_id, value.as_deref()).map_err(AppError)?
        {
            removed += 1;
        }
    }

    Ok(Json(serde_json::json!({ "removed": removed })))
}

// ---------------------------------------------------------------------------
// Tag color + delete
// ---------------------------------------------------------------------------

pub async fn api_rename_tag(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RenameTagRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_at(&state, body.root_id)?;
    let conn = open_conn(db_root)?;
    let ok = db::rename_tag(&conn, &body.name, &body.new_name).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "ok": ok })))
}

pub async fn api_tag_color(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TagColorRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_at(&state, body.root_id)?;
    let conn = open_conn(db_root)?;
    let ok = db::set_tag_color(&conn, &body.name, body.color.as_deref()).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "ok": ok })))
}

pub async fn api_delete_tag(
    State(state): State<Arc<AppState>>,
    Json(body): Json<DeleteTagRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db_root = root_at(&state, body.root_id)?;
    let conn = open_conn(db_root)?;
    let deleted = db::delete_tag(&conn, &body.name).map_err(AppError)?;
    Ok(Json(serde_json::json!({ "deleted": deleted })))
}
