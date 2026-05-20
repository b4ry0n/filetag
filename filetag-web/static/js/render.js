// ---------------------------------------------------------------------------
// Render: Breadcrumb
// ---------------------------------------------------------------------------

function renderBreadcrumb() {
    const el = document.getElementById('breadcrumb');

    if (state.mode === 'search') {
        el.innerHTML = `<span class="breadcrumb-item current">Search: ${esc(state.searchQuery)}</span>`;
        return;
    }

    const rootIsCurrent = state.currentPath === '' && state.mode !== 'zip' && state.currentBasePath != null;

    // "/" always goes to the roots overview, whether there is one root or many.
    let html = `<button class="breadcrumb-item${state.currentBasePath == null ? ' current' : ''}" onclick="goVirtualRoot()">/</button>`;
    if (state.currentBasePath != null) {
        // Ancestor roots: any root whose path is a strict prefix of the current base path.
        const ancestors = state.roots
            .filter(r => state.currentBasePath.startsWith(r.path + '/'))
            .sort((a, b) => a.path.length - b.path.length);

        for (const anc of ancestors) {
            if (ancestors.indexOf(anc) > 0) html += `<span class="breadcrumb-sep">/</span>`;
            html += `<button class="breadcrumb-item" onclick="enterRoot('${jesc(anc.path)}')">${esc(anc.name)}</button>`;
        }

        if (ancestors.length > 0) html += `<span class="breadcrumb-sep">/</span>`;

        const root = state.roots.find(r => r.path === state.currentBasePath);
        const rootName = root ? root.name : state.currentBasePath.split('/').pop();
        if (rootIsCurrent) {
            html += `<span class="breadcrumb-item current" title="Click to rename" ondblclick="startRootRename('${jesc(state.currentBasePath)}', this)">${esc(rootName)}</span>`;
        } else {
            html += `<button class="breadcrumb-item" onclick="navigateTo('')">${esc(rootName)}</button>`;
        }
    }

    if (state.currentPath) {
        const parts = state.currentPath.split('/');
        let accumulated = '';
        for (let i = 0; i < parts.length; i++) {
            accumulated += (i === 0 ? '' : '/') + parts[i];
            const isCurrent = i === parts.length - 1 && state.mode !== 'zip';
            const path = accumulated;
            // The root-name segment is always shown before path parts, so always
            // emit a separator between the root name and the first path component.
            html += `<span class="breadcrumb-sep">/</span>`;
            html += `<button class="breadcrumb-item${isCurrent ? ' current' : ''}" onclick="navigateTo('${jesc(path)}')">${esc(parts[i])}</button>`;
        }
    }

    if (state.mode === 'zip') {
        const zipName = state.zipPath.split('/').pop();
        if (!state.zipSubdir) {
            html += `<span class="breadcrumb-sep">/</span><span class="breadcrumb-item current">${esc(zipName)}</span>`;
        } else {
            html += `<span class="breadcrumb-sep">/</span><button class="breadcrumb-item" onclick="enterZipSubdir('')">${esc(zipName)}</button>`;
            const parts = state.zipSubdir.replace(/\/$/, '').split('/');
            let accum = '';
            for (let i = 0; i < parts.length; i++) {
                accum += (i === 0 ? '' : '/') + parts[i];
                const isCurrent = i === parts.length - 1;
                const target = accum + '/';
                html += `<span class="breadcrumb-sep">/</span>`;
                if (isCurrent) {
                    html += `<span class="breadcrumb-item current">${esc(parts[i])}</span>`;
                } else {
                    html += `<button class="breadcrumb-item" onclick="enterZipSubdir('${jesc(target)}')">${esc(parts[i])}</button>`;
                }
            }
        }
    }

    el.innerHTML = html;
    _compactBreadcrumb(el);
}

/**
 * Collapse the leftmost breadcrumb segments into a single "…" item when
 * the breadcrumb overflows its container (Finder-style: oldest ancestors
 * are hidden first so the current directory is always fully visible).
 */
function _compactBreadcrumb(el) {
    if (el.scrollWidth <= el.clientWidth) return;

    // Build a list of segments from the flat children list.
    // children[0] is always the root "/" button (no preceding separator).
    // Subsequent children are [sep][item] pairs.
    const children = Array.from(el.children);
    const segs = [];
    let i = 1;
    while (i < children.length) {
        const isSep = children[i] && children[i].classList.contains('breadcrumb-sep');
        const sep  = isSep ? children[i++] : null;
        const item = children[i++];
        if (item) segs.push({ sep, item });
    }

    // Need at least 2 segments to collapse anything (last segment is always kept).
    if (segs.length <= 1) return;

    // Hide segments from the left until the breadcrumb fits.
    const hiddenLabels = [];
    for (let h = 0; h < segs.length - 1 && el.scrollWidth > el.clientWidth; h++) {
        const seg = segs[h];
        if (seg.sep) seg.sep.style.display = 'none';
        seg.item.style.display = 'none';
        hiddenLabels.push(seg.item.textContent.trim());
        void el.offsetWidth; // flush layout so scrollWidth is re-measured
    }

    if (hiddenLabels.length === 0) return;

    // Insert a "…" button just before the separator of the first visible
    // segment.  That separator then acts as the "/" between "…" and the
    // next visible name:  / … / dir / current
    const firstVisible = segs[hiddenLabels.length];
    const insertBefore = (firstVisible && firstVisible.sep) || null;

    const ellEl = document.createElement('button');
    ellEl.className = 'breadcrumb-item';
    ellEl.textContent = '\u2026'; // …
    ellEl.title = hiddenLabels.join(' / ');
    if (insertBefore) {
        el.insertBefore(ellEl, insertBefore);
    } else {
        el.appendChild(ellEl);
    }
}

// Inline rename of a root database name.
function startRootRename(rootPath, el) {
    const root = state.roots.find(r => r.path === rootPath);
    if (!root) return;
    const currentName = root.name;
    const input = document.createElement('input');
    input.type = 'text';
    input.value = currentName;
    input.className = 'breadcrumb-rename-input';
    input.style.cssText = 'background:transparent;border:1px solid var(--accent);border-radius:3px;color:inherit;font:inherit;padding:0 4px;width:10em;';
    el.replaceWith(input);
    input.focus();
    input.select();

    async function commit() {
        const newName = input.value.trim();
        if (newName && newName !== currentName) {
            await apiPost('/api/db/rename', { dir: rootPath, name: newName });
            // Update local state
            const idx = state.roots.findIndex(r => r.path === rootPath);
            if (idx !== -1) state.roots[idx] = { ...root, name: newName };
        }
        renderBreadcrumb();
    }
    input.addEventListener('blur', commit);
    input.addEventListener('keydown', e => {
        if (e.key === 'Enter') { commit(); }
        if (e.key === 'Escape') { renderBreadcrumb(); }
    });
}

// ---------------------------------------------------------------------------
// Render: File grid
// ---------------------------------------------------------------------------

function renderGrid(items) {
    let html = '';
    // Render directories first, then files (DOM order = preview-queue order).
    const files = items.filter(e => !e.is_dir);
    const dirs  = items.filter(e =>  e.is_dir);
    for (const entry of [...dirs, ...files]) {
        const isDir = entry.is_dir;
        const name = isDir ? entry.name : (entry.name || entry.path.split('/').pop());
        const path = isDir ? null : fullPath(entry);
        const selected = state.selectedFile && state.selectedFile.path === path ? ' selected' : '';
        const type_ = isDir ? 'folder' : fileType(name);
        // Resolve the numeric root ID for this entry (used in API calls and data attributes).
        // In search mode each entry carries its own root_id; in browse mode use the active root.
        const entryRootId = (!isDir && state.mode === 'search' && entry.root_id != null)
            ? entry.root_id
            : state.currentRootId;
        // entryDir is kept for thumbnail/preview URLs (absolute path from server, not constructed).
        const entryDir = !isDir && state.mode === 'search' && entry.root_path
            ? entry.root_path
            : currentAbsDir();
        const entryDirParam = '?dir=' + encodeURIComponent(entryDir);

        let preview = '';
        if (isDir) {
            // Root cards (entry.root_path != null) keep the plain icon.
            // All other dirs request a sprite thumbnail via the regular thumb queue.
            const dirPath = entry.root_path == null ? fullPath(entry) : null;
            if (dirPath) {
                const dts = `/api/dir-thumbs?${new URLSearchParams({path: dirPath}).toString()}${dirParam('&')}`;
                        preview = `<div class="card-icon" data-thumb-src="${esc(dts)}" data-dir-path="${esc(dirPath)}" data-name="${esc(name)}">${ICONS.folder}</div>` +
                            `<div class="card-type-badge">${ICONS.folder}</div>`;
            } else {
                preview = `<div class="card-icon">${ICONS.folder}</div>`;
            }
        } else if (type_ === 'image' || type_ === 'raw') {
            preview = `<div class="card-icon" data-thumb-src="/thumb/${encodePath(fullPath(entry))}${entryDirParam}" data-name="${esc(name)}" data-thumb-hover="1">${fileIcon(name)}</div>` +
                `<div class="card-type-badge">${fileIcon(name)}</div>`;
        } else if (type_ === 'video') {
            const vpath = fullPath(entry);
            preview = `<div class="card-icon" data-thumb-src="/thumb/${encodePath(vpath)}${entryDirParam}" data-name="${esc(name)}" data-video-path="${esc(vpath)}">${ICONS.video}</div>` +
                `<div class="card-filmstrip-badge">${ICONS.video}</div>`;
        } else if (type_ === 'zip') {
            preview = `<div class="card-icon" data-thumb-src="/thumb/${encodePath(fullPath(entry))}${entryDirParam}" data-name="${esc(name)}" data-thumb-hover="1">${ICONS.zip || ''}</div>` +
                `<div class="card-filmstrip-badge">${ICONS.zip || ''}</div>`;
        } else if (type_ === 'pdf') {
            preview = `<div class="card-icon" data-thumb-src="/thumb/${encodePath(fullPath(entry))}${entryDirParam}" data-name="${esc(name)}" data-thumb-hover="1">${ICONS.pdf}</div>` +
                `<div class="card-filmstrip-badge">${ICONS.pdf}</div>`;
        } else {
            preview = `<div class="card-icon">${fileIcon(name)}</div>`;
        }

        const dirCountHint = entry.file_count != null ? `${entry.file_count} item${entry.file_count === 1 ? '' : 's'}` : '';
        const dirTagHint = isDir && entry.tag_count ? `${dirCountHint ? ' · ' : ''}${entry.tag_count} tag${entry.tag_count === 1 ? '' : 's'}` : '';
        const meta = isDir ? (dirCountHint + dirTagHint) || '\u2014' : formatSize(entry.size);

        if (isDir) {
            // Virtual root entry (shown at the top level when multiple roots exist)
            if (entry.root_path != null) {
                html += `<div class="card folder root-card" data-root-path="${esc(entry.root_path)}"
                    draggable="true"
                    ondragstart="_rootDragStart(event,'${jesc(entry.root_path)}')"
                    ondragover="_rootDragOver(event)"
                    ondragleave="_rootDragLeave(event)"
                    ondrop="_rootDrop(event,'${jesc(entry.root_path)}')"
                    onclick="handleRootClick('${jesc(entry.root_path)}')">
                    <div class="card-preview"><div class="card-icon">${ICONS.root}</div></div>
                    <div class="card-body"><div class="card-name">${esc(name)}</div><div class="card-meta">root</div></div>
                </div>`;
            } else {
                const dirPath = fullPath(entry);
                const dirSelected = state.selectedDir && state.selectedDir.path === dirPath ? ' selected' : '';
                const dirSymlinkBadge = entry.is_symlink ? '<span class="card-symlink" title="Symbolic link">&#10138;</span>' : '';
                // Resolve absolute path for the filetree drop handler.
                const absDirPath = state.currentBasePath
                    ? (dirPath ? state.currentBasePath + '/' + dirPath : state.currentBasePath)
                    : dirPath;
                html += `<div class="card folder${dirSelected}" data-path="${esc(dirPath)}" draggable="true" ondragstart="cardDragStart(event,'${jesc(dirPath)}')" ondragover="ftreeDirDragOver(event,'${jesc(absDirPath)}')" ondragleave="ftreeDirDragLeave(event)" ondrop="ftreeDirDrop(event,'${jesc(absDirPath)}')" onclick="handleDirClick('${jesc(dirPath)}','${jesc(name)}',${entry.file_count ?? null})" oncontextmenu="showFileMenu(event,'${jesc(dirPath)}',true,${state.currentRootId})">
                    ${dirSymlinkBadge}<div class="card-preview">${preview}</div>
                    <div class="card-body"><div class="card-name">${esc(name)}</div><div class="card-meta">${meta}</div></div>
                </div>`;
            }
        } else {
            const multiSel = state.selectedPaths.has(path) ? ' selected' : '';
            const checkmark = state.selectedPaths.size > 1 && state.selectedPaths.has(path) ? '<span class="card-check">&#10003;</span>' : '';
            const isArchiveEntry = path.includes('::');
            const gotoDirBtn = state.mode === 'search'
                ? isArchiveEntry
                    ? `<button class="card-goto" onclick="event.stopPropagation();openZipDir('${jesc(path.split('::')[0])}',${entryRootId})" title="Go to archive">${ICONS.gotoDir}</button>`
                    : `<button class="card-goto" onclick="event.stopPropagation();navigateToParent('${jesc(path)}')" title="Go to directory">${ICONS.gotoDir}</button>`
                : '';
            const uncoveredBadge = entry.covered === false ? '<span class="card-uncovered" title="No filetag database on this filesystem">&#128274;</span>' : '';
            const uncoveredCls = entry.covered === false ? ' uncovered' : '';
            const symlinkBadge = entry.is_symlink ? '<span class="card-symlink" title="Symbolic link">&#10138;</span>' : '';
            if (type_ === 'zip') {
                html += `<div class="card${multiSel}${uncoveredCls}" data-path="${esc(path)}" data-root-id="${entryRootId}" draggable="true" ondragstart="cardDragStart(event,'${jesc(path)}')" onclick="handleZipClick('${jesc(path)}', event)" oncontextmenu="showFileMenu(event,'${jesc(path)}',false,${entryRootId})">
                    ${checkmark}${gotoDirBtn}${uncoveredBadge}${symlinkBadge}<div class="card-preview">${preview}</div>
                    <div class="card-body"><div class="card-name">${esc(name)}</div><div class="card-meta">${meta}</div></div>
                </div>`;
            } else {
                const dblFn = `cvOpenFile('${jesc(path)}','${fileType(name)}')`;
                html += `<div class="card${multiSel}${uncoveredCls}" data-path="${esc(path)}" data-root-id="${entryRootId}" draggable="true" ondragstart="cardDragStart(event,'${jesc(path)}')" onclick="selectFile('${jesc(path)}', event)" ondblclick="${dblFn}" oncontextmenu="showFileMenu(event,'${jesc(path)}',false,${entryRootId})">
                    ${checkmark}${gotoDirBtn}${uncoveredBadge}${symlinkBadge}<div class="card-preview">${preview}</div>
                    <div class="card-body"><div class="card-name">${esc(name)}</div><div class="card-meta">${meta}</div></div>
                </div>`;
            }
        }
    }
    return html;
}

// ---------------------------------------------------------------------------
// Render: File list
// ---------------------------------------------------------------------------

function renderList(items) {
    return `<div class="list-header">
        <span></span><span>Name</span><span>Size</span><span>Modified</span><span>Tags</span>
    </div>` + _renderListRows(items);
}

// Render list rows without the header.  Used by renderList and by the
// chunked-rendering path in renderContent so that the header is only
// emitted once (with the first chunk).
function _renderListRows(items) {
    let html = '';

    for (const entry of items) {
        const isDir = entry.is_dir;
        const name = isDir ? entry.name : (entry.name || entry.path.split('/').pop());
        const path = isDir ? null : fullPath(entry);
        const selected = state.selectedFile && state.selectedFile.path === path ? ' selected' : '';
        const icon = isDir ? ICONS.folder : fileIcon(name);
        const size = isDir
            ? (entry.file_count != null ? `${entry.file_count} item${entry.file_count === 1 ? '' : 's'}` : '')
            : formatSize(entry.size);
        const date = isDir ? '' : formatDate(entry.mtime);
        const tags = isDir
            ? (entry.tag_count ? `${entry.tag_count} tag${entry.tag_count === 1 ? '' : 's'}` : '')
            : (entry.tag_count != null ? `${entry.tag_count} tags` : '');

        if (isDir) {
            if (entry.root_path != null) {
                html += `<div class="list-row folder" data-root-path="${esc(entry.root_path)}" onclick="handleRootClick('${jesc(entry.root_path)}')">
                    <span class="icon">${icon}</span>
                    <span class="name">${esc(name)}</span>
                    <span class="size"></span>
                    <span class="date"></span>
                    <span class="tags-count">root</span>
                </div>`;
            } else {
                const dirPath = fullPath(entry);
                const dirSelected = state.selectedDir && state.selectedDir.path === dirPath ? ' selected' : '';
                const dirSymlinkSuffix = entry.is_symlink ? ' <span class="list-symlink" title="Symbolic link">&#10138;</span>' : '';
                const absDirPath2 = state.currentBasePath
                    ? (dirPath ? state.currentBasePath + '/' + dirPath : state.currentBasePath)
                    : dirPath;
                html += `<div class="list-row folder${dirSelected}" data-path="${esc(dirPath)}" draggable="true" ondragstart="cardDragStart(event,'${jesc(dirPath)}')" ondragover="ftreeDirDragOver(event,'${jesc(absDirPath2)}')" ondragleave="ftreeDirDragLeave(event)" ondrop="ftreeDirDrop(event,'${jesc(absDirPath2)}')" onclick="handleDirClick('${jesc(dirPath)}','${jesc(name)}',${entry.file_count ?? null})">
                    <span class="icon">${icon}</span>
                    <span class="name">${esc(name)}${dirSymlinkSuffix}</span>
                    <span class="size">${size}</span>
                    <span class="date">${date}</span>
                    <span class="tags-count">${tags}</span>
                </div>`;
            }
        } else {
            const multiSel = state.selectedPaths.has(path) ? ' selected' : '';
            const isArchiveEntry = path.includes('::');
            // entryRootId for API calls; entryDir kept for zip/legacy thumbnail URLs.
            const entryRootId = state.mode === 'search' && entry.root_id != null
                ? entry.root_id
                : state.currentRootId;
            const entryDir = state.mode === 'search' && entry.root_path
                ? entry.root_path
                : currentAbsDir();
            const gotoDirBtn = state.mode === 'search'
                ? isArchiveEntry
                    ? `<button class="goto-dir-btn" onclick="event.stopPropagation();openZipDir('${jesc(path.split('::')[0])}',${entryRootId})" title="Go to archive">${ICONS.gotoDir}</button>`
                    : `<button class="goto-dir-btn" onclick="event.stopPropagation();navigateToParent('${jesc(path)}')" title="Go to directory">${ICONS.gotoDir}</button>`
                : '';
            const uncoveredBadge = entry.covered === false ? ' &#128274;' : '';
            const uncoveredCls = entry.covered === false ? ' uncovered' : '';
            const symlinkSuffix = entry.is_symlink ? ' <span class="list-symlink" title="Symbolic link">&#10138;</span>' : '';
            if (fileType(name) === 'zip') {
                html += `<div class="list-row${multiSel}${uncoveredCls}" data-path="${esc(path)}" data-root-id="${entryRootId}" draggable="true" ondragstart="cardDragStart(event,'${jesc(path)}')" onclick="handleZipClick('${jesc(path)}', event)">
                    <span class="icon">${icon}</span>
                    <span class="name">${esc(name)}${uncoveredBadge}${symlinkSuffix}</span>
                    <span class="size">${size}</span>
                    <span class="date">${date}</span>
                    <span class="tags-count">${tags}${gotoDirBtn}</span>
                </div>`;
            } else {
                const dblFnL = `cvOpenFile('${jesc(path)}','${fileType(name)}')`;
                html += `<div class="list-row${multiSel}${uncoveredCls}" data-path="${esc(path)}" data-root-id="${entryRootId}" draggable="true" ondragstart="cardDragStart(event,'${jesc(path)}')" onclick="selectFile('${jesc(path)}', event)" ondblclick="${dblFnL}">
                    <span class="icon">${icon}</span>
                    <span class="name">${esc(name)}${uncoveredBadge}${symlinkSuffix}</span>
                    <span class="size">${size}</span>
                    <span class="date">${date}</span>
                    <span class="tags-count">${tags}${gotoDirBtn}</span>
                </div>`;
            }
        }
    }
    return html;
}

// ---------------------------------------------------------------------------
// File card / list-row drag: drag one or more files to a sidebar tag/subject.
// ---------------------------------------------------------------------------

function cardDragStart(event, path) {
    // When the dragged file is part of a multi-selection, carry all selected
    // paths; otherwise just this one file.
    const paths = state.selectedPaths.has(path)
        ? [...state.selectedPaths]
        : [path];
    event.dataTransfer.setData('text/filetag-paths', JSON.stringify(paths));
    // Pass the numeric root ID so tag-drop handlers use the correct database
    // without needing to know or construct any system paths.
    const rootId = parseInt(event.currentTarget?.dataset?.rootId);
    const rid = isNaN(rootId) ? state.currentRootId : rootId;
    event.dataTransfer.setData('text/filetag-root-id', rid != null ? String(rid) : '');
    event.dataTransfer.effectAllowed = 'move';
    // Prevent the root-reorder drag from interfering.
    event.stopPropagation();
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Root drag-and-drop reordering
// ---------------------------------------------------------------------------

let _rootDragPath = null;

function _rootDragStart(ev, rootPath) {
    _rootDragPath = rootPath;          // fix: was missing — reorder never worked before
    ev.dataTransfer.effectAllowed = 'move';
}

function _rootDragOver(ev) {
    ev.preventDefault();
    ev.dataTransfer.dropEffect = 'move';
    ev.currentTarget.classList.add('drag-over');
}

function _rootDragLeave(ev) {
    ev.currentTarget.classList.remove('drag-over');
}

async function _rootDrop(ev, targetPath) {
    ev.preventDefault();
    ev.currentTarget.classList.remove('drag-over');
    if (_rootDragPath === null || _rootDragPath === targetPath) return;

    // Build new order: swap dragged root to just before target
    const current = state.roots.map(r => r.path);
    const from = current.indexOf(_rootDragPath);
    const to = current.indexOf(targetPath);
    if (from < 0 || to < 0) return;
    current.splice(from, 1);
    current.splice(to, 0, _rootDragPath);
    _rootDragPath = null;

    await apiPost('/api/roots/reorder', { order: current });
    // Reload roots and re-render virtual root page
    state.roots = await api('/api/roots');
    await loadFiles('');
    render();
}

// ---------------------------------------------------------------------------
// Surgical DOM updates (avoid full re-render to preserve thumbnails)
// ---------------------------------------------------------------------------

// Toggle .selected class and checkmark on cards to match state.selectedPaths.
function _updateCardSelection() {
    const content = document.getElementById('content');
    const multiSelect = state.selectedPaths.size > 1;
    content.querySelectorAll('.card[data-path]').forEach(card => {
        const p = card.dataset.path;
        const want = state.selectedPaths.has(p);
        card.classList.toggle('selected', want);
        // Checkmark: handled via CSS ::after on .card-checked — no span creation.
        card.classList.toggle('card-checked', multiSelect && want);
    });
    // Also handle folders:
    content.querySelectorAll('.card.folder[data-path]').forEach(card => {
        const p = card.dataset.path;
        card.classList.toggle('selected', state.selectedDir && state.selectedDir.path === p);
    });
    // List rows
    content.querySelectorAll('.list-row[data-path]').forEach(row => {
        const p = row.dataset.path;
        row.classList.toggle('selected', state.selectedPaths.has(p) ||
            (state.selectedFile && state.selectedFile.path === p));
    });
}

// Update tag-count badges on cards after tag add/remove (for list view).
function _updateCardTagBadges() {
    if (!state.selectedFile) return;
    const path = state.selectedFile.path;
    const count = state.selectedFile.tags.length;
    // Update the entry in state.entries so we stay in sync.
    const entry = (state.mode === 'search' ? state.searchResults : state.entries)
        .find(e => (e.path || fullPath(e)) === path);
    if (entry) entry.tag_count = count;
    // Update list-view cell if visible.
    const row = document.querySelector(`.list-row[data-path="${CSS.escape(path)}"]`);
    if (row) {
        const tagCell = row.querySelector('.tags-count');
        if (tagCell) tagCell.textContent = `${count} tag${count === 1 ? '' : 's'}`;
    }
}

// ---------------------------------------------------------------------------
// Render: Content area
// ---------------------------------------------------------------------------

// Returns the grid/list class name with the current label/badge preference
// applied, so that navigation never strips those classes.
function _gridClass(base) {
    const v = localStorage.getItem('ft-card-labels');
    if (v === 'minimal') return base + ' hide-labels hide-badges';
    if (v === '0' || v === 'hide') return base + ' hide-labels';
    return base;
}

// Generation counter: incremented before every render so that background
// chunks that belong to a superseded navigation detect it and stop.
let _renderGen = 0;

// Items rendered synchronously (first paint) and per subsequent chunk.
const _RENDER_INITIAL = 15;
const _RENDER_CHUNK   = 100;

// Scroll a tile into view once it appears in the DOM.  Set
// window._pendingScrollToTile = absolutePath before navigating; renderContent
// clears the target on every new navigation so stale requests never fire.
function _checkPendingTileScroll() {
    const path = window._pendingScrollToTile;
    if (!path) return;
    const tile = document.querySelector('#content [data-path="' + CSS.escape(path) + '"]');
    if (!tile) return;
    window._pendingScrollToTile = null;
    tile.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    if (typeof selectFile === 'function') selectFile(path, null);
}

function renderContent() {
    // Bump generation so any pending chunks from a previous renderContent call stop.
    _renderGen++;
    const myGen = _renderGen;
    // A new render supersedes any in-flight tile scroll from the previous navigation.
    window._pendingScrollToTile = null;

    // Remove any floating directory trickplay overlays left over from the
    // previous render (e.g. when the user double-clicks into a directory
    // before the mouse-leave event fires).
    document.querySelectorAll('.card-trickplay-sprite').forEach(s => s.remove());

    // Dismiss any open thumbnail hover popup: the card DOM is about to be
    // replaced so mouseleave will never fire on the old element.
    if (typeof dismissThumbPopup === 'function') dismissThumbPopup();

    const el = document.getElementById('content');

    // Tags tab: when no tag is selected show an empty state instead of the
    // regular browse listing so the user must choose a tag to see files.
    if (!state.sidebarSplit && state.sidebarTab === 'tags'
            && state.activeTags.size === 0 && state.mode !== 'search') {
        el.className = '';
        el.innerHTML = `<div class="empty-state"><span class="empty-state-icon">&#127991;</span><span class="empty-state-text">Selecteer een tag om bestanden te tonen</span></div>`;
        document.getElementById('entry-count').textContent = '';
        return;
    }

    // --- Zip directory mode ---
    if (state.mode === 'zip') {
        const entries = state.zipEntries;
        if (!entries.length) {
            el.className = '';
            el.innerHTML = `<div class="empty-state"><span class="empty-state-icon">🗜️</span><span class="empty-state-text">Empty archive</span></div>`;
            document.getElementById('entry-count').textContent = '0 files';
            return;
        }
        if (state.viewMode === 'grid') {
            el.className = _gridClass('file-grid');
            el.innerHTML = renderZipGrid(entries);
        } else {
            el.className = _gridClass('file-list');
            el.innerHTML = renderZipList(entries);
        }
        const images = entries.filter(e => e.is_image).length;
        document.getElementById('entry-count').textContent =
            `${entries.length} file${entries.length === 1 ? '' : 's'} (${images} image${images === 1 ? '' : 's'})`;
        return;
    }

    const items = state.mode === 'search' ? state.searchResults : state.entries;

    if (!items.length) {
        el.className = '';
        el.innerHTML = `<div class="empty-state">
            <span class="empty-state-icon">📂</span>
            <span class="empty-state-text">${state.mode === 'search' ? 'No results' : 'No files'}</span>
        </div>`;
        return;
    }

    // For search results, transform to match grid/list entry format.
    // root_path is preserved so renderGrid can compute the correct dir for
    // thumb/preview URLs in multi-root setups.
    let displayItems = state.mode === 'search'
        ? items.map(r => ({
            name: r.path.split('/').pop(),
            path: r.path,
            root_id: r.root_id,
            root_path: r.root_path,
            is_dir: false,
            size: null,
            mtime: null,
            tag_count: r.tags.length,
        }))
        : items;

    // Pre-sort: dirs first, then files — mirrors renderGrid's internal sort and
    // lets us pass correct slices to both grid and list chunked rendering.
    const _dirs  = displayItems.filter(e =>  e.is_dir);
    const _files = displayItems.filter(e => !e.is_dir);
    const sorted = [..._dirs, ..._files];

    // Entry count is known immediately — set it before the first paint.
    const _parts = [];
    if (_dirs.length  > 0) _parts.push(`${_dirs.length} folder${_dirs.length === 1 ? '' : 's'}`);
    if (_files.length > 0) _parts.push(`${_files.length} file${_files.length === 1 ? '' : 's'}`);
    document.getElementById('entry-count').textContent = _parts.join(', ');

    // Auto-trigger vtile pregen for webm/autoplay mode when navigating a directory.
    if (typeof _autoPregenVtiles === 'function') _autoPregenVtiles();

    const isGrid = state.viewMode === 'grid';

    // --- First paint: render up to _RENDER_INITIAL items synchronously ---
    if (isGrid) {
        el.className = _gridClass('file-grid');
        // renderGrid re-sorts internally; pre-sorted slice keeps dirs-first order.
        el.innerHTML = renderGrid(sorted.slice(0, _RENDER_INITIAL));
    } else {
        el.className = _gridClass('file-list');
        // Header emitted once; _renderListRows renders rows in order.
        el.innerHTML = `<div class="list-header">
        <span></span><span>Name</span><span>Size</span><span>Modified</span><span>Tags</span>
    </div>` + _renderListRows(sorted.slice(0, _RENDER_INITIAL));
    }

    if (sorted.length <= _RENDER_INITIAL) {
        // Everything fits in the first paint — schedule thumbnails and done.
        setTimeout(_dirThumbSchedule, 200);
        return;
    }

    // --- Background chunks: append remaining items without blocking the UI ---
    let _offset = _RENDER_INITIAL;
    function _appendChunk() {
        if (_renderGen !== myGen) return; // user navigated away; discard stale chunk
        // Check whether a pending scroll target landed in a previously appended
        // chunk (or in the initial synchronous paint) before adding more items.
        _checkPendingTileScroll();
        const chunk = sorted.slice(_offset, _offset + _RENDER_CHUNK);
        if (!chunk.length) {
            // All items appended — kick off thumbnail loading for any remaining
            // elements that were added by the last chunk.
            _thumbInit();
            setTimeout(_dirThumbSchedule, 0);
            return;
        }
        const tmp = document.createElement('div');
        tmp.innerHTML = isGrid ? renderGrid(chunk) : _renderListRows(chunk);
        // Append via DocumentFragment for a single reflow.
        const frag = document.createDocumentFragment();
        while (tmp.firstChild) frag.appendChild(tmp.firstChild);
        el.appendChild(frag);
        _offset += _RENDER_CHUNK;
        // Check immediately after this chunk: the target tile may have just
        // been added.  We check both here and at the top of the next call so
        // no chunk boundary is ever missed.
        _checkPendingTileScroll();
        // Register newly appended elements with the thumbnail observer/queue so
        // they load as the user scrolls to them.  Must be called after each
        // append because _thumbInit() only sees elements already in the DOM.
        _thumbInit();
        // Use requestAnimationFrame so the browser paints the current chunk
        // before we append the next one — giving a true progressive load feel.
        requestAnimationFrame(_appendChunk);
    }
    // Trigger thumbnail loading for the first batch while chunks queue up.
    setTimeout(_dirThumbSchedule, 200);
    requestAnimationFrame(_appendChunk);
}
