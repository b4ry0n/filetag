use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params};

const DB_DIR: &str = ".filetag";
const DB_FILE: &str = "db.sqlite3";
const SCHEMA_VERSION: i32 = 6;

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
///
/// Never crosses a filesystem boundary: if the parent directory resides on a
/// different device than the starting path, the search stops.  A database from
/// another filesystem must not be used as the authority for files on this one.
pub fn find_and_open(start: &Path) -> Result<(Connection, PathBuf)> {
    let start = std::fs::canonicalize(start)
        .with_context(|| format!("canonicalizing {}", start.display()))?;

    // Record the device of the starting path once so we can detect a mount-
    // point crossing without an extra stat(2) on every ancestor directory.
    #[cfg(unix)]
    let start_dev: Option<u64> = {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(&start).ok().map(|m| m.dev())
    };

    let mut dir = start.as_path();
    loop {
        let candidate = dir.join(DB_DIR).join(DB_FILE);
        if candidate.is_file() {
            let conn = open_at(&candidate)?;
            migrate(&conn)?;
            return Ok((conn, dir.to_path_buf()));
        }
        match dir.parent() {
            Some(parent) => {
                // On Unix: stop if the parent is on a different device.
                // Tags on files without a database on the same filesystem
                // must be refused — storing them in a database on another
                // device would break portability and file-identity tracking.
                #[cfg(unix)]
                if let Some(start_d) = start_dev {
                    use std::os::unix::fs::MetadataExt;
                    if let Ok(meta) = std::fs::metadata(parent)
                        && meta.dev() != start_d
                    {
                        bail!(
                            "no filetag database found on this filesystem \
                                 (stopped at filesystem boundary at {})\n\
                                 Run `filetag init` inside this filesystem to create one.",
                            parent.display()
                        );
                    }
                }
                dir = parent;
            }
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
                file_id     TEXT,
                size        INTEGER NOT NULL,
                mtime_ns    INTEGER NOT NULL,
                indexed_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_files_path    ON files(path);
            CREATE INDEX IF NOT EXISTS idx_files_file_id ON files(file_id);

            CREATE TABLE IF NOT EXISTS tags (
                id   INTEGER PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                color TEXT
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

    if version < 3 {
        // Drop blake3, add file_id.  SQLite cannot DROP COLUMN on older versions,
        // so we just add the new column and ignore the old one.
        conn.execute_batch(
            "ALTER TABLE files ADD COLUMN file_id TEXT;
             CREATE INDEX IF NOT EXISTS idx_files_file_id ON files(file_id);",
        )
        .ok(); // ignore "duplicate column" if fresh DB already has it
    }

    if version < 4 {
        conn.execute_batch("ALTER TABLE tags ADD COLUMN color TEXT;")
            .ok(); // ignore if fresh DB already has it
    }

    if version < 5 {
        // Rename table and column: child_databases.rel_path -> linked_databases.path
        conn.execute_batch(
            "ALTER TABLE child_databases RENAME TO linked_databases;
             ALTER TABLE linked_databases RENAME COLUMN rel_path TO path;",
        )?;
    }
    if version < 6 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS settings (
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL DEFAULT ''
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
    pub file_id: Option<String>,
    pub size: i64,
    pub mtime_ns: i64,
}

/// Get or insert a file record. Updates metadata if the file has changed.
///
/// # Filesystem boundary check (Unix)
///
/// A file MUST reside on the same filesystem as the database root.  Storing a
/// tag in a database on a different device would silently break portability and
/// file-identity tracking.  This function enforces that invariant by comparing
/// `st_dev` of the file against `st_dev` of the database root.
pub fn get_or_index_file(conn: &Connection, rel_path: &str, root: &Path) -> Result<FileRecord> {
    let abs = root.join(rel_path);
    let meta = std::fs::metadata(&abs)
        .with_context(|| format!("reading metadata for {}", abs.display()))?;

    // --- filesystem boundary guard (Unix only) ----------------------------
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let file_dev = meta.dev();
        let root_dev = std::fs::metadata(root)
            .with_context(|| format!("reading metadata for database root {}", root.display()))?
            .dev();
        if file_dev != root_dev {
            bail!(
                "cannot tag {}: file is on a different filesystem than the database at {}\n\
                 Run `filetag init` inside the filesystem that contains this file.",
                abs.display(),
                root.display()
            );
        }
    }
    // ----------------------------------------------------------------------
    let size = meta.len() as i64;
    let mtime_ns = {
        let mt = meta
            .modified()
            .with_context(|| format!("reading mtime for {}", abs.display()))?;
        mt.duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0)
    };
    let fid = get_file_id(&meta);

    // Check for existing record
    let existing: Option<FileRecord> = conn
        .prepare_cached("SELECT id, path, file_id, size, mtime_ns FROM files WHERE path = ?1")?
        .query_row(params![rel_path], |row| {
            Ok(FileRecord {
                id: row.get(0)?,
                path: row.get(1)?,
                file_id: row.get(2)?,
                size: row.get(3)?,
                mtime_ns: row.get(4)?,
            })
        })
        .ok();

    if let Some(rec) = existing {
        // Update metadata if changed (size, mtime, or file_id)
        if rec.size != size || rec.mtime_ns != mtime_ns || rec.file_id != fid {
            conn.execute(
                "UPDATE files SET file_id = ?1, size = ?2, mtime_ns = ?3, indexed_at = datetime('now') WHERE id = ?4",
                params![fid, size, mtime_ns, rec.id],
            )?;
            return Ok(FileRecord {
                file_id: fid,
                size,
                mtime_ns,
                ..rec
            });
        }
        return Ok(rec);
    }

    // New file
    conn.execute(
        "INSERT INTO files (path, file_id, size, mtime_ns) VALUES (?1, ?2, ?3, ?4)",
        params![rel_path, fid, size, mtime_ns],
    )?;
    let id = conn.last_insert_rowid();
    Ok(FileRecord {
        id,
        path: rel_path.to_string(),
        file_id: fid,
        size,
        mtime_ns,
    })
}

/// Platform-specific persistent file identifier (device:inode on Unix).
#[cfg(unix)]
fn get_file_id(meta: &std::fs::Metadata) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    Some(format!("{}:{}", meta.dev(), meta.ino()))
}

#[cfg(windows)]
fn get_file_id(_meta: &std::fs::Metadata) -> Option<String> {
    // Windows file IDs require opening a handle; not yet implemented.
    None
}

#[cfg(not(any(unix, windows)))]
fn get_file_id(_meta: &std::fs::Metadata) -> Option<String> {
    None
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
pub fn all_tags(conn: &Connection) -> Result<Vec<(String, i64, Option<String>)>> {
    let mut stmt = conn.prepare(
        "SELECT t.name, COUNT(ft.file_id), t.color
         FROM tags t
         LEFT JOIN file_tags ft ON ft.tag_id = t.id
         GROUP BY t.id
         ORDER BY t.name",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// Set or clear the color for a tag.
pub fn set_tag_color(conn: &Connection, name: &str, color: Option<&str>) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE tags SET color = ?1 WHERE name = ?2",
        params![color, name],
    )?;
    Ok(changed > 0)
}

/// Delete a tag entirely: removes all file_tags rows and the tag itself.
pub fn delete_tag(conn: &Connection, name: &str) -> Result<bool> {
    let tag_id: Option<i64> = conn
        .prepare_cached("SELECT id FROM tags WHERE name = ?1")?
        .query_row(params![name], |r| r.get(0))
        .ok();
    if let Some(id) = tag_id {
        conn.execute("DELETE FROM file_tags WHERE tag_id = ?1", params![id])?;
        conn.execute("DELETE FROM tags WHERE id = ?1", params![id])?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Look up a file record by relative path.
pub fn file_by_path(conn: &Connection, rel_path: &str) -> Result<Option<FileRecord>> {
    let rec = conn
        .prepare_cached("SELECT id, path, file_id, size, mtime_ns FROM files WHERE path = ?1")?
        .query_row(params![rel_path], |row| {
            Ok(FileRecord {
                id: row.get(0)?,
                path: row.get(1)?,
                file_id: row.get(2)?,
                size: row.get(3)?,
                mtime_ns: row.get(4)?,
            })
        })
        .ok();
    Ok(rec)
}

// ---------------------------------------------------------------------------
// Archive entry indexing
// ---------------------------------------------------------------------------

/// Resolve a user-provided archive-entry path (e.g. `"archive.cbz::entry.jpg"` or
/// `"/abs/path/archive.cbz::entry.jpg"`) to a DB-relative virtual path
/// (e.g. `"photos/archive.cbz::entry.jpg"`).
///
/// The archive file itself must exist on disk and must be under `root`.
pub fn resolve_archive_entry(raw: &str, root: &Path) -> Result<String> {
    let (zip_str, entry) = raw
        .split_once("::")
        .with_context(|| format!("not an archive entry path: {}", raw))?;
    let zip_abs = std::fs::canonicalize(zip_str)
        .with_context(|| format!("cannot find archive file: {}", zip_str))?;
    let zip_rel = zip_abs.strip_prefix(root).with_context(|| {
        format!(
            "{} is not under database root {}",
            zip_abs.display(),
            root.display()
        )
    })?;
    Ok(format!("{}::{}", zip_rel.to_string_lossy(), entry))
}

/// Ensure a `files` record exists for a virtual archive-entry path such as
/// `"photos/archive.cbz::cover.jpg"`.  Does not touch the filesystem beyond
/// the existence check already done in `resolve_archive_entry`.
///
/// Returns the file record (creating it with `size=0 / mtime_ns=0` when new).
pub fn get_or_index_archive_entry(conn: &Connection, virtual_path: &str) -> Result<FileRecord> {
    if let Some(existing) = file_by_path(conn, virtual_path)? {
        return Ok(existing);
    }
    conn.execute(
        "INSERT INTO files (path, file_id, size, mtime_ns, indexed_at) \
         VALUES (?1, NULL, 0, 0, datetime('now'))",
        params![virtual_path],
    )?;
    let id = conn.last_insert_rowid();
    Ok(FileRecord {
        id,
        path: virtual_path.to_string(),
        file_id: None,
        size: 0,
        mtime_ns: 0,
    })
}

// ---------------------------------------------------------------------------
// Child database management
// ---------------------------------------------------------------------------

/// Register a linked database. Stores a path relative to the current root when the
/// target is under this root (child), or an absolute path otherwise (partner/parent).
pub fn link_database(conn: &Connection, path: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO linked_databases (path) VALUES (?1)",
        params![path],
    )?;
    Ok(())
}

/// Remove a linked database registration.
pub fn unlink_database(conn: &Connection, path: &str) -> Result<bool> {
    let changed = conn.execute(
        "DELETE FROM linked_databases WHERE path = ?1",
        params![path],
    )?;
    Ok(changed > 0)
}

/// List all registered linked database paths (relative or absolute).
pub fn list_linked(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT path FROM linked_databases ORDER BY path")?;
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
    pub file_id: Option<String>,
    pub size: i64,
    pub mtime_ns: i64,
    /// (tag_name, value) pairs
    pub tags: Vec<(String, String)>,
}

/// Collect tag (name, value) pairs for a file by its `files.id`.
fn collect_file_tags(
    tag_stmt: &mut rusqlite::Statement<'_>,
    file_id: i64,
) -> Vec<(String, String)> {
    tag_stmt
        .query_map(params![file_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
}

/// Find all files whose path starts with `prefix/` and return them with their tags.
pub fn files_under_prefix(conn: &Connection, prefix: &str) -> Result<Vec<FileWithTags>> {
    let pattern = format!("{}/%", prefix.trim_end_matches('/'));
    let mut stmt = conn.prepare(
        "SELECT f.id, f.path, f.file_id, f.size, f.mtime_ns
         FROM files f
         WHERE f.path LIKE ?1",
    )?;
    let mut tag_stmt = conn.prepare(
        "SELECT t.name, ft.value
         FROM file_tags ft
         JOIN tags t ON t.id = ft.tag_id
         WHERE ft.file_id = ?1",
    )?;
    let rows = stmt.query_map(params![pattern], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;
    let mut results = Vec::new();
    for row in rows {
        let (id, path, file_id, size, mtime_ns) = row?;
        results.push(FileWithTags {
            rel_path: path,
            file_id,
            size,
            mtime_ns,
            tags: collect_file_tags(&mut tag_stmt, id),
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
        conn.prepare("SELECT f.id, f.path, f.file_id, f.size, f.mtime_ns FROM files f")?;
    let mut tag_stmt = conn.prepare(
        "SELECT t.name, ft.value
         FROM file_tags ft
         JOIN tags t ON t.id = ft.tag_id
         WHERE ft.file_id = ?1",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;
    let mut results = Vec::new();
    for row in rows {
        let (id, path, file_id, size, mtime_ns) = row?;
        results.push(FileWithTags {
            rel_path: path,
            file_id,
            size,
            mtime_ns,
            tags: collect_file_tags(&mut tag_stmt, id),
        });
    }
    Ok(results)
}

/// An opened database with its root directory.
pub struct OpenDb {
    pub conn: Connection,
    pub root: PathBuf,
}

/// Collect this database and all reachable linked databases recursively.
/// Gracefully skips missing or broken databases. Uses cycle detection on
/// canonical root paths.
///
/// Linked paths may be relative (child, under current root) or absolute
/// (partner/parent, outside current root). `PathBuf::join` handles both:
/// joining with an absolute path replaces the base entirely.
/// Collect this database and all reachable linked databases recursively.
/// When `include_ancestors` is `false`, automatic ancestor-database discovery
/// is skipped and only explicit links are followed.
pub fn collect_all_databases(
    conn: Connection,
    root: PathBuf,
    include_ancestors: bool,
) -> Result<Vec<OpenDb>> {
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

        // Collect linked databases before moving the connection.
        // Relative paths resolve under r; absolute paths (partner DBs) replace r entirely.
        let linked = list_linked(&c).unwrap_or_default();
        for linked_path in linked {
            let linked_root = r.join(&linked_path);
            let linked_db_path = linked_root.join(DB_DIR).join(DB_FILE);
            match open_at(&linked_db_path) {
                Ok(linked_conn) => {
                    // Run migration in case the linked DB is an older schema version
                    if migrate(&linked_conn).is_ok() {
                        queue.push((linked_conn, linked_root));
                    }
                }
                Err(e) => {
                    eprintln!(
                        "warning: skipping linked database {}: {}",
                        linked_db_path.display(),
                        e
                    );
                }
            }
        }

        // Automatically include ancestor databases. When working under a
        // sub-tree that has its own database (e.g. ~/Documents), parent
        // databases (e.g. ~/) are implicitly relevant even if they are not
        // explicitly registered as a linked database.
        //
        // Ancestors are pushed directly into `result` (not `queue`) so that
        // their own linked databases (siblings of the current root) are NOT
        // pulled in transitively. Only the ancestor chain itself is relevant.
        if include_ancestors {
            let mut ancestor = r.parent();
            while let Some(dir) = ancestor {
                let ancestor_db_path = dir.join(DB_DIR).join(DB_FILE);
                if ancestor_db_path.is_file() {
                    let canonical_anc =
                        std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
                    if !visited.contains(&canonical_anc)
                        && let Ok(ancestor_conn) = open_at(&ancestor_db_path)
                        && migrate(&ancestor_conn).is_ok()
                    {
                        visited.insert(canonical_anc);
                        result.push(OpenDb {
                            conn: ancestor_conn,
                            root: dir.to_path_buf(),
                        });
                    }
                }
                ancestor = dir.parent();
            }
        }

        result.push(OpenDb { conn: c, root: r });
    }

    Ok(result)
}

/// Recursively scan `root` for nested `.filetag/db.sqlite3` databases up to
/// `max_depth` levels deep. Returns one `OpenDb` per database found, skipping
/// any path already in `visited` (canonical paths).
///
/// This is used by filetag-web to discover all databases under the served
/// root(s) at startup, so that browsing, tagging, and searching are always
/// consistent: every directory that has its own database is included in the
/// session, whether or not it was explicitly registered via `filetag db add`.
/// Recursively scan the filesystem under `root` for nested `.filetag/db.sqlite3` databases.
///
/// `visited` is the set of already-known canonical roots (used for cycle detection).
/// `max_depth` limits how deep the scan descends (10 is a sensible default).
/// `on_dir` is called for every directory entered (depth >= 1) so callers can show progress.
pub fn scan_for_databases(
    root: &Path,
    visited: &mut std::collections::HashSet<std::path::PathBuf>,
    max_depth: usize,
    on_dir: &mut dyn FnMut(&Path),
) -> Vec<OpenDb> {
    let mut result = Vec::new();
    scan_recursive(root, visited, 0, max_depth, &mut result, on_dir);
    result
}

/// Directories that are known to never contain `.filetag/` databases and are either
/// extremely large or virtual/system filesystems. Skipping them keeps the scan fast.
///
/// Names are matched against the final path component only (case-sensitive).
/// Note: hidden directories (names starting with `.`) are already skipped unconditionally.
const SCAN_SKIP_DIRS: &[&str] = &[
    // macOS system directories
    "Library", // ~/Library and /Library (caches, app support, frameworks)
    "System",  // /System — macOS OS files
    "private", // /private — macOS private system tree
    "cores",   // /cores — kernel core dumps
    // Linux virtual/system filesystems
    "proc", // /proc — Linux process virtual fs
    "sys",  // /sys — Linux sysfs
    "run",  // /run — Linux runtime data
    "snap", // /snap — snapd package mount point
    // Common large build/cache directories that never hold databases
    "node_modules",
    "__pycache__",
    ".Trash",
];

fn scan_recursive(
    dir: &Path,
    visited: &mut std::collections::HashSet<std::path::PathBuf>,
    depth: usize,
    max_depth: usize,
    result: &mut Vec<OpenDb>,
    on_dir: &mut dyn FnMut(&Path),
) {
    if depth > max_depth {
        return;
    }
    if depth >= 1 {
        on_dir(dir);
    }
    let db_path = dir.join(DB_DIR).join(DB_FILE);
    if db_path.is_file() {
        let canonical = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        if !visited.contains(&canonical)
            && let Ok(conn) = open_at(&db_path)
            && migrate(&conn).is_ok()
        {
            visited.insert(canonical);
            result.push(OpenDb {
                conn,
                root: dir.to_path_buf(),
            });
        }
    }
    let read_dir = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return,
    };
    for entry in read_dir.flatten() {
        // Use file_type() from the readdir entry — avoids an extra stat(2) syscall.
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        // Skip hidden directories (the .filetag/ dir we already handled above).
        if name_str.starts_with('.') {
            continue;
        }
        // Skip known large directories that never contain .filetag/ databases.
        if SCAN_SKIP_DIRS.contains(&&*name_str) {
            continue;
        }
        scan_recursive(&entry.path(), visited, depth + 1, max_depth, result, on_dir);
    }
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

// ---------------------------------------------------------------------------
// Settings (per-database key/value store)
// ---------------------------------------------------------------------------

/// Read a setting value, returning `None` if the key does not exist.
pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![key],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(Into::into)
}

/// Insert or update a setting value.
pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}
