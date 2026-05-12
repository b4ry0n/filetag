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
        state.faceImageWidth = 0;            // original image width (from API)
        state.faceImageHeight = 0;           // original image height (from API)
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
            <button class="people-action-btn" onclick="openSettings('face')">${esc(t('face.enable-in-settings'))}</button>
        </div>`;
    }

    const batchRunning = state.faceProgressTimer !== null;
    const progressHtml = batchRunning ? _faceBatchProgressHtml() : '';

    const clusterBtn = `<button class="people-action-btn" onclick="faceClusters()"
        title="${esc(t('face.cluster-btn-title'))}">${esc(t('face.cluster-btn'))}</button>`;
    const detectAllBtn = `<button class="people-action-btn" onclick="faceDetectBatch()"
        ${batchRunning ? 'disabled' : ''}>${esc(t('face.detect-all'))}</button>`;
    const toolbar = `<div class="people-section-toolbar">${detectAllBtn}${clusterBtn}</div>`;

    const all = state.people || [];
    const named   = all.filter(p => p.name && !_faceIsUnknown(p.name));
    const unknown = all.filter(p => _faceIsUnknown(p.name));

    const sorted = (list) => {
        const s = [...list];
        if (state.tagSortMode === 'count') {
            s.sort((a, b) => b.count - a.count || (a.name || '').localeCompare(b.name || ''));
        } else {
            s.sort((a, b) => (a.name || '').localeCompare(b.name || ''));
        }
        return s;
    };

    const makeItem = (p, isUnknown) => {
        const isActive = state.faceActivePerson === p.name;
        const thumbUrl = '/api/face/thumbnail?' + new URLSearchParams({ id: p.det_id }) + dirParam('&');
        let label;
        if (!p.name) {
            label = t('face.unassigned');
        } else if (isUnknown) {
            label = t('face.unknown-person');
        } else {
            const prefix = (state.faceConfig && state.faceConfig.tag_prefix) || 'person';
            label = p.name.startsWith(prefix + '/') ? p.name.slice(prefix.length + 1) : p.name;
        }
        const activeClass = isActive ? ' active' : '';
        const unknownClass = isUnknown ? ' unknown' : '';
        const onclickArg = p.name ? `'${jesc(p.name)}'` : `''`;
        const thumbHtml = isUnknown
            ? `<span class="person-thumb person-thumb-unknown">&#x1F464;</span>`
            : `<img class="person-thumb" src="${thumbUrl}" alt="${esc(label)}"
                 onerror="this.style.display='none';this.nextElementSibling.style.display='flex'">
               <span class="person-thumb-placeholder" style="display:none">&#x1F464;</span>`;
        return `<button class="person-item${activeClass}${unknownClass}" onclick="faceSelectPerson(${onclickArg})"
            title="${esc(p.name || t('face.unassigned'))}">
            ${thumbHtml}
            <span class="person-label">${esc(label)}</span>
            <span class="person-count">${p.count}</span>
        </button>`;
    };

    // Empty state: show workflow guide
    if (all.length === 0) {
        return toolbar + progressHtml + `<div class="people-workflow-steps">
            <div class="people-wf-title">${esc(t('face.how-it-works'))}</div>
            <div class="people-wf-step"><span class="people-wf-n">1</span><span>${esc(t('face.step-detect'))}</span></div>
            <div class="people-wf-step"><span class="people-wf-n">2</span><span>${esc(t('face.step-group'))}</span></div>
            <div class="people-wf-step"><span class="people-wf-n">3</span><span>${esc(t('face.step-name'))}</span></div>
        </div>`;
    }

    // Unified view: named persons first, unknown clusters below a divider
    let html = toolbar + progressHtml;
    if (named.length > 0) {
        html += sorted(named).map(p => makeItem(p, false)).join('');
    }
    if (unknown.length > 0) {
        html += `<div class="people-unknown-divider">
            <span>${esc(t('face.unknown-section'))}</span>
            <span class="people-unknown-count">${unknown.length}</span>
        </div>`;
        html += sorted(unknown).map(p => makeItem(p, true)).join('');
    }
    return html;
}

/** Returns true when `name` is an auto-generated unknown cluster name or unassigned (empty). */
function _faceIsUnknown(name) {
    if (!name) return true;   // null, undefined, or empty string = unassigned
    const prefix = (state.faceConfig && state.faceConfig.tag_prefix) || 'person';
    return name.startsWith(prefix + '/unknown-');
}

/** No-op toggle kept for back-compat; unknown clusters are now shown inline. */
function faceToggleUnknown() {}

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

/** Detect faces in the current multi-file selection.
 *  Only image and raw files are submitted; others are silently skipped. */
async function faceDetectSelection() {
    if (state.faceProgressTimer) {
        showToast(t('face.batch-already-running'), 3000);
        return;
    }
    const base = state.currentBasePath;
    if (!base) {
        showToast(t('face.no-images-in-selection'), 3000);
        return;
    }
    const FACE_TYPES = new Set(['image', 'raw']);
    // Convert DB-relative paths to absolute filesystem paths for the backend.
    const absPaths = [...state.selectedPaths]
        .filter(p => {
            const name = p.split('/').pop() || p;
            return FACE_TYPES.has(fileType(name));
        })
        .map(p => base + '/' + p);
    if (absPaths.length === 0) {
        showToast(t('face.no-images-in-selection'), 3000);
        return;
    }
    try {
        await apiPost('/api/face/analyse-batch', {
            dir: currentAbsDir() || base,
            paths: absPaths,
        });
        _faceStartPolling();
    } catch (e) {
        showToast('Face detect: ' + e.message, 4000);
    }
}

/** Update the #bulk-status element (multi-select detail panel) with face batch progress.
 *  No-op when the element is absent (single-file or directory selected). */
function _faceUpdateBulkStatus() {
    const el = document.getElementById('bulk-status');
    if (!el) return;
    if (state.faceProgressTimer !== null) {
        el.innerHTML = _faceBatchProgressHtml();
    } else {
        el.innerHTML = '';
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
                // Reload detections for the currently displayed file so face
                // boxes appear immediately without needing to re-select it.
                if (state.faceDetectionsPath) {
                    await faceLoadDetections(state.faceDetectionsPath);
                    _faceRenderOverlays(state.faceDetectionsPath);
                }
                renderTags(); // refresh sidebar
                _faceUpdateBulkStatus();
            } else {
                renderTags(); // update progress bar in sidebar
                _faceUpdateBulkStatus();
            }
        } catch (_) {
            clearInterval(state.faceProgressTimer);
            state.faceProgressTimer = null;
            _faceUpdateBulkStatus();
        }
    }, 1000);
    // Show the initial progress bar immediately after the timer is armed.
    _faceUpdateBulkStatus();
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
    // 10-minute timeout: with OpenVINO, the first detect may need to compile
    // the model.  Pre-warming at startup normally prevents this, but as a
    // safety net we abort after 10 min and show a helpful message.
    const ctrl = new AbortController();
    const tid = setTimeout(() => ctrl.abort(), 600_000);
    try {
        const result = await apiPost('/api/face/analyse', { path, dir: currentAbsDir() }, { signal: ctrl.signal });
        state.faceDetections = result.detections || [];
        state.faceDetectionsPath = path;
        state.faceImageWidth  = result.image_width  || 0;
        state.faceImageHeight = result.image_height || 0;
        await loadPeople();
        renderTags(); // refresh sidebar
        if (state.faceDetections.length === 0) {
            showToast(t('face.no-faces-found'), 3000);
        }
    } catch (e) {
        const msg = ctrl.signal.aborted ? t('face.detect-timeout') : e.message;
        showToast(t('face.detect-error') + ': ' + msg, 6000);
        state.faceDetections = [];
    } finally {
        clearTimeout(tid);
        state.faceDetecting = false;
        _faceRefreshDetailControls(path);
        _faceRenderOverlays(path);
    }
}

// Generation counter: incremented whenever a new load supersedes previous ones.
// Each faceLoadDetections call captures the counter at start; if it has changed
// by the time the fetch resolves, the result is discarded (stale).
let _faceLoadSeq = 0;

/** Load existing detections for a file (no re-analysis). */
async function faceLoadDetections(path) {
    if (state.faceDetectionsPath === path) return; // already loaded
    const seq = ++_faceLoadSeq;
    try {
        const result = await api('/api/face/detections?' + new URLSearchParams({ path }) + dirParam('&'));
        if (seq !== _faceLoadSeq) return; // superseded by a newer load
        state.faceDetections = result.detections || [];
        state.faceDetectionsPath = path;
        state.faceImageWidth  = result.image_width  || 0;
        state.faceImageHeight = result.image_height || 0;
    } catch (_) {
        if (seq !== _faceLoadSeq) return;
        state.faceDetections = [];
        state.faceDetectionsPath = path;
        state.faceImageWidth  = 0;
        state.faceImageHeight = 0;
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

    // If layout hasn't happened yet (offsetWidth = 0), retry on the next frame.
    if (!img.offsetWidth || !img.offsetHeight) {
        requestAnimationFrame(() => _faceRenderOverlays(path));
        return;
    }

    // Use original image dimensions from the API for scaling; fall back to
    // img.naturalWidth/Height (correct for regular images, wrong for zip-thumb previews).
    const origW = state.faceImageWidth  || img.naturalWidth;
    const origH = state.faceImageHeight || img.naturalHeight;

    // Update the wrapper's aspect-ratio with the authoritative server dimensions
    // (may differ from naturalWidth/Height for zip-thumb previews).
    if (origW && origH) wrap.style.aspectRatio = origW + ' / ' + origH;

    for (const det of state.faceDetections) {
        // Skip detections whose box falls entirely outside the image area —
        // these are artefacts from a previous (incorrect) analysis run.
        if (det.x + det.w <= 0 || det.y + det.h <= 0 ||
            det.x >= origW || det.y >= origH) continue;

        const bx = Math.max(0, det.x);
        const by = Math.max(0, det.y);
        const bw = Math.min(det.x + det.w, origW) - bx;
        const bh = Math.min(det.y + det.h, origH) - by;
        if (bw <= 0 || bh <= 0) continue;

        const box = document.createElement('div');
        box.className = 'face-box' + (det.subject_name ? ' assigned' : '');
        // Percentage positions relative to the wrapper, which now shrinks to
        // the rendered image size (face.css). These scale automatically with
        // the image when the panel is resized — no JS re-calculation needed.
        box.style.left   = (bx / origW * 100) + '%';
        box.style.top    = (by / origH * 100) + '%';
        box.style.width  = (bw / origW * 100) + '%';
        box.style.height = (bh / origH * 100) + '%';
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

/** Wrap the preview img inside a .face-preview-wrap div for face-box overlays.
 * Only the <img> is wrapped — .preview-zoomable keeps its own box as the
 * position:relative anchor for the hover-zone overlay. */
function _faceWrapPreviewImg() {
    // Check if already wrapped
    if (document.querySelector('.face-preview-wrap')) return;
    const anchor = document.querySelector('#detail .preview-zoomable');
    if (!anchor) return;
    const img = anchor.querySelector('img');
    if (!img) return;

    const wrap = document.createElement('div');
    wrap.className = 'face-preview-wrap';
    img.parentNode.insertBefore(wrap, img);
    wrap.appendChild(img);

    // Set aspect-ratio immediately from the image's natural dimensions so the
    // wrapper already has the correct size before detections finish loading.
    // _faceRenderOverlays will update this with the server-provided dimensions.
    if (img.naturalWidth && img.naturalHeight) {
        wrap.style.aspectRatio = img.naturalWidth + ' / ' + img.naturalHeight;
    } else {
        img.addEventListener('load', () => {
            if (img.naturalWidth && img.naturalHeight && !wrap.style.aspectRatio)
                wrap.style.aspectRatio = img.naturalWidth + ' / ' + img.naturalHeight;
        }, { once: true });
    }
}

/** Refresh only the face toolbar div without rebuilding the full detail panel. */
function _faceRefreshDetailControls(path) {
    const el = document.getElementById('face-toolbar-row');
    if (!el) return;
    el.innerHTML = faceDetailToolbar(path);
}

// ---------------------------------------------------------------------------
// Re-render face boxes when the viewport size changes (e.g. window resize
// changes vh, or the user resizes the browser). Debounced to 150 ms.
// ---------------------------------------------------------------------------

/** Public: no-op — face boxes use percentage positions and scale automatically
 * because .face-preview-wrap shrinks to the image size (see face.css).
 * Called by main.js after separator drag; kept for API compatibility. */
function faceRerenderPreviewBoxes() {}


// ---------------------------------------------------------------------------
// Assign dialog
// ---------------------------------------------------------------------------

let _faceAssignDialog = null;

function _faceShowAssignDialog(det, boxEl) {
    _faceCloseAssignDialog();

    const prefix = (state.faceConfig && state.faceConfig.tag_prefix) ? state.faceConfig.tag_prefix : 'person';
    const existing = det.subject_name
        ? (det.subject_name.startsWith(prefix + '/') ? det.subject_name.slice(prefix.length + 1) : det.subject_name)
        : '';

    // Build contextual info line
    const isUnknown = det.subject_name && _faceIsUnknown(det.subject_name);
    const isNamed   = det.subject_name && !isUnknown;
    const clusterEntry = det.subject_name ? (state.people || []).find(p => p.name === det.subject_name) : null;
    let contextLine;
    if (isUnknown && clusterEntry) {
        contextLine = t('face.assign-cluster-info', { n: clusterEntry.count });
    } else if (isNamed) {
        contextLine = t('face.assign-named', { name: existing });
    } else {
        contextLine = t('face.assign-no-subject');
    }

    const thumbUrl = '/api/face/thumbnail?' + new URLSearchParams({ id: det.id }) + dirParam('&');

    const dialog = document.createElement('div');
    dialog.className = 'face-assign-dialog';
    dialog.innerHTML = `
        <div class="face-assign-header">
            <img class="face-assign-thumb-img" src="${thumbUrl}" alt=""
                 onerror="this.style.display='none'">
            <div class="face-assign-info">
                <div class="face-assign-current-name">${esc(existing || t('face.unknown'))}</div>
                <div class="face-assign-cluster-info">${esc(contextLine)}</div>
            </div>
        </div>
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
    dialog.style.left = Math.min(rect.left, window.innerWidth - 260) + 'px';
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
    // Always re-render overlays: the detail DOM was just rebuilt so existing
    // boxes are gone even when detections were already cached.
    _faceRenderOverlays(path);
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
            const filePath = _cv.mode === 'dir'
                ? _cv.filePaths[_cv.current]
                : (_cv.path ? _cv.path + '::' + _cv.pages[_cv.current] : null);
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
    // Invalidate any in-flight faceLoadDetections calls from the viewer so they
    // cannot overwrite state after the detail panel has taken over.
    ++_faceLoadSeq;
    state.faceDetections = [];
    state.faceDetectionsPath = null;
    const btn = document.getElementById('cv-face-btn');
    if (btn) btn.classList.remove('active');
    // Force the detail panel to reload the correct detections for the selected file.
    if (state.selectedFile) {
        faceOnDetailRendered(state.selectedFile.path, state.selectedFile.type);
    }
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
        // Use original image dimensions from the API for scaling.
        const origW = state.faceImageWidth  || img.naturalWidth;
        const origH = state.faceImageHeight || img.naturalHeight;
        const scaleX = img.offsetWidth  / origW;
        const scaleY = img.offsetHeight / origH;

        for (const det of state.faceDetections) {
            // Skip detections whose box falls entirely outside the image area.
            if (det.x + det.w <= 0 || det.y + det.h <= 0 ||
                det.x >= origW || det.y >= origH) continue;

            const bx = Math.max(0, det.x);
            const by = Math.max(0, det.y);
            const bw = Math.min(det.x + det.w, origW) - bx;
            const bh = Math.min(det.y + det.h, origH) - by;
            if (bw <= 0 || bh <= 0) continue;

            const box = document.createElement('div');
            box.className = 'face-box' + (det.subject_name ? ' assigned' : '');
            box.style.left   = (bx * scaleX) + 'px';
            box.style.top    = (by * scaleY) + 'px';
            box.style.width  = (bw * scaleX) + 'px';
            box.style.height = (bh * scaleY) + 'px';
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
