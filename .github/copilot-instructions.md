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
- `migrate(conn)` - schema creation + migration (v1→v2: child_databases, v2→v3: file_id, v3→v4: color, v4→v5: rename to linked_databases, v5→v6: settings)
- `get_or_index_file(conn, rel_path, root)` - indexes file with metadata + file_id
- `get_or_create_tag(conn, name)` - tag CRUD
- `apply_tag()`, `remove_tag()`, `tags_for_file()`, `all_tags()`, `file_by_path()`
- `relative_to_root(path, root)` - resolves abs path to relative path
- `add_child()`, `remove_child()`, `list_children()` - linked database registration
- `collect_all_databases(conn, root, include_parents)` - recursively opens all linked DBs (cycle detection)
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
`--isolated`/`-i` flag on `tags` and `find`: query only the current database (no children, no ancestors). `--all-dbs` flag: query all globally registered databases. Default queries current DB + linked children + ancestor databases.
`db` subcommand with `DbAction` enum: List, Add, Remove, Prune, Push, Pull, Register, Unregister, Registered.
Helper functions: `parse_tag_args()`, `expand_recursive()`, `format_size()`, `collect_files()`, `open_db()`.

### filetag-web/src/main.rs (~266 lines)
Entry point, CLI `Args`, database discovery, router setup, tree display. Delegates to modules below.
CLI: `filetag-web [--port 3000] [--bind 127.0.0.1] [--no-parents] [path]`.
Frontend: grid/list file browser, tag sidebar (grouped by prefix, color dots, right-click context menu for color + delete), search bar (full query language), detail panel with preview + tag management.
Static files embedded via `include_str!` from `filetag-web/static/`.

### filetag-web/src/state.rs (~236 lines)
Core application state and helpers.
- `DbRoot` struct (name, root, db_path, dev, entry_point)
- `AppState` (roots, ai_progress)
- `AppError` (anyhow → 500)
- `open_conn(db_root)` — open DB with WAL + busy_timeout PRAGMAs
- `open_for_file_op(db_root, rel_path)` — **mandatory** gateway for file operations; finds correct DB (child or root) for a given path
- `open_for_file_op_under(root, rel_path)` — same but takes raw `Path` (for background tasks)
- `safe_path()`, `preview_safe_path()` — path traversal protection
- `parse_tag()`, `root_at()`, `file_is_covered()`, `resolve_names()`, `terminal_width()`
- `THUMB_LIMITER` — semaphore for concurrent thumbnail generation

### filetag-web/src/types.rs (~158 lines)
All API request/response structs (~20 structs). No logic.

### filetag-web/src/preview.rs (~1130 lines)
File preview/serving, RAW/HEIC conversion, thumbnailing, video transcoding, trickplay.
- `preview_handler`, `thumb_handler` — main Axum handlers
- `thumb_cached` — deduplicates cache-check-permit-generate-serve pattern
- `serve_file_bytes`, `serve_file_range`, `serve_transcoded_mp4`
- `raw_extract_jpeg`, `image_thumb_jpeg`, `pdf_thumb_jpeg`
- `video_info`, `video_thumb_strip`, `api_vthumbs`, `api_vthumbs_pregen`

### filetag-web/src/archive.rs (~705 lines)
ZIP/RAR/7z archive handling (via `zip`, `unrar`, `sevenz_rust`).
- `archive_cover_image`, `archive_image_entries`, `archive_read_entry`, `archive_list_entries_raw`
- `api_dir_images`, `api_zip_pages`, `api_zip_page`, `api_zip_thumb`, `api_zip_entries`

### filetag-web/src/ai.rs (~1047 lines)
AI/VLM image analysis (OpenAI-compatible + Ollama).
- `AiConfig`, `AiProgress`, `load_ai_config`
- `ai_prepare_jpeg`, `ai_prepare_jpeg_from_bytes`, `vlm_call`, `vlm_call_multi`, `analyse_image`, `analyse_archive`, `parse_ai_tags`
- `apply_ai_tags`, `remove_prefixed_tags`
- `api_ai_analyse` (handles both images and archives), `api_ai_analyse_batch`, `api_ai_status`, `api_ai_clear_tags`
- `api_ai_config_get`, `api_ai_config_set`
- Archive analysis: lists entries, picks sample images, sends entry listing + images to VLM

### filetag-web/src/api.rs (~571 lines)
Core CRUD API handlers + static file serving.
- `api_roots`, `api_reorder_roots`, `api_rename_db`, `api_info`, `api_cache_clear`
- `api_tags`, `api_files`, `api_search`, `api_file_detail`
- `api_tag`, `api_untag` — use `open_for_file_op` for correct child-DB routing
- `api_tag_color`, `api_delete_tag`
- `index_html`, `style_css`, `app_js`, `favicon` — embedded static assets

## Database Schema

```sql
files (id INTEGER PK, path TEXT UNIQUE, file_id TEXT, size INTEGER, mtime_ns INTEGER, indexed_at TEXT)
tags (id INTEGER PK, name TEXT UNIQUE, color TEXT)
file_tags (file_id INTEGER, tag_id INTEGER, value TEXT NOT NULL DEFAULT '', created_at TEXT, PK(file_id, tag_id, value))
linked_databases (id INTEGER PK, path TEXT NOT NULL UNIQUE)
settings (key TEXT PK, value TEXT NOT NULL DEFAULT '')
```

Schema version 6. v1→v2: `child_databases` table. v2→v3: `file_id`, removes `blake3`. v3→v4: `color` on tags. v4→v5: rename `child_databases`→`linked_databases`, `rel_path`→`path`. v5→v6: `settings` table.

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

# Cross-database queries (default includes linked children + ancestors)
filetag tags                            # Tags from current DB + linked children + ancestors
filetag tags --isolated                 # Current database only (alias: -i)
filetag tags --all-dbs                  # All globally registered DBs
filetag find QUERY --isolated           # Current database only
filetag find QUERY --all-dbs            # All globally registered DBs

# Global options
--json                                  # JSON Lines output
--color auto|always|never               # Colour mode
-q, --quiet                             # Suppress informational messages
-v, --verbose                           # Extra detail
--db PATH                               # Override database location
```
