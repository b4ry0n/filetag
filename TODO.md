# filetag – backlog

## Ideas / future features

### Preview improvements (web UI)

- **PDF** – render the first page via `<canvas>` + PDF.js; currently shown via iframe only

### Tagging workflow (web UI)

- **Bulk tag application from aggregated list** – when multiple files are selected, allow applying any tag from "Tags on selected files" to all selected files with one action. Keep per-tag coverage visible (for example: 3/7) so users can see whether a tag is already present on all selected files. On hover over a tag in this list, highlight the files in the current view that currently have that tag.

- **Tag sorting with groups first** – add a sorting mode for tag lists that keeps grouped tags (for example tags with a `/` prefix hierarchy) above ungrouped tags, while preserving stable alphabetical sorting within each section.

- **Nested tag sub-groups** – support expandable multi-level grouping in tag lists (for example `genre/rock/alt`) so users can navigate parent and child groups, collapse or expand each level independently, and apply group-level actions where relevant.

### AI workflow (web UI)

- **Multi-file AI analysis modes** – support running AI analysis for multiple selected files in one operation, with an explicit mode choice:
	- **Per-file mode**: analyse each file independently and apply file-specific tags.
	- **Common-traits mode**: analyse the selected files together and apply only tags that represent shared characteristics across the selection.

### Bugs

- **Tag value rename does not work** – renaming the value part of a key=value tag currently fails. Expected behaviour: users can rename a tag value and all matching assignments are updated consistently.
