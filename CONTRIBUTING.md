# Contributing to filetag

## Prerequisites

- Rust (edition 2024, stable toolchain)
- No system dependencies: SQLite is bundled via `rusqlite`

## Building

```sh
cargo build                    # all workspace members
cargo build -p filetag-cli    # CLI only
cargo build -p filetag-web    # web interface only
```

## Testing

```sh
cargo test                     # all unit tests
cargo clippy --all-targets -- -D warnings
```

All tests are unit tests inside the library crate. There are no integration tests yet.

## Workspace structure

```
filetag/
  Cargo.toml          # workspace root (shared versions in [workspace.dependencies])
  filetag-lib/        # core library (filetag-lib)
  filetag-cli/        # CLI binary (filetag)
  filetag-web/        # web interface binary (filetag-web)
```

### filetag-lib

| File              | Responsibility                                                           |
| :---------------- | :----------------------------------------------------------------------- |
| `src/lib.rs`      | Re-exports modules, defines `TagList` type alias                         |
| `src/db.rs`       | SQLite schema, migrations, all read/write operations                     |
| `src/query.rs`    | Recursive descent parser + SQL generator for the query language          |
| `src/view.rs`     | Symlink view generation                                                  |
| `src/registry.rs` | Global database registry (`~/.config/filetag/databases.json`)            |

### filetag-cli

`src/main.rs` — clap derive CLI. Subcommands map directly to library calls. Handles stdin piping, NUL-delimited I/O, JSON Lines output, confirmation prompts for destructive operations, and cross-database queries.

### filetag-web

`src/main.rs` — axum 0.7 web server. JSON REST API + embedded static files (`include_str!`). Frontend is vanilla HTML/CSS/JS in `static/`.

## Database schema

```sql
files (
    id         INTEGER PRIMARY KEY,
    path       TEXT UNIQUE,          -- relative to database root
    file_id    TEXT,                 -- platform file identity (device:inode on Unix)
    size       INTEGER,
    mtime_ns   INTEGER,
    indexed_at TEXT                  -- ISO 8601
)

tags (
    id   INTEGER PRIMARY KEY,
    name TEXT UNIQUE
)

file_tags (
    file_id    INTEGER REFERENCES files(id) ON DELETE CASCADE,
    tag_id     INTEGER REFERENCES tags(id)  ON DELETE CASCADE,
    value      TEXT NOT NULL DEFAULT '',    -- empty string = no value
    created_at TEXT,
    PRIMARY KEY (file_id, tag_id, value)
)

child_databases (
    id       INTEGER PRIMARY KEY,
    rel_path TEXT NOT NULL UNIQUE   -- relative to this database's root
)
```

Schema version 3. Migration from v1 to v2 adds `child_databases`. Migration from v2 to v3 adds `file_id` and removes `blake3` usage.

## Key design decisions

**SQLite per directory tree.** The database lives next to the files it describes. Moving the directory keeps everything intact. No central server required.

**Relative paths.** All paths stored in the database are relative to the database root. This makes the database portable when the directory is moved or mounted elsewhere.

**File identity tracking.** Files get a platform-specific identifier (`device:inode` on Unix, `None` on other platforms). Used by `repair` to detect files that have been moved or renamed. Falls back to filename+size matching as a heuristic.

**Tags as flat strings with `/` as separator.** `genre/rock` is just a string; the slash is a naming convention. This keeps the schema simple while allowing `genre/*` glob queries to work naturally.

**Tag values.** Stored as `value TEXT NOT NULL DEFAULT ''`. An empty string means "no value". The `key=value` syntax is parsed at the CLI layer, not in the database.

**Symlink views.** The `view` command generates relative symlinks so views remain valid after the root directory is moved.

**Child databases.** Large collections can be split: each sub-directory gets its own `.filetag/` database, registered as a child of the parent. `db push` / `db pull` transfer tag records without touching files. Cross-database queries open all child databases recursively with cycle detection.

## Query language grammar

```
expr        = or_expr
or_expr     = and_expr  ("or"  and_expr)*
and_expr    = not_expr  ("and" not_expr)*
not_expr    = "not" not_expr | atom
atom        = "(" expr ")" | tag_expr
tag_expr    = QUOTED_STRING | NAME (CMP_OP VALUE)?
CMP_OP      = "=" | "!=" | ">=" | "<=" | ">" | "<"
```

Tokeniser supports quoted strings (`"Extra models"`, `'tag with spaces'`) for tags that contain spaces. Glob patterns (`genre/*`) are supported in bare tag names and translated to SQL `LIKE`.

## Web API

| Method | Path             | Description                            |
| :----- | :--------------- | :------------------------------------- |
| GET    | `/api/info`      | Database statistics                    |
| GET    | `/api/tags`      | All tags with counts                   |
| GET    | `/api/files`     | Directory listing (filesystem-based)   |
| GET    | `/api/search`    | Query files using the query language   |
| GET    | `/api/file`      | File detail + tags                     |
| POST   | `/api/tag`       | Add tags to a file (auto-indexes)      |
| POST   | `/api/untag`     | Remove tags from a file                |
| GET    | `/preview/*`     | Serve raw file for preview             |

## Commit style

Plain imperative subject line: `Fix breadcrumb double slash`, `web: add detail panel toggle`. No conventional-commits prefix required except `web:` or `cli:` when the change is scoped to one binary.
