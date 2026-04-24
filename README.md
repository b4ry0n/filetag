# filetag

> Local-first file tagging: arbitrary labels, boolean queries, hierarchical tags, and a built-in web interface.

A command-line tool for tagging files with arbitrary labels and querying them with boolean expressions. Tags are stored in a local SQLite database (`.filetag/db.sqlite3`) next to your files — self-contained, portable, and independent of any external service.

Use it to organise collections that don't fit neatly into folders: photos, music, scans, drawings, documents. Tag a file with as many labels as you like (`genre/rock`, `year=2024`, `favourite`), then find everything matching a query in milliseconds.

A web interface is included for browsing, previewing, searching, and tagging through a browser.

## Contents

- [Features](#features)
- [Install](#install)
- [Quick start](#quick-start)
- [Commands](#commands)
  - [init](#init)
  - [tag / untag](#tag--untag)
  - [tags / show](#tags--show)
  - [find](#find)
  - [view](#view-unix-only)
  - [status / repair](#status--repair)
  - [mv / merge](#mv--merge)
  - [synonym](#synonym)
  - [db](#db)
  - [info / completions](#info--completions)
- [Global options](#global-options)
- [Query language](#query-language)
- [Web interface](#web-interface)
  - [Options](#options)
  - [Authentication](#authentication)
  - [Database scope](#database-scope-in-filetag-web)
  - [File previews](#file-previews)
  - [AI analysis](#ai-image-and-video-analysis)
  - [Image and comic viewer](#image-and-comic-viewer)
- [How it works](#how-it-works)
  - [Child databases](#child-databases)
  - [Global registry](#global-registry)
- [License](#license)

## Features

**Tagging and querying**

- Tags are stored in a SQLite database (`.filetag/db.sqlite3`) next to your files. Move the directory and everything still works.
- Full boolean query language: `and`, `or`, `not`, glob patterns (`genre/*`), and value comparisons (`year>=2020 and not live`).
- Hierarchical tags with `/` as separator: tag with `genre/rock`, query with `genre/*`.
- Key=value tags for metadata: `year=2024`, `rating=5`.
- Subject-grouped tags: attach a subject label to a set of tags to describe multiple distinct entities within one file (e.g. two cars, two people). Query with `{colour=red and make=Toyota}` to find files where a single subject has both properties.

**Composability**

- Pipe file paths in from `fd`, `find`, or any other tool. NUL-delimited I/O (`-0`) for paths with spaces or special characters.
- JSON Lines output (`--json`) for use with `jq` and scripts.
- Symlink views: generate a directory of symlinks matching a query for use with other tools.
- Shell completions for bash, zsh, and fish.

**Collection maintenance**

- `status`: report missing, modified, and untagged files.
- `repair`: recover moved files by matching file identity (inode) or name+size.
- `mv` and `merge`: rename or consolidate tags across the entire database.

**Multiple databases**

- Link separate databases as children and query them together.
- `push`/`pull` transfer tag records between parent and child databases.
- Optional global registry for cross-collection queries with `--all-dbs`.

**Web interface**

- Grid and list file browser with tag sidebar, colour-coded tags, and right-click context menu.
- Image, video, PDF, and archive previews. Trickplay hover animation for video.
- Optional AI analysis (OpenAI-compatible or Ollama) to auto-tag images, videos, and archives.
- Optional password authentication.

## Install

```sh
cargo install --path filetag-cli           # CLI (binary: filetag)
cargo install --path filetag-web           # Web interface (binary: filetag-web)
```

## Quick start

```sh
filetag init
filetag tag photo.jpg -t vacation,beach,year=2024
filetag find 'vacation and year=2024'
filetag tags
```

## Commands

| Command       | Alias | Description |
| :------------ | :---- | :---------- |
| `init`        |       | Create a database in the current directory |
| `tag`         | `t`   | Add tags to files |
| `untag`       | `u`   | Remove tags from files |
| `tags`        | `ls`  | List tags (all, or for specific files) |
| `show`        | `s`   | Show detailed file information |
| `find`        | `f`   | Find files matching a tag query |
| `view`        |       | Generate a symlink view (Unix only) |
| `status`      |       | Show missing, modified, and untagged files |
| `repair`      |       | Recover moved files by file identity or name+size |
| `mv`          |       | Rename a tag |
| `merge`       |       | Merge a tag into another (destructive) |
| `synonym`     |       | Manage tag synonyms |
| `db`          |       | Manage linked databases |
| `info`        |       | Show database statistics |
| `completions` |       | Generate shell completions |

### init

```sh
filetag init [--register]
```

| Option       | Description |
| :----------- | :---------- |
| `--register` | Also register in the global database registry |

### tag / untag

```sh
filetag tag FILE... -t TAG[,TAG...]
filetag untag FILE... -t TAG[,TAG...]
fd -e flac | filetag tag -t lossless
find . -name '*.jpg' -print0 | filetag tag -0 -t photo

# Tag properties of a specific subject within a file
filetag tag photo.jpg -t colour=red,make=Toyota -s car-1
filetag tag photo.jpg -t colour=blue,make=BMW -s car-2
filetag untag photo.jpg -t colour=red -s car-1   # remove one tag from a subject
filetag untag photo.jpg -t colour=red            # remove from all subjects
```

| Option             | Description |
| :----------------- | :---------- |
| `-t, --tags`       | Tags to add or remove, comma-separated; use `key=value` for values |
| `-s, --subject`    | Group these tags under a subject label (e.g. `car-1`, `person/alice`); omit for subject-less tags |
| `-r, --recursive`  | Tag all files under the given directories (`tag` only) |
| `-0, --null`       | Read NUL-delimited paths from stdin |

A **subject** is a free-form label (any non-empty string) that groups related tags within a single file. It is useful when a file contains multiple distinct entities whose properties you want to track separately — for example two cars in one photo, or two speakers in one recording. Subject-less tags (the default) work exactly as before.

### tags / show

```sh
filetag tags                    # all tags
filetag tags song.mp3           # tags for a specific file
filetag show photo.jpg          # full file info
```

| Option (`tags`)  | Description |
| :--------------- | :---------- |
| `-i, --isolated` | Query only the current database (no children, no ancestors) |
| `--all-dbs`      | Search across all globally registered databases |

### find

```sh
filetag find genre/rock
filetag find 'genre/rock and year>=2020' --with-tags
filetag find vacation --count
filetag find vacation -0 | xargs -0 ls -l
```

| Option           | Description |
| :--------------- | :---------- |
| `--with-tags`    | Include tags alongside file paths in output |
| `-c, --count`    | Print only the number of matches |
| `-0, --null`     | NUL-delimited output (for `xargs -0`) |
| `-i, --isolated` | Query only the current database |
| `--all-dbs`      | Search across all globally registered databases |

### view (Unix only)

```sh
filetag view 'genre/rock and year>=2020' -o ~/Views/rock
```

| Option               | Default  | Description |
| :------------------- | :------- | :---------- |
| `-o, --output <DIR>` | `_.tags` | Output directory for the generated symlinks |

### status / repair

```sh
filetag status
filetag status ./Music          # limit to a subdirectory
filetag repair
filetag repair ./Music -n       # dry run, limit to a subdirectory
```

| Option (`repair`) | Description |
| :---------------- | :---------- |
| `-n, --dry-run`   | Show what would change without modifying anything |

### mv / merge

```sh
filetag mv old-tag new-tag
filetag merge old-tag target-tag
```

| Option (`merge`) | Description |
| :--------------- | :---------- |
| `-f, --force`    | Skip the confirmation prompt |
| `-n, --dry-run`  | Show what would change without modifying anything |

### synonym

Synonyms map an alias to a canonical tag. When a file is tagged with the alias, the canonical tag is applied instead.

```sh
filetag synonym add pic image     # 'pic' is an alias for 'image'
filetag synonym remove pic
filetag synonym ls
```

### db

Linked databases are separate `.filetag` roots that are queried together.

| Subcommand          | Description |
| :------------------ | :---------- |
| `db ls`             | List all linked databases |
| `db add <PATH>`     | Register a database root as a child |
| `db remove <PATH>`  | Remove a child registration |
| `db prune`          | Remove registrations for databases that no longer exist |
| `db push <PATH>`    | Copy tags for files under PATH from this DB to the child |
| `db pull <PATH>`    | Copy tags from the child DB back to this DB |
| `db register`       | Add this database to the global registry |
| `db unregister`     | Remove from the global registry |
| `db registered`     | List all globally registered databases |

`db push` and `db pull` accept `-n`/`--dry-run`.

### info / completions

```sh
filetag info
filetag completions zsh  > ~/.zfunc/_filetag
filetag completions bash > ~/.bash_completion.d/filetag
filetag completions fish > ~/.config/fish/completions/filetag.fish
```

## Global options

| Option           | Description |
| :--------------- | :---------- |
| `--json`         | JSON Lines output (one object per line) |
| `--color <WHEN>` | `auto` \| `always` \| `never` (default: `auto`) |
| `-q, --quiet`    | Suppress informational messages |
| `-v, --verbose`  | Extra detail |
| `--db <PATH>`    | Use a specific database (override auto-detect) |
| `--no-parents`   | Do not automatically include ancestor databases |

`tags` and `find` include all linked child databases and ancestor databases by default. Use `-i`/`--isolated` to query only the current database (no children, no ancestors).

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

{colour=red and make=Toyota}      # subject query: a single subject has both
{colour=red} and {colour=blue}    # file has one subject that is red AND another that is blue
```

**Subject queries** (`{...}`) match files where at least one subject satisfies all the conditions inside the braces simultaneously. Without braces, `colour=red and colour=blue` would match a file that has those tags on *any* combination of subjects; with braces, both tags must belong to the *same* subject.

**Supported type names:** `image` (aliases: `img`, `photo`, `pic`), `video` (aliases: `vid`, `movie`), `audio` (aliases: `aud`, `music`), `document` (alias: `doc`), `archive` (aliases: `arc`, `compressed`), `text`, `font`. Type filters match by file extension.

## Web interface

`filetag-web` provides a browser-based file manager with tag sidebar, search, grid/list views, and file previews.

```sh
filetag-web [PATH] [OPTIONS]
```

Open `http://127.0.0.1:3000` (default) in your browser. The full query language works in the search bar.

### Subjects in the web interface

The detail panel shows each file's tags grouped by subject. Tags without a subject are listed first. Tags that belong to a subject appear together in a labelled box.

To add a tag with a subject:

1. Open the detail panel for a file (click the file).
2. Type the tag in the **Add tag** field (e.g. `colour=red`).
3. Type the subject label in the **subject** field next to it (e.g. `car-1`).
4. Click **Add** (or press Enter).

Leave the subject field empty to add a subject-less tag.

To remove all tags of a subject at once, click the × button on the subject box.

### Options

| Option                      | Default       | Description |
| :-------------------------- | :------------ | :---------- |
| `PATH`                      | `.`           | Directory to serve (must contain a `.filetag` database, or a parent must) |
| `-p, --port <PORT>`         | `3000`        | Port to listen on |
| `-b, --bind <ADDR>`         | `127.0.0.1`   | Address to bind to (`0.0.0.0` for all interfaces) |
| `--password <SECRET>`       |               | Require a password (also `$FILETAG_PASSWORD`); see Authentication below |
| `--password-file <PATH>`    |               | Read the password from a file; takes precedence over `--password` |
| `-P, --generate-password`   |               | Generate a random password and print it; useful for ad-hoc access |
| `--no-parents`              |               | Do not include ancestor databases in the session |
| `--no-scan`                 |               | Skip the startup scan for nested databases |

### Authentication

By default filetag-web binds to `127.0.0.1` (loopback only) and requires no password. When you bind to a non-loopback address without a password, a warning is printed at startup.

`--password-file` takes precedence over `--password` and `$FILETAG_PASSWORD`.

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

Use `--no-scan` to skip that recursive discovery step and start faster. With `--no-scan`, the session includes only the primary database, explicitly linked child databases, and any ancestor databases that are still enabled.

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
