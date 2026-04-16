const state = {
    mode: 'browse', // browse | search | zip
    currentPath: '',
    currentRootId: null, // null = virtual root (multi-root), number = index into state.roots
    roots: [],           // [{id, name, path}] loaded from /api/roots
    viewMode: 'grid',
    showHidden: false,
    tags: [],
    entries: [],
    searchQuery: '',
    searchResults: [],
    zipPath: null,         // path to the currently browsed zip archive
    zipEntries: [],        // [{name, size, is_image, image_index, tag_count}]
    selectedFile: null,  // { path, size, file_id, mtime, indexed_at, tags } | null
    selectedDir: null,   // { path, name, file_count } | null
    selectedRoot: null,  // root id (number) when a root card is selected | null
    selectedRootInfo: null, // ApiInfo fetched for the selected root | null
    selectedPaths: new Set(), // multi-select: Set of paths
    selectedFilesData: new Map(), // path → file detail (for tag aggregation)
    info: null,
    detailOpen: true,
    expandedGroups: new Set(), // tag group prefixes that are expanded
    activeTags: new Set(),     // sidebar multi-tag filter: set of selected tag names
    aiAnalysing: new Set(),    // paths currently being analysed by AI
};

let _lastClickedPath = null; // for shift-range selection
let _armedBulkTag = null;    // two-step delete: which tag is armed
let _kbCursor = -1;          // keyboard navigation cursor (-1 = none)

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

// Helper: append root query param when a root is selected.
function rootParam(sep) {
    return state.currentRootId != null ? `${sep}root=${state.currentRootId}` : '';
}

// ---------------------------------------------------------------------------
// Toast notifications
// ---------------------------------------------------------------------------

function showToast(msg, duration = 3000) {
    const container = document.getElementById('toast-container');
    const el = document.createElement('div');
    el.className = 'toast';
    el.textContent = msg;
    container.appendChild(el);
    if (duration > 0) {
        setTimeout(() => dismissToast(el), duration);
    }
    return el;
}

function updateToast(el, msg) {
    if (el && el.isConnected) el.textContent = msg;
}

function dismissToast(el) {
    if (!el || !el.isConnected) return;
    el.classList.add('toast-out');
    el.addEventListener('animationend', () => el.remove(), { once: true });
}

async function loadRoots() {
    state.roots = await api('/api/roots');
    // Single entry-point: enter it automatically so the UI is transparent.
    const entryPoints = state.roots.filter(r => r.entry_point);
    if (entryPoints.length === 1) {
        state.currentRootId = entryPoints[0].id;
    }
}

async function loadInfo() {
    state.info = await api('/api/info' + rootParam('?'));
}

async function loadTags() {
    state.tags = await api('/api/tags' + rootParam('?'));
}

async function loadFiles(path) {
    const url = '/api/files?path=' + encodeURIComponent(path) +
        (state.showHidden ? '&show_hidden=true' : '') +
        rootParam('&');
    const data = await api(url);
    state.currentPath = data.path;
    state.entries = data.entries;
    state.mode = 'browse';
    state.searchQuery = '';
    state.zipPath = null;
    state.zipEntries = [];
    sessionStorage.setItem('ft_path', state.currentPath);
    sessionStorage.setItem('ft_root', state.currentRootId != null ? String(state.currentRootId) : '');
}

async function searchFiles(query) {
    try {
        const data = await api('/api/search?q=' + encodeURIComponent(query) + rootParam('&'));
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
    _kbCursor = -1;
}

async function loadFileDetail(path) {
    state.selectedFile = await api('/api/file?path=' + encodeURIComponent(path) + rootParam('&'));
    state.selectedDir = null;
}

function selectDir(path, name, fileCount) {
    const anchor = saveScrollAnchor(path);
    state.selectedDir = { path, name, file_count: fileCount };
    state.selectedFile = null;
    state.selectedPaths.clear();
    state.selectedFilesData.clear();
    _lastClickedPath = null;
    if (!state.detailOpen) {
        state.detailOpen = true;
        document.querySelector('.layout').classList.remove('detail-collapsed');
        document.getElementById('detail-toggle').classList.add('active');
    }
    _updateCardSelection();
    renderDetail();
    restoreScrollAnchor(anchor);
}

// Timer to distinguish single click (select) from double click (navigate) on directories.
let _dirClickTimer = null;
let _zipClickTimer = null;

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

function handleZipClick(path, event) {
    if (_zipClickTimer) {
        clearTimeout(_zipClickTimer);
        _zipClickTimer = null;
        openZipDir(path); // double click
    } else {
        _zipClickTimer = setTimeout(() => {
            _zipClickTimer = null;
            selectFile(path, event); // single click
        }, 250);
    }
}

async function addTagToFile(path, tagStr) {
    await apiPost('/api/tag', { path, tags: [tagStr], root_id: state.currentRootId });
    await loadFileDetail(path);
    await loadTags();
    if (state.mode === 'browse') await loadFiles(state.currentPath);
    if (state.mode === 'zip')    await refreshZipEntries();
}

async function removeTagFromFile(path, tagStr) {
    await apiPost('/api/untag', { path, tags: [tagStr], root_id: state.currentRootId });
    await loadFileDetail(path);
    await loadTags();
    if (state.mode === 'browse') await loadFiles(state.currentPath);
    if (state.mode === 'zip')    await refreshZipEntries();
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
