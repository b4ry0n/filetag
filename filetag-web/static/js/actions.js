
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
    if (!/^[a-zA-Z0-9_\-\/.:*]+$/.test(name)) return '"' + name.replace(/"/g, "'") + '"';
    return name;
}

function handleTagSearch(val) {
    state.tagFilter = val.trim();
    renderTags();
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
        // When transitioning from single-select: sync the currently selected file's
        // data into selectedFilesData. Always overwrite to avoid using stale cached data
        // (e.g. tags added after the last multi-select were only reflected in selectedFile).
        if (state.selectedFile) {
            state.selectedFilesData.set(state.selectedFile.path, state.selectedFile);
        }
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
        _syncChatRight();
    }
    _updateCardSelection();
    renderDetail();
    restoreScrollAnchor(anchor);
}

async function doAddTag() {
    if (!state.selectedFile) return;
    const input = document.getElementById('tag-input');
    const subjectInput = document.getElementById('tag-subject');
    const tagStr = input.value.trim();
    if (!tagStr) return;
    const subject = subjectInput?.value.trim() || undefined;
    await addTagToFile(state.selectedFile.path, tagStr, subject);
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

async function doRemoveTag(path, tagStr, subject) {
    await removeTagFromFile(path, tagStr, subject);
    renderTags();
    _updateCardTagBadges();
    renderDetailTagsOnly();
}

async function doRemoveSubject(path, subject) {
    const f = state.selectedFilesData.get(path) || state.selectedFile;
    if (!f) return;
    const subjectTags = (f.tags || []).filter(tag => (tag.subject || '') === subject);
    for (const tag of subjectTags) {
        await removeTagFromFile(path, formatTag(tag), subject);
    }
    renderTags();
    _updateCardTagBadges();
    renderDetailTagsOnly();
}

// ---------------------------------------------------------------------------
// Tag autocomplete
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Subject autocomplete
// ---------------------------------------------------------------------------

/// Collect subjects for the single-file subject autocomplete:
/// own subjects (already on this file) first, then all state.subjects.
function collectSingleFileSubjects() {
    const own = new Set(
        (state.selectedFile?.tags || []).map(t => t.subject).filter(Boolean)
    );
    const all = (state.subjects || []).map(s => s.name);
    return [...own, ...all.filter(s => !own.has(s))];
}

/// Collect subjects for the bulk subject autocomplete:
/// subjects from selected files first, then all state.subjects.
function collectBulkSubjects() {
    const own = new Set();
    for (const [, data] of state.selectedFilesData) {
        for (const tag of (data.tags || [])) {
            if (tag.subject) own.add(tag.subject);
        }
    }
    const all = (state.subjects || []).map(s => s.name);
    return [...own, ...all.filter(s => !own.has(s))];
}

/// Attach a simple autocomplete dropdown to a subject text input.
/// `getSubjects` is called each time to retrieve the current subject list.
function attachSubjectAutocomplete(inputEl, getSubjects) {
    if (!inputEl) return;
    let _dropdown = null;

    function buildDropdown(subjects) {
        if (!_dropdown) {
            _dropdown = document.createElement('ul');
            _dropdown.className = 'tag-autocomplete';
            inputEl.parentElement.appendChild(_dropdown);
        }
        const q = inputEl.value.trim().toLowerCase();
        const matches = q
            ? subjects.filter(s => s.toLowerCase().includes(q))
            : subjects;
        if (!matches.length) { _dropdown.innerHTML = ''; _dropdown.hidden = true; return; }
        _dropdown.innerHTML = matches
            .map(s => `<li data-subject="${esc(s)}"><span class="ac-name">${esc(s)}</span></li>`)
            .join('');
        _dropdown.hidden = false;
        _dropdown.querySelectorAll('li').forEach(li => {
            li.addEventListener('mousedown', e => {
                e.preventDefault();
                inputEl.value = li.dataset.subject;
                _dropdown.hidden = true;
            });
        });
    }

    function closeDropdown() {
        if (_dropdown) _dropdown.hidden = true;
    }

    inputEl.addEventListener('focus', () => buildDropdown(getSubjects()));
    inputEl.addEventListener('input', () => buildDropdown(getSubjects()));
    inputEl.addEventListener('blur',  () => setTimeout(closeDropdown, 150));
    inputEl.addEventListener('keydown', e => {
        if (e.key === 'Escape') { e.preventDefault(); closeDropdown(); }
        if (e.key === 'Enter')
            setTimeout(closeDropdown, 0);
    });
}

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
    const subjectInput = document.getElementById('bulk-tag-subject');
    const tagStr = input.value.trim();
    if (!tagStr) return;
    const subject = subjectInput?.value.trim() || undefined;
    const paths = [...state.selectedPaths];
    const status = document.getElementById('bulk-status');
    status.textContent = 'Adding...';
    const body = (p) => {
        const b = { path: p, tags: [tagStr], dir: currentAbsDir() };
        if (subject) b.subject = subject;
        return b;
    };
    await Promise.all(paths.map(p => apiPost('/api/tag', body(p))));
    // Refresh cached data for all selected files
    await Promise.all(paths.map(async p => {
        const data = await api('/api/file?path=' + encodeURIComponent(p) + dirParam('&'));
        state.selectedFilesData.set(p, data);
    }));
    await loadTags();
    if (state.mode === 'browse') await loadFiles(state.currentPath);
    input.value = '';
    status.textContent = `Added "${tagStr}"${subject ? ` [${subject}]` : ''} to ${paths.length} file${paths.length === 1 ? '' : 's'}.`;
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

function toggleMoreMenu(e) {
    e.stopPropagation();
    const menu = document.getElementById('more-menu');
    menu.hidden = !menu.hidden;
}

document.addEventListener('click', () => {
    const menu = document.getElementById('more-menu');
    if (menu) menu.hidden = true;
});

async function clearCache(all = false) {
    // Close dropdown if open
    const menu = document.getElementById('more-menu');
    if (menu) menu.hidden = true;

    // A root must always be explicitly selected. Never write to other roots.
    if (state.currentBasePath == null) {
        showToast('No database selected — navigate into a database first');
        return;
    }

    const btn = document.getElementById('more-btn');
    btn.disabled = true;
    const toast = showToast(all ? t('toast.cache-cleared') + '\u2026' : t('toast.page-cache-cleared') + '\u2026', 0);
    let success = false;
    let errorMsg = t('toast.cache-clear-failed');
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
            errorMsg = t('toast.cache-clear-failed') + ': ' + (await resp.text()).trim();
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
        showToast(success ? (all ? t('toast.cache-cleared') : t('toast.page-cache-cleared')) : errorMsg);
        if (state.mode === 'search') {
            await doSearch();
        } else {
            await loadFiles(state.currentPath);
        }
        render();
    }
}

async function pregenSprites() {
    const menu = document.getElementById('more-menu');
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
        updateToast(toast, t('toolbar.sprites-gen') + ` (${done} / ${videoPaths.length})`);
        try {
            const minN = state.settings.sprite_min ?? 8;
            const maxN = state.settings.sprite_max ?? 16;
            await fetch('/api/vthumbs?' + new URLSearchParams({ path, min_n: minN, max_n: maxN }) + dirParam('&'));
        } catch (_) { /* ignore */ }
        done++;
        _trickplayCache.delete(path);
    }

    dismissToast(toast);
    showToast(t('toast.sprites-done'));
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

const AI_VIDEO_EXTS = new Set([
    'mp4','mov','avi','mkv','wmv','m4v','webm','flv','mpg','mpeg','m2ts','mts','ts','3gp','f4v',
]);

function isAiImage(path) {
    const ext = (path || '').split('.').pop().toLowerCase();
    return AI_IMAGE_EXTS.has(ext) || AI_ARCHIVE_EXTS.has(ext) || AI_VIDEO_EXTS.has(ext);
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
    const menu = document.getElementById('more-menu');
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
        document.getElementById('ai-subject').value = cfg.subject || '';
        document.getElementById('ai-prompt-image').value = cfg.prompt_image || '';
        document.getElementById('ai-prompt-image').placeholder = cfg.default_prompt_image || '';
        document.getElementById('ai-prompt-video').value = cfg.prompt_video || '';
        document.getElementById('ai-prompt-video').placeholder = cfg.default_prompt_video || 'Look at this video contact sheet.';
        document.getElementById('ai-video-max-mb') && (document.getElementById('ai-video-max-mb').value = cfg.video_max_mb ?? 50);
        document.getElementById('ai-video-sheet-max-frames').value = cfg.video_sheet_max_frames ?? 16;
        document.getElementById('ai-video-frame-selection').value = cfg.video_frame_selection || 'interval';
        _updateVideoMaxMbVisibility();
        document.getElementById('ai-prompt-archive').value = cfg.prompt_archive || '';
        document.getElementById('ai-prompt-archive').placeholder = cfg.default_prompt_archive || '';
        const pre = document.getElementById('ai-output-format');
        if (pre) {
            pre.value = cfg.output_format || '';
            pre.placeholder = cfg.default_output_format || '';
        }
        document.getElementById('ai-format').value = cfg.format || 'openai';
        const enabled = cfg.enabled !== false; // default true if key absent
        document.getElementById('ai-enabled').checked = enabled;
        document.getElementById('ai-settings-fields').hidden = !enabled;
    } catch (_) { /* defaults are fine */ }
    document.getElementById('ai-test-result').hidden = true;
    // Language selector — populate on demand so it works regardless of load order.
    const langSel = document.getElementById('lang-select');
    if (langSel) {
        if (!langSel.options.length) {
            langSel.innerHTML = LANG_OPTIONS.map(o =>
                `<option value="${o.code}">${o.label}</option>`
            ).join('');
        }
        langSel.value = getLang();
    }
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

function _updateVideoMaxMbVisibility() {
    // Full video mode is disabled; max-MB row is always hidden.
    const row = document.getElementById('ai-video-max-mb-row');
    if (row) row.hidden = true;
}

function aiVideoModeChanged() {
    _updateVideoMaxMbVisibility();
}

function aiToggleEnabled() {
    const enabled = document.getElementById('ai-enabled').checked;
    document.getElementById('ai-settings-fields').hidden = !enabled;
}

function aiUseDefault(type) {
    if (type === 'output-format') {
        const el = document.getElementById('ai-output-format');
        if (el) el.value = el.placeholder;
        return;
    }
    if (type === 'video') {
        // Use the mode-specific placeholder as the default.
        const el = document.getElementById('ai-prompt-video');
        if (el) el.value = el.placeholder;
        return;
    }
    const el = document.getElementById('ai-prompt-' + type);
    if (el) el.value = el.placeholder;
}

async function aiSaveSettings() {
    try {
    const body = {
        endpoint: document.getElementById('ai-endpoint').value.trim(),
        model: document.getElementById('ai-model').value.trim(),
        subject: document.getElementById('ai-subject').value,
        prompt_image: document.getElementById('ai-prompt-image').value,
        prompt_video: document.getElementById('ai-prompt-video').value,
        prompt_archive: document.getElementById('ai-prompt-archive').value,
        output_format: document.getElementById('ai-output-format').value,
        tag_prefix: document.getElementById('ai-tag-prefix').value.trim(),
        max_tokens: parseInt(document.getElementById('ai-max-tokens').value, 10) || 512,
        format: document.getElementById('ai-format').value,
        video_mode: 'sprite',
        video_sheet_max_frames: parseInt(document.getElementById('ai-video-sheet-max-frames').value, 10) || 16,
        video_frame_selection: document.getElementById('ai-video-frame-selection').value,
        enabled: document.getElementById('ai-enabled').checked,
        dir: currentAbsDir(),
    };
    const apiKey = document.getElementById('ai-api-key').value;
    if (apiKey) body.api_key = apiKey;
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
    const toast = showToast(t('toast.promoting', {tag: newTagStr}), 0);
    try {
        // Add the promoted tag, then remove the ai/ original.
        await apiPost('/api/tag', { path, tags: [newTagStr], dir: currentAbsDir() });
        const origStr = value ? `${tagName}=${value}` : tagName;
        await apiPost('/api/untag', { path, tags: [origStr], dir: currentAbsDir() });
        await loadFileDetail(path);
        await loadTags();
        renderTags();
        renderDetailTagsOnly();
        _updateCardTagBadges();
    } catch (e) {
        showToast(t('toast.promote-failed', {err: e.message}));
    } finally {
        dismissToast(toast);
    }
}

/// Remove all ai/* tags from given paths.
async function aiClearTags(paths) {
    const np = paths.length;
    const toast = showToast(t('toast.removing-ai-tags', {n: np, plural: np !== 1 ? t('toast.removing-ai-plural') : ''}), 0);
    try {
        await apiPost('/api/ai/clear-tags', { paths, dir: currentAbsDir() });
        await loadTags();
        renderTags();
        await refreshSelectedFilesData();
        renderDetail();
        _updateCardTagBadges();
        dismissToast(toast);
        showToast(t('toast.ai-tags-removed'));
    } catch (e) {
        dismissToast(toast);
        showToast(t('toast.remove-failed', {err: e.message}));
    }
}

/// Promote all ai/* tags on the given paths: add tags without ai/ prefix,
/// then remove the original ai/ tags.
async function aiAcceptAllTags(paths) {
    const nap = paths.length;
    const toast = showToast(t('toast.accepting-ai-tags', {n: nap, plural: nap !== 1 ? t('toast.accepting-plural') : ''}), 0);
    try {
        let accepted = 0;
        for (const path of paths) {
            let data = state.selectedFilesData.get(path);
            if (!data || !Array.isArray(data.tags)) {
                data = await api('/api/file?path=' + encodeURIComponent(path) + dirParam('&'));
            }

            const aiTags = (data.tags || []).filter(tag => tag.name && tag.name.startsWith('ai/'));
            if (aiTags.length === 0) continue;

            const promoted = aiTags
                .map(tag => {
                    const bare = tag.name.slice('ai/'.length);
                    if (!bare) return null;
                    return tag.value ? `${bare}=${tag.value}` : bare;
                })
                .filter(Boolean);

            if (promoted.length === 0) continue;

            const originals = aiTags.map(tag => (tag.value ? `${tag.name}=${tag.value}` : tag.name));
            await apiPost('/api/tag', { path, tags: promoted, dir: currentAbsDir() });
            await apiPost('/api/untag', { path, tags: originals, dir: currentAbsDir() });

            accepted += originals.length;
            const refreshed = await api('/api/file?path=' + encodeURIComponent(path) + dirParam('&'));
            state.selectedFilesData.set(path, refreshed);
            if (state.selectedFile?.path === path) state.selectedFile = refreshed;
        }

        await loadTags();
        renderTags();
        renderDetail();
        _updateCardTagBadges();
        dismissToast(toast);
        showToast(accepted > 0 ? t('toast.accepted', {n: accepted, plural: accepted !== 1 ? t('toast.accepted-plural') : ''}) : t('toast.no-ai-tags'));
    } catch (e) {
        dismissToast(toast);
        showToast(t('toast.accept-failed', {err: e.message}));
    }
}

async function aiAnalyseSingle(path) {
    if (state.aiAnalysing.has(path)) return; // already running
    state.aiAnalysing.add(path);
    // Re-render so the button shows "Analysing…" immediately (also persists on navigate-away & back)
    if (state.selectedFile?.path === path) renderDetail();
    const toast = showToast(t('toast.ai-analysing'), 0);
    const autoFramesEl = document.getElementById('ai-frames-auto');
    if (autoFramesEl) aiSetVideoFramesAuto(autoFramesEl.checked);
    const framesEl = document.getElementById('ai-frames-input');
    const n_frames = state.aiVideoFramesAuto
        ? null
        : (framesEl
            ? aiSetVideoFrames(framesEl.value)
            : (Number.isFinite(state.aiVideoFrames) ? state.aiVideoFrames : null));
    try {
        const res = await fetch('/api/ai/analyse', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ path, dir: currentAbsDir(), ...(n_frames ? { n_frames } : {}) }),
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || res.statusText);
        const n = (data.tags || []).length;
        dismissToast(toast);
        const nTagsMsg = t('toast.ai-n-tags', {n, plural: n !== 1 ? t('toast.ai-tags-plural') : ''});
        if (data.warning) {
            showToast(nTagsMsg + ` (⚠ ${data.warning})`, 8000);
        } else {
            showToast(nTagsMsg);
        }
        if (state.selectedFile?.path === path) {
            await loadFileDetail(path);
            await loadTags();
            renderTags();
        }
    } catch (e) {
        dismissToast(toast);
        showToast(t('toast.ai-error', {err: e.message}));
    } finally {
        state.aiAnalysing.delete(path);
        if (state.selectedFile?.path === path) renderDetail();
    }
}

/// Per-file AI analysis for all currently selected files (sequential).
async function aiAnalyseSelected() {
    const paths = [...state.selectedPaths].filter(p => isAiImage(p));
    if (paths.length === 0) {
        showToast(t('toast.ai-no-images'));
        return;
    }
    const toast = showToast(t('toast.ai-analysing-n', {done: 0, total: paths.length}), 0);
    toast.classList.add('toast-progress');
    let done = 0;
    let errors = 0;
    for (const path of paths) {
        if (state.aiAnalysing.has(path)) { done++; continue; }
        state.aiAnalysing.add(path);
        renderDetail();
        updateToast(toast, t('toast.ai-analysing-n', {done, total: paths.length}));
        try {
            const res = await fetch('/api/ai/analyse', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ path, dir: currentAbsDir() }),
            });
            const data = await res.json();
            if (!res.ok) throw new Error(data.error || res.statusText);
        } catch (_) {
            errors++;
        } finally {
            state.aiAnalysing.delete(path);
            done++;
        }
    }
    dismissToast(toast);
    const msg = errors > 0
        ? t('toast.ai-errors', {done: done - errors, errors, plural: errors !== 1 ? t('toast.ai-errors-plural') : ''})
        : t('toast.ai-analysed-n', {n: done, plural: done !== 1 ? t('toast.ai-analysed-plural') : ''});
    showToast(msg);
    await loadTags();
    renderTags();
    await refreshSelectedFilesData();
    renderDetail();
    _updateCardTagBadges();
}

/// Common-traits AI analysis: send all selected images to the VLM together
/// and apply only the shared tags to every selected file.
async function aiAnalyseCommonTraits() {
    const paths = [...state.selectedPaths];
    if (paths.length === 0) {
        showToast(t('toast.ai-no-images'));
        return;
    }
    const toast = showToast(t('toast.ai-common-analysing'), 0);
    toast.classList.add('toast-progress');
    try {
        const res = await fetch('/api/ai/analyse-common', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ paths, dir: currentAbsDir() }),
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || res.statusText);
        const n = (data.tags || []).length;
        const m = data.applied_count || 0;
        dismissToast(toast);
        showToast(t('toast.ai-common-done', {n, plural: n !== 1 ? t('toast.ai-common-plural') : '', m, plural2: m !== 1 ? t('toast.ai-common-plural2') : ''}));
        await loadTags();
        renderTags();
        await refreshSelectedFilesData();
        renderDetail();
        _updateCardTagBadges();
    } catch (e) {
        dismissToast(toast);
        showToast(t('toast.ai-error', {err: e.message}));
    }
}

/** Keep the chat panel to the left of the detail panel when it is visible. */
function _syncChatRight() {
    const panel = document.getElementById('chat-panel');
    if (panel && panel.dataset.free === '1') return; // free-floating, leave alone
    const w = state.detailOpen
        ? getComputedStyle(document.documentElement).getPropertyValue('--detail-width').trim()
        : '0px';
    document.documentElement.style.setProperty('--chat-r', w);
}

function aiSetVideoFrames(rawValue) {
    const parsed = parseInt(rawValue, 10);
    if (!Number.isFinite(parsed)) return state.aiVideoFrames;
    const clamped = Math.max(2, Math.min(256, parsed));
    state.aiVideoFrames = clamped;
    // Keep chat-panel bar in sync.
    const chatInput = document.getElementById('chat-frames-input');
    if (chatInput) chatInput.value = clamped;
    return clamped;
}

function aiSetVideoFramesAuto(enabled) {
    state.aiVideoFramesAuto = !!enabled;
    const input = document.getElementById('ai-frames-input');
    if (input) input.disabled = state.aiVideoFramesAuto;
    // Keep chat-panel bar in sync.
    const chatAuto  = document.getElementById('chat-frames-auto');
    const chatInput = document.getElementById('chat-frames-input');
    if (chatAuto)  chatAuto.checked  = state.aiVideoFramesAuto;
    if (chatInput) chatInput.disabled = state.aiVideoFramesAuto;
    return state.aiVideoFramesAuto;
}

async function aiAnalyseBatch() {
    const menu = document.getElementById('more-menu');
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
        showToast(t('toast.ai-none-in-view'));
        return;
    }

    const btn = document.getElementById('ai-analyse-btn');
    if (btn) btn.disabled = true;

    const toast = showToast(t('toast.ai-analysing'), 0);
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
                updateToast(toast, t('toast.ai-analysing-n', {done: prog.done, total: prog.total}));
                if (!prog.running) {
                    clearInterval(poll);
                    dismissToast(toast);
                    const nd = prog.done || 0;
                    const fb = prog.fallback_count || 0;
                    const msg = t('toast.ai-done', {n: nd, plural: nd !== 1 ? t('toast.ai-done-plural') : ''})
                        + (fb > 0 ? t('toast.ai-fallback', {n: fb, plural: fb !== 1 ? t('toast.ai-fallback-plural') : ''}) : '');
                    showToast(msg, fb > 0 ? 8000 : 3000);
                    if (btn) btn.disabled = false;
                    await loadTags();
                    renderTags();
                    await refreshSelectedFilesData();
                    renderDetail();
                    _updateCardTagBadges();
                }
            } catch (_) {
                clearInterval(poll);
                dismissToast(toast);
                if (btn) btn.disabled = false;
            }
        }, 2000);
    } catch (e) {
        dismissToast(toast);
        showToast(t('toast.ai-error', {err: e.message}));
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
    renderTags();
}

/// Re-fetch file detail for every path currently in selectedFilesData so the
/// bulk tag panel reflects freshly applied tags without requiring a deselect.
async function refreshSelectedFilesData() {
    await Promise.all([...state.selectedFilesData.keys()].map(async p => {
        try {
            const res = await fetch('/api/file?path=' + encodeURIComponent(p) + dirParam('&'));
            if (res.ok) {
                const data = await res.json();
                state.selectedFilesData.set(p, data);
                if (state.selectedFile?.path === p) state.selectedFile = data;
            }
        } catch (_) {}
    }));
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
    _syncChatRight();
    restoreScrollAnchor(anchor);
}

// ---------------------------------------------------------------------------
// Cache Manager modal
// ---------------------------------------------------------------------------

function openCacheManager() {
    const menu = document.getElementById('more-menu');
    if (menu) menu.hidden = true;
    document.getElementById('cache-manager-modal').hidden = false;
    document.getElementById('cm-status').textContent = '';
    loadCacheInfo();
}

function closeCacheManager() {
    document.getElementById('cache-manager-modal').hidden = true;
}

async function loadCacheInfo() {
    const listEl = document.getElementById('cm-subdir-list');
    const totalEl = document.getElementById('cm-total-label');
    if (!listEl) return;
    if (state.currentBasePath == null) {
        listEl.innerHTML = `<div class="cm-loading">${esc(t('cm.empty'))}</div>`;
        return;
    }
    listEl.innerHTML = `<div class="cm-loading">${esc(t('cm.loading'))}</div>`;
    try {
        const data = await api('/api/cache/info' + dirParam('?'));
        if (!data.subdirs || data.subdirs.length === 0) {
            listEl.innerHTML = `<div class="cm-loading">${esc(t('cm.empty'))}</div>`;
            if (totalEl) totalEl.textContent = '';
            return;
        }
        const CACHE_DIR_LABELS_BY_LANG = {
            thumbs:     {en:'Thumbnails',       nl:'Miniaturen',      de:'Miniaturbilder',  fr:'Miniatures',       es:'Miniaturas',    it:'Anteprime',      pt:'Miniaturas',    pl:'Miniatury',      sv:'Miniatyrer'},
            raw:        {en:'RAW previews',     nl:'RAW-voorbeelden', de:'RAW-Vorschauen',  fr:'Aperçus RAW',      es:'Vistas previas RAW', it:'Anteprime RAW', pt:'Pré-visualizações RAW', pl:'Podglądy RAW', sv:'RAW-förhandsvisningar'},
            vthumbs:    {en:'Video sprites',    nl:'Video-sprites',   de:'Video-Sprites',   fr:'Sprites vidéo',    es:'Sprites de vídeo', it:'Sprite video', pt:'Sprites de vídeo', pl:"Sprite'y wideo", sv:'Videospritar'},
            ai_sprites: {en:'AI sprites',       nl:'AI-sprites',      de:'KI-Sprites',      fr:'Sprites IA',       es:'Sprites de IA', it:'Sprite IA',      pt:'Sprites de IA', pl:"Sprite'y AI",    sv:'AI-spritar'},
            hls2:       {en:'HLS video files',  nl:'HLS-videobestanden', de:'HLS-Videodateien', fr:'Fichiers HLS', es:'Archivos HLS',  it:'File HLS',       pt:'Ficheiros HLS', pl:'Pliki HLS',      sv:'HLS-filer'},
            video:      {en:'Video transcodes', nl:'Video-transcodes', de:'Transkodierungen', fr:'Transcodages',   es:'Transcodificaciones', it:'Transcodifiche', pt:'Transcodificações', pl:'Transkodowania', sv:'Transkodningar'},
        };
        function cacheDirLabel(name) {
            const map = CACHE_DIR_LABELS_BY_LANG[name];
            if (!map) return name;
            const lang = getLang();
            return map[lang] || map.en || name;
        }
        let html = '';
        for (const sd of data.subdirs) {
            const label = cacheDirLabel(sd.name);
            const size = formatSize(sd.size);
            const count = sd.count.toLocaleString();
            html += `<div class="cm-subdir-row">
                <div class="cm-subdir-info">
                    <span class="cm-subdir-name">${esc(label)}</span>
                    <span class="cm-subdir-meta">${count} &middot; ${size}</span>
                </div>
                <button class="cm-btn cm-btn-sm" onclick="doCacheClearSubdir('${jesc(sd.name)}')">${esc(t('cm.clear-btn'))}</button>
            </div>`;
        }
        listEl.innerHTML = html;
        const totalCount = data.total_count || data.subdirs.reduce((s, d) => s + d.count, 0);
        if (totalEl) totalEl.textContent = t('cm.total', {size: formatSize(data.total), n: totalCount, plural: t('cm.total-plural')});
    } catch (e) {
        listEl.innerHTML = `<div class="cm-loading">${esc(e.message || String(e))}</div>`;
    }
}

async function doCachePrune() {
    if (state.currentBasePath == null) return;
    const btn = document.getElementById('cm-prune-btn');
    const statusEl = document.getElementById('cm-status');
    if (btn) btn.disabled = true;
    if (statusEl) statusEl.textContent = t('cm.prune-btn') + '\u2026';
    try {
        const resp = await fetch('/api/cache/prune' + dirParam('?'), {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: '{}',
        });
        const data = await resp.json();
        const freed = formatSize(data.freed || 0);
        const n = data.removed || 0;
        if (statusEl) statusEl.textContent = t('cm.pruned', {n, plural: n !== 1 ? t('cm.pruned-plural') : '', freed});
        _thumbClearCache();
        await loadCacheInfo();
    } catch (e) {
        if (statusEl) statusEl.textContent = e.message || String(e);
    } finally {
        if (btn) btn.disabled = false;
    }
}

async function doCacheClearSubdir(subdir) {
    if (state.currentBasePath == null) return;
    const statusEl = document.getElementById('cm-status');
    if (statusEl) statusEl.textContent = t('cm.loading');
    try {
        await fetch('/api/cache/clear-subdir' + dirParam('?'), {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ subdir }),
        });
        _thumbClearCache();
        if (statusEl) statusEl.textContent = t('toast.cache-cleared');
        await loadCacheInfo();
    } catch (e) {
        if (statusEl) statusEl.textContent = e.message || String(e);
    }
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

function toggleTagSortMode() {
    const modes = ['groups-first', 'alpha', 'count'];
    state.tagSortMode = modes[(modes.indexOf(state.tagSortMode) + 1) % modes.length];
    const btn = document.getElementById('sidebar-sort-btn');
    if (btn) {
        const labels = { 'groups-first': 'Groups first', 'alpha': 'A–Z', 'count': 'By count' };
        const titles = { 'groups-first': 'Sort: groups first', 'alpha': 'Sort: A–Z', 'count': 'Sort: by count' };
        btn.title = titles[state.tagSortMode];
        btn.classList.toggle('active', state.tagSortMode !== 'alpha');
        // Update the label text node
        const label = btn.querySelector('.sort-label');
        if (label) label.textContent = labels[state.tagSortMode];
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
    _syncChatRight();
    _updateCardSelection();
    renderDetail();
    restoreScrollAnchor(anchor);
}

async function doLogout() {
    try {
        await fetch('/logout', { method: 'POST' });
    } catch (_) { /* ignore */ }
    window.location.href = '/login';
}

// ---------------------------------------------------------------------------
// Tag Picker mode: apply multiple tags at once from the sidebar
// ---------------------------------------------------------------------------

/// Collect all leaf tag names under a tree node map (recursively).
function _collectLeafTags(nodeMap, out = []) {
    for (const node of nodeMap.values()) {
        if (node.tag && !node.tag.has_values) out.push(node.fullPath);
        if (node.children.size) _collectLeafTags(node.children, out);
    }
    return out;
}

/// True if any descendant tag in this node map is currently picked.
function _anyDescendantPicked(nodeMap) {
    for (const node of nodeMap.values()) {
        if (node.tag && state.tagPickerPicks.has(node.fullPath)) return true;
        if (_anyDescendantPicked(node.children)) return true;
    }
    return false;
}

/// Enter the tag picker mode.
/// Pre-checks tags already present on the selected file (or intersection of selected files).
function enterTagPickerMode() {
    if (state.tagPickerMode) {
        cancelTagPickerMode();
        return;
    }
    state.tagPickerMode = true;
    state.tagPickerPicks = new Set();
    state.tagPickerOriginal = new Set();

    // Pre-check tags already on the current selection.
    if (state.selectedPaths.size > 1) {
        // Multi-select: intersect tags that ALL selected files have.
        const tagSets = [...state.selectedFilesData.values()].map(d =>
            new Set((d.tags || []).map(t => t.name))
        );
        if (tagSets.length > 0) {
            for (const tagName of tagSets[0]) {
                if (tagSets.every(s => s.has(tagName))) {
                    state.tagPickerPicks.add(tagName);
                    state.tagPickerOriginal.add(tagName);
                }
            }
        }
    } else if (state.selectedFile) {
        for (const t of (state.selectedFile.tags || [])) {
            if (t.name) {
                state.tagPickerPicks.add(t.name);
                state.tagPickerOriginal.add(t.name);
            }
        }
    }

    renderTags();
}

/// Toggle a single tag in picker mode, then re-render the bar.
function toggleTagPick(tagName) {
    if (state.tagPickerPicks.has(tagName)) {
        state.tagPickerPicks.delete(tagName);
    } else {
        state.tagPickerPicks.add(tagName);
    }
    renderTags();
}

/// In picker mode, clicking a group-name toggles all leaf tags under that prefix.
function toggleTagGroupPick(prefix) {
    const tree = buildTagTree(state.tags);
    // Walk to the node for this prefix.
    let nodeMap = tree;
    for (const seg of prefix.split('/')) {
        if (!nodeMap.has(seg)) return;
        const n = nodeMap.get(seg);
        nodeMap = n.children;
    }
    const leaves = _collectLeafTags(nodeMap);
    // Also include the prefix itself if it has a tag.
    const node = (() => {
        let m = tree;
        let nd = null;
        for (const seg of prefix.split('/')) {
            nd = m.get(seg);
            if (!nd) return null;
            m = nd.children;
        }
        return nd;
    })();
    if (node?.tag && !node.tag.has_values) leaves.push(prefix);

    // If ALL are picked, un-pick; otherwise pick all.
    const allPicked = leaves.length > 0 && leaves.every(t => state.tagPickerPicks.has(t));
    for (const t of leaves) {
        if (allPicked) state.tagPickerPicks.delete(t);
        else state.tagPickerPicks.add(t);
    }
    renderTags();
}

/// Apply the delta: add newly-checked tags, remove unchecked tags from all target files.
async function applyTagPicker() {
    const paths = state.selectedPaths.size > 0
        ? [...state.selectedPaths]
        : state.selectedFile ? [state.selectedFile.path] : [];
    if (!paths.length) return;

    const toAdd    = [...state.tagPickerPicks].filter(t => !state.tagPickerOriginal.has(t));
    const toRemove = [...state.tagPickerOriginal].filter(t => !state.tagPickerPicks.has(t));

    if (toAdd.length === 0 && toRemove.length === 0) {
        cancelTagPickerMode();
        return;
    }

    const dir = currentAbsDir();
    await Promise.all([
        ...paths.flatMap(p => toAdd.map(t => apiPost('/api/tag',   { path: p, tags: [t], dir }))),
        ...paths.flatMap(p => toRemove.map(t => apiPost('/api/untag', { path: p, tags: [t], dir }))),
    ]);

    const parts = [];
    if (toAdd.length)    parts.push(`+${toAdd.length} tag${toAdd.length === 1 ? '' : 's'}`);
    if (toRemove.length) parts.push(`-${toRemove.length} tag${toRemove.length === 1 ? '' : 's'}`);
    showToast(`${parts.join(', ')} on ${paths.length} file${paths.length === 1 ? '' : 's'}.`);

    cancelTagPickerMode();
    await loadTags();
    if (state.selectedFile) await loadFileDetail(state.selectedFile.path);
    renderDetailTagsOnly();
    renderTags();
    _updateCardTagBadges();
}

/// Exit picker mode without applying changes.
function cancelTagPickerMode() {
    state.tagPickerMode = false;
    state.tagPickerPicks = new Set();
    state.tagPickerOriginal = new Set();
    renderTags();
}
