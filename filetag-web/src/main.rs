use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use clap::Parser;
use filetag_lib::{db, query};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tower_http::services::ServeDir;

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "filetag-web", about = "Web interface for filetag", version)]
struct Args {
    /// Database root directory (default: current directory)
    path: Option<PathBuf>,

    /// Port to listen on
    #[arg(short, long, default_value_t = 3000)]
    port: u16,

    /// Address to bind to
    #[arg(short, long, default_value = "127.0.0.1")]
    bind: String,
}

// ---------------------------------------------------------------------------
// State and error handling
// ---------------------------------------------------------------------------

struct AppState {
    db_path: PathBuf,
    root: PathBuf,
}

struct AppError(anyhow::Error);

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

fn open_conn(state: &AppState) -> anyhow::Result<Connection> {
    let conn =
        Connection::open(&state.db_path).context("opening database")?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )?;
    Ok(conn)
}

/// Resolve a relative path under `root`, rejecting directory traversal.
fn safe_path(root: &Path, rel: &str) -> anyhow::Result<PathBuf> {
    let joined = root.join(rel);
    let canonical = std::fs::canonicalize(&joined)
        .with_context(|| format!("resolving {}", joined.display()))?;
    anyhow::ensure!(canonical.starts_with(root), "path escapes database root");
    Ok(canonical)
}

// ---------------------------------------------------------------------------
// API types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ApiInfo {
    root: String,
    files: i64,
    tags: i64,
    assignments: i64,
    total_size: i64,
}

#[derive(Serialize)]
struct ApiTag {
    name: String,
    count: i64,
}

#[derive(Serialize)]
struct ApiDirListing {
    path: String,
    entries: Vec<ApiDirEntry>,
}

#[derive(Serialize)]
struct ApiDirEntry {
    name: String,
    is_dir: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mtime: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tag_count: Option<i64>,
}

#[derive(Serialize)]
struct ApiFileDetail {
    path: String,
    size: i64,
    blake3: Option<String>,
    mtime: i64,
    indexed_at: String,
    tags: Vec<ApiFileTag>,
}

#[derive(Serialize)]
struct ApiFileTag {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<String>,
}

#[derive(Serialize)]
struct ApiSearchResult {
    query: String,
    results: Vec<ApiSearchEntry>,
}

#[derive(Serialize)]
struct ApiSearchEntry {
    path: String,
    tags: Vec<ApiFileTag>,
}

#[derive(Deserialize)]
struct FileListParams {
    #[serde(default)]
    path: String,
}

#[derive(Deserialize)]
struct SearchParams {
    q: String,
}

#[derive(Deserialize)]
struct FileDetailParams {
    path: String,
}

#[derive(Deserialize)]
struct TagRequest {
    path: String,
    tags: Vec<String>,
}

// ---------------------------------------------------------------------------
// Static files (embedded)
// ---------------------------------------------------------------------------

async fn index_html() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_str!("../static/index.html"),
    )
}

async fn style_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../static/style.css"),
    )
}

async fn app_js() -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        include_str!("../static/app.js"),
    )
}

async fn favicon() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "image/svg+xml")],
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><text y=".9em" font-size="90">🏷</text></svg>"#,
    )
}

// ---------------------------------------------------------------------------
// API handlers
// ---------------------------------------------------------------------------

async fn api_info(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiInfo>, AppError> {
    let conn = open_conn(&state)?;
    let files: i64 =
        conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
    let tags: i64 =
        conn.query_row("SELECT COUNT(*) FROM tags", [], |r| r.get(0))?;
    let assignments: i64 =
        conn.query_row("SELECT COUNT(*) FROM file_tags", [], |r| r.get(0))?;
    let total_size: i64 = conn.query_row(
        "SELECT COALESCE(SUM(size), 0) FROM files",
        [],
        |r| r.get(0),
    )?;

    Ok(Json(ApiInfo {
        root: state.root.display().to_string(),
        files,
        tags,
        assignments,
        total_size,
    }))
}

async fn api_tags(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<ApiTag>>, AppError> {
    let conn = open_conn(&state)?;
    let tags = db::all_tags(&conn).map_err(AppError)?;
    Ok(Json(
        tags.into_iter()
            .map(|(name, count)| ApiTag { name, count })
            .collect(),
    ))
}

async fn api_files(
    State(state): State<Arc<AppState>>,
    Query(params): Query<FileListParams>,
) -> Result<Json<ApiDirListing>, AppError> {
    let dir = if params.path.is_empty() {
        state.root.clone()
    } else {
        safe_path(&state.root, &params.path)?
    };

    let prefix = if params.path.is_empty() {
        String::new()
    } else {
        format!("{}/", params.path.trim_end_matches('/'))
    };

    let conn = open_conn(&state)?;
    let mut tag_stmt = conn.prepare_cached(
        "SELECT COUNT(*) FROM file_tags ft \
         JOIN files f ON f.id = ft.file_id WHERE f.path = ?1",
    )?;

    let mut dirs = Vec::new();
    let mut files = Vec::new();

    let rd = std::fs::read_dir(&dir)
        .with_context(|| format!("reading directory {}", dir.display()))?;

    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".filetag" {
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
            });
        }
    }

    dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    dirs.extend(files);

    Ok(Json(ApiDirListing {
        path: params.path,
        entries: dirs,
    }))
}

async fn api_search(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchParams>,
) -> Result<Json<ApiSearchResult>, AppError> {
    let conn = open_conn(&state)?;
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

async fn api_file_detail(
    State(state): State<Arc<AppState>>,
    Query(params): Query<FileDetailParams>,
) -> Result<Json<ApiFileDetail>, AppError> {
    let conn = open_conn(&state)?;

    if let Some(record) = db::file_by_path(&conn, &params.path).map_err(AppError)? {
        let tags = db::tags_for_file(&conn, record.id).map_err(AppError)?;
        let indexed_at: String = conn.query_row(
            "SELECT indexed_at FROM files WHERE id = ?1",
            rusqlite::params![record.id],
            |r| r.get(0),
        )?;

        Ok(Json(ApiFileDetail {
            path: params.path,
            size: record.size,
            blake3: record.blake3,
            mtime: record.mtime_ns,
            indexed_at,
            tags: tags
                .into_iter()
                .map(|(name, value)| ApiFileTag { name, value })
                .collect(),
        }))
    } else {
        // File not yet indexed: return filesystem metadata
        let abs = safe_path(&state.root, &params.path)?;
        let meta = std::fs::metadata(&abs)
            .with_context(|| format!("reading {}", abs.display()))?;
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
            blake3: None,
            mtime,
            indexed_at: String::new(),
            tags: vec![],
        }))
    }
}

async fn api_tag(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TagRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    safe_path(&state.root, &body.path)?;
    let conn = open_conn(&state)?;
    // Auto-index the file if not yet in the database
    let record =
        db::get_or_index_file(&conn, &body.path, &state.root).map_err(AppError)?;

    let mut added = 0i64;
    for tag_str in &body.tags {
        let (name, value) = parse_tag(tag_str);
        let tag_id = db::get_or_create_tag(&conn, &name).map_err(AppError)?;
        db::apply_tag(&conn, record.id, tag_id, value.as_deref()).map_err(AppError)?;
        added += 1;
    }

    Ok(Json(serde_json::json!({ "added": added })))
}

async fn api_untag(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TagRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let conn = open_conn(&state)?;
    let record = db::file_by_path(&conn, &body.path)
        .map_err(AppError)?
        .ok_or_else(|| AppError(anyhow::anyhow!("file not found: {}", body.path)))?;

    let mut removed = 0i64;
    for tag_str in &body.tags {
        let (name, value) = parse_tag(tag_str);
        if let Ok(tag_id) = conn.query_row(
            "SELECT id FROM tags WHERE name = ?1",
            rusqlite::params![&name],
            |r| r.get::<_, i64>(0),
        )
            && db::remove_tag(&conn, record.id, tag_id, value.as_deref()).map_err(AppError)?
        {
            removed += 1;
        }
    }

    Ok(Json(serde_json::json!({ "removed": removed })))
}

/// Parse "genre/rock" -> ("genre/rock", None), "year=2024" -> ("year", Some("2024"))
fn parse_tag(s: &str) -> (String, Option<String>) {
    if let Some(eq) = s.find('=') {
        (s[..eq].to_string(), Some(s[eq + 1..].to_string()))
    } else {
        (s.to_string(), None)
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let root = args.path.unwrap_or_else(|| ".".into());
    let root = std::fs::canonicalize(&root)
        .with_context(|| format!("resolving {}", root.display()))?;

    // Verify database exists
    let (conn, root) = db::find_and_open(&root)?;
    drop(conn);

    let db_path = root.join(".filetag").join("db.sqlite3");
    let state = Arc::new(AppState {
        db_path,
        root: root.clone(),
    });

    let app = Router::new()
        .route("/", get(index_html))
        .route("/style.css", get(style_css))
        .route("/app.js", get(app_js))
        .route("/favicon.svg", get(favicon))
        .route("/api/info", get(api_info))
        .route("/api/tags", get(api_tags))
        .route("/api/files", get(api_files))
        .route("/api/search", get(api_search))
        .route("/api/file", get(api_file_detail))
        .route("/api/tag", post(api_tag))
        .route("/api/untag", post(api_untag))
        .nest_service("/preview", ServeDir::new(&root))
        .with_state(state);

    let addr = format!("{}:{}", args.bind, args.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding to {}", addr))?;

    println!("filetag-web serving {} at http://{}", root.display(), addr);
    axum::serve(listener, app).await?;

    Ok(())
}
