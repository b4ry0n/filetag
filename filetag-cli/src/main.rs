#[cfg(unix)]
use filetag_lib::view;
use filetag_lib::{TagList, db, query, registry};

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use indicatif::{ProgressBar, ProgressStyle};
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

    /// Do not automatically include ancestor databases (stop at the current root)
    #[arg(long, global = true)]
    no_parents: bool,

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
    Init {
        /// Also register in the global database registry
        #[arg(long)]
        register: bool,
    },

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

        /// Only query this database (no linked children, no ancestor databases)
        #[arg(short, long)]
        isolated: bool,

        /// Search across all registered databases (global registry)
        #[arg(long)]
        all_dbs: bool,
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

        /// Only query this database (no linked children, no ancestor databases)
        #[arg(short, long)]
        isolated: bool,

        /// Search across all registered databases (global registry)
        #[arg(long)]
        all_dbs: bool,
    },

    /// Generate a symlink view for a tag query (Unix only)
    #[cfg(unix)]
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

    /// Find moved files by matching file identity or name+size
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

    /// Manage tag synonyms (aliases)
    Synonym {
        #[command(subcommand)]
        action: SynonymAction,
    },

    /// Manage linked databases
    Db {
        #[command(subcommand)]
        action: DbAction,
    },

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },
}

#[derive(Subcommand)]
enum SynonymAction {
    /// Register an alias as a synonym for a canonical tag
    Add {
        /// Alias name (the synonym to add)
        alias: String,
        /// Canonical tag name (the tag the alias maps to)
        canonical: String,
    },

    /// Remove a registered synonym
    Remove {
        /// Alias name to remove
        alias: String,
    },

    /// List all registered synonyms
    #[command(visible_alias = "ls")]
    List,
}

#[derive(Subcommand)]
enum DbAction {
    /// List registered linked databases
    #[command(visible_alias = "ls")]
    List,

    /// Link another database to this one
    Add {
        /// Path to the database root to link (must contain .filetag/)
        path: PathBuf,
    },

    /// Remove a linked database registration
    Remove {
        /// Path to the linked database root
        path: PathBuf,
    },

    /// Remove registrations for missing databases
    Prune,

    /// Transfer tag records for files under a linked path from this DB to the linked DB
    Push {
        /// Path to the linked database root (must be a child, i.e. under the current root)
        path: PathBuf,

        /// Only show what would be transferred
        #[arg(short = 'n', long)]
        dry_run: bool,
    },

    /// Transfer tag records from a linked DB back to this DB
    Pull {
        /// Path to the linked database root (must be a child, i.e. under the current root)
        path: PathBuf,

        /// Only show what would be transferred
        #[arg(short = 'n', long)]
        dry_run: bool,
    },

    /// Register this database in the global registry (~/.config/filetag/)
    Register,

    /// Remove this database from the global registry
    Unregister,

    /// List all globally registered databases
    Registered,
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
    file_id: Option<String>,
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
        Command::Init { register } => cmd_init(&cli, *register),
        Command::Tag {
            files,
            tags,
            recursive,
            null,
        } => cmd_tag(&cli, files.clone(), tags.clone(), *recursive, *null),
        Command::Untag { files, tags, null } => cmd_untag(&cli, files.clone(), tags.clone(), *null),
        Command::Tags {
            files,
            isolated,
            all_dbs,
        } => cmd_tags(&cli, files.clone(), *isolated, *all_dbs),
        Command::Show { file } => cmd_show(&cli, file.clone()),
        Command::Find {
            query,
            with_tags,
            count,
            null,
            isolated,
            all_dbs,
        } => cmd_find(
            &cli,
            query.clone(),
            *with_tags,
            *count,
            *null,
            *isolated,
            *all_dbs,
        ),
        #[cfg(unix)]
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
        Command::Synonym { action } => cmd_synonym(&cli, action),
        Command::Db { action } => cmd_db(&cli, action),
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

fn parse_tag_args(tags: &[String]) -> Vec<(String, Option<String>)> {
    tags.iter().map(|t| filetag_lib::parse_tag(t)).collect()
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

/// If `path` is an archive entry path (contains `::`), return `(archive_part, entry_part)`.
/// Works on both absolute and relative paths: `"archive.zip::entry.jpg"`.
fn split_archive_path(path: &std::path::Path) -> Option<(String, String)> {
    let s = path.to_string_lossy();
    let (zip, entry) = s.split_once("::")?;
    Some((zip.to_string(), entry.to_string()))
}

/// Resolve a path to its DB-relative path string, handling archive entries.
/// For normal files: canonicalize + strip root prefix.
/// For archive entries (`archive.zip::entry`): canonicalize archive, strip prefix, append entry.
fn path_to_rel(path: &std::path::Path, root: &std::path::Path) -> Result<String> {
    if let Some((zip_str, entry)) = split_archive_path(path) {
        db::resolve_archive_entry(&format!("{}::{}", zip_str, entry), root)
    } else {
        db::relative_to_root(path, root).with_context(|| format!("resolving {}", path.display()))
    }
}

/// Index a file or archive entry, returning its FileRecord.
fn index_path(
    conn: &rusqlite::Connection,
    rel: &str,
    root: &std::path::Path,
) -> Result<db::FileRecord> {
    if rel.contains("::") {
        db::get_or_index_archive_entry(conn, rel)
    } else {
        db::get_or_index_file(conn, rel, root)
    }
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

/// Create a progress bar for bulk operations. Returns a hidden bar if --quiet
/// or stderr is not a terminal.
fn make_progress(cli: &Cli, len: u64, msg: &str) -> ProgressBar {
    if cli.quiet || !io::stderr().is_terminal() {
        return ProgressBar::hidden();
    }
    let pb = ProgressBar::new(len);
    pb.set_style(
        ProgressStyle::with_template("{msg} [{bar:30}] {pos}/{len} {per_sec}")
            .unwrap()
            .progress_chars("=> "),
    );
    pb.set_message(msg.to_string());
    pb
}

// ---------------------------------------------------------------------------
// Subcommand implementations
// ---------------------------------------------------------------------------

fn cmd_init(cli: &Cli, register: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    db::init(&cwd)?;
    if register {
        registry::add(&cwd)?;
        info!(cli, "Registered in global registry");
    }
    println!("Initialized filetag database in {}", cwd.display());
    Ok(())
}

fn cmd_tag(
    cli: &Cli,
    files: Vec<PathBuf>,
    tags: Vec<String>,
    recursive: bool,
    null: bool,
) -> Result<()> {
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

    let tx = conn.unchecked_transaction()?;
    let mut tagged_count = 0;
    let pb = make_progress(cli, file_paths.len() as u64, "Tagging");
    for file_path in &file_paths {
        let rel = path_to_rel(file_path, &root)?;
        let record = index_path(&tx, &rel, &root)?;

        for (tag_name, tag_value) in &parsed_tags {
            let tag_id = db::get_or_create_tag(&tx, tag_name)?;
            db::apply_tag(&tx, record.id, tag_id, tag_value.as_deref())?;
        }
        tagged_count += 1;
        pb.inc(1);
    }
    pb.finish_and_clear();
    tx.commit()?;

    info!(
        cli,
        "Tagged {} file(s) with {} tag(s)",
        tagged_count,
        parsed_tags.len()
    );
    Ok(())
}

fn cmd_untag(cli: &Cli, files: Vec<PathBuf>, tags: Vec<String>, null: bool) -> Result<()> {
    let (conn, root) = open_db(cli)?;
    let parsed_tags = parse_tag_args(&tags);

    let collected = collect_files(files, null)?;
    if collected.is_empty() {
        anyhow::bail!("no files specified (provide paths as arguments or pipe them via stdin)");
    }

    let tx = conn.unchecked_transaction()?;
    let mut removed_count = 0;
    let pb = make_progress(cli, collected.len() as u64, "Untagging");
    for file_path in &collected {
        let rel = path_to_rel(file_path, &root)?;
        if let Some(record) = db::file_by_path(&tx, &rel)? {
            for (tag_name, tag_value) in &parsed_tags {
                if let Ok(tag_id) = tx.query_row(
                    "SELECT id FROM tags WHERE name = ?1",
                    rusqlite::params![tag_name],
                    |r| r.get::<_, i64>(0),
                ) && db::remove_tag(&tx, record.id, tag_id, tag_value.as_deref())?
                {
                    removed_count += 1;
                }
            }
        }
        pb.inc(1);
    }
    pb.finish_and_clear();
    tx.commit()?;

    info!(cli, "Removed {} tag assignment(s)", removed_count);
    Ok(())
}

fn cmd_tags(cli: &Cli, files: Vec<PathBuf>, isolated: bool, all_dbs: bool) -> Result<()> {
    let (conn, root) = open_db(cli)?;

    if files.is_empty() {
        // Collect tags from databases
        let mut merged_tags: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();

        if all_dbs {
            // Query across all globally registered databases
            let db_roots = registry::list()?;
            for db_root in &db_roots {
                let db_path = PathBuf::from(db_root).join(".filetag").join("db.sqlite3");
                if let Ok(c) = rusqlite::Connection::open(&db_path)
                    && let Ok(tags) = db::all_tags(&c)
                {
                    for (name, count, _color, _has_values) in tags {
                        *merged_tags.entry(name).or_insert(0) += count;
                    }
                }
            }
        } else if isolated {
            // Isolated: only the current database, no linked children, no ancestors.
            let tags = db::all_tags(&conn)?;
            for (name, count, _color, _has_values) in tags {
                merged_tags.insert(name, count);
            }
        } else {
            let databases = db::collect_all_databases(conn, root, !cli.no_parents)?;
            for db in &databases {
                if let Ok(tags) = db::all_tags(&db.conn) {
                    for (name, count, _color, _has_values) in tags {
                        *merged_tags.entry(name).or_insert(0) += count;
                    }
                }
            }
        }

        let mut tags: Vec<(String, i64)> = merged_tags.into_iter().collect();
        tags.sort_by(|a, b| a.0.cmp(&b.0));

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
            let rel = path_to_rel(file_path, &root)
                .unwrap_or_else(|_| file_path.to_string_lossy().into_owned());
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
    let rel = path_to_rel(&file, &root)?;

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
            file_id: record.file_id.clone(),
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
        println!("Indexed: {}", indexed_at);
        let tag_strs: Vec<String> = tags.iter().map(|(n, v)| format_tag(n, v)).collect();
        println!(
            "Tags:    {}",
            if tag_strs.is_empty() {
                "(none)".into()
            } else {
                tag_strs.join(", ")
            }
        );
    }
    Ok(())
}

fn cmd_find(
    cli: &Cli,
    query_parts: Vec<String>,
    with_tags: bool,
    count: bool,
    null: bool,
    isolated: bool,
    all_dbs: bool,
) -> Result<()> {
    let (conn, root) = open_db(cli)?;
    let query_str = query_parts.join(" ");
    let expr = query::parse(&query_str)?;
    let cwd = std::env::current_dir()?;

    let terminator = if null { "\0" } else { "\n" };

    // Collect results from one or more databases
    let mut collector = FindCollector::default();

    if all_dbs {
        // Query across all globally registered databases
        let db_roots = registry::list()?;
        for db_root in &db_roots {
            let db_path = PathBuf::from(db_root).join(".filetag").join("db.sqlite3");
            if let Ok(c) = rusqlite::Connection::open(&db_path) {
                let r = PathBuf::from(db_root);
                collector.add(&c, &r, &cwd, &expr, with_tags || cli.json)?;
            }
        }
    } else if isolated {
        // Isolated: only the current database, no linked children, no ancestors.
        collector.add(&conn, &root, &cwd, &expr, with_tags || cli.json)?;
    } else {
        let databases = db::collect_all_databases(conn, root, !cli.no_parents)?;
        for database in &databases {
            collector.add(
                &database.conn,
                &database.root,
                &cwd,
                &expr,
                with_tags || cli.json,
            )?;
        }
    }

    if count {
        let n = if with_tags || cli.json {
            collector.results.len()
        } else {
            collector.paths.len()
        };
        if cli.json {
            println!("{{\"count\":{}}}", n);
        } else {
            println!("{}", n);
        }
        return Ok(());
    }

    let stdout = io::stdout();
    let mut out = stdout.lock();

    if with_tags || cli.json {
        for (path, tags) in &collector.results {
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
                let tag_strs: Vec<String> = tags.iter().map(|(n, v)| format_tag(n, v)).collect();
                write!(out, "{}\t{}{}", path, tag_strs.join(" "), terminator)?;
            }
        }
    } else {
        for path in &collector.paths {
            write!(out, "{}{}", path, terminator)?;
        }
    }
    Ok(())
}

#[derive(Default)]
struct FindCollector {
    paths: Vec<String>,
    results: Vec<(String, TagList)>,
    seen: std::collections::HashSet<String>,
}

impl FindCollector {
    fn add(
        &mut self,
        conn: &rusqlite::Connection,
        db_root: &std::path::Path,
        cwd: &std::path::Path,
        expr: &query::Expr,
        need_tags: bool,
    ) -> Result<()> {
        if need_tags {
            let results = query::execute_with_tags(conn, expr)?;
            for (rel_path, tags) in results {
                let abs = db_root.join(&rel_path);
                let display_path = make_display_path(&abs, cwd);
                if self.seen.insert(display_path.clone()) {
                    self.results.push((display_path, tags));
                }
            }
        } else {
            let paths = query::execute(conn, expr)?;
            for rel_path in paths {
                let abs = db_root.join(&rel_path);
                let display_path = make_display_path(&abs, cwd);
                if self.seen.insert(display_path.clone()) {
                    self.paths.push(display_path);
                }
            }
        }
        Ok(())
    }
}

/// Convert an absolute path to a display path relative to CWD when possible.
fn make_display_path(abs: &std::path::Path, cwd: &std::path::Path) -> String {
    match abs.strip_prefix(cwd) {
        Ok(rel) => rel.to_string_lossy().into_owned(),
        Err(_) => abs.to_string_lossy().into_owned(),
    }
}

#[cfg(unix)]
fn cmd_view(cli: &Cli, query_parts: Vec<String>, output: PathBuf) -> Result<()> {
    let (conn, root) = open_db(cli)?;
    let query_str = query_parts.join(" ");
    let expr = query::parse(&query_str)?;
    let paths = query::execute(&conn, &expr)?;

    let stats = view::generate(&root, &paths, &output)?;
    info!(
        cli,
        "View: {} created, {} skipped, {} missing", stats.created, stats.skipped, stats.missing
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

        if let Ok(canonical) = std::fs::canonicalize(&abs)
            && !canonical.starts_with(&scan_dir)
        {
            continue;
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
            && db::file_by_path(&conn, &rel)?.is_none()
        {
            println!("untagged: {}", rel);
            untagged += 1;
        }
    }

    info!(
        cli,
        "\n{} ok, {} missing, {} modified, {} untagged", ok, missing, modified, untagged
    );
    Ok(())
}

fn cmd_repair(cli: &Cli, search_path: Option<PathBuf>, dry_run: bool) -> Result<()> {
    let (conn, root) = open_db(cli)?;

    let search_dir = match &search_path {
        Some(p) => std::fs::canonicalize(p)?,
        None => root.clone(),
    };

    // Step 1: Find all files that are missing from disk
    let mut stmt = conn.prepare("SELECT id, path, file_id, size FROM files ORDER BY path")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;

    let mut missing_files: Vec<(i64, String, Option<String>, i64)> = Vec::new();
    for row in rows {
        let (id, path, file_id, size) = row?;
        let abs = root.join(&path);
        if !abs.exists() {
            missing_files.push((id, path, file_id, size));
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

    // Step 2: Build lookup maps
    // file_id -> (db_id, old_path)  [strong match]
    let mut fid_to_missing: std::collections::HashMap<String, Vec<(i64, String)>> =
        std::collections::HashMap::new();
    // (filename, size) -> (db_id, old_path)  [weak match / candidate]
    let mut name_size_to_missing: std::collections::HashMap<(String, i64), Vec<(i64, String)>> =
        std::collections::HashMap::new();

    for (id, path, file_id, size) in &missing_files {
        if let Some(fid) = file_id {
            fid_to_missing
                .entry(fid.clone())
                .or_default()
                .push((*id, path.clone()));
        }
        let filename = path.rsplit('/').next().unwrap_or(path).to_string();
        name_size_to_missing
            .entry((filename, *size))
            .or_default()
            .push((*id, path.clone()));
    }

    // Step 3: Walk search_dir, match against missing files
    let mut repaired = 0;
    let pb = if !cli.quiet && io::stderr().is_terminal() {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::with_template("{spinner} {msg} ({pos} files scanned)").unwrap(),
        );
        pb.set_message("Scanning");
        pb
    } else {
        ProgressBar::hidden()
    };
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
        pb.inc(1);

        let rel_path = match entry.path().strip_prefix(&root) {
            Ok(r) => r.to_string_lossy().into_owned(),
            Err(_) => continue,
        };

        // Skip files already in the database
        if db::file_by_path(&conn, &rel_path)?.is_some() {
            continue;
        }

        let meta = match std::fs::metadata(entry.path()) {
            Ok(m) => m,
            Err(_) => continue,
        };

        // Try file_id match first (strong: same inode = same file, moved)
        #[cfg(unix)]
        let candidate_fid = {
            use std::os::unix::fs::MetadataExt;
            Some(format!("{}:{}", meta.dev(), meta.ino()))
        };
        #[cfg(not(unix))]
        let candidate_fid: Option<String> = None;

        let matched = if let Some(ref fid) = candidate_fid
            && let Some(entries) = fid_to_missing.get(fid)
            && let Some((id, old_path)) = entries.first()
        {
            Some((*id, old_path.clone(), "file_id"))
        } else {
            // Fallback: match on (filename, size)
            let filename = rel_path.rsplit('/').next().unwrap_or(&rel_path).to_string();
            let size = meta.len() as i64;
            if let Some(entries) = name_size_to_missing.get(&(filename, size))
                && let Some((id, old_path)) = entries.first()
            {
                Some((*id, old_path.clone(), "name+size"))
            } else {
                None
            }
        };

        if let Some((id, old_path, method)) = matched {
            let size = meta.len() as i64;
            let mtime = meta
                .modified()
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0);

            if dry_run {
                println!(
                    "would repair: {} -> {} (matched by {})",
                    old_path, rel_path, method
                );
            } else {
                conn.execute(
                    "UPDATE files SET path = ?1, file_id = ?2, size = ?3, mtime_ns = ?4, indexed_at = datetime('now') WHERE id = ?5",
                    rusqlite::params![rel_path, candidate_fid, size, mtime, id],
                )?;
                println!(
                    "repaired: {} -> {} (matched by {})",
                    old_path, rel_path, method
                );
            }
            repaired += 1;
        }
    }

    pb.finish_and_clear();
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
    match db::rename_tag(&conn, &from, &to)? {
        db::RenameOutcome::Renamed => {
            info!(cli, "Renamed '{}' -> '{}'", from, to);
        }
        db::RenameOutcome::Merged { assignments } => {
            info!(
                cli,
                "Merged '{}' into '{}' ({} assignment(s) moved)", from, to, assignments
            );
        }
        db::RenameOutcome::NotFound => {
            anyhow::bail!("tag '{}' not found", from);
        }
    }
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
    conn.execute("DELETE FROM tags WHERE id = ?1", rusqlite::params![from_id])?;

    info!(
        cli,
        "Merged '{}' into '{}' ({} assignment(s) moved)", from, into, moved
    );
    Ok(())
}

fn cmd_info(cli: &Cli) -> Result<()> {
    let (conn, root) = open_db(cli)?;

    let file_count: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
    let tag_count: i64 = conn.query_row("SELECT COUNT(*) FROM tags", [], |r| r.get(0))?;
    let assignment_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM file_tags", [], |r| r.get(0))?;
    let total_size: i64 =
        conn.query_row("SELECT COALESCE(SUM(size), 0) FROM files", [], |r| r.get(0))?;

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
    clap_complete::generate(shell, &mut Cli::command(), "filetag", &mut io::stdout());
    Ok(())
}

fn cmd_synonym(cli: &Cli, action: &SynonymAction) -> Result<()> {
    let (conn, _root) = open_db(cli)?;
    match action {
        SynonymAction::Add { alias, canonical } => {
            db::add_synonym(&conn, alias, canonical)?;
            info!(cli, "Added synonym '{}' → '{}'", alias, canonical);
        }
        SynonymAction::Remove { alias } => {
            if db::remove_synonym(&conn, alias)? {
                info!(cli, "Removed synonym '{}'", alias);
            } else {
                anyhow::bail!("synonym '{}' not found", alias);
            }
        }
        SynonymAction::List => {
            let synonyms = db::list_synonyms(&conn)?;
            if synonyms.is_empty() {
                if !cli.quiet {
                    eprintln!("No synonyms registered.");
                }
                return Ok(());
            }
            if cli.json {
                let j: Vec<serde_json::Value> = synonyms
                    .iter()
                    .map(|(alias, canonical)| {
                        serde_json::json!({ "alias": alias, "canonical": canonical })
                    })
                    .collect();
                println!("{}", serde_json::to_string(&j)?);
            } else {
                for (alias, canonical) in &synonyms {
                    println!("{alias} → {canonical}");
                }
            }
        }
    }
    Ok(())
}

/// Validate that `path` is a registered linked database.
/// Returns the stored key (relative if under root, absolute if partner) and its `.filetag/db.sqlite3` path.
fn resolve_registered_linked(
    conn: &rusqlite::Connection,
    root: &std::path::Path,
    path: &PathBuf,
) -> Result<(String, PathBuf)> {
    let abs =
        std::fs::canonicalize(path).with_context(|| format!("resolving {}", path.display()))?;
    let linked_db_path = abs.join(".filetag").join("db.sqlite3");
    if !linked_db_path.is_file() {
        anyhow::bail!(
            "no filetag database found at {} (run 'filetag init' there first)",
            abs.display()
        );
    }
    let stored_path = abs
        .strip_prefix(root)
        .map(|rel| rel.to_string_lossy().into_owned())
        .unwrap_or_else(|_| abs.to_string_lossy().into_owned());
    let linked = db::list_linked(conn)?;
    if !linked.contains(&stored_path) {
        anyhow::bail!(
            "'{}' is not a linked database (use 'filetag db add' first)",
            stored_path
        );
    }
    Ok((stored_path, linked_db_path))
}

/// Open a linked database connection with the standard PRAGMA settings.
fn open_linked_conn(db_path: &std::path::Path) -> Result<rusqlite::Connection> {
    let conn = rusqlite::Connection::open(db_path)
        .with_context(|| format!("opening linked database {}", db_path.display()))?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;",
    )?;
    Ok(conn)
}

fn cmd_db(cli: &Cli, action: &DbAction) -> Result<()> {
    let (conn, root) = open_db(cli)?;

    match action {
        DbAction::List => {
            let linked = db::list_linked(&conn)?;
            if linked.is_empty() {
                info!(cli, "No linked databases registered");
            } else {
                for linked_path in &linked {
                    let linked_root = root.join(linked_path);
                    let db_path = linked_root.join(".filetag").join("db.sqlite3");
                    let status = if db_path.is_file() { "ok" } else { "missing" };
                    if cli.json {
                        println!(
                            "{{\"path\":{},\"status\":{}}}",
                            serde_json::to_string(linked_path)?,
                            serde_json::to_string(status)?
                        );
                    } else {
                        println!("{}\t{}", linked_path, status);
                    }
                }
            }
        }
        DbAction::Add { path } => {
            let abs = std::fs::canonicalize(path)
                .with_context(|| format!("resolving {}", path.display()))?;
            let linked_db = abs.join(".filetag").join("db.sqlite3");
            if !linked_db.is_file() {
                anyhow::bail!(
                    "no filetag database found at {} (run 'filetag init' there first)",
                    abs.display()
                );
            }
            // Store relative path if the target is under the current root (child),
            // or absolute path if it is outside (partner/parent).
            let stored_path = abs
                .strip_prefix(&root)
                .map(|rel| rel.to_string_lossy().into_owned())
                .unwrap_or_else(|_| abs.to_string_lossy().into_owned());
            db::link_database(&conn, &stored_path)?;
            info!(cli, "Linked database: {}", stored_path);
        }
        DbAction::Remove { path } => {
            let abs = std::fs::canonicalize(path)
                .or_else(|_| Ok::<PathBuf, std::io::Error>(path.clone()))?;
            let stored_path = abs
                .strip_prefix(&root)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| abs.to_string_lossy().into_owned());
            if db::unlink_database(&conn, &stored_path)? {
                info!(cli, "Removed linked database: {}", stored_path);
            } else {
                anyhow::bail!("linked database '{}' not found in registry", stored_path);
            }
        }
        DbAction::Prune => {
            let linked = db::list_linked(&conn)?;
            let mut pruned = 0;
            for linked_path in &linked {
                let linked_root = root.join(linked_path);
                let db_path = linked_root.join(".filetag").join("db.sqlite3");
                if !db_path.is_file() {
                    db::unlink_database(&conn, linked_path)?;
                    println!("pruned: {}", linked_path);
                    pruned += 1;
                }
            }
            // Also prune global registry
            let global_pruned = registry::prune()?;
            for p in &global_pruned {
                println!("pruned global: {}", p);
            }
            info!(
                cli,
                "Pruned {} linked + {} global registration(s)",
                pruned,
                global_pruned.len()
            );
        }
        DbAction::Push { path, dry_run } => {
            let (linked_path, linked_db_path) = resolve_registered_linked(&conn, &root, path)?;
            // Push only makes sense when the linked DB is under the current root (child relationship)
            if PathBuf::from(&linked_path).is_absolute() {
                anyhow::bail!(
                    "push/pull is only supported for databases under the current root (child relationship)"
                );
            }

            let files = db::files_under_prefix(&conn, &linked_path)?;
            if files.is_empty() {
                info!(cli, "No files in this DB under {}/", linked_path);
                return Ok(());
            }

            if *dry_run {
                for f in &files {
                    let linked_rel = f
                        .rel_path
                        .strip_prefix(&linked_path)
                        .unwrap_or(&f.rel_path)
                        .trim_start_matches('/');
                    let tag_count = f.tags.len();
                    println!(
                        "{} ({} tag{})",
                        linked_rel,
                        tag_count,
                        if tag_count == 1 { "" } else { "s" }
                    );
                }
                info!(cli, "{} record(s) would be transferred", files.len());
                return Ok(());
            }

            let linked_conn = open_linked_conn(&linked_db_path)?;

            let parent_tx = conn.unchecked_transaction()?;
            let linked_tx = linked_conn.unchecked_transaction()?;

            let mut transferred = 0u64;
            let pb = make_progress(cli, files.len() as u64, "Pushing");
            let prefix_with_slash = format!("{}/", linked_path.trim_end_matches('/'));
            for f in &files {
                let dest_path = f
                    .rel_path
                    .strip_prefix(&prefix_with_slash)
                    .unwrap_or(&f.rel_path);

                // Insert file record into linked DB
                linked_conn.execute(
                    "INSERT OR IGNORE INTO files (path, file_id, size, mtime_ns) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![dest_path, f.file_id, f.size, f.mtime_ns],
                )?;
                let linked_file_id: i64 = linked_conn.query_row(
                    "SELECT id FROM files WHERE path = ?1",
                    rusqlite::params![dest_path],
                    |row| row.get(0),
                )?;

                // Copy tags
                for (tag_name, value) in &f.tags {
                    let tag_id = db::get_or_create_tag(&linked_conn, tag_name)?;
                    db::apply_tag(
                        &linked_conn,
                        linked_file_id,
                        tag_id,
                        if value.is_empty() { None } else { Some(value) },
                    )?;
                }

                // Remove from this DB
                db::delete_file_by_path(&parent_tx, &f.rel_path)?;
                transferred += 1;
                pb.inc(1);
            }
            pb.finish_and_clear();

            linked_tx.commit()?;
            parent_tx.commit()?;

            info!(
                cli,
                "Transferred {} record(s) to linked database {}", transferred, linked_path
            );
        }
        DbAction::Pull { path, dry_run } => {
            let (linked_path, linked_db_path) = resolve_registered_linked(&conn, &root, path)?;
            // Pull only makes sense when the linked DB is under the current root (child relationship)
            if PathBuf::from(&linked_path).is_absolute() {
                anyhow::bail!(
                    "push/pull is only supported for databases under the current root (child relationship)"
                );
            }
            let linked_conn = open_linked_conn(&linked_db_path)?;

            let files = db::all_files_with_tags(&linked_conn)?;
            if files.is_empty() {
                info!(cli, "No files in linked DB {}", linked_path);
                return Ok(());
            }

            if *dry_run {
                let prefix_with_slash = format!("{}/", linked_path.trim_end_matches('/'));
                for f in &files {
                    let parent_path = format!("{}{}", prefix_with_slash, f.rel_path);
                    let tag_count = f.tags.len();
                    println!(
                        "{} ({} tag{})",
                        parent_path,
                        tag_count,
                        if tag_count == 1 { "" } else { "s" }
                    );
                }
                info!(cli, "{} record(s) would be transferred", files.len());
                return Ok(());
            }

            let parent_tx = conn.unchecked_transaction()?;
            let linked_tx = linked_conn.unchecked_transaction()?;

            let mut transferred = 0u64;
            let pb = make_progress(cli, files.len() as u64, "Pulling");
            let prefix_with_slash = format!("{}/", linked_path.trim_end_matches('/'));
            for f in &files {
                let parent_path = format!("{}{}", prefix_with_slash, f.rel_path);

                // Insert file record into this DB
                parent_tx.execute(
                    "INSERT OR IGNORE INTO files (path, file_id, size, mtime_ns) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![parent_path, f.file_id, f.size, f.mtime_ns],
                )?;
                let parent_file_id: i64 = parent_tx.query_row(
                    "SELECT id FROM files WHERE path = ?1",
                    rusqlite::params![parent_path],
                    |row| row.get(0),
                )?;

                // Copy tags
                for (tag_name, value) in &f.tags {
                    let tag_id = db::get_or_create_tag(&parent_tx, tag_name)?;
                    db::apply_tag(
                        &parent_tx,
                        parent_file_id,
                        tag_id,
                        if value.is_empty() { None } else { Some(value) },
                    )?;
                }

                // Remove from linked DB
                db::delete_file_by_path(&linked_tx, &f.rel_path)?;
                transferred += 1;
                pb.inc(1);
            }
            pb.finish_and_clear();

            linked_tx.commit()?;
            parent_tx.commit()?;

            info!(
                cli,
                "Transferred {} record(s) from linked database {}", transferred, linked_path
            );
        }
        DbAction::Register => {
            if registry::add(&root)? {
                info!(cli, "Registered {} in global registry", root.display());
            } else {
                info!(cli, "Already registered: {}", root.display());
            }
        }
        DbAction::Unregister => {
            if registry::remove(&root)? {
                info!(cli, "Removed {} from global registry", root.display());
            } else {
                anyhow::bail!("{} is not in the global registry", root.display());
            }
        }
        DbAction::Registered => {
            let dbs = registry::list()?;
            if dbs.is_empty() {
                info!(cli, "No databases in global registry");
            } else {
                for db_root in &dbs {
                    let db_path = PathBuf::from(db_root).join(".filetag").join("db.sqlite3");
                    let status = if db_path.is_file() { "ok" } else { "missing" };
                    if cli.json {
                        println!(
                            "{{\"path\":{},\"status\":{}}}",
                            serde_json::to_string(db_root)?,
                            serde_json::to_string(status)?
                        );
                    } else {
                        println!("{}\t{}", db_root, status);
                    }
                }
            }
        }
    }
    Ok(())
}
