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
            openZipDir(path);
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
    await Promise.all(filePaths.map(async p => {
        if (!state.selectedFilesData.has(p)) {
            const data = await api('/api/file?path=' + encodeURIComponent(p) + dirParam('&'));
            state.selectedFilesData.set(p, data);
        }
    }));
    _armedBulkTag = null;
    state.selectedDir = null;
    state.selectedFile = filePaths.length === 1 ? state.selectedFilesData.get(filePaths[0]) : null;
    render();
}

document.addEventListener('DOMContentLoaded', async () => {
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

    // Close detail panel when clicking empty space in the content area
    document.getElementById('content').addEventListener('click', e => {
        if (e.target === e.currentTarget) closeDetail();
    });

    // Keyboard shortcuts (file browser navigation + Escape handling)
    document.addEventListener('keydown', e => {
        // Escape is handled regardless of focus state
        if (e.key === 'Escape') {
            if (state.selectedPaths.size > 1) { clearSelection(); return; }
            if (state.selectedFile)            { closeDetail();   return; }
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
    const savedBase = sessionStorage.getItem('ft_base');
    await loadRoots();
    // Restore the previously active root from sessionStorage, or auto-enter when
    // there is exactly one entry-point (good UX on first visit).
    const entryPoints = state.roots.filter(r => r.entry_point);
    const baseToRestore = savedBase || (entryPoints.length === 1 ? entryPoints[0].path : null);
    if (baseToRestore) {
        const rootMeta = state.roots.find(r => r.path === baseToRestore || baseToRestore.startsWith(r.path + '/'));
        if (rootMeta) state.currentBasePath = baseToRestore;
    }
    try { await Promise.all([loadInfo(), loadTags(), loadSettings(), loadAuthStatus()]); } catch (e) { console.error('loadInfo/loadTags failed:', e); }
    // Attempt to restore the last-visited path. If that fails (e.g. because new
    // databases changed the root index mapping, or the directory was removed),
    // fall back to the root of the selected database so the page is never blank.
    try {
        await loadFiles(initialPath);
    } catch (e) {
        console.error('loadFiles failed for path', JSON.stringify(initialPath), e);
        sessionStorage.removeItem('ft_path');
        try {
            await loadFiles('');
        } catch (e2) { console.error('loadFiles fallback also failed:', e2); }
    }
    render();

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

    // Panel resize handles
    initResizeHandles();
    _initChatResize();
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
        },
        180, 800,
        /* leftEdge */ false  // dragging left increases width
    );
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

/** Wire up row-resize drag for the detail-panel inner splitter.
 *  Exposed globally so detail.js can call it after rendering. */
/** Wire up row-resize drag for the detail-panel inner splitter.
 *  Controls --detail-top-height so the top section (header+preview+meta)
 *  has a fixed height and the tags section fills the remaining space.
 *  On first render (no saved value) the natural content height is snapshotted
 *  so the divider starts exactly below the meta section. */
function initDetailVHandle(el) {
    if (!el) return;
    const root      = document.documentElement;
    const detailTop = document.querySelector('#detail .detail-top');
    if (!detailTop) return;

    // Measure natural (content) height by temporarily removing the forced value.
    function naturalHeight() {
        const was = root.style.getPropertyValue('--detail-top-height');
        root.style.removeProperty('--detail-top-height');
        const h = detailTop.getBoundingClientRect().height;
        if (was) root.style.setProperty('--detail-top-height', was);
        return h;
    }

    function snapOrSet(h) {
        const nat = naturalHeight();
        const snapped = Math.abs(h - nat) <= 20 ? nat : h;
        const isNatural = snapped === nat;
        if (isNatural) {
            root.style.removeProperty('--detail-top-height');
            localStorage.removeItem('ft-detail-top-height');
        } else {
            root.style.setProperty('--detail-top-height', snapped + 'px');
            localStorage.setItem('ft-detail-top-height', snapped + 'px');
        }
    }

    const saved = localStorage.getItem('ft-detail-top-height');
    if (saved) {
        root.style.setProperty('--detail-top-height', saved);
    }
    // If no saved value, leave the variable unset → height:auto → natural position.

    // Double-click: snap back to natural height.
    el.addEventListener('dblclick', () => {
        root.style.removeProperty('--detail-top-height');
        localStorage.removeItem('ft-detail-top-height');
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
            const detail = el.closest('.detail');
            const max = detail ? detail.getBoundingClientRect().height - 60 - 6 : 800;
            const h = Math.max(60, Math.min(max, startH + (ev.clientY - startY)));
            // Live snap preview within 20px of natural position.
            const nat = naturalHeight();
            const display = Math.abs(h - nat) <= 20 ? nat : h;
            if (display === nat) {
                root.style.removeProperty('--detail-top-height');
            } else {
                root.style.setProperty('--detail-top-height', display + 'px');
            }
        }
        function onUp(ev) {
            el.classList.remove('dragging');
            document.body.style.cursor = '';
            document.body.style.userSelect = '';
            // Commit: snap or save final position.
            const detail = el.closest('.detail');
            const max = detail ? detail.getBoundingClientRect().height - 60 - 6 : 800;
            const h = Math.max(60, Math.min(max, startH + (ev.clientY - startY)));
            snapOrSet(h);
            window.removeEventListener('mousemove', onMove);
            window.removeEventListener('mouseup', onUp);
        }
        window.addEventListener('mousemove', onMove);
        window.addEventListener('mouseup', onUp);
    });
}
