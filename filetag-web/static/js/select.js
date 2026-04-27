// ---------------------------------------------------------------------------
// File selection logic (single + multi-select)
// ---------------------------------------------------------------------------

// Handles file selection (single and multi-select with shift/ctrl/cmd)
function selectFile(path, event) {
    // Multi-select with Ctrl/Cmd
    if (event && (event.ctrlKey || event.metaKey)) {
        if (state.selectedPaths.has(path)) {
            state.selectedPaths.delete(path);
            state.selectedFilesData.delete(path);
        } else {
            state.selectedPaths.add(path);
            // Optionally load file detail for aggregation
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
            for (let i = start; i <= end; ++i) {
                const p = items[i].getAttribute('data-path');
                state.selectedPaths.add(p);
            }
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
    // Optionally load file detail for the selected file
    loadFileDetail(path);
    _updateCardSelection();
    renderDetail();
}

// Helper to update card/list selection classes
function _updateCardSelection() {
    document.querySelectorAll('#content [data-path]').forEach(el => {
        const path = el.getAttribute('data-path');
        el.classList.toggle('selected', state.selectedPaths.has(path));
    });
}

// Expose for global use
window.selectFile = selectFile;
