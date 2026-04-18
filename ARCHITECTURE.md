# filetag Architecture

This document describes the core architectural concepts and constraints of filetag. It is the authoritative reference for contributors making structural decisions.

---

## 1. The Root

### 1.1 Definition

A **Root** is a directory that has been initialised as a filetag tagging scope by running `filetag init`. Initialisation creates a `.filetag/` subdirectory inside the Root, which holds:

- `db.sqlite3` — the SQLite database containing all file records, tags, and settings.
- `cache/` — derived artefacts such as thumbnails, RAW previews, and trickplay sprites.
- `tmp/` — short-lived intermediate files (cleaned up after use).

The canonical identifier for a Root is the absolute, canonicalised path of its directory.

### 1.2 Invariants

The following invariants hold for every valid Root at all times:

1. **Co-location.** The `.filetag/` directory is always located directly inside the Root directory, as `<root>/.filetag/`. There are no exceptions.

2. **Same filesystem.** The Root directory and its `.filetag/` subdirectory reside on the same filesystem. On Unix, "same filesystem" means the same `st_dev` device number. On Windows, it means the same volume (identified by volume serial number via `MetadataExt::volume_serial_number`). A `.filetag/` directory that is itself a mount point, or that has been moved to a different filesystem, makes the Root invalid.

3. **Scope boundary.** All files governed by a Root reside on the same filesystem as the Root. A file on a different filesystem has no relationship to that Root, even if the Root directory appears to be an ancestor of the file's path. The scope boundary is enforced on all supported platforms (Unix and Windows).

4. **Self-contained artefacts.** filetag MUST NOT write any file outside `.filetag/`. Temporary files, cache entries, and logs go under `.filetag/cache/` or `.filetag/tmp/`. System temporary directories (`/tmp`, `$TMPDIR`, `~/.cache`) are forbidden.

### 1.3 Root Topology

Roots are independent of each other. Any arrangement is valid:

**Nested Roots.** One Root may be a proper subdirectory of another:

```
~/Photos/              ← Root A
~/Photos/Events/2024/  ← Root B  (nested inside A)
```

Files under `~/Photos/Events/2024/` are governed by Root B. Files elsewhere under `~/Photos/` are governed by Root A. A file in Root B is not visible to Root A unless the two Roots are explicitly linked (see §4).

**Sibling Roots.** Roots with no ancestor-descendant relationship:

```
~/Photos/     ← Root A
~/Documents/  ← Root B  (sibling of A)
```

There is no automatic relationship between sibling Roots.

---

## 2. Root Resolution

### 2.1 The Algorithm

For any file system item (file, directory, or archive entry), at most one Root is active. The active Root is determined as follows:

1. If the item is an archive entry (identified by the `::` separator in its path), use the archive file itself as the starting point.
2. Start at the item's directory (or the item itself if it is a directory).
3. Walk upward through parent directories, **strictly within the same filesystem** (Unix: same `st_dev`; Windows: same volume serial number).
4. The first directory encountered that is a Root — i.e. that contains a `.filetag/db.sqlite3` file — is the **active Root** for the item.
5. If the filesystem boundary is crossed before any Root is found, **no Root exists** for the item. Any CRUD operation on that item is rejected.

The result is deterministic: there is always exactly zero or one active Root per item.

### 2.2 The Canonical Resolution Functions

Root resolution is split into two public functions in `filetag-lib/src/db.rs`:

```rust
/// Walk parent directories to find the Root that governs `start`.
/// Returns the absolute, canonicalised Root directory path.
pub fn find_root(start: &Path) -> Result<PathBuf>

/// Open the database for the Root at `root` and run pending migrations.
/// Returns (connection, root).
pub fn open_root_db(root: &Path) -> Result<(Connection, PathBuf)>
```

A convenience wrapper combines them for the common case:

```rust
/// Equivalent to open_root_db(&find_root(start)?).
pub fn find_and_open(start: &Path) -> Result<(Connection, PathBuf)>
```

Use `find_root` when you need the Root path before committing to opening a connection. Use `find_and_open` when you only need a connection.

Every subsystem (CLI, web server, library consumers) that needs to operate on a file MUST reach one of these functions. There are no alternative code paths for Root resolution.

**Consequences:**

- The database connection and the Root path are always derived together from the item's location. It is not possible to open the wrong database for a given file.
- Nested Roots are handled correctly automatically: a file in a deep nested Root finds that Root's database, not a parent Root's.
- Filesystem boundary enforcement is centralised here and cannot be bypassed.

**The only permitted exception** is the web server's startup phase. At startup, filetag-web reads a set of entry-point paths from the command line or a registry, opens those databases by direct path, and holds them in memory as the session's Root set. This is configuration-time loading, not per-item resolution. Once the session is initialised, all per-item operations go through `find_and_open`.

### 2.3 Path-based Resolution in the Web Layer

The web server holds a pre-loaded set of `TagRoot` values in `AppState`. Every HTTP request that involves a file or directory carries an absolute filesystem path as its `dir` parameter (query string or JSON body). The web layer resolves the active Root from that path using the single function:

```rust
/// Return the deepest TagRoot whose root path contains `abs`.
///
/// This is the single source of truth for determining which database root owns
/// a given path. All API handlers that need to access .filetag/ data call
/// this function. No other root-resolution functions exist.
pub fn root_for_dir<'a>(state: &'a AppState, abs: &Path) -> Option<&'a TagRoot>
```

This function is not a replacement for `find_and_open`. It answers the question "which already-loaded Root covers this absolute path?" by selecting the `TagRoot` with the longest root path that is a prefix of `abs`. For any subsequent write operation on a specific file, the API still calls `open_for_file_op`, which internally calls `find_and_open` and may route to a nested child database.

There is no numeric root index in the HTTP API. The frontend always supplies absolute filesystem paths. Numeric root IDs are never exchanged between the browser and the server.

#### The `api_files` response

Every response from `GET /api/files` includes a `root_path` field — the absolute filesystem path of the deepest `TagRoot` that covers the listed directory. The frontend stores this as `state.currentBasePath` and sends it back in subsequent file-operation requests. This ensures that all file paths in those requests are relative to the correct (deepest) Root, so `open_for_file_op` always receives consistent input.

---

## 3. Paths Inside a Root

All file paths stored in the database are **relative to their Root**. Absolute paths are not stored. This ensures that a Root directory tree can be moved or mounted at a different path without invalidating the database.

### 3.1 Archive Entries

An archive entry is addressed by a **virtual path** of the form:

```
<archive-relative-path>::<entry-name>
```

For example: `Photos/album.zip::cover.jpg`.

The Root for an archive entry is always the Root of the archive file itself. Resolution starts at the archive's directory and follows the standard algorithm (§2.1, step 1).

### 3.2 Path Validation

Any path received from an external source (HTTP API, CLI argument) must be validated before use. Validation rejects:

- Absolute paths that escape the Root directory.
- Relative traversal sequences (`..`).
- Symlinks that resolve to a path outside the Root.

The function `preview_safe_path(root, rel)` in `filetag-web` enforces these rules and is the required entry point for all path handling in the web layer.

---

## 4. Linked Roots

Two Roots can be **linked** so that queries span both. Links are stored in the `linked_databases` table of each participating Root's database. A link records a path to the other Root's database, either relative (for a child Root) or absolute (for a sibling or parent Root).

Linking is asymmetric in storage but symmetric in behaviour: once established, queries from either side traverse the link.

Linking does not alter Root Resolution. Each file still belongs to exactly the Root found by the algorithm in §2.1. Links only affect read-access query scope, not write routing.

---

## 5. Naming Conventions

The table below maps domain concepts to their current code names.

| Domain concept                          | Code name                                  | Notes                                                                         |
| :-------------------------------------- | :----------------------------------------- | :---------------------------------------------------------------------------- |
| An initialised Root directory           | `TagRoot` (`filetag-lib::db`)              | Stores root path, db path, volume id, entry-point flag, display name          |
| Resolve Root for a file                 | `find_root` (lib)                          | Returns Root path; does not open a connection                                 |
| Open DB at a known Root                 | `open_root_db` (lib)                       | Opens connection; does not perform resolution                                 |
| Resolve Root and open DB                | `find_and_open` (lib)                      | Convenience wrapper around the two above                                      |
| Resolve Root from absolute path (web)   | `root_for_dir` (web `state.rs`)            | The one root-resolution function in the web layer; selects deepest Root       |
| Open DB for a file operation            | `open_for_file_op` (web `state.rs`)        | Calls `find_and_open` internally; routes to child DB if applicable            |
| Active Root path communicated to JS     | `root_path` in `ApiDirListing`             | Absolute path; frontend stores as `state.currentBasePath`                     |
| Root tile identity in virtual root view | `root_path` in `ApiDirEntry`               | Absolute path; replaces any numeric index                                     |
| Path relative to Root                   | `rel_path`, `rel`                          | Consistent across lib and web; keep as-is                                     |

---

## 6. Open Questions

No structural decisions remain open.
