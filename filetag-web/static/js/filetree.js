// ---------------------------------------------------------------------------
// Directory / file tree sidebar pane
// ---------------------------------------------------------------------------
//
// State (fields added in state.js):
//   state.sidebarTab      — 'tags' | 'files'
//   state.sidebarSplit    — boolean: show both panes side-by-side
//   state.ftreeExpanded   — { absPath: boolean }
//   state.ftreeCache      — { absPath: ApiDirEntry[] }
//   state.ftreeFilter     — string: text filter applied to file/dir names

// ---------------------------------------------------------------------------
// Tab + split management
// ---------------------------------------------------------------------------

window.setSidebarTab = function (tab) {
    state.sidebarTab = tab;
    try { localStorage.setItem('ft-sidebar-tab', tab); } catch (_) {}
    _applyTabState();
};

window.toggleSidebarSplit = function () {
    state.sidebarSplit = !state.sidebarSplit;
    try { localStorage.setItem('ft-sidebar-split', state.sidebarSplit ? '1' : ''); } catch (_) {}
    _applyTabState();
};

function _applyTabState() {
    const tagsPane = document.getElementById('tags-pane');
    const filePane = document.getElementById('filetree-pane');
    const sidebar  = document.getElementById('sidebar');
    if (!tagsPane || !filePane) return;

    if (state.sidebarSplit) {
        // Wrap both panes in a flex-row container if not already done.
        let row = sidebar.querySelector('.sidebar-panes-row');
        if (!row) {
            row = document.createElement('div');
            row.className = 'sidebar-panes-row';
            tagsPane.parentNode.insertBefore(row, tagsPane);
            row.appendChild(tagsPane);
            row.appendChild(filePane);
        }
        tagsPane.hidden = false;
        filePane.hidden = false;
        sidebar.classList.add('sidebar-split');
    } else {
        // Move panes back to sidebar root if they were wrapped.
        const row = sidebar.querySelector('.sidebar-panes-row');
        if (row) {
            sidebar.appendChild(tagsPane);
            sidebar.appendChild(filePane);
            row.remove();
        }
        tagsPane.hidden = state.sidebarTab !== 'tags';
        filePane.hidden = state.sidebarTab !== 'files';
        sidebar.classList.remove('sidebar-split');
    }

    // Sync tab button active states.
    document.querySelectorAll('.sidebar-tab-btn[data-tab]').forEach(btn => {
        btn.classList.toggle('active',
            !state.sidebarSplit && btn.dataset.tab === state.sidebarTab);
    });
    const splitBtn = document.getElementById('sidebar-split-btn');
    if (splitBtn) splitBtn.classList.toggle('active', state.sidebarSplit);

    if (!filePane.hidden) renderFiletree();
}

/** Call once after DOMContentLoaded to apply the persisted tab/split state. */
function initSidebarTabs() {
    _applyTabState();
}

// ---------------------------------------------------------------------------
// Text filter
// ---------------------------------------------------------------------------

window.ftreeSetFilter = function (value) {
    state.ftreeFilter = value;
    const clearBtn = document.getElementById('ft-filter-clear');
    if (clearBtn) clearBtn.hidden = !value;
    renderFiletree();
};

// ---------------------------------------------------------------------------
// Tree rendering
// ---------------------------------------------------------------------------

window.renderFiletree = function () {
    const el = document.getElementById('filetree-content');
    if (!el) return;
    const roots = state.roots || [];
    if (!roots.length) {
        el.innerHTML = '<div class="ft-empty">No databases loaded</div>';
        return;
    }
    el.innerHTML = roots.map(r => _ftRenderRoot(r)).join('');
};

function _ftRenderRoot(root) {
    const expanded = state.ftreeExpanded[root.path] !== false; // open by default
    if (expanded && !state.ftreeCache[root.path]) {
        // Kick off async load; re-render when done.
        _ftLoadDir(root.path).then(renderFiletree);
    }
    const chevCls = expanded ? '' : ' chevron-collapsed';
    const children = (expanded && state.ftreeCache[root.path])
        ? _ftRenderChildren(root.path, state.ftreeCache[root.path], 1)
        : (expanded
            ? `<div class="ft-loading" style="padding-left:20px">Loading\u2026</div>`
            : '');

    return `<div class="ft-root">
        <div class="ft-row ft-root-row" style="padding-left:4px"
            onclick="ftreeNavDir('${jesc(root.path)}')"
            title="${esc(root.path)}">
            <svg class="chevron-icon${chevCls} ft-chevron" viewBox="0 0 12 12" width="11" height="11"
                onclick="event.stopPropagation();ftreeToggleDir('${jesc(root.path)}')">
                <polyline points="2,3 6,8 10,3" fill="none" stroke="currentColor" stroke-width="1.4"
                    stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
            <svg class="ft-icon" viewBox="0 0 16 14" width="13" height="13" fill="none"
                stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round">
                <path d="M1 3.5A1.5 1.5 0 0 1 2.5 2H6l1.5 1.5H13.5A1.5 1.5 0 0 1 15 5v6.5A1.5 1.5 0 0 1 13.5 13h-11A1.5 1.5 0 0 1 1 11.5V3.5z"/>
            </svg>
            <span class="ft-label">${esc(root.name)}</span>
        </div>
        ${children}
    </div>`;
}

function _ftRenderChildren(parentAbs, entries, depth) {
    const f = (state.ftreeFilter || '').toLowerCase();
    const filtered = f
        ? entries.filter(e => e.name.toLowerCase().includes(f))
        : entries;
    if (!filtered.length && !entries.length) {
        return `<div class="ft-empty-dir" style="padding-left:${4 + depth * 14}px">Empty</div>`;
    }
    if (!filtered.length) {
        return `<div class="ft-empty-dir" style="padding-left:${4 + depth * 14}px">No matches</div>`;
    }
    return filtered.map(e => _ftRenderEntry(e, parentAbs, depth)).join('');
}

function _ftRenderEntry(e, parentAbs, depth) {
    const absPath = parentAbs.replace(/\/$/, '') + '/' + e.name;
    const indent  = 4 + depth * 14;

    if (e.is_dir) {
        const expanded = !!state.ftreeExpanded[absPath];
        if (expanded && !state.ftreeCache[absPath]) {
            _ftLoadDir(absPath).then(renderFiletree);
        }
        const chevCls  = expanded ? '' : ' chevron-collapsed';
        const countBadge = e.file_count != null
            ? ` <span class="count">${e.file_count}</span>` : '';
        const children = (expanded && state.ftreeCache[absPath])
            ? _ftRenderChildren(absPath, state.ftreeCache[absPath], depth + 1)
            : (expanded
                ? `<div class="ft-loading" style="padding-left:${indent + 14}px">Loading\u2026</div>`
                : '');
        return `<div class="ft-dir">
            <div class="ft-row" style="padding-left:${indent}px"
                onclick="ftreeNavDir('${jesc(absPath)}')"
                draggable="true"
                ondragstart="ftreeDragDir(event,'${jesc(absPath)}')"
                title="${esc(absPath)}">
                <svg class="chevron-icon${chevCls} ft-chevron" viewBox="0 0 12 12" width="11" height="11"
                    onclick="event.stopPropagation();ftreeToggleDir('${jesc(absPath)}')">
                    <polyline points="2,3 6,8 10,3" fill="none" stroke="currentColor" stroke-width="1.4"
                        stroke-linecap="round" stroke-linejoin="round"/>
                </svg>
                <span class="ft-label">${esc(e.name)}${countBadge}</span>
            </div>
            ${children}
        </div>`;
    } else {
        const tagBadge = e.tag_count
            ? ` <span class="ft-tag-badge">${e.tag_count}</span>` : '';
        const icon = _ftFileIcon(e.name);
        return `<div class="ft-row ft-file-row" style="padding-left:${indent}px"
            draggable="true"
            ondragstart="ftreeDragStart(event,'${jesc(absPath)}','${jesc(parentAbs)}')"
            onclick="ftreeSelectFile('${jesc(absPath)}','${jesc(parentAbs)}')"
            title="${esc(absPath)}">
            <span class="ft-file-icon" aria-hidden="true">${icon}</span>
            <span class="ft-label ft-file-label">${esc(e.name)}${tagBadge}</span>
        </div>`;
    }
}

// ---------------------------------------------------------------------------
// File-type icon helper
// ---------------------------------------------------------------------------

/** Return an inline SVG string for the given filename, based on its extension. */
function _ftFileIcon(name) {
    const ext = (name.split('.').pop() || '').toLowerCase();

    // Image
    if (['jpg','jpeg','png','gif','webp','bmp','tiff','tif','heic','heif','avif','svg','ico','raw','cr2','nef','arw','dng','orf','rw2'].includes(ext)) {
        return `<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" class="ft-type-icon ft-type-image"><rect x="1.5" y="2" width="13" height="12" rx="1.5"/><circle cx="5.5" cy="5.5" r="1.2"/><path d="M1.5 10.5l3.5-3.5 3 3 2-2 3.5 3.5"/></svg>`;
    }
    // Video
    if (['mp4','mkv','avi','mov','wmv','flv','webm','m4v','mpg','mpeg','ts','mts','m2ts','vob','3gp'].includes(ext)) {
        return `<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" class="ft-type-icon ft-type-video"><rect x="1" y="2.5" width="10" height="11" rx="1.5"/><path d="M11 6l4-2v8l-4-2V6z"/></svg>`;
    }
    // Audio
    if (['mp3','flac','ogg','wav','aac','m4a','opus','wma','aiff','alac','ape','mka'].includes(ext)) {
        return `<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" class="ft-type-icon ft-type-audio"><circle cx="4.5" cy="12" r="2"/><circle cx="12" cy="10.5" r="2"/><path d="M6.5 12V4.5l7.5-2v7.5"/><line x1="6.5" y1="4.5" x2="14" y2="2.5"/></svg>`;
    }
    // PDF
    if (ext === 'pdf') {
        return `<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" class="ft-type-icon ft-type-pdf"><path d="M3 1h7l3 3v11H3V1z"/><path d="M10 1v3h3" stroke-linejoin="round"/><path d="M5.5 8.5c0-.8.6-1.5 1.5-1.5h.5c.8 0 1.5.7 1.5 1.5S8.3 10 7.5 10H7v2" stroke-linecap="round"/><line x1="5.5" y1="12" x2="8.5" y2="12"/></svg>`;
    }
    // Archive / zip / rar
    if (['zip','rar','7z','tar','gz','bz2','xz','zst','cbz','cbr','cb7'].includes(ext)) {
        return `<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" class="ft-type-icon ft-type-archive"><path d="M3 1h7l3 3v11H3V1z"/><path d="M10 1v3h3"/><line x1="7" y1="4" x2="9" y2="4"/><line x1="7" y1="6" x2="9" y2="6"/><line x1="7" y1="8" x2="9" y2="8"/><rect x="6" y="9.5" width="4" height="3" rx=".5"/></svg>`;
    }
    // Ebook / document formats
    if (['epub','mobi','azw','azw3','djvu'].includes(ext)) {
        return `<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" class="ft-type-icon ft-type-ebook"><path d="M3 1.5A1.5 1.5 0 0 1 4.5 0H12l3 3v12a1.5 1.5 0 0 1-1.5 1.5H4.5A1.5 1.5 0 0 1 3 15V1.5z"/><path d="M12 0v3h3"/><line x1="6" y1="6" x2="12" y2="6"/><line x1="6" y1="8.5" x2="12" y2="8.5"/><line x1="6" y1="11" x2="10" y2="11"/></svg>`;
    }
    // Word / text documents
    if (['doc','docx','odt','rtf','txt','md','markdown'].includes(ext)) {
        return `<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" class="ft-type-icon ft-type-doc"><path d="M3 1h7l3 3v11H3V1z"/><path d="M10 1v3h3"/><line x1="5" y1="7" x2="11" y2="7"/><line x1="5" y1="9.5" x2="11" y2="9.5"/><line x1="5" y1="12" x2="9" y2="12"/></svg>`;
    }
    // Spreadsheet
    if (['xls','xlsx','ods','csv'].includes(ext)) {
        return `<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" class="ft-type-icon ft-type-sheet"><path d="M3 1h7l3 3v11H3V1z"/><path d="M10 1v3h3"/><line x1="5" y1="7" x2="11" y2="7"/><line x1="5" y1="9.5" x2="11" y2="9.5"/><line x1="8" y1="7" x2="8" y2="12"/></svg>`;
    }
    // Code / config
    if (['js','ts','jsx','tsx','py','rs','go','java','c','cpp','h','hpp','cs','php','rb','sh','bash','zsh','fish','toml','json','yaml','yml','xml','html','htm','css','scss','less','sql','lua','swift','kt','dart'].includes(ext)) {
        return `<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" class="ft-type-icon ft-type-code"><path d="M3 1h7l3 3v11H3V1z"/><path d="M10 1v3h3"/><polyline points="6,7 4.5,9 6,11"/><polyline points="10,7 11.5,9 10,11"/></svg>`;
    }
    // Font
    if (['ttf','otf','woff','woff2','eot'].includes(ext)) {
        return `<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" class="ft-type-icon ft-type-font"><path d="M3 1h7l3 3v11H3V1z"/><path d="M10 1v3h3"/><path d="M6 12V6h4M6 9h3.5" stroke-linecap="round"/></svg>`;
    }

    // Generic file
    return `<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" class="ft-type-icon ft-type-generic"><path d="M3 1h7l3 3v11H3V1z"/><path d="M10 1v3h3"/></svg>`;
}



async function _ftLoadDir(absPath) {
    try {
        const url = '/api/files?dir=' + encodeURIComponent(absPath)
            + (state.showHidden ? '&show_hidden=true' : '');
        const r = await fetch(url);
        if (!r.ok) { state.ftreeCache[absPath] = []; return; }
        const data = await r.json();
        state.ftreeCache[absPath] = data.entries || [];
    } catch (_) {
        state.ftreeCache[absPath] = [];
    }
}

// ---------------------------------------------------------------------------
// Tree interaction
// ---------------------------------------------------------------------------

window.ftreeToggleDir = async function (absPath) {
    if (state.ftreeExpanded[absPath]) {
        state.ftreeExpanded[absPath] = false;
        renderFiletree();
    } else {
        state.ftreeExpanded[absPath] = true;
        if (!state.ftreeCache[absPath]) {
            await _ftLoadDir(absPath);
        }
        renderFiletree();
    }
};

/** Double-click: navigate the main panel to this directory. */
window.ftreeNavDir = async function (absPath) {
    const root = _ftFindRoot(absPath);
    if (root) state.currentBasePath = root.path;
    await navigateTo(absPath);
};

/** Click: navigate to parent dir and pre-select the file. */
window.ftreeSelectFile = async function (absPath, parentAbs) {
    const root = _ftFindRoot(absPath);
    if (root) state.currentBasePath = root.path;
    await navigateTo(parentAbs);
    // Try to select the file after navigation has settled.
    const relPath = _ftAbsToRootRel(absPath);
    if (relPath) {
        const entry = state.entries.find(e => {
            const ep = state.currentPath ? state.currentPath + '/' + e.name : e.name;
            return ep === relPath || e.name === absPath.split('/').pop();
        });
        if (entry) {
            const cardPath = state.currentPath
                ? state.currentPath + '/' + entry.name
                : entry.name;
            await selectFile(cardPath, null);
        }
    }
};

// ---------------------------------------------------------------------------
// Drag-and-drop: file tree → sidebar tag
// ---------------------------------------------------------------------------

/** Drag a single file from the tree onto a sidebar tag to apply it. */
window.ftreeDragStart = function (event, absPath, parentAbs) {
    event.stopPropagation();
    event.dataTransfer.effectAllowed = 'copy';
    // Tags expect root-relative paths; compute that here.
    const relPath = _ftAbsToRootRel(absPath);
    event.dataTransfer.setData('text/filetag-paths', JSON.stringify([relPath]));
    // Pass the parent directory so tagDrop uses the correct root.
    event.dataTransfer.setData('text/filetag-dir', parentAbs);
};

/** Drag a directory — sets a custom type (no tagging action yet). */
window.ftreeDragDir = function (event, absPath) {
    event.stopPropagation();
    event.dataTransfer.effectAllowed = 'none';
    event.dataTransfer.setData('text/filetag-dir-path', absPath);
};

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

function _ftFindRoot(absPath) {
    return (state.roots || []).find(
        r => absPath === r.path || absPath.startsWith(r.path + '/'));
}

function _ftAbsToRootRel(absPath) {
    const root = _ftFindRoot(absPath);
    if (!root) return absPath;
    if (absPath === root.path) return '';
    return absPath.slice(root.path.length + 1);
}

// ---------------------------------------------------------------------------
// Invalidate cached entries when files are tagged / reloaded
// ---------------------------------------------------------------------------

/** Clear the loaded-entries cache for a given directory so the next
 *  expansion re-fetches (picks up tag_count changes etc.). */
function ftreeInvalidateDir(absDir) {
    delete state.ftreeCache[absDir];
}

/** Clear the entire tree cache (e.g. after a bulk operation). */
function ftreeClearCache() {
    state.ftreeCache = {};
}
