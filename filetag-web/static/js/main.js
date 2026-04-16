// ---------------------------------------------------------------------------
// Keyboard navigation (file browser)
// ---------------------------------------------------------------------------

/** All navigable items in the content area, in DOM order. */
function _kbItems() {
    return [...document.querySelectorAll('#content [data-path], #content [data-root-id]')];
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
    if (el.dataset.rootId != null) {
        enterRoot(Number(el.dataset.rootId));
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
        if (state.zipPath) {
            const parts = state.zipPath.split('/');
            await navigateTo(parts.length > 1 ? parts.slice(0, -1).join('/') : '');
        }
        return;
    }
    if (!state.currentPath) {
        if (state.currentRootId != null) await goVirtualRoot();
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
        : state.mode === 'zip' ? state.zipEntries.map(en => ({ path: state.zipPath + '::' + en.name, is_dir: false }))
        : state.entries;
    const filePaths = (allItems || [])
        .filter(en => !en.is_dir)
        .map(en => (state.mode === 'search' || state.mode === 'zip') ? en.path : fullPath(en));
    filePaths.forEach(p => state.selectedPaths.add(p));
    await Promise.all(filePaths.map(async p => {
        if (!state.selectedFilesData.has(p)) {
            const data = await api('/api/file?path=' + encodeURIComponent(p) + rootParam('&'));
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
    const savedRoot = sessionStorage.getItem('ft_root');
    await loadRoots();
    // Restore root selection; for single-root loadRoots() already set currentRootId = 0.
    if (state.roots.filter(r => r.entry_point).length > 1 && savedRoot !== '' && savedRoot != null) {
        const id = parseInt(savedRoot, 10);
        // Only restore if the saved id still maps to an entry-point root.
        if (!isNaN(id) && id < state.roots.length && state.roots[id]?.entry_point) {
            state.currentRootId = id;
        }
    }
    try { await Promise.all([loadInfo(), loadTags()]); } catch (e) { console.error('loadInfo/loadTags failed:', e); }
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

    // Media viewer stage events (wheel, drag, pinch)
    _cvInitStageEvents();
});
