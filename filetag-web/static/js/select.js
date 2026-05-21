// ---------------------------------------------------------------------------
// File selection logic (single + multi-select)
// ---------------------------------------------------------------------------

// Loads file detail into selectedFilesData for paths not yet present.
// For small selections the individual /api/file endpoint is used; for larger
// ones a single /api/files-tags request fetches all tags at once.
async function _loadMissingFilesData(paths) {
    const missing = paths.filter(p => !state.selectedFilesData.has(p));
    if (!missing.length) return;

    if (missing.length <= 5) {
        // Small selection: full detail per file (includes size, mtime, duration…).
        await Promise.all(missing.map(async p => {
            const rid = searchRootIdForPath(p);
            const rootParam = rid != null ? '&root_id=' + rid : '';
            const data = await api('/api/file?path=' + encodeURIComponent(p) + rootParam);
            state.selectedFilesData.set(p, data);
        }));
        return;
    }

    // Large selection: one bulk request per root, only tags returned.
    // Group paths by root_id (handles cross-root search results).
    const byRootId = new Map();
    for (const p of missing) {
        const d = searchRootIdForPath(p);
        if (!byRootId.has(d)) byRootId.set(d, []);
        byRootId.get(d).push(p);
    }
    await Promise.all([...byRootId.entries()].map(async ([rootId, ps]) => {
        const body = rootId != null ? { paths: ps, root_id: rootId } : { paths: ps };
        const res = await apiPost('/api/files-tags', body);
        for (const [path, tags] of Object.entries(res.files || {})) {
            if (!state.selectedFilesData.has(path)) {
                // Minimal record: enough for bulk tag chips and tag operations.
                state.selectedFilesData.set(path, { path, tags: tags || [], covered: true });
            }
        }
    }));
}

// Handles file selection (single and multi-select with shift/ctrl/cmd)
async function selectFile(path, event) {
    // Cancel any in-progress select-all so it doesn't overwrite this click.
    _selectAllToken = null;
    state.selectionLoading = false;

    // Multi-select with Ctrl/Cmd
    if (event && (event.ctrlKey || event.metaKey)) {
        if (state.selectedPaths.has(path)) {
            state.selectedPaths.delete(path);
            state.selectedFilesData.delete(path);
        } else {
            state.selectedPaths.add(path);
            await _loadMissingFilesData([path]);
        }
        _updateCardSelection();
        renderDetail();
        return;
    }
    // Range select with Shift
    if (event && event.shiftKey && _lastClickedPath && _lastClickedPath !== path) {
        const items = Array.from(document.querySelectorAll('#content [data-path]'));
        const idx1 = items.findIndex(el => el.getAttribute('data-path') === _lastClickedPath);
        const idx2 = items.findIndex(el => el.getAttribute('data-path') === path);
        if (idx1 !== -1 && idx2 !== -1) {
            const [start, end] = idx1 < idx2 ? [idx1, idx2] : [idx2, idx1];
            const newPaths = [];
            for (let i = start; i <= end; ++i) {
                const p = items[i].getAttribute('data-path');
                state.selectedPaths.add(p);
                newPaths.push(p);
            }
            _updateCardSelection();
            state.selectionLoading = newPaths.some(p => !state.selectedFilesData.has(p));
            if (state.selectionLoading) renderDetail(); // show spinner immediately
            await _loadMissingFilesData(newPaths);
            state.selectionLoading = false;
            _updateCardSelection();
            renderDetail();
            _lastClickedPath = path;
            return;
        }
    }
    // Single select (default)
    state.selectedPaths.clear();
    state.selectedFilesData.clear();
    state.selectedPaths.add(path);
    _lastClickedPath = path;
    // Highlight the card immediately so the UI responds without waiting for
    // the network.  Keep the old detail panel visible until the new data
    // arrives to avoid an empty-panel flash.
    _updateCardSelection();
    // Use the card's data-root-id attribute when available (correct root in search mode).
    const rootId = event?.currentTarget?.dataset?.rootId != null
        ? parseInt(event.currentTarget.dataset.rootId)
        : null;
    // Load the file detail and refresh the panel when done.
    await loadFileDetail(path, rootId != null && !isNaN(rootId) ? rootId : undefined);
    // Guard: another file may have been selected while we were waiting.
    if (state.selectedPaths.size === 1 && state.selectedPaths.has(path)) {
        renderDetail();
    }
}

// Expose for global use
window.selectFile = selectFile;
