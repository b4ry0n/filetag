# filetag

SQLite-backed file tagging CLI with content hashing and symlink views.

Tag any file with arbitrary tags (including `key=value` pairs), query them with boolean expressions, and generate symlink views of the results.

## Features

- Per-directory SQLite database (`.filetag/db.sqlite3`), portable relative paths
- BLAKE3 content hashing for file identity (lazy: only hashes on first tag)
- Boolean query language with `and`, `or`, `not`, glob patterns (`genre/*`), and value comparisons (`year>=2020`)
- Symlink-based views for integration with other tools
- JSON Lines output (`--json`) for composability with `jq` and scripts
- Stdin support: pipe file paths from `fd`, `find`, or other queries
- NUL-delimited I/O (`-0`) for safe handling of paths with special characters
- Detects missing, modified, and untagged files
- Repairs moved files by matching BLAKE3 hashes
- Tag renaming (`mv`) and merging (`merge`)
- Hierarchical child databases with push/pull transfer
- Cross-database queries (`--all`) across child databases
- Shell completions for bash, zsh, and fish

## Install

```sh
cargo install --path filetag-cli           # CLI (binary: filetag)
cargo install --path filetag-web           # Web interface (binary: filetag-web)
```

## Quick start

```sh
# Initialize a database in the current directory
filetag init

# Tag files (comma-separated or repeated -t)
filetag tag photo.jpg -t vacation,beach,year=2024
filetag tag *.mp3 -t genre/rock -t year=2024
filetag tag -r ./Music -t collection/main

# Pipe file paths from other tools
fd -e flac | filetag tag -t lossless
find . -name '*.jpg' -print0 | filetag tag -0 -t photo

# List all tags
filetag tags

# List tags for a specific file
filetag tags song.mp3

# Detailed file info
filetag show photo.jpg

# Find files by tag query
filetag find genre/rock
filetag find 'genre/rock and year>=2020'
filetag find 'genre/* and not live' --with-tags
filetag find vacation --count
filetag find vacation -0 | xargs -0 ls -l

# JSON output
filetag find 'genre/rock' --json
filetag tags --json
filetag info --json

# Generate a symlink view
filetag view genre/rock -o ~/Views/rock

# Check for missing/modified/untagged files
filetag status

# Find moved files by content hash
filetag repair
filetag repair --dry-run

# Rename a tag
filetag mv old-tag new-tag

# Merge a tag into another
filetag merge old-tag target-tag
filetag merge --dry-run old-tag target-tag

# Child database management
filetag db add ./Music                # Register a child database
filetag db ls                         # List registered children
filetag db push ./Music               # Transfer tags to child DB
filetag db push ./Music -n            # Dry run
filetag db pull ./Music               # Transfer tags back to parent DB
filetag db prune                      # Remove dead registrations

# Cross-database queries
filetag tags --all                    # Tags from all child databases
filetag find genre/rock --all         # Search across all databases

# Global database registry
filetag db register                   # Add current DB to global registry
filetag db unregister                 # Remove from global registry
filetag db registered                 # List all globally registered databases
filetag tags --all-dbs                # Tags across all registered databases
filetag find genre/rock --all-dbs     # Search across all registered databases

# Database statistics
filetag info

# Shell completions
filetag completions zsh >> ~/.zfunc/_filetag
filetag completions bash >> ~/.bash_completion.d/filetag
filetag completions fish > ~/.config/fish/completions/filetag.fish
```

## Global options

```
--json              JSON Lines output (one object per line)
--color <WHEN>      auto | always | never (default: auto)
-q, --quiet         Suppress informational messages
-v, --verbose       Extra detail
--db <PATH>         Use a specific database (override auto-detect)
```

## Command aliases

| Command | Alias |
| :------ | :---- |
| `tag`   | `t`   |
| `untag` | `u`   |
| `tags`  | `ls`  |
| `show`  | `s`   |
| `find`  | `f`   |

## Query language

Queries support boolean logic and glob patterns:

```
genre/rock                        # exact tag
genre/*                           # glob
year=2024                         # exact value
year>=2020                        # comparison (>=, <=, >, <)
genre/rock and not live           # boolean
(genre/rock or genre/metal) and year>=2020
```

## Web interface

`filetag-web` provides a browser-based file manager with tag sidebar, search, grid/list views, and file previews.

```sh
# Start in the current directory (must contain a .filetag database or a parent must)
filetag-web

# Specify a directory and port
filetag-web ~/Music --port 8080

# Bind to all interfaces (e.g. for LAN access)
filetag-web --bind 0.0.0.0
```

Open `http://127.0.0.1:3000` (default) in your browser. The full query language works in the search bar.

## Data safety

filetag never modifies, moves, or deletes your files. It only reads them to collect metadata and compute hashes. All tag data lives in `.filetag/db.sqlite3`. Nothing is written outside that directory: no caches, no temp files, no global state. The only exception is the optional global registry (`~/.config/filetag/databases.json`), which is created only when you explicitly run `filetag db register` or `filetag init --register`.

To completely remove filetag from a directory tree, delete the `.filetag/` folder. If you previously registered the database, also run `filetag db unregister` (or manually remove the entry from `~/.config/filetag/databases.json`).

## How it works

The database lives in `.filetag/db.sqlite3` at the root of your tagged tree. Files are tracked by relative path. On first tag, the file's BLAKE3 hash, size, and mtime are stored. Subsequent operations skip rehashing if size and mtime are unchanged.

The `view` command creates a directory of relative symlinks pointing back to the original files, letting you browse query results in any file manager.

The `repair` command scans for files whose paths no longer exist and tries to find them at new locations by matching their BLAKE3 hash.

### Child databases

For large collections you can split the database hierarchy: initialize separate databases in subdirectories and register them as children of the parent.

```sh
filetag init                # parent at .
cd Music && filetag init    # child at ./Music
cd .. && filetag db add ./Music
```

`db push` transfers tag records for files under the child path from the parent database to the child database. `db pull` does the reverse. Files on disk are never touched. `--all` on `tags` and `find` queries across the entire tree. Child discovery is recursive with cycle detection.

### Global registry

Databases can be registered in `~/.config/filetag/databases.json` via `filetag init --register` or `filetag db register`. Use `--all-dbs` on `tags` and `find` to query across all registered databases, even in unrelated directory trees.

```sh
filetag find genre/rock --all-dbs    # search everywhere
filetag db registered                # see all known databases
filetag db prune                     # clean up dead entries
```

## License

MIT
