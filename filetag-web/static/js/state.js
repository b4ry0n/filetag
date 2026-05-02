const state = {
    mode: 'browse', // browse | search | zip
    currentPath: '',
    currentBasePath: null,   // absolute filesystem path of the deepest active DB root (updated by api_files)
    roots: [],               // [{id, name, path, entry_point}] loaded from /api/roots
    viewMode: 'grid',
    showHidden: false,
    tags: [],
    subjects: [],
    entries: [],
    searchQuery: '',
    searchResults: [],
    zipPath: null,         // path to the currently browsed zip archive
    zipSubdir: '',         // current sub-path within the archive (e.g. "chapter1/")
    zipEntries: [],        // [{name, size, is_image, image_index, tag_count}]
    selectedFile: null,  // { path, size, file_id, mtime, indexed_at, tags } | null
    selectedDir: null,   // { path, name, file_count } | null
    selectedRoot: null,  // root path (string) when a root card is selected | null
    selectedRootInfo: null, // ApiInfo fetched for the selected root | null
    selectedPaths: new Set(), // multi-select: Set of paths
    selectedFilesData: new Map(), // path → file detail (for tag aggregation)
    info: null,
    detailOpen: true,
    expandedGroups: new Set([
        // Sections collapsed by default; only Tags starts open.
        '\x01section:subjects:hide',
        '\x01section:people:hide',
        '\x01section:distribution:hide',
        // '\x01section:ai' is absent → AI section also starts collapsed (needs key to open).
    ]), // tag group full paths that are expanded
    tagSortMode: 'groups-first', // 'groups-first' | 'alpha' | 'count'
    tagFilter: '',             // sidebar tag search filter string
    activeTags: new Set(),     // sidebar multi-tag filter: set of selected tag names
    kvValueCache: {},          // tagName → [{value, count}] loaded lazily for k/v tags
    tagPickerMode: false,      // true while the multi-tag picker is active
    tagPickerPicks: new Set(), // tags checked in picker mode (to be applied)
    tagPickerOriginal: new Set(), // tags the file(s) already had when picker opened
    tagPickerSubject: null,          // subject selected in picker mode (null = none, string = selected)
    tagPickerOriginalSubject: null,  // subject on the file(s) when picker was opened (for delta detection)
    aiAnalysing: new Set(),    // paths currently being analysed by AI
    aiVideoFrames: 12,         // preferred frame count for single-video AI analysis
    aiVideoFramesAuto: false,  // true => let backend choose frame count by duration
    settings: { sprite_min: 8, sprite_max: 16, feature_video: false, feature_imagemagick: false, feature_pdf: false }, // per-root settings (loaded from DB)
    sectionVisibility: _loadSectionVisibility(), // { tags, subjects, people, ai, distribution }
    sectionOrder: _loadSectionOrder(),            // ['tags','subjects','people','ai','distribution']
    sectionHeights: _loadSectionHeights(),        // { tags: px, subjects: px, ... } or null = auto
};

/** Load section visibility from localStorage, with defaults all-on. */
function _loadSectionVisibility() {
    const defaults = { tags: true, subjects: true, people: true, ai: true, distribution: false };
    try {
        const saved = JSON.parse(localStorage.getItem('ft-section-visibility') || 'null');
        if (saved && typeof saved === 'object') {
            return { ...defaults, ...saved };
        }
    } catch (_) { /* ignore */ }
    return defaults;
}

function saveSectionVisibility() {
    try {
        localStorage.setItem('ft-section-visibility', JSON.stringify(state.sectionVisibility));
    } catch (_) { /* ignore */ }
}

/** Load section order from localStorage. */
function _loadSectionOrder() {
    const defaults = ['tags', 'subjects', 'people', 'ai', 'distribution'];
    try {
        const saved = JSON.parse(localStorage.getItem('ft-section-order') || 'null');
        if (Array.isArray(saved) && saved.length === defaults.length
            && defaults.every(k => saved.includes(k))) {
            return saved;
        }
    } catch (_) { /* ignore */ }
    return defaults;
}

function saveSectionOrder() {
    try {
        localStorage.setItem('ft-section-order', JSON.stringify(state.sectionOrder));
    } catch (_) { /* ignore */ }
}

/** Load saved section heights (px) from localStorage. */
function _loadSectionHeights() {
    try {
        const saved = JSON.parse(localStorage.getItem('ft-section-heights') || 'null');
        if (saved && typeof saved === 'object') return saved;
    } catch (_) { /* ignore */ }
    return {};
}

function saveSectionHeights() {
    try {
        localStorage.setItem('ft-section-heights', JSON.stringify(state.sectionHeights));
    } catch (_) { /* ignore */ }
}

let _lastClickedPath = null; // for shift-range selection
let _armedBulkTag = null;    // two-step delete: which tag is armed
let _kbCursor = -1;          // keyboard navigation cursor (-1 = none)

// ---------------------------------------------------------------------------
// API
// ---------------------------------------------------------------------------

async function api(url) {
    const res = await fetch(url);
    if (!res.ok) {
        const text = await res.text().catch(() => '');
        let body = {};
        try { body = text ? JSON.parse(text) : {}; } catch (_) { /* plain text */ }
        throw new Error(body.error || text || res.statusText);
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
        const text = await res.text().catch(() => '');
        let body = {};
        try { body = text ? JSON.parse(text) : {}; } catch (_) { /* plain text */ }
        throw new Error(body.error || text || res.statusText);
    }
    return res.json();
}

// Returns the absolute filesystem path of the currently browsed directory.
// This is what the backend uses to determine the correct (deepest) root.
function currentAbsDir() {
    if (state.currentBasePath == null) return null;
    if (!state.currentPath) return state.currentBasePath;
    // Avoid duplicate prefix: if currentPath already starts with currentBasePath, do not prepend again.
    if (state.currentPath.startsWith(state.currentBasePath + '/')) return state.currentPath;
    return state.currentBasePath + '/' + state.currentPath;
}

// Append dir query param with the current absolute directory path.
// The backend resolves the active root from this path using root_for_dir.
function dirParam(sep) {
    const d = currentAbsDir();
    return d != null ? sep + 'dir=' + encodeURIComponent(d) : '';
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
        state.currentBasePath = entryPoints[0].path;
    }
}

async function loadInfo() {
    state.info = await api('/api/info' + dirParam('?'));
}

async function loadSettings() {
    try {
        state.settings = await api('/api/settings' + dirParam('?'));
    } catch (_) {
        state.settings = { sprite_min: 8, sprite_max: 16, feature_video: false, feature_imagemagick: false, feature_pdf: false };
    }
}

async function loadAuthStatus() {
    try {
        const res = await api('/api/auth/status');
        const btn = document.getElementById('logout-btn');
        if (btn) btn.hidden = !res.auth;
    } catch (_) { /* ignore */ }
}

async function loadTags() {
    [state.tags, state.subjects] = await Promise.all([
        api('/api/tags' + dirParam('?')),
        api('/api/subjects' + dirParam('?')),
    ]);
}

async function loadSubjects() {
    state.subjects = await api('/api/subjects' + dirParam('?'));
}

async function loadFiles(path) {
    // Compute the absolute path of the target directory.
    // 'path' is relative to the current deepest root (currentBasePath).
    let absDir = null;
    if (state.currentBasePath != null) {
        if (!path) {
            absDir = state.currentBasePath;
        } else if (path.startsWith(state.currentBasePath + '/')) {
            absDir = path;
        } else {
            absDir = state.currentBasePath + '/' + path;
        }
    }
    const dirPart = absDir != null ? 'dir=' + encodeURIComponent(absDir) : null;
    const url = '/api/files?' + [dirPart, state.showHidden ? 'show_hidden=true' : null]
        .filter(Boolean).join('&');
    const data = await api(url);
    // Update the deepest active root from server response so all subsequent
    // operations (tag/untag, file detail, cache) target the correct database.
    if (data.root_path) state.currentBasePath = data.root_path;
    state.currentPath = data.path;
    state.entries = data.entries;
    state.mode = 'browse';
    state.searchQuery = '';
    state.zipPath = null;
    state.zipEntries = [];
    sessionStorage.setItem('ft_path', state.currentPath);
    sessionStorage.setItem('ft_base', state.currentBasePath || '');
}

async function searchFiles(query) {
    try {
        const data = await api('/api/search?q=' + encodeURIComponent(query) + dirParam('&'));
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
    state.selectedFile = await api('/api/file?path=' + encodeURIComponent(path) + dirParam('&'));
    state.selectedDir = null;
    // Keep selectedFilesData in sync so multi-select tag aggregation stays fresh.
    if (state.selectedFilesData.has(path)) {
        state.selectedFilesData.set(path, state.selectedFile);
    }
}

async function selectDir(path, name, fileCount) {
    const anchor = saveScrollAnchor(path);
    state.selectedDir = { path, name, file_count: fileCount, tags: null };
    state.selectedFile = null;
    state.selectedPaths.clear();
    state.selectedFilesData.clear();
    _lastClickedPath = null;
    _updateCardSelection();
    renderDetail();
    restoreScrollAnchor(anchor);
    // Load tags asynchronously so the panel appears immediately.
    try {
        const detail = await api('/api/file?path=' + encodeURIComponent(path) + dirParam('&'));
        if (state.selectedDir && state.selectedDir.path === path) {
            state.selectedDir.tags = detail.tags || [];
            renderDetail();
        }
    } catch (_) {
        if (state.selectedDir && state.selectedDir.path === path) {
            state.selectedDir.tags = [];
            renderDetail();
        }
    }
}

async function addTagToDir(path, tagStr) {
    await apiPost('/api/tag', { path, tags: [tagStr], dir: currentAbsDir() });
    const detail = await api('/api/file?path=' + encodeURIComponent(path) + dirParam('&'));
    if (state.selectedDir && state.selectedDir.path === path) {
        state.selectedDir.tags = detail.tags || [];
        renderDetail();
    }
    await loadTags();
    if (state.mode === 'browse') await loadFiles(state.currentPath);
}

async function removeTagFromDir(path, tagStr) {
    await apiPost('/api/untag', { path, tags: [tagStr], dir: currentAbsDir() });
    const detail = await api('/api/file?path=' + encodeURIComponent(path) + dirParam('&'));
    if (state.selectedDir && state.selectedDir.path === path) {
        state.selectedDir.tags = detail.tags || [];
        renderDetail();
    }
    await loadTags();
    if (state.mode === 'browse') await loadFiles(state.currentPath);
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

async function addTagToFile(path, tagStr, subject) {
    const body = { path, tags: [tagStr], dir: currentAbsDir() };
    if (subject) body.subject = subject;
    await apiPost('/api/tag', body);
    await loadFileDetail(path);
    await loadTags();
    if (state.mode === 'browse') await loadFiles(state.currentPath);
    if (state.mode === 'zip')    await refreshZipEntries();
}

async function removeTagFromFile(path, tagStr, subject) {
    const body = { path, tags: [tagStr], dir: currentAbsDir() };
    if (subject) body.subject = subject;
    await apiPost('/api/untag', body);
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
