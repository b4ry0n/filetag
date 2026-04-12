# filetag

> Local-first file tagging: arbitrary labels, boolean queries, hierarchical tags, and a built-in web interface.

A command-line tool for tagging files with arbitrary labels and querying them with boolean expressions. Tags are stored in a local SQLite database alongside your files; nothing is uploaded, synced, or tracked outside your own directories.

Use it to organise collections that don't fit neatly into folders: photos, music, scans, drawings, documents. Tag a file with as many labels as you like (`genre/rock`, `year=2024`, `favourite`), then find everything matching a query in milliseconds.

A web interface is included for browsing, searching, and tagging through a browser.

## Features

- Per-directory SQLite database (`.filetag/db.sqlite3`), portable relative paths
- Boolean query language with `and`, `or`, `not`, glob patterns (`genre/*`), and value comparisons (`year>=2020`)
- Symlink-based views for integration with other tools
- JSON Lines output (`--json`) for composability with `jq` and scripts
- Stdin support: pipe file paths from `fd`, `find`, or other queries
- NUL-delimited I/O (`-0`) for safe handling of paths with special characters
- Detects missing and untagged files
- Repairs moved files by matching file identity (inode) or name+size
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

# Find moved files by file identity or name+size
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

### File previews

Double-clicking a file in the grid or list opens a preview. Supported types:

| Type | How it works |
| :--- | :--- |
| JPEG, PNG, GIF, WebP, SVG, BMP, AVIF | Served as-is; displayed in the browser |
| RAW camera files (ARW, CR2, CR3, NEF, DNG, ...) | Embedded JPEG extracted via `dcraw`/`exiftool`; cached in `.filetag/cache/raw/` |
| HEIC / HEIF | Converted to JPEG via `magick` or `ffmpeg`; cached in `.filetag/cache/raw/` |
| Video (MP4, MKV, MOV, ...) | Streamed; thumbnail is a 2×2 contact sheet via `ffmpeg`; cached in `.filetag/cache/thumbs/` |
| Audio (MP3, FLAC, WAV, ...) | Played in the browser's `<audio>` element |
| PDF | Served as `application/pdf`; rendered by the browser's built-in PDF viewer (see note below) |
| Markdown | Rendered to HTML in the browser |
| Text, source code | Displayed as plain text |
| ZIP / CBZ | First image page extracted and resized as thumbnail; opened with the built-in comic viewer |

**Thumbnail cache.** All generated thumbnails (resized images, RAW previews, video contact sheets) are written to `.filetag/cache/thumbs/` or `.filetag/cache/raw/`. They are keyed by mtime and file size, so stale entries accumulate when files are replaced. Use the refresh button (↺) in the toolbar to clear the cache for the current directory, or the drop-down next to it to clear the entire cache.

## Data safety

filetag never modifies, moves, or deletes your files. It only reads them to collect metadata. All tag data lives in `.filetag/db.sqlite3`; all generated caches live under `.filetag/cache/`. Nothing is written outside that directory: no temp files, no global state (except the optional global registry described below).

The only file written outside `.filetag/` is the optional global registry (`~/.config/filetag/databases.json`), created only when you explicitly run `filetag db register` or `filetag init --register`.

To completely remove filetag from a directory tree, delete the `.filetag/` folder. If you previously registered the database, also run `filetag db unregister` (or manually remove the entry from `~/.config/filetag/databases.json`).

### Caching in the web interface

`filetag-web` caches two kinds of derived data, all inside `.filetag/cache/`:

| Directory | Contents | Keyed by |
| :-------- | :------- | :------- |
| `.filetag/cache/thumbs/` | JPEG thumbnails for images, RAW, video, ZIP (400 px) | filename + mtime + file size |
| `.filetag/cache/raw/` | Full-resolution JPEG conversions of RAW/HEIC files | filename + mtime + file size |

These files are safe to delete at any time; they will be regenerated on demand.

### Browser-side caching

The server does not send `Cache-Control` headers, so the browser may cache previewed files in its HTTP cache (typically in memory or in the browser's own disk cache, not on your filesystem). The categories below describe what reaches the browser and what stays server-side:

| What | Leaves the server? | Where the browser may cache it |
| :--- | :--- | :--- |
| JPEG / PNG / WebP / GIF originals | Yes, full file | Browser HTTP cache (memory + disk) |
| RAW files | No. Only the extracted JPEG is sent | Browser HTTP cache |
| HEIC / HEIF files | No. Only the converted JPEG is sent | Browser HTTP cache |
| Video files | Yes, streamed in full | Browser may buffer; not written to disk by default |
| PDF files | Yes, full file | Browser HTTP cache; may also be cached by the browser's built-in PDF viewer |
| Audio | Yes, full file | Browser HTTP cache |
| ZIP / CBZ pages | Individual pages only, as JPEG (via `/api/zip/page`) | Browser HTTP cache |

For most use cases (private, single-user, `localhost`) this is not a concern. If you are running `filetag-web` on a shared or public-facing server and want to prevent browser caching of the actual file content, add a reverse proxy (e.g. nginx or Caddy) in front that injects `Cache-Control: no-store` for `/preview/*`, `/thumb/*`, and `/api/zip/page`.

PDF files specifically are rendered entirely within the browser using its built-in PDF viewer (Firefox: PDF.js; Chrome/Safari: native renderer). The PDF bytes are fetched once and displayed in-page; no separate PDF viewer application is invoked and no file is written to your downloads folder, unless you explicitly save or print the document.

## How it works

The database lives in `.filetag/db.sqlite3` at the root of your tagged tree. Files are tracked by relative path. On first tag, the file's size, mtime, and a platform-specific file identifier (device:inode on Unix) are stored.

The `view` command creates a directory of relative symlinks pointing back to the original files, letting you browse query results in any file manager.

The `repair` command scans for files whose paths no longer exist and tries to find them at new locations by matching their file identity (strong match) or filename+size (heuristic match).

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
