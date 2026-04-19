# filetag

> Local-first file tagging: arbitrary labels, boolean queries, hierarchical tags, and a built-in web interface.

A command-line tool for tagging files with arbitrary labels and querying them with boolean expressions. Tags are stored in a local SQLite database (`.filetag/db.sqlite3`) next to your files — self-contained, portable, and independent of any external service.

Use it to organise collections that don't fit neatly into folders: photos, music, scans, drawings, documents. Tag a file with as many labels as you like (`genre/rock`, `year=2024`, `favourite`), then find everything matching a query in milliseconds.

A web interface is included for browsing, previewing, searching, and tagging through a browser.

## Features

- Per-directory SQLite database (`.filetag/db.sqlite3`) with relative paths — move the directory and everything still works
- Boolean query language with `and`, `or`, `not`, glob patterns (`genre/*`), and value comparisons (`year>=2020`)
- Symlink-based views for integration with other tools
- JSON Lines output (`--json`) for composability with `jq` and scripts
- Stdin support: pipe file paths from `fd`, `find`, or other queries
- NUL-delimited I/O (`-0`) for safe handling of paths with special characters
- Detects missing and untagged files
- Repairs moved files by matching file identity (inode) or name+size
- Tag renaming (`mv`) and merging (`merge`)
- Hierarchical child databases with push/pull transfer
- Cross-database queries across child and ancestor databases; optional global registry
- Shell completions for bash, zsh, and fish
- Web interface with grid/list browser, image/video/PDF previews, trickplay hover animation, and optional AI image and video analysis
- Optional password authentication for the web interface (`--password` or `$FILETAG_PASSWORD`)

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
find . -name '*.mp3' -print0 | filetag untag -0 -t genre

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
filetag find vacation --count        # or: -c
filetag find vacation -0 | xargs -0 ls -l

# JSON output
filetag find 'genre/rock' --json
filetag tags --json
filetag info --json

# Generate a symlink view (Unix only)
filetag view genre/rock -o ~/Views/rock

# Check for missing/modified/untagged files
filetag status
filetag status ./Music       # Limit to a subdirectory

# Find moved files by file identity or name+size
filetag repair
filetag repair ./Music       # Limit to a subdirectory
filetag repair -n            # Dry run (alias: --dry-run)

# Rename a tag
filetag mv old-tag new-tag

# Merge a tag into another (destructive: removes source tag)
filetag merge old-tag target-tag
filetag merge -n old-tag target-tag   # Dry run (alias: --dry-run)
filetag merge -f old-tag target-tag   # Skip confirmation prompt (alias: --force)

# Child database management
filetag db add ./Music                # Register a child database
filetag db ls                         # List registered children
filetag db push ./Music               # Transfer tags to child DB
filetag db push ./Music -n            # Dry run
filetag db pull ./Music               # Transfer tags back to parent DB
filetag db prune                      # Remove dead registrations

# Cross-database queries
filetag tags                          # Tags from current DB and all linked databases
filetag find genre/rock               # Search current DB and all linked databases
filetag tags --isolated               # Tags from current database only (or: -i)
filetag find genre/rock --isolated    # Search current database only (or: -i)

# Global database registry
filetag db register                   # Add current DB to global registry
filetag db unregister                 # Remove from global registry
filetag db registered                 # List all globally registered databases
filetag tags --all-dbs                # Tags across all registered databases
filetag find genre/rock --all-dbs     # Search across all registered databases

# Database statistics
filetag info

# Shell completions
filetag completions zsh  > ~/.zfunc/_filetag
filetag completions bash > ~/.bash_completion.d/filetag
filetag completions fish > ~/.config/fish/completions/filetag.fish
```

## Global options

```
--json              JSON Lines output (one object per line)
--color <WHEN>      auto | always | never (default: auto)
-q, --quiet         Suppress informational messages
-v, --verbose       Extra detail
--db <PATH>         Use a specific database (override auto-detect)
--no-parents        Do not automatically include ancestor databases
```

`tags` and `find` include all linked child databases and ancestor databases by default. Use `-i`/`--isolated` on those commands to query only the current database (no children, no ancestors).

## Command aliases

| Command | Alias |
| :------ | :---- |
| `tag`   | `t`   |
| `untag` | `u`   |
| `tags`  | `ls`  |
| `show`  | `s`   |
| `find`  | `f`   |

## Query language

Queries support boolean logic, glob patterns, and file type filters:

```
genre/rock                        # exact tag
genre/*                           # glob
year=2024                         # exact value
year>=2020                        # comparison (>=, <=, >, <)
genre/rock and not live           # boolean
(genre/rock or genre/metal) and year>=2020
type:image                        # file type filter
type:video and year>=2020         # combine with other expressions
type:audio and genre/*            # find tagged audio files
```

**Supported type names:** `image` (aliases: `img`, `photo`, `pic`), `video` (aliases: `vid`, `movie`), `audio` (aliases: `aud`, `music`), `document` (alias: `doc`), `archive` (aliases: `arc`, `compressed`), `text`, `font`. Type filters match by file extension.

## Web interface

`filetag-web` provides a browser-based file manager with tag sidebar, search, grid/list views, and file previews.

```sh
# Start in the current directory (must contain a .filetag database or a parent must)
filetag-web

# Specify a directory and port
filetag-web ~/Music --port 8080

# Bind to all interfaces (e.g. for LAN access)
filetag-web --bind 0.0.0.0

# Read the password from a file (recommended for regular use, see Authentication below)
filetag-web --password-file ~/.filetag-password

# Generate a random password for ad-hoc access (printed to the terminal)
filetag-web -P --bind 0.0.0.0

# Suppress automatic ancestor database discovery
filetag-web --no-parents
```

Open `http://127.0.0.1:3000` (default) in your browser. The full query language works in the search bar.

### Authentication

By default filetag-web binds to `127.0.0.1` (loopback only) and requires no password. When you bind to a non-loopback address without a password, a warning is printed at startup.

To require a password, use one of these options. `--password-file` takes precedence over `--password` and `$FILETAG_PASSWORD`.

**Generated password (easiest for ad-hoc use).** filetag-web generates a random password and prints it to the terminal. Valid for the current session only.

```sh
filetag-web -P --bind 0.0.0.0
# Generated password: a3Kx-9mRp-Zq2w-Lf7v
```

**Password file (recommended for regular use).** The password never appears in your shell history or process listing. Create the file with your editor, or type the password interactively:

```sh
# Type the password without it appearing on screen; press Enter when done.
read -rs FTPW && printf '%s' "$FTPW" > ~/.filetag-password && unset FTPW
chmod 600 ~/.filetag-password

filetag-web --password-file ~/.filetag-password
```

**Environment variable.** Useful in systemd units (via `EnvironmentFile=`) or Docker (`--env-file`). Do not assign it inline on the command line — that ends up in history too.

```sh
# In a systemd unit (EnvironmentFile=/etc/filetag.env):
#   FILETAG_PASSWORD=mysecret
# Then:
filetag-web --bind 0.0.0.0

# Interactive: type without echo, export, then run.
read -rs FILETAG_PASSWORD && export FILETAG_PASSWORD
filetag-web
```

**Command-line flag.** Convenient for quick tests, but visible in shell history and `ps aux`.

```sh
filetag-web --password mysecret
```

When a password is set:

- A login page is served at `/login`.
- Successful login sets an `HttpOnly`, `SameSite=Strict` session cookie (`ft_session`).
- API requests without a valid session cookie receive `401 Unauthorized`; page requests are redirected to `/login`.
- A logout button appears in the toolbar.
- Session tokens are kept in memory and lost on server restart (users must log in again).

### Database scope in filetag-web

filetag-web intentionally has a broader database scope than the CLI:

At startup it loads the primary database, all explicitly linked child databases, and all ancestor databases (same as the CLI). It then also performs a recursive filesystem scan under each loaded root to find any nested `.filetag/` databases, up to 10 levels deep. This means that every sub-directory with its own database is automatically included in the session, whether or not it was registered with `filetag db add`.

The reason for this difference: in the web interface the user browses the full directory tree and expects that what they can see they can also search and tag consistently. Tags always land in the most specific database for a file (the innermost `.filetag/` that covers it), and searches cover all loaded databases, so browsing, tagging, and searching are always in sync.

The CLI does not scan automatically because it operates from a working directory and follows explicit intent: unexpected databases in unrelated sub-trees would be surprising there.

### File previews

Double-clicking a file in the grid or list opens a preview. Supported types:

| Type                                          | How it works |
| :-------------------------------------------- | :--- |
| JPEG, PNG, GIF, WebP, SVG, BMP, AVIF          | Served as-is; displayed in the image viewer |
| RAW camera files (ARW, CR2, CR3, NEF, DNG, …) | Embedded JPEG extracted with pure-Rust parser first; falls back to `dcraw` (ImageMagick feature) or `ffmpeg` (Video feature); cached in `.filetag/cache/raw/` |
| HEIC / HEIF                                   | Embedded thumbnail extracted with pure-Rust ISOBMFF parser first; falls back to `sips` (macOS built-in, ImageMagick feature) or `ffmpeg` (Video feature); cached in `.filetag/cache/raw/` |
| Video (MP4, MKV, MOV, …)                      | Streamed; thumbnail is a 2×2 contact sheet via `ffmpeg`; cached in `.filetag/cache/thumbs/`. Hovering a video card in the grid shows a trickplay preview (sprite sheet of evenly-spaced frames, generated by `ffmpeg`; cached in `.filetag/cache/vthumbs/`) |
| Audio (MP3, FLAC, WAV, …)                     | Played in the browser's `<audio>` element |
| PDF                                           | Served as `application/pdf`; rendered by the browser's built-in PDF viewer (see note below) |
| Markdown                                      | Rendered to HTML in the browser |
| Text, source code                             | Displayed as plain text |
| ZIP / CBZ                                     | First image page extracted and resized as thumbnail; opened with the built-in comic/image viewer |

**Thumbnail cache.** All generated thumbnails (resized images, RAW previews, video contact sheets) are written to `.filetag/cache/thumbs/` or `.filetag/cache/raw/`. They are keyed by mtime and file size, so stale entries accumulate when files are replaced. Use the refresh button (↺) in the toolbar to clear the cache for the current directory, or the drop-down next to it to clear the entire cache.

**Settings.** The gear icon in the toolbar opens the settings dialog, which has four tabs:

- **Video:** configures the trickplay sprite count. The number of frames is computed automatically from video duration (1 frame per 30 seconds), clamped to the configured minimum and maximum. Lower values generate faster; defaults are min 8, max 16.
- **Features:** opt-in feature flags for functionality that requires external tools. All are off by default. Settings are saved per database root.
  - *Video* — enables `ffmpeg` for video transcoding, HLS streaming, trickplay sprite generation, and video/image thumbnail fallback.
  - *ImageMagick* — enables `magick`/`convert` for exotic image formats (PSD, XCF, EPS, ...) and `sips` (macOS built-in) for HEIC/HEIF files including HEVC-encoded dynamic wallpapers. Also enables `dcraw` as a RAW extraction fallback.
  - *PDF thumbnails* — enables `pdftoppm` (poppler) or ImageMagick+Ghostscript for PDF thumbnail generation.
- **AI:** connection settings for the optional AI image analysis feature. Configure the endpoint URL, API format, model, API key, tag prefix, and max tokens here. Use the "Test connection" button to verify the setup. Accessible via the "Analyse images (AI)" option in the cache drop-down.
- **Prompts:** customise how the AI interprets your files.
  - *Collection description* — free-text description of what this collection contains (e.g. "Family photos and videos, 2010–present" or "Bird photography: species identification, behaviour, and habitat"). Injected into every analysis prompt as context.
  - *Type instructions* — per-type opening sentence sent before the output-format instruction. Separate overrides for images, videos, and archives. Leave empty to use the built-in defaults. Use the "Use default" button to restore a type's default.
  - *Output format* — the fixed JSON output instruction appended to every prompt (shown for reference; not editable).

### AI image and video analysis

filetag-web can send files to any OpenAI-compatible or Ollama VLM endpoint and apply the returned tags to the database automatically. To use it:

1. Open Settings (gear icon) and go to the **AI** tab.
2. Configure the endpoint URL, API format (`openai` or `ollama`), model name, and tag prefix. For local models (llama.cpp, LM Studio, Ollama) no API key is needed.
3. Optionally go to the **Prompts** tab to describe the collection and customise per-type instructions.
4. Click "Analyse images (AI)" in the cache drop-down to analyse all image files in the current directory, or right-click a single file in the detail panel and choose "Analyse with AI".

**What gets analysed:**

| File type    | How it is sent to the model |
| :----------- | :-------------------------- |
| JPEG, PNG, WebP, HEIC, RAW, … | Resized to max 800 px, stripped of metadata, sent as a base64 JPEG |
| Video (MP4, MKV, MOV, …) | A trickplay-style contact sheet of evenly-spaced frames is generated and sent as a JPEG. The number of frames is derived from video duration (1 per 30 s), clamped to the sprite min/max from Video settings. AI sprite sheets are cached separately in `.filetag/cache/ai_sprites/` and do not interfere with the trickplay cache in `.filetag/cache/vthumbs/`. |
| ZIP / CBZ / RAR / 7z | The archive's file listing plus up to five sample images extracted from it are sent together |

The model is instructed to return a JSON array of tags (`["tag1", "key=value", …]`). The configured tag prefix (default `ai/`) is prepended to each returned tag before it is applied to the file. Use "Clear AI tags" to remove all tags with that prefix from the current directory.

### Image and comic viewer

Images (JPEG, PNG, WebP, RAW, HEIC, etc.) open in a full-screen viewer that also doubles as a comic book reader for ZIP/CBZ archives. The viewer loads all images from the same directory as navigation context, so you can page through a folder without leaving the viewer.

**Keyboard shortcuts:**

| Key               | Action |
| :---------------- | :----- |
| `←` / `→` or `A` / `D` | Previous / next image or page |
| `+` / `-`         | Zoom in / out |
| `0`               | Reset zoom and pan |
| `V`               | Toggle vertical scroll mode (continuous scroll) |
| `H`               | Toggle horizontal scroll mode (side-by-side scroll) |
| `S`               | Toggle two-page spread |
| `T`               | Toggle thumbnail strip |
| `R`               | Toggle right-to-left (manga) mode |
| `F`               | Toggle full-screen |
| `Escape`          | Close viewer |

**Mouse / trackpad:**

| Gesture | Action |
| :------ | :----- |
| Scroll (no modifier) | Pan (when zoomed in single-page mode), or scroll pages (scroll modes) |
| Pinch / Ctrl+scroll  | Zoom towards cursor position |
| Drag                 | Pan (when zoomed) |
| Double-click (centre zone) | Zoom in; double-click again to reset |
| Click left/right edge | Previous / next page |

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

#### Database discovery rules

filetag uses two directions of discovery:

- **Upward (ancestors):** automatic. When your current directory has a database, all ancestor directories with a database are automatically included. This means tags on files under `~/` are always visible when working from `~/Documents`, even if `~/` was never explicitly linked. Use `--no-parents` to suppress this.

- **Downward (children):** explicit only, via `filetag db add`. filetag does not automatically scan subdirectories for databases, because it cannot know which sub-trees belong to your workflow. An unregistered child (e.g. `~/Documents/Work` with its own database) is never included in queries unless you register it.

This means that by default `tags` and `find` include the current database, all registered child databases (recursively), and all ancestor databases. Use `--isolated` to query only the current database in isolation, ignoring both children and ancestors.

### Global registry

Databases can be registered in `~/.config/filetag/databases.json` via `filetag init --register` or `filetag db register`. Use `--all-dbs` on `tags` and `find` to query across all registered databases, even in unrelated directory trees.

```sh
filetag find genre/rock --all-dbs    # search everywhere
filetag db registered                # see all known databases
filetag db prune                     # clean up dead entries
```

## License

MIT
