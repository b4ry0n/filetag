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
    for (const entry of items) {
        const isDir = entry.is_dir;
        const name = isDir ? entry.name : (entry.name || entry.path.split('/').pop());
        const path = isDir ? null : fullPath(entry);
        const selected = state.selectedFile && state.selectedFile.path === path ? ' selected' : '';
        const type_ = isDir ? 'folder' : fileType(name);

        let preview = '';
        if (isDir) {
            // Add data-dir-path so the trickplay logic can request the sprite.
            // Root cards (entry.root_path != null) keep the plain icon.
            const dirPath = entry.root_path == null ? fullPath(entry) : null;
            preview = dirPath
                ? `<div class="card-icon dir-thumb-anchor" data-dir-path="${esc(dirPath)}">${ICONS.folder}</div>`
                : `<div class="card-icon">${ICONS.folder}</div>`;
        } else if (type_ === 'image' || type_ === 'raw') {
            preview = `<div class="card-icon" data-thumb-src="/thumb/${encodePath(fullPath(entry))}${dirParam('?')}" data-name="${esc(name)}" data-thumb-hover="1">${fileIcon(name)}</div>`;
        } else if (type_ === 'video') {
            const vpath = fullPath(entry);
            preview = `<div class="card-icon" data-thumb-src="/thumb/${encodePath(vpath)}${dirParam('?')}" data-name="${esc(name)}" data-video-path="${esc(vpath)}">${ICONS.video}</div>` +
                `<div class="card-filmstrip-badge">${ICONS.video}</div>`;
        } else if (type_ === 'zip') {
            preview = `<div class="card-icon" data-thumb-src="/thumb/${encodePath(fullPath(entry))}${dirParam('?')}" data-name="${esc(name)}" data-thumb-hover="1">${ICONS.zip || ''}</div>` +
                `<div class="card-filmstrip-badge">${ICONS.zip || ''}</div>`;
        } else if (type_ === 'pdf') {
            preview = `<div class="card-icon" data-thumb-src="/thumb/${encodePath(fullPath(entry))}${dirParam('?')}" data-name="${esc(name)}" data-thumb-hover="1">${ICONS.pdf}</div>` +
                `<div class="card-filmstrip-badge">${ICONS.pdf}</div>`;
        } else {
            preview = `<div class="card-icon">${fileIcon(name)}</div>`;
        }

        const dirTagHint = isDir && entry.tag_count ? ` · ${entry.tag_count} tag${entry.tag_count === 1 ? '' : 's'}` : '';
        const meta = isDir ? `${entry.file_count} file${entry.file_count === 1 ? '' : 's'}${dirTagHint}` : formatSize(entry.size);

        if (isDir) {
            // Virtual root entry (shown at the top level when multiple roots exist)
            if (entry.root_path != null) {
                html += `<div class="card folder root-card" data-root-path="${esc(entry.root_path)}"
                    draggable="true"
                    ondragstart="_rootDragStart(event,'${jesc(entry.root_path)}')"
                    ondragover="_rootDragOver(event)"
                    ondragleave="_rootDragLeave(event)"
                    ondrop="_rootDrop(event,'${jesc(entry.root_path)}')"
                    onclick="selectRoot('${jesc(entry.root_path)}')"
                    ondblclick="enterRoot('${jesc(entry.root_path)}')">
                    <div class="card-preview"><div class="card-icon">${ICONS.root}</div></div>
                    <div class="card-body"><div class="card-name">${esc(name)}</div><div class="card-meta">root</div></div>
                </div>`;
            } else {
                const dirPath = fullPath(entry);
                const dirSelected = state.selectedDir && state.selectedDir.path === dirPath ? ' selected' : '';
                html += `<div class="card folder${dirSelected}" data-path="${esc(dirPath)}" onclick="handleDirClick('${jesc(dirPath)}','${jesc(name)}',${entry.file_count})">
                    <div class="card-preview">${preview}</div>
                    <div class="card-body"><div class="card-name">${esc(name)}</div><div class="card-meta">${meta}</div></div>
                </div>`;
            }
        } else {
            const multiSel = state.selectedPaths.has(path) ? ' selected' : '';
            const checkmark = state.selectedPaths.has(path) ? '<span class="card-check">&#10003;</span>' : '';
            const isArchiveEntry = path.includes('::');
            const gotoDirBtn = state.mode === 'search'
                ? isArchiveEntry
                    ? `<button class="card-goto" onclick="event.stopPropagation();openZipDir('${jesc(path.split('::')[0])}')" title="Go to archive">${ICONS.gotoDir}</button>`
                    : `<button class="card-goto" onclick="event.stopPropagation();navigateToParent('${jesc(path)}')" title="Go to directory">${ICONS.gotoDir}</button>`
                : '';
            const uncoveredBadge = entry.covered === false ? '<span class="card-uncovered" title="No filetag database on this filesystem">&#128274;</span>' : '';
            const uncoveredCls = entry.covered === false ? ' uncovered' : '';
            if (type_ === 'zip') {
                html += `<div class="card${multiSel}${uncoveredCls}" data-path="${esc(path)}" onclick="handleZipClick('${jesc(path)}', event)">
                    ${checkmark}${gotoDirBtn}${uncoveredBadge}<div class="card-preview">${preview}</div>
                    <div class="card-body"><div class="card-name">${esc(name)}</div><div class="card-meta">${meta}</div></div>
                </div>`;
            } else {
                const dblFn = `cvOpenFile('${jesc(path)}','${fileType(name)}')`;
                html += `<div class="card${multiSel}${uncoveredCls}" data-path="${esc(path)}" draggable="true" ondragstart="cardDragStart(event,'${jesc(path)}')" onclick="selectFile('${jesc(path)}', event)" ondblclick="${dblFn}">
                    ${checkmark}${gotoDirBtn}${uncoveredBadge}<div class="card-preview">${preview}</div>
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
    let html = `<div class="list-header">
        <span></span><span>Name</span><span>Size</span><span>Modified</span><span>Tags</span>
    </div>`;

    for (const entry of items) {
        const isDir = entry.is_dir;
        const name = isDir ? entry.name : (entry.name || entry.path.split('/').pop());
        const path = isDir ? null : fullPath(entry);
        const selected = state.selectedFile && state.selectedFile.path === path ? ' selected' : '';
        const icon = isDir ? ICONS.folder : fileIcon(name);
        const size = isDir ? '' : formatSize(entry.size);
        const date = isDir ? '' : formatDate(entry.mtime);
        const tags = isDir
            ? (entry.tag_count ? `${entry.tag_count} tag${entry.tag_count === 1 ? '' : 's'}` : '')
            : (entry.tag_count != null ? `${entry.tag_count} tags` : '');

        if (isDir) {
            if (entry.root_path != null) {
                html += `<div class="list-row folder" data-root-path="${esc(entry.root_path)}" ondblclick="enterRoot('${jesc(entry.root_path)}')" onclick="enterRoot('${jesc(entry.root_path)}')">
                    <span class="icon">${icon}</span>
                    <span class="name">${esc(name)}</span>
                    <span class="size"></span>
                    <span class="date"></span>
                    <span class="tags-count">root</span>
                </div>`;
            } else {
                const dirPath = fullPath(entry);
                const dirSelected = state.selectedDir && state.selectedDir.path === dirPath ? ' selected' : '';
                html += `<div class="list-row folder${dirSelected}" data-path="${esc(dirPath)}" onclick="handleDirClick('${jesc(dirPath)}','${jesc(name)}',${entry.file_count})">
                    <span class="icon">${icon}</span>
                    <span class="name">${esc(name)}</span>
                    <span class="size">${size}</span>
                    <span class="date">${date}</span>
                    <span class="tags-count">${tags}</span>
                </div>`;
            }
        } else {
            const multiSel = state.selectedPaths.has(path) ? ' selected' : '';
            const isArchiveEntry = path.includes('::');
            const gotoDirBtn = state.mode === 'search'
                ? isArchiveEntry
                    ? `<button class="goto-dir-btn" onclick="event.stopPropagation();openZipDir('${jesc(path.split('::')[0])}')" title="Go to archive">${ICONS.gotoDir}</button>`
                    : `<button class="goto-dir-btn" onclick="event.stopPropagation();navigateToParent('${jesc(path)}')" title="Go to directory">${ICONS.gotoDir}</button>`
                : '';
            const uncoveredBadge = entry.covered === false ? ' &#128274;' : '';
            const uncoveredCls = entry.covered === false ? ' uncovered' : '';
            if (fileType(name) === 'zip') {
                html += `<div class="list-row${multiSel}${uncoveredCls}" data-path="${esc(path)}" onclick="handleZipClick('${jesc(path)}', event)">
                    <span class="icon">${icon}</span>
                    <span class="name">${esc(name)}${uncoveredBadge}</span>
                    <span class="size">${size}</span>
                    <span class="date">${date}</span>
                    <span class="tags-count">${tags}${gotoDirBtn}</span>
                </div>`;
            } else {
                const dblFnL = `cvOpenFile('${jesc(path)}','${fileType(name)}')`;
                html += `<div class="list-row${multiSel}${uncoveredCls}" data-path="${esc(path)}" draggable="true" ondragstart="cardDragStart(event,'${jesc(path)}')" onclick="selectFile('${jesc(path)}', event)" ondblclick="${dblFnL}">
                    <span class="icon">${icon}</span>
                    <span class="name">${esc(name)}${uncoveredBadge}</span>
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
    event.dataTransfer.effectAllowed = 'copy';
    // Prevent the root-reorder drag from interfering.
    event.stopPropagation();
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Root drag-and-drop reordering
// ---------------------------------------------------------------------------

let _rootDragPath = null;

function _rootDragStart(ev, rootPath) {
    _rootDragPath = rootPath;
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
    content.querySelectorAll('.card[data-path]').forEach(card => {
        const p = card.dataset.path;
        const want = state.selectedPaths.has(p);
        const has = card.classList.contains('selected');
        if (want === has) return;
        card.classList.toggle('selected', want);
        const existing = card.querySelector('.card-check');
        if (want && !existing) {
            const chk = document.createElement('span');
            chk.className = 'card-check';
            chk.innerHTML = '&#10003;';
            card.prepend(chk);
        } else if (!want && existing) {
            existing.remove();
        }
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

function renderContent() {
    // Remove any floating directory trickplay overlays left over from the
    // previous render (e.g. when the user double-clicks into a directory
    // before the mouse-leave event fires).
    document.querySelectorAll('.card-trickplay-sprite').forEach(s => s.remove());

    const el = document.getElementById('content');

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
            el.className = 'file-grid';
            el.innerHTML = renderZipGrid(entries);
        } else {
            el.className = 'file-list';
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

    // For search results, transform to match grid/list entry format
    const displayItems = state.mode === 'search'
        ? items.map(r => ({
            name: r.path.split('/').pop(),
            path: r.path,
            is_dir: false,
            size: null,
            mtime: null,
            tag_count: r.tags.length,
        }))
        : items;

    if (state.viewMode === 'grid') {
        el.className = 'file-grid';
        el.innerHTML = renderGrid(displayItems);
    } else {
        el.className = 'file-list';
        el.innerHTML = renderList(displayItems);
    }

    // Entry count
    const dirs = displayItems.filter(e => e.is_dir).length;
    const files = displayItems.filter(e => !e.is_dir).length;
    const parts = [];
    if (dirs > 0) parts.push(`${dirs} folder${dirs === 1 ? '' : 's'}`);
    if (files > 0) parts.push(`${files} file${files === 1 ? '' : 's'}`);
    document.getElementById('entry-count').textContent = parts.join(', ');
}
