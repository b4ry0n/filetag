# filetag – backlog

## Ideas / future features

### Range tags
Tags that hold an ordinal or continuous value on a user-defined scale.
Examples:
- Rating 1–10 for a drawing, with user-defined named ranges (e.g. 1–3 = poor, 4–6 = average, 7–10 = good)
- Light intensity as a slider
- Colour hue value

Requirements:
- Fuzzy/range queries in the query language: `rating>=7`, `rating:4-6`, `hue:warm`
- Named ranges definable and storable by the user
- UI: slider or numeric input field alongside the tag chip
- Filterable via `filetag find rating>=7`

### Tagging inside archives
Archives (zip, 7z, rar, cbz, cbr, tar, …) can be tagged in two modes:

1. **Archive as a whole** – the archive file itself receives tags, as today
2. **Files inside the archive** – individual entries receive their own tags in the database (virtual paths, e.g. `archive.cbz::cover.jpg`)

Considerations:
- Virtual path as key in the `files` table (`archive.cbz::entry.jpg` or similar)
- Preview of entries in the web UI (images, text)
- `file_id` for entries inside an archive: content hash (no inode available)
- CLI: `filetag tag archive.zip::subfile.txt -t tagname`

### More preview formats (web UI)
Most images already work via `<img>`. Missing:

- **Video** – more containers/codecs (mkv, avi, mov, wmv) via browser-native `<video>`; show a message when the codec is unsupported
- **Audio** – waveform display or a plain `<audio>` element for mp3/flac/ogg/opus/aac
- **ZIP / archive** – display a table of contents (filenames, sizes); optionally link to in-archive tagging (see above)
- **PDF** – render the first page via `<canvas>` + PDF.js (optional dependency)
- **Text / code** – display plain-text files with syntax highlighting (e.g. via highlight.js)
- **SVG** – inline render (partially works via `<img>`, but interactive SVG can do more)
- Fallback: file size + extension + "no preview available"
