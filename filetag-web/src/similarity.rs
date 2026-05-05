//! Visual and semantic similarity search.
//!
//! Two methods are supported:
//!
//! * **pHash** (dHash): a 64-bit perceptual fingerprint computed from a
//!   9×8 greyscale thumbnail.  Hamming distance between hashes is a fast
//!   proxy for visual similarity and works well for near-duplicates and
//!   lightly edited variants.  CPU-only, no external service required.
//!
//! * **Text embeddings**: a float vector obtained by sending the file's tag
//!   list to an OpenAI-compatible `/v1/embeddings` endpoint.  Cosine
//!   similarity between vectors captures semantic relatedness (e.g. two
//!   unrelated photos of sunsets both tagged `golden-hour` will score
//!   highly, even if their pixel content differs).  Requires `ai.embed_model`
//!   to be configured.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use axum::{
    Json,
    extract::{Query, State},
};
use image::imageops;
use serde::Deserialize;

use crate::state::{AppError, AppState, root_for_dir};
use filetag_lib::db;

// ---------------------------------------------------------------------------
// dHash
// ---------------------------------------------------------------------------

/// Compute a 64-bit difference hash of `img`.
///
/// The image is resized to 9×8 pixels in greyscale; for each of the 8 rows
/// each pixel is compared with its right neighbour.  The resulting 64 bits
/// are packed into a `u64`.  Hamming distance between two hashes correlates
/// strongly with perceptual similarity.
pub fn dhash(img: &image::DynamicImage) -> u64 {
    let small = imageops::resize(&img.to_luma8(), 9, 8, imageops::FilterType::Triangle);
    let mut hash: u64 = 0;
    for y in 0..8u32 {
        for x in 0..8u32 {
            if small.get_pixel(x, y)[0] < small.get_pixel(x + 1, y)[0] {
                hash |= 1u64 << (y * 8 + x);
            }
        }
    }
    hash
}

/// Number of bits that differ between `a` and `b`.
#[inline]
pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// Normalised similarity in `[0.0, 1.0]` derived from Hamming distance.
/// 1.0 = identical; ≥ 0.83 ≈ "very similar" (≤ 10 bits different).
#[inline]
pub fn phash_similarity(a: u64, b: u64) -> f32 {
    1.0 - (hamming(a, b) as f32 / 64.0)
}

// ---------------------------------------------------------------------------
// Cosine similarity for embeddings
// ---------------------------------------------------------------------------

/// Cosine similarity between two equal-length float slices, clamped to `[0, 1]`.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    (dot / (na * nb)).clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

const IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "tiff", "tif", "bmp", "gif", "ico",
];

/// Open a SQLite connection with standard pragmas, given a path to the `.db` file.
///
/// IMPORTANT: The returned `Connection` is `!Send`.  Never hold it across an
/// `await` point — drop it before any `.await` to keep the enclosing future `Send`.
fn open_db(db_path: &Path) -> Result<rusqlite::Connection, AppError> {
    let conn = rusqlite::Connection::open(db_path).map_err(|e| AppError(e.into()))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;",
    )
    .map_err(|e| AppError(e.into()))?;
    Ok(conn)
}

// ---------------------------------------------------------------------------
// Embedding API call
// ---------------------------------------------------------------------------

async fn call_embedding_api(
    endpoint: &str,
    model: &str,
    api_key: Option<&str>,
    text: &str,
) -> Result<Vec<f32>> {
    let url = format!("{}/v1/embeddings", endpoint.trim_end_matches('/'));

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()?;

    let mut req = client.post(&url).json(&serde_json::json!({
        "model": model,
        "input": text,
    }));

    if let Some(key) = api_key
        && !key.is_empty()
    {
        req = req.header("Authorization", format!("Bearer {key}"));
    }

    let resp = req.send().await?.error_for_status()?;
    let body: serde_json::Value = resp.json().await?;

    let embedding: Vec<f32> = body["data"][0]["embedding"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("invalid embedding response shape"))?
        .iter()
        .filter_map(|v| v.as_f64().map(|f| f as f32))
        .collect();

    if embedding.is_empty() {
        anyhow::bail!("empty embedding returned from API");
    }
    Ok(embedding)
}

// ---------------------------------------------------------------------------
// GET /api/similar
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct SimilarQuery {
    /// Absolute filesystem path of the file to find neighbours for.
    pub path: String,
    /// Absolute path of any file in the current directory (used to find the
    /// database root when `path` is outside the indexed tree).
    pub dir: Option<String>,
    /// `"phash"` (default) or `"embedding"`.
    pub method: Option<String>,
    /// Number of results to return (default 20, max 100).
    pub n: Option<usize>,
}

pub async fn api_similar(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SimilarQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let dir_path = q.dir.as_deref().unwrap_or(&q.path);

    // Extract owned values from the borrowed root — the borrow must not cross
    // any `.await` point because `rusqlite::Connection` is `!Send`.
    let (db_path, root_path, rel_path) = {
        let root = root_for_dir(&state, Path::new(dir_path))
            .ok_or_else(|| anyhow::anyhow!("No database root found for this path"))?;
        let abs = PathBuf::from(&q.path);
        let rel = db::relative_to_root(&abs, &root.root)?;
        (root.db_path.clone(), root.root.clone(), rel)
    };

    let n = q.n.unwrap_or(20).min(100);
    let abs_path = PathBuf::from(&q.path);
    let method = q.method.as_deref().unwrap_or("phash").to_owned();

    match method.as_str() {
        "embedding" => similar_by_embedding(&db_path, &rel_path, n).await,
        _ => similar_by_phash(&db_path, &root_path, &abs_path, &rel_path, n).await,
    }
}

/// Find similar files by perceptual hash.
///
/// All DB access is scoped to short synchronous blocks so that no
/// `rusqlite::Connection` is held across any `.await` suspension point.
async fn similar_by_phash(
    db_path: &Path,
    root_path: &Path,
    abs_path: &Path,
    rel_path: &str,
    n: usize,
) -> Result<Json<serde_json::Value>, AppError> {
    // 1. Look up an existing hash — connection dropped before any await.
    let existing: Option<u64> = {
        let conn = open_db(db_path)?;
        db::get_phash_by_path(&conn, rel_path)?.map(|(_, h)| h)
        // conn dropped here
    };

    let query_hash: u64 = match existing {
        Some(h) => h,
        None => {
            // Compute hash asynchronously — no DB conn held here.
            let ext = abs_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            if !IMAGE_EXTS.contains(&ext.as_str()) {
                return Ok(Json(serde_json::json!({
                    "method": "phash",
                    "error": format!("unsupported file type: {ext}"),
                    "results": [],
                })));
            }

            let p = abs_path.to_path_buf();
            let img = match tokio::task::spawn_blocking(move || image::open(&p)).await {
                Ok(Ok(img)) => img,
                Ok(Err(e)) => {
                    return Ok(Json(serde_json::json!({
                        "method": "phash",
                        "error": e.to_string(),
                        "results": [],
                    })));
                }
                Err(e) => {
                    return Ok(Json(serde_json::json!({
                        "method": "phash",
                        "error": e.to_string(),
                        "results": [],
                    })));
                }
            };

            let hash = dhash(&img);

            // Store hash — new connection opened after the await.
            {
                let conn = open_db(db_path)?;
                if let Ok(Some(file_id)) = db::file_id_by_path(&conn, rel_path) {
                    let _ = db::store_phash(&conn, file_id, hash);
                }
                // conn dropped here
            }

            hash
        }
    };

    // 2. Collect all stored hashes — connection dropped immediately after.
    let all_hashes: Vec<(i64, String, u64)> = {
        let conn = open_db(db_path)?;
        db::all_phashes(&conn)?
        // conn dropped here
    };

    let mut results: Vec<(f32, String)> = all_hashes
        .into_iter()
        .filter(|(_, path, _)| path != rel_path)
        .map(|(_, path, hash)| (phash_similarity(query_hash, hash), path))
        .filter(|(score, _)| *score >= 0.75)
        .collect();

    results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    results.truncate(n);

    let items: Vec<_> = results
        .iter()
        .map(|(score, path)| {
            serde_json::json!({
                "path": path,
                "abs_path": root_path.join(path).to_string_lossy(),
                "score": score,
                "method": "phash",
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "method": "phash",
        "query_hash": format!("{:016x}", query_hash),
        "results": items,
    })))
}

/// Find similar files by tag-list embedding (cosine similarity).
///
/// No async work is done here; the function is `async` only to satisfy the
/// call site's pattern.  All DB access is synchronous and dropped before any
/// await point.
async fn similar_by_embedding(
    db_path: &Path,
    rel_path: &str,
    n: usize,
) -> Result<Json<serde_json::Value>, AppError> {
    let (file_id, query_vec): (i64, Vec<f32>) = {
        let conn = open_db(db_path)?;
        let fid = db::file_id_by_path(&conn, rel_path)?.ok_or_else(|| {
            AppError(anyhow::anyhow!(
                "file not in database — add a tag first to index it"
            ))
        })?;
        let (_, vec) = db::get_embedding(&conn, fid)?.ok_or_else(|| {
            AppError(anyhow::anyhow!(
                "no embedding stored — run 'Index embeddings' first"
            ))
        })?;
        (fid, vec)
        // conn dropped here
    };

    let all_embeddings: Vec<(i64, String, Vec<f32>)> = {
        let conn = open_db(db_path)?;
        db::all_embeddings(&conn)?
        // conn dropped here
    };

    let mut results: Vec<(f32, String)> = all_embeddings
        .into_iter()
        .filter(|(fid, _, _)| *fid != file_id)
        .map(|(_, path, emb)| (cosine_similarity(&query_vec, &emb), path))
        .filter(|(score, _)| *score >= 0.4)
        .collect();

    results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    results.truncate(n);

    let items: Vec<_> = results
        .iter()
        .map(|(score, path)| {
            serde_json::json!({
                "path": path,
                "score": score,
                "method": "embedding",
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "method": "embedding",
        "results": items,
    })))
}

// ---------------------------------------------------------------------------
// POST /api/similar/index-phash
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct IndexRequest {
    pub dir: String,
    /// Re-compute even when a hash already exists.
    pub force: Option<bool>,
}

pub async fn api_index_phash(
    State(state): State<Arc<AppState>>,
    Json(req): Json<IndexRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (db_path, root_path) = {
        let root = root_for_dir(&state, Path::new(&req.dir))
            .ok_or_else(|| anyhow::anyhow!("No database root found"))?;
        (root.db_path.clone(), root.root.clone())
        // root borrow dropped
    };
    let force = req.force.unwrap_or(false);

    // Collect files that need hashing — connection dropped before any await.
    let files: Vec<(i64, String)> = {
        let conn = open_db(&db_path)?;
        let query = if force {
            "SELECT id, path FROM files"
        } else {
            "SELECT id, path FROM files WHERE phash IS NULL"
        };
        let mut stmt = conn.prepare(query)?;
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect()
        // conn dropped here
    };

    let total = files.len();
    let mut indexed = 0usize;
    let mut skipped = 0usize;
    let mut errors = 0usize;

    for (file_id, rel_path) in files {
        let abs = root_path.join(&rel_path);
        let ext = abs
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        if !IMAGE_EXTS.contains(&ext.as_str()) {
            skipped += 1;
            continue;
        }

        // No DB conn held across this await.
        let img_result = tokio::task::spawn_blocking({
            let p = abs.clone();
            move || image::open(&p)
        })
        .await;

        match img_result {
            Ok(Ok(img)) => {
                let hash = dhash(&img);
                // Open fresh connection after the await to store the hash.
                let conn = open_db(&db_path)?;
                if db::store_phash(&conn, file_id, hash).is_ok() {
                    indexed += 1;
                } else {
                    errors += 1;
                }
                // conn dropped here
            }
            _ => {
                errors += 1;
            }
        }
    }

    Ok(Json(serde_json::json!({
        "total": total,
        "indexed": indexed,
        "skipped": skipped,
        "errors": errors,
    })))
}

// ---------------------------------------------------------------------------
// POST /api/similar/index-embedding
// ---------------------------------------------------------------------------

pub async fn api_index_embedding(
    State(state): State<Arc<AppState>>,
    Json(req): Json<IndexRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (db_path, endpoint, embed_model, api_key) = {
        let root = root_for_dir(&state, Path::new(&req.dir))
            .ok_or_else(|| anyhow::anyhow!("No database root found"))?;
        let conn = open_db(&root.db_path)?;
        let ep = db::get_setting(&conn, "ai.endpoint")
            .ok()
            .flatten()
            .unwrap_or_default();
        let model = db::get_setting(&conn, "ai.embed_model")
            .ok()
            .flatten()
            .unwrap_or_default();
        let key = db::get_setting(&conn, "ai.api_key").ok().flatten();
        let dp = root.db_path.clone();
        (dp, ep, model, key)
        // root borrow + conn dropped here
    };

    if endpoint.is_empty() || embed_model.is_empty() {
        return Ok(Json(serde_json::json!({
            "error": "No embedding model configured. Set ai.endpoint and ai.embed_model in the AI settings.",
            "indexed": 0,
        })));
    }

    let force = req.force.unwrap_or(false);

    // Load files that need embeddings — connection dropped before any await.
    let files_and_tags: Vec<(i64, String)> = {
        let conn = open_db(&db_path)?;
        let query = if force {
            "SELECT f.id,
                    COALESCE(GROUP_CONCAT(
                        t.name || CASE WHEN ft.value != '' THEN '=' || ft.value ELSE '' END,
                    ', '), '')
             FROM files f
             LEFT JOIN file_tags ft ON ft.file_id = f.id
             LEFT JOIN tags t ON t.id = ft.tag_id
             GROUP BY f.id"
        } else {
            "SELECT f.id,
                    COALESCE(GROUP_CONCAT(
                        t.name || CASE WHEN ft.value != '' THEN '=' || ft.value ELSE '' END,
                    ', '), '')
             FROM files f
             LEFT JOIN file_tags ft ON ft.file_id = f.id
             LEFT JOIN tags t ON t.id = ft.tag_id
             WHERE NOT EXISTS (SELECT 1 FROM file_embeddings fe WHERE fe.file_id = f.id)
             GROUP BY f.id"
        };
        let mut stmt = conn.prepare(query)?;
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1).unwrap_or_default(),
            ))
        })?
        .filter_map(|r| r.ok())
        .filter(|(_, tags)| !tags.is_empty())
        .collect()
        // conn dropped here
    };

    let total = files_and_tags.len();
    let mut indexed = 0usize;
    let mut errors = 0usize;

    for (file_id, tags) in files_and_tags {
        // No DB conn held across this await.
        match call_embedding_api(&endpoint, &embed_model, api_key.as_deref(), &tags).await {
            Ok(embedding) => {
                // Open fresh connection after the await to store the embedding.
                let conn = open_db(&db_path)?;
                if db::store_embedding(&conn, file_id, &embed_model, &embedding).is_ok() {
                    indexed += 1;
                } else {
                    errors += 1;
                }
                // conn dropped here
            }
            Err(_) => {
                errors += 1;
            }
        }
    }

    Ok(Json(serde_json::json!({
        "total": total,
        "indexed": indexed,
        "errors": errors,
    })))
}

// ---------------------------------------------------------------------------
// GET /api/similar/status
// ---------------------------------------------------------------------------

pub async fn api_similarity_status(
    State(state): State<Arc<AppState>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let dir = q.get("dir").map(String::as_str).unwrap_or("");

    let db_path = {
        let root = root_for_dir(&state, Path::new(dir))
            .ok_or_else(|| anyhow::anyhow!("No database root found"))?;
        root.db_path.clone()
        // root borrow dropped
    };

    // All work is synchronous; no await points.
    let conn = open_db(&db_path)?;

    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
        .unwrap_or(0);
    let phash_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE phash IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let embed_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM file_embeddings", [], |r| r.get(0))
        .unwrap_or(0);
    let embed_model = db::get_setting(&conn, "ai.embed_model")
        .ok()
        .flatten()
        .unwrap_or_default();

    Ok(Json(serde_json::json!({
        "total_files": total,
        "phash_indexed": phash_count,
        "embedding_indexed": embed_count,
        "embed_model": embed_model,
    })))
}
