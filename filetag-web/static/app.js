// ---------------------------------------------------------------------------
// Icons (inline SVG)
// ---------------------------------------------------------------------------

const ICONS = {
    folder: '<svg viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="0.5" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z"/></svg>',
    file: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>',
    image: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="8.5" cy="8.5" r="1.5"/><polyline points="21 15 16 10 5 21"/></svg>',
    audio: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>',
    video: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="23 7 16 12 23 17 23 7"/><rect x="1" y="5" width="15" height="14" rx="2"/></svg>',
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
    tags: [],
    entries: [],
    searchQuery: '',
    searchResults: [],
    selectedFile: null,  // { path, size, blake3, mtime, indexed_at, tags } | null
    selectedDir: null,   // { path, name, file_count } | null
    info: null,
};

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
    const data = await api('/api/files?path=' + encodeURIComponent(path));
    state.currentPath = data.path;
    state.entries = data.entries;
    state.mode = 'browse';
    state.searchQuery = '';
    state.selectedFile = null;
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
}

async function loadFileDetail(path) {
    state.selectedFile = await api('/api/file?path=' + encodeURIComponent(path));
    state.selectedDir = null;
}

function selectDir(path, name, fileCount) {
    state.selectedDir = { path, name, file_count: fileCount };
    state.selectedFile = null;
    render();
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
            groups[prefix].push({ suffix, fullName: tag.name, count: tag.count });
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
            html += `<button class="tag-item${active}" onclick="doTagSearch('${esc(item.fullName)}')">
                ${esc(item.suffix)} <span class="count">${item.count}</span>
            </button>`;
        }
        html += '</div></div>';
    }

    // Standalone tags
    for (const tag of standalone.sort((a, b) => a.name.localeCompare(b.name))) {
        const q = quoteTag(tag.name);
        const active = state.mode === 'search' && state.searchQuery === q ? ' active' : '';
        html += `<button class="tag-item tag-standalone${active}" onclick="doTagSearch('${esc(tag.name)}')">
            ${esc(tag.name)} <span class="count">${tag.count}</span>
        </button>`;
    }

    el.innerHTML = html;
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
            html += `<span class="breadcrumb-sep">/</span>`;
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
            html += `<div class="card folder${dirSelected}" onclick="selectDir('${esc(dirPath)}','${esc(name)}',${entry.file_count})" ondblclick="navigateTo('${esc(dirPath)}')">
                <div class="card-preview">${preview}</div>
                <div class="card-body"><div class="card-name">${esc(name)}</div><div class="card-meta">${meta}</div></div>
            </div>`;
        } else {
            html += `<div class="card${selected}" onclick="selectFile('${esc(path)}')" ondblclick="selectFile('${esc(path)}')">
                <div class="card-preview">${preview}</div>
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
            html += `<div class="list-row folder${dirSelected}" onclick="selectDir('${esc(dirPath)}','${esc(name)}',${entry.file_count})" ondblclick="navigateTo('${esc(dirPath)}')">
                <span class="icon">${icon}</span>
                <span class="name">${esc(name)}</span>
                <span class="size">${size}</span>
                <span class="date">${date}</span>
                <span class="tags-count">${tags}</span>
            </div>`;
        } else {
            html += `<div class="list-row${selected}" onclick="selectFile('${esc(path)}')">
                <span class="icon">${icon}</span>
                <span class="name">${esc(name)}</span>
                <span class="size">${size}</span>
                <span class="date">${date}</span>
                <span class="tags-count">${tags}</span>
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

function renderDetail() {\n    const panel = document.getElementById('detail');

    if (!state.selectedFile && !state.selectedDir) {
        panel.hidden = true;
        return;
    }

    panel.hidden = false;

    // Directory selected
    if (state.selectedDir) {
        const d = state.selectedDir;
        document.getElementById('detail-name').textContent = d.name;
        document.getElementById('detail-preview').innerHTML =
            `<div class="no-preview" style="color:#fab005;font-size:64px">${ICONS.folder}</div>`;
        document.getElementById('detail-meta').innerHTML = `
            <div class="detail-meta-row"><span class="detail-meta-label">Path</span><span class="detail-meta-value">${esc(d.path)}</span></div>
            <div class="detail-meta-row"><span class="detail-meta-label">Items</span><span class="detail-meta-value">${d.file_count}</span></div>
        `;
        document.querySelector('.detail-tags-section').hidden = true;
        return;
    }

    document.querySelector('.detail-tags-section').hidden = false;
    const f = state.selectedFile;
    const name = f.path.split('/').pop();
    const type_ = fileType(name);

    document.getElementById('detail-name').textContent = name;

    // Preview
    const previewEl = document.getElementById('detail-preview');
    if (type_ === 'image') {
        previewEl.innerHTML = `<img src="/preview/${encodeURI(f.path)}" alt="${esc(name)}">`;
    } else if (type_ === 'audio') {
        previewEl.innerHTML = `<audio controls preload="metadata" src="/preview/${encodeURI(f.path)}"></audio>`;
    } else if (type_ === 'video') {
        previewEl.innerHTML = `<video controls preload="metadata" src="/preview/${encodeURI(f.path)}"></video>`;
    } else {
        previewEl.innerHTML = `<div class="no-preview">${fileIcon(name)}</div>`;
    }

    // Metadata
    document.getElementById('detail-meta').innerHTML = `
        <div class="detail-meta-row"><span class="detail-meta-label">Path</span><span class="detail-meta-value">${esc(f.path)}</span></div>
        <div class="detail-meta-row"><span class="detail-meta-label">Size</span><span class="detail-meta-value">${formatSize(f.size)}</span></div>
        <div class="detail-meta-row"><span class="detail-meta-label">BLAKE3</span><span class="detail-meta-value">${f.blake3 || '(not hashed)'}</span></div>
        ${f.indexed_at ? `<div class="detail-meta-row"><span class="detail-meta-label">Indexed</span><span class="detail-meta-value">${esc(f.indexed_at)}</span></div>` : ''}
    `;

    // Tags
    const tagsEl = document.getElementById('detail-tags');
    if (f.tags.length === 0) {
        tagsEl.innerHTML = '<span class="no-tags">No tags assigned</span>';
    } else {
        tagsEl.innerHTML = f.tags.map(t => {
            const tagStr = formatTag(t);
            return `<span class="tag-chip">${esc(tagStr)}<button class="remove" onclick="doRemoveTag('${esc(f.path)}','${esc(tagStr)}')">&times;</button></span>`;
        }).join('');
    }
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

async function selectFile(path) {
    await loadFileDetail(path);
    render();
}

async function doAddTag() {
    if (!state.selectedFile) return;
    const input = document.getElementById('tag-input');
    const tagStr = input.value.trim();
    if (!tagStr) return;
    await addTagToFile(state.selectedFile.path, tagStr);
    input.value = '';
    render();
}

async function doRemoveTag(path, tagStr) {
    await removeTagFromFile(path, tagStr);
    render();
}

function setViewMode(mode) {
    state.viewMode = mode;
    document.getElementById('view-grid').classList.toggle('active', mode === 'grid');
    document.getElementById('view-list').classList.toggle('active', mode === 'list');
    document.getElementById('zoom-slider').style.display = mode === 'grid' ? '' : 'none';
    renderContent();
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
    state.selectedFile = null;
    state.selectedDir = null;
    render();
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

    // Detail panel
    document.getElementById('detail-close').addEventListener('click', closeDetail);

    // Tag add
    document.getElementById('tag-add-btn').addEventListener('click', doAddTag);
    document.getElementById('tag-input').addEventListener('keydown', e => {
        if (e.key === 'Enter') doAddTag();
    });

    // Keyboard shortcuts
    document.addEventListener('keydown', e => {
        if (e.key === 'Escape') {
            if (state.selectedFile) { closeDetail(); }
            else if (state.mode === 'search') { doClearSearch(); }
        }
    });

    // Initial load
    await Promise.all([loadInfo(), loadTags(), loadFiles('')]);
    render();
});
