// ---------------------------------------------------------------------------
// Keyboard navigation (file browser)
// ---------------------------------------------------------------------------

/** All navigable items in the content area, in DOM order. */
function _kbItems() {
    return [...document.querySelectorAll('#content [data-path], #content [data-root-path], #content [data-zip-folder]')];
}

/** Move the keyboard cursor to idx and apply the visual indicator. */
function _kbSetCursor(idx, scroll = true) {
    const items = _kbItems();
    if (!items.length) { _kbCursor = -1; return; }
    _kbCursor = Math.max(0, Math.min(idx, items.length - 1));
    items.forEach((el, i) => el.classList.toggle('kb-focus', i === _kbCursor));
    if (scroll) items[_kbCursor].scrollIntoView({ block: 'nearest', behavior: 'smooth' });
}

/** Remove the visual cursor without changing selection. */
function _kbClearCursor() {
    _kbCursor = -1;
    document.querySelectorAll('#content .kb-focus').forEach(el => el.classList.remove('kb-focus'));
}

/**
 * Re-apply the kb-focus class after a DOM re-render.
 * Called at the end of render() and renderContent() invocations.
 */
function _kbRestoreFocus() {
    const items = _kbItems();
    if (_kbCursor < 0 || !items.length) {
        document.querySelectorAll('#content .kb-focus').forEach(el => el.classList.remove('kb-focus'));
        return;
    }
    if (_kbCursor >= items.length) _kbCursor = items.length - 1;
    items.forEach((el, i) => el.classList.toggle('kb-focus', i === _kbCursor));
}

/** Activate the item under the cursor (navigate into dir; select file). */
function _kbActivate() {
    const items = _kbItems();
    if (_kbCursor < 0 || _kbCursor >= items.length) return;
    const el = items[_kbCursor];
    if (el.dataset.zipFolder !== undefined) {
        // Navigate into a sub-folder within the current archive.
        enterZipSubdir(state.zipSubdir + el.dataset.zipFolder + '/');
    } else if (el.dataset.rootPath != null) {
        enterRoot(el.dataset.rootPath);
    } else if (el.classList.contains('folder')) {
        navigateTo(el.dataset.path);
    } else if (el.dataset.path) {
        const path = el.dataset.path;
        const name = path.split('/').pop();
        if (fileType(name) === 'zip') {
            const rootId = el.dataset.rootId != null ? parseInt(el.dataset.rootId) : null;
            openZipDir(path, rootId ?? state.currentRootId);
        } else {
            el.click(); // selectFile(path, event)
        }
    }
}

/** Navigate to the parent directory (or exit search/zip mode). */
async function _kbGoUp() {
    if (state.mode === 'search') { doClearSearch(); return; }
    if (state.mode === 'zip') {
        if (state.zipSubdir) {
            // Go to parent folder within the archive.
            const parts = state.zipSubdir.replace(/\/$/, '').split('/');
            parts.pop();
            enterZipSubdir(parts.length > 0 ? parts.join('/') + '/' : '');
            return;
        }
        if (state.zipPath) {
            const parts = state.zipPath.split('/');
            await navigateTo(parts.length > 1 ? parts.slice(0, -1).join('/') : '');
        }
        return;
    }
    if (!state.currentPath) {
        if (state.currentBasePath != null) await goVirtualRoot();
        return;
    }
    const parts = state.currentPath.split('/');
    await navigateTo(parts.length > 1 ? parts.slice(0, -1).join('/') : '');
}

/**
 * Move the cursor spatially in grid mode, or sequentially in list mode.
 * dir: 'up' | 'down' | 'left' | 'right'
 *
 * Grid:  up/down find the card with the closest horizontal centre in the
 *        nearest row above/below. Left/right step one position in DOM order.
 * List:  up/down step sequentially; left = go to parent; right = activate.
 */
function _kbMove(dir) {
    const items = _kbItems();
    if (!items.length) return;

    // No cursor yet: jump to first or last item.
    if (_kbCursor < 0) {
        _kbSetCursor(dir === 'up' || dir === 'left' ? items.length - 1 : 0);
        return;
    }

    const isGrid = document.getElementById('content').classList.contains('file-grid');

    if (!isGrid) {
        // List mode: sequential navigation.
        if      (dir === 'down')  _kbSetCursor(_kbCursor + 1);
        else if (dir === 'up')    _kbSetCursor(_kbCursor - 1);
        else if (dir === 'left')  _kbGoUp();
        else if (dir === 'right') _kbActivate();
        return;
    }

    // Grid mode: spatial navigation.
    if (dir === 'left') {
        if (_kbCursor > 0) _kbSetCursor(_kbCursor - 1);
        return;
    }
    if (dir === 'right') {
        if (_kbCursor + 1 < items.length) _kbSetCursor(_kbCursor + 1);
        return;
    }

    const curRect = items[_kbCursor].getBoundingClientRect();
    const curCX   = curRect.left + curRect.width  / 2;
    const curCY   = curRect.top  + curRect.height / 2;

    // Find the edge of the nearest row in the desired direction.
    // For 'down': look at r.top values that are greater than curCY (strictly below centre).
    // For 'up':   look at r.bottom values that are less than curCY (strictly above centre).
    let rowEdge = dir === 'down' ? Infinity : -Infinity;
    for (let i = 0; i < items.length; i++) {
        if (i === _kbCursor) continue;
        const r = items[i].getBoundingClientRect();
        if (dir === 'down' && r.top    > curCY) rowEdge = Math.min(rowEdge, r.top);
        if (dir === 'up'   && r.bottom < curCY) rowEdge = Math.max(rowEdge, r.bottom);
    }
    if (!isFinite(rowEdge)) return; // already at the first or last row

    // Among cards in that row (within ±8 px), pick the one whose horizontal
    // centre is closest to the current card's horizontal centre.
    const tol = 8;
    let best = -1, bestHDist = Infinity;
    for (let i = 0; i < items.length; i++) {
        if (i === _kbCursor) continue;
        const r = items[i].getBoundingClientRect();
        const onRow = dir === 'down'
            ? Math.abs(r.top    - rowEdge) <= tol
            : Math.abs(r.bottom - rowEdge) <= tol;
        if (!onRow) continue;
        const hDist = Math.abs((r.left + r.width / 2) - curCX);
        if (hDist < bestHDist) { bestHDist = hDist; best = i; }
    }
    if (best >= 0) _kbSetCursor(best);
}

// ---------------------------------------------------------------------------
// Event binding
// ---------------------------------------------------------------------------

async function _selectAllFiles() {
    const allItems = state.mode === 'search' ? state.searchResults
        : state.mode === 'zip'
            ? getZipDirContents(state.zipEntries, state.zipSubdir).files
                .map(en => ({ path: state.zipPath + '::' + en.name, is_dir: false }))
            : state.entries;
    const filePaths = (allItems || [])
        .filter(en => !en.is_dir)
        .map(en => (state.mode === 'search' || state.mode === 'zip') ? en.path : fullPath(en));
    filePaths.forEach(p => state.selectedPaths.add(p));
    _armedBulkTag = null;
    state.selectedDir = null;

    // Show selection highlighting + spinner immediately (no server round-trip yet).
    state.selectionLoading = filePaths.some(p => !state.selectedFilesData.has(p));
    _updateCardSelection();
    if (state.selectionLoading) renderDetail();

    // Cancel token — if the user makes a new selection while we are loading,
    // we discard the result so we don't overwrite their selection.
    const token = {};
    _selectAllToken = token;

    await _loadMissingFilesData(filePaths);

    if (_selectAllToken !== token) return; // selection changed while loading
    _selectAllToken = null;
    state.selectionLoading = false;
    state.selectedFile = filePaths.length === 1 ? state.selectedFilesData.get(filePaths[0]) : null;
    render();
}

document.addEventListener('DOMContentLoaded', async () => {
    // Click on empty space in the content area deselects everything (Finder-style).
    document.getElementById('content').addEventListener('click', e => {
        if (e.target === e.currentTarget) clearSelection();
    });

    // Search
    document.getElementById('search-btn').addEventListener('click', doSearch);
    document.getElementById('search-clear').addEventListener('click', doClearSearch);
    attachSearchAutocomplete(document.getElementById('search-input'), doSearch);

    // View toggle
    document.getElementById('view-grid').addEventListener('click', () => setViewMode('grid'));
    document.getElementById('view-list').addEventListener('click', () => setViewMode('list'));

    // Zoom slider
    const zoom = document.getElementById('zoom-slider');
    zoom.addEventListener('input', () => setCardSize(zoom.value));
    setCardSize(zoom.value);



    // Keyboard shortcuts (file browser navigation + Escape handling)
    document.addEventListener('keydown', e => {
        // Escape is handled regardless of focus state
        if (e.key === 'Escape') {
            if (state.tagPickerMode)           { cancelTagPickerMode(); return; }
            if (state.selectedPaths.size > 1)  { clearSelection(); return; }
            // The detail pane is no longer closed via Escape.
            if (state.mode === 'search')       { doClearSearch(); return; }
            if (_kbCursor >= 0)                { _kbClearCursor(); return; }
            return;
        }

        // Skip navigation when a modal overlay is open
        if (!document.getElementById('media-viewer').hidden) return;
        if (!document.getElementById('lightbox').hidden) return;

        // Skip when an input element has focus
        const active = document.activeElement;
        if (active && (active.tagName === 'INPUT' || active.tagName === 'TEXTAREA' || active.isContentEditable)) return;

        // Cmd+A / Ctrl+A: select all files on the current page.
        if ((e.metaKey || e.ctrlKey) && e.key === 'a') {
            e.preventDefault();
            _selectAllFiles();
            return;
        }

        // Alt+Left / Alt+Right: browser-style back/forward navigation.
        if (e.altKey && (e.key === 'ArrowLeft' || e.key === 'ArrowRight')) {
            e.preventDefault();
            if (e.key === 'ArrowLeft') navBack(); else navForward();
            return;
        }

        // Skip when the tag context menu is open
        if (document.getElementById('tag-context-menu')) return;

        const items = _kbItems();
        if (!items.length) return;

        switch (e.key) {
            // Spatial navigation: arrow keys and vi-keys h/j/k/l move visually
            // in the grid (left/down/up/right), and sequentially in list mode.
            case 'ArrowDown':
            case 'j':
                e.preventDefault();
                _kbMove('down');
                break;
            case 'ArrowUp':
            case 'k':
                e.preventDefault();
                _kbMove('up');
                break;
            case 'ArrowRight':
            case 'l':
                e.preventDefault();
                _kbMove('right');
                break;
            case 'ArrowLeft':
            case 'h':
                e.preventDefault();
                _kbMove('left');
                break;
            // Enter / Return: open the focused item.
            case 'Enter':
                if (_kbCursor >= 0) { e.preventDefault(); _kbActivate(); }
                break;
            // u / Backspace: go to parent directory.
            case 'u':
            case 'Backspace':
                e.preventDefault();
                _kbGoUp();
                break;
        }
    });

    // Initial load: restore directory from sessionStorage if present (survives Cmd-R).
    const initialPath = sessionStorage.getItem('ft_path') || '';
    // Prefer the new numeric root_id; fall back to the legacy path string.
    const savedBaseId = sessionStorage.getItem('ft_base_id');
    const savedBase = sessionStorage.getItem('ft_base'); // legacy fallback
    await loadRoots();
    // Restore the previously active root from sessionStorage, or auto-enter when
    // there is exactly one entry-point (good UX on first visit).
    const entryPoints = state.roots.filter(r => r.entry_point);
    let rootToRestore = null;
    if (savedBaseId != null) {
        rootToRestore = state.roots.find(r => r.id === parseInt(savedBaseId));
    }
    if (!rootToRestore && savedBase) {
        rootToRestore = state.roots.find(r => r.path === savedBase || savedBase.startsWith(r.path + '/'));
    }
    if (!rootToRestore && entryPoints.length === 1) {
        rootToRestore = entryPoints[0];
    }
    if (rootToRestore) {
        state.currentRootId = rootToRestore.id;
        state.currentBasePath = rootToRestore.path;
    }
    // Only load database-specific data when we already know which root to
    // target.  When currentBasePath is null (first visit with multiple roots,
    // no session) the backend requires a `dir` parameter and will return 400.
    // These calls will be retried below once loadFiles() has resolved the root.
    // Run sequentially rather than in parallel to avoid simultaneous SQLite
    // lock contention on network shares (SMB/NFS), where each concurrent open
    // causes dozens of extra round-trips due to WAL locking overhead.
    const _hadContext = state.currentRootId != null;
    await loadAuthStatus();
    if (_hadContext) {
        await loadInfo().catch(e => console.error('loadInfo failed:', e));
        await loadTags().catch(e => console.error('loadTags failed:', e));
        await loadSettings().catch(() => {});
    }
    if (_hadContext && typeof loadFaceConfig === 'function') {
        Promise.all([loadFaceConfig(), loadPeople()]).catch(() => {});
    }
    // Attempt to restore the last-visited path. If that fails (e.g. because new
    // databases changed the root index mapping, or the directory was removed),
    // fall back to the root of the selected database so the page is never blank.
    // Check whether initialPath is valid within the current roots.
    let validInitial = false;
    if (initialPath) {
        validInitial = state.roots.some(r => initialPath === '' || initialPath === r.path || initialPath.startsWith(r.path + '/'));
    }
    try {
        if (validInitial) {
            // Strip the root prefix from initialPath so that currentPath stays relative.
            let relPath = initialPath;
            const root = state.currentBasePath;
            if (root && relPath && relPath.startsWith(root + '/')) {
                relPath = relPath.slice(root.length + 1);
            } else if (root && relPath === root) {
                relPath = '';
            }
            await loadFiles(relPath);
        } else {
            sessionStorage.removeItem('ft_path');
            // Fall back to the first entry-point root when initialPath is invalid.
            await loadFiles('');
        }
    } catch (e) {
        console.error('loadFiles failed for path', JSON.stringify(initialPath), e);
        sessionStorage.removeItem('ft_path');
        try {
            await loadFiles('');
        } catch (e2) { console.error('loadFiles fallback also failed:', e2); }
    }
    // If db-specific data was not loaded before loadFiles (no context at startup),
    // load it now. /api/tags works without a dir param (merges all roots), so we
    // always load tags. /api/info and /api/settings require a dir, so only load
    // those when a root is now known.
    if (!_hadContext) {
        await loadTags().catch(e => console.error('loadTags failed:', e));
        if (currentAbsDir() != null) {
            await loadInfo().catch(e => console.error('loadInfo failed:', e));
            await loadSettings().catch(() => {});
            if (typeof loadFaceConfig === 'function') {
                Promise.all([loadFaceConfig(), loadPeople()]).catch(() => {});
            }
        }
    }
    _navPush(); // seed the history with the initial location so back-button works after first navigation
    render();

    // Initialise sidebar tabs (restores persisted tab/split state).
    if (typeof initSidebarTabs === 'function') initSidebarTabs();

    // Sync multi-select toggle button to persisted state.
    const _msBtn = document.getElementById('tag-multiselect-btn');
    if (_msBtn) _msBtn.classList.toggle('active', !!state.tagMultiSelectMode);

    // Build language selector and apply i18n translations to all data-i18n elements.
    const langSel = document.getElementById('lang-select');
    if (langSel) {
        langSel.innerHTML = LANG_OPTIONS.map(o =>
            `<option value="${o.code}"${o.code === getLang() ? ' selected' : ''}>${o.label}</option>`
        ).join('');
    }
    applyI18n();

    // Media viewer stage events (wheel, drag, pinch)
    _cvInitStageEvents();
    _cvInitFsToolbar();

    // Panel resize handles
    initResizeHandles();
    _initChatResize();

    // Restore card-labels toggle state from localStorage.
    const _savedLabels = localStorage.getItem('ft-card-labels');
    // Migrate legacy values ('0' → 'hide', '1' → 'show').
    const _labelsMode = (_savedLabels === '0' || _savedLabels === 'hide') ? 'hide'
                      : _savedLabels === 'minimal' ? 'minimal' : 'show';
    if (_labelsMode !== 'show') setCardLabels(_labelsMode);

    // ---------------------------------------------------------------------------
    // Pub/sub subscribers — all tag and file-tag mutations emit one of these two
    // events after their data loads complete. Subscribers are the single source
    // of truth for post-mutation rendering.
    //
    // 'ft:tags-meta'  — tag metadata changed (name, colour, synonyms)
    //                   → re-render sidebar and detail panel
    // 'ft:file-tags'  — file↔tag associations changed (tag / untag operations)
    //                   → re-render sidebar, detail panel, and card tag badges
    // ---------------------------------------------------------------------------
    ftOn('ft:tags-meta', () => {
        renderTags();
        renderDetailTagsSectionOnly();
    });
    ftOn('ft:file-tags', () => {
        // Invalidate k=v value cache so the next sidebar expand re-fetches fresh data.
        state.kvValueCache = {};
        // Remember which input had focus so we can restore it after the render.
        const focusedId = document.activeElement?.id || null;
        renderTags();
        renderDetailTagsSectionOnly();
        _updateCardTagBadges();
        // Restore focus — renderDetailTagsSectionOnly() only updates the tags
        // section HTML, so the input elements are replaced; re-focus by id.
        if (focusedId) {
            const el = document.getElementById(focusedId);
            if (el) el.focus();
        }
        // Re-fetch kv values for any expanded groups so the sidebar stays up to date.
        for (const key of state.expandedGroups) {
            if (!key.startsWith('\x01kv:')) continue;
            const tagName = key.slice(4);
            api('/api/tag-values?' + new URLSearchParams({ name: tagName, dir: currentAbsDir() }))
                .then(values => { state.kvValueCache[tagName] = values; renderTags(); })
                .catch(() => { state.kvValueCache[tagName] = null; });
        }
    });
});

// ---------------------------------------------------------------------------
// Panel resize (sidebar width, detail width, detail inner split)
// ---------------------------------------------------------------------------

function initResizeHandles() {
    // Restore saved sizes from localStorage.
    const sw  = localStorage.getItem('ft-sidebar-width');
    const dw  = localStorage.getItem('ft-detail-width');
    const root = document.documentElement;
    if (sw)  root.style.setProperty('--sidebar-width',    sw);
    if (dw)  root.style.setProperty('--detail-width',     dw);
    _syncChatRight();

    _colResize(
        document.getElementById('resize-sidebar'),
        () => parseFloat(getComputedStyle(root).getPropertyValue('--sidebar-width')),
        (w) => {
            root.style.setProperty('--sidebar-width', w + 'px');
            localStorage.setItem('ft-sidebar-width', w + 'px');
        },
        120, 600,
        /* leftEdge */ true   // dragging right increases width
    );

    _colResize(
        document.getElementById('resize-detail'),
        () => parseFloat(getComputedStyle(root).getPropertyValue('--detail-width')),
        (w) => {
            root.style.setProperty('--detail-width', w + 'px');
            localStorage.setItem('ft-detail-width', w + 'px');
            _syncChatRight();
        },
        180, 800,
        /* leftEdge */ false  // dragging left increases width
    );

    _initSidebarSubjectDivider();
}

/** Wire up col-resize drag on an element.
 *  leftEdge=true  → dragging right increases the panel (sidebar on its right).
 *  leftEdge=false → dragging left  increases the panel (detail on its left). */
function _colResize(el, getWidth, setWidth, minW, maxW, leftEdge) {
    if (!el) return;
    el.addEventListener('mousedown', (e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        const startX = e.clientX;
        const startW = getWidth();
        el.classList.add('dragging');
        document.body.style.cursor = 'col-resize';
        document.body.style.userSelect = 'none';

        function onMove(ev) {
            const delta = leftEdge ? ev.clientX - startX : startX - ev.clientX;
            setWidth(Math.max(minW, Math.min(maxW, startW + delta)));
        }
        function onUp() {
            el.classList.remove('dragging');
            document.body.style.cursor = '';
            document.body.style.userSelect = '';
            window.removeEventListener('mousemove', onMove);
            window.removeEventListener('mouseup', onUp);
        }
        window.addEventListener('mousemove', onMove);
        window.addEventListener('mouseup', onUp);
    });
}

/** Wire up the draggable divider between the tag pane and subject pane in the sidebar. */
function _initSidebarSubjectDivider() {
    const el      = document.getElementById('sidebar-subject-divider');
    const subjEl  = document.getElementById('subject-list');
    if (!el || !subjEl) return;

    // Restore saved height
    const saved = localStorage.getItem('ft-subject-pane-height');
    if (saved) subjEl.style.flex = `0 0 ${saved}`;

    el.addEventListener('mousedown', (e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        const startY   = e.clientY;
        const startH   = subjEl.getBoundingClientRect().height;
        el.classList.add('dragging');
        document.body.style.cursor    = 'row-resize';
        document.body.style.userSelect = 'none';

        function onMove(ev) {
            // Dragging up increases subject pane height
            const delta  = startY - ev.clientY;
            const newH   = Math.max(40, Math.min(startH + delta, window.innerHeight * 0.7));
            subjEl.style.flex = `0 0 ${newH}px`;
            localStorage.setItem('ft-subject-pane-height', `${newH}px`);
        }
        function onUp() {
            el.classList.remove('dragging');
            document.body.style.cursor    = '';
            document.body.style.userSelect = '';
            window.removeEventListener('mousemove', onMove);
            window.removeEventListener('mouseup',   onUp);
        }
        window.addEventListener('mousemove', onMove);
        window.addEventListener('mouseup',   onUp);
    });
}

/** Wire up row-resize drag for the detail-panel inner splitter.
 *
 * Layout contract:
 *   .detail         — flex column, full panel height
 *   .detail-top     — flex:none, height set HERE in px directly on the element
 *     .detail-header  — fixed ~38px
 *     .detail-preview — flex:1 1 0, min-height:0  → fills remainder of .detail-top
 *     #face-toolbar-row — shrinks to content
 *   .detail-v-handle  — 5px drag zone
 *   .detail-tags-section — flex:1 1 0, scrollable
 *
 * Because .detail-top has a definite px height (set below), .detail-preview
 * also gets a definite height via flexbox, so max-height:100% on the img
 * always resolves correctly — no CSS variables, no calc, no JS measuring.
 */
function initDetailVHandle(el) {
    if (!el) return;
    const detailTop = document.querySelector('#detail .detail-top');
    if (!detailTop) return;

    function setHeight(h) {
        detailTop.style.height = h + 'px';
    }

    function clamp(h) {
        const detail = el.closest('.detail');
        const max = detail ? detail.getBoundingClientRect().height - 60 - 6 : 800;
        return Math.max(60, Math.min(max, h));
    }

    // Natural height: momentarily clear the forced height so the browser
    // lays out to content size, measure, then restore.
    function naturalHeight() {
        const prev = detailTop.style.height;
        detailTop.style.height = '';
        const h = detailTop.getBoundingClientRect().height;
        detailTop.style.height = prev;
        return h;
    }

    function afterDrag() {
        if (typeof faceRerenderPreviewBoxes === 'function') faceRerenderPreviewBoxes();
    }

    // Restore saved position, clamped to the current panel height.
    const saved = localStorage.getItem('ft-detail-top-height');
    if (saved) {
        const maxPx = Math.min(window.innerHeight * 0.85, window.innerHeight - 120);
        let px = parseInt(saved, 10);
        if (isNaN(px) || px < 60) px = 60;
        if (px > maxPx) px = maxPx;
        setHeight(px);
    } else {
        // No saved value: snapshot the natural (content) height in px on the
        // next frame so the CSS 55vh fallback is replaced immediately.
        requestAnimationFrame(() => {
            const h = naturalHeight();
            if (h > 0) setHeight(h);
        });
    }

    // Double-click: snap back to content height.
    el.addEventListener('dblclick', () => {
        detailTop.style.height = '';
        const h = detailTop.getBoundingClientRect().height;
        if (h > 0) setHeight(h);
        localStorage.removeItem('ft-detail-top-height');
        afterDrag();
    });

    el.addEventListener('mousedown', (e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        const startY = e.clientY;
        const startH = detailTop.getBoundingClientRect().height;
        el.classList.add('dragging');
        document.body.style.cursor = 'row-resize';
        document.body.style.userSelect = 'none';

        function onMove(ev) {
            setHeight(clamp(startH + (ev.clientY - startY)));
        }
        function onUp(ev) {
            el.classList.remove('dragging');
            document.body.style.cursor = '';
            document.body.style.userSelect = '';
            const h = clamp(startH + (ev.clientY - startY));
            // Snap to natural height if close enough.
            const nat = naturalHeight();
            const final = Math.abs(h - nat) <= 20 ? nat : h;
            setHeight(final);
            localStorage.setItem('ft-detail-top-height', final + 'px');
            afterDrag();
            window.removeEventListener('mousemove', onMove);
            window.removeEventListener('mouseup', onUp);
        }
        window.addEventListener('mousemove', onMove);
        window.addEventListener('mouseup', onUp);
    });
}
