mod db;
mod query;
mod view;

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use serde::Serialize;

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "filetag",
    about = "SQLite-backed file tagging CLI",
    version,
    after_help = "See 'filetag help <command>' for more information on a specific command."
)]
struct Cli {
    /// Output format: JSON Lines (one object per line)
    #[arg(long, global = true)]
    json: bool,

    /// Color output
    #[arg(long, global = true, default_value = "auto")]
    color: ColorWhen,

    /// Suppress informational messages
    #[arg(short, long, global = true)]
    quiet: bool,

    /// Extra detail in output
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Use a specific database path (override auto-detect)
    #[arg(long, global = true)]
    db: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, ValueEnum)]
enum ColorWhen {
    Auto,
    Always,
    Never,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a new filetag database in the current directory
    Init,

    /// Add tags to files
    #[command(visible_alias = "t")]
    Tag {
        /// Files to tag (reads from stdin if omitted and stdin is a pipe)
        files: Vec<PathBuf>,

        /// Tags to apply, comma-separated (use key=value for values)
        #[arg(short, long, value_delimiter = ',', required = true)]
        tags: Vec<String>,

        /// Tag files recursively (treat arguments as directories)
        #[arg(short, long)]
        recursive: bool,

        /// Read NUL-delimited paths from stdin
        #[arg(short = '0', long)]
        null: bool,
    },

    /// Remove tags from files
    #[command(visible_alias = "u")]
    Untag {
        /// Files to untag (reads from stdin if omitted and stdin is a pipe)
        files: Vec<PathBuf>,

        /// Tags to remove, comma-separated
        #[arg(short, long, value_delimiter = ',', required = true)]
        tags: Vec<String>,

        /// Read NUL-delimited paths from stdin
        #[arg(short = '0', long)]
        null: bool,
    },

    /// List tags (all tags, or tags for specific files)
    #[command(visible_alias = "ls")]
    Tags {
        /// Show tags for specific files (omit for all tags)
        files: Vec<PathBuf>,
    },

    /// Show detailed file information
    #[command(visible_alias = "s")]
    Show {
        /// File to inspect
        file: PathBuf,
    },

    /// Find files matching a tag query
    #[command(visible_alias = "f")]
    Find {
        /// Tag query (e.g. "genre/rock and not live")
        query: Vec<String>,

        /// Show tags alongside file paths
        #[arg(long)]
        with_tags: bool,

        /// Only print the number of matches
        #[arg(short, long)]
        count: bool,

        /// NUL-delimited output (for xargs -0)
        #[arg(short = '0', long)]
        null: bool,
    },

    /// Generate a symlink view for a tag query
    View {
        /// Tag query
        query: Vec<String>,

        /// Output directory for symlinks
        #[arg(short, long, default_value = "_.tags")]
        output: PathBuf,
    },

    /// Show file status (missing, modified, untagged)
    Status {
        /// Path to check (default: entire database)
        path: Option<PathBuf>,
    },

    /// Find moved files by matching BLAKE3 content hashes
    Repair {
        /// Directory to scan (default: database root)
        path: Option<PathBuf>,

        /// Only show what would change, don't modify
        #[arg(short = 'n', long)]
        dry_run: bool,
    },

    /// Rename a tag
    Mv {
        /// Current tag name
        from: String,
        /// New tag name
        to: String,
    },

    /// Merge a tag into another (destructive: removes source tag)
    Merge {
        /// Source tag (will be removed)
        from: String,
        /// Target tag (will receive all assignments)
        into: String,

        /// Skip confirmation prompt
        #[arg(short, long)]
        force: bool,

        /// Only show what would change
        #[arg(short = 'n', long)]
        dry_run: bool,
    },

    /// Show database statistics
    Info,

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },
}

// ---------------------------------------------------------------------------
// JSON output types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct JsonTag {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<String>,
}

#[derive(Serialize)]
struct JsonTagCount {
    name: String,
    count: i64,
}

#[derive(Serialize)]
struct JsonFileTags {
    path: String,
    tags: Vec<JsonTag>,
}

#[derive(Serialize)]
struct JsonShowFile {
    path: String,
    size: i64,
    blake3: Option<String>,
    mtime: i64,
    indexed_at: String,
    tags: Vec<JsonTag>,
}

#[derive(Serialize)]
struct JsonInfo {
    root: String,
    files: i64,
    tags: i64,
    assignments: i64,
    total_size: i64,
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Command::Init => cmd_init(&cli),
        Command::Tag {
            files,
            tags,
            recursive,
            null,
        } => cmd_tag(&cli, files.clone(), tags.clone(), *recursive, *null),
        Command::Untag { files, tags, null } => cmd_untag(&cli, files.clone(), tags.clone(), *null),
        Command::Tags { files } => cmd_tags(&cli, files.clone()),
        Command::Show { file } => cmd_show(&cli, file.clone()),
        Command::Find {
            query,
            with_tags,
            count,
            null,
        } => cmd_find(&cli, query.clone(), *with_tags, *count, *null),
        Command::View { query, output } => cmd_view(&cli, query.clone(), output.clone()),
        Command::Status { path } => cmd_status(&cli, path.clone()),
        Command::Repair { path, dry_run } => cmd_repair(&cli, path.clone(), *dry_run),
        Command::Mv { from, to } => cmd_mv(&cli, from.clone(), to.clone()),
        Command::Merge {
            from,
            into,
            force,
            dry_run,
        } => cmd_merge(&cli, from.clone(), into.clone(), *force, *dry_run),
        Command::Info => cmd_info(&cli),
        Command::Completions { shell } => cmd_completions(*shell),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Open the database, respecting --db override.
fn open_db(cli: &Cli) -> Result<(rusqlite::Connection, PathBuf)> {
    if let Some(db_path) = &cli.db {
        let conn = db::init(db_path)?;
        let root = std::fs::canonicalize(db_path)?;
        Ok((conn, root))
    } else {
        let cwd = std::env::current_dir()?;
        db::find_and_open(&cwd)
    }
}

/// Collect file paths from arguments + stdin.
fn collect_files(files: Vec<PathBuf>, null: bool) -> Result<Vec<PathBuf>> {
    if !files.is_empty() {
        return Ok(files);
    }
    let stdin = io::stdin();
    if stdin.is_terminal() {
        return Ok(Vec::new());
    }
    read_paths_from_stdin(null)
}

/// Read paths from stdin, either newline- or NUL-delimited.
fn read_paths_from_stdin(null: bool) -> Result<Vec<PathBuf>> {
    let stdin = io::stdin();
    let mut paths = Vec::new();
    if null {
        let mut buf = Vec::new();
        io::Read::read_to_end(&mut stdin.lock(), &mut buf)?;
        for chunk in buf.split(|&b| b == 0) {
            if !chunk.is_empty() {
                let s = String::from_utf8_lossy(chunk);
                paths.push(PathBuf::from(s.as_ref()));
            }
        }
    } else {
        for line in stdin.lock().lines() {
            let line = line?;
            if !line.is_empty() {
                paths.push(PathBuf::from(line));
            }
        }
    }
    Ok(paths)
}

/// Parse tag arguments: "genre/rock" -> ("genre/rock", None), "year=2024" -> ("year", Some("2024"))
fn parse_tag_args(tags: &[String]) -> Vec<(String, Option<String>)> {
    tags.iter()
        .map(|t| {
            if let Some(eq_pos) = t.find('=') {
                let name = t[..eq_pos].to_string();
                let value = t[eq_pos + 1..].to_string();
                (name, Some(value))
            } else {
                (t.clone(), None)
            }
        })
        .collect()
}

/// Expand directory arguments recursively into file lists.
fn expand_recursive(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    for path in paths {
        if path.is_dir() {
            for entry in walkdir::WalkDir::new(path)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if entry.file_type().is_file() {
                    if entry
                        .path()
                        .components()
                        .any(|c| c.as_os_str() == ".filetag")
                    {
                        continue;
                    }
                    result.push(entry.into_path());
                }
            }
        } else {
            result.push(path.clone());
        }
    }
    Ok(result)
}

fn format_size(bytes: i64) -> String {
    let bytes = bytes as f64;
    if bytes < 1024.0 {
        return format!("{} B", bytes as i64);
    }
    let units = ["KiB", "MiB", "GiB", "TiB"];
    let mut size = bytes / 1024.0;
    for unit in &units {
        if size < 1024.0 {
            return format!("{:.1} {}", size, unit);
        }
        size /= 1024.0;
    }
    format!("{:.1} PiB", size)
}

fn format_tag(name: &str, value: &Option<String>) -> String {
    match value {
        Some(v) => format!("{}={}", name, v),
        None => name.to_string(),
    }
}

/// Print to stdout, silenced by --quiet.
macro_rules! info {
    ($cli:expr, $($arg:tt)*) => {
        if !$cli.quiet {
            println!($($arg)*);
        }
    };
}

// ---------------------------------------------------------------------------
// Subcommand implementations
// ---------------------------------------------------------------------------

fn cmd_init(_cli: &Cli) -> Result<()> {
    let cwd = std::env::current_dir()?;
    db::init(&cwd)?;
    println!("Initialized filetag database in {}", cwd.display());
    Ok(())
}

fn cmd_tag(cli: &Cli, files: Vec<PathBuf>, tags: Vec<String>, recursive: bool, null: bool) -> Result<()> {
    let (conn, root) = open_db(cli)?;
    let parsed_tags = parse_tag_args(&tags);

    let collected = collect_files(files, null)?;
    if collected.is_empty() {
        anyhow::bail!("no files specified (provide paths as arguments or pipe them via stdin)");
    }

    let file_paths = if recursive {
        expand_recursive(&collected)?
    } else {
        collected
    };

    let mut tagged_count = 0;
    for file_path in &file_paths {
        let rel = db::relative_to_root(file_path, &root)
            .with_context(|| format!("resolving {}", file_path.display()))?;
        let record = db::get_or_index_file(&conn, &rel, &root)?;

        for (tag_name, tag_value) in &parsed_tags {
            let tag_id = db::get_or_create_tag(&conn, tag_name)?;
            db::apply_tag(&conn, record.id, tag_id, tag_value.as_deref())?;
        }
        tagged_count += 1;
    }

    info!(cli, "Tagged {} file(s) with {} tag(s)", tagged_count, parsed_tags.len());
    Ok(())
}

fn cmd_untag(cli: &Cli, files: Vec<PathBuf>, tags: Vec<String>, null: bool) -> Result<()> {
    let (conn, root) = open_db(cli)?;
    let parsed_tags = parse_tag_args(&tags);

    let collected = collect_files(files, null)?;
    if collected.is_empty() {
        anyhow::bail!("no files specified (provide paths as arguments or pipe them via stdin)");
    }

    let mut removed_count = 0;
    for file_path in &collected {
        let rel = db::relative_to_root(file_path, &root)?;
        if let Some(record) = db::file_by_path(&conn, &rel)? {
            for (tag_name, tag_value) in &parsed_tags {
                if let Ok(tag_id) = conn.query_row(
                    "SELECT id FROM tags WHERE name = ?1",
                    rusqlite::params![tag_name],
                    |r| r.get::<_, i64>(0),
                ) {
                    if db::remove_tag(&conn, record.id, tag_id, tag_value.as_deref())? {
                        removed_count += 1;
                    }
                }
            }
        }
    }

    info!(cli, "Removed {} tag assignment(s)", removed_count);
    Ok(())
}

fn cmd_tags(cli: &Cli, files: Vec<PathBuf>) -> Result<()> {
    let (conn, root) = open_db(cli)?;

    if files.is_empty() {
        let tags = db::all_tags(&conn)?;
        if cli.json {
            for (name, count) in &tags {
                let j = JsonTagCount {
                    name: name.clone(),
                    count: *count,
                };
                println!("{}", serde_json::to_string(&j)?);
            }
        } else {
            if tags.is_empty() {
                info!(cli, "No tags defined");
                return Ok(());
            }
            let max_name_len = tags.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
            for (name, count) in &tags {
                println!("{:width$}  {}", name, count, width = max_name_len);
            }
        }
    } else {
        for file_path in &files {
            let rel = db::relative_to_root(file_path, &root)?;
            if let Some(record) = db::file_by_path(&conn, &rel)? {
                let tags = db::tags_for_file(&conn, record.id)?;
                if cli.json {
                    let j = JsonFileTags {
                        path: rel.clone(),
                        tags: tags
                            .iter()
                            .map(|(n, v)| JsonTag {
                                name: n.clone(),
                                value: v.clone(),
                            })
                            .collect(),
                    };
                    println!("{}", serde_json::to_string(&j)?);
                } else {
                    if files.len() > 1 {
                        print!("{}: ", rel);
                    }
                    let tag_strs: Vec<String> =
                        tags.iter().map(|(n, v)| format_tag(n, v)).collect();
                    println!("{}", tag_strs.join(" "));
                }
            } else {
                eprintln!("{}: not indexed", file_path.display());
            }
        }
    }
    Ok(())
}

fn cmd_show(cli: &Cli, file: PathBuf) -> Result<()> {
    let (conn, root) = open_db(cli)?;
    let rel = db::relative_to_root(&file, &root)?;

    let record = db::file_by_path(&conn, &rel)?
        .with_context(|| format!("{} is not indexed", file.display()))?;
    let tags = db::tags_for_file(&conn, record.id)?;

    let indexed_at: String = conn.query_row(
        "SELECT indexed_at FROM files WHERE id = ?1",
        rusqlite::params![record.id],
        |r| r.get(0),
    )?;

    if cli.json {
        let j = JsonShowFile {
            path: rel,
            size: record.size,
            blake3: record.blake3.clone(),
            mtime: record.mtime_ns,
            indexed_at,
            tags: tags
                .iter()
                .map(|(n, v)| JsonTag {
                    name: n.clone(),
                    value: v.clone(),
                })
                .collect(),
        };
        println!("{}", serde_json::to_string(&j)?);
    } else {
        println!("Path:    {}", rel);
        println!("Size:    {}", format_size(record.size));
        println!(
            "BLAKE3:  {}",
            record.blake3.as_deref().unwrap_or("(not hashed)")
        );
        println!("Indexed: {}", indexed_at);
        let tag_strs: Vec<String> = tags.iter().map(|(n, v)| format_tag(n, v)).collect();
        println!("Tags:    {}", if tag_strs.is_empty() { "(none)".into() } else { tag_strs.join(", ") });
    }
    Ok(())
}

fn cmd_find(
    cli: &Cli,
    query_parts: Vec<String>,
    with_tags: bool,
    count: bool,
    null: bool,
) -> Result<()> {
    let (conn, _root) = open_db(cli)?;
    let query_str = query_parts.join(" ");
    let expr = query::parse(&query_str)?;

    if count {
        let paths = query::execute(&conn, &expr)?;
        if cli.json {
            println!("{{\"count\":{}}}", paths.len());
        } else {
            println!("{}", paths.len());
        }
        return Ok(());
    }

    let terminator = if null { "\0" } else { "\n" };

    if with_tags || cli.json {
        let results = query::execute_with_tags(&conn, &expr)?;
        let stdout = io::stdout();
        let mut out = stdout.lock();
        for (path, tags) in &results {
            if cli.json {
                let j = JsonFileTags {
                    path: path.clone(),
                    tags: tags
                        .iter()
                        .map(|(n, v)| JsonTag {
                            name: n.clone(),
                            value: v.clone(),
                        })
                        .collect(),
                };
                write!(out, "{}{}", serde_json::to_string(&j)?, terminator)?;
            } else {
                let tag_strs: Vec<String> =
                    tags.iter().map(|(n, v)| format_tag(n, v)).collect();
                write!(out, "{}\t{}{}", path, tag_strs.join(" "), terminator)?;
            }
        }
    } else {
        let paths = query::execute(&conn, &expr)?;
        let stdout = io::stdout();
        let mut out = stdout.lock();
        for path in &paths {
            write!(out, "{}{}", path, terminator)?;
        }
    }
    Ok(())
}

fn cmd_view(cli: &Cli, query_parts: Vec<String>, output: PathBuf) -> Result<()> {
    let (conn, root) = open_db(cli)?;
    let query_str = query_parts.join(" ");
    let expr = query::parse(&query_str)?;
    let paths = query::execute(&conn, &expr)?;

    let stats = view::generate(&root, &paths, &output)?;
    info!(
        cli,
        "View: {} created, {} skipped, {} missing",
        stats.created,
        stats.skipped,
        stats.missing
    );
    Ok(())
}

fn cmd_status(cli: &Cli, path: Option<PathBuf>) -> Result<()> {
    let (conn, root) = open_db(cli)?;

    let scan_dir = match &path {
        Some(p) => std::fs::canonicalize(p)?,
        None => root.clone(),
    };

    let mut stmt = conn.prepare("SELECT id, path, size, mtime_ns FROM files ORDER BY path")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;

    let mut missing = 0;
    let mut modified = 0;
    let mut ok = 0;

    for row in rows {
        let (_id, rel_path, db_size, db_mtime) = row?;
        let abs = root.join(&rel_path);

        if let Ok(canonical) = std::fs::canonicalize(&abs) {
            if !canonical.starts_with(&scan_dir) {
                continue;
            }
        }

        if !abs.exists() {
            println!("missing:  {}", rel_path);
            missing += 1;
            continue;
        }

        if let Ok(meta) = std::fs::metadata(&abs) {
            let size = meta.len() as i64;
            let mtime = meta
                .modified()
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0);

            if size != db_size || mtime != db_mtime {
                println!("modified: {}", rel_path);
                modified += 1;
                continue;
            }
        }
        ok += 1;
    }

    let mut untagged = 0;
    for entry in walkdir::WalkDir::new(&scan_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.components().any(|c| c.as_os_str() == ".filetag") {
            continue;
        }
        if let Ok(rel) = path
            .strip_prefix(&root)
            .map(|p| p.to_string_lossy().into_owned())
        {
            if db::file_by_path(&conn, &rel)?.is_none() {
                println!("untagged: {}", rel);
                untagged += 1;
            }
        }
    }

    info!(
        cli,
        "\n{} ok, {} missing, {} modified, {} untagged",
        ok,
        missing,
        modified,
        untagged
    );
    Ok(())
}

fn cmd_repair(cli: &Cli, search_path: Option<PathBuf>, dry_run: bool) -> Result<()> {
    let (conn, root) = open_db(cli)?;

    let search_dir = match &search_path {
        Some(p) => std::fs::canonicalize(p)?,
        None => root.clone(),
    };

    let mut stmt =
        conn.prepare("SELECT id, path, blake3 FROM files WHERE blake3 IS NOT NULL ORDER BY path")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;

    let mut missing_files: Vec<(i64, String, String)> = Vec::new();
    for row in rows {
        let (id, path, hash) = row?;
        let abs = root.join(&path);
        if !abs.exists() {
            missing_files.push((id, path, hash));
        }
    }

    if missing_files.is_empty() {
        info!(cli, "No missing files to repair");
        return Ok(());
    }

    info!(
        cli,
        "Found {} missing file(s), scanning for matches...",
        missing_files.len()
    );

    let mut hash_to_missing: std::collections::HashMap<String, Vec<(i64, String)>> =
        std::collections::HashMap::new();
    for (id, path, hash) in &missing_files {
        hash_to_missing
            .entry(hash.clone())
            .or_default()
            .push((*id, path.clone()));
    }

    let mut repaired = 0;
    for entry in walkdir::WalkDir::new(&search_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        if entry
            .path()
            .components()
            .any(|c| c.as_os_str() == ".filetag")
        {
            continue;
        }

        let rel_path = match entry.path().strip_prefix(&root) {
            Ok(r) => r.to_string_lossy().into_owned(),
            Err(_) => continue,
        };

        if db::file_by_path(&conn, &rel_path)?.is_some() {
            continue;
        }

        let hash = {
            use std::io::Read;
            let mut file = match std::fs::File::open(entry.path()) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let mut hasher = blake3::Hasher::new();
            let mut buf = [0u8; 65536];
            loop {
                let n = match file.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(_) => break,
                };
                hasher.update(&buf[..n]);
            }
            hasher.finalize().to_hex().to_string()
        };

        if let Some(entries) = hash_to_missing.get(&hash) {
            for (id, old_path) in entries {
                if dry_run {
                    println!("would repair: {} -> {}", old_path, rel_path);
                } else {
                    let meta = std::fs::metadata(entry.path())?;
                    let size = meta.len() as i64;
                    let mtime = meta
                        .modified()
                        .ok()
                        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_nanos() as i64)
                        .unwrap_or(0);

                    conn.execute(
                        "UPDATE files SET path = ?1, size = ?2, mtime_ns = ?3, indexed_at = datetime('now') WHERE id = ?4",
                        rusqlite::params![rel_path, size, mtime, id],
                    )?;
                    println!("repaired: {} -> {}", old_path, rel_path);
                }
                repaired += 1;
                break;
            }
        }
    }

    info!(
        cli,
        "{} {} file(s)",
        if dry_run { "Would repair" } else { "Repaired" },
        repaired
    );
    Ok(())
}

fn cmd_mv(cli: &Cli, from: String, to: String) -> Result<()> {
    let (conn, _root) = open_db(cli)?;

    // Check source exists
    let from_id: i64 = conn
        .query_row(
            "SELECT id FROM tags WHERE name = ?1",
            rusqlite::params![&from],
            |r| r.get(0),
        )
        .with_context(|| format!("tag '{}' not found", from))?;

    // Check target doesn't exist
    let target_exists = conn
        .query_row(
            "SELECT id FROM tags WHERE name = ?1",
            rusqlite::params![&to],
            |r| r.get::<_, i64>(0),
        )
        .is_ok();

    if target_exists {
        anyhow::bail!(
            "tag '{}' already exists (use 'filetag merge' to combine tags)",
            to
        );
    }

    conn.execute(
        "UPDATE tags SET name = ?1 WHERE id = ?2",
        rusqlite::params![&to, from_id],
    )?;

    info!(cli, "Renamed '{}' -> '{}'", from, to);
    Ok(())
}

fn cmd_merge(cli: &Cli, from: String, into: String, force: bool, dry_run: bool) -> Result<()> {
    let (conn, _root) = open_db(cli)?;

    let from_id: i64 = conn
        .query_row(
            "SELECT id FROM tags WHERE name = ?1",
            rusqlite::params![&from],
            |r| r.get(0),
        )
        .with_context(|| format!("tag '{}' not found", from))?;

    let assignment_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM file_tags WHERE tag_id = ?1",
        rusqlite::params![from_id],
        |r| r.get(0),
    )?;

    if dry_run {
        println!(
            "Would merge '{}' ({} assignments) into '{}'",
            from, assignment_count, into
        );
        return Ok(());
    }

    // Confirmation prompt for interactive terminals (unless --force)
    if !force && io::stdin().is_terminal() {
        eprint!(
            "Merge '{}' ({} assignments) into '{}'? [y/N] ",
            from, assignment_count, into
        );
        io::stderr().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        if !answer.trim().eq_ignore_ascii_case("y") {
            eprintln!("Aborted.");
            return Ok(());
        }
    }

    let to_id = db::get_or_create_tag(&conn, &into)?;

    let moved = conn.execute(
        "INSERT OR IGNORE INTO file_tags (file_id, tag_id, value, created_at)
         SELECT file_id, ?1, value, created_at FROM file_tags WHERE tag_id = ?2",
        rusqlite::params![to_id, from_id],
    )?;

    conn.execute(
        "DELETE FROM file_tags WHERE tag_id = ?1",
        rusqlite::params![from_id],
    )?;
    conn.execute(
        "DELETE FROM tags WHERE id = ?1",
        rusqlite::params![from_id],
    )?;

    info!(
        cli,
        "Merged '{}' into '{}' ({} assignment(s) moved)",
        from,
        into,
        moved
    );
    Ok(())
}

fn cmd_info(cli: &Cli) -> Result<()> {
    let (conn, root) = open_db(cli)?;

    let file_count: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
    let tag_count: i64 = conn.query_row("SELECT COUNT(*) FROM tags", [], |r| r.get(0))?;
    let assignment_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM file_tags", [], |r| r.get(0))?;
    let total_size: i64 = conn.query_row(
        "SELECT COALESCE(SUM(size), 0) FROM files",
        [],
        |r| r.get(0),
    )?;

    if cli.json {
        let j = JsonInfo {
            root: root.display().to_string(),
            files: file_count,
            tags: tag_count,
            assignments: assignment_count,
            total_size,
        };
        println!("{}", serde_json::to_string(&j)?);
    } else {
        println!("Database root: {}", root.display());
        println!("Files:         {}", file_count);
        println!("Tags:          {}", tag_count);
        println!("Assignments:   {}", assignment_count);
        println!("Total size:    {}", format_size(total_size));
    }
    Ok(())
}

fn cmd_completions(shell: Shell) -> Result<()> {
    clap_complete::generate(
        shell,
        &mut Cli::command(),
        "filetag",
        &mut io::stdout(),
    );
    Ok(())
}
