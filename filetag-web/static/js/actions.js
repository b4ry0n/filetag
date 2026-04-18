
// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

async function navigateTo(path) {
    _thumbClearCache();
    _kbCursor = -1;
    state.selectedFile = null;
    state.selectedDir = null;
    state.selectedPaths.clear();
    state.selectedFilesData.clear();
    state.activeTags.clear();
    _lastClickedPath = null;
    _armedBulkTag = null;
    await loadFiles(path);
    await loadSettings();
    render();
}

// Select a root card (virtual root page) — shows info in the detail panel.
async function selectRoot(rootPath) {
    state.selectedRoot = rootPath;
    state.selectedRootInfo = null;
    state.selectedFile = null;
    state.selectedDir = null;
    state.selectedPaths.clear();
    state.selectedFilesData.clear();
    _lastClickedPath = null;
    _armedBulkTag = null;
    renderDetail();
    try {
        state.selectedRootInfo = await api('/api/info?dir=' + encodeURIComponent(rootPath));
        renderDetail();
    } catch (_) { /* ignore */ }
}

// Enter a specific root database (from the virtual root listing).
async function enterRoot(rootPath) {
    _thumbClearCache();
    _kbCursor = -1;
    state.currentBasePath = rootPath;
    state.currentPath = '';
    state.selectedFile = null;
    state.selectedDir = null;
    state.selectedRoot = null;
    state.selectedRootInfo = null;
    state.selectedPaths.clear();
    state.selectedFilesData.clear();
    state.activeTags.clear();
    _lastClickedPath = null;
    _armedBulkTag = null;
    await Promise.all([loadInfo(), loadTags(), loadFiles(''), loadSettings()]);
    render();
}

// Navigate back to the virtual root (show all roots).
async function goVirtualRoot() {
    _thumbClearCache();
    _kbCursor = -1;
    state.currentBasePath = null;
    state.currentPath = '';
    state.selectedFile = null;
    state.selectedDir = null;
    state.selectedPaths.clear();
    state.selectedFilesData.clear();
    state.activeTags.clear();
    state.tags = [];
    state.info = null;
    _lastClickedPath = null;
    _armedBulkTag = null;
    await loadFiles('');
    render();
}


async function doSearch() {
    _thumbClearCache();
    const input = document.getElementById('search-input');
    const query = input.value.trim();
    if (!query) return;
    state.activeTags.clear();
    await searchFiles(query);
    document.getElementById('search-clear').hidden = false;
    render();
}

function doClearSearch() {
    _thumbClearCache();
    state.activeTags.clear();
    document.getElementById('search-input').value = '';
    document.getElementById('search-clear').hidden = true;
    navigateTo(state.currentPath || '');
}

function navigateToParent(filePath) {
    const parts = filePath.split('/');
    const dir = parts.length > 1 ? parts.slice(0, -1).join('/') : '';
    document.getElementById('search-input').value = '';
    document.getElementById('search-clear').hidden = true;
    navigateTo(dir);
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

async function toggleTagFilter(tagName) {
    if (state.activeTags.has(tagName)) {
        state.activeTags.delete(tagName);
    } else {
        state.activeTags.add(tagName);
    }
    if (state.activeTags.size === 0) {
        document.getElementById('search-input').value = '';
        document.getElementById('search-clear').hidden = true;
        await navigateTo(state.currentPath || '');
        return;
    }
    _thumbClearCache();
    const q = [...state.activeTags].map(quoteTag).join(' and ');
    document.getElementById('search-input').value = q;
    await searchFiles(q);
    document.getElementById('search-clear').hidden = false;
    render();
}

async function clearTagFilters() {
    state.activeTags.clear();
    document.getElementById('search-input').value = '';
    document.getElementById('search-clear').hidden = true;
    await navigateTo(state.currentPath || '');
}

async function selectFile(path, event) {
    // Dismiss trickplay overlay and move keyboard focus to the clicked item.
    // Floating sprites are cleaned up by the trickplay click handler; here we
    // only clear pinned inline sprites from cards that are not the clicked one.
    if (event && event.target) {
        const clickedCard = event.target.closest('.card');
        document.querySelectorAll('.card-trickplay-pinned').forEach(el => {
            if (!clickedCard || !clickedCard.contains(el)) el.remove();
        });
        const el = event.target.closest('[data-path], [data-root-path]');
        if (el) {
            const items = _kbItems();
            const idx = items.indexOf(el);
            if (idx !== -1) _kbSetCursor(idx, false);
        }
    }

    const layout = document.querySelector('.layout');
    const anchor = saveScrollAnchor(path);

    const isMulti = event && (event.ctrlKey || event.metaKey);
    const isShift = event && event.shiftKey;

    if (isMulti) {
        // Toggle this path in the multi-select set
        if (state.selectedPaths.has(path)) {
            state.selectedPaths.delete(path);
        } else {
            state.selectedPaths.add(path);
            if (!state.selectedFilesData.has(path)) {
                const data = await api('/api/file?path=' + encodeURIComponent(path) + dirParam('&'));
                state.selectedFilesData.set(path, data);
            }
        }
        _armedBulkTag = null;
        _lastClickedPath = path;
        // Keep selectedFile in sync with the most recently toggled file, or clear if set is empty
        if (state.selectedPaths.size === 1) {
            await loadFileDetail([...state.selectedPaths][0]);
        } else if (state.selectedPaths.size === 0) {
            state.selectedFile = null;
            state.selectedDir = null;
        } else {
            // Multi: update state but don't reload preview
            state.selectedDir = null;
        }
    } else if (isShift && _lastClickedPath) {
        // Range-select between _lastClickedPath and path
        const items = state.mode === 'search' ? state.searchResults
            : state.mode === 'zip' ? state.zipEntries.map(e => ({ path: state.zipPath + '::' + e.name }))
            : state.entries;
        const paths = items.filter(e => !e.is_dir).map(e => state.mode === 'search' ? e.path
            : state.mode === 'zip' ? e.path
            : fullPath(e));
        const a = paths.indexOf(_lastClickedPath);
        const b = paths.indexOf(path);
        if (a !== -1 && b !== -1) {
            const [lo, hi] = a < b ? [a, b] : [b, a];
            for (let i = lo; i <= hi; i++) state.selectedPaths.add(paths[i]);
        }
        _armedBulkTag = null;
        state.selectedDir = null;
        // Batch-fetch file data for newly added paths
        await Promise.all([...state.selectedPaths].map(async p => {
            if (!state.selectedFilesData.has(p)) {
                const data = await api('/api/file?path=' + encodeURIComponent(p) + dirParam('&'));
                state.selectedFilesData.set(p, data);
            }
        }));
    } else {
        // Plain click: single select
        state.selectedPaths.clear();
        state.selectedPaths.add(path);
        _lastClickedPath = path;
        await loadFileDetail(path);
    }

    if (!state.detailOpen) {
        state.detailOpen = true;
        layout.classList.remove('detail-collapsed');
        document.getElementById('detail-toggle').classList.add('active');
    }
    _updateCardSelection();
    renderDetail();
    restoreScrollAnchor(anchor);
}

async function doAddTag() {
    if (!state.selectedFile) return;
    const input = document.getElementById('tag-input');
    const tagStr = input.value.trim();
    if (!tagStr) return;
    await addTagToFile(state.selectedFile.path, tagStr);
    input.value = '';
    renderTags();
    _updateCardTagBadges();
    renderDetailTagsOnly();
    input.focus();
}

async function doDirAddTag() {
    if (!state.selectedDir) return;
    const input = document.getElementById('dir-tag-input');
    const tagStr = input?.value.trim();
    if (!tagStr) return;
    input.value = '';
    await addTagToDir(state.selectedDir.path, tagStr);
    renderTags();
    _updateCardTagBadges();
}

async function doRemoveTag(path, tagStr) {
    await removeTagFromFile(path, tagStr);
    renderTags();
    _updateCardTagBadges();
    renderDetailTagsOnly();
}

// ---------------------------------------------------------------------------
// Tag autocomplete
// ---------------------------------------------------------------------------

function attachTagAutocomplete(inputEl, submitFn) {
    let _dropdown = null;
    let _activeIdx = -1;

    function getMatches(query) {
        const q = query.toLowerCase();
        if (!q) {
            // Show top tags by count, excluding ones already on the selected file(s)
            const applied = new Set(
                (state.selectedFile?.tags || []).map(t => formatTag(t))
            );
            return [...state.tags]
                .filter(t => !applied.has(t.name))
                .sort((a, b) => b.count - a.count)
                .slice(0, 12);
        }
        return state.tags
            .filter(t => t.name.toLowerCase().includes(q))
            .sort((a, b) => {
                // Prefer prefix matches first
                const aPrefix = a.name.toLowerCase().startsWith(q);
                const bPrefix = b.name.toLowerCase().startsWith(q);
                if (aPrefix !== bPrefix) return aPrefix ? -1 : 1;
                return b.count - a.count;
            })
            .slice(0, 15);
    }

    function buildDropdown(tags) {
        if (!_dropdown) {
            _dropdown = document.createElement('ul');
            _dropdown.className = 'tag-autocomplete';
            inputEl.parentElement.appendChild(_dropdown);
        }
        _activeIdx = -1;
        if (!tags.length) { _dropdown.innerHTML = ''; _dropdown.hidden = true; return; }
        _dropdown.innerHTML = tags.map(tag => {
            const dot = tag.color
                ? `<span class="tag-color-dot" style="background:${tag.color}"></span>`
                : '';
            return `<li data-tagname="${esc(tag.name)}">${dot}<span class="ac-name">${esc(tag.name)}</span><span class="ac-count">${tag.count}</span></li>`;
        }).join('');
        _dropdown.hidden = false;
        _dropdown.querySelectorAll('li').forEach(li => {
            li.addEventListener('mousedown', e => {
                e.preventDefault(); // keep focus on input
                inputEl.value = li.dataset.tagname;
                closeDropdown();
                submitFn();
            });
        });
    }

    function closeDropdown() {
        if (_dropdown) { _dropdown.hidden = true; }
        _activeIdx = -1;
    }

    function setActive(idx) {
        const items = _dropdown ? _dropdown.querySelectorAll('li') : [];
        items.forEach(li => li.classList.remove('ac-active'));
        _activeIdx = idx;
        if (_activeIdx >= 0 && _activeIdx < items.length) {
            items[_activeIdx].classList.add('ac-active');
            inputEl.value = items[_activeIdx].dataset.tagname;
            items[_activeIdx].scrollIntoView({ block: 'nearest' });
        }
    }

    inputEl.addEventListener('input', () => buildDropdown(getMatches(inputEl.value.trim())));

    inputEl.addEventListener('focus', () => buildDropdown(getMatches(inputEl.value.trim())));

    inputEl.addEventListener('blur', () => setTimeout(closeDropdown, 150));

    inputEl.addEventListener('keydown', e => {
        const items = _dropdown ? _dropdown.querySelectorAll('li') : [];
        const count = items.length;
        if (e.key === 'ArrowDown') {
            e.preventDefault();
            if (!_dropdown || _dropdown.hidden) buildDropdown(getMatches(inputEl.value.trim()));
            setActive(Math.min(_activeIdx + 1, count - 1));
        } else if (e.key === 'ArrowUp') {
            e.preventDefault();
            setActive(Math.max(_activeIdx - 1, 0));
        } else if (e.key === 'Escape') {
            e.preventDefault();
            closeDropdown();
        } else if (e.key === 'Enter' || e.key === 'Tab') {
            e.preventDefault();
            if (_activeIdx >= 0 && !_dropdown.hidden) {
                inputEl.value = items[_activeIdx].dataset.tagname;
                closeDropdown();
            } else {
                closeDropdown();
            }
            submitFn();
        }
    });
}

// ---------------------------------------------------------------------------
// Search bar autocomplete (query-aware: completes the last token in place)
// ---------------------------------------------------------------------------

function attachSearchAutocomplete(inputEl, submitFn) {
    let _dropdown = null;
    let _activeIdx = -1;

    const OPS = new Set(['and', 'or', 'not']);

    function currentToken() {
        const cur = inputEl.selectionStart ?? inputEl.value.length;
        const before = inputEl.value.slice(0, cur);
        const m = before.match(/(\S+)$/);
        if (!m) return '';
        return OPS.has(m[1].toLowerCase()) ? '' : m[1];
    }

    function replaceCurrentToken(replacement) {
        const val = inputEl.value;
        const cur = inputEl.selectionStart ?? val.length;
        const before = val.slice(0, cur);
        const after = val.slice(cur);
        const m = before.match(/^([\s\S]*)(\S+)$/);
        if (!m) {
            // Nothing before cursor: prepend replacement
            inputEl.value = replacement + (after ? ' ' + after.trimStart() : '');
            inputEl.setSelectionRange(replacement.length, replacement.length);
            return;
        }
        const prefix = m[1];
        const token = m[2];
        if (OPS.has(token.toLowerCase())) {
            // Token is a keyword — insert after it
            const newBefore = before + ' ' + replacement;
            inputEl.value = newBefore + (after ? (after.startsWith(' ') ? after : ' ' + after) : '');
            inputEl.setSelectionRange(newBefore.length, newBefore.length);
        } else {
            // Replace the partial tag token
            const newBefore = prefix + replacement;
            inputEl.value = newBefore + after;
            inputEl.setSelectionRange(newBefore.length, newBefore.length);
        }
    }

    function getMatches(token) {
        const q = token.toLowerCase();
        if (!q) return [...state.tags].sort((a, b) => b.count - a.count).slice(0, 12);
        return state.tags
            .filter(t => t.name.toLowerCase().includes(q))
            .sort((a, b) => {
                const aP = a.name.toLowerCase().startsWith(q);
                const bP = b.name.toLowerCase().startsWith(q);
                if (aP !== bP) return aP ? -1 : 1;
                return b.count - a.count;
            })
            .slice(0, 15);
    }

    function buildDropdown(tags) {
        if (!_dropdown) {
            _dropdown = document.createElement('ul');
            _dropdown.className = 'tag-autocomplete';
            inputEl.parentElement.appendChild(_dropdown);
        }
        _activeIdx = -1;
        if (!tags.length) { _dropdown.innerHTML = ''; _dropdown.hidden = true; return; }
        _dropdown.innerHTML = tags.map(tag => {
            const dot = tag.color
                ? `<span class="tag-color-dot" style="background:${tag.color}"></span>`
                : '';
            return `<li data-tagname="${esc(tag.name)}">${dot}<span class="ac-name">${esc(tag.name)}</span><span class="ac-count">${tag.count}</span></li>`;
        }).join('');
        _dropdown.hidden = false;
        _dropdown.querySelectorAll('li').forEach(li => {
            li.addEventListener('mousedown', e => {
                e.preventDefault();
                replaceCurrentToken(li.dataset.tagname);
                closeDropdown();
                inputEl.focus();
            });
        });
    }

    function closeDropdown() {
        if (_dropdown) { _dropdown.hidden = true; }
        _activeIdx = -1;
    }

    function setActive(idx) {
        const items = _dropdown ? _dropdown.querySelectorAll('li') : [];
        items.forEach(li => li.classList.remove('ac-active'));
        _activeIdx = idx;
        if (_activeIdx >= 0 && _activeIdx < items.length) {
            items[_activeIdx].classList.add('ac-active');
            items[_activeIdx].scrollIntoView({ block: 'nearest' });
        }
    }

    inputEl.addEventListener('input', () => buildDropdown(getMatches(currentToken())));
    inputEl.addEventListener('focus', () => buildDropdown(getMatches(currentToken())));
    inputEl.addEventListener('blur', () => setTimeout(closeDropdown, 150));

    inputEl.addEventListener('keydown', e => {
        const items = _dropdown ? _dropdown.querySelectorAll('li') : [];
        const count = items.length;
        if (e.key === 'ArrowDown') {
            e.preventDefault();
            if (!_dropdown || _dropdown.hidden) buildDropdown(getMatches(currentToken()));
            setActive(Math.min(_activeIdx + 1, count - 1));
        } else if (e.key === 'ArrowUp') {
            e.preventDefault();
            setActive(Math.max(_activeIdx - 1, 0));
        } else if (e.key === 'Escape') {
            if (_dropdown && !_dropdown.hidden) { e.preventDefault(); closeDropdown(); }
        } else if (e.key === 'Tab' && _activeIdx >= 0 && _dropdown && !_dropdown.hidden) {
            e.preventDefault();
            replaceCurrentToken(items[_activeIdx].dataset.tagname);
            closeDropdown();
        } else if (e.key === 'Enter') {
            if (_activeIdx >= 0 && _dropdown && !_dropdown.hidden) {
                e.preventDefault();
                replaceCurrentToken(items[_activeIdx].dataset.tagname);
                closeDropdown();
            } else {
                closeDropdown();
                submitFn();
            }
        }
    });
}

function clearSelection() {
    state.selectedPaths.clear();
    state.selectedFilesData.clear();
    state.selectedFile = null;
    state.selectedDir = null;
    state.selectedRoot = null;
    state.selectedRootInfo = null;
    _lastClickedPath = null;
    _armedBulkTag = null;
    _updateCardSelection();
    renderDetail();
}

async function doBulkAddTag() {
    const input = document.getElementById('bulk-tag-input');
    const tagStr = input.value.trim();
    if (!tagStr) return;
    const paths = [...state.selectedPaths];
    const status = document.getElementById('bulk-status');
    status.textContent = 'Adding...';
    await Promise.all(paths.map(p => apiPost('/api/tag', { path: p, tags: [tagStr], dir: currentAbsDir() })));
    // Refresh cached data for all selected files
    await Promise.all(paths.map(async p => {
        const data = await api('/api/file?path=' + encodeURIComponent(p) + dirParam('&'));
        state.selectedFilesData.set(p, data);
    }));
    await loadTags();
    if (state.mode === 'browse') await loadFiles(state.currentPath);
    input.value = '';
    status.textContent = `Added "${tagStr}" to ${paths.length} file${paths.length === 1 ? '' : 's'}.`;
    renderTags();
    renderContent();
    _thumbInit();
    _dirThumbInit();
    _kbRestoreFocus();
    const el = document.getElementById('bulk-tag-chips');
    if (el) el.innerHTML = renderBulkTagChips(aggregateBulkTags(), state.selectedPaths.size);
    else renderDetail(); // chips container not in DOM yet (first tag added)
    input.focus();
}

async function toggleShowHidden() {
    state.showHidden = !state.showHidden;
    document.getElementById('hidden-toggle').classList.toggle('active', state.showHidden);
    if (state.mode === 'browse') {
        await loadFiles(state.currentPath);
        renderContent();
        _thumbInit();
        _dirThumbInit();
    }
}

function toggleCacheMenu(e) {
    e.stopPropagation();
    const menu = document.getElementById('cache-menu');
    menu.hidden = !menu.hidden;
}

document.addEventListener('click', () => {
    const menu = document.getElementById('cache-menu');
    if (menu) menu.hidden = true;
});

async function clearCache(all = false) {
    // Close dropdown if open
    const menu = document.getElementById('cache-menu');
    if (menu) menu.hidden = true;

    // A root must always be explicitly selected. Never write to other roots.
    if (state.currentBasePath == null) {
        showToast('No database selected — navigate into a database first');
        return;
    }

    const btn = document.getElementById('cache-clear-page-btn');
    btn.disabled = true;
    const toast = showToast(all ? 'Clearing cache…' : 'Clearing page cache…', 0);
    let success = false;
    let errorMsg = 'Cache clear failed';
    try {
        let body = null;
        if (!all) {
            if (state.mode === 'search') {
                const paths = (state.searchResults || [])
                    .filter(e => !e.is_dir)
                    .map(e => e.path)
                    .filter(Boolean);
                body = JSON.stringify({ paths });
            } else {
                body = JSON.stringify({ dir: state.currentPath || '' });
            }
        } else {
            // Send all:true plus the current directory so the server can find
            // the exact (deepest) root that owns it and clear only that root's cache.
            body = JSON.stringify({ all: true });
        }
        // Always send the explicit current dir — never omit it.
        const resp = await fetch('/api/cache/clear' + dirParam('?'), {
            method: 'POST',
            headers: body ? { 'Content-Type': 'application/json' } : {},
            body: body ?? undefined,
        });
        if (!resp.ok) {
            errorMsg = 'Cache clear failed: ' + (await resp.text()).trim();
            throw new Error(errorMsg);
        }
        // Invalidate the in-memory blob URL cache so thumbnails reload from
        // the server rather than being served from the old cached blobs.
        _thumbClearCache();
        success = true;
    } catch (e) {
        errorMsg = e.message || errorMsg;
    } finally {
        btn.disabled = false;
        dismissToast(toast);
        showToast(success ? (all ? 'Cache cleared' : 'Page cache cleared') : errorMsg);
        if (state.mode === 'search') {
            await doSearch();
        } else {
            await loadFiles(state.currentPath);
        }
        render();
    }
}

async function pregenSprites() {
    const menu = document.getElementById('cache-menu');
    if (menu) menu.hidden = true;

    const VIDEO_EXTS = new Set([
        'mp4','webm','mkv','avi','mov','wmv','flv','m4v','3gp','f4v','mpg','mpeg',
        'm2v','m2ts','mts','mxf','rm','rmvb','divx','vob','ogv','ogg','dv','asf','amv',
        'mpe','m1v','mpv','qt',
    ]);

    const items = state.mode === 'search' ? state.searchResults : state.entries;
    const videoPaths = (items || [])
        .filter(e => {
            if (e.is_dir) return false;
            const p = state.mode === 'search' ? e.path : fullPath(e);
            return p && VIDEO_EXTS.has(p.split('.').pop().toLowerCase());
        })
        .map(e => state.mode === 'search' ? e.path : fullPath(e));

    if (videoPaths.length === 0) return;

    const btn = document.getElementById('pregen-sprites-btn');
    btn.disabled = true;

    const toast = showToast(`Generating video sprites… (0 / ${videoPaths.length})`, 0);
    toast.classList.add('toast-progress');

    let done = 0;
    for (const path of videoPaths) {
        updateToast(toast, `Generating video sprites… (${done} / ${videoPaths.length})`);
        try {
            const minN = state.settings.sprite_min ?? 8;
            const maxN = state.settings.sprite_max ?? 16;
            await fetch('/api/vthumbs?' + new URLSearchParams({ path, min_n: minN, max_n: maxN }) + dirParam('&'));
        } catch (_) { /* ignore */ }
        done++;
        _trickplayCache.delete(path);
    }

    dismissToast(toast);
    showToast(`Done: ${done} sprite${done !== 1 ? 's' : ''} generated`);
    btn.disabled = false;
}

// ---------------------------------------------------------------------------
// AI image analysis
// ---------------------------------------------------------------------------

const AI_IMAGE_EXTS = new Set([
    'jpg','jpeg','png','gif','webp','bmp','avif','tiff','tif','heic','heif',
    'arw','cr2','cr3','nef','orf','rw2','dng','raf','pef','srw',
    'raw','3fr','x3f','rwl','iiq','mef','mos',
]);

const AI_ARCHIVE_EXTS = new Set(['zip','cbz','rar','cbr','7z','cb7']);

function isAiImage(path) {
    const ext = (path || '').split('.').pop().toLowerCase();
    return AI_IMAGE_EXTS.has(ext) || AI_ARCHIVE_EXTS.has(ext);
}

// ---------------------------------------------------------------------------
// Settings modal
// ---------------------------------------------------------------------------

function switchSettingsTab(tab) {
    document.querySelectorAll('.modal-tab-btn').forEach(b => {
        b.classList.toggle('active', b.dataset.tab === tab);
    });
    document.querySelectorAll('.modal-tab-panel').forEach(p => {
        p.hidden = (p.id !== `settings-tab-${tab}`);
    });
}

async function openSettings(tab = 'video') {
    const menu = document.getElementById('cache-menu');
    if (menu) menu.hidden = true;
    // Video settings from per-root state
    document.getElementById('sprite-min').value = state.settings.sprite_min ?? 8;
    document.getElementById('sprite-max').value = state.settings.sprite_max ?? 16;
    // Feature flags from per-root state
    document.getElementById('feat-video').checked = state.settings.feature_video ?? false;
    document.getElementById('feat-imagemagick').checked = state.settings.feature_imagemagick ?? false;
    document.getElementById('feat-pdf').checked = state.settings.feature_pdf ?? false;
    // AI settings from server
    try {
        const res = await fetch('/api/ai/config?' + new URLSearchParams({ dir: currentAbsDir() || '' }));
        const cfg = await res.json();
        document.getElementById('ai-endpoint').value = cfg.endpoint || '';
        document.getElementById('ai-model').value = cfg.model || '';
        document.getElementById('ai-api-key').value = '';
        document.getElementById('ai-api-key').placeholder = cfg.api_key || 'Leave empty for local models';
        document.getElementById('ai-tag-prefix').value = cfg.tag_prefix || 'ai/';
        document.getElementById('ai-max-tokens').value = cfg.max_tokens || 512;
        document.getElementById('ai-prompt').value = cfg.prompt || '';
        document.getElementById('ai-prompt').placeholder = cfg.default_prompt || '';
        document.getElementById('ai-format').value = cfg.format || 'openai';
        const enabled = cfg.enabled !== false; // default true if key absent
        document.getElementById('ai-enabled').checked = enabled;
        document.getElementById('ai-settings-fields').hidden = !enabled;
    } catch (_) { /* defaults are fine */ }
    document.getElementById('ai-test-result').hidden = true;
    switchSettingsTab(tab);
    document.getElementById('settings-modal').hidden = false;
}

function closeSettings() {
    document.getElementById('settings-modal').hidden = true;
}

async function saveVideoSettings() {
    const min = parseInt(document.getElementById('sprite-min').value, 10);
    const max = parseInt(document.getElementById('sprite-max').value, 10);
    if (min >= 2 && min <= 64 && max >= 2 && max <= 64) {
        const body = {
            dir: currentAbsDir(),
            sprite_min: min,
            sprite_max: Math.max(max, min),
        };
        try {
            await apiPost('/api/settings', body);
            state.settings.sprite_min = body.sprite_min;
            state.settings.sprite_max = body.sprite_max;
        } catch (e) {
            showToast('Failed to save settings: ' + e.message);
            return;
        }
    }
    closeSettings();
}

async function saveFeaturesSettings() {
    const body = {
        dir: currentAbsDir(),
        feature_video: document.getElementById('feat-video').checked,
        feature_imagemagick: document.getElementById('feat-imagemagick').checked,
        feature_pdf: document.getElementById('feat-pdf').checked,
    };
    try {
        await apiPost('/api/settings', body);
        state.settings.feature_video = body.feature_video;
        state.settings.feature_imagemagick = body.feature_imagemagick;
        state.settings.feature_pdf = body.feature_pdf;
        _thumbClearCache();
        closeSettings();
        await loadFiles(state.currentPath);
    } catch (e) {
        showToast('Failed to save settings: ' + e.message);
        return;
    }
}

// Backward-compat wrappers (called from cache-menu & ai-test flow)
function openAiSettings() { openSettings('ai'); }
function closeAiSettings() { closeSettings(); }

function aiToggleEnabled() {
    const enabled = document.getElementById('ai-enabled').checked;
    document.getElementById('ai-settings-fields').hidden = !enabled;
}

function aiUseDefaultPrompt() {
    const el = document.getElementById('ai-prompt');
    if (el) el.value = el.placeholder;
}

async function aiSaveSettings() {
    const body = {
        endpoint: document.getElementById('ai-endpoint').value.trim(),
        model: document.getElementById('ai-model').value.trim(),
        prompt: document.getElementById('ai-prompt').value,
        tag_prefix: document.getElementById('ai-tag-prefix').value.trim(),
        max_tokens: parseInt(document.getElementById('ai-max-tokens').value, 10) || 512,
        format: document.getElementById('ai-format').value,
        enabled: document.getElementById('ai-enabled').checked,
        dir: currentAbsDir(),
    };
    const apiKey = document.getElementById('ai-api-key').value;
    if (apiKey) body.api_key = apiKey;
    try {
        await apiPost('/api/ai/config', body);
        closeAiSettings();
    } catch (e) {
        alert('Save failed: ' + e.message);
    }
}

async function aiTestConnection() {
    const resultEl = document.getElementById('ai-test-result');
    resultEl.hidden = false;
    resultEl.textContent = 'Saving settings and testing…';

    // Save first so the server has the current config
    await aiSaveSettings();
    // Re-open the modal on the AI tab (save closes it)
    document.getElementById('settings-modal').hidden = false;
    switchSettingsTab('ai');
    resultEl.hidden = false;

    // Find an image to test with: prefer the selected file, then the current view
    let testFile = null;
    if (state.selectedFile && isAiImage(state.selectedFile.path)) {
        testFile = state.selectedFile;
    } else {
        const items = state.mode === 'search' ? (state.searchResults || []) : (state.entries || []);
        testFile = items.find(e => !e.is_dir && isAiImage(e.path));
    }
    if (!testFile) {
        resultEl.textContent = 'No image found in current view to test with.';
        return;
    }

    try {
        const res = await fetch('/api/ai/analyse', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ path: testFile.path, dir: currentAbsDir(), dry_run: true }),
        });
        const data = await res.json();
        if (!res.ok) {
            resultEl.textContent = 'Error: ' + (data.error || res.statusText);
        } else {
            if ((data.tags || []).length > 0) {
                resultEl.textContent = 'Tags: ' + data.tags.join(', ');
            } else {
                resultEl.textContent = 'No tags recognised.\n\nRaw model response:\n' + (data.raw || '(empty)');
            }
        }
    } catch (e) {
        resultEl.textContent = 'Connection failed: ' + e.message;
    }
}

/// Promote an ai/* tag: add it without the prefix, then remove the original.
async function aiPromoteTag(path, tagName, value) {
    // tagName is e.g. "ai/necklace", promoted becomes "necklace".
    // value may be "" or e.g. "gold" for key=value tags.
    const promoted = tagName.slice('ai/'.length);
    if (!promoted) return;
    const newTagStr = value ? `${promoted}=${value}` : promoted;
    const toast = showToast(`Promoting: ${newTagStr}…`, 0);
    try {
        // Add the promoted tag, then remove the ai/ original.
        await apiPost('/api/tag', { path, tags: [newTagStr], dir: currentAbsDir() });
        const origStr = value ? `${tagName}=${value}` : tagName;
        await apiPost('/api/untag', { path, tags: [origStr], dir: currentAbsDir() });
        await loadFileDetail(path);
        await loadTags();
        renderDetailTagsOnly();
        _updateCardTagBadges();
    } catch (e) {
        showToast('Promote failed: ' + e.message);
    } finally {
        dismissToast(toast);
    }
}

/// Remove all ai/* tags from given paths.
async function aiClearTags(paths) {
    const toast = showToast(`Removing ai/ tags from ${paths.length} file${paths.length !== 1 ? 's' : ''}…`, 0);
    try {
        await apiPost('/api/ai/clear-tags', { paths, dir: currentAbsDir() });
        // Refresh each file that may be currently selected.
        for (const p of paths) {
            if (state.selectedFile?.path === p) {
                await loadFileDetail(p);
            }
        }
        await loadTags();
        renderDetail();
        _updateCardTagBadges();
        dismissToast(toast);
        showToast(`ai/ tags removed`);
    } catch (e) {
        dismissToast(toast);
        showToast('Remove failed: ' + e.message);
    }
}

async function aiAnalyseSingle(path) {
    if (state.aiAnalysing.has(path)) return; // already running
    state.aiAnalysing.add(path);
    // Re-render so the button shows "Analysing…" immediately (also persists on navigate-away & back)
    if (state.selectedFile?.path === path) renderDetail();
    const toast = showToast(`AI: analysing…`, 0);
    try {
        const res = await fetch('/api/ai/analyse', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ path, dir: currentAbsDir() }),
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || res.statusText);
        const n = (data.tags || []).length;
        dismissToast(toast);
        showToast(`AI: ${n} tag${n !== 1 ? 's' : ''} added`);
        if (state.selectedFile?.path === path) {
            await loadFileDetail(path);
            await loadTags();
        }
    } catch (e) {
        dismissToast(toast);
        showToast('AI error: ' + e.message);
    } finally {
        state.aiAnalysing.delete(path);
        if (state.selectedFile?.path === path) renderDetail();
    }
}

async function aiAnalyseBatch() {
    const menu = document.getElementById('cache-menu');
    if (menu) menu.hidden = true;

    let imagePaths;
    if (state.mode === 'zip') {
        imagePaths = (state.zipEntries || [])
            .filter(e => e.is_image)
            .map(e => state.zipPath + '::' + e.name);
    } else {
        const items = state.mode === 'search' ? state.searchResults : state.entries;
        imagePaths = (items || [])
            .filter(e => !e.is_dir && isAiImage(e.path))
            .map(e => e.path);
    }

    if (imagePaths.length === 0) {
        showToast('No images in current view');
        return;
    }

    const btn = document.getElementById('ai-analyse-btn');
    if (btn) btn.disabled = true;

    const toast = showToast(`AI: queuing ${imagePaths.length} image${imagePaths.length !== 1 ? 's' : ''}…`, 0);
    toast.classList.add('toast-progress');

    try {
        const res = await fetch('/api/ai/analyse-batch', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ paths: imagePaths, dir: currentAbsDir() }),
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || res.statusText);

        // Poll progress
        const poll = setInterval(async () => {
            try {
                const sr = await fetch('/api/ai/status');
                const prog = await sr.json();
                updateToast(toast, `AI: analysing… (${prog.done} / ${prog.total})`);
                if (!prog.running) {
                    clearInterval(poll);
                    dismissToast(toast);
                    showToast(`AI done: ${prog.done} image${prog.done !== 1 ? 's' : ''} analysed`);
                    if (btn) btn.disabled = false;
                    await loadTags();
                }
            } catch (_) {
                clearInterval(poll);
                dismissToast(toast);
                if (btn) btn.disabled = false;
            }
        }, 2000);
    } catch (e) {
        dismissToast(toast);
        showToast('AI error: ' + e.message);
        if (btn) btn.disabled = false;
    }
}

async function refreshSelectedFile() {
    if (!state.selectedFile) return;
    try {
        const res = await fetch('/api/file?path=' + encodeURIComponent(state.selectedFile.path) + dirParam('&'));
        if (res.ok) {
            state.selectedFile = await res.json();
            renderDetailTagsOnly();
        }
    } catch (_) {}
    await loadTags();
}

function setViewMode(mode) {
    state.viewMode = mode;
    document.getElementById('view-grid').classList.toggle('active', mode === 'grid');
    document.getElementById('view-list').classList.toggle('active', mode === 'list');
    document.getElementById('zoom-slider').style.display = mode === 'grid' ? '' : 'none';
    renderContent();
    _thumbInit();
    _dirThumbInit();
    _kbRestoreFocus();
}

function toggleDetailPanel() {
    const activePath = state.selectedFile?.path || state.selectedDir?.path || null;
    const anchor = saveScrollAnchor(activePath);
    const layout = document.querySelector('.layout');
    const collapsed = layout.classList.toggle('detail-collapsed');
    state.detailOpen = !collapsed;
    document.getElementById('detail-toggle').classList.toggle('active', !collapsed);
    restoreScrollAnchor(anchor);
}

function setCardSize(size) {
    document.getElementById('content').style.setProperty('--card-size', size + 'px');
}

function toggleTagGroup(prefix) {
    if (state.expandedGroups.has(prefix)) {
        state.expandedGroups.delete(prefix);
    } else {
        state.expandedGroups.add(prefix);
    }
    renderTags();
}

async function doTagGroupSearch(prefix) {
    // Expand group on click and clear any active tag filters
    _thumbClearCache();
    state.activeTags.clear();
    state.expandedGroups.add(prefix);
    const hasRoot = state.tags.some(t => t.name === prefix);
    const q = hasRoot ? `${prefix} or ${prefix}/*` : `${prefix}/*`;
    document.getElementById('search-input').value = q;
    await searchFiles(q);
    document.getElementById('search-clear').hidden = false;
    render();
}

function closeDetail() {
    const activePath = state.selectedFile?.path || state.selectedDir?.path || null;
    const anchor = saveScrollAnchor(activePath);
    state.selectedFile = null;
    state.selectedDir = null;
    state.selectedRoot = null;
    state.selectedRootInfo = null;
    state.selectedPaths.clear();
    state.selectedFilesData.clear();
    state.detailOpen = false;
    document.querySelector('.layout').classList.add('detail-collapsed');
    document.getElementById('detail-toggle').classList.remove('active');
    _updateCardSelection();
    renderDetail();
    restoreScrollAnchor(anchor);
}
