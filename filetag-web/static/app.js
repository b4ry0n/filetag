// ---------------------------------------------------------------------------
// Icons (inline SVG)
// ---------------------------------------------------------------------------

const ICONS = {
    folder: '<svg viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="0.5" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z"/></svg>',
    file: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>',
    image: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="8.5" cy="8.5" r="1.5"/><polyline points="21 15 16 10 5 21"/></svg>',
    audio: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>',
    video: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="23 7 16 12 23 17 23 7"/><rect x="1" y="5" width="15" height="14" rx="2"/></svg>',
    gotoDir: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M1 4.5v7A1.5 1.5 0 002.5 13h11A1.5 1.5 0 0015 11.5V6a1.5 1.5 0 00-1.5-1.5H7L5.5 3H2.5A1.5 1.5 0 001 4.5z"/><polyline points="9 8 11 10 9 12"/><line x1="6" y1="10" x2="11" y2="10"/></svg>',
};

// ---------------------------------------------------------------------------
// File type detection
// ---------------------------------------------------------------------------

const EXT_MAP = {
    image: ['jpg','jpeg','png','gif','webp','svg','bmp','ico','tiff','tif','avif'],
    audio: ['mp3','flac','wav','ogg','opus','aac','m4a','wma','aiff','alac'],
    video: ['mp4','webm','mkv','avi','mov','wmv','flv','m4v','ts'],
};

function fileType(name) {
    const ext = name.split('.').pop().toLowerCase();
    for (const [type_, exts] of Object.entries(EXT_MAP)) {
        if (exts.includes(ext)) return type_;
    }
    return 'file';
}

function fileIcon(name) {
    const type_ = fileType(name);
    return ICONS[type_] || ICONS.file;
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

function formatSize(bytes) {
    if (bytes == null) return '';
    if (bytes < 1024) return bytes + ' B';
    const units = ['KiB', 'MiB', 'GiB', 'TiB'];
    let size = bytes / 1024;
    for (const unit of units) {
        if (size < 1024) return size.toFixed(1) + ' ' + unit;
        size /= 1024;
    }
    return size.toFixed(1) + ' PiB';
}

function formatDate(mtimeNs) {
    if (!mtimeNs) return '';
    const ms = Math.floor(mtimeNs / 1_000_000);
    const d = new Date(ms);
    return d.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}

function formatTag(tag) {
    if (tag.value) return tag.name + '=' + tag.value;
    return tag.name;
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

const state = {
    mode: 'browse', // browse | search
    currentPath: '',
    viewMode: 'grid',
    showHidden: false,
    tags: [],
    entries: [],
    searchQuery: '',
    searchResults: [],
    selectedFile: null,  // { path, size, file_id, mtime, indexed_at, tags } | null
    selectedDir: null,   // { path, name, file_count } | null
    selectedPaths: new Set(), // multi-select: Set of paths
    selectedFilesData: new Map(), // path → file detail (for tag aggregation)
    info: null,
    detailOpen: true,
};

let _lastClickedPath = null; // for shift-range selection
let _armedBulkTag = null;    // two-step delete: which tag is armed

// ---------------------------------------------------------------------------
// API
// ---------------------------------------------------------------------------

async function api(url) {
    const res = await fetch(url);
    if (!res.ok) {
        const body = await res.json().catch(() => ({}));
        throw new Error(body.error || res.statusText);
    }
    return res.json();
}

async function apiPost(url, body) {
    const res = await fetch(url, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
    });
    if (!res.ok) {
        const b = await res.json().catch(() => ({}));
        throw new Error(b.error || res.statusText);
    }
    return res.json();
}

async function loadInfo() {
    state.info = await api('/api/info');
}

async function loadTags() {
    state.tags = await api('/api/tags');
}

async function loadFiles(path) {
    const url = '/api/files?path=' + encodeURIComponent(path) + (state.showHidden ? '&show_hidden=true' : '');
    const data = await api(url);
    state.currentPath = data.path;
    state.entries = data.entries;
    state.mode = 'browse';
    state.searchQuery = '';
}

async function searchFiles(query) {
    try {
        const data = await api('/api/search?q=' + encodeURIComponent(query));
        state.searchQuery = query;
        state.searchResults = data.results;
        state.mode = 'search';
        state.selectedFile = null;
    } catch (e) {
        state.searchQuery = query;
        state.searchResults = [];
        state.mode = 'search';
        state.selectedFile = null;
    }
    state.selectedPaths.clear();
    state.selectedFilesData.clear();
    _lastClickedPath = null;
    _armedBulkTag = null;
}

async function loadFileDetail(path) {
    state.selectedFile = await api('/api/file?path=' + encodeURIComponent(path));
    state.selectedDir = null;
}

function selectDir(path, name, fileCount) {
    const anchor = saveScrollAnchor(path);
    state.selectedDir = { path, name, file_count: fileCount };
    state.selectedFile = null;
    if (!state.detailOpen) {
        state.detailOpen = true;
        document.querySelector('.layout').classList.remove('detail-collapsed');
        document.getElementById('detail-toggle').classList.add('active');
    }
    renderDetail();
    restoreScrollAnchor(anchor);
}

// Timer to distinguish single click (select) from double click (navigate) on directories.
let _dirClickTimer = null;

function handleDirClick(path, name, fileCount) {
    if (_dirClickTimer) {
        clearTimeout(_dirClickTimer);
        _dirClickTimer = null;
        navigateTo(path); // double click
    } else {
        _dirClickTimer = setTimeout(() => {
            _dirClickTimer = null;
            selectDir(path, name, fileCount); // single click
        }, 250);
    }
}

async function addTagToFile(path, tagStr) {
    await apiPost('/api/tag', { path, tags: [tagStr] });
    await loadFileDetail(path);
    await loadTags();
    if (state.mode === 'browse') await loadFiles(state.currentPath);
}

async function removeTagFromFile(path, tagStr) {
    await apiPost('/api/untag', { path, tags: [tagStr] });
    await loadFileDetail(path);
    await loadTags();
    if (state.mode === 'browse') await loadFiles(state.currentPath);
}

// ---------------------------------------------------------------------------
// Full path helper
// ---------------------------------------------------------------------------

function fullPath(entry) {
    if (state.mode === 'search') return entry.path;
    if (state.currentPath) return state.currentPath + '/' + entry.name;
    return entry.name;
}

// ---------------------------------------------------------------------------
// Render: Sidebar tags
// ---------------------------------------------------------------------------

const TAG_COLORS = [
    '#ef4444', '#f97316', '#f59e0b', '#eab308', '#84cc16',
    '#22c55e', '#14b8a6', '#06b6d4', '#3b82f6', '#6366f1',
    '#8b5cf6', '#a855f7', '#d946ef', '#ec4899', '#f43f5e',
];

function colorDot(color) {
    if (!color) return '';
    return `<span class="tag-color-dot" style="background:${color}"></span>`;
}

function renderTags() {
    const el = document.getElementById('tag-list');
    if (!state.tags.length) {
        el.innerHTML = '<div class="empty-state"><span class="empty-state-text">No tags</span></div>';
        return;
    }

    // Group tags by prefix
    const groups = {};
    const standalone = [];
    for (const tag of state.tags) {
        const slash = tag.name.indexOf('/');
        if (slash > 0) {
            const prefix = tag.name.slice(0, slash);
            const suffix = tag.name.slice(slash + 1);
            if (!groups[prefix]) groups[prefix] = [];
            groups[prefix].push({ suffix, fullName: tag.name, count: tag.count, color: tag.color });
        } else {
            standalone.push(tag);
        }
    }

    let html = '';

    // Grouped tags
    const groupNames = Object.keys(groups).sort();
    for (const prefix of groupNames) {
        const items = groups[prefix].sort((a, b) => a.suffix.localeCompare(b.suffix));
        const totalCount = items.reduce((s, i) => s + i.count, 0);
        html += `<div class="tag-group">
            <button class="tag-group-label" onclick="toggleTagGroup(this)">
                <span class="chevron">▸</span>
                ${esc(prefix)}/
                <span class="count">${totalCount}</span>
            </button>
            <div class="tag-group-items">`;
        for (const item of items) {
            const q = quoteTag(item.fullName);
            const active = state.mode === 'search' && state.searchQuery === q ? ' active' : '';
            html += `<button class="tag-item${active}" onclick="doTagSearch('${esc(item.fullName)}')" oncontextmenu="showTagMenu(event, '${esc(item.fullName)}')">
                ${colorDot(item.color)}${esc(item.suffix)} <span class="count">${item.count}</span>
            </button>`;
        }
        html += '</div></div>';
    }

    // Standalone tags
    for (const tag of standalone.sort((a, b) => a.name.localeCompare(b.name))) {
        const q = quoteTag(tag.name);
        const active = state.mode === 'search' && state.searchQuery === q ? ' active' : '';
        html += `<button class="tag-item tag-standalone${active}" onclick="doTagSearch('${esc(tag.name)}')" oncontextmenu="showTagMenu(event, '${esc(tag.name)}')">
            ${colorDot(tag.color)}${esc(tag.name)} <span class="count">${tag.count}</span>
        </button>`;
    }

    el.innerHTML = html;
}

// ---------------------------------------------------------------------------
// Tag context menu
// ---------------------------------------------------------------------------

function showTagMenu(e, tagName) {
    e.preventDefault();
    e.stopPropagation();
    closeTagMenu();

    const tag = state.tags.find(t => t.name === tagName);
    const currentColor = tag?.color || null;

    let swatches = TAG_COLORS.map(c => {
        const sel = c === currentColor ? ' selected' : '';
        return `<button class="tag-menu-swatch${sel}" style="background:${c}" onclick="setTagColor('${esc(tagName)}','${c}')"></button>`;
    }).join('');
    // "no color" swatch
    const noSel = !currentColor ? ' selected' : '';
    swatches = `<button class="tag-menu-swatch tag-menu-swatch-none${noSel}" onclick="setTagColor('${esc(tagName)}', null)" title="No color">✕</button>` + swatches;

    const menu = document.createElement('div');
    menu.id = 'tag-context-menu';
    menu.className = 'tag-context-menu';
    menu.innerHTML = `
        <div class="tag-menu-header">${esc(tagName)}</div>
        <div class="tag-menu-section">
            <div class="tag-menu-label">Color</div>
            <div class="tag-menu-swatches">${swatches}</div>
        </div>
        <div class="tag-menu-divider"></div>
        <button class="tag-menu-action tag-menu-delete" onclick="deleteTag('${esc(tagName)}')">Delete tag</button>
    `;
    document.body.appendChild(menu);

    // Position near the click
    const rect = menu.getBoundingClientRect();
    let x = e.clientX;
    let y = e.clientY;
    if (x + rect.width > window.innerWidth) x = window.innerWidth - rect.width - 8;
    if (y + rect.height > window.innerHeight) y = window.innerHeight - rect.height - 8;
    menu.style.left = x + 'px';
    menu.style.top = y + 'px';

    // Close on outside click (wait a tick so this click doesn't close it)
    requestAnimationFrame(() => {
        document.addEventListener('click', closeTagMenu, { once: true });
    });
}

function closeTagMenu() {
    const menu = document.getElementById('tag-context-menu');
    if (menu) menu.remove();
}

async function setTagColor(tagName, color) {
    closeTagMenu();
    await apiPost('/api/tag-color', { name: tagName, color });
    await loadTags();
    render();
}

async function deleteTag(tagName) {
    closeTagMenu();
    const tag = state.tags.find(t => t.name === tagName);
    const count = tag?.count || 0;
    if (count > 0 && !confirm(`Delete tag "${tagName}"? It is applied to ${count} file(s).`)) {
        return;
    }
    await apiPost('/api/delete-tag', { name: tagName });
    await loadTags();
    if (state.selectedFile) await loadFileDetail(state.selectedFile.path);
    render();
}

// ---------------------------------------------------------------------------
// Render: Breadcrumb
// ---------------------------------------------------------------------------

function renderBreadcrumb() {
    const el = document.getElementById('breadcrumb');

    if (state.mode === 'search') {
        el.innerHTML = `<span class="breadcrumb-item current">Search: ${esc(state.searchQuery)}</span>`;
        return;
    }

    let html = `<button class="breadcrumb-item${state.currentPath === '' ? ' current' : ''}" onclick="navigateTo('')">/</button>`;

    if (state.currentPath) {
        const parts = state.currentPath.split('/');
        let accumulated = '';
        for (let i = 0; i < parts.length; i++) {
            accumulated += (i === 0 ? '' : '/') + parts[i];
            const isCurrent = i === parts.length - 1;
            const path = accumulated;
            if (i > 0) html += `<span class="breadcrumb-sep">/</span>`;
            html += `<button class="breadcrumb-item${isCurrent ? ' current' : ''}" onclick="navigateTo('${esc(path)}')">${esc(parts[i])}</button>`;
        }
    }

    el.innerHTML = html;
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
            preview = `<div class="card-icon">${ICONS.folder}</div>`;
        } else if (type_ === 'image') {
            preview = `<img src="/preview/${encodeURI(fullPath(entry))}" loading="lazy" alt="">`;
        } else {
            preview = `<div class="card-icon">${fileIcon(name)}</div>`;
        }

        const meta = isDir ? `${entry.file_count} file${entry.file_count === 1 ? '' : 's'}` : formatSize(entry.size);

        if (isDir) {
            const dirPath = fullPath(entry);
            const dirSelected = state.selectedDir && state.selectedDir.path === dirPath ? ' selected' : '';
            html += `<div class="card folder${dirSelected}" data-path="${esc(dirPath)}" onclick="handleDirClick('${esc(dirPath)}','${esc(name)}',${entry.file_count})">
                <div class="card-preview">${preview}</div>
                <div class="card-body"><div class="card-name">${esc(name)}</div><div class="card-meta">${meta}</div></div>
            </div>`;
        } else {
            const multiSel = state.selectedPaths.has(path) ? ' selected' : '';
            const checkmark = state.selectedPaths.has(path) ? '<span class="card-check">&#10003;</span>' : '';
            const gotoDirBtn = state.mode === 'search'
                ? `<button class="card-goto" onclick="event.stopPropagation();navigateToParent('${esc(path)}')" title="Go to directory">${ICONS.gotoDir}</button>`
                : '';
            html += `<div class="card${multiSel}" data-path="${esc(path)}" onclick="selectFile('${esc(path)}', event)">
                ${checkmark}${gotoDirBtn}<div class="card-preview">${preview}</div>
                <div class="card-body"><div class="card-name">${esc(name)}</div><div class="card-meta">${meta}</div></div>
            </div>`;
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
        const tags = isDir ? `${entry.file_count} files` : (entry.tag_count != null ? `${entry.tag_count} tags` : '');

        if (isDir) {
            const dirPath = fullPath(entry);
            const dirSelected = state.selectedDir && state.selectedDir.path === dirPath ? ' selected' : '';
            html += `<div class="list-row folder${dirSelected}" data-path="${esc(dirPath)}" onclick="handleDirClick('${esc(dirPath)}','${esc(name)}',${entry.file_count})">
                <span class="icon">${icon}</span>
                <span class="name">${esc(name)}</span>
                <span class="size">${size}</span>
                <span class="date">${date}</span>
                <span class="tags-count">${tags}</span>
            </div>`;
        } else {
            const multiSel = state.selectedPaths.has(path) ? ' selected' : '';
            const gotoDirBtn = state.mode === 'search'
                ? `<button class="goto-dir-btn" onclick="event.stopPropagation();navigateToParent('${esc(path)}')" title="Go to directory">${ICONS.gotoDir}</button>`
                : '';
            html += `<div class="list-row${multiSel}" data-path="${esc(path)}" onclick="selectFile('${esc(path)}', event)">
                <span class="icon">${icon}</span>
                <span class="name">${esc(name)}</span>
                <span class="size">${size}</span>
                <span class="date">${date}</span>
                <span class="tags-count">${tags}${gotoDirBtn}</span>
            </div>`;
        }
    }
    return html;
}

// ---------------------------------------------------------------------------
// Render: Content area
// ---------------------------------------------------------------------------

function renderContent() {
    const el = document.getElementById('content');
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

// ---------------------------------------------------------------------------
// Render: Detail panel
// ---------------------------------------------------------------------------

function renderDetail() {
    const panel = document.getElementById('detail');

    // Multi-select bulk panel
    if (state.selectedPaths.size > 1) {
        const count = state.selectedPaths.size;
        const bulkTags = aggregateBulkTags();
        const chipsHtml = renderBulkTagChips(bulkTags, count);
        panel.innerHTML = `
            <div class="detail-header">
                <h3>${count} files selected</h3>
                <button class="detail-close" onclick="clearSelection()" title="Clear selection">&times;</button>
            </div>
            <div class="bulk-tag-section">
                ${bulkTags.length > 0 ? `<p class="bulk-section-label">Tags on selected files</p>
                <div class="bulk-tag-chips" id="bulk-tag-chips">${chipsHtml}</div>` : ''}
                <p class="bulk-section-label" style="margin-top:12px">Add tag</p>
                <div class="tag-add-form">
                    <input type="text" id="bulk-tag-input" placeholder="Tag (e.g. genre/rock)">
                    <button onclick="doBulkAddTag()">Add</button>
                </div>
                <div id="bulk-status" class="bulk-status"></div>
            </div>`;
        attachTagAutocomplete(document.getElementById('bulk-tag-input'), () => doBulkAddTag());
        return;
    }

    if (!state.selectedFile && !state.selectedDir) {
        panel.innerHTML = '<div class="detail-empty">Select a file or folder to see details</div>';
        return;
    }

    // Directory selected
    if (state.selectedDir) {
        const d = state.selectedDir;
        panel.innerHTML = `
            <div class="detail-header">
                <h3>${esc(d.name)}</h3>
                <button class="detail-close" onclick="closeDetail()" title="Close">&times;</button>
            </div>
            <div class="detail-preview">
                <div class="no-preview" style="color:#fab005">${ICONS.folder}</div>
            </div>
            <div class="detail-meta">
                <div class="detail-meta-row"><span class="detail-meta-label">Path</span><span class="detail-meta-value">${esc(d.path)}</span></div>
                <div class="detail-meta-row"><span class="detail-meta-label">Items</span><span class="detail-meta-value">${d.file_count}</span></div>
            </div>`;
        return;
    }

    const f = state.selectedFile;
    const name = f.path.split('/').pop();
    const type_ = fileType(name);

    let preview;
    if (type_ === 'image') {
        preview = `<img src="/preview/${encodeURI(f.path)}" alt="${esc(name)}">`;
    } else if (type_ === 'audio') {
        preview = `<audio controls preload="metadata" src="/preview/${encodeURI(f.path)}"></audio>`;
    } else if (type_ === 'video') {
        preview = `<video controls preload="metadata" src="/preview/${encodeURI(f.path)}"></video>`;
    } else {
        preview = `<div class="no-preview">${fileIcon(name)}</div>`;
    }

    const tagChips = f.tags.length === 0
        ? '<span class="no-tags">No tags assigned</span>'
        : f.tags.map(t => {
            const tagStr = formatTag(t);
            const stateTag = state.tags.find(st => st.name === t.name);
            const chipColor = stateTag?.color ? ` style="border-left: 3px solid ${stateTag.color}"` : '';
            return `<span class="tag-chip"${chipColor}>${esc(tagStr)}<button class="remove" onclick="doRemoveTag('${esc(f.path)}','${esc(tagStr)}')">&times;</button></span>`;
        }).join('');

    panel.innerHTML = `
        <div class="detail-header">
            <h3>${esc(name)}</h3>
            <button class="detail-close" onclick="closeDetail()" title="Close">&times;</button>
        </div>
        <div class="detail-preview">${preview}</div>
        <div class="detail-meta">
            <div class="detail-meta-row"><span class="detail-meta-label">Path</span><span class="detail-meta-value">${esc(f.path)}</span></div>
            <div class="detail-meta-row"><span class="detail-meta-label">Size</span><span class="detail-meta-value">${formatSize(f.size)}</span></div>
            ${f.indexed_at ? `<div class="detail-meta-row"><span class="detail-meta-label">Indexed</span><span class="detail-meta-value">${esc(f.indexed_at)}</span></div>` : ''}
        </div>
        <div class="detail-tags-section">
            <h4>Tags</h4>
            <div class="detail-tags">${tagChips}</div>
            <div class="tag-add-form">
                <input type="text" id="tag-input" placeholder="Add tag (e.g. genre/rock)">
                <button onclick="doAddTag()">Add</button>
            </div>
        </div>`;

    attachTagAutocomplete(document.getElementById('tag-input'), () => doAddTag());
}

// Update only the tag chips in the detail panel, leaving the preview (video/audio/image) untouched.
function renderDetailTagsOnly() {
    if (!state.selectedFile) return;
    const tagsEl = document.querySelector('#detail .detail-tags');
    if (!tagsEl) return;
    const f = state.selectedFile;
    const tagChips = f.tags.length === 0
        ? '<span class="no-tags">No tags assigned</span>'
        : f.tags.map(t => {
            const tagStr = formatTag(t);
            const stateTag = state.tags.find(st => st.name === t.name);
            const chipColor = stateTag?.color ? ` style="border-left: 3px solid ${stateTag.color}"` : '';
            return `<span class="tag-chip"${chipColor}>${esc(tagStr)}<button class="remove" onclick="doRemoveTag('${esc(f.path)}','${esc(tagStr)}')">&times;</button></span>`;
        }).join('');
    tagsEl.innerHTML = tagChips;
}

// ---------------------------------------------------------------------------
// Bulk tag helpers (multi-select)
// ---------------------------------------------------------------------------

function aggregateBulkTags() {
    const counts = new Map(); // tagStr → count of selected files that have it
    for (const [path, data] of state.selectedFilesData) {
        if (!state.selectedPaths.has(path)) continue;
        for (const t of (data.tags || [])) {
            const str = formatTag(t);
            counts.set(str, (counts.get(str) || 0) + 1);
        }
    }
    return [...counts.entries()]
        .map(([tagStr, count]) => ({ tagStr, count }))
        .sort((a, b) => b.count - a.count || a.tagStr.localeCompare(b.tagStr));
}

function renderBulkTagChips(bulkTags, total) {
    if (bulkTags.length === 0) return '';
    return bulkTags.map(({ tagStr, count }) => {
        const stateTag = state.tags.find(st => st.name === tagStr || st.name === tagStr.split('=')[0]);
        const chipBorder = stateTag?.color ? ` style="border-left: 3px solid ${stateTag.color}"` : '';
        const countBadge = count < total
            ? `<span class="bulk-chip-count">${count}/${total}</span>`
            : '';
        const isArmed = _armedBulkTag === tagStr;
        if (isArmed) {
            return `<span class="bulk-chip armed"${chipBorder}>
                <span class="bulk-chip-label">${esc(tagStr)}${countBadge}</span>
                <button class="bulk-chip-cancel" onclick="armBulkTag('${esc(tagStr)}')" title="Cancel">&#8617;</button>
                <button class="bulk-chip-fire" onclick="doBulkRemoveTagChip('${esc(tagStr)}')">Remove</button>
            </span>`;
        }
        return `<span class="bulk-chip"${chipBorder}>
            <span class="bulk-chip-label">${esc(tagStr)}${countBadge}</span>
            <button class="bulk-chip-arm" onclick="armBulkTag('${esc(tagStr)}')" title="Remove from selection">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14H6L5 6"/><path d="M10 11v6M14 11v6"/><path d="M9 6V4h6v2"/></svg>
            </button>
        </span>`;
    }).join('');
}

function armBulkTag(tagStr) {
    _armedBulkTag = _armedBulkTag === tagStr ? null : tagStr;
    const el = document.getElementById('bulk-tag-chips');
    if (el) {
        const total = state.selectedPaths.size;
        el.innerHTML = renderBulkTagChips(aggregateBulkTags(), total);
    }
}

async function doBulkRemoveTagChip(tagStr) {
    _armedBulkTag = null;
    // Only remove from files that actually have this tag
    const paths = [...state.selectedPaths].filter(p => {
        const data = state.selectedFilesData.get(p);
        return data && data.tags.some(t => formatTag(t) === tagStr);
    });
    await Promise.all(paths.map(p => apiPost('/api/untag', { path: p, tags: [tagStr] })));
    // Update cache locally
    for (const p of paths) {
        const d = state.selectedFilesData.get(p);
        if (d) d.tags = d.tags.filter(t => formatTag(t) !== tagStr);
    }
    await loadTags();
    if (state.mode === 'browse') await loadFiles(state.currentPath);
    const status = document.getElementById('bulk-status');
    if (status) status.textContent = `Removed "${tagStr}" from ${paths.length} file${paths.length === 1 ? '' : 's'}.`;
    const el = document.getElementById('bulk-tag-chips');
    if (el) el.innerHTML = renderBulkTagChips(aggregateBulkTags(), state.selectedPaths.size);
    renderTags();
    renderContent();
}

// ---------------------------------------------------------------------------
// Render: DB info header
// ---------------------------------------------------------------------------

function renderInfo() {
    if (!state.info) return;
    const i = state.info;
    document.getElementById('db-info').textContent =
        `${i.files} files, ${i.tags} tags, ${formatSize(i.total_size)}`;
}

// ---------------------------------------------------------------------------
// Full render
// ---------------------------------------------------------------------------

function render() {
    renderTags();
    renderBreadcrumb();
    renderContent();
    renderDetail();
    renderInfo();
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

async function navigateTo(path) {
    state.selectedFile = null;
    state.selectedDir = null;
    state.selectedPaths.clear();
    state.selectedFilesData.clear();
    _lastClickedPath = null;
    _armedBulkTag = null;
    await loadFiles(path);
    render();
}

async function doSearch() {
    const input = document.getElementById('search-input');
    const query = input.value.trim();
    if (!query) return;
    await searchFiles(query);
    document.getElementById('search-clear').hidden = false;
    render();
}

function doClearSearch() {
    document.getElementById('search-input').value = '';
    document.getElementById('search-clear').hidden = true;
    navigateTo(state.currentPath || '');
}

function navigateToParent(filePath) {
    const parts = filePath.split('/');
    const dir = parts.length > 1 ? parts.slice(0, -1).join('/') : '';
    document.getElementById('search-input').value = '';
    document.getElementById('search-clear').hidden = true;
    navigateTo(dir);
}

/// Quote a tag name for the query language if it contains special characters.
function quoteTag(name) {
    if (/[\s()"']/.test(name)) return '"' + name.replace(/"/g, "'") + '"';
    return name;
}

async function doTagSearch(tagName) {
    const q = quoteTag(tagName);
    document.getElementById('search-input').value = q;
    await searchFiles(q);
    document.getElementById('search-clear').hidden = false;
    render();
}

async function selectFile(path, event) {
    const layout = document.querySelector('.layout');
    const anchor = saveScrollAnchor(path);

    const isMulti = event && (event.ctrlKey || event.metaKey);
    const isShift = event && event.shiftKey;

    if (isMulti) {
        // Toggle this path in the multi-select set
        if (state.selectedPaths.has(path)) {
            state.selectedPaths.delete(path);
        } else {
            state.selectedPaths.add(path);
            if (!state.selectedFilesData.has(path)) {
                const data = await api('/api/file?path=' + encodeURIComponent(path));
                state.selectedFilesData.set(path, data);
            }
        }
        _armedBulkTag = null;
        _lastClickedPath = path;
        // Keep selectedFile in sync with the most recently toggled file, or clear if set is empty
        if (state.selectedPaths.size === 1) {
            await loadFileDetail([...state.selectedPaths][0]);
        } else if (state.selectedPaths.size === 0) {
            state.selectedFile = null;
            state.selectedDir = null;
        } else {
            // Multi: update state but don't reload preview
            state.selectedDir = null;
        }
    } else if (isShift && _lastClickedPath) {
        // Range-select between _lastClickedPath and path
        const items = state.mode === 'search' ? state.searchResults : state.entries;
        const paths = items.filter(e => !e.is_dir).map(e => state.mode === 'search' ? e.path : fullPath(e));
        const a = paths.indexOf(_lastClickedPath);
        const b = paths.indexOf(path);
        if (a !== -1 && b !== -1) {
            const [lo, hi] = a < b ? [a, b] : [b, a];
            for (let i = lo; i <= hi; i++) state.selectedPaths.add(paths[i]);
        }
        _armedBulkTag = null;
        state.selectedDir = null;
        // Batch-fetch file data for newly added paths
        await Promise.all([...state.selectedPaths].map(async p => {
            if (!state.selectedFilesData.has(p)) {
                const data = await api('/api/file?path=' + encodeURIComponent(p));
                state.selectedFilesData.set(p, data);
            }
        }));
    } else {
        // Plain click: single select
        state.selectedPaths.clear();
        state.selectedPaths.add(path);
        _lastClickedPath = path;
        await loadFileDetail(path);
    }

    if (!state.detailOpen) {
        state.detailOpen = true;
        layout.classList.remove('detail-collapsed');
        document.getElementById('detail-toggle').classList.add('active');
    }
    render();
    restoreScrollAnchor(anchor);
}

async function doAddTag() {
    if (!state.selectedFile) return;
    const input = document.getElementById('tag-input');
    const tagStr = input.value.trim();
    if (!tagStr) return;
    await addTagToFile(state.selectedFile.path, tagStr);
    input.value = '';
    renderTags();
    renderContent();
    renderDetailTagsOnly();
    input.focus();
}

async function doRemoveTag(path, tagStr) {
    await removeTagFromFile(path, tagStr);
    renderTags();
    renderContent();
    renderDetailTagsOnly();
}

// ---------------------------------------------------------------------------
// Tag autocomplete
// ---------------------------------------------------------------------------

function attachTagAutocomplete(inputEl, submitFn) {
    let _dropdown = null;
    let _activeIdx = -1;

    function getMatches(query) {
        const q = query.toLowerCase();
        if (!q) {
            // Show top tags by count, excluding ones already on the selected file(s)
            const applied = new Set(
                (state.selectedFile?.tags || []).map(t => formatTag(t))
            );
            return [...state.tags]
                .filter(t => !applied.has(t.name))
                .sort((a, b) => b.count - a.count)
                .slice(0, 12);
        }
        return state.tags
            .filter(t => t.name.toLowerCase().includes(q))
            .sort((a, b) => {
                // Prefer prefix matches first
                const aPrefix = a.name.toLowerCase().startsWith(q);
                const bPrefix = b.name.toLowerCase().startsWith(q);
                if (aPrefix !== bPrefix) return aPrefix ? -1 : 1;
                return b.count - a.count;
            })
            .slice(0, 15);
    }

    function buildDropdown(tags) {
        if (!_dropdown) {
            _dropdown = document.createElement('ul');
            _dropdown.className = 'tag-autocomplete';
            inputEl.parentElement.appendChild(_dropdown);
        }
        _activeIdx = -1;
        if (!tags.length) { _dropdown.innerHTML = ''; _dropdown.hidden = true; return; }
        _dropdown.innerHTML = tags.map(tag => {
            const dot = tag.color
                ? `<span class="tag-color-dot" style="background:${tag.color}"></span>`
                : '';
            return `<li data-tagname="${esc(tag.name)}">${dot}<span class="ac-name">${esc(tag.name)}</span><span class="ac-count">${tag.count}</span></li>`;
        }).join('');
        _dropdown.hidden = false;
        _dropdown.querySelectorAll('li').forEach(li => {
            li.addEventListener('mousedown', e => {
                e.preventDefault(); // keep focus on input
                inputEl.value = li.dataset.tagname;
                closeDropdown();
                submitFn();
            });
        });
    }

    function closeDropdown() {
        if (_dropdown) { _dropdown.hidden = true; }
        _activeIdx = -1;
    }

    function setActive(idx) {
        const items = _dropdown ? _dropdown.querySelectorAll('li') : [];
        items.forEach(li => li.classList.remove('ac-active'));
        _activeIdx = idx;
        if (_activeIdx >= 0 && _activeIdx < items.length) {
            items[_activeIdx].classList.add('ac-active');
            inputEl.value = items[_activeIdx].dataset.tagname;
            items[_activeIdx].scrollIntoView({ block: 'nearest' });
        }
    }

    inputEl.addEventListener('input', () => buildDropdown(getMatches(inputEl.value.trim())));

    inputEl.addEventListener('focus', () => buildDropdown(getMatches(inputEl.value.trim())));

    inputEl.addEventListener('blur', () => setTimeout(closeDropdown, 150));

    inputEl.addEventListener('keydown', e => {
        const items = _dropdown ? _dropdown.querySelectorAll('li') : [];
        const count = items.length;
        if (e.key === 'ArrowDown') {
            e.preventDefault();
            if (!_dropdown || _dropdown.hidden) buildDropdown(getMatches(inputEl.value.trim()));
            setActive(Math.min(_activeIdx + 1, count - 1));
        } else if (e.key === 'ArrowUp') {
            e.preventDefault();
            setActive(Math.max(_activeIdx - 1, 0));
        } else if (e.key === 'Escape') {
            e.preventDefault();
            closeDropdown();
        } else if (e.key === 'Enter' || e.key === 'Tab') {
            e.preventDefault();
            if (_activeIdx >= 0 && !_dropdown.hidden) {
                inputEl.value = items[_activeIdx].dataset.tagname;
                closeDropdown();
            } else {
                closeDropdown();
            }
            submitFn();
        }
    });
}

function clearSelection() {
    state.selectedPaths.clear();
    state.selectedFilesData.clear();
    state.selectedFile = null;
    state.selectedDir = null;
    _lastClickedPath = null;
    _armedBulkTag = null;
    render();
}

async function doBulkAddTag() {
    const input = document.getElementById('bulk-tag-input');
    const tagStr = input.value.trim();
    if (!tagStr) return;
    const paths = [...state.selectedPaths];
    const status = document.getElementById('bulk-status');
    status.textContent = 'Adding...';
    await Promise.all(paths.map(p => apiPost('/api/tag', { path: p, tags: [tagStr] })));
    // Refresh cached data for all selected files
    await Promise.all(paths.map(async p => {
        const data = await api('/api/file?path=' + encodeURIComponent(p));
        state.selectedFilesData.set(p, data);
    }));
    await loadTags();
    if (state.mode === 'browse') await loadFiles(state.currentPath);
    input.value = '';
    status.textContent = `Added "${tagStr}" to ${paths.length} file${paths.length === 1 ? '' : 's'}.`;
    renderTags();
    renderContent();
    const el = document.getElementById('bulk-tag-chips');
    if (el) el.innerHTML = renderBulkTagChips(aggregateBulkTags(), state.selectedPaths.size);
    else renderDetail(); // chips container not in DOM yet (first tag added)
    input.focus();
}

async function toggleShowHidden() {
    state.showHidden = !state.showHidden;
    document.getElementById('hidden-toggle').classList.toggle('active', state.showHidden);
    if (state.mode === 'browse') {
        await loadFiles(state.currentPath);
        renderContent();
    }
}

function setViewMode(mode) {
    state.viewMode = mode;
    document.getElementById('view-grid').classList.toggle('active', mode === 'grid');
    document.getElementById('view-list').classList.toggle('active', mode === 'list');
    document.getElementById('zoom-slider').style.display = mode === 'grid' ? '' : 'none';
    renderContent();
}

function toggleDetailPanel() {
    const activePath = state.selectedFile?.path || state.selectedDir?.path || null;
    const anchor = saveScrollAnchor(activePath);
    const layout = document.querySelector('.layout');
    const collapsed = layout.classList.toggle('detail-collapsed');
    state.detailOpen = !collapsed;
    document.getElementById('detail-toggle').classList.toggle('active', !collapsed);
    restoreScrollAnchor(anchor);
}

function setCardSize(size) {
    document.getElementById('content').style.setProperty('--card-size', size + 'px');
}

function toggleTagGroup(btn) {
    btn.classList.toggle('expanded');
    const items = btn.nextElementSibling;
    items.classList.toggle('open');
}

function closeDetail() {
    const activePath = state.selectedFile?.path || state.selectedDir?.path || null;
    const anchor = saveScrollAnchor(activePath);
    state.selectedFile = null;
    state.selectedDir = null;
    state.detailOpen = false;
    document.querySelector('.layout').classList.add('detail-collapsed');
    document.getElementById('detail-toggle').classList.remove('active');
    render();
    restoreScrollAnchor(anchor);
}

// Scroll anchor helpers — keep an element at the same viewport-Y across reflows
// ---------------------------------------------------------------------------

function saveScrollAnchor(path) {
    const content = document.getElementById('content');
    if (path) {
        const el = content.querySelector(`[data-path="${CSS.escape(path)}"]`);
        if (el) return { path, top: el.getBoundingClientRect().top };
    }
    // fallback: proportional position
    return { ratio: content.scrollHeight > 0 ? content.scrollTop / content.scrollHeight : 0 };
}

function restoreScrollAnchor(anchor) {
    const content = document.getElementById('content');
    requestAnimationFrame(() => {
        if (anchor.path) {
            const el = content.querySelector(`[data-path="${CSS.escape(anchor.path)}"]`);
            if (el) {
                content.scrollTop += el.getBoundingClientRect().top - anchor.top;
                return;
            }
        }
        content.scrollTop = anchor.ratio * content.scrollHeight;
    });
}

// ---------------------------------------------------------------------------
// Escape HTML
// ---------------------------------------------------------------------------

function esc(s) {
    if (!s) return '';
    return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
            .replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}

// ---------------------------------------------------------------------------
// Event binding
// ---------------------------------------------------------------------------

document.addEventListener('DOMContentLoaded', async () => {
    // Search
    document.getElementById('search-btn').addEventListener('click', doSearch);
    document.getElementById('search-input').addEventListener('keydown', e => {
        if (e.key === 'Enter') doSearch();
    });
    document.getElementById('search-clear').addEventListener('click', doClearSearch);

    // View toggle
    document.getElementById('view-grid').addEventListener('click', () => setViewMode('grid'));
    document.getElementById('view-list').addEventListener('click', () => setViewMode('list'));

    // Zoom slider
    const zoom = document.getElementById('zoom-slider');
    zoom.addEventListener('input', () => setCardSize(zoom.value));
    setCardSize(zoom.value);

    // Close detail panel when clicking empty space in the content area
    document.getElementById('content').addEventListener('click', e => {
        if (e.target === e.currentTarget) closeDetail();
    });

    // Keyboard shortcuts
    document.addEventListener('keydown', e => {
        if (e.key === 'Escape') {
            if (state.selectedPaths.size > 1) { clearSelection(); }
            else if (state.selectedFile) { closeDetail(); }
            else if (state.mode === 'search') { doClearSearch(); }
        }
    });

    // Initial load
    await Promise.all([loadInfo(), loadTags(), loadFiles('')]);
    render();
});
