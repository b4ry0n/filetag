use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params};

const DB_DIR: &str = ".filetag";
const DB_FILE: &str = "db.sqlite3";
const SCHEMA_VERSION: i32 = 2;

/// Open (or create) the database inside the given root directory.
/// Creates `.filetag/db.sqlite3` under `root`.
pub fn init(root: &Path) -> Result<Connection> {
    let db_dir = root.join(DB_DIR);
    std::fs::create_dir_all(&db_dir).with_context(|| format!("creating {}", db_dir.display()))?;
    let db_path = db_dir.join(DB_FILE);
    let conn = open_at(&db_path)?;
    migrate(&conn)?;
    Ok(conn)
}

/// Walk parent directories to find an existing `.filetag/db.sqlite3`.
/// Returns (connection, root_dir) where root_dir is the parent of `.filetag/`.
pub fn find_and_open(start: &Path) -> Result<(Connection, PathBuf)> {
    let start = std::fs::canonicalize(start)
        .with_context(|| format!("canonicalizing {}", start.display()))?;
    let mut dir = start.as_path();
    loop {
        let candidate = dir.join(DB_DIR).join(DB_FILE);
        if candidate.is_file() {
            let conn = open_at(&candidate)?;
            return Ok((conn, dir.to_path_buf()));
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => bail!(
                "no filetag database found (looked from {} upward)\n\
                 Run `filetag init` to create one.",
                start.display()
            ),
        }
    }
}

fn open_at(path: &Path) -> Result<Connection> {
    let conn =
        Connection::open(path).with_context(|| format!("opening database {}", path.display()))?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> Result<()> {
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap_or(0);

    if version < 1 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS files (
                id          INTEGER PRIMARY KEY,
                path        TEXT NOT NULL,
                blake3      TEXT,
                size        INTEGER NOT NULL,
                mtime_ns    INTEGER NOT NULL,
                indexed_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_files_path   ON files(path);
            CREATE INDEX IF NOT EXISTS idx_files_blake3 ON files(blake3);

            CREATE TABLE IF NOT EXISTS tags (
                id   INTEGER PRIMARY KEY,
                name TEXT NOT NULL UNIQUE
            );

            CREATE TABLE IF NOT EXISTS file_tags (
                file_id    INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                tag_id     INTEGER NOT NULL REFERENCES tags(id)  ON DELETE CASCADE,
                value      TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (file_id, tag_id, value)
            );
            CREATE INDEX IF NOT EXISTS idx_file_tags_tag ON file_tags(tag_id);",
        )?;
    }

    if version < 2 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS child_databases (
                id       INTEGER PRIMARY KEY,
                rel_path TEXT NOT NULL UNIQUE
            );",
        )?;
    }

    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

/// Resolve a file path to a relative path from the database root.
pub fn relative_to_root(path: &Path, root: &Path) -> Result<String> {
    let abs = std::fs::canonicalize(path)
        .with_context(|| format!("canonicalizing {}", path.display()))?;
    let rel = abs.strip_prefix(root).with_context(|| {
        format!(
            "{} is not under database root {}",
            abs.display(),
            root.display()
        )
    })?;
    Ok(rel.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------
// File indexing
// ---------------------------------------------------------------------------

/// Metadata stored per file in the database.
pub struct FileRecord {
    pub id: i64,
    #[allow(dead_code)]
    pub path: String,
    pub blake3: Option<String>,
    pub size: i64,
    pub mtime_ns: i64,
}

/// Get or insert a file record. Hashes with BLAKE3 if the file is new or changed.
pub fn get_or_index_file(conn: &Connection, rel_path: &str, root: &Path) -> Result<FileRecord> {
    let abs = root.join(rel_path);
    let meta = std::fs::metadata(&abs)
        .with_context(|| format!("reading metadata for {}", abs.display()))?;
    let size = meta.len() as i64;
    let mtime_ns = {
        let mt = meta
            .modified()
            .with_context(|| format!("reading mtime for {}", abs.display()))?;
        mt.duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0)
    };

    // Check for existing record
    let existing: Option<FileRecord> = conn
        .prepare_cached("SELECT id, path, blake3, size, mtime_ns FROM files WHERE path = ?1")?
        .query_row(params![rel_path], |row| {
            Ok(FileRecord {
                id: row.get(0)?,
                path: row.get(1)?,
                blake3: row.get(2)?,
                size: row.get(3)?,
                mtime_ns: row.get(4)?,
            })
        })
        .ok();

    if let Some(rec) = existing {
        if rec.size == size && rec.mtime_ns == mtime_ns && rec.blake3.is_some() {
            return Ok(rec);
        }
        // Changed: rehash
        let hash = hash_file(&abs)?;
        conn.execute(
            "UPDATE files SET blake3 = ?1, size = ?2, mtime_ns = ?3, indexed_at = datetime('now') WHERE id = ?4",
            params![hash, size, mtime_ns, rec.id],
        )?;
        return Ok(FileRecord {
            blake3: Some(hash),
            size,
            mtime_ns,
            ..rec
        });
    }

    // New file
    let hash = hash_file(&abs)?;
    conn.execute(
        "INSERT INTO files (path, blake3, size, mtime_ns) VALUES (?1, ?2, ?3, ?4)",
        params![rel_path, hash, size, mtime_ns],
    )?;
    let id = conn.last_insert_rowid();
    Ok(FileRecord {
        id,
        path: rel_path.to_string(),
        blake3: Some(hash),
        size,
        mtime_ns,
    })
}

fn hash_file(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("opening {} for hashing", path.display()))?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

// ---------------------------------------------------------------------------
// Tag operations
// ---------------------------------------------------------------------------

/// Get or create a tag, returning its id.
pub fn get_or_create_tag(conn: &Connection, name: &str) -> Result<i64> {
    if let Ok(id) = conn.query_row("SELECT id FROM tags WHERE name = ?1", params![name], |r| {
        r.get::<_, i64>(0)
    }) {
        return Ok(id);
    }
    conn.execute("INSERT INTO tags (name) VALUES (?1)", params![name])?;
    Ok(conn.last_insert_rowid())
}

/// Apply a tag (with optional value) to a file.
pub fn apply_tag(conn: &Connection, file_id: i64, tag_id: i64, value: Option<&str>) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO file_tags (file_id, tag_id, value) VALUES (?1, ?2, ?3)",
        params![file_id, tag_id, value.unwrap_or("")],
    )?;
    Ok(())
}

/// Remove a tag (with optional value) from a file.
pub fn remove_tag(
    conn: &Connection,
    file_id: i64,
    tag_id: i64,
    value: Option<&str>,
) -> Result<bool> {
    let changed = if let Some(v) = value {
        conn.execute(
            "DELETE FROM file_tags WHERE file_id = ?1 AND tag_id = ?2 AND value = ?3",
            params![file_id, tag_id, v],
        )?
    } else {
        conn.execute(
            "DELETE FROM file_tags WHERE file_id = ?1 AND tag_id = ?2",
            params![file_id, tag_id],
        )?
    };
    Ok(changed > 0)
}

/// List all tags on a file, returned as `(tag_name, Option<value>)`.
pub fn tags_for_file(conn: &Connection, file_id: i64) -> Result<Vec<(String, Option<String>)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT t.name, ft.value
         FROM file_tags ft
         JOIN tags t ON t.id = ft.tag_id
         WHERE ft.file_id = ?1
         ORDER BY t.name, ft.value",
    )?;
    let rows = stmt.query_map(params![file_id], |row| {
        let name: String = row.get(0)?;
        let value: String = row.get(1)?;
        let value = if value.is_empty() { None } else { Some(value) };
        Ok((name, value))
    })?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// List all known tags (with usage count).
pub fn all_tags(conn: &Connection) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT t.name, COUNT(ft.file_id)
         FROM tags t
         LEFT JOIN file_tags ft ON ft.tag_id = t.id
         GROUP BY t.id
         ORDER BY t.name",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// Look up a file record by relative path.
pub fn file_by_path(conn: &Connection, rel_path: &str) -> Result<Option<FileRecord>> {
    let rec = conn
        .prepare_cached("SELECT id, path, blake3, size, mtime_ns FROM files WHERE path = ?1")?
        .query_row(params![rel_path], |row| {
            Ok(FileRecord {
                id: row.get(0)?,
                path: row.get(1)?,
                blake3: row.get(2)?,
                size: row.get(3)?,
                mtime_ns: row.get(4)?,
            })
        })
        .ok();
    Ok(rec)
}

// ---------------------------------------------------------------------------
// Child database management
// ---------------------------------------------------------------------------

/// Register a child database by its path relative to the current DB root.
pub fn add_child(conn: &Connection, rel_path: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO child_databases (rel_path) VALUES (?1)",
        params![rel_path],
    )?;
    Ok(())
}

/// Remove a child database registration.
pub fn remove_child(conn: &Connection, rel_path: &str) -> Result<bool> {
    let changed = conn.execute(
        "DELETE FROM child_databases WHERE rel_path = ?1",
        params![rel_path],
    )?;
    Ok(changed > 0)
}

/// List all registered child database paths.
pub fn list_children(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT rel_path FROM child_databases ORDER BY rel_path")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// A file record with all its tags, used for transferring between databases.
pub struct FileWithTags {
    pub rel_path: String,
    pub blake3: Option<String>,
    pub size: i64,
    pub mtime_ns: i64,
    /// (tag_name, value) pairs
    pub tags: Vec<(String, String)>,
}

/// Find all files whose path starts with `prefix/` and return them with their tags.
pub fn files_under_prefix(conn: &Connection, prefix: &str) -> Result<Vec<FileWithTags>> {
    let pattern = format!("{}/%", prefix.trim_end_matches('/'));
    let mut stmt = conn.prepare(
        "SELECT f.id, f.path, f.blake3, f.size, f.mtime_ns
         FROM files f
         WHERE f.path LIKE ?1",
    )?;
    let mut tag_stmt = conn.prepare(
        "SELECT t.name, ft.value
         FROM file_tags ft
         JOIN tags t ON t.id = ft.tag_id
         WHERE ft.file_id = ?1",
    )?;
    let mut results = Vec::new();
    let rows = stmt.query_map(params![pattern], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;
    for row in rows {
        let (id, path, blake3, size, mtime_ns) = row?;
        let tags: Vec<(String, String)> = tag_stmt
            .query_map(params![id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect();
        results.push(FileWithTags {
            rel_path: path,
            blake3,
            size,
            mtime_ns,
            tags,
        });
    }
    Ok(results)
}

/// Delete a file and its tags from the database (cascade via FK).
pub fn delete_file_by_path(conn: &Connection, rel_path: &str) -> Result<bool> {
    let changed = conn.execute("DELETE FROM files WHERE path = ?1", params![rel_path])?;
    Ok(changed > 0)
}

/// Get all files with their tags from the database.
pub fn all_files_with_tags(conn: &Connection) -> Result<Vec<FileWithTags>> {
    let mut stmt =
        conn.prepare("SELECT f.id, f.path, f.blake3, f.size, f.mtime_ns FROM files f")?;
    let mut tag_stmt = conn.prepare(
        "SELECT t.name, ft.value
         FROM file_tags ft
         JOIN tags t ON t.id = ft.tag_id
         WHERE ft.file_id = ?1",
    )?;
    let mut results = Vec::new();
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;
    for row in rows {
        let (id, path, blake3, size, mtime_ns) = row?;
        let tags: Vec<(String, String)> = tag_stmt
            .query_map(params![id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect();
        results.push(FileWithTags {
            rel_path: path,
            blake3,
            size,
            mtime_ns,
            tags,
        });
    }
    Ok(results)
}

/// An opened database with its root directory.
pub struct OpenDb {
    pub conn: Connection,
    pub root: PathBuf,
}

/// Collect this database and all reachable child databases recursively.
/// Gracefully skips missing or broken databases. Uses cycle detection on
/// canonical root paths.
pub fn collect_all_databases(conn: Connection, root: PathBuf) -> Result<Vec<OpenDb>> {
    use std::collections::HashSet;

    let mut result = Vec::new();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut queue: Vec<(Connection, PathBuf)> = vec![(conn, root)];

    while let Some((c, r)) = queue.pop() {
        let canonical = match std::fs::canonicalize(&r) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if !visited.insert(canonical) {
            continue; // cycle detection
        }

        // Collect children before moving the connection
        let children = list_children(&c).unwrap_or_default();
        for child_rel in children {
            let child_root = r.join(&child_rel);
            let child_db_path = child_root.join(DB_DIR).join(DB_FILE);
            match open_at(&child_db_path) {
                Ok(child_conn) => {
                    // Run migration in case the child is an older schema version
                    if migrate(&child_conn).is_ok() {
                        queue.push((child_conn, child_root));
                    }
                }
                Err(e) => {
                    eprintln!(
                        "warning: skipping child database {}: {}",
                        child_db_path.display(),
                        e
                    );
                }
            }
        }

        result.push(OpenDb { conn: c, root: r });
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Directory listing (for web interface)
// ---------------------------------------------------------------------------

/// An entry in a directory listing (file or subdirectory).
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: i64,
    pub mtime_ns: i64,
    pub file_count: i64,
    pub tag_count: i64,
}

/// List files and subdirectories at a given path level in the database.
/// Directories are synthesised from file paths. Directories come first, then
/// files, both sorted alphabetically.
pub fn list_directory(conn: &Connection, prefix: &str) -> Result<Vec<DirEntry>> {
    use std::collections::HashMap;

    let prefix_with_slash = if prefix.is_empty() {
        String::new()
    } else {
        format!("{}/", prefix.trim_end_matches('/'))
    };
    let pattern = format!("{}%", prefix_with_slash);
    let prefix_len = prefix_with_slash.len();

    let mut stmt = conn.prepare(
        "SELECT f.path, f.size, f.mtime_ns,
                (SELECT COUNT(*) FROM file_tags WHERE file_id = f.id)
         FROM files f WHERE f.path LIKE ?1 ORDER BY f.path",
    )?;

    let mut files = Vec::new();
    let mut subdirs: HashMap<String, i64> = HashMap::new();

    let rows = stmt.query_map(params![pattern], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;

    for row in rows {
        let (path, size, mtime_ns, tag_count) = row?;
        let relative = &path[prefix_len..];
        if let Some(slash_pos) = relative.find('/') {
            let dir_name = &relative[..slash_pos];
            *subdirs.entry(dir_name.to_string()).or_insert(0) += 1;
        } else {
            files.push(DirEntry {
                name: relative.to_string(),
                is_dir: false,
                size,
                mtime_ns,
                file_count: 0,
                tag_count,
            });
        }
    }

    let mut entries: Vec<DirEntry> = subdirs
        .into_iter()
        .map(|(name, count)| DirEntry {
            name,
            is_dir: true,
            size: 0,
            mtime_ns: 0,
            file_count: count,
            tag_count: 0,
        })
        .collect();

    entries.sort_by(|a, b| a.name.cmp(&b.name));
    files.sort_by(|a, b| a.name.cmp(&b.name));
    entries.extend(files);

    Ok(entries)
}
