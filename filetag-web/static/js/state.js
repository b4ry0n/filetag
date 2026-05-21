const state = {
    mode: 'browse', // browse | search | zip
    currentPath: '',
    currentRootId: null,     // numeric root ID of the active database root (index from /api/roots)
    currentBasePath: null,   // derived: absolute path of the active root (set from roots array, never constructed)
    roots: [],               // [{id, name, path, entry_point}] loaded from /api/roots
    viewMode: 'grid',
    showHidden: false,
    tags: [],
    subjects: [],
    entries: [],
    searchQuery: '',
    searchResults: [],
    zipPath: null,         // path to the currently browsed zip archive
    zipRootId: null,       // numeric root_id owning the active zip archive
    zipSubdir: '',         // current sub-path within the archive (e.g. "chapter1/")
    zipEntries: [],        // [{name, size, is_image, image_index, tag_count}]
    selectedFile: null,  // { path, size, file_id, mtime, indexed_at, tags } | null
    selectedDir: null,   // { path, name, file_count } | null
    selectedRoot: null,  // root path (string) when a root card is selected | null
    selectedRootInfo: null, // ApiInfo fetched for the selected root | null
    selectedPaths: new Set(), // multi-select: Set of paths
    selectedFilesData: new Map(), // path → file detail (for tag aggregation)
    searchResultRoots: new Map(), // search mode: path → {root_id, root_path}
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
    activeSubjects: new Set(), // sidebar multi-subject filter: set of selected subject names
    activePeople: new Set(),   // sidebar multi-people filter: set of selected person names
    kvValueCache: {},          // tagName → [{value, count}] loaded lazily for k/v tags
    tagPickerMode: false,      // true while the multi-tag picker is active
    tagMultiSelectMode: (() => { try { return localStorage.getItem('ft-tag-multiselect') === '1'; } catch (_) { return false; } })(), // sticky multi-select toggle
    tagPickerPicks: new Set(), // tags checked in picker mode (to be applied)
    tagPickerOriginal: new Set(), // tags the file(s) already had when picker opened
    tagPickerSubject: null,          // subject selected in picker mode (null = none, string = selected)
    tagPickerOriginalSubject: null,  // subject on the file(s) when picker was opened (for delta detection)
    aiAnalysing: new Set(),    // paths currently being analysed by AI
    aiVideoFrames: 12,         // preferred frame count for single-video AI analysis
    aiVideoFramesAuto: false,  // true => let backend choose frame count by duration
    settings: { sprite_min: 8, sprite_max: 16, feature_video: false, feature_imagemagick: false, feature_pdf: false, dir_preview_style: 'crop', tile_preview_mode: 'sprite', vtile_duration: 8 }, // per-root settings (loaded from DB)
    sectionVisibility: _loadSectionVisibility(), // { tags, subjects, people, ai, distribution }
    sectionOrder: _loadSectionOrder(),            // ['tags','subjects','people','ai','distribution']
    sectionHeights: _loadSectionHeights(),        // { tags: px, subjects: px, ... } or null = auto
    // Sidebar tab / split state
    sidebarTab: (() => { try { return localStorage.getItem('ft-sidebar-tab') || 'tags'; } catch (_) { return 'tags'; } })(),
    sidebarSplit: (() => { try { return localStorage.getItem('ft-sidebar-split') === '1'; } catch (_) { return false; } })(),
    // File tree
    ftreeExpanded: {},  // absPath → boolean (true = open)
    ftreeCache: {},     // absPath → ApiDirEntry[]
    ftreeFilter: '',    // text filter for tree node names
    selectionLoading: false, // true while _loadMissingFilesData is in progress
    jobs: [],                // background job snapshots polled from /api/jobs
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
let _selectAllToken = null;  // cancel token: set to a new object each time _selectAllFiles starts

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

async function apiPost(url, body, opts = {}) {
    const res = await fetch(url, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
        signal: opts.signal,
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
// For internal use only — never sent directly to the API.
function currentAbsDir() {
    if (state.currentBasePath == null) return null;
    if (!state.currentPath) return state.currentBasePath;
    return state.currentBasePath + '/' + state.currentPath;
}

// Append root_id query param for the active database root.
// The backend resolves all context from the numeric root ID.
function dirParam(sep) {
    return state.currentRootId != null ? sep + 'root_id=' + state.currentRootId : '';
}

// ---------------------------------------------------------------------------
// Toast notifications
// ---------------------------------------------------------------------------

function showToast(msg, duration = 3000) {
    const container = document.getElementById('toast-container');
    const el = document.createElement('div');
    el.className = 'toast';
    if (/<[a-z][\s\S]*>/i.test(msg)) {
        el.innerHTML = msg;
    } else {
        el.textContent = msg;
    }
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
        state.settings = { sprite_min: 8, sprite_max: 16, feature_video: false, feature_imagemagick: false, feature_pdf: false, dir_preview_style: 'crop', tile_preview_mode: 'sprite', vtile_duration: 8 };
    }
    if (typeof updatePregenBtn === 'function') updatePregenBtn();
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
        api('/api/subjects'),
    ]);
}

async function loadSubjects() {
    state.subjects = await api('/api/subjects');
}

async function loadFiles(path) {
    // Build URL using root_id + root-relative path.
    // The backend resolves the absolute directory; the frontend never constructs system paths.
    const params = new URLSearchParams();
    if (state.currentRootId != null) {
        params.set('root_id', state.currentRootId);
        if (path) params.set('path', path);
    }
    if (state.showHidden) params.set('show_hidden', 'true');
    const url = '/api/files?' + params.toString();
    const data = await api(url);
    // Update root context from server response (server is authoritative).
    if (data.root_id != null) {
        state.currentRootId = data.root_id;
        // Derive currentBasePath from roots array (not from path concatenation).
        const root = state.roots.find(r => r.id === data.root_id);
        state.currentBasePath = root ? root.path : (data.root_path || null);
    }
    state.currentPath = data.path;
    state.entries = data.entries;
    state.mode = 'browse';
    state.searchQuery = '';
    state.zipPath = null;
    state.zipRootId = null;
    state.zipEntries = [];
    sessionStorage.setItem('ft_path', state.currentPath);
    sessionStorage.setItem('ft_base_id', state.currentRootId ?? '');
}

async function searchFiles(query) {
    // Queries starting with "filename:" (or shorthand "file:") search the
    // filesystem by filename/glob, independent of DB indexing.
    const isNameSearch = query.startsWith('filename:') || query.startsWith('file:');
    const prefixLen = query.startsWith('filename:') ? 'filename:'.length : 'file:'.length;
    const apiQuery = isNameSearch ? query.slice(prefixLen).trim() : query;
    const endpoint = isNameSearch ? '/api/fs-search' : '/api/search';
    try {
        const data = await api(endpoint + '?q=' + encodeURIComponent(apiQuery) + dirParam('&'));
        state.searchQuery = query;
        state.searchResults = data.results;
        state.searchResultRoots = new Map(
            (data.results || [])
                .filter(r => r.root_id != null)
                .map(r => [r.path, { root_id: r.root_id, root_path: r.root_path }])
        );
        state.mode = 'search';
        state.selectedFile = null;
    } catch (e) {
        state.searchQuery = query;
        state.searchResults = [];
        state.searchResultRoots = new Map();
        state.mode = 'search';
        state.selectedFile = null;
    }
    state.selectedPaths.clear();
    state.selectedFilesData.clear();
    _lastClickedPath = null;
    _armedBulkTag = null;
    _kbCursor = -1;
}

async function loadFileDetail(path, rootId) {
    const effectiveId = rootId ?? searchRootIdForPath(path);
    const rootParam = effectiveId != null ? '&root_id=' + effectiveId : '';
    state.selectedFile = await api('/api/file?path=' + encodeURIComponent(path) + rootParam);
    state.selectedDir = null;
    // Keep selectedFilesData in sync so multi-select tag aggregation stays fresh.
    state.selectedFilesData.set(path, state.selectedFile);
    // If the file is in the DB but missing from disk, attempt a per-file repair
    // transparently in the background.  The panel renders immediately with the
    // existing tags; a toast confirms if re-linking succeeds.
    if (state.selectedFile.missing) {
        _tryAutoRepairFile(path, effectiveId);
    }
}

// Background per-file repair.  Called when api_file_detail returns missing:true.
// Scans the root tree for the moved/renamed file and updates the DB path.
// If successful, reloads the detail panel and shows a toast.
async function _tryAutoRepairFile(path, rootId) {
    try {
        const body = rootId != null ? { path, root_id: rootId } : { path };
        const repair = await apiPost('/api/db/repair/file', body);
        if (!repair.found || !repair.new_path) return;
        // Guard: abort if the user has navigated away.
        if (!state.selectedPaths.has(path) && state.selectedFile?.path !== path) return;
        showToast(t('detail.repair-auto', { path: repair.new_path }), 6000);
        // Re-fetch detail for the new path.
        const effectiveId = rootId ?? searchRootIdForPath(path);
        const rootParam = effectiveId != null ? '&root_id=' + effectiveId : '';
        state.selectedFile = await api(
            '/api/file?path=' + encodeURIComponent(repair.new_path) + rootParam
        );
        state.selectedDir = null;
        // Update selection map from old path to new path.
        if (state.selectedPaths.has(path)) {
            state.selectedPaths.delete(path);
            state.selectedFilesData.delete(path);
            state.selectedPaths.add(repair.new_path);
        }
        if (state.selectedFilesData.has(path)) {
            state.selectedFilesData.delete(path);
            state.selectedFilesData.set(repair.new_path, state.selectedFile);
        }
        renderDetail();
        _thumbClearCache();
        if (typeof refreshCurrentDir === 'function') refreshCurrentDir();
    } catch (_) { /* silent — repair failure is non-fatal */ }
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
    await apiPost('/api/tag', { path, tags: [tagStr], root_id: state.currentRootId });
    const detail = await api('/api/file?path=' + encodeURIComponent(path) + dirParam('&'));
    if (state.selectedDir && state.selectedDir.path === path) {
        state.selectedDir.tags = detail.tags || [];
    }
    await loadTags();
    if (state.mode === 'browse') await loadFiles(state.currentPath);
    ftEmit('ft:file-tags', { paths: [path] });
}

async function removeTagFromDir(path, tagStr) {
    await apiPost('/api/untag', { path, tags: [tagStr], root_id: state.currentRootId });
    const detail = await api('/api/file?path=' + encodeURIComponent(path) + dirParam('&'));
    if (state.selectedDir && state.selectedDir.path === path) {
        state.selectedDir.tags = detail.tags || [];
    }
    await loadTags();
    if (state.mode === 'browse') await loadFiles(state.currentPath);
    ftEmit('ft:file-tags', { paths: [path] });
}

// Timer to distinguish single click (select) from double click (navigate) on directories/roots.
let _dirClickTimer = null;
let _rootClickTimer = null;
let _zipClickTimer = null;

function handleRootClick(rootPath) {
    if (_rootClickTimer) {
        clearTimeout(_rootClickTimer);
        _rootClickTimer = null;
        enterRoot(rootPath); // double click
    } else {
        _rootClickTimer = setTimeout(() => {
            _rootClickTimer = null;
            selectRoot(rootPath); // single click
        }, 250);
    }
}

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
        // event.currentTarget is still valid on double-click (same synchronous handler).
        const rootId = event?.currentTarget?.dataset?.rootId != null
            ? parseInt(event.currentTarget.dataset.rootId)
            : state.currentRootId;
        openZipDir(path, rootId); // double click
    } else {
        _zipClickTimer = setTimeout(() => {
            _zipClickTimer = null;
            selectFile(path, event); // single click
        }, 250);
    }
}

async function addTagToFile(path, tagStr, subject) {
    const body = { path, tags: [tagStr], root_id: searchRootIdForPath(path) };
    if (subject) body.subject = subject;
    await apiPost('/api/tag', body);
    await loadFileDetail(path);
    await loadTags();
    if (state.mode === 'browse') await loadFiles(state.currentPath);
    if (state.mode === 'zip')    await refreshZipEntries();
    ftEmit('ft:file-tags', { paths: [path] });
}

async function removeTagFromFile(path, tagStr, subject) {
    const body = { path, tags: [tagStr], root_id: searchRootIdForPath(path) };
    if (subject) body.subject = subject;
    await apiPost('/api/untag', body);
    await loadFileDetail(path);
    await loadTags();
    if (state.mode === 'browse') await loadFiles(state.currentPath);
    if (state.mode === 'zip')    await refreshZipEntries();
    ftEmit('ft:file-tags', { paths: [path] });
}

// ---------------------------------------------------------------------------
// Full path helper
// ---------------------------------------------------------------------------

function fullPath(entry) {
    if (state.mode === 'search') return entry.path;
    if (state.currentPath) return state.currentPath + '/' + entry.name;
    return entry.name;
}

/**
 * Returns the absolute DB-root directory to use as `dir` for legacy API calls
 * that still accept absolute paths.  In search mode the path may belong to any
 * loaded root; `state.searchResultRoots` maps each result path to its entry.
 * Outside search mode (or when no mapping exists) falls back to the currently
 * browsed directory.
 */
function searchDirForPath(path) {
    const entry = state.mode === 'search' ? state.searchResultRoots.get(path) : null;
    return (entry?.root_path) || currentAbsDir();
}

/**
 * Returns the numeric root ID for API calls that operate on `path`.
 * In search mode, looks up the root from `state.searchResultRoots`.
 * Outside search mode (or when no mapping exists) falls back to `state.currentRootId`.
 */
function searchRootIdForPath(path) {
    const entry = state.mode === 'search' ? state.searchResultRoots.get(path) : null;
    if (entry != null) return entry.root_id;
    return state.currentRootId;
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
