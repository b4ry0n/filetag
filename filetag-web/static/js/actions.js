// ---------------------------------------------------------------------------
// Navigation history  (Alt+Left / Alt+Right, and zip sub-directory traversal)
// ---------------------------------------------------------------------------

let _navHistory   = [];   // array of state snapshots
let _navHistoryIdx = -1;  // current position
let _navRestoring  = false; // suppresses recursive pushes during restore

/** Update the enabled/disabled state of the nav-back and nav-forward buttons. */
function _navUpdateButtons() {
    const back    = document.getElementById('nav-back');
    const forward = document.getElementById('nav-forward');
    if (back)    back.disabled    = (_navHistoryIdx <= 0);
    if (forward) forward.disabled = (_navHistoryIdx >= _navHistory.length - 1);
}

/** Capture the current navigation state and push it onto the history stack. */
function _navPush() {
    if (_navRestoring) return;
    // Save the scroll position of the page we're navigating away from.
    const _content = document.getElementById('content');
    if (_content && _navHistoryIdx >= 0 && _navHistory[_navHistoryIdx]) {
        _navHistory[_navHistoryIdx].scrollTop = _content.scrollTop;
    }
    const snap = {
        mode:            state.mode,
        currentPath:     state.currentPath,
        currentRootId:   state.currentRootId,
        currentBasePath: state.currentBasePath,
        zipPath:         state.zipPath,
        zipRootId:       state.zipRootId ?? null,
        zipSubdir:       state.zipSubdir,
        searchQuery:     state.searchQuery,
    };
    // Discard any forward history beyond the current position.
    _navHistory = _navHistory.slice(0, _navHistoryIdx + 1);
    _navHistory.push(snap);
    if (_navHistory.length > 50) { _navHistory.shift(); } // cap size
    _navHistoryIdx = _navHistory.length - 1;
    _navUpdateButtons();
}

/** Restore a previously saved snapshot without recording a new push. */
async function _navRestore(snap) {
    _navRestoring = true;
    try {
        _thumbClearCache();
        _kbCursor = -1;
        state.selectedFile = null;
        state.selectedDir  = null;
        state.selectedPaths.clear();
        state.selectedFilesData.clear();
        state.activeTags.clear();
        state.activeSubjects.clear();
        state.activePeople.clear();
        _lastClickedPath = null;
        _armedBulkTag    = null;
        if (snap.mode === 'zip') {
            state.currentRootId   = snap.currentRootId ?? null;
            state.currentBasePath = snap.currentBasePath;
            state.currentPath     = snap.currentPath;
            state.zipPath         = snap.zipPath;
            state.zipRootId       = snap.zipRootId ?? snap.currentRootId ?? null;
            state.zipSubdir       = snap.zipSubdir || '';
            state.mode            = 'zip';
            const params = new URLSearchParams({ path: snap.zipPath });
            if (state.zipRootId != null) params.set('root_id', String(state.zipRootId));
            const data = await api('/api/zip/entries?' + params.toString());
            state.zipEntries = data.entries || [];
        } else if (snap.mode === 'search') {
            await searchFiles(snap.searchQuery);
        } else {
            await loadFiles(snap.currentPath);
            await loadSettings();
        }
    } finally {
        _navRestoring = false;
    }
    render();
    const _rc = document.getElementById('content');
    if (_rc && snap.scrollTop) _rc.scrollTop = snap.scrollTop;
}

/** Go back one step in the navigation history (Alt+Left). */
async function navBack() {
    if (_navHistoryIdx <= 0) return;
    _navHistoryIdx--;
    await _navRestore(_navHistory[_navHistoryIdx]);
    _navUpdateButtons();
}

/** Go forward one step in the navigation history (Alt+Right). */
async function navForward() {
    if (_navHistoryIdx >= _navHistory.length - 1) return;
    _navHistoryIdx++;
    await _navRestore(_navHistory[_navHistoryIdx]);
    _navUpdateButtons();
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

async function navigateTo(path) {
    closeMobileSidebar(); // auto-close the mobile drawer when navigating
    _thumbClearCache();
    _kbCursor = -1;
    state.selectedFile = null;
    state.selectedDir = null;
    state.selectedPaths.clear();
    state.selectedFilesData.clear();
    state.activeTags.clear();
    state.activeSubjects.clear();
    state.activePeople.clear();
    if ('faceActivePerson' in state) state.faceActivePerson = null;
    _lastClickedPath = null;
    _armedBulkTag = null;

    // Show a spinner immediately so the user gets instant feedback that the
    // double-click was registered, before the server responds.
    const _contentEl = document.getElementById('content');
    if (_contentEl) {
        _contentEl.className = '';
        _contentEl.innerHTML = '<div class="nav-loading"><div class="nav-loading-spinner"></div></div>';
    }

    const _prevRootId = state.currentRootId;
    await loadFiles(path);
    _navPush(); // record this directory in the navigation history
    // When navigating into a different database root (e.g. a child DB), reload
    // tags so the sidebar reflects the correct counts for the new root.
    if (state.currentRootId !== _prevRootId) {
        loadTags().then(() => renderTags()).catch(() => {});
    }
    await loadSettings();
    if (typeof loadFaceConfig === 'function') {
        Promise.all([loadFaceConfig(), loadPeople()]).then(() => renderTags()).catch(() => {});
    } else if (typeof loadPeople === 'function') {
        loadPeople().then(() => renderTags()).catch(() => {});
    }
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
        const root = state.roots.find(r => r.path === rootPath);
        const infoUrl = root != null
            ? '/api/info?root_id=' + root.id
            : '/api/info?dir=' + encodeURIComponent(rootPath);
        state.selectedRootInfo = await api(infoUrl);
        renderDetail();
    } catch (_) { /* ignore */ }
}

// Enter a specific root database (from the virtual root listing).
async function enterRoot(rootPath) {
    _thumbClearCache();
    _kbCursor = -1;
    const root = state.roots.find(r => r.path === rootPath);
    state.currentRootId = root ? root.id : null;
    state.currentBasePath = rootPath;
    state.currentPath = '';
    state.selectedFile = null;
    state.selectedDir = null;
    state.selectedRoot = null;
    state.selectedRootInfo = null;
    state.selectedPaths.clear();
    state.selectedFilesData.clear();
    state.activeTags.clear();
    state.activeSubjects.clear();
    state.activePeople.clear();
    _lastClickedPath = null;
    _armedBulkTag = null;

    // Show spinner immediately so the user gets feedback before the server responds.
    const _contentEl = document.getElementById('content');
    if (_contentEl) {
        _contentEl.className = '';
        _contentEl.innerHTML = '<div class="nav-loading"><div class="nav-loading-spinner"></div></div>';
    }

    await Promise.all([loadInfo(), loadTags(), loadFiles(''), loadSettings()]);
    render();
}

// Navigate back to the virtual root (show all roots).
async function goVirtualRoot() {
    _thumbClearCache();
    _kbCursor = -1;
    state.currentRootId = null;
    state.currentBasePath = null;
    state.currentPath = '';
    state.selectedFile = null;
    state.selectedDir = null;
    state.selectedPaths.clear();
    state.selectedFilesData.clear();
    state.activeTags.clear();
    state.activeSubjects.clear();
    state.activePeople.clear();
    state.info = null;
    _lastClickedPath = null;
    _armedBulkTag = null;

    // Show spinner immediately.
    const _contentEl = document.getElementById('content');
    if (_contentEl) {
        _contentEl.className = '';
        _contentEl.innerHTML = '<div class="nav-loading"><div class="nav-loading-spinner"></div></div>';
    }

    await Promise.all([loadFiles(''), loadTags()]);
    render();
}


async function doSearch() {
    _thumbClearCache();
    const input = document.getElementById('search-input');
    const query = input.value.trim();
    if (!query) return;
    state.activeTags.clear();
    state.activeSubjects.clear();
    state.activePeople.clear();
    await searchFiles(query);
    document.getElementById('search-clear').hidden = false;
    render();
}

function doClearSearch() {
    _thumbClearCache();
    state.activeTags.clear();
    state.activeSubjects.clear();
    state.activePeople.clear();
    if ('faceActivePerson' in state) state.faceActivePerson = null;
    document.getElementById('search-input').value = '';
    document.getElementById('search-clear').hidden = true;
    // When the Tags tab is active: don't navigate to the browse view — just
    // show the empty-state so the user must select a tag to see files.
    if (!state.sidebarSplit && state.sidebarTab === 'tags') {
        state.mode = 'browse';
        state.searchResults = [];
        state.searchResultRoots = new Map();
        renderContent();
        return;
    }
    navigateTo(state.currentPath || '');
}

function navigateToParent(filePath) {
    const parts = filePath.split('/');
    const dir = parts.length > 1 ? parts.slice(0, -1).join('/') : '';
    // In multi-root search the file may belong to a different root than the
    // currently active one — switch currentRootId so loadFiles targets the
    // correct database root.
    const fileRootId = searchRootIdForPath(filePath);
    if (fileRootId != null && fileRootId !== state.currentRootId) {
        state.currentRootId = fileRootId;
        const rootMeta = state.roots.find(r => r.id === fileRootId);
        if (rootMeta) state.currentBasePath = rootMeta.path;
    }
    document.getElementById('search-input').value = '';
    document.getElementById('search-clear').hidden = true;
    // Switch sidebar to the files/tree pane (unless in split mode both are visible).
    if (!state.sidebarSplit) setSidebarTab('files');
    // Expand the tree to the target directory so it is visible after navigation.
    const absDir = state.currentBasePath
        ? (dir ? state.currentBasePath + '/' + dir : state.currentBasePath)
        : null;
    const doNav = () => {
        if (typeof ftreeRequestScrollToActive === 'function') ftreeRequestScrollToActive();
        return navigateTo(dir).then(() => {
            // renderContent appends tiles in rAF chunks; set the pending target
            // so _appendChunk scrolls to the tile the moment it lands in the DOM.
            window._pendingScrollToTile = filePath;
        });
    };
    if (absDir && typeof ftreeExpandToPath === 'function') {
        ftreeExpandToPath(absDir).then(doNav);
    } else {
        doNav();
    }
}

/// Quote a tag name for the query language if it contains special characters.
function openSettings(tab = 'general') {
    const menu = document.getElementById('more-menu');
    if (menu) menu.hidden = true;
    // Video settings from per-root state.
    document.getElementById('sprite-min').value = state.settings.sprite_min ?? 8;
    document.getElementById('sprite-max').value = state.settings.sprite_max ?? 16;
    const dpsSel = document.getElementById('dir-preview-style');
    if (dpsSel) dpsSel.value = state.settings.dir_preview_style ?? 'crop';
    const tpmSel = document.getElementById('tile-preview-mode');
    if (tpmSel) tpmSel.value = state.settings.tile_preview_mode ?? 'sprite';
    _updateTilePreviewHint();
    const vtdEl = document.getElementById('vtile-duration');
    if (vtdEl) vtdEl.value = state.settings.vtile_duration ?? 8;
    const vtulEl = document.getElementById('vtile-use-longest');
    if (vtulEl) vtulEl.checked = state.settings.vtile_use_longest ?? false;
    // PDF field is always present — populate regardless of active tab.
    document.getElementById('feat-pdf').checked = state.settings.feature_pdf ?? false;
    // Features tab initialisation is deferred until the tab is visible.

    // AI settings from server
    fetch('/api/ai/config?' + new URLSearchParams({ dir: currentAbsDir() || '' }))
        .then(res => res.json())
        .then(cfg => {
            document.getElementById('ai-endpoint').value = cfg.endpoint || '';
            document.getElementById('ai-model').value = cfg.model || '';
            document.getElementById('ai-api-key').value = '';
            document.getElementById('ai-api-key').placeholder = cfg.api_key || 'Leave empty for local models';
            document.getElementById('ai-tag-prefix').value = cfg.tag_prefix || 'ai/';
            document.getElementById('ai-max-tokens').value = cfg.max_tokens || 512;
            document.getElementById('ai-chat-max-tokens').value = cfg.chat_max_tokens || 2048;
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
            document.getElementById('ai-test-result').hidden = true;
            // Reset model picker state.
            const modelSel = document.getElementById('ai-model-select');
            const modelMsg = document.getElementById('ai-model-msg');
            if (modelSel) { modelSel.hidden = true; modelSel.innerHTML = ''; }
            if (modelMsg) { modelMsg.style.display = 'none'; modelMsg.textContent = ''; }
        });
    // Face settings — always fetch fresh so the form reflects the saved DB value.
    const dirQ = currentAbsDir() ? '?dir=' + encodeURIComponent(currentAbsDir()) : '';
    fetch('/api/face/config' + dirQ)
        .then(r => r.json())
        .then(fc => {
            const enabled = !!fc.enabled;
            document.getElementById('face-enabled').checked = enabled;
            document.getElementById('face-settings-fields').hidden = !enabled;
            document.getElementById('face-confidence').value = fc.confidence ?? 0.7;
            document.getElementById('face-min-size').value = fc.min_face_px ?? 20;
            document.getElementById('face-cluster-dist').value = fc.cluster_distance ?? 0.4;
            document.getElementById('face-tag-prefix').value = fc.tag_prefix || 'person';
            document.getElementById('face-auto-match-threshold').value = fc.auto_match_threshold ?? 0.25;
            document.getElementById('face-tiling-enabled').checked = !!fc.tiling_enabled;
            const ready = !!fc.models_ready;
            document.getElementById('face-models-status').textContent = ready ? t('face.settings-models-ready') : t('face.settings-models-missing');
            document.getElementById('face-models-download-btn').hidden = ready;
        });
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

    // If the features tab is opened directly, initialise its toggles and warnings.
    if (tab === 'features') updateFeaturesTab();
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
    input.focus();
}

async function doDirAddTag() {
    if (!state.selectedDir) return;
    const input = document.getElementById('dir-tag-input');
    const tagStr = input?.value.trim();
    if (!tagStr) return;

    const recursive = document.getElementById('dir-tag-recursive')?.checked || false;
    if (!recursive) {
        input.value = '';
        await addTagToDir(state.selectedDir.path, tagStr);
        return;
    }

    // Recursive mode — submit background job.
    const includeArchives = document.getElementById('dir-tag-archives')?.checked || false;
    const statusEl = document.getElementById('dir-recursive-status');
    const btn = document.querySelector('#detail-panel button[onclick="doDirAddTag()"]');

    if (btn) btn.disabled = true;
    if (statusEl) statusEl.textContent = '\u29d7 Verwerken\u2026';

    try {
        const res = await apiPost('/api/tag-dir-recursive', {
            path: state.selectedDir.path,
            tags: [tagStr],
            include_archives: includeArchives,
            dir: currentAbsDir(),
        });
        if (input) input.value = '';
        const shortId = (res.job_id || '').slice(0, 8);
        if (statusEl) statusEl.textContent = `\u2713 Job gestart (${shortId}\u2026)`;
        if (typeof onJobSubmitted === 'function') onJobSubmitted(res.job_id);
    } catch (e) {
        if (statusEl) statusEl.textContent = `\u2717 ${e.message || 'Fout'}`;
    } finally {
        if (btn) btn.disabled = false;
    }
}

function _dirRecursiveToggle() {
    const recursive = document.getElementById('dir-tag-recursive')?.checked || false;
    localStorage.setItem('dirTag:recursive', recursive ? '1' : '0');
    const archivesWrap = document.getElementById('dir-tag-archives-wrap');
    if (archivesWrap) archivesWrap.hidden = !recursive;
    const statusEl = document.getElementById('dir-recursive-status');
    if (statusEl && !recursive) statusEl.textContent = '';
}

function _dirArchivesToggle() {
    const archives = document.getElementById('dir-tag-archives')?.checked || false;
    localStorage.setItem('dirTag:archives', archives ? '1' : '0');
}

async function doRemoveTag(path, tagStr, subject) {
    await removeTagFromFile(path, tagStr, subject);
}

// ---------------------------------------------------------------------------
// Detail panel: drag tag chips between subject groups
// ---------------------------------------------------------------------------

function detailChipDragStart(event, path, tagStr, subject) {
    event.stopPropagation();
    event.dataTransfer.effectAllowed = 'move';
    event.dataTransfer.setData('text/filetag-detail-tag', JSON.stringify({ path, tagStr, subject: subject || null }));
}

function detailSubjectDragOver(event) {
    if (!event.dataTransfer.types.includes('text/filetag-detail-tag')) return;
    event.preventDefault();
    event.currentTarget.classList.add('subject-drag-over');
}

function detailSubjectDragLeave(event) {
    event.currentTarget.classList.remove('subject-drag-over');
}

async function detailSubjectDrop(event, filePath, newSubject) {
    event.preventDefault();
    event.currentTarget.classList.remove('subject-drag-over');
    const raw = event.dataTransfer.getData('text/filetag-detail-tag');
    if (!raw) return;
    const { path, tagStr, subject: oldSubject } = JSON.parse(raw);
    if (path !== filePath) return;
    const normNew = newSubject || null;
    const normOld = oldSubject || null;
    if (normNew === normOld) return; // no change

    const dir = currentAbsDir();
    await apiPost('/api/untag', { path, tags: [tagStr], dir, ...(normOld ? { subject: normOld } : {}) });
    await apiPost('/api/tag',   { path, tags: [tagStr], dir, ...(normNew ? { subject: normNew } : {}) });
    await loadFileDetail(path);
    await loadTags();
    ftEmit('ft:file-tags', { paths: [path] });
}

async function doRemoveSubject(path, subject) {
    const f = state.selectedFilesData.get(path) || state.selectedFile;
    if (!f) return;
    const subjectTags = (f.tags || []).filter(tag => (tag.subject || '') === subject);
    for (const tag of subjectTags) {
        await removeTagFromFile(path, formatTag(tag), subject);
    }
    // ftEmit is handled by the last removeTagFromFile call
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

    inputEl.addEventListener('input', () => buildDropdown(getSubjects()));
    inputEl.addEventListener('blur',  () => setTimeout(closeDropdown, 150));
    inputEl.addEventListener('keydown', e => {
        if (e.key === 'Escape') { e.preventDefault(); closeDropdown(); }
        if (e.key === 'Enter')
            setTimeout(closeDropdown, 0);
    });
}

/// Fill (or toggle-clear) the subject input from a subject-label click.
/// Sets the value programmatically so no autocomplete dropdown is triggered.
function toggleSubjectInput(subject) {
    const input = document.getElementById('tag-subject');
    if (!input) return;
    input.value = input.value.trim() === subject ? '' : subject;
    _updateSubjectLabelHighlight();
}

/// Start inline rename for a subject label in the detail panel.
/// Replaces the label (and its rename button) with a text input;
/// Enter or blur confirms; Escape cancels.
function startSubjectRename(groupEl, filePath, oldSubj) {
    const label = groupEl.querySelector('.subject-label');
    const renameBtn = groupEl.querySelector('.subject-rename');
    if (!label) return;

    const input = document.createElement('input');
    input.className = 'subject-rename-input';
    input.type = 'text';
    input.value = oldSubj;

    let done = false;

    async function commit() {
        if (done) return;
        done = true;
        const newSubj = input.value.trim();
        input.replaceWith(label);
        if (renameBtn) label.insertAdjacentElement('afterend', renameBtn);
        if (newSubj && newSubj !== oldSubj) {
            await doRenameSubject(oldSubj, newSubj);
        }
    }

    function cancel() {
        if (done) return;
        done = true;
        input.replaceWith(label);
        if (renameBtn) label.insertAdjacentElement('afterend', renameBtn);
    }

    input.addEventListener('keydown', e => {
        if (e.key === 'Enter')  { e.preventDefault(); commit(); }
        if (e.key === 'Escape') { e.preventDefault(); cancel(); }
    });
    input.addEventListener('blur', () => setTimeout(commit, 150));

    label.replaceWith(input);
    if (renameBtn) renameBtn.remove();
    input.select();
}

/// Rename a subject globally across all files in the database.
async function doRenameSubject(oldName, newName) {
    await apiPost('/api/rename-subject', { name: oldName, new_name: newName, dir: currentAbsDir() });
    if (state.selectedFile) await loadFileDetail(state.selectedFile.path);
    await loadTags();
    ftEmit('ft:file-tags', { paths: [] });
}

function attachTagAutocomplete(inputEl, submitFn) {
    let _dropdown = null;
    let _activeIdx = -1;

    function getTagMatches(query) {
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

    // Return { type: 'tag', matches } or { type: 'value', key, matches }
    async function getMatches(raw) {
        const eqIdx = raw.indexOf('=');
        if (eqIdx > 0) {
            const key = raw.slice(0, eqIdx);
            const valQuery = raw.slice(eqIdx + 1).toLowerCase();
            // Load values lazily (same as sidebar)
            if (!state.kvValueCache[key]) {
                try {
                    const values = await api(
                        '/api/tag-values?' + new URLSearchParams({ name: key }) + dirParam('&')
                    );
                    state.kvValueCache[key] = values;
                } catch (_) {
                    state.kvValueCache[key] = [];
                }
            }
            const allVals = state.kvValueCache[key] || [];
            const matched = allVals
                .filter(v => !valQuery || String(v.value).toLowerCase().includes(valQuery))
                .sort((a, b) => b.count - a.count)
                .slice(0, 15);
            return { type: 'value', key, matches: matched };
        }
        return { type: 'tag', matches: getTagMatches(raw) };
    }

    async function buildDropdown(raw) {
        const result = await getMatches(raw.trim());
        if (!_dropdown) {
            _dropdown = document.createElement('ul');
            _dropdown.className = 'tag-autocomplete';
            inputEl.parentElement.appendChild(_dropdown);
        }
        _activeIdx = -1;
        if (result.type === 'tag') {
            const tags = result.matches;
            if (!tags.length) { _dropdown.innerHTML = ''; _dropdown.hidden = true; return; }
            _dropdown.innerHTML = tags.map(tag => {
                const dot = tag.color
                    ? `<span class="tag-color-dot" style="background:${tag.color}"></span>`
                    : '';
                return `<li data-tagname="${esc(tag.name)}">${dot}<span class="ac-name">${esc(tag.name)}</span><span class="ac-count">${tag.count}</span></li>`;
            }).join('');
        } else {
            const { key, matches } = result;
            if (!matches.length) { _dropdown.innerHTML = ''; _dropdown.hidden = true; return; }
            _dropdown.innerHTML = matches.map(v =>
                `<li data-tagname="${esc(key + '=' + v.value)}"><span class="ac-name">${esc(key + '=')}<strong>${esc(String(v.value))}</strong></span><span class="ac-count">${v.count}</span></li>`
            ).join('');
        }
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

    inputEl.addEventListener('input', () => buildDropdown(inputEl.value));

    inputEl.addEventListener('blur', () => setTimeout(closeDropdown, 150));

    inputEl.addEventListener('keydown', e => {
        const items = _dropdown ? _dropdown.querySelectorAll('li') : [];
        const count = items.length;
        if (e.key === 'ArrowDown') {
            e.preventDefault();
            if (!_dropdown || _dropdown.hidden) buildDropdown(inputEl.value);
            setActive(Math.min(_activeIdx + 1, count - 1));
        } else if (e.key === 'ArrowUp') {
            e.preventDefault();
            setActive(Math.max(_activeIdx - 1, 0));
        } else if (e.key === 'Escape') {
            e.preventDefault();
            closeDropdown();
        } else if (e.key === 'Tab') {
            // Accept active completion, then let Tab naturally move focus.
            // Do NOT submit — user explicitly wants Tab to only navigate fields.
            if (_activeIdx >= 0 && _dropdown && !_dropdown.hidden) {
                inputEl.value = items[_activeIdx].dataset.tagname;
                e.preventDefault(); // prevent focus move after in-place completion
            }
            closeDropdown();
            // No submitFn() call — Enter or button submit only.
        } else if (e.key === 'Enter') {
            e.preventDefault();
            if (_activeIdx >= 0 && _dropdown && !_dropdown.hidden) {
                inputEl.value = items[_activeIdx].dataset.tagname;
            }
            closeDropdown();
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
            }
            submitFn();
        }
    });
}

function clearSelection() {
    _selectAllToken = null;       // cancel any in-progress select-all
    state.selectionLoading = false;
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
    status.textContent = `⧗ ${paths.length} best…`; // spinner + count
    const bulkBody = { paths, tags: [tagStr], dir: currentAbsDir() };
    if (subject) bulkBody.subject = subject;
    await apiPost('/api/tag-bulk', bulkBody);
    // Optimistic local cache update — no need to re-fetch each file.
    const eqIdx = tagStr.indexOf('=');
    const tName  = eqIdx !== -1 ? tagStr.slice(0, eqIdx) : tagStr;
    const tValue = eqIdx !== -1 ? tagStr.slice(eqIdx + 1) : '';
    for (const p of paths) {
        const d = state.selectedFilesData.get(p);
        if (d) {
            d.tags = d.tags || [];
            if (!d.tags.some(t => t.name === tName && (t.value || '') === tValue)) {
                d.tags.push({ name: tName, value: tValue || null });
            }
        }
    }
    await loadTags();
    input.value = '';
    status.textContent = `Added "${tagStr}"${subject ? ` [${subject}]` : ''} to ${paths.length} file${paths.length === 1 ? '' : 's'}.`;
    // Update tag-count badges surgically — avoids scroll reset and checkmark loss.
    for (const p of paths) {
        const d = state.selectedFilesData.get(p);
        if (d && d.tags) {
            const tagCount = d.tags.length;
            const entries = state.mode === 'search' ? state.searchResults : state.entries;
            const entry = entries.find(e => (e.path || fullPath(e)) === p);
            if (entry) entry.tag_count = tagCount;
            const badge = document.querySelector(`#content [data-path="${CSS.escape(p)}"] .tags-count`);
            if (badge) badge.textContent = `${tagCount} tag${tagCount === 1 ? '' : 's'}`;
        }
    }
    _updateCardSelection();
    renderTags();
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

function toggleLabelsMenu(e) {
    e.stopPropagation();
    const menu = document.getElementById('labels-menu');
    menu.hidden = !menu.hidden;
}

function toggleCardLabels() {
    const current = localStorage.getItem('ft-card-labels') || 'show';
    // Main button: simple on/off. Off uses the last non-show mode stored,
    // defaulting to 'hide'. On always goes back to 'show'.
    const next = current === 'show' ? (localStorage.getItem('ft-card-labels-off') || 'hide') : 'show';
    setCardLabels(next);
}

function setCardLabels(mode) {
    document.getElementById('labels-menu').hidden = true;
    if (mode !== 'show') localStorage.setItem('ft-card-labels-off', mode);
    const grid = document.getElementById('content');
    const btn  = document.getElementById('labels-toggle');
    grid.classList.toggle('hide-labels', mode !== 'show');
    grid.classList.toggle('hide-badges', mode === 'minimal');
    // Active = labels are visible
    btn.classList.toggle('active', mode === 'show');
    // Update checkmarks in the dropdown
    ['show', 'hide', 'minimal'].forEach(m => {
        const el = document.getElementById(`labels-opt-${m}`);
        if (!el) return;
        el.classList.toggle('active', m === mode);
    });
    localStorage.setItem('ft-card-labels', mode);
}

function toggleMoreMenu(e) {
    e.stopPropagation();
    const menu = document.getElementById('more-menu');
    menu.hidden = !menu.hidden;
}

document.addEventListener('click', () => {
    const menu = document.getElementById('more-menu');
    if (menu) menu.hidden = true;
    const lmenu = document.getElementById('labels-menu');
    if (lmenu) lmenu.hidden = true;
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

function updatePregenBtn() {
    const btn = document.getElementById('pregen-sprites-btn');
    if (!btn) return;
    const mode = state.settings.tile_preview_mode ?? 'sprite';
    if (mode === 'webm-seek') {
        btn.hidden = true;
    } else {
        btn.hidden = false;
        const key = mode === 'sprite' ? 'toolbar.sprites-gen' : 'toolbar.tiles-gen';
        btn.textContent = t(key);
    }
}

async function pregenSprites() {
    const menu = document.getElementById('more-menu');
    if (menu) menu.hidden = true;

    const tileMode = state.settings.tile_preview_mode ?? 'sprite';
    // webm-seek serves the original file directly — nothing to pre-generate.
    if (tileMode === 'webm-seek') return;

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

    try {
        let res;
        if (tileMode === 'sprite') {
            // Submit a background job for trickplay sprite-sheet generation.
            res = await apiPost('/api/vthumbs/pregenerate' + dirParam('?'), { paths: videoPaths });
        } else if (tileMode === 'autoplay') {
            // autoplay mode: generate sprite sheets first (used as hover fallback),
            // then generate the WebM tile previews.
            try {
                const sr = await apiPost('/api/vthumbs/pregenerate' + dirParam('?'), { paths: videoPaths });
                if (sr?.job_id) onJobSubmitted(sr.job_id);
            } catch (_) {}
            res = await apiPost('/api/vtile/pregenerate' + dirParam('?'), { paths: videoPaths });
        } else {
            // webm: submit background WebM tile-preview generation job.
            res = await apiPost('/api/vtile/pregenerate' + dirParam('?'), { paths: videoPaths });
        }
        if (res?.job_id) onJobSubmitted(res.job_id);
    } catch (e) {
        showToast(`Fout: ${e.message || e}`);
    } finally {
        btn.disabled = false;
    }
}

// ---------------------------------------------------------------------------
// Auto-pregen vtiles on directory navigation (webm/autoplay mode)
// ---------------------------------------------------------------------------

// IDs of currently running background pregen jobs (sprites + vtile).
// Cancelled when the user navigates to a different directory so they do not
// keep consuming resources on the new page.
let _activePregenJobIds = [];

function _cancelActivePregenJobs() {
    if (_activePregenJobIds.length === 0) return;
    const ids = _activePregenJobIds.splice(0);
    for (const id of ids) {
        fetch(`/api/jobs/${encodeURIComponent(id)}/cancel`, { method: 'POST' }).catch(() => {});
    }
}

// ---------------------------------------------------------------------------
// Viewport-priority vtile pregen queue (autoplay mode)
// ---------------------------------------------------------------------------
// In autoplay mode, WebM tile previews are NOT batch-submitted for the whole
// directory.  Instead, each card queues itself here as it enters the viewport
// (via _trickplayAttach in detail.js), so the currently-visible tiles are
// always generated first.  A short debounce batches simultaneous entries.
const _vtilePregenQueue = [];
let   _vtilePregenTimer = null;

function _queueVtilePregen(path) {
    if (_vtilePregenQueue.includes(path)) return;
    _vtilePregenQueue.push(path);
    if (_vtilePregenTimer) clearTimeout(_vtilePregenTimer);
    _vtilePregenTimer = setTimeout(_flushVtilePregenQueue, 150);
}

async function _flushVtilePregenQueue() {
    _vtilePregenTimer = null;
    if (_vtilePregenQueue.length === 0) return;
    const batch = _vtilePregenQueue.splice(0);  // drain current snapshot
    try {
        const res = await apiPost('/api/vtile/pregenerate' + dirParam('?'), { paths: batch });
        if (res?.job_id) {
            onJobSubmitted(res.job_id);
            _activePregenJobIds.push(res.job_id);
        }
    } catch (_) {}
}

async function _autoPregenVtiles() {
    const tileMode = state.settings.tile_preview_mode ?? 'sprite';
    if (tileMode !== 'webm' && tileMode !== 'autoplay') return;
    if (state.mode !== 'browse') return;

    // Cancel batch pregen jobs left over from the previous directory.
    _cancelActivePregenJobs();

    // Reset the viewport-priority vtile queue so a new directory starts fresh.
    _vtilePregenQueue.length = 0;
    if (_vtilePregenTimer) { clearTimeout(_vtilePregenTimer); _vtilePregenTimer = null; }

    // Pregen is now entirely viewport-priority: each card queues itself via
    // _queueVtilePregen (webm + autoplay) as its thumbnail loads through the
    // IntersectionObserver pipeline, and _apEnsureSprite (autoplay) fetches
    // sprites on-demand for visible tiles.  No bulk job is submitted here to
    // avoid queueing hundreds of files the user may never reach.
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
    // Update feature toggles and warnings whenever the features tab becomes visible.
    if (tab === 'features') updateFeaturesTab();
}

// Populates the toggles and tool-availability warnings in the features tab.
function updateFeaturesTab() {
    // Video/ffmpeg
    const ffmpegInstalled = state.settings.ffmpeg_installed === true;
    const ffmpegBox = document.getElementById('feat-video');
    if (ffmpegBox) {
        ffmpegBox.checked = state.settings.feature_video ?? false;
        ffmpegBox.disabled = !ffmpegInstalled;
    }
    const ffmpegWarn = document.getElementById('feat-video-warn');
    if (ffmpegWarn) {
        ffmpegWarn.hidden = ffmpegInstalled;
        if (ffmpegInstalled) ffmpegWarn.style.display = 'none';
        else ffmpegWarn.style.display = 'block';
    }

    // ImageMagick
    const magickInstalled = state.settings.imagemagick_installed === true;
    const magickBox = document.getElementById('feat-imagemagick');
    if (magickBox) {
        magickBox.checked = state.settings.feature_imagemagick ?? false;
        magickBox.disabled = !magickInstalled;
    }
    const magickWarn = document.getElementById('feat-imagemagick-warn');
    if (magickWarn) {
        magickWarn.hidden = magickInstalled;
        if (magickInstalled) magickWarn.style.display = 'none';
        else magickWarn.style.display = 'block';
    }

    // Saliency (smart cropping)
    const poseBox = document.getElementById('feat-saliency-pose');
    if (poseBox) poseBox.checked = state.settings.feature_saliency_pose ?? false;
    const objBox = document.getElementById('feat-saliency-object');
    if (objBox) objBox.checked = state.settings.feature_saliency_object ?? false;
    _updateSaliencyStatus();
}

function closeSettings() {
    document.getElementById('settings-modal').hidden = true;
}

function _updateTilePreviewHint() {
    const sel = document.getElementById('tile-preview-mode');
    const hint = document.getElementById('tile-preview-hint');
    if (!sel || !hint) return;
    const hints = {
        sprite:    'Lightest option. Scrubs through a pre-generated sprite sheet on hover — no extra processing while browsing. Requires trickplay sprites to be generated in advance.',
        webm:      'Requires ffmpeg with libvpx. A short clip is transcoded once per video and cached; plays as a looping clip on hover only. Moderate one-time CPU cost per video.',
        'webm-seek': 'Requires ffmpeg with libvpx. Heaviest backend mode — the <em>full video</em> is always transcoded (regardless of clip duration), using the most CPU and storage. Mouse position seeks the timeline.',
        autoplay:  'Same backend cost as WebM clip (short clip per video, cached). Heaviest for frontend and network: all visible tiles are requested and start playing simultaneously on page load — may trigger a burst of encode jobs on first visit and significant ongoing GPU/CPU usage.',
    };
    hint.innerHTML = hints[sel.value] ?? '';
}

async function saveVideoSettings() {
    const min = parseInt(document.getElementById('sprite-min').value, 10);
    const max = parseInt(document.getElementById('sprite-max').value, 10);
    if (min >= 2 && min <= 64 && max >= 2 && max <= 64) {
        const body = {
            dir: currentAbsDir(),
            sprite_min: min,
            sprite_max: Math.max(max, min),
            dir_preview_style: document.getElementById('dir-preview-style')?.value ?? 'crop',
            tile_preview_mode: document.getElementById('tile-preview-mode')?.value ?? 'sprite',
            vtile_duration: Math.min(120, Math.max(0, parseInt(document.getElementById('vtile-duration')?.value ?? '8', 10))),
            vtile_use_longest: document.getElementById('vtile-use-longest')?.checked ?? false,
        };
        try {
            await apiPost('/api/settings', body);
            state.settings.sprite_min = body.sprite_min;
            state.settings.sprite_max = body.sprite_max;
            state.settings.dir_preview_style = body.dir_preview_style;
            state.settings.tile_preview_mode = body.tile_preview_mode;
            state.settings.vtile_duration = body.vtile_duration;
            state.settings.vtile_use_longest = body.vtile_use_longest;
            updatePregenBtn();
            renderContent();
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
        feature_saliency_pose: document.getElementById('feat-saliency-pose').checked,
        feature_saliency_object: document.getElementById('feat-saliency-object').checked,
    };
    try {
        await apiPost('/api/settings', body);
        state.settings.feature_video = body.feature_video;
        state.settings.feature_imagemagick = body.feature_imagemagick;
        state.settings.feature_pdf = body.feature_pdf;
        state.settings.feature_saliency_pose = body.feature_saliency_pose;
        state.settings.feature_saliency_object = body.feature_saliency_object;
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

// ---------------------------------------------------------------------------
// Saliency (smart cropping) settings
// ---------------------------------------------------------------------------

function _updateSaliencyStatus() {
    const poseReady = state.settings.saliency_pose_ready === true;
    const objReady = state.settings.saliency_object_ready === true;
    const poseEl = document.getElementById('feat-saliency-pose-status');
    const objEl = document.getElementById('feat-saliency-object-status');
    if (poseEl) {
        if (state._saliencyPoseDownloading) {
            const dl = state._saliencyStatus?.download;
            if (dl && dl.pose_bytes_total > 0) {
                const pct = Math.round(dl.pose_bytes_done / dl.pose_bytes_total * 100);
                poseEl.textContent = `Downloading… ${pct}%`;
            } else {
                poseEl.textContent = 'Downloading…';
            }
        } else if (poseReady) {
            poseEl.innerHTML = '✓ Model ready &nbsp;<button onclick="saliencyTestFile()">Test on a file…</button>';
        } else {
            poseEl.innerHTML = '⚠ Model not downloaded <button onclick="saliencyDownloadPose()">Download</button>';
        }
    }
    if (objEl) {
        if (state._saliencyObjectDownloading) {
            const dl = state._saliencyStatus?.download;
            if (dl && dl.object_bytes_total > 0) {
                const pct = Math.round(dl.object_bytes_done / dl.object_bytes_total * 100);
                objEl.textContent = `Downloading… ${pct}%`;
            } else {
                objEl.textContent = 'Downloading…';
            }
        } else {
            objEl.textContent = objReady ? '✓ Model ready' : '⚠ Model not downloaded';
            if (!objReady) {
                objEl.innerHTML += ' <button onclick="saliencyDownloadObject()">Download</button>';
            }
        }
    }
}

function saliencyPoseToggled() {
    // When pose is disabled, also disable object.
    if (!document.getElementById('feat-saliency-pose').checked) {
        document.getElementById('feat-saliency-object').checked = false;
    }
}

function saliencyObjectToggled() {
    // Object model requires pose to be enabled.
    if (document.getElementById('feat-saliency-object').checked) {
        document.getElementById('feat-saliency-pose').checked = true;
    }
}

async function saliencyDownloadPose() {
    if (state._saliencyPoseDownloading) return;
    state._saliencyPoseDownloading = true;
    _updateSaliencyStatus();
    try {
        await apiPost('/api/saliency/ensure-pose', {});
        _pollSaliency();
    } catch (e) {
        state._saliencyPoseDownloading = false;
        _updateSaliencyStatus();
        showToast('Download failed: ' + e.message, 5000);
    }
}

async function saliencyDownloadObject() {
    if (state._saliencyObjectDownloading) return;
    state._saliencyObjectDownloading = true;
    _updateSaliencyStatus();
    try {
        await apiPost('/api/saliency/ensure-object', {});
        _pollSaliency();
    } catch (e) {
        state._saliencyObjectDownloading = false;
        _updateSaliencyStatus();
        showToast('Download failed: ' + e.message, 5000);
    }
}

function _pollSaliency() {
    if (state._saliencyPollTimer) return;
    state._saliencyPollTimer = setInterval(async () => {
        try {
            const s = await api('/api/saliency/status');
            state._saliencyStatus = s;
            state._saliencyPoseDownloading = s.download?.pose_active === true;
            state._saliencyObjectDownloading = s.download?.object_active === true;
            state.settings.saliency_pose_ready = s.pose_ready === true;
            state.settings.saliency_object_ready = s.object_ready === true;
            _updateSaliencyStatus();
            if (!s.download?.pose_active && !s.download?.object_active) {
                clearInterval(state._saliencyPollTimer);
                state._saliencyPollTimer = null;
                if (s.download?.error) showToast('Download error: ' + s.download.error, 5000);
            }
        } catch (_) {
            clearInterval(state._saliencyPollTimer);
            state._saliencyPollTimer = null;
        }
    }, 600);
}

async function saliencyTestFile() {
    const path = prompt('Enter the absolute path to an image file to test detection on:');
    if (!path) return;
    const statusEl = document.getElementById('feat-saliency-pose-status');
    if (statusEl) statusEl.textContent = 'Running detection…';
    try {
        const res = await api(`/api/saliency/test?path=${encodeURIComponent(path)}`);
        if (res.error) {
            if (statusEl) statusEl.innerHTML = `✓ Model ready &nbsp;<button onclick="saliencyTestFile()">Test on a file…</button>`;
            showToast('Detection error: ' + res.error, 5000);
        } else if (res.salient_point) {
            const cx = res.salient_point.cx.toFixed(3);
            const cy = res.salient_point.cy.toFixed(3);
            if (statusEl) statusEl.innerHTML = `✓ Model ready — detected (cx=${cx}, cy=${cy}) &nbsp;<button onclick="saliencyTestFile()">Test again…</button>`;
        } else {
            if (statusEl) statusEl.innerHTML = `✓ Model ready — no subject detected &nbsp;<button onclick="saliencyTestFile()">Test again…</button>`;
        }
    } catch (e) {
        if (statusEl) statusEl.innerHTML = `✓ Model ready &nbsp;<button onclick="saliencyTestFile()">Test on a file…</button>`;
        showToast('Test failed: ' + e.message, 5000);
    }
}


function faceToggleEnabled() {
    const enabled = document.getElementById('face-enabled').checked;
    document.getElementById('face-settings-fields').hidden = !enabled;
}

function faceResetDefaults() {
    document.getElementById('face-confidence').value         = 0.7;
    document.getElementById('face-min-size').value           = 40;
    document.getElementById('face-cluster-dist').value       = 0.35;
    document.getElementById('face-tag-prefix').value         = 'person';
    document.getElementById('face-auto-match-threshold').value = 0.30;
    document.getElementById('face-tiling-enabled').checked   = false;
}

async function faceSaveSettings() {
    try {
        const body = {
            enabled:               document.getElementById('face-enabled').checked,
            confidence:            parseFloat(document.getElementById('face-confidence').value) || 0.7,
            min_face_px:           parseInt(document.getElementById('face-min-size').value, 10) || 20,
            cluster_distance:      parseFloat(document.getElementById('face-cluster-dist').value) || 0.4,
            tag_prefix:            document.getElementById('face-tag-prefix').value.trim() || 'person',
            auto_match_threshold:  parseFloat(document.getElementById('face-auto-match-threshold').value) || 0,
            tiling_enabled:        document.getElementById('face-tiling-enabled').checked,
            dir:                   currentAbsDir() || null,
        };
        await apiPost('/api/face/config', body);
        // Refresh in-memory config and re-render sidebar / toolbar.
        if (typeof loadFaceConfig === 'function') {
            await loadFaceConfig();
        }
        if (typeof loadPeople === 'function') {
            await loadPeople();
        }
        renderTags();
        closeSettings();
    } catch (e) {
        showToast('Save failed: ' + e.message, 4000);
    }
}

function _updateVideoMaxMbVisibility() {
    // Full video mode is disabled; max-MB row is always hidden.
    const row = document.getElementById('ai-video-max-mb-row');
    if (row) row.hidden = true;
}

function aiVideoModeChanged() {
    _updateVideoMaxMbVisibility();
}

/** Fetch available models from the currently entered endpoint and populate the
 *  model field: auto-select when only one model is returned, show a select
 *  list when multiple are available, or show an error message. */
async function aiFetchModels() {
    const btn     = document.getElementById('ai-fetch-models-btn');
    const input   = document.getElementById('ai-model');
    const sel     = document.getElementById('ai-model-select');
    const msg     = document.getElementById('ai-model-msg');
    const endpoint = document.getElementById('ai-endpoint').value.trim();
    const format   = document.getElementById('ai-format').value;
    const apiKey   = document.getElementById('ai-api-key').value.trim();

    if (!endpoint) {
        if (msg) { msg.textContent = 'Enter an endpoint URL first.'; msg.style.display = ''; }
        return;
    }

    btn.disabled = true;
    btn.textContent = '…';
    if (msg) { msg.style.display = 'none'; msg.textContent = ''; }
    if (sel) { sel.hidden = true; sel.innerHTML = ''; }

    try {
        const qs = new URLSearchParams({ endpoint, format, dir: currentAbsDir() || '' });
        if (apiKey) qs.set('api_key', apiKey);
        const data = await api('/api/ai/models?' + qs);

        const models = data.models || [];
        if (data.error && models.length === 0) {
            if (msg) { msg.textContent = data.error; msg.style.display = ''; }
            return;
        }

        if (models.length === 0) {
            if (msg) { msg.textContent = 'No models returned by endpoint.'; msg.style.display = ''; }
            return;
        }

        if (models.length === 1) {
            input.value = models[0];
            if (msg) { msg.textContent = ''; msg.style.display = 'none'; }
            return;
        }

        // Multiple models: show a <select> list.
        sel.innerHTML = models.map(m =>
            `<option value="${m.replace(/"/g, '&quot;')}">${m}</option>`
        ).join('');
        // Pre-select current value if it matches.
        const cur = input.value.trim();
        if (cur && models.includes(cur)) sel.value = cur;
        sel.hidden = false;
        sel.focus();
    } catch (e) {
        if (msg) { msg.textContent = String(e); msg.style.display = ''; }
    } finally {
        btn.disabled = false;
        btn.textContent = 'Fetch';
    }
}

/** When the user picks from the model dropdown, copy to the text input. */
function aiModelSelectChange(sel) {
    const input = document.getElementById('ai-model');
    if (input && sel.value) input.value = sel.value;
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
        chat_max_tokens: parseInt(document.getElementById('ai-chat-max-tokens').value, 10) || 2048,
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

// ---------------------------------------------------------------------------
// Similarity index helpers
// ---------------------------------------------------------------------------

async function indexPhash() {
    const btn = document.getElementById('cm-phash-btn');
    const cancelBtn = document.getElementById('cm-cancel-phash-btn');
    const statusEl = document.getElementById('cm-phash-status');
    const barEl = document.getElementById('cm-phash-progress-bar');
    const barFill = document.getElementById('cm-phash-progress-fill');
    if (btn) { btn.disabled = true; btn.textContent = t('cm.phash-indexing'); }
    if (cancelBtn) cancelBtn.hidden = false;
    if (barEl) barEl.hidden = false;

    let pollId = null;
    async function pollProgress() {
        try {
            const p = await api('/api/similar/index-phash/progress');
            if (p && p.total > 0) {
                const pct = Math.round((p.done / p.total) * 100);
                if (barFill) barFill.style.width = pct + '%';
                if (statusEl) statusEl.textContent =
                    `${p.done} / ${p.total} (${pct}%)` +
                    (p.current ? ` — ${p.current.split('/').pop()}` : '');
            }
            if (p && p.running) pollId = setTimeout(pollProgress, 400);
        } catch (_) {}
    }
    pollId = setTimeout(pollProgress, 300);

    try {
        const res = await apiPost('/api/similar/index-phash', { dir: currentAbsDir() });
        clearTimeout(pollId);
        if (barFill) barFill.style.width = '100%';
        if (res.cancelled) {
            if (statusEl) statusEl.textContent = t('cm.phash-cancelled', { indexed: res.indexed, skipped: res.skipped, errors: res.errors });
            showToast(t('cm.phash-cancelled-toast'), 'info');
        } else {
            if (statusEl) statusEl.textContent = t('cm.phash-done', { indexed: res.indexed, skipped: res.skipped, errors: res.errors, total: res.total });
            showToast(t('cm.phash-done-toast', { indexed: res.indexed }));
        }
    } catch (e) {
        clearTimeout(pollId);
        showToast(String(e), 'error');
    } finally {
        if (btn) { btn.disabled = false; btn.textContent = t('cm.phash-btn'); }
        if (cancelBtn) cancelBtn.hidden = true;
    }
}

async function cancelPhash() {
    try {
        await apiPost('/api/similar/index-phash/cancel', {});
    } catch (_) {}
    const cancelBtn = document.getElementById('cm-cancel-phash-btn');
    if (cancelBtn) cancelBtn.disabled = true;
}

async function loadSimilarFiles(path, n = 20) {
    const dir = currentAbsDir();
    const params = new URLSearchParams({ path, dir, n });
    return await api('/api/similar?' + params);
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
async function aiPromoteTag(path, tagName, value, subject) {
    // tagName is e.g. "ai/necklace", promoted becomes "necklace".
    // value may be "" or e.g. "gold" for key=value tags.
    const promoted = tagName.slice('ai/'.length);
    if (!promoted) return;
    const newTagStr = value ? `${promoted}=${value}` : promoted;
    const toast = showToast(t('toast.promoting', {tag: newTagStr}), 0);
    try {
        // Add the promoted tag, then remove the ai/ original.
        const tagBody = { path, tags: [newTagStr], dir: currentAbsDir() };
        if (subject && typeof subject === 'string' && subject !== 'null' && subject.trim() !== '') {
            tagBody.subject = subject;
        }
        await apiPost('/api/tag', tagBody);
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
        if (state.selectedFile) await loadFileDetail(state.selectedFile.path);
        renderDetailTagsSectionOnly();
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
        renderDetailTagsSectionOnly();
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
    // Update button to "Analysing…" without rebuilding the video element.
    if (state.selectedFile?.path === path) renderDetailTagsSectionOnly();
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
        if (state.selectedFile?.path === path) renderDetailTagsSectionOnly();
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

    // On mobile: toggle overlay detail panel.
    if (window.matchMedia('(max-width: 639px)').matches) {
        layout.classList.toggle('detail-force-open');
        restoreScrollAnchor(anchor);
        return;
    }

    // On tablet: toggle overlay detail panel.
    if (window.matchMedia('(max-width: 1024px)').matches) {
        layout.classList.toggle('detail-force-open');
        restoreScrollAnchor(anchor);
        return;
    }

    const collapsed = layout.classList.toggle('detail-collapsed');
    state.detailOpen = !collapsed;
    document.getElementById('detail-toggle').classList.toggle('active', !collapsed);
    _syncChatRight();
    restoreScrollAnchor(anchor);
}

/**
 * Toggle the sidebar drawer on mobile (< 640 px).
 * On larger screens this is a no-op — the sidebar is always visible.
 */
function toggleMobileSidebar() {
    const layout = document.querySelector('.layout');
    const backdrop = document.getElementById('sidebar-backdrop');
    const isOpen = layout.classList.toggle('sidebar-open');
    if (backdrop) backdrop.classList.toggle('visible', isOpen);
    // Prevent body scrolling while the sidebar drawer is open.
    document.body.style.overflow = isOpen ? 'hidden' : '';
}

/**
 * Close the mobile sidebar drawer if a tag or navigation item is tapped.
 * Call this from any handler that should implicitly close the drawer.
 */
function closeMobileSidebar() {
    const layout = document.querySelector('.layout');
    if (!layout.classList.contains('sidebar-open')) return;
    const backdrop = document.getElementById('sidebar-backdrop');
    layout.classList.remove('sidebar-open');
    if (backdrop) backdrop.classList.remove('visible');
    document.body.style.overflow = '';
}

// ---------------------------------------------------------------------------
// Cache Manager modal
// ---------------------------------------------------------------------------

function openCacheManager() {
    const menu = document.getElementById('more-menu');
    if (menu) menu.hidden = true;
    document.getElementById('cache-manager-modal').hidden = false;
    document.getElementById('cm-status').textContent = '';
    switchCmTab('cache');
    loadCacheInfo();
}

function closeCacheManager() {
    document.getElementById('cache-manager-modal').hidden = true;
}

function switchCmTab(tab) {
    ['cache', 'db'].forEach(name => {
        const panel = document.getElementById('cm-tab-' + name);
        const btn = document.getElementById('cm-tab-' + name + '-btn');
        if (panel) panel.hidden = name !== tab;
        if (btn) btn.classList.toggle('active', name === tab);
    });
    document.getElementById('cm-status').textContent = '';
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
            thumbs:     {en:'Thumbnails',          nl:'Miniaturen',          de:'Miniaturbilder',     fr:'Miniatures',          es:'Miniaturas',             it:'Anteprime',          pt:'Miniaturas',             pl:'Miniatury',          sv:'Miniatyrer'},
            'dir-thumbs':{en:'Folder previews',    nl:'Mapminiaturen',       de:'Ordner-Vorschauen',  fr:'Aperçus de dossiers', es:'Vistas previas de carpetas', it:'Anteprime cartelle', pt:'Pré-visualizações de pastas', pl:'Podglądy folderów', sv:'Mappförhandsvisningar'},
            raw:        {en:'RAW previews',         nl:'RAW-voorbeelden',     de:'RAW-Vorschauen',     fr:'Aperçus RAW',         es:'Vistas previas RAW',     it:'Anteprime RAW',      pt:'Pré-visualizações RAW',  pl:'Podglądy RAW',       sv:'RAW-förhandsvisningar'},
            vthumbs:    {en:'Video sprites',        nl:'Video-sprites',       de:'Video-Sprites',      fr:'Sprites vidéo',       es:'Sprites de vídeo',       it:'Sprite video',       pt:'Sprites de vídeo',       pl:"Sprite'y wideo",     sv:'Videospritar'},
            ai_sprites: {en:'AI sprites',           nl:'AI-sprites',          de:'KI-Sprites',         fr:'Sprites IA',          es:'Sprites de IA',          it:'Sprite IA',          pt:'Sprites de IA',          pl:"Sprite'y AI",        sv:'AI-spritar'},
            hls2:       {en:'HLS video files',      nl:'HLS-videobestanden',  de:'HLS-Videodateien',   fr:'Fichiers HLS',        es:'Archivos HLS',           it:'File HLS',           pt:'Ficheiros HLS',          pl:'Pliki HLS',          sv:'HLS-filer'},
            video:      {en:'Video transcodes',     nl:'Video-transcodes',    de:'Transkodierungen',   fr:'Transcodages',        es:'Transcodificaciones',    it:'Transcodifiche',     pt:'Transcodificações',      pl:'Transkodowania',     sv:'Transkodningar'},
            'zip-pages':{en:'Archive pages',        nl:'Archiefpagina\'s',   de:'Archivseiten',       fr:'Pages d\'archive',   es:'Páginas de archivo',     it:'Pagine archivio',    pt:'Páginas de arquivo',     pl:'Strony archiwum',    sv:'Arkivsidor'},
            'vtiles':   {en:'Video tiles',           nl:'Videotegels',         de:'Videokacheln',       fr:'Tuiles vidéo',      es:'Mosaicos de vídeo',      it:'Tile video',         pt:'Mosaicos de vídeo',      pl:'Kafle wideo',        sv:'Videobrickor'},
            'tiff-preview':{en:'TIFF previews',      nl:'TIFF-voorbeelden',    de:'TIFF-Vorschauen',    fr:'Aperçus TIFF',      es:'Vistas previas TIFF',    it:'Anteprime TIFF',     pt:'Pré-visualizações TIFF',  pl:'Podglądy TIFF',   sv:'TIFF-förhandsvisningar'},
            'tmp':      {en:'Temporary files',       nl:'Tijdelijke bestanden', de:'Temporäre Dateien',  fr:'Fichiers temporaires', es:'Archivos temporales',    it:'File temporanei',    pt:'Ficheiros temporários',  pl:'Pliki tymczasowe',   sv:'Temporära filer'},
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

async function doPurgeMissing() {
    if (state.currentBasePath == null) return;
    const btn = document.getElementById('cm-purge-btn');
    const statusEl = document.getElementById('cm-status');
    if (btn) btn.disabled = true;
    if (statusEl) statusEl.textContent = t('cm.loading');
    try {
        const resp = await fetch('/api/db/purge-missing' + dirParam('?'), {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: '{}',
        });
        const data = await resp.json();
        const n = data.removed || 0;
        if (statusEl) statusEl.textContent = n === 0
            ? t('cm.purge-none')
            : t('cm.purged-db', { n, plural: n !== 1 ? t('cm.purged-plural') : '' });
        if (n > 0) { _thumbClearCache(); refreshCurrentDir(); }
    } catch (e) {
        if (statusEl) statusEl.textContent = e.message || String(e);
    } finally {
        if (btn) btn.disabled = false;
    }
}

async function doPurgeUnusedTags() {
    if (state.currentBasePath == null) return;
    const btn = document.getElementById('cm-unused-tags-btn');
    const statusEl = document.getElementById('cm-status');
    if (btn) btn.disabled = true;
    if (statusEl) statusEl.textContent = t('cm.loading');
    try {
        const resp = await fetch('/api/db/purge-unused-tags' + dirParam('?'), {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: '{}',
        });
        const data = await resp.json();
        const n = data.removed || 0;
        if (statusEl) statusEl.textContent = n === 0
            ? t('cm.unused-tags-none')
            : t('cm.unused-tags-removed', { n, plural: n !== 1 ? t('cm.pruned-plural') : '' });
        if (n > 0) refreshTags();
    } catch (e) {
        if (statusEl) statusEl.textContent = e.message || String(e);
    } finally {
        if (btn) btn.disabled = false;
    }
}

async function doPurgeOrphanFileTags() {
    if (state.currentBasePath == null) return;
    const btn = document.getElementById('cm-orphan-ft-btn');
    const statusEl = document.getElementById('cm-status');
    if (btn) btn.disabled = true;
    if (statusEl) statusEl.textContent = t('cm.loading');
    try {
        const resp = await fetch('/api/db/purge-orphan-file-tags' + dirParam('?'), {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: '{}',
        });
        const data = await resp.json();
        const n = data.removed || 0;
        if (statusEl) statusEl.textContent = n === 0
            ? t('cm.orphan-ft-none')
            : t('cm.orphan-ft-removed', { n, plural: n !== 1 ? t('cm.pruned-plural') : '' });
        if (n > 0) { refreshCurrentDir(); refreshTags(); }
    } catch (e) {
        if (statusEl) statusEl.textContent = e.message || String(e);
    } finally {
        if (btn) btn.disabled = false;
    }
}

async function doVacuum() {
    if (state.currentBasePath == null) return;
    const btn = document.getElementById('cm-vacuum-btn');
    const statusEl = document.getElementById('cm-status');
    if (btn) btn.disabled = true;
    if (statusEl) statusEl.textContent = t('cm.loading');
    try {
        await fetch('/api/db/vacuum' + dirParam('?'), {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: '{}',
        });
        if (statusEl) statusEl.textContent = t('cm.vacuum-done');
    } catch (e) {
        if (statusEl) statusEl.textContent = e.message || String(e);
    } finally {
        if (btn) btn.disabled = false;
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
    const labels = { 'groups-first': 'Groups first', 'alpha': 'A–Z', 'count': 'By count' };
    const titles = { 'groups-first': 'Sort: groups first', 'alpha': 'Sort: A–Z', 'count': 'Sort: by count' };
    for (const id of ['sidebar-sort-btn', 'tm-sort-btn']) {
        const btn = document.getElementById(id);
        if (!btn) continue;
        btn.title = titles[state.tagSortMode];
        btn.classList.toggle('active', state.tagSortMode !== 'alpha');
        const label = btn.querySelector('.sort-label');
        if (label) label.textContent = labels[state.tagSortMode];
    }
    renderTags();
    renderTmList();
}

async function doTagGroupSearch(prefix) {
    // Expand group on click and clear any active tag filters
    _thumbClearCache();
    state.activeTags.clear();
    state.activeSubjects.clear();
    state.activePeople.clear();
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
    state.tagPickerSubject = null;
    state.tagPickerOriginalSubject = null;

    // Pre-check tags and subject already on the current selection.
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
        // Pre-select subject if all selected files share the same one.
        const subjectSets = [...state.selectedFilesData.values()].map(d => {
            const subjects = new Set((d.tags || []).map(t => t.subject).filter(Boolean));
            return subjects;
        });
        if (subjectSets.length > 0) {
            const first = [...subjectSets[0]];
            if (first.length === 1 && subjectSets.every(s => s.has(first[0]))) {
                state.tagPickerSubject = first[0];
                state.tagPickerOriginalSubject = first[0];
            }
        }
    } else if (state.selectedFile) {
        for (const t of (state.selectedFile.tags || [])) {
            if (t.name) {
                state.tagPickerPicks.add(t.name);
                state.tagPickerOriginal.add(t.name);
            }
        }
        // Pre-select the first subject found on this file's tags.
        const subjects = [...new Set((state.selectedFile.tags || []).map(t => t.subject).filter(Boolean))];
        if (subjects.length === 1) {
            state.tagPickerSubject = subjects[0];
            state.tagPickerOriginalSubject = subjects[0];
        }
    }

    renderTags();
    renderDetail();
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
/// If a subject is selected in the picker, it is attached to all newly-added tags.
async function applyTagPicker() {
    const paths = state.selectedPaths.size > 0
        ? [...state.selectedPaths]
        : state.selectedFile ? [state.selectedFile.path] : [];
    if (!paths.length) return;

    const toAdd    = [...state.tagPickerPicks].filter(t => !state.tagPickerOriginal.has(t));
    const toRemove = [...state.tagPickerOriginal].filter(t => !state.tagPickerPicks.has(t));
    const subjectChanged = state.tagPickerSubject !== state.tagPickerOriginalSubject;
    const subject = state.tagPickerSubject || undefined;

    if (toAdd.length === 0 && toRemove.length === 0 && !subjectChanged) {
        cancelTagPickerMode();
        return;
    }

    // Disable the Apply button immediately to prevent double-clicks and show
    // a file-count hint so the user knows something is happening.
    const _applyBtn = document.querySelector('.tag-picker-apply');
    if (_applyBtn) { _applyBtn.disabled = true; _applyBtn.textContent = `⧗ ${paths.length} best.`; }

    const dir = currentAbsDir();
    const ops = [];

    // Helper: group paths by their root_id (handles cross-root search results).
    function groupByRootId(ps) {
        const map = new Map();
        for (const p of ps) {
            const rid = searchRootIdForPath(p);
            if (!map.has(rid)) map.set(rid, []);
            map.get(rid).push(p);
        }
        return map;
    }

    // Add new tags (with selected subject if any) — one bulk request per root.
    if (toAdd.length > 0) {
        for (const [rid, ps] of groupByRootId(paths)) {
            const rootParam = rid != null ? { root_id: rid } : {};
            ops.push(apiPost('/api/tag-bulk', { paths: ps, tags: toAdd, ...rootParam, ...(subject ? { subject } : {}) }));
        }
    }
    // Remove unchecked tags — one bulk request per root.
    if (toRemove.length > 0) {
        for (const [rid, ps] of groupByRootId(paths)) {
            const rootParam = rid != null ? { root_id: rid } : {};
            ops.push(apiPost('/api/untag-bulk', { paths: ps, tags: toRemove, ...rootParam }));
        }
    }
    // If subject changed but no tag delta, re-apply existing tags with new subject.
    // Each file may have different tags, so fall back to individual requests here.
    if (subjectChanged && toAdd.length === 0 && toRemove.length === 0 && subject) {
        for (const p of paths) {
            const data = state.selectedFilesData.get(p) || (state.selectedFile?.path === p ? state.selectedFile : null);
            for (const t of (data?.tags || [])) {
                ops.push(apiPost('/api/tag', { path: p, tags: [t.value ? `${t.name}=${t.value}` : t.name], dir, subject }));
            }
        }
    }
    await Promise.all(ops);

    const parts = [];
    if (toAdd.length)    parts.push(`+${toAdd.length} tag${toAdd.length === 1 ? '' : 's'}`);
    if (toRemove.length) parts.push(`-${toRemove.length} tag${toRemove.length === 1 ? '' : 's'}`);
    if (subjectChanged)  parts.push(subject ? `subject: ${subject}` : 'subject removed');
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
    state.tagPickerSubject = null;
    state.tagPickerOriginalSubject = null;
    renderTags();
    renderDetail();
}

window.doSearch = doSearch;
window.doClearSearch = doClearSearch;

// ---------------------------------------------------------------------------
// File management: context menu + operations
// ---------------------------------------------------------------------------

function showFileMenu(e, path, isDir, rootId) {
    e.preventDefault();
    e.stopPropagation();
    closeFileMenu();

    // Use the provided rootId (from the card rendering context), or fall back
    // to the currently active root. Never construct absolute paths.
    rootId = rootId ?? state.currentRootId;

    // When path is empty the target is the root directory itself (no rename/move/trash).
    const isRootDir = (path === '' || path == null);
    const name = isRootDir
        ? ((state.roots || []).find(r => r.id === rootId)?.name || 'Root')
        : path.split('/').pop();

    const menu = document.createElement('div');
    menu.id = 'file-context-menu';
    menu.className = 'tag-context-menu';

    let html = `<div class="tag-menu-header">${esc(name)}</div>`;
    if (!isRootDir) {
        html += `
        <button class="tag-menu-action" onclick="closeFileMenu(); promptRename(${rootId},'${jesc(path)}',${isDir})">Rename\u2026</button>
        <button class="tag-menu-action" onclick="closeFileMenu(); promptMove(${rootId},'${jesc(path)}',${isDir})">Move to\u2026</button>`;
    }
    if (isDir) {
        if (!isRootDir) html += `<div class="tag-menu-divider"></div>`;
        html += `
        <button class="tag-menu-action" onclick="closeFileMenu(); promptMkdir(${rootId},'${jesc(path || '')}')">New folder\u2026</button>`;
    }
    if (!isRootDir) {
        html += `
        <div class="tag-menu-divider"></div>
        <button class="tag-menu-action tag-menu-delete" onclick="closeFileMenu(); trashItem(${rootId},'${jesc(path)}',${isDir})">Move to Trash</button>`;
    }
    menu.innerHTML = html;

    document.body.appendChild(menu);

    // Position the menu near the cursor, clamped to viewport.
    const rect = menu.getBoundingClientRect();
    let x = e.clientX;
    let y = e.clientY;
    if (x + rect.width  > window.innerWidth)  x = window.innerWidth  - rect.width  - 8;
    if (y + rect.height > window.innerHeight) y = window.innerHeight - rect.height - 8;
    menu.style.left = x + 'px';
    menu.style.top  = y + 'px';

    requestAnimationFrame(() => {
        document.addEventListener('click', closeFileMenu, { once: true });
    });
}

function closeFileMenu() {
    const m = document.getElementById('file-context-menu');
    if (m) m.remove();
}

// ---------------------------------------------------------------------------
// Shared dialog helpers
// ---------------------------------------------------------------------------

/** Remove the active fs-dialog overlay (if any). */
function _closeFsDialog() {
    const d = document.getElementById('fs-dialog-overlay');
    if (d) d.remove();
}

/**
 * Show a small inline dialog.
 * @param {string} title
 * @param {string} html        — inner HTML for the form body
 * @param {function} onSubmit  — called with the FormData when committed
 */
function _showFsDialog(title, html, onSubmit) {
    _closeFsDialog();

    const overlay = document.createElement('div');
    overlay.id = 'fs-dialog-overlay';
    overlay.className = 'fs-dialog-overlay';
    overlay.innerHTML = `
        <div class="fs-dialog" role="dialog">
            <div class="fs-dialog-header">
                <span>${esc(title)}</span>
                <button class="fs-dialog-close" onclick="_closeFsDialog()">\u00d7</button>
            </div>
            <div class="fs-dialog-body">${html}</div>
        </div>
    `;
    document.body.appendChild(overlay);

    // Close on backdrop click.
    overlay.addEventListener('click', e => { if (e.target === overlay) _closeFsDialog(); });

    // Wire up the submit button (id="fs-dialog-submit").
    const btn = overlay.querySelector('#fs-dialog-submit');
    if (btn) {
        btn.addEventListener('click', () => {
            const form = overlay.querySelector('form');
            onSubmit(form ? new FormData(form) : null);
        });
    }

    // Auto-focus the first input.
    requestAnimationFrame(() => {
        const inp = overlay.querySelector('input[type=text]');
        if (inp) { inp.focus(); inp.select(); }
    });
}

// ---------------------------------------------------------------------------
// Rename
// ---------------------------------------------------------------------------

/**
 * Invalidate the filetree cache for a set of affected absolute directories and
 * re-render the sidebar tree. Call this after any filesystem operation.
 * @param {...string} absDirs  — absolute paths of directories that changed
 */
function _ftreeRefresh(...absDirs) {
    if (typeof ftreeInvalidateDir !== 'function') return;
    if (absDirs.length === 0) {
        if (typeof ftreeClearCache === 'function') ftreeClearCache();
    } else {
        for (const d of absDirs) { if (d) ftreeInvalidateDir(d); }
    }
    if (typeof renderFiletree === 'function') renderFiletree();
}

function promptRename(rootId, relPath, isDir) {
    const name = relPath.split('/').pop();
    _showFsDialog('Rename', `
        <form onsubmit="return false">
            <label class="fs-dialog-label">New name</label>
            <input class="fs-dialog-input" type="text" name="new_name" value="${esc(name)}" autocomplete="off" spellcheck="false"
                onkeydown="if(event.key==='Enter'){document.getElementById('fs-dialog-submit').click();}if(event.key==='Escape'){_closeFsDialog();}">
        </form>
        <div class="fs-dialog-footer">
            <button class="fs-dialog-btn" onclick="_closeFsDialog()">Cancel</button>
            <button class="fs-dialog-btn primary" id="fs-dialog-submit">Rename</button>
        </div>
    `, async fd => {
        const newName = (fd.get('new_name') || '').trim();
        if (!newName || newName === name) { _closeFsDialog(); return; }
        try {
            await apiPost('/api/fs/rename', { root_id: rootId, rel_path: relPath, new_name: newName });
            _closeFsDialog();

            // Compute the parent-relative path and new relative path of the renamed item.
            const parentRel = relPath.includes('/')
                ? relPath.slice(0, relPath.lastIndexOf('/'))
                : '';
            const newRelPath = parentRel ? parentRel + '/' + newName : newName;

            // If the current view is inside the renamed directory (or IS it), follow the
            // rename so that loadFiles does not request a path that no longer exists.
            let pathToLoad = state.currentPath;
            if (isDir && state.currentRootId === rootId && state.currentPath &&
                    (state.currentPath === relPath || state.currentPath.startsWith(relPath + '/'))) {
                pathToLoad = newRelPath + state.currentPath.slice(relPath.length);
            }

            await loadFiles(pathToLoad);
            render();

            // Invalidate the parent directory of the renamed item in the file tree
            // (not just the root) so that nested tree entries are refreshed correctly.
            const root = (state.roots || []).find(r => r.id === rootId);
            const parentAbs = root
                ? (parentRel ? root.path + '/' + parentRel : root.path)
                : state.currentBasePath;
            _ftreeRefresh(parentAbs);
        } catch (err) {
            alert('Rename failed: ' + err.message);
        }
    });
}

// ---------------------------------------------------------------------------
// New folder (mkdir)
// ---------------------------------------------------------------------------

/**
 * Show a context menu with a "New folder…" action for the content area's
 * empty space (right-click not on a card).
 */
function showContentAreaMenu(e) {
    // Ignore clicks that land on a card or interactive element.
    if (e.target.closest('.card, .tag-context-menu, .fs-dialog-overlay')) return;
    // Only available in browse mode with an active root.
    if (state.mode === 'zip' || state.currentRootId == null) return;

    e.preventDefault();
    e.stopPropagation();
    closeFileMenu();

    const rootId = state.currentRootId;
    const currentDir = state.currentPath || '';

    const menu = document.createElement('div');
    menu.id = 'file-context-menu';
    menu.className = 'tag-context-menu';
    menu.innerHTML = `
        <button class="tag-menu-action" onclick="closeFileMenu(); promptMkdir(${rootId},'${jesc(currentDir)}')">New folder\u2026</button>
    `;
    document.body.appendChild(menu);

    const rect = menu.getBoundingClientRect();
    let x = e.clientX;
    let y = e.clientY;
    if (x + rect.width  > window.innerWidth)  x = window.innerWidth  - rect.width  - 8;
    if (y + rect.height > window.innerHeight) y = window.innerHeight - rect.height - 8;
    menu.style.left = x + 'px';
    menu.style.top  = y + 'px';

    requestAnimationFrame(() => {
        document.addEventListener('click', closeFileMenu, { once: true });
    });
}

/**
 * Prompt the user for a new folder name and create it via POST /api/fs/mkdir.
 * @param {number} rootId       — root ID of the parent directory
 * @param {string} parentRelDir — path of the parent directory relative to the root
 *                                (empty string = the root itself)
 */
function promptMkdir(rootId, parentRelDir) {
    _showFsDialog('New folder', `
        <form onsubmit="return false">
            <label class="fs-dialog-label">Folder name</label>
            <input class="fs-dialog-input" type="text" name="dir_name" value="New folder" autocomplete="off" spellcheck="false"
                onkeydown="if(event.key==='Enter'){document.getElementById('fs-dialog-submit').click();}if(event.key==='Escape'){_closeFsDialog();}">
        </form>
        <div class="fs-dialog-footer">
            <button class="fs-dialog-btn" onclick="_closeFsDialog()">Cancel</button>
            <button class="fs-dialog-btn primary" id="fs-dialog-submit">Create</button>
        </div>
    `, async fd => {
        const name = (fd.get('dir_name') || '').trim();
        if (!name) { _closeFsDialog(); return; }
        try {
            await apiPost('/api/fs/mkdir', { root_id: rootId, rel_path: parentRelDir, name });
            _closeFsDialog();

            // Refresh the file listing if we are currently viewing the parent dir.
            if (state.currentRootId === rootId &&
                    (state.currentPath || '') === (parentRelDir || '')) {
                await loadFiles(state.currentPath);
                render();
            }

            // Invalidate the filetree cache for the parent directory.
            const root = (state.roots || []).find(r => r.id === rootId);
            const parentAbs = root
                ? (parentRelDir ? root.path + '/' + parentRelDir : root.path)
                : state.currentBasePath;
            _ftreeRefresh(parentAbs);
        } catch (err) {
            alert('Could not create folder: ' + err.message);
        }
    });
}

// ---------------------------------------------------------------------------
// Directory picker dialog  (for Move and Copy)
// ---------------------------------------------------------------------------

/** Cache of fetched children: absPath → [{name, is_dir, ...}] */
const _dpCache = {};
/** Which paths are expanded in the picker */
const _dpExpanded = {};
/** Currently selected directory in the picker */
let _dpSelected = null;
/** Callback once the user confirms a directory */
let _dpOnSelect = null;

function _closeDirPicker() {
    const el = document.getElementById('dir-picker-overlay');
    if (el) el.remove();
    _dpSelected = null;
    _dpOnSelect = null;
}

/**
 * Show a directory-picker dialog.
 * @param {string} title
 * @param {string} initialDir  — pre-selected directory (absolute path)
 * @param {function} onSelect  — called with the chosen absolute directory path
 */
async function _showDirPickerDialog(title, initialDir, onSelect) {
    _closeDirPicker();
    _dpOnSelect = onSelect;
    _dpSelected = initialDir;

    const overlay = document.createElement('div');
    overlay.id = 'dir-picker-overlay';
    overlay.className = 'fs-dialog-overlay';
    overlay.innerHTML = `
        <div class="fs-dialog dir-picker-dialog" role="dialog">
            <div class="fs-dialog-header">
                <span>${esc(title)}</span>
                <button class="fs-dialog-close" onclick="_closeDirPicker()">\u00d7</button>
            </div>
            <div class="fs-dialog-body">
                <div id="dir-picker-tree" class="dir-picker-tree"></div>
                <div id="dir-picker-selected" class="dir-picker-selected"></div>
            </div>
            <div class="fs-dialog-footer">
                <button class="fs-dialog-btn" onclick="_closeDirPicker()">Cancel</button>
                <button class="fs-dialog-btn primary" id="dir-picker-confirm" onclick="_dpConfirm()">Move here</button>
            </div>
        </div>
    `;
    document.body.appendChild(overlay);
    overlay.addEventListener('click', e => { if (e.target === overlay) _closeDirPicker(); });

    // Expand path to the initial dir.
    await _dpExpandToPath(initialDir);
    _dpRender();
    _dpUpdateSelected(initialDir);
}

/** Expand all ancestor directories of absPath in the picker. */
async function _dpExpandToPath(absPath) {
    const roots = state.roots || [];
    // Find which root this path belongs to.
    const root = roots.find(r => absPath === r.path || absPath.startsWith(r.path + '/'));
    if (!root) return;

    // Preload the root itself.
    if (!_dpCache[root.path]) await _dpLoad(root.path);
    _dpExpanded[root.path] = true;

    // Walk down the path segments.
    const rel = absPath.slice(root.path.length).replace(/^\//, '');
    if (!rel) return;
    const segs = rel.split('/');
    let current = root.path;
    for (const seg of segs) {
        current = current + '/' + seg;
        if (!_dpCache[current]) await _dpLoad(current);
        _dpExpanded[current] = true;
    }
}

/** Load directory children (dirs only) into _dpCache. */
async function _dpLoad(absPath) {
    try {
        // Resolve root_id + rel_path for the request; avoids sending absolute
        // system paths to the API.
        const root = state.roots.find(r => absPath === r.path || absPath.startsWith(r.path + '/'));
        let url;
        if (root) {
            const rel = absPath.slice(root.path.length).replace(/^\//, '');
            const params = new URLSearchParams({ root_id: root.id });
            if (rel) params.set('path', rel);
            url = '/api/files?' + params.toString();
        } else {
            url = '/api/files?dir=' + encodeURIComponent(absPath);
        }
        const r = await fetch(url);
        if (!r.ok) { _dpCache[absPath] = []; return; }
        const data = await r.json();
        _dpCache[absPath] = (data.entries || []).filter(e => e.is_dir);
    } catch (_) {
        _dpCache[absPath] = [];
    }
}

function _dpRender() {
    const el = document.getElementById('dir-picker-tree');
    if (!el) return;
    const roots = state.roots || [];
    el.innerHTML = roots.map(r => _dpRenderRoot(r)).join('');
}

function _dpRenderRoot(root) {
    const expanded = _dpExpanded[root.path] !== false;
    const selCls = _dpSelected === root.path ? ' dp-selected' : '';
    const chevCls = expanded ? '' : ' chevron-collapsed';
    const children = expanded
        ? _dpRenderChildren(root.path, _dpCache[root.path] || [], 1)
        : '';
    return `<div>
        <div class="dp-row dp-root${selCls}" style="padding-left:4px"
            onclick="_dpSelectDir('${jesc(root.path)}')"
            ondblclick="event.stopPropagation()">
            <svg class="chevron-icon${chevCls} dp-chevron" viewBox="0 0 12 12" width="11" height="11"
                onclick="event.stopPropagation();_dpToggle('${jesc(root.path)}')">
                <polyline points="2,3 6,8 10,3" fill="none" stroke="currentColor" stroke-width="1.4"
                    stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
            <span class="dp-label">${esc(root.name)}</span>
        </div>
        ${children}
    </div>`;
}

function _dpRenderChildren(parentAbs, entries, depth) {
    if (!entries || !entries.length) return '';
    return entries.map(e => _dpRenderEntry(e, parentAbs, depth)).join('');
}

function _dpRenderEntry(e, parentAbs, depth) {
    const abs = parentAbs.replace(/\/$/, '') + '/' + e.name;
    const indent = 4 + depth * 16;
    const expanded = !!_dpExpanded[abs];
    const selCls = _dpSelected === abs ? ' dp-selected' : '';
    const chevCls = expanded ? '' : ' chevron-collapsed';
    const children = expanded
        ? _dpRenderChildren(abs, _dpCache[abs] || [], depth + 1)
        : '';
    return `<div>
        <div class="dp-row${selCls}" style="padding-left:${indent}px"
            onclick="_dpSelectDir('${jesc(abs)}')"
            ondblclick="event.stopPropagation()">
            <svg class="chevron-icon${chevCls} dp-chevron" viewBox="0 0 12 12" width="11" height="11"
                onclick="event.stopPropagation();_dpToggle('${jesc(abs)}')">
                <polyline points="2,3 6,8 10,3" fill="none" stroke="currentColor" stroke-width="1.4"
                    stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
            <svg class="dp-folder-icon" viewBox="0 0 16 14" width="13" height="13" fill="none"
                stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round">
                <path d="M1 3.5A1.5 1.5 0 0 1 2.5 2H6l1.5 1.5H13.5A1.5 1.5 0 0 1 15 5v6.5A1.5 1.5 0 0 1 13.5 13h-11A1.5 1.5 0 0 1 1 11.5V3.5z"/>
            </svg>
            <span class="dp-label">${esc(e.name)}</span>
        </div>
        ${children}
    </div>`;
}

async function _dpToggle(absPath) {
    if (_dpExpanded[absPath]) {
        _dpExpanded[absPath] = false;
        _dpRender();
    } else {
        _dpExpanded[absPath] = true;
        if (!_dpCache[absPath]) await _dpLoad(absPath);
        _dpRender();
    }
}

function _dpSelectDir(absPath) {
    _dpSelected = absPath;
    _dpUpdateSelected(absPath);
    // Re-render to update highlight.
    _dpRender();
}

function _dpUpdateSelected(absPath) {
    const el = document.getElementById('dir-picker-selected');
    if (el) el.textContent = absPath || '(none)';
}

function _dpConfirm() {
    if (!_dpSelected) return;
    // Decompose absolute path into root_id + rel_path so the caller never
    // needs to deal with system paths.
    const root = state.roots.find(r => _dpSelected === r.path || _dpSelected.startsWith(r.path + '/'));
    const destInfo = root
        ? { rootId: root.id, relDir: _dpSelected.slice(root.path.length).replace(/^\//, '') }
        : { rootId: null, relDir: _dpSelected }; // graceful fallback
    const cb = _dpOnSelect;
    if (cb) cb(destInfo);
}

// ---------------------------------------------------------------------------
// Move
// ---------------------------------------------------------------------------

function promptMove(rootId, relPath, isDir) {
    // Determine initial dir for the picker (parent directory of the item).
    const root = state.roots.find(r => r.id === rootId);
    const parentRel = relPath.includes('/') ? relPath.slice(0, relPath.lastIndexOf('/')) : '';
    const initialDir = root ? (parentRel ? root.path + '/' + parentRel : root.path) : null;
    _showDirPickerDialog('Move to\u2026', initialDir || state.currentBasePath, async destInfo => {
        try {
            await apiPost('/api/fs/move', {
                root_id: rootId,
                rel_path: relPath,
                dest_root_id: destInfo.rootId,
                dest_rel_dir: destInfo.relDir,
            });
            _closeDirPicker();
            await loadFiles(state.currentPath);
            render();
            showToast('Moved.');
            _ftreeRefresh();
        } catch (err) {
            alert('Move failed: ' + err.message);
        }
    });
}

// ---------------------------------------------------------------------------
// Copy
// ---------------------------------------------------------------------------

function promptCopy(rootId, relPath) {
    const name = relPath.split('/').pop();
    const dot = name.lastIndexOf('.');
    const stem = dot > 0 ? name.slice(0, dot) : name;
    const ext  = dot > 0 ? name.slice(dot) : '';
    const defaultName = `Copy of ${stem}${ext}`;
    const root = state.roots.find(r => r.id === rootId);
    const parentRel = relPath.includes('/') ? relPath.slice(0, relPath.lastIndexOf('/')) : '';
    const initialDir = root ? (parentRel ? root.path + '/' + parentRel : root.path) : state.currentBasePath;

    _showDirPickerDialog('Copy to\u2026', initialDir, async destInfo => {
        // Ask for the new filename after the directory is chosen.
        _closeDirPicker();
        const destLabel = destInfo.rootId != null
            ? (state.roots.find(r => r.id === destInfo.rootId)?.name ?? '') +
              (destInfo.relDir ? '/' + destInfo.relDir : '')
            : destInfo.relDir;
        _showFsDialog('Copy file', `
            <form onsubmit="return false">
                <label class="fs-dialog-label">New filename</label>
                <input class="fs-dialog-input" type="text" name="new_name" value="${esc(defaultName)}" autocomplete="off" spellcheck="false"
                    onkeydown="if(event.key==='Enter'){document.getElementById('fs-dialog-submit').click();}if(event.key==='Escape'){_closeFsDialog();}">
                <div class="fs-dialog-hint">Destination: ${esc(destLabel)}</div>
            </form>
            <div class="fs-dialog-footer">
                <button class="fs-dialog-btn" onclick="_closeFsDialog()">Cancel</button>
                <button class="fs-dialog-btn primary" id="fs-dialog-submit">Copy</button>
            </div>
        `, async fd => {
            const newName = (fd.get('new_name') || '').trim();
            if (!newName) { _closeFsDialog(); return; }
            try {
                await apiPost('/api/fs/copy', {
                    root_id: rootId,
                    rel_path: relPath,
                    dest_root_id: destInfo.rootId,
                    dest_rel_dir: destInfo.relDir,
                    new_name: newName,
                });
                _closeFsDialog();
                await loadFiles(state.currentPath);
                render();
                _ftreeRefresh();
            } catch (err) {
                alert('Copy failed: ' + err.message);
            }
        });
    });
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

function confirmDelete(rootId, relPath, isDir) {
    const name = relPath.split('/').pop();
    const what = isDir ? 'directory' : 'file';
    _showFsDialog(`Delete ${what}`, `
        <div class="fs-dialog-warn">
            Are you sure you want to permanently delete:<br>
            <strong>${esc(name)}</strong>${isDir ? '<br><em>All contents will be deleted.</em>' : ''}
        </div>
        <div class="fs-dialog-footer">
            <button class="fs-dialog-btn" onclick="_closeFsDialog()">Cancel</button>
            <button class="fs-dialog-btn danger" id="fs-dialog-submit">Delete</button>
        </div>
    `, async () => {
        try {
            await apiPost('/api/fs/delete', { root_id: rootId, rel_path: relPath });
            _closeFsDialog();
            // If the deleted item was selected, clear selection.
            if (state.selectedFile && (state.selectedFile.path === relPath || state.selectedFile.path?.endsWith('/' + name) || state.selectedFile.path === name)) {
                state.selectedFile = null;
            }
            // selectedPaths uses root-relative paths; remove any matching entry.
            for (const p of state.selectedPaths) {
                if (relPath.endsWith('/' + p) || p === relPath) { state.selectedPaths.delete(p); break; }
            }
            await loadFiles(state.currentPath);
            render();
            _ftreeRefresh(state.currentBasePath);
        } catch (err) {
            alert('Delete failed: ' + err.message);
        }
    });
}

// ---------------------------------------------------------------------------
// Trash
// ---------------------------------------------------------------------------

/** Move a file or directory to the trash (no confirmation needed). */
async function trashItem(rootId, relPath, isDir) {
    try {
        await apiPost('/api/trash/move', { root_id: rootId, rel_path: relPath });
    } catch (err) {
        alert('Could not move to trash: ' + err.message);
        return;
    }

    // Clear selection if the trashed item was selected.
    if (state.selectedFile && (state.selectedFile.path === relPath ||
            state.selectedFile.path === relPath.split('/').pop())) {
        state.selectedFile = null;
    }
    for (const p of state.selectedPaths) {
        if (relPath.endsWith('/' + p) || p === relPath) { state.selectedPaths.delete(p); break; }
    }

    // If the trashed directory is (or contains) the currently viewed directory,
    // navigate to its parent so we don't try to list a now-deleted path.
    const trashedAbs  = state.currentBasePath + (relPath ? '/' + relPath : '');
    const currentAbs  = state.currentPath ? state.currentBasePath + '/' + state.currentPath : state.currentBasePath;
    if (isDir && (currentAbs === trashedAbs || currentAbs.startsWith(trashedAbs + '/'))) {
        const parentRel = relPath.includes('/') ? relPath.split('/').slice(0, -1).join('/') : '';
        await navigateTo(parentRel);
    } else {
        await loadFiles(state.currentPath);
        render();
    }

    _ftreeRefresh(state.currentPath
        ? state.currentBasePath + '/' + state.currentPath
        : state.currentBasePath);
    updateTrashBadge(rootId);
    showToast('Moved to trash. <a href="#" onclick="openTrashPanel();return false;">View</a>', 5000);
}

/** Open the trash panel for the current root. */
function openTrashPanel() {
    const rootId = _currentRootId();
    if (rootId == null) return;
    document.getElementById('trash-panel').removeAttribute('hidden');
    loadTrashItems(rootId);
}

function closeTrashPanel() {
    document.getElementById('trash-panel').setAttribute('hidden', '');
}

function _currentRootId() {
    const root = state.roots.find(r => r.path === state.currentBasePath) ||
        (state.roots.length ? state.roots[0] : null);
    return root ? root.id : null;
}

async function loadTrashItems(rootId) {
    try {
        const data = await api(`/api/trash?root_id=${rootId}`);
        renderTrashPanel(rootId, data.items || []);
    } catch (err) {
        document.getElementById('trash-list').innerHTML =
            `<div class="trash-empty-msg">Error loading trash: ${esc(err.message)}</div>`;
    }
}

function renderTrashPanel(rootId, items) {
    const list = document.getElementById('trash-list');
    const emptyBtn = document.getElementById('trash-empty-btn');
    emptyBtn.disabled = items.length === 0;

    if (items.length === 0) {
        list.innerHTML = '<div class="trash-empty-msg">Trash is empty.</div>';
        return;
    }

    list.innerHTML = items.map(item => `
        <div class="trash-item" data-id="${esc(item.trash_id)}">
            <span class="trash-item-icon">${item.is_dir ? '📁' : '📄'}</span>
            <div class="trash-item-info">
                <div class="trash-item-name" title="${esc(item.original_rel_path)}">${esc(item.original_name)}</div>
                <div class="trash-item-path">${esc(item.original_rel_path)}</div>
                <div class="trash-item-date">${esc(item.trashed_at.replace('T', ' ').replace('Z', ' UTC'))}</div>
            </div>
            <div class="trash-item-actions">
                <button class="trash-btn restore" title="Restore"
                    onclick="restoreTrashItem(${rootId},'${esc(item.trash_id)}')">↩ Restore</button>
                <button class="trash-btn delete" title="Delete permanently"
                    onclick="deleteTrashItem(${rootId},'${esc(item.trash_id)}')">✕</button>
            </div>
        </div>
    `).join('');
}

async function restoreTrashItem(rootId, trashId) {
    let result;
    try {
        result = await apiPost('/api/trash/restore', { root_id: rootId, trash_id: trashId });
    } catch (err) {
        if (err.message && err.message.startsWith('conflict:')) {
            // Original location is occupied — offer to restore with a different name.
            if (!confirm('The original location is already occupied.\n\nRestore with a different name instead?')) return;
            try {
                result = await apiPost('/api/trash/restore', {
                    root_id: rootId, trash_id: trashId, rename_on_conflict: true,
                });
            } catch (err2) {
                alert('Restore failed: ' + err2.message);
                return;
            }
        } else {
            alert('Restore failed: ' + err.message);
            return;
        }
    }
    await loadFiles(state.currentPath);
    render();
    _ftreeRefresh();
    loadTrashItems(rootId);
    updateTrashBadge(rootId);
    const label = (result && result.restored_name) ? `Restored as \u201c${result.restored_name}\u201d.` : 'Restored.';
    showToast(label);
}

async function deleteTrashItem(rootId, trashId) {
    if (!confirm('Permanently delete this item? This cannot be undone.')) return;
    try {
        await apiPost('/api/trash/delete', { root_id: rootId, trash_id: trashId });
        loadTrashItems(rootId);
        updateTrashBadge(rootId);
    } catch (err) {
        alert('Delete failed: ' + err.message);
    }
}

async function emptyTrash(rootId) {
    if (rootId == null) rootId = _currentRootId();
    if (rootId == null) return;
    if (!confirm('Permanently delete everything in the trash? This cannot be undone.')) return;
    try {
        const res = await apiPost('/api/trash/empty', { root_id: rootId });
        loadTrashItems(rootId);
        updateTrashBadge(rootId);
        showToast(`Trash emptied (${res.deleted || 0} item${res.deleted === 1 ? '' : 's'} deleted).`);
    } catch (err) {
        alert('Could not empty trash: ' + err.message);
    }
}

async function updateTrashBadge(rootId) {
    try {
        const data = await api(`/api/trash?root_id=${rootId}`);
        const count = (data.items || []).length;
        const badge = document.getElementById('trash-badge');
        if (badge) {
            badge.textContent = count > 0 ? count : '';
            badge.style.display = count > 0 ? '' : 'none';
        }
    } catch (_) {}
}
