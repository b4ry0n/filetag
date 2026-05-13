// ---------------------------------------------------------------------------
// File selection logic (single + multi-select)
// ---------------------------------------------------------------------------

// Loads file detail into selectedFilesData for paths not yet present.
async function _loadMissingFilesData(paths) {
    await Promise.all(paths.map(async p => {
        if (!state.selectedFilesData.has(p)) {
            const data = await api('/api/file?path=' + encodeURIComponent(p) + '&dir=' + encodeURIComponent(searchDirForPath(p)));
            state.selectedFilesData.set(p, data);
        }
    }));
}

// Handles file selection (single and multi-select with shift/ctrl/cmd)
async function selectFile(path, event) {
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
            await _loadMissingFilesData(newPaths);
            _updateCardSelection();
            renderDetail();
            _lastClickedPath = path;
            return;
        }
    }
    // Single select (default)
    state.selectedPaths.clear();
    state.selectedPaths.add(path);
    _lastClickedPath = path;
    // Highlight the card immediately so the UI responds without waiting for
    // the network.  Keep the old detail panel visible until the new data
    // arrives to avoid an empty-panel flash.
    _updateCardSelection();
    // Use the card's data-dir attribute when available (correct root in search mode).
    const dir = event?.currentTarget?.dataset?.dir || null;
    // Load the file detail and refresh the panel when done.
    await loadFileDetail(path, dir);
    // Guard: another file may have been selected while we were waiting.
    if (state.selectedPaths.size === 1 && state.selectedPaths.has(path)) {
        renderDetail();
    }
}

// Expose for global use
window.selectFile = selectFile;
