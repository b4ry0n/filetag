//! Database layer: initialisation, schema migration, and all CRUD operations.
//!
//! Each filetag database lives at `<root>/.filetag/db.sqlite3`. The current
//! schema version is 13.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params};

const DB_DIR: &str = ".filetag";
const DB_FILE: &str = "db.sqlite3";
const SCHEMA_VERSION: i32 = 15;

// ---------------------------------------------------------------------------
// Database identity
// ---------------------------------------------------------------------------

/// Generate a random UUID v4, formatted as lowercase hex with hyphens.
fn generate_db_id() -> String {
    let mut bytes = [0u8; 16];
    rand::fill(&mut bytes);
    // Set UUID v4 version and variant bits.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        u16::from_be_bytes([bytes[4], bytes[5]]),
        u16::from_be_bytes([bytes[6], bytes[7]]),
        u16::from_be_bytes([bytes[8], bytes[9]]),
        u64::from_be_bytes([
            0, 0, bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
        ])
    )
}

/// Return this database's own unique ID.
///
/// The ID is stored in the `settings` table under the key `"db_id"`.  If it
/// is absent (e.g. an existing database that has not been migrated yet), a new
/// UUID v4 is generated, persisted, and returned.
pub fn get_db_id(conn: &Connection) -> Result<String> {
    use rusqlite::OptionalExtension;
    if let Some(id) = conn
        .query_row("SELECT value FROM settings WHERE key = 'db_id'", [], |r| {
            r.get::<_, String>(0)
        })
        .optional()?
    {
        return Ok(id);
    }
    let id = generate_db_id();
    set_setting(conn, "db_id", &id)?;
    Ok(id)
}

// ---------------------------------------------------------------------------
// Filesystem boundary detection
// ---------------------------------------------------------------------------

/// Return an opaque volume identifier for `path`, or `None` if it cannot be
/// determined.
///
/// On Unix this is the `st_dev` device number from `stat(2)`.  On Windows it
/// is the volume serial number exposed by
/// [`MetadataExt::volume_serial_number`][std::os::windows::fs::MetadataExt::volume_serial_number]
/// (stable since Rust 1.58), which reliably distinguishes drive letters and
/// mount points without any unsafe code or extra crates.  When the identifier
/// cannot be obtained the check is skipped and `None` is returned, which is a
/// safe fallback (the walk continues; worst case the user receives a
/// "not found" error rather than a spurious boundary error).
pub fn volume_id(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).ok().map(|m| m.dev())
    }

    #[cfg(windows)]
    {
        // `volume_serial_number()` is not yet stable on Windows. Volume-id is
        // used only for filesystem-boundary detection; returning None disables
        // the check, which is acceptable on Windows.
        let _ = path;
        None
    }

    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

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

/// Walk parent directories to find the Root that governs `start`.
///
/// Returns the absolute, canonicalised path of the Root directory (the
/// directory that directly contains `.filetag/`), or an error if no Root
/// exists on the same filesystem as `start`.
///
/// Never crosses a filesystem boundary: walking stops when the parent
/// directory resides on a different device than `start`.  A database from
/// another filesystem must not be used as the authority for files on this one.
pub fn find_root(start: &Path) -> Result<PathBuf> {
    let start = std::fs::canonicalize(start)
        .with_context(|| format!("canonicalizing {}", start.display()))?;

    let start_vol = volume_id(&start);

    let mut dir = start.as_path();
    loop {
        let candidate = dir.join(DB_DIR).join(DB_FILE);
        if candidate.is_file() {
            return Ok(dir.to_path_buf());
        }
        match dir.parent() {
            Some(parent) => {
                // Stop if the parent is on a different filesystem.
                // Tags on files without a database on the same filesystem
                // must be refused — storing them in a database on another
                // device would break portability and file-identity tracking.
                if let (Some(sv), Some(pv)) = (start_vol, volume_id(parent))
                    && sv != pv
                {
                    bail!(
                        "no filetag database found on this filesystem \
                         (stopped at filesystem boundary at {})\n\
                         Run `filetag init` inside this filesystem to create one.",
                        parent.display()
                    );
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

/// Open the database for the Root at `root` and run any pending migrations.
///
/// This is the second half of Root resolution: call [`find_root`] first to
/// locate the Root, then call this function to obtain a connection.
///
/// Returns `(connection, root)` where `root` is the canonicalised path passed
/// in, echoed back for ergonomic chaining.
pub fn open_root_db(root: &Path) -> Result<(Connection, PathBuf)> {
    let db_path = root.join(DB_DIR).join(DB_FILE);
    let conn = open_at(&db_path)?;
    migrate(&conn)?;
    Ok((conn, root.to_path_buf()))
}

/// Convenience wrapper: resolve the Root for `start`, then open its database.
///
/// Equivalent to `open_root_db(&find_root(start)?)`.  Use the individual
/// functions when you need the Root path before opening a connection.
pub fn find_and_open(start: &Path) -> Result<(Connection, PathBuf)> {
    let root = find_root(start)?;
    open_root_db(&root)
}

/// Fast variant of `find_and_open` that skips migration.  Use this for every
/// file-operation request after server startup.
pub fn find_and_open_fast(start: &Path) -> Result<(Connection, PathBuf)> {
    let root = find_root(start)?;
    let db_path = root.join(DB_DIR).join(DB_FILE);
    let conn = open_at(&db_path)?;
    Ok((conn, root))
}

/// Open an existing, already-migrated database without running migrations.
/// Use this for every read/write request after server startup.
pub fn open_db_fast(db_path: &Path) -> Result<Connection> {
    let conn = Connection::open(db_path)
        .with_context(|| format!("opening database {}", db_path.display()))?;
    conn.execute_batch(
        "PRAGMA busy_timeout = 5000;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;",
    )?;
    Ok(conn)
}

fn open_at(path: &Path) -> Result<Connection> {
    open_db_fast(path)
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
    if version < 7 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tag_synonyms (
                 alias        TEXT NOT NULL,
                 canonical_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
                 PRIMARY KEY (alias)
             );",
        )?
    }
    if version < 8 {
        // Add `subject` column to file_tags and extend the PRIMARY KEY to include it.
        // SQLite cannot alter primary keys, so we recreate the table.
        conn.execute_batch(
            "ALTER TABLE file_tags RENAME TO file_tags_v7;
             CREATE TABLE file_tags (
                 file_id    INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                 tag_id     INTEGER NOT NULL REFERENCES tags(id)  ON DELETE CASCADE,
                 value      TEXT NOT NULL DEFAULT '',
                 subject    TEXT NOT NULL DEFAULT '',
                 created_at TEXT NOT NULL DEFAULT (datetime('now')),
                 PRIMARY KEY (file_id, tag_id, value, subject)
             );
             CREATE INDEX IF NOT EXISTS idx_file_tags_tag ON file_tags(tag_id);
             INSERT INTO file_tags (file_id, tag_id, value, created_at)
                 SELECT file_id, tag_id, value, created_at FROM file_tags_v7;
             DROP TABLE file_tags_v7;",
        )?
    }
    if version < 9 {
        // subject_tags stores properties/tags that describe a subject entity itself
        // (distinct from file_tags.subject which groups per-file tag assignments).
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS subject_tags (
                 subject    TEXT NOT NULL,
                 tag_id     INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
                 value      TEXT NOT NULL DEFAULT '',
                 created_at TEXT NOT NULL DEFAULT (datetime('now')),
                 PRIMARY KEY (subject, tag_id, value)
             );
             CREATE INDEX IF NOT EXISTS idx_subject_tags_subject ON subject_tags(subject);",
        )?
    }
    if version < 10 {
        // Dedicated subjects table so subjects can exist independently of
        // file-tag assignments or entity properties.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS subjects (
                 name TEXT PRIMARY KEY
             );
             -- Seed from existing subject data so nothing is lost.
             INSERT OR IGNORE INTO subjects (name)
                 SELECT DISTINCT subject FROM file_tags   WHERE subject != '';
             INSERT OR IGNORE INTO subjects (name)
                 SELECT DISTINCT subject FROM subject_tags WHERE subject != '';",
        )?
    }
    if version < 11 {
        // Face detection results: bounding box, confidence, 512-dim embedding,
        // and optional subject assignment.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS face_detections (
                 id           INTEGER PRIMARY KEY,
                 file_id      INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                 x            INTEGER NOT NULL,
                 y            INTEGER NOT NULL,
                 w            INTEGER NOT NULL,
                 h            INTEGER NOT NULL,
                 confidence   REAL    NOT NULL,
                 embedding    BLOB,
                 subject_name TEXT,
                 detected_at  TEXT NOT NULL DEFAULT (datetime('now'))
             );
             CREATE INDEX IF NOT EXISTS idx_face_detections_file_id
                 ON face_detections(file_id);
             CREATE INDEX IF NOT EXISTS idx_face_detections_subject
                 ON face_detections(subject_name);",
        )?
    }
    if version < 12 {
        // Perceptual hash (dHash) for fast visual similarity search.
        // file_embeddings stores AI text-embedding vectors for semantic similarity.
        conn.execute_batch(
            "ALTER TABLE files ADD COLUMN phash TEXT;
             CREATE TABLE IF NOT EXISTS file_embeddings (
                 file_id    INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
                 model      TEXT NOT NULL DEFAULT '',
                 embedding  BLOB NOT NULL,
                 indexed_at TEXT NOT NULL DEFAULT (datetime('now'))
             );",
        )
        .ok(); // ok() because ALTER TABLE fails on fresh DBs that already have the column
        // Ensure the table exists even on fresh DBs.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS file_embeddings (
                 file_id    INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
                 model      TEXT NOT NULL DEFAULT '',
                 embedding  BLOB NOT NULL,
                 indexed_at TEXT NOT NULL DEFAULT (datetime('now'))
             );",
        )?;
    }
    if version < 13 {
        // Replace directional alias system with symmetric synonym groups.
        //
        // New tables:
        //   tag_groups  – the semantic group concept (id, created_at)
        //   tag_attrs   – per-tag attributes for ABAC display-name selection
        //
        // tags gets a `group_id` FK; all members with the same group_id are
        // synonyms of each other.  The old `tag_synonyms` alias→canonical
        // table is migrated and then dropped.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tag_groups (
                 id         INTEGER PRIMARY KEY,
                 created_at TEXT NOT NULL DEFAULT (datetime('now'))
             );
             CREATE TABLE IF NOT EXISTS tag_attrs (
                 tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
                 key    TEXT NOT NULL,
                 value  TEXT NOT NULL DEFAULT '',
                 PRIMARY KEY (tag_id, key)
             );",
        )?;
        // ALTER TABLE fails when the column already exists (fresh DB), so
        // we ignore the error.
        conn.execute_batch(
            "ALTER TABLE tags ADD COLUMN group_id INTEGER REFERENCES tag_groups(id);",
        )
        .ok();

        // Migrate existing tag_synonyms rows into groups.
        // Each (alias, canonical_id) pair becomes a two-member group.
        // The alias is inserted as a real tag if it does not yet exist.
        let synonyms: Vec<(String, i64)> = {
            // tag_synonyms may not exist on a fresh DB that jumped straight
            // to v13 (it was only created in the v7 migration path).
            let exists: bool = conn
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type='table' AND name='tag_synonyms'",
                    [],
                    |_| Ok(true),
                )
                .unwrap_or(false);
            if exists {
                let mut stmt = conn
                    .prepare("SELECT alias, canonical_id FROM tag_synonyms")
                    .unwrap();
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
                    .unwrap()
                    .filter_map(|r| r.ok())
                    .collect()
            } else {
                Vec::new()
            }
        };

        for (alias, canonical_id) in synonyms {
            // Create a new group.
            conn.execute(
                "INSERT INTO tag_groups (created_at) VALUES (datetime('now'))",
                [],
            )?;
            let group_id = conn.last_insert_rowid();

            // Assign the canonical tag to this group.
            conn.execute(
                "UPDATE tags SET group_id = ?1 WHERE id = ?2 AND group_id IS NULL",
                params![group_id, canonical_id],
            )?;

            // Create the alias as a real tag if needed, then assign it to the group.
            let alias_id: i64 = conn
                .query_row(
                    "SELECT id FROM tags WHERE name = ?1",
                    params![&alias],
                    |r| r.get(0),
                )
                .unwrap_or_else(|_| {
                    conn.execute("INSERT INTO tags (name) VALUES (?1)", params![&alias])
                        .unwrap();
                    conn.last_insert_rowid()
                });
            conn.execute(
                "UPDATE tags SET group_id = ?1 WHERE id = ?2 AND group_id IS NULL",
                params![group_id, alias_id],
            )?;
        }

        conn.execute_batch("DROP TABLE IF EXISTS tag_synonyms;")?;
    }

    if version < 14 {
        // Add db_id column to linked_databases so we can verify identity when
        // a database tree is re-mounted at a different prefix.
        conn.execute_batch("ALTER TABLE linked_databases ADD COLUMN db_id TEXT;")
            .ok(); // ok(): fresh DBs already have the column from the CREATE TABLE below

        // Generate a unique ID for this database if it does not have one yet
        // (all existing databases being migrated from an earlier version).
        let already_set: bool = conn
            .query_row("SELECT 1 FROM settings WHERE key = 'db_id'", [], |_| {
                Ok(true)
            })
            .unwrap_or(false);
        if !already_set {
            let id = generate_db_id();
            conn.execute(
                "INSERT INTO settings (key, value) VALUES ('db_id', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![id],
            )?;
        }
    }

    if version < 15 {
        // Migrate the legacy `ai.prompt` setting (which used to act as the
        // image prompt) to the canonical `ai.prompt_image` key, then remove
        // the old key so no runtime fallback code is needed.
        use rusqlite::OptionalExtension;
        let old_prompt: Option<String> = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'ai.prompt'",
                [],
                |r| r.get(0),
            )
            .optional()
            .unwrap_or(None);
        let new_exists: bool = conn
            .query_row(
                "SELECT 1 FROM settings WHERE key = 'ai.prompt_image'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        if let Some(v) = old_prompt {
            if !new_exists {
                conn.execute(
                    "INSERT INTO settings (key, value) VALUES ('ai.prompt_image', ?1)",
                    params![v],
                )?;
            }
            conn.execute("DELETE FROM settings WHERE key = 'ai.prompt'", [])?;
        }
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
    /// Primary key.
    pub id: i64,
    /// Path relative to the database root.
    #[allow(dead_code)]
    pub path: String,
    /// Platform file identity string (`"dev:ino"` on Unix, `None` on Windows).
    pub file_id: Option<String>,
    /// File size in bytes at the time of last indexing.
    pub size: i64,
    /// Last-modified time as nanoseconds since the Unix epoch.
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
        match mt.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_nanos() as i64,
            Err(_) => 0,
        }
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

// ---------------------------------------------------------------------------
// Synonym groups
// ---------------------------------------------------------------------------

/// Link two or more tag names into a synonym group.
///
/// All names are created as real tags if they do not yet exist.  If any name
/// is already in a group those groups are merged together.
pub fn link_synonyms(conn: &Connection, names: &[&str]) -> Result<()> {
    if names.len() < 2 {
        bail!("need at least two names to link as synonyms");
    }

    // Collect (tag_id, Option<group_id>) for each name.
    let mut tag_ids: Vec<i64> = Vec::new();
    let mut group_ids: Vec<Option<i64>> = Vec::new();
    for &name in names {
        let id = get_or_create_tag(conn, name)?;
        let gid: Option<i64> = conn
            .query_row(
                "SELECT group_id FROM tags WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        tag_ids.push(id);
        group_ids.push(gid);
    }

    // Collect distinct existing groups (deduplicated).
    let distinct: Vec<i64> = {
        let mut seen = std::collections::HashSet::new();
        group_ids
            .iter()
            .filter_map(|&g| g)
            .filter(|&g| seen.insert(g))
            .collect()
    };

    let group_id: i64 = if distinct.is_empty() {
        // No existing group — create a fresh one.
        conn.execute(
            "INSERT INTO tag_groups (created_at) VALUES (datetime('now'))",
            [],
        )?;
        conn.last_insert_rowid()
    } else {
        // Use the first existing group; merge any additional groups into it.
        let primary = distinct[0];
        for &other in &distinct[1..] {
            conn.execute(
                "UPDATE tags SET group_id = ?1 WHERE group_id = ?2",
                params![primary, other],
            )?;
            conn.execute("DELETE FROM tag_groups WHERE id = ?1", params![other])?;
        }
        primary
    };

    // Assign all tags to the group.
    for &id in &tag_ids {
        conn.execute(
            "UPDATE tags SET group_id = ?1 WHERE id = ?2",
            params![group_id, id],
        )?;
    }
    Ok(())
}

/// Remove `name` from its synonym group.
///
/// If the group would have only one member left after removal, that member is
/// also ungrouped and the empty group is deleted.
pub fn unlink_synonym(conn: &Connection, name: &str) -> Result<()> {
    let (tag_id, group_id): (i64, Option<i64>) = conn
        .query_row(
            "SELECT id, group_id FROM tags WHERE name = ?1",
            params![name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| anyhow::anyhow!("tag '{}' not found", name))?;

    let group_id = match group_id {
        Some(g) => g,
        None => bail!("tag '{}' is not in a synonym group", name),
    };

    // Remove this tag from the group.
    conn.execute(
        "UPDATE tags SET group_id = NULL WHERE id = ?1",
        params![tag_id],
    )?;

    // If only one member remains, ungroup it too and delete the group.
    let remaining: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tags WHERE group_id = ?1",
        params![group_id],
        |r| r.get(0),
    )?;
    if remaining <= 1 {
        conn.execute(
            "UPDATE tags SET group_id = NULL WHERE group_id = ?1",
            params![group_id],
        )?;
        conn.execute("DELETE FROM tag_groups WHERE id = ?1", params![group_id])?;
    }
    Ok(())
}

/// Return all members of the synonym group that contains `name`, including
/// `name` itself.  Returns `[name]` when the tag is not in any group.
pub fn synonym_group_members(conn: &Connection, name: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT t2.name
         FROM tags t1
         JOIN tags t2 ON t2.group_id = t1.group_id
         WHERE t1.name = ?1 AND t1.group_id IS NOT NULL
         ORDER BY t2.name",
    )?;
    let rows = stmt.query_map(params![name], |r| r.get::<_, String>(0))?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    if result.is_empty() {
        result.push(name.to_string());
    }
    Ok(result)
}

/// Register `alias` as a synonym for `canonical` (backward-compatible wrapper).
///
/// Both names become real tags and are linked into the same group.
pub fn add_synonym(conn: &Connection, alias: &str, canonical: &str) -> Result<()> {
    link_synonyms(conn, &[alias, canonical])
}

/// Remove a synonym.  Returns `false` if the tag was not in a group.
pub fn remove_synonym(conn: &Connection, alias: &str) -> Result<bool> {
    match unlink_synonym(conn, alias) {
        Ok(()) => Ok(true),
        Err(e) if e.to_string().contains("not in a synonym group") => Ok(false),
        Err(e) => Err(e),
    }
}

/// Return all registered synonym groups as `(member_a, member_b, ...)` tuples,
/// ordered by group id.
pub fn list_synonyms(conn: &Connection) -> Result<Vec<Vec<String>>> {
    let mut stmt = conn.prepare(
        "SELECT tg.id, t.name
         FROM tag_groups tg
         JOIN tags t ON t.group_id = tg.id
         ORDER BY tg.id, t.name",
    )?;
    let mut groups: std::collections::BTreeMap<i64, Vec<String>> =
        std::collections::BTreeMap::new();
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (gid, name) = row?;
        groups.entry(gid).or_default().push(name);
    }
    Ok(groups.into_values().collect())
}

/// Return all other members of the synonym group containing `name`
/// (i.e. every member except `name` itself).
pub fn synonyms_for_tag(conn: &Connection, name: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT t2.name
         FROM tags t1
         JOIN tags t2 ON t2.group_id = t1.group_id AND t2.name != t1.name
         WHERE t1.name = ?1 AND t1.group_id IS NOT NULL
         ORDER BY t2.name",
    )?;
    let rows = stmt.query_map(params![name], |r| r.get::<_, String>(0))?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// Tag attributes
// ---------------------------------------------------------------------------

/// Set (or overwrite) an attribute on the tag named `name`.
///
/// The tag is created if it does not yet exist.  Attributes are arbitrary
/// key=value pairs; they are used for ABAC-style display-name selection.
pub fn set_tag_attr(conn: &Connection, name: &str, key: &str, value: &str) -> Result<()> {
    let tag_id = get_or_create_tag(conn, name)?;
    conn.execute(
        "INSERT OR REPLACE INTO tag_attrs (tag_id, key, value) VALUES (?1, ?2, ?3)",
        params![tag_id, key, value],
    )?;
    Ok(())
}

/// Remove an attribute from the tag named `name`.  Returns `false` if the
/// attribute did not exist.
pub fn remove_tag_attr(conn: &Connection, name: &str, key: &str) -> Result<bool> {
    let tag_id: Option<i64> = conn
        .query_row("SELECT id FROM tags WHERE name = ?1", params![name], |r| {
            r.get(0)
        })
        .ok();
    let tag_id = match tag_id {
        Some(id) => id,
        None => return Ok(false),
    };
    let changed = conn.execute(
        "DELETE FROM tag_attrs WHERE tag_id = ?1 AND key = ?2",
        params![tag_id, key],
    )?;
    Ok(changed > 0)
}

/// Return all attributes for the tag named `name` as `(key, value)` pairs.
pub fn get_tag_attrs(conn: &Connection, name: &str) -> Result<Vec<(String, String)>> {
    let tag_id: Option<i64> = conn
        .query_row("SELECT id FROM tags WHERE name = ?1", params![name], |r| {
            r.get(0)
        })
        .ok();
    let tag_id = match tag_id {
        Some(id) => id,
        None => return Ok(Vec::new()),
    };
    let mut stmt =
        conn.prepare("SELECT key, value FROM tag_attrs WHERE tag_id = ?1 ORDER BY key")?;
    let rows = stmt.query_map(params![tag_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// Display context
// ---------------------------------------------------------------------------

/// Return the current display context stored in `settings`.
///
/// The context is a flat map of key→value pairs used to select the preferred
/// display name from a synonym group.  Stored as a comma-separated
/// `key=value,key=value` string under the settings key `"display_context"`.
pub fn get_display_context(conn: &Connection) -> Result<HashMap<String, String>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'display_context'",
            [],
            |r| r.get(0),
        )
        .ok();
    let mut map = HashMap::new();
    if let Some(s) = raw {
        for part in s.split(',') {
            if let Some((k, v)) = part.split_once('=') {
                map.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    Ok(map)
}

/// Store the display context in `settings`.
///
/// Keys and values must not contain `=` or `,`.
pub fn set_display_context(conn: &Connection, context: &HashMap<String, String>) -> Result<()> {
    let serialised: String = context
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join(",");
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES ('display_context', ?1)",
        params![serialised],
    )?;
    Ok(())
}

/// Apply a tag (with optional value and optional subject) to a file.
/// When a non-empty subject is given, it is also registered in the `subjects` table.
pub fn apply_tag(
    conn: &Connection,
    file_id: i64,
    tag_id: i64,
    value: Option<&str>,
    subject: Option<&str>,
) -> Result<()> {
    if let Some(subject) = subject
        && !subject.is_empty()
    {
        create_subject(conn, subject)?;
    }
    conn.execute(
        "INSERT OR IGNORE INTO file_tags (file_id, tag_id, value, subject) VALUES (?1, ?2, ?3, ?4)",
        params![file_id, tag_id, value.unwrap_or(""), subject.unwrap_or("")],
    )?;
    Ok(())
}

/// Remove a tag (with optional value and optional subject) from a file.
///
/// When `subject` is `None` the operation applies to all rows matching the
/// (file_id, tag_id[, value]) constraint regardless of subject, preserving
/// backward compatibility.  Pass `Some("")` to target only unsubjectted rows.
pub fn remove_tag(
    conn: &Connection,
    file_id: i64,
    tag_id: i64,
    value: Option<&str>,
    subject: Option<&str>,
) -> Result<bool> {
    let changed = match (value, subject) {
        (Some(v), Some(s)) => conn.execute(
            "DELETE FROM file_tags WHERE file_id = ?1 AND tag_id = ?2 AND value = ?3 AND subject = ?4",
            params![file_id, tag_id, v, s],
        )?,
        (Some(v), None) => conn.execute(
            "DELETE FROM file_tags WHERE file_id = ?1 AND tag_id = ?2 AND value = ?3",
            params![file_id, tag_id, v],
        )?,
        (None, Some(s)) => conn.execute(
            "DELETE FROM file_tags WHERE file_id = ?1 AND tag_id = ?2 AND subject = ?3",
            params![file_id, tag_id, s],
        )?,
        (None, None) => conn.execute(
            "DELETE FROM file_tags WHERE file_id = ?1 AND tag_id = ?2",
            params![file_id, tag_id],
        )?,
    };
    Ok(changed > 0)
}

/// List all tags on a file, returned as `(tag_name, Option<value>)`.
///
/// Subject information is stripped; use [`tags_for_file_with_subject`] when
/// subject grouping is needed.
pub fn tags_for_file(conn: &Connection, file_id: i64) -> Result<Vec<(String, Option<String>)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT t.name, ft.value
         FROM file_tags ft
         JOIN tags t ON t.id = ft.tag_id
         WHERE ft.file_id = ?1
         ORDER BY ft.subject, t.name, ft.value",
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

/// List all tags on a file including their subject group.
///
/// Returns `(tag_name, Option<value>, subject)` triples. The `subject` field
/// is an empty string for tags that were applied without a subject.
pub fn tags_for_file_with_subject(
    conn: &Connection,
    file_id: i64,
) -> Result<Vec<(String, Option<String>, String)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT t.name, ft.value, ft.subject
         FROM file_tags ft
         JOIN tags t ON t.id = ft.tag_id
         WHERE ft.file_id = ?1
         ORDER BY ft.subject, t.name, ft.value",
    )?;
    let rows = stmt.query_map(params![file_id], |row| {
        let name: String = row.get(0)?;
        let value: String = row.get(1)?;
        let subject: String = row.get(2)?;
        let value = if value.is_empty() { None } else { Some(value) };
        Ok((name, value, subject))
    })?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// A single tag as returned by [`all_tags`]: (name, count, color, has_values).
pub type TagRow = (String, i64, Option<String>, bool);

/// List all known tags (with usage count).
pub fn all_tags(conn: &Connection) -> Result<Vec<TagRow>> {
    let mut stmt = conn.prepare(
        "SELECT t.name, COUNT(ft.file_id), t.color,
                MAX(CASE WHEN ft.value IS NOT NULL AND ft.value != '' THEN 1 ELSE 0 END) AS has_values
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
            row.get::<_, bool>(3)?,
        ))
    })?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// Return all distinct non-empty values for a given tag, with per-value file counts.
/// Results are ordered by count descending, then by value.
pub fn tag_values(conn: &Connection, name: &str) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT ft.value, COUNT(DISTINCT ft.file_id)
         FROM file_tags ft
         JOIN tags t ON t.id = ft.tag_id
         WHERE t.name = ?1 AND ft.value != ''
         GROUP BY ft.value
         ORDER BY COUNT(DISTINCT ft.file_id) DESC, ft.value",
    )?;
    let rows = stmt.query_map(params![name], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// List all distinct non-empty subjects with the number of files they appear on.
/// Includes subjects that exist only in `subject_tags` (zero file count).
/// Files are counted from both `file_tags.subject` and `face_detections.subject_name`
/// so that named face persons reflect the correct file count.
/// Results are ordered alphabetically by subject name.
pub fn all_subjects(conn: &Connection) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT s.name,
                (
                    SELECT COUNT(DISTINCT src.file_id)
                    FROM (
                        SELECT file_id FROM file_tags WHERE subject = s.name
                        UNION
                        SELECT file_id FROM face_detections WHERE subject_name = s.name
                    ) AS src
                ) AS cnt
         FROM subjects s
         ORDER BY s.name",
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

/// Create a new subject entry (with no files and no properties yet).
/// Returns `true` if newly created, `false` if it already existed.
pub fn create_subject(conn: &Connection, name: &str) -> Result<bool> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO subjects (name) VALUES (?1)",
        params![name],
    )?;
    Ok(n > 0)
}

/// Rename a subject label across all file-tag assignments, entity properties,
/// and face detection records.  Returns the number of rows updated.
pub fn rename_subject(conn: &Connection, old_name: &str, new_name: &str) -> Result<usize> {
    conn.execute(
        "INSERT OR IGNORE INTO subjects (name) VALUES (?1)",
        params![new_name],
    )?;
    conn.execute(
        "UPDATE subject_tags SET subject = ?1 WHERE subject = ?2",
        params![new_name, old_name],
    )?;
    let n = conn.execute(
        "UPDATE file_tags SET subject = ?1 WHERE subject = ?2",
        params![new_name, old_name],
    )?;
    // Keep face detections in sync.
    conn.execute(
        "UPDATE face_detections SET subject_name = ?1 WHERE subject_name = ?2",
        params![new_name, old_name],
    )?;
    conn.execute("DELETE FROM subjects WHERE name = ?1", params![old_name])?;
    Ok(n)
}

/// Delete a subject: remove from the subjects registry, clear all file-tag
/// assignments (sets subject to ''), drop all entity properties, and clear
/// face detection assignments for that subject.
/// Returns the number of file_tags rows cleared.
pub fn delete_subject(conn: &Connection, name: &str) -> Result<usize> {
    conn.execute("DELETE FROM subject_tags WHERE subject = ?1", params![name])?;
    conn.execute("DELETE FROM subjects WHERE name = ?1", params![name])?;
    // Clear face detection assignments.
    conn.execute(
        "UPDATE face_detections SET subject_name = NULL WHERE subject_name = ?1",
        params![name],
    )?;
    let n = conn.execute(
        "UPDATE file_tags SET subject = '' WHERE subject = ?1",
        params![name],
    )?;
    Ok(n)
}

/// Assign a file to a subject by adding/reusing a tag with the same name as the
/// subject and applying it under that subject. Returns the number of rows added.
pub fn assign_file_to_subject(
    conn: &Connection,
    file_id: i64,
    subject_name: &str,
) -> Result<usize> {
    create_subject(conn, subject_name)?;
    let tag_id = get_or_create_tag(conn, subject_name)?;
    let n = conn.execute(
        "INSERT OR IGNORE INTO file_tags (file_id, tag_id, value, subject) VALUES (?1, ?2, '', ?3)",
        params![file_id, tag_id, subject_name],
    )?;
    Ok(n)
}

/// Reassign an existing bare tag row for `tag_name` on `file_id` to `subject`.
/// Returns the number of rows updated.
pub fn reassign_file_tag_to_subject(
    conn: &Connection,
    file_id: i64,
    tag_name: &str,
    subject: &str,
) -> Result<usize> {
    create_subject(conn, subject)?;
    let n = conn.execute(
        "UPDATE OR IGNORE file_tags
         SET subject = ?1
         WHERE file_id = ?2
           AND tag_id = (SELECT id FROM tags WHERE name = ?3)
           AND subject = ''",
        params![subject, file_id, tag_name],
    )?;
    let deleted = conn.execute(
        "DELETE FROM file_tags
         WHERE file_id = ?1
           AND tag_id = (SELECT id FROM tags WHERE name = ?2)
           AND subject = ''",
        params![file_id, tag_name],
    )?;
    Ok(n + deleted)
}

/// Clone a subject: insert copies of all file_tags and subject_tags rows for
/// `old_name` under `new_name`.  Rows that already exist are silently skipped.
/// Returns the number of file_tags rows inserted.
pub fn clone_subject(conn: &Connection, old_name: &str, new_name: &str) -> Result<usize> {
    conn.execute(
        "INSERT OR IGNORE INTO subjects (name) VALUES (?1)",
        params![new_name],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO subject_tags (subject, tag_id, value, created_at) \
         SELECT ?1, tag_id, value, created_at FROM subject_tags WHERE subject = ?2",
        params![new_name, old_name],
    )?;
    let n = conn.execute(
        "INSERT OR IGNORE INTO file_tags (file_id, tag_id, value, subject, created_at) \
         SELECT file_id, tag_id, value, ?1, created_at FROM file_tags WHERE subject = ?2",
        params![new_name, old_name],
    )?;
    Ok(n)
}

// ---------------------------------------------------------------------------
// Subject entity properties (subject_tags)
// ---------------------------------------------------------------------------

/// For every subject this file belongs to, return the subject's own tags
/// as `(subject, tag_name, value)` tuples.  These are "implicit" tags —
/// they describe the subject entity, not the file directly.
pub fn subject_props_for_file(
    conn: &Connection,
    file_id: i64,
) -> Result<Vec<(String, String, String)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT ft.subject, t.name, st.value \
         FROM file_tags ft \
         JOIN subject_tags st ON st.subject = ft.subject \
         JOIN tags t ON t.id = st.tag_id \
         WHERE ft.file_id = ?1 AND ft.subject != '' \
         ORDER BY ft.subject, t.name, st.value",
    )?;
    let rows = stmt.query_map(params![file_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// Return all properties of a subject entity as (tag_name, value) pairs,
/// ordered by tag name then value.
pub fn get_subject_props(conn: &Connection, subject: &str) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT t.name, st.value \
         FROM subject_tags st \
         JOIN tags t ON t.id = st.tag_id \
         WHERE st.subject = ?1 \
         ORDER BY t.name, st.value",
    )?;
    let rows = stmt.query_map(params![subject], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// Return file-level tags that are assigned under a subject as `(tag_name, count)`.
pub fn subject_file_tags(conn: &Connection, subject: &str) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT t.name, COUNT(DISTINCT ft.file_id)
         FROM file_tags ft
         JOIN tags t ON t.id = ft.tag_id
         WHERE ft.subject = ?1
         GROUP BY t.id
         ORDER BY t.name",
    )?;
    let rows = stmt.query_map(params![subject], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// Add `tag_name` to every file currently belonging to `subject`.
/// The added rows remain scoped to the same subject.
pub fn add_tag_to_subject_files(conn: &Connection, subject: &str, tag_name: &str) -> Result<usize> {
    create_subject(conn, subject)?;
    let tag_id = get_or_create_tag(conn, tag_name)?;
    let n = conn.execute(
        "INSERT OR IGNORE INTO file_tags (file_id, tag_id, value, subject)
         SELECT DISTINCT file_id, ?1, '', ?2
         FROM file_tags
         WHERE subject = ?2",
        params![tag_id, subject],
    )?;
    Ok(n)
}

/// Remove `tag_name` from all files where it is assigned under `subject`.
pub fn remove_tag_from_subject_files(
    conn: &Connection,
    subject: &str,
    tag_name: &str,
) -> Result<usize> {
    let n = conn.execute(
        "DELETE FROM file_tags
         WHERE subject = ?1
           AND tag_id = (SELECT id FROM tags WHERE name = ?2)",
        params![subject, tag_name],
    )?;
    Ok(n)
}

/// Add (or silently ignore if already present) a property to a subject entity.
/// Also ensures the subject is registered in the `subjects` table.
/// Returns the number of rows inserted (0 or 1).
pub fn set_subject_prop(
    conn: &Connection,
    subject: &str,
    tag_name: &str,
    value: &str,
) -> Result<usize> {
    conn.execute(
        "INSERT OR IGNORE INTO subjects (name) VALUES (?1)",
        params![subject],
    )?;
    let tag_id = get_or_create_tag(conn, tag_name)?;
    let n = conn.execute(
        "INSERT OR IGNORE INTO subject_tags (subject, tag_id, value) VALUES (?1, ?2, ?3)",
        params![subject, tag_id, value],
    )?;
    Ok(n)
}

/// Remove a specific property from a subject entity.
/// If `value` is `None`, all rows for that (subject, tag) are deleted.
/// Returns the number of rows deleted.
pub fn remove_subject_prop(
    conn: &Connection,
    subject: &str,
    tag_name: &str,
    value: Option<&str>,
) -> Result<usize> {
    let n = match value {
        Some(v) => conn.execute(
            "DELETE FROM subject_tags WHERE subject = ?1 \
             AND tag_id = (SELECT id FROM tags WHERE name = ?2) \
             AND value = ?3",
            params![subject, tag_name, v],
        )?,
        None => conn.execute(
            "DELETE FROM subject_tags WHERE subject = ?1 \
             AND tag_id = (SELECT id FROM tags WHERE name = ?2)",
            params![subject, tag_name],
        )?,
    };
    Ok(n)
}

/// Set or clear the color for a tag.
pub fn set_tag_color(conn: &Connection, name: &str, color: Option<&str>) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE tags SET color = ?1 WHERE name = ?2",
        params![color, name],
    )?;
    Ok(changed > 0)
}

/// Outcome of a [`rename_tag`] operation.
pub enum RenameOutcome {
    /// The tag was simply renamed in-place.
    Renamed,
    /// The source tag was merged: all its file-tag assignments were moved to the
    /// target (or converted to the given key=value form). `assignments` is the
    /// number of file_tags rows that were inserted.
    Merged { assignments: usize },
    /// The source tag was not found.
    NotFound,
}

/// Rename a tag.
///
/// * If `name` is a plain tag name and `new_name` does not yet exist, the tag
///   is renamed in-place.
/// * If `name` is a plain tag name and `new_name` already exists, the source is
///   merged into it: all file-tag assignments are moved and the source tag is
///   deleted.
/// * If `name` is a plain tag name and `new_name` contains `=` (e.g.
///   `rating=5`), every file that held the source tag is assigned `key=value`
///   instead; the source tag is deleted.
/// * If `name` contains `=` (e.g. `year=2024`), only the assignments for that
///   specific value are updated. The target may be another `key=value` pair
///   (including a different key) or a plain tag name. The source key tag is
///   kept even if it ends up with no remaining assignments.
pub fn rename_tag(conn: &Connection, name: &str, new_name: &str) -> Result<RenameOutcome> {
    // Guard against renaming to itself.
    if name == new_name {
        return Ok(RenameOutcome::Renamed);
    }

    // --- source is a key=value assignment (e.g. "year=2024") -------------------
    if let Some(from_eq) = name.find('=') {
        let from_key = &name[..from_eq];
        let from_value = &name[from_eq + 1..];

        let from_id: Option<i64> = conn
            .prepare_cached("SELECT id FROM tags WHERE name = ?1")?
            .query_row(params![from_key], |r| r.get(0))
            .ok();
        let Some(from_id) = from_id else {
            return Ok(RenameOutcome::NotFound);
        };

        // Verify at least one assignment exists for this specific value.
        let has_assignments: bool = conn
            .prepare_cached("SELECT 1 FROM file_tags WHERE tag_id = ?1 AND value = ?2 LIMIT 1")?
            .query_row(params![from_id, from_value], |_| Ok(true))
            .unwrap_or(false);
        if !has_assignments {
            return Ok(RenameOutcome::NotFound);
        }

        // Resolve the target key and value.
        let (to_id, to_value) = if let Some(to_eq) = new_name.find('=') {
            let to_key = &new_name[..to_eq];
            let to_val = &new_name[to_eq + 1..];
            (get_or_create_tag(conn, to_key)?, to_val.to_owned())
        } else {
            (get_or_create_tag(conn, new_name)?, String::new())
        };

        let moved = conn.execute(
            "INSERT OR IGNORE INTO file_tags (file_id, tag_id, value, subject, created_at) \
             SELECT file_id, ?1, ?2, subject, created_at FROM file_tags \
             WHERE tag_id = ?3 AND value = ?4",
            params![to_id, to_value, from_id, from_value],
        )?;
        conn.execute(
            "DELETE FROM file_tags WHERE tag_id = ?1 AND value = ?2",
            params![from_id, from_value],
        )?;
        return Ok(RenameOutcome::Merged { assignments: moved });
    }

    // --- source is a plain tag name ---------------------------------------------
    let from_id: Option<i64> = conn
        .prepare_cached("SELECT id FROM tags WHERE name = ?1")?
        .query_row(params![name], |r| r.get(0))
        .ok();
    let Some(from_id) = from_id else {
        return Ok(RenameOutcome::NotFound);
    };

    // --- key=value target: convert all source assignments to key=value ----------
    if let Some(eq_pos) = new_name.find('=') {
        let key = &new_name[..eq_pos];
        let value = &new_name[eq_pos + 1..];
        let key_id = get_or_create_tag(conn, key)?;
        // One row per (file, subject) combination (files may have multiple source values;
        // all collapse to the single target value).
        let moved = conn.execute(
            "INSERT OR IGNORE INTO file_tags (file_id, tag_id, value, subject, created_at) \
             SELECT file_id, ?1, ?2, subject, MIN(created_at) \
             FROM file_tags WHERE tag_id = ?3 GROUP BY file_id, subject",
            params![key_id, value, from_id],
        )?;
        conn.execute("DELETE FROM file_tags WHERE tag_id = ?1", params![from_id])?;
        // Migrate subject entity properties to the target tag before the cascade
        // delete removes them.
        if key_id != from_id {
            conn.execute(
                "INSERT OR IGNORE INTO subject_tags (subject, tag_id, value, created_at) \
                 SELECT subject, ?1, value, created_at FROM subject_tags WHERE tag_id = ?2",
                params![key_id, from_id],
            )?;
        }
        conn.execute("DELETE FROM tags WHERE id = ?1", params![from_id])?;
        return Ok(RenameOutcome::Merged { assignments: moved });
    }

    // --- plain rename or merge --------------------------------------------------
    let to_id: Option<i64> = conn
        .prepare_cached("SELECT id FROM tags WHERE name = ?1")?
        .query_row(params![new_name], |r| r.get(0))
        .ok();
    if let Some(to_id) = to_id {
        // Target already exists: merge all source assignments into it.
        let moved = conn.execute(
            "INSERT OR IGNORE INTO file_tags (file_id, tag_id, value, subject, created_at) \
             SELECT file_id, ?1, value, subject, created_at FROM file_tags WHERE tag_id = ?2",
            params![to_id, from_id],
        )?;
        conn.execute("DELETE FROM file_tags WHERE tag_id = ?1", params![from_id])?;
        // Migrate subject entity properties to the surviving tag before the
        // cascade delete removes them from subject_tags.
        conn.execute(
            "INSERT OR IGNORE INTO subject_tags (subject, tag_id, value, created_at) \
             SELECT subject, ?1, value, created_at FROM subject_tags WHERE tag_id = ?2",
            params![to_id, from_id],
        )?;
        conn.execute("DELETE FROM tags WHERE id = ?1", params![from_id])?;
        return Ok(RenameOutcome::Merged { assignments: moved });
    }

    // Simple rename in-place.
    conn.execute(
        "UPDATE tags SET name = ?1 WHERE id = ?2",
        params![new_name, from_id],
    )?;
    Ok(RenameOutcome::Renamed)
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

/// Delete all tags that have no file assignments.
/// Returns the number of tags removed.
pub fn prune_unused_tags(conn: &Connection) -> Result<usize> {
    let n = conn.execute(
        "DELETE FROM tags \
         WHERE id NOT IN (SELECT DISTINCT tag_id FROM file_tags) \
           AND id NOT IN (SELECT DISTINCT tag_id FROM subject_tags)",
        [],
    )?;
    Ok(n)
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

/// A registered linked database entry.
#[derive(Debug, Clone)]
pub struct LinkedDb {
    /// Relative path from the parent root to the linked root directory (may
    /// contain `../` components for partner databases outside the root).
    pub path: String,
    /// UUID v4 of the linked database at the time it was registered, or
    /// `None` for legacy entries registered before schema v14.
    pub db_id: Option<String>,
}

/// Register a linked database, storing its path and unique ID so the link can
/// be verified even when the filesystem tree is re-mounted at a different
/// prefix.
pub fn link_database(conn: &Connection, path: &str, db_id: Option<&str>) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO linked_databases (path, db_id) VALUES (?1, ?2)",
        params![path, db_id],
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

/// Update the stored path for a linked database entry (e.g. absolute → relative).
pub fn set_linked_path(conn: &Connection, old_path: &str, new_path: &str) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE linked_databases SET path = ?1 WHERE path = ?2",
        params![new_path, old_path],
    )?;
    Ok(changed > 0)
}

/// Update (or set for the first time) the stored db_id for a linked entry.
pub fn set_linked_db_id(conn: &Connection, path: &str, db_id: &str) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE linked_databases SET db_id = ?1 WHERE path = ?2",
        params![db_id, path],
    )?;
    Ok(changed > 0)
}

/// List all registered linked databases with their stored paths and IDs.
pub fn list_linked(conn: &Connection) -> Result<Vec<LinkedDb>> {
    let mut stmt = conn.prepare("SELECT path, db_id FROM linked_databases ORDER BY path")?;
    let rows = stmt.query_map([], |row| {
        Ok(LinkedDb {
            path: row.get::<_, String>(0)?,
            db_id: row.get::<_, Option<String>>(1)?,
        })
    })?;
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
    /// (tag_name, value, subject) triples.
    pub tags: Vec<(String, String, String)>,
}

/// Collect tag (name, value, subject) triples for a file by its `files.id`.
fn collect_file_tags(
    tag_stmt: &mut rusqlite::Statement<'_>,
    file_id: i64,
) -> Vec<(String, String, String)> {
    tag_stmt
        .query_map(params![file_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
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
        "SELECT t.name, ft.value, ft.subject
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
        "SELECT t.name, ft.value, ft.subject
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

/// An opened database paired with its root directory.
pub struct OpenDb {
    /// Active SQLite connection.
    pub conn: Connection,
    /// Absolute path to the directory that contains `.filetag/`.
    pub root: PathBuf,
}

/// A loaded database root, combining the Root path with the metadata needed to
/// serve it over a session (web server, long-running tool, etc.).
///
/// Construct a `TagRoot` after locating a Root with [`find_root`] or
/// [`find_and_open`].  The struct itself carries no open connection; open one
/// on demand with [`crate::db`]'s connection helpers.
pub struct TagRoot {
    /// Display name for this root (user-facing, e.g. in the browser sidebar).
    pub name: String,
    /// Absolute path to the SQLite database file (`<root>/.filetag/db.sqlite3`).
    pub db_path: PathBuf,
    /// Absolute path to the directory that contains `.filetag/`.
    pub root: PathBuf,
    /// Volume/device identifier of the root directory. Used to detect
    /// filesystem boundary crossings when resolving which root covers a file.
    /// On Unix this is `st_dev`; on Windows the volume serial number.
    pub dev: Option<u64>,
    /// `true` when no other loaded root is a strict ancestor of this one.
    /// Entry-point roots are shown as top-level navigation items.
    pub entry_point: bool,
}

/// Collect this database and all reachable linked databases recursively.
///
/// Gracefully skips missing or broken databases. Uses cycle detection on
/// canonical root paths.
///
/// Linked paths may be relative (child, under current root) or absolute
/// (partner/parent, outside current root). `PathBuf::join` handles both:
/// joining with an absolute path replaces the base entirely.
///
/// When `include_ancestors` is `false`, automatic ancestor-database discovery
/// via parent directories is skipped; only explicit links are followed.
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
        for linked_db in linked {
            let linked_root = r.join(&linked_db.path);
            // Canonicaliseer de path om .. componenten op te lossen
            let linked_root = match std::fs::canonicalize(&linked_root) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!(
                        "warning: skipping linked database {}: {}",
                        linked_root.display(),
                        e
                    );
                    continue;
                }
            };
            let linked_db_path = linked_root.join(DB_DIR).join(DB_FILE);
            match open_at(&linked_db_path) {
                Ok(linked_conn) => {
                    // Run migration in case the linked DB is an older schema version
                    if migrate(&linked_conn).is_ok() {
                        // Verify the stored ID matches the actual database ID.
                        // A mismatch means a different database has been mounted at
                        // the same path — skip it and warn rather than silently
                        // mixing tags from the wrong source.
                        if let Some(expected) = &linked_db.db_id {
                            match get_db_id(&linked_conn) {
                                Ok(actual) if actual != *expected => {
                                    eprintln!(
                                        "warning: skipping linked database {}: \
                                         ID mismatch (expected {}, got {}). \
                                         Re-link with `filetag db remove` + \
                                         `filetag db add` to update the stored ID.",
                                        linked_db_path.display(),
                                        expected,
                                        actual
                                    );
                                    continue;
                                }
                                _ => {}
                            }
                        }
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

/// Recursively scan `root` for nested `.filetag/db.sqlite3` databases.
///
/// Returns one [`OpenDb`] per database found, skipping any path already in
/// `visited` (canonical paths, used for cycle detection).
///
/// - `max_depth` limits how deep the scan descends (10 is a sensible default).
/// - `on_dir` is called for every directory entered at depth ≥ 1, so callers
///   can show progress to the user.
///
/// Used by `filetag-web` at startup to discover all databases under the served
/// root, so every directory with its own database is included in the session
/// without requiring explicit `filetag db add` registration.
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

// ---------------------------------------------------------------------------
// Face detection CRUD
// ---------------------------------------------------------------------------

/// A single face detected in a file.
#[derive(Debug, Clone)]
pub struct FaceDetectionRow {
    /// Primary key.
    pub id: i64,
    /// Foreign key into `files`.
    pub file_id: i64,
    /// Bounding box, in pixels of the original image.
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// Detector confidence score (0.0–1.0).
    pub confidence: f32,
    /// 128 × f32 embedding stored as 512 little-endian bytes; `None` when the
    /// embedding step was skipped (e.g. face too small).
    pub embedding: Option<Vec<u8>>,
    /// Subject name assigned to this face, or `None` if not yet identified.
    pub subject_name: Option<String>,
}

/// Insert a new face detection record.  Returns the rowid of the new row.
#[allow(clippy::too_many_arguments)]
pub fn insert_face_detection(
    conn: &Connection,
    file_id: i64,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    confidence: f32,
    embedding: Option<&[u8]>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO face_detections (file_id, x, y, w, h, confidence, embedding)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![file_id, x, y, w, h, confidence, embedding],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Assign (or clear) the subject for a face detection.
///
/// Pass `subject_name = None` to clear the assignment.
pub fn set_face_subject(
    conn: &Connection,
    detection_id: i64,
    subject_name: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE face_detections SET subject_name = ?1 WHERE id = ?2",
        params![subject_name, detection_id],
    )?;
    Ok(())
}

/// Return all face detections for the file with the given `files.id`.
pub fn face_detections_for_file(conn: &Connection, file_id: i64) -> Result<Vec<FaceDetectionRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, file_id, x, y, w, h, confidence, embedding, subject_name
         FROM face_detections
         WHERE file_id = ?1
         ORDER BY id",
    )?;
    let rows = stmt.query_map(params![file_id], |r| {
        Ok(FaceDetectionRow {
            id: r.get(0)?,
            file_id: r.get(1)?,
            x: r.get(2)?,
            y: r.get(3)?,
            w: r.get(4)?,
            h: r.get(5)?,
            confidence: r.get(6)?,
            embedding: r.get(7)?,
            subject_name: r.get(8)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Return every face detection that has an embedding stored.
///
/// Used by the clustering step, which needs all embeddings at once.
pub fn all_face_detections_with_embeddings(conn: &Connection) -> Result<Vec<FaceDetectionRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, file_id, x, y, w, h, confidence, embedding, subject_name
         FROM face_detections
         WHERE embedding IS NOT NULL
         ORDER BY id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(FaceDetectionRow {
            id: r.get(0)?,
            file_id: r.get(1)?,
            x: r.get(2)?,
            y: r.get(3)?,
            w: r.get(4)?,
            h: r.get(5)?,
            confidence: r.get(6)?,
            embedding: r.get(7)?,
            subject_name: r.get(8)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Delete all face detections for a given `files.id`.
pub fn delete_face_detections_for_file(conn: &Connection, file_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM face_detections WHERE file_id = ?1",
        params![file_id],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// File similarity: pHash + text embeddings
// ---------------------------------------------------------------------------

/// Store a 64-bit dHash for a file (by internal file id).
pub fn store_phash(conn: &Connection, file_id: i64, phash: u64) -> Result<()> {
    conn.execute(
        "UPDATE files SET phash = ?1 WHERE id = ?2",
        params![format!("{:016x}", phash), file_id],
    )?;
    Ok(())
}

/// Return every `(file_id, rel_path, phash)` row that has a hash stored.
pub fn all_phashes(conn: &Connection) -> Result<Vec<(i64, String, u64)>> {
    let mut stmt = conn.prepare("SELECT id, path, phash FROM files WHERE phash IS NOT NULL")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, path, hex) = row?;
        if let Ok(h) = u64::from_str_radix(&hex, 16) {
            out.push((id, path, h));
        }
    }
    Ok(out)
}

/// Return the stored pHash (if any) for a file by relative path.
pub fn get_phash_by_path(conn: &Connection, rel_path: &str) -> Result<Option<(i64, u64)>> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT id, phash FROM files WHERE path = ?1 AND phash IS NOT NULL",
        params![rel_path],
        |row| {
            let id: i64 = row.get(0)?;
            let hex: String = row.get(1)?;
            Ok((id, hex))
        },
    )
    .optional()
    .map_err(anyhow::Error::from)?
    .map(|(id, hex): (i64, String)| {
        u64::from_str_radix(&hex, 16)
            .map(|h| (id, h))
            .map_err(|e| anyhow::anyhow!("invalid phash hex: {e}"))
    })
    .transpose()
}

/// Store a text-embedding vector for a file (keyed by file_id).
/// Embeddings are stored as little-endian f32 bytes.
pub fn store_embedding(
    conn: &Connection,
    file_id: i64,
    model: &str,
    embedding: &[f32],
) -> Result<()> {
    let bytes: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
    conn.execute(
        "INSERT OR REPLACE INTO file_embeddings (file_id, model, embedding) VALUES (?1, ?2, ?3)",
        params![file_id, model, bytes],
    )?;
    Ok(())
}

/// Return the stored embedding (if any) for a file by its internal id.
pub fn get_embedding(conn: &Connection, file_id: i64) -> Result<Option<(String, Vec<f32>)>> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT model, embedding FROM file_embeddings WHERE file_id = ?1",
        params![file_id],
        |row| {
            let model: String = row.get(0)?;
            let bytes: Vec<u8> = row.get(1)?;
            Ok((model, bytes))
        },
    )
    .optional()
    .map_err(anyhow::Error::from)?
    .map(|(model, bytes): (String, Vec<u8>)| {
        let floats: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        Ok::<_, anyhow::Error>((model, floats))
    })
    .transpose()
}

/// Return every `(file_id, rel_path, embedding)` row from `file_embeddings`.
pub fn all_embeddings(conn: &Connection) -> Result<Vec<(i64, String, Vec<f32>)>> {
    let mut stmt = conn.prepare(
        "SELECT fe.file_id, f.path, fe.embedding
         FROM file_embeddings fe
         JOIN files f ON f.id = fe.file_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Vec<u8>>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, path, bytes) = row?;
        let floats: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        out.push((id, path, floats));
    }
    Ok(out)
}

/// Return the internal `files.id` for a relative path, or `None` if not indexed.
pub fn file_id_by_path(conn: &Connection, rel_path: &str) -> Result<Option<i64>> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT id FROM files WHERE path = ?1",
        params![rel_path],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .map_err(Into::into)
}
