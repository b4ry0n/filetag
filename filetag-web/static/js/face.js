// ---------------------------------------------------------------------------
// Face detection UI
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// State additions
// ---------------------------------------------------------------------------

// Lazily extend `state` with face-specific fields (called once on DOMContentLoaded).
function _initFaceState() {
    if (!('faceConfig' in state)) {
        state.faceConfig = null;             // FaceConfigResponse | null
        state.faceDetections = [];           // ApiFaceDetection[] for the detail panel
        state.faceDetectionsPath = null;     // path for which detections were loaded
        state.faceDetecting = false;         // single-file detection in progress
        state.faceBoxesVisible = true;       // whether face boxes are shown on the preview
        state.people = [];                   // [{name, count, det_id}] for sidebar
        state.faceProgressTimer = null;      // setInterval handle for batch polling
        state.faceActivePerson = null;       // selected person name for sidebar highlight
        state.faceShowUnknown = false;       // sidebar: show unknown persons instead of named
    }
}

// ---------------------------------------------------------------------------
// Config + models
// ---------------------------------------------------------------------------

async function loadFaceConfig() {
    try {
        state.faceConfig = await api('/api/face/config' + dirParam('?'));
    } catch (_) {
        state.faceConfig = null;
    }
}

async function loadPeople() {
    try {
        state.people = await api('/api/face/subjects' + dirParam('?'));
    } catch (_) {
        state.people = [];
    }
}

// ---------------------------------------------------------------------------
// Sidebar people section
// ---------------------------------------------------------------------------

/**
 * Build the HTML for the People section that is injected into the sidebar.
 * Called from tags.js renderTags() after the tags tree.
 */
function renderPeopleSection() {
    if (!state.faceConfig) return '';

    // When the feature is disabled, show a compact enable-prompt instead.
    if (!state.faceConfig.enabled) {
        return `<div class="people-section-header">
            <span class="people-section-label">${esc(t('face.people-section'))}</span>
            <button class="people-detect-all-btn" onclick="openSettings('face')">${esc(t('face.enable-in-settings'))}</button>
        </div>`;
    }

    const batchRunning = state.faceProgressTimer !== null;
    const progressHtml = batchRunning ? _faceBatchProgressHtml() : '';

    const clusterBtn = `<button class="people-detect-all-btn" onclick="faceClusters()"
        title="${esc(t('face.cluster-btn-title'))}">${esc(t('face.cluster-btn'))}</button>`;
    const detectAllBtn = `<button class="people-detect-all-btn" onclick="faceDetectBatch()"
        ${batchRunning ? 'disabled' : ''}>${esc(t('face.detect-all'))}</button>`;

    const all = state.people || [];
    const named   = all.filter(p => !_faceIsUnknown(p.name));
    const unknown = all.filter(p =>  _faceIsUnknown(p.name));

    const makeItems = (list) => list.map(p => {
        const isActive = state.faceActivePerson === p.name;
        const thumbUrl = '/api/face/thumbnail?' + new URLSearchParams({ id: p.det_id }) + dirParam('&');
        const label = !p.name
            ? t('face.unassigned')
            : (p.name.startsWith('person/') ? p.name.slice(7) : p.name);
        const activeClass = isActive ? ' active' : '';
        const onclickArg = p.name ? `'${jesc(p.name)}'` : `''`;
        return `<button class="person-item${activeClass}" onclick="faceSelectPerson(${onclickArg})"
            title="${esc(p.name || t('face.unassigned'))}">
            <img class="person-thumb" src="${thumbUrl}" alt="${esc(label)}"
                 onerror="this.style.display='none';this.nextElementSibling.style.display='flex'">
            <span class="person-thumb-placeholder" style="display:none">&#x1F464;</span>
            <span class="person-label">${esc(label)}</span>
            <span class="person-count">${p.count}</span>
        </button>`;
    }).join('');

    // Unknown persons view
    if (state.faceShowUnknown) {
        const backBtn = `<button class="people-back-btn" onclick="faceToggleUnknown()">← ${esc(t('face.people-section'))}</button>`;
        const noUnknown = `<div class="face-progress-bar-wrap"><span style="font-size:11px;color:var(--text-secondary)">${esc(t('face.no-unknown'))}</span></div>`;
        return `<div class="people-section-header">
            ${backBtn}
            ${clusterBtn}
        </div>
        ${progressHtml}
        ${unknown.length === 0 ? noUnknown : makeItems(unknown)}`;
    }

    // Normal view: named persons only
    if (all.length === 0) {
        return `<div class="people-section-header">
            <span class="people-section-label">${esc(t('face.people-section'))}</span>
            <div style="display:flex;gap:4px">${clusterBtn}${detectAllBtn}</div>
        </div>
        ${progressHtml}
        <div class="face-progress-bar-wrap" style="color:var(--text-secondary)">
            <span style="font-size:11px">${esc(t('face.no-faces'))}</span>
        </div>`;
    }

    const unknownToggle = unknown.length > 0
        ? `<button class="people-unknown-toggle" onclick="faceToggleUnknown()">${unknown.length} ${esc(t('face.unknown-persons'))} ›</button>`
        : '';

    return `<div class="people-section-header">
        <span class="people-section-label">${esc(t('face.people-section'))}</span>
        <div style="display:flex;gap:4px">${clusterBtn}${detectAllBtn}</div>
    </div>
    ${progressHtml}
    ${makeItems(named)}
    ${unknownToggle}`;
}

/** Returns true when `name` is an auto-generated unknown cluster name or unassigned (empty). */
function _faceIsUnknown(name) {
    if (!name) return true;   // null, undefined, or empty string = unassigned
    const prefix = (state.faceConfig && state.faceConfig.tag_prefix) || 'person';
    return name.startsWith(prefix + '/unknown-');
}

/** Toggle between named-persons view and unknown-persons view in the sidebar. */
function faceToggleUnknown() {
    state.faceShowUnknown = !state.faceShowUnknown;
    // Deselect an unknown person when leaving the unknown view.
    if (!state.faceShowUnknown && state.faceActivePerson && _faceIsUnknown(state.faceActivePerson)) {
        state.faceActivePerson = null;
        doClearSearch();
    }
    renderTags();
}

function _faceBatchProgressHtml() {
    const p = state._faceBatchProgress || {};
    const pct = p.total > 0 ? Math.round((p.done / p.total) * 100) : 0;
    const label = p.total > 0
        ? `${p.done} / ${p.total}${p.current ? ' — ' + p.current.split('/').pop() : ''}`
        : t('face.detecting');
    return `<div class="face-progress-bar-wrap">
        <span>${esc(label)}</span>
        <div class="face-progress-bar"><div class="face-progress-bar-fill" style="width:${pct}%"></div></div>
    </div>`;
}

/** Filter grid to files containing this person. */
async function faceSelectPerson(name) {
    if (state.faceActivePerson === name) {
        // Toggle off
        state.faceActivePerson = null;
        doClearSearch();
        renderTags();
        return;
    }
    state.faceActivePerson = name;
    try {
        const data = await api(
            '/api/face/files?' +
            new URLSearchParams({ subject: name }) +
            dirParam('&')
        );
        const paths = data.paths || [];
        // Load these paths into search-result mode so the grid shows them.
        state.searchQuery = 'subject:' + name;
        state.searchResults = paths.map(p => ({ path: p, tags: [] }));
        state.mode = 'search';
        state.selectedFile = null;
        state.selectedPaths.clear();
        state.selectedFilesData.clear();
        render();
        renderTags();
    } catch (e) {
        showToast('Face select: ' + e.message, 4000);
    }
}

// ---------------------------------------------------------------------------
// Batch detect
// ---------------------------------------------------------------------------

async function faceDetectBatch() {
    if (state.faceProgressTimer) return; // already running
    try {
        await apiPost('/api/face/analyse-batch', {
            dir: currentAbsDir() || '',
            recursive: false,
        });
        _faceStartPolling();
    } catch (e) {
        showToast('Face batch: ' + e.message, 4000);
    }
}

function _faceStartPolling() {
    state._faceBatchProgress = null;
    state.faceProgressTimer = setInterval(async () => {
        try {
            const s = await api('/api/face/status');
            state._faceBatchProgress = s;
            if (!s.running) {
                clearInterval(state.faceProgressTimer);
                state.faceProgressTimer = null;
                await loadPeople();
                renderTags(); // refresh sidebar
            } else {
                renderTags(); // update progress bar
            }
        } catch (_) {
            clearInterval(state.faceProgressTimer);
            state.faceProgressTimer = null;
        }
    }, 1000);
    renderTags();
}

// ---------------------------------------------------------------------------
// Detail panel: detect button + face overlays
// ---------------------------------------------------------------------------

/**
 * Return HTML for the "Detect faces" toolbar strip to embed in the detail panel.
 * Only shown for image and raw files.
 */
function faceDetailToolbar(path) {
    if (!state.faceConfig) return '';
    if (!state.faceConfig.enabled) {
        return `<div class="face-toolbar">
            <button class="face-detect-btn" onclick="openSettings('face')">${esc(t('face.enable-in-settings'))}</button>
        </div>`;
    }
    if (!state.faceConfig.models_ready) {
        const downloading = !!state._faceModelsDownloading;
        if (downloading) {
            const s = state._faceDownloadStatus;
            const label = _faceDownloadLabel(s);
            const pct = s && s.percent != null ? s.percent : null;
            const barHtml = pct != null
                ? `<div class="face-progress-bar"><div class="face-progress-bar-fill" style="width:${pct}%"></div></div>`
                : `<div class="face-indeterminate-bar"></div>`;
            return `<div class="face-toolbar">
                <div class="face-download-progress">
                    <span>${esc(label)}</span>
                    ${barHtml}
                </div>
            </div>`;
        }
        return `<div class="face-toolbar">
            <span>${esc(t('face.models-missing'))}</span>
            <button class="face-detect-btn" onclick="faceDownloadModels()">${esc(t('face.download-models'))}</button>
        </div>`;
    }

    const isDetecting = state.faceDetecting;
    const dets = state.faceDetectionsPath === path ? state.faceDetections : [];
    // Show badge after analysis: green with count, or grey "0" when analysed but none found.
    const analysed = state.faceDetectionsPath === path;
    const countBadge = analysed
        ? `<span class="face-count-badge${dets.length === 0 ? ' face-count-none' : ''}">${dets.length}</span>`
        : '';

    const toggleBtn = analysed && dets.length > 0
        ? `<button class="face-toggle-boxes-btn${state.faceBoxesVisible ? ' active' : ''}"
               onclick="faceToggleBoxes('${jesc(path)}')"
               title="${esc(t('face.toggle-boxes'))}">
               <svg width="14" height="14" viewBox="0 0 20 14" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                   <ellipse cx="10" cy="7" rx="9" ry="6"/>
                   <circle cx="10" cy="7" r="2.8" fill="currentColor" stroke="none"/>
               </svg>
           </button>`
        : '';

    return `<div class="face-toolbar">
        <button class="face-detect-btn" id="face-detect-single-btn"
            onclick="faceDetectSingle('${jesc(path)}')"
            ${isDetecting ? 'disabled' : ''}>${isDetecting ? esc(t('face.detecting')) : esc(t('face.detect-btn'))}</button>
        ${countBadge}${toggleBtn}
    </div>`;
}

/** Toggle visibility of face bounding boxes on the detail preview. */
function faceToggleBoxes(path) {
    state.faceBoxesVisible = !state.faceBoxesVisible;
    _faceRefreshDetailControls(path);
    document.querySelectorAll('.face-preview-wrap .face-box').forEach(b => {
        b.style.display = state.faceBoxesVisible ? '' : 'none';
    });
}

/** Detect faces for a single file and update the detail panel. */
async function faceDetectSingle(path) {
    state.faceDetecting = true;
    _faceRefreshDetailControls(path);
    try {
        const result = await apiPost('/api/face/analyse', { path, dir: currentAbsDir() });
        state.faceDetections = result.detections || [];
        state.faceDetectionsPath = path;
        await loadPeople();
        renderTags(); // refresh sidebar
        if (state.faceDetections.length === 0) {
            showToast(t('face.no-faces-found'), 3000);
        }
    } catch (e) {
        showToast(t('face.detect-error') + ': ' + e.message, 6000);
        state.faceDetections = [];
    } finally {
        state.faceDetecting = false;
        _faceRefreshDetailControls(path);
        _faceRenderOverlays(path);
    }
}

/** Load existing detections for a file (no re-analysis). */
async function faceLoadDetections(path) {
    if (state.faceDetectionsPath === path) return; // already loaded
    try {
        const result = await api('/api/face/detections?' + new URLSearchParams({ path }) + dirParam('&'));
        state.faceDetections = result.detections || [];
        state.faceDetectionsPath = path;
    } catch (_) {
        state.faceDetections = [];
        state.faceDetectionsPath = path;
    }
    _faceRenderOverlays(path);
    _faceRefreshDetailControls(path);
}

/** Trigger cluster+assign for the current directory. */
async function faceClusters() {
    try {
        const r = await apiPost('/api/face/cluster', { dir: currentAbsDir() });
        showToast(t('face.cluster-done', { n: r.clusters }), 4000);
        await loadPeople();
        await loadSubjects();
        // Re-load detections for current file so labels update
        if (state.faceDetectionsPath) {
            state.faceDetectionsPath = null;
            await faceLoadDetections(state.selectedFile?.path || state.faceDetectionsPath);
        }
        renderTags();
    } catch (e) {
        showToast('Cluster: ' + e.message, 4000);
    }
}

/** Format bytes as human-readable string (KB / MB). */
function _faceFormatBytes(b) {
    if (b == null) return '';
    if (b >= 1024 * 1024) return (b / (1024 * 1024)).toFixed(1) + ' MB';
    if (b >= 1024) return (b / 1024).toFixed(0) + ' KB';
    return b + ' B';
}

/** Build the download status label from an /api/face/models/status response. */
function _faceDownloadLabel(s) {
    if (!s || !s.downloading) return t('face.models-downloading');
    const done = _faceFormatBytes(s.bytes_done);
    const total = s.bytes_total != null ? _faceFormatBytes(s.bytes_total) : null;
    const speed = s.speed_bps > 0 ? _faceFormatBytes(s.speed_bps) + '/s' : null;
    const phaseLbl = s.phase === 'embed' ? t('face.models-downloading-embed') : t('face.models-downloading-detect');
    let label = phaseLbl;
    if (done) label += ' — ' + done + (total ? ' / ' + total : '');
    if (speed) label += ' (' + speed + ')';
    return label;
}

async function faceDownloadModels() {
    if (state._faceModelsDownloading) return;
    try {
        await apiPost('/api/face/models/download', {});
        state._faceModelsDownloading = true;
        state._faceDownloadStatus = null;
        // Show animated progress bar in the toolbar immediately.
        if (state.selectedFile) _faceRefreshDetailControls(state.selectedFile.path);
        // Poll at 500 ms for live bytes + speed.
        const timer = setInterval(async () => {
            try {
                const s = await api('/api/face/models/status');
                state._faceDownloadStatus = s;
                // Refresh toolbar with fresh numbers.
                if (state.selectedFile) _faceRefreshDetailControls(state.selectedFile.path);
                if (s.models_ready) {
                    clearInterval(timer);
                    state._faceModelsDownloading = false;
                    state._faceDownloadStatus = null;
                    await loadFaceConfig();
                    // Update models status row in settings modal if open.
                    const statusEl = document.getElementById('face-models-status');
                    const downloadBtn = document.getElementById('face-models-download-btn');
                    if (statusEl) statusEl.textContent = t('face.settings-models-ready');
                    if (downloadBtn) downloadBtn.hidden = true;
                    if (state.selectedFile) _faceRefreshDetailControls(state.selectedFile.path);
                    renderTags();
                    showToast(t('face.models-ready'), 3000);
                } else if (s.error) {
                    clearInterval(timer);
                    state._faceModelsDownloading = false;
                    state._faceDownloadStatus = null;
                    if (state.selectedFile) _faceRefreshDetailControls(state.selectedFile.path);
                    showToast('Download failed: ' + s.error, 5000);
                }
            } catch (_) {
                clearInterval(timer);
                state._faceModelsDownloading = false;
                state._faceDownloadStatus = null;
            }
        }, 500);
    } catch (e) {
        state._faceModelsDownloading = false;
        showToast('Download failed: ' + e.message, 4000);
    }
}

// ---------------------------------------------------------------------------
// Overlay rendering
// ---------------------------------------------------------------------------

/**
 * Wrap the preview image in a .face-preview-wrap and paint bounding boxes.
 * Called after detections are loaded.
 */
function _faceRenderOverlays(path) {
    if (state.faceDetectionsPath !== path) return;
    const wrap = document.querySelector('.face-preview-wrap');
    if (!wrap) return;

    // Remove existing boxes
    wrap.querySelectorAll('.face-box').forEach(b => b.remove());

    const img = wrap.querySelector('img');
    if (!img || !img.complete || !img.naturalWidth) {
        img && img.addEventListener('load', () => _faceRenderOverlays(path), { once: true });
        return;
    }

    const scaleX = img.offsetWidth / img.naturalWidth;
    const scaleY = img.offsetHeight / img.naturalHeight;

    for (const det of state.faceDetections) {
        const box = document.createElement('div');
        box.className = 'face-box' + (det.subject_name ? ' assigned' : '');
        box.style.left   = (det.x * scaleX) + 'px';
        box.style.top    = (det.y * scaleY) + 'px';
        box.style.width  = (det.w * scaleX) + 'px';
        box.style.height = (det.h * scaleY) + 'px';
        box.title = det.subject_name || t('face.unknown');
        box.dataset.detId = det.id;
        box.onclick = (e) => { e.stopPropagation(); _faceShowAssignDialog(det, box); };

        const label = document.createElement('div');
        label.className = 'face-box-label';
        const name = det.subject_name
            ? (det.subject_name.startsWith('person/') ? det.subject_name.slice(7) : det.subject_name)
            : t('face.unknown');
        label.textContent = name;
        box.appendChild(label);
        if (!state.faceBoxesVisible) box.style.display = 'none';
        wrap.appendChild(box);
    }
}

/** Wrap the .preview-zoomable anchor's img inside a .face-preview-wrap div. */
function _faceWrapPreviewImg() {
    // Check if already wrapped
    if (document.querySelector('.face-preview-wrap')) return;
    const anchor = document.querySelector('#detail .preview-zoomable');
    if (!anchor) return;
    const img = anchor.querySelector('img');
    if (!img) return;

    const wrap = document.createElement('div');
    wrap.className = 'face-preview-wrap';
    anchor.parentNode.insertBefore(wrap, anchor);
    wrap.appendChild(anchor);
}

/** Refresh only the face toolbar div without rebuilding the full detail panel. */
function _faceRefreshDetailControls(path) {
    const el = document.getElementById('face-toolbar-row');
    if (!el) return;
    el.innerHTML = faceDetailToolbar(path);
}

// ---------------------------------------------------------------------------
// Assign dialog
// ---------------------------------------------------------------------------

let _faceAssignDialog = null;

function _faceShowAssignDialog(det, boxEl) {
    _faceCloseAssignDialog();

    const existing = det.subject_name
        ? (det.subject_name.startsWith('person/') ? det.subject_name.slice(7) : det.subject_name)
        : '';

    const dialog = document.createElement('div');
    dialog.className = 'face-assign-dialog';
    dialog.innerHTML = `<h4>${esc(t('face.assign-title'))}</h4>
        <input type="text" id="face-assign-input" value="${esc(existing)}"
               placeholder="${esc(t('face.assign-label'))}" autocomplete="off">
        <div id="face-assign-suggestions" class="face-assign-suggestions" hidden></div>
        <div class="face-assign-btns">
            <button class="face-delete-btn" onclick="_faceDoDelete(${det.id})" title="${esc(t('face.delete-title'))}">${esc(t('face.delete-btn'))}</button>
            <button onclick="_faceCloseAssignDialog()">${esc(t('face.assign-cancel'))}</button>
            <button class="primary" onclick="_faceDoAssign(${det.id})">${esc(t('face.assign-ok'))}</button>
        </div>`;

    // Position near the box
    const rect = boxEl.getBoundingClientRect();
    dialog.style.left = Math.min(rect.left, window.innerWidth - 240) + 'px';
    dialog.style.top  = (rect.bottom + 6) + 'px';

    // Append inside the media-viewer when it is open (z-index 9500),
    // otherwise fall back to document.body.
    const viewerEl = document.getElementById('media-viewer');
    const container = (viewerEl && !viewerEl.hidden) ? viewerEl : document.body;
    container.appendChild(dialog);
    _faceAssignDialog = dialog;

    const inp = dialog.querySelector('#face-assign-input');
    inp.focus();
    inp.select();
    inp.addEventListener('keydown', e => {
        // Always stop propagation so viewer navigation keys don't fire while typing.
        e.stopPropagation();
        if (e.key === 'Enter') _faceDoAssign(det.id);
        if (e.key === 'Escape') _faceCloseAssignDialog();
    });

    // Load suggestions asynchronously if the face has an embedding.
    if (det.id) {
        _faceLoadSuggestions(det.id, dialog);
    }

    // Close when clicking outside
    const outsideHandler = (e) => {
        if (!dialog.contains(e.target)) _faceCloseAssignDialog();
    };
    setTimeout(() => document.addEventListener('click', outsideHandler, { once: true }), 10);
}

async function _faceLoadSuggestions(detId, dialog) {
    // Prefer currentDir; fall back to the root_path returned with the last
    // file listing (state.currentRootPath) so the backend can find the DB
    // even when browsing from a top-level virtual root where currentDir is empty.
    const dir = state.currentDir || state.currentRootPath || '';
    try {
        const qs = new URLSearchParams({ detection_id: detId, dir });
        const res = await fetch('/api/face/suggest?' + qs);
        if (!res.ok) return;
        const data = await res.json();
        const suggestions = data.suggestions || [];
        if (!suggestions.length) return;

        // Dialog may have been closed while we were fetching.
        if (!dialog.isConnected) return;

        const container = dialog.querySelector('#face-assign-suggestions');
        if (!container) return;

        const heading = document.createElement('div');
        heading.className = 'face-suggestions-heading';
        heading.textContent = t('face.suggestions');

        const chips = suggestions.map(s => {
            const pct = Math.round((1 - s.distance) * 100);
            const chip = document.createElement('button');
            chip.type = 'button';
            chip.className = 'face-suggestion-chip' + (s.auto ? ' face-suggestion-auto' : '');
            chip.title = `${s.name} (${pct}%)`;
            chip.innerHTML = `<span class="chip-name">${esc(s.label)}</span><span class="chip-pct">${pct}%</span>`;
            chip.addEventListener('click', () => {
                const inp = dialog.querySelector('#face-assign-input');
                if (inp) { inp.value = s.label; inp.focus(); }
            });
            return chip;
        });

        container.append(heading, ...chips);
        container.hidden = false;
    } catch (_) {
        // Silently ignore errors (embeddings may not exist yet).
    }
}

function _faceCloseAssignDialog() {
    if (_faceAssignDialog) {
        _faceAssignDialog.remove();
        _faceAssignDialog = null;
    }
}

async function _faceDoAssign(detId) {
    const inp = document.getElementById('face-assign-input');
    const rawName = inp ? inp.value.trim() : '';
    _faceCloseAssignDialog();

    // Build full subject name with prefix
    const prefix = (state.faceConfig && state.faceConfig.tag_prefix) ? state.faceConfig.tag_prefix : 'person';
    let subjectName = null;
    if (rawName) {
        subjectName = rawName.startsWith(prefix + '/') ? rawName : prefix + '/' + rawName;
    }

    try {
        await apiPost('/api/face/assign', {
            detection_id: detId,
            subject_name: subjectName,
            dir: currentAbsDir(),
        });
        // Update local detection record
        const det = state.faceDetections.find(d => d.id === detId);
        if (det) det.subject_name = subjectName;
        await loadPeople();
        await loadSubjects();
        renderTags();
        if (state.faceDetectionsPath) _faceRenderOverlays(state.faceDetectionsPath);
        // Also refresh viewer overlay if active
        if (_faceViewerActive && _faceViewerPath) await _faceApplyViewerOverlay(_faceViewerPath);
    } catch (e) {
        showToast('Assign: ' + e.message, 4000);
    }
}

/** Permanently delete a face detection (e.g. an unwanted bystander). */
async function _faceDoDelete(detId) {
    _faceCloseAssignDialog();
    try {
        await apiPost('/api/face/delete', {
            detection_ids: [detId],
            dir: currentAbsDir(),
        });
        // Remove from local state
        state.faceDetections = state.faceDetections.filter(d => d.id !== detId);
        await loadPeople();
        renderTags();
        if (state.faceDetectionsPath) _faceRenderOverlays(state.faceDetectionsPath);
        if (_faceViewerActive && _faceViewerPath) await _faceApplyViewerOverlay(_faceViewerPath);
    } catch (e) {
        showToast('Delete: ' + e.message, 4000);
    }
}

// ---------------------------------------------------------------------------
// Integration hooks called from detail.js / tags.js
// ---------------------------------------------------------------------------

/**
 * Called by renderDetail() after it has inserted the preview HTML.
 * Wraps the image in a face-preview-wrap and loads existing detections.
 */
async function faceOnDetailRendered(path, type_) {
    if (type_ !== 'image' && type_ !== 'raw') return;
    // Reset stale detection state so overlays won't show for a different file
    if (state.faceDetectionsPath !== path) {
        state.faceDetections = [];
        state.faceDetectionsPath = null;
    }
    // Show toolbar immediately using cached config
    _faceRefreshDetailControls(path);
    _faceWrapPreviewImg();
    await faceLoadDetections(path);
    // Refresh toolbar now that detections are known
    _faceRefreshDetailControls(path);
}

// ---------------------------------------------------------------------------
// Viewer overlay
// ---------------------------------------------------------------------------

let _faceViewerActive = false;
let _faceViewerPath   = null;

/** Toggle face overlays in the media viewer on/off. */
async function cvFaceToggle() {
    _faceViewerActive = !_faceViewerActive;
    const btn = document.getElementById('cv-face-btn');
    if (btn) btn.classList.toggle('active', _faceViewerActive);
    if (_faceViewerActive) {
        const img = document.querySelector('#cv-pages img.cv-page');
        if (img) {
            const filePath = _cv.mode === 'dir' ? _cv.filePaths[_cv.current] : null;
            await _faceApplyViewerOverlay(filePath);
        }
    } else {
        _faceClearViewerOverlay();
    }
}

/**
 * Called by viewer.js after the image has been decoded and inserted into the DOM.
 * Only acts if the face overlay is currently active.
 */
async function faceOnViewerPageChanged(filePath) {
    _faceViewerPath = filePath;
    if (!_faceViewerActive) return;
    await _faceApplyViewerOverlay(filePath);
}

/** Called when the media viewer is closed. */
function faceOnViewerClosed() {
    _faceViewerActive = false;
    _faceViewerPath = null;
    const btn = document.getElementById('cv-face-btn');
    if (btn) btn.classList.remove('active');
}

/** Wrap the current viewer image and render face boxes on it. */
async function _faceApplyViewerOverlay(filePath) {
    _faceClearViewerOverlay();
    if (!filePath) return;
    if (!state.faceConfig || !state.faceConfig.enabled) return;

    // Reuse cached detections when possible; otherwise fetch them.
    if (state.faceDetectionsPath !== filePath) {
        await faceLoadDetections(filePath);
    }

    const img = document.querySelector('#cv-pages img.cv-page');
    if (!img) return;

    const wrap = document.createElement('div');
    wrap.className = 'face-preview-wrap face-viewer-wrap';
    img.parentNode.insertBefore(wrap, img);
    wrap.appendChild(img);

    // Render boxes after the image dimensions are known.
    const doRender = () => {
        wrap.querySelectorAll('.face-box').forEach(b => b.remove());
        if (!img.naturalWidth) return;
        const scaleX = img.offsetWidth  / img.naturalWidth;
        const scaleY = img.offsetHeight / img.naturalHeight;

        for (const det of state.faceDetections) {
            const box = document.createElement('div');
            box.className = 'face-box' + (det.subject_name ? ' assigned' : '');
            box.style.left   = (det.x * scaleX) + 'px';
            box.style.top    = (det.y * scaleY) + 'px';
            box.style.width  = (det.w * scaleX) + 'px';
            box.style.height = (det.h * scaleY) + 'px';
            box.title = det.subject_name || t('face.unknown');
            box.dataset.detId = det.id;
            box.onclick = (e) => { e.stopPropagation(); _faceShowAssignDialog(det, box); };

            const label = document.createElement('div');
            label.className = 'face-box-label';
            const name = det.subject_name
                ? (det.subject_name.startsWith('person/') ? det.subject_name.slice(7) : det.subject_name)
                : t('face.unknown');
            label.textContent = name;
            box.appendChild(label);
            wrap.appendChild(box);
        }
    };

    if (img.complete && img.naturalWidth) {
        doRender();
    } else {
        img.addEventListener('load', doRender, { once: true });
    }
}

/** Remove any face-viewer-wrap and its boxes, restoring the img to cv-pages. */
function _faceClearViewerOverlay() {
    const wrap = document.querySelector('.face-viewer-wrap');
    if (!wrap) return;
    const img = wrap.querySelector('img');
    if (img) wrap.parentNode.insertBefore(img, wrap);
    wrap.remove();
    _faceCloseAssignDialog();
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

document.addEventListener('DOMContentLoaded', async () => {
    _initFaceState();
    // Load config once; sidebar/detail will use it after renderTags/renderDetail
    await loadFaceConfig();
    await loadPeople();
});
