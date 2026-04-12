# filetag - Rust file tagging CLI

Project location: `~/Code/filetag`

## Language conventions

- All code, comments, documentation, commit messages, and project files are written in **UK English**.
- Conversation with the user is in **Dutch**.

## Overview

SQLite-backed file tagging tool in Rust. Tags with key=value support, file identity tracking, symlink view generation. Cross-platform (macOS + Linux).

## Core principles

- **Data isolation:** filetag MUST NEVER write outside the `.filetag/` directory of the active database root. This applies to all temporary files, caches, logs, and any other artefacts. System directories such as `std::env::temp_dir()`, `$TMPDIR`, `/tmp`, or `~/.cache` are forbidden. All intermediate and cached files go under `.filetag/` (e.g. `.filetag/cache/`, `.filetag/tmp/`).

## Decisions

- Name: `filetag` (binary and crate name)
- Language: Rust (edition 2024)
- Structure: Cargo workspace with three members: `filetag-lib` (core library), `filetag-cli` (CLI binary, published as `filetag`), `filetag-web` (web interface)
- Database: SQLite via rusqlite 0.33 (bundled), per directory tree (`.filetag/db.sqlite3`)
- File tracking: platform file identity (device:inode on Unix), size, mtime
- Tags: flat storage, `/` as separator (genre/rock is a string), `genre/*` glob queries
- Tag values: key=value (year=2024), stored as `value TEXT NOT NULL DEFAULT ''` (empty string = no value)
- Views: symlink-based, relative symlinks
- Relative paths in DB for portability

## Dependencies (workspace)

```toml
# workspace.dependencies (shared versions)
anyhow = "1.0"
rusqlite = { version = "0.33", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
walkdir = "2"

# filetag-lib only
dirs = "6"

# filetag-cli only
clap = { version = "4", features = ["derive"] }
clap_complete = "4"
indicatif = "0.17"

# filetag-web only
axum = "0.7"
tokio = { version = "1", features = ["full"] }
tower-http = { version = "0.5", features = ["fs"] }
clap = { version = "4", features = ["derive"] }
```

## Source Files

### filetag-lib/src/lib.rs
Library entry point. Re-exports modules and defines `TagList` type alias.

### filetag-lib/src/db.rs (~500 lines)
Database module. Key functions:
- `init(root)` - creates `.filetag/db.sqlite3`
- `find_and_open(start)` - walks parents until DB found
- `migrate(conn)` - schema creation + migration (v1→v2: child_databases, v2→v3: file_id)
- `get_or_index_file(conn, rel_path, root)` - indexes file with metadata + file_id
- `get_or_create_tag(conn, name)` - tag CRUD
- `apply_tag()`, `remove_tag()`, `tags_for_file()`, `all_tags()`, `file_by_path()`
- `relative_to_root(path, root)` - resolves abs path to relative path
- `add_child()`, `remove_child()`, `list_children()` - child database registration
- `collect_all_databases(conn, root)` - recursively opens all child DBs (cycle detection)
- `files_under_prefix(conn, prefix)` - file records + tags under a path prefix (for push)
- `all_files_with_tags(conn)` - all file records + tags (for pull)
- `delete_file_by_path(conn, rel_path)` - deletes file record from DB (cascade)
- `FileWithTags` struct: rel_path, file_id, size, mtime_ns, tags
- `OpenDb` struct: conn + root for multi-database operations
- `DirEntry` struct + `list_directory(conn, prefix)` - directory listing for web UI

### filetag-lib/src/query.rs (~300 lines, 9 unit tests)
Recursive descent parser + SQL generator.
- Grammar: `expr = or_expr; or_expr = and_expr ("or" and_expr)*; and_expr = not_expr ("and" not_expr)*`
- Tokenizer supports quoted strings (`"Extra models"`) for tags with spaces
- AST: `Expr::Tag`, `Expr::TagValue(name, CmpOp, value)`, `Expr::Glob`, `Expr::And/Or/Not`
- `QueryBuilder` translates AST to parameterised SQL subqueries
- `execute(conn, expr)` returns `Vec<String>` (paths)
- `execute_with_tags(conn, expr)` returns paths + tags

### filetag-lib/src/registry.rs (~105 lines)
Global database registry (`~/.config/filetag/databases.json`).
- `load()` / `save()` - JSON registry I/O
- `add(root)`, `remove(root)` - register/unregister database roots
- `prune()` - removes dead registrations
- `list()` - all registered roots
- Registration via `filetag init --register` or `filetag db register`

### filetag-lib/src/view.rs (~170 lines, 2 unit tests)
Symlink view generation.
- `generate(root, paths, output_dir)` - creates symlinks
- `path_to_link_name()` - `Music/Album/song.mp3` → `Music__Album__song.mp3`
- `truncate_filename()` - 255 byte limit, preserves extension
- `relative_path()` - relative symlink targets
- `cleanup_broken_symlinks()`, `cleanup_empty_dirs()`

### filetag-cli/src/main.rs
CLI entry point with clap derive. Subcommands: init, tag, untag, tags, show, find, view, status, repair, mv, merge, info, db, completions.
Global options: `--json`, `--color`, `-q`/`--quiet`, `-v`/`--verbose`, `--db`.
Aliases: tag=t, untag=u, tags=ls, show=s, find=f.
Features: stdin pipe support (auto-detect), NUL-delimited I/O (`-0`), JSON Lines output, confirmation prompts for destructive ops.
`--all` flag on `tags` and `find` for cross-database queries.
`db` subcommand with `DbAction` enum: List, Add, Remove, Prune, Push, Pull, Register, Unregister, Registered.
Helper functions: `parse_tag_args()`, `expand_recursive()`, `format_size()`, `collect_files()`, `open_db()`.

### filetag-web/src/main.rs
axum-based web interface. JSON REST API + embedded static frontend.
Routes: `/api/info`, `/api/tags`, `/api/files`, `/api/search`, `/api/file`, `/api/tag`, `/api/untag`, `/api/tag-color`, `/api/delete-tag`, `/preview/*`.
CLI: `filetag-web [--port 3000] [--bind 127.0.0.1] [path]`.
Frontend: grid/list file browser, tag sidebar (grouped by prefix, color dots, right-click context menu for color + delete), search bar (full query language), detail panel with preview + tag management.
Static files embedded via `include_str!` from `filetag-web/static/`.

## Database Schema

```sql
files (id INTEGER PK, path TEXT UNIQUE, file_id TEXT, size INTEGER, mtime_ns INTEGER, indexed_at TEXT)
tags (id INTEGER PK, name TEXT UNIQUE, color TEXT)
file_tags (file_id INTEGER, tag_id INTEGER, value TEXT NOT NULL DEFAULT '', created_at TEXT, PK(file_id, tag_id, value))
child_databases (id INTEGER PK, rel_path TEXT NOT NULL UNIQUE)
```

Schema version 4. Migration from v1→v2 adds `child_databases` table. v2→v3 adds `file_id`, removes `blake3` usage. v3→v4 adds `color` to tags.

## CLI Usage

```
filetag init [--register]               # Create database in CWD (optionally register globally)
filetag tag FILE... -t TAG[,TAG...]     # Tag files (key=value for values)
filetag tag -r DIR -t TAG               # Tag recursively
fd -e flac | filetag tag -t lossless    # Stdin pipe support
filetag untag FILE... -t TAG            # Remove tags
filetag tags [FILE...]                  # Show tags (alias: ls)
filetag show FILE                       # Full file info (alias: s)
filetag find QUERY [--with-tags]        # Query files (alias: f)
filetag find QUERY --count              # Match count only
filetag find QUERY -0 | xargs -0 ...   # NUL-delimited output
filetag view QUERY [-o DIR]             # Symlink views (default: _.tags/)
filetag status [PATH]                   # Missing/untagged
filetag repair [PATH] [--dry-run]       # Find moved files via file_id or name+size
filetag mv OLD NEW                      # Rename a tag
filetag merge SOURCE TARGET [--force]   # Merge tags (destructive, with prompt)
filetag info                            # Database stats
filetag db ls                           # Show registered child databases
filetag db add PATH                     # Register child database
filetag db remove PATH                  # Remove registration
filetag db prune                        # Remove dead registrations
filetag db push PATH [--dry-run]        # Transfer tag records parent → child
filetag db pull PATH [--dry-run]        # Transfer tag records child → parent
filetag db register                     # Register in global registry
filetag db unregister                   # Remove from global registry
filetag db registered                   # Show all globally registered DBs
filetag completions SHELL               # Shell completions (bash/zsh/fish)

# Cross-database queries
filetag tags --all                      # Tags from all child databases
filetag find QUERY --all                # Search across all child databases
filetag tags --all-dbs                  # Tags from all globally registered DBs
filetag find QUERY --all-dbs            # Search across all globally registered DBs

# Global options
--json                                  # JSON Lines output
--color auto|always|never               # Colour mode
-q, --quiet                             # Suppress informational messages
-v, --verbose                           # Extra detail
--db PATH                               # Override database location
```
