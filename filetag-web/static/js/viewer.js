// ---------------------------------------------------------------------------
// Media viewer
// ---------------------------------------------------------------------------

const _cv = {
    path: null,
    pages: [],
    current: 0,
    spread: false,   // two-page spread mode
    thumbs: false,   // thumbnail strip visible
    rtl: false,      // right-to-left reading (manga)
    scroll: false,   // continuous scroll mode
    scrollDir: 'v',  // 'v' vertical | 'h' horizontal
    scrollWidth: 100, // image width % in vertical scroll mode
    scrollHeight: 100, // image height % of stage in horizontal scroll mode (100 = fill)
    zoom: 1,
    panX: 0,
    panY: 0,
    mode: 'zip',     // 'zip' | 'dir'
    filePaths: [],   // used in 'dir' mode: absolute relative paths per page
    _prefetchCache: new Map(), // url → HTMLImageElement, keeps references alive
};

// Prefetch pages adjacent to the current index so navigation is instant.
// Caches Image objects keyed by URL to keep decoded bitmaps in browser memory.
function _cvPrefetch(idx) {
    const step = _cv.spread ? 2 : 1;
    const total = _cv.pages.length;
    // Indices to preload: 1 and 2 steps in each direction
    const candidates = [
        idx + step, idx + step * 2,
        idx - step, idx - step * 2,
    ].filter(i => i >= 0 && i < total);
    // In spread mode also load the partner page of each candidate
    if (_cv.spread) {
        for (const i of [...candidates]) {
            if (i + 1 < total) candidates.push(i + 1);
        }
    }
    const wanted = new Set(candidates.map(i => cvPageUrl(i)));
    // Evict entries that are no longer neighbours
    for (const url of _cv._prefetchCache.keys()) {
        if (!wanted.has(url)) _cv._prefetchCache.delete(url);
    }
    // Start loading new entries
    for (const url of wanted) {
        if (!_cv._prefetchCache.has(url)) {
            const img = new Image();
            img.src = url;
            _cv._prefetchCache.set(url, img);
        }
    }
}

// Return the URL for a single page image.
function cvPageUrl(i) {
    if (_cv.mode === 'dir') return '/preview/' + encodePath(_cv.filePaths[i]) + dirParam('?');
    return `/api/zip/page?${new URLSearchParams({ path: _cv.path, page: i })}` + dirParam('&');
}

// Return the URL for a thumbnail of a single page.
function cvThumbUrl(i) {
    if (_cv.mode === 'dir') return '/thumb/' + encodePath(_cv.filePaths[i]) + dirParam('?');
    return `/api/zip/thumb?${new URLSearchParams({ path: _cv.path, page: i })}` + dirParam('&');
}

async function openMediaViewer(path, startPage = 0) {
    const overlay = document.getElementById('media-viewer');
    overlay.hidden = false;

    _cv.path = path;
    _cv.current = startPage;
    _cv.pages = [];

    document.getElementById('cv-status').textContent = 'Loading…';
    document.getElementById('cv-pages').innerHTML = '';

    const res = await fetch('/api/zip/pages?' + new URLSearchParams({ path }) + dirParam('&'));
    if (!res.ok) {
        document.getElementById('cv-status').textContent = 'Cannot read ZIP';
        return;
    }
    const data = await res.json();
    _cv.pages = data.pages || [];
    if (_cv.pages.length === 0) {
        document.getElementById('cv-status').textContent = 'No images in ZIP';
        return;
    }
    cvBuildThumbs();
    if (_cv.scroll) {
        cvBuildScrollView();
    } else {
        cvShowPage(startPage);
    }
    document.addEventListener('keydown', _cvKeyHandler);
}

// Open the viewer for a list of plain image files from a directory.
async function openDirViewer(filePaths, startIdx = 0) {
    const overlay = document.getElementById('media-viewer');
    overlay.hidden = false;

    _cv.mode = 'dir';
    _cv.path = null;
    _cv.filePaths = filePaths;
    _cv.pages = filePaths.map(p => p.split('/').pop()); // names for display/count
    _cv.current = Math.max(0, Math.min(startIdx, filePaths.length - 1));

    document.getElementById('cv-status').textContent = 'Loading…';
    document.getElementById('cv-pages').innerHTML = '';

    if (_cv.pages.length === 0) {
        document.getElementById('cv-status').textContent = 'No images found';
        return;
    }
    cvBuildThumbs();
    if (_cv.scroll) {
        cvBuildScrollView();
    } else {
        cvShowPage(_cv.current);
    }
    document.addEventListener('keydown', _cvKeyHandler);
}

// Open the directory viewer starting at the given file, loading sibling images
// from the same directory.
async function openFileInDirViewer(filePath) {
    const lastSlash = filePath.lastIndexOf('/');
    const dirPath = lastSlash > 0 ? filePath.substring(0, lastSlash) : '';
    let images = [filePath];
    let startIdx = 0;
    try {
        const res = await fetch('/api/dir/images?' + new URLSearchParams({ path: dirPath || '.' }) + dirParam('&'));
        if (res.ok) {
            const data = await res.json();
            if (data.images && data.images.length > 0) {
                images = data.images;
                const idx = images.indexOf(filePath);
                startIdx = idx >= 0 ? idx : 0;
            }
        }
    } catch (_) { /* fall through with single file */ }
    openDirViewer(images, startIdx);
}

function closeMediaViewer() {
    if (document.fullscreenElement) document.exitFullscreen();
    if (_cv.scroll) cvExitScrollView();
    document.getElementById('media-viewer').hidden = true;
    document.removeEventListener('keydown', _cvKeyHandler);
    _cv.mode = 'zip'; _cv.path = null; _cv.pages = []; _cv.filePaths = []; _cv.current = 0;
    _cv._prefetchCache.clear();
    document.getElementById('cv-pages').innerHTML = '';
    document.getElementById('cv-thumbs').innerHTML = '';
}

function cvToggleRtl() {
    _cv.rtl = !_cv.rtl;
    document.getElementById('cv-rtl-btn').classList.toggle('active', _cv.rtl);
    // Mirror the thumbnail strip: RTL puts it on the right
    const thumbs = document.getElementById('cv-thumbs');
    const body   = document.getElementById('cv-body');
    if (_cv.rtl) {
        body.style.flexDirection = 'row-reverse';
        thumbs.style.borderRight = '';
        thumbs.style.borderLeft  = '1px solid rgba(255,255,255,0.08)';
    } else {
        body.style.flexDirection = '';
        thumbs.style.borderRight = '1px solid rgba(255,255,255,0.08)';
        thumbs.style.borderLeft  = '';
    }
    // In horizontal scroll mode, also flip the pages row without rebuilding
    if (_cv.scroll && _cv.scrollDir === 'h') {
        const container = document.getElementById('cv-pages');
        container.style.flexDirection = _cv.rtl ? 'row-reverse' : '';
    }
    cvShowPage(_cv.current);
}

function cvToggleThumbs() {
    _cv.thumbs = !_cv.thumbs;
    const panel = document.getElementById('cv-thumbs');
    panel.hidden = !_cv.thumbs;
    document.getElementById('cv-thumbs-btn').classList.toggle('active', _cv.thumbs);
    if (_cv.thumbs) cvScrollThumbIntoView(_cv.current);
}

function cvBuildThumbs() {
    const panel = document.getElementById('cv-thumbs');
    panel.innerHTML = '';
    _cv.pages.forEach((_name, i) => {
        const cell = document.createElement('div');
        cell.className = 'cv-thumb' + (i === 0 ? ' active' : '');
        cell.dataset.page = i;
        cell.onclick = () => cvShowPage(i);
        const url = cvThumbUrl(i);
        cell.innerHTML = `<img src="${url}" loading="lazy" alt="page ${i + 1}" onerror="this.style.visibility='hidden'">` +
            `<div class="cv-thumb-num">${i + 1}</div>`;
        panel.appendChild(cell);
    });
}

function cvUpdateThumbActive(idx) {
    const panel = document.getElementById('cv-thumbs');
    panel.querySelectorAll('.cv-thumb').forEach(el => {
        el.classList.toggle('active', Number(el.dataset.page) === idx);
    });
    cvScrollThumbIntoView(idx);
}

function cvScrollThumbIntoView(idx) {
    if (!_cv.thumbs) return;
    const panel = document.getElementById('cv-thumbs');
    const cell = panel.querySelector(`.cv-thumb[data-page="${idx}"]`);
    if (cell) cell.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
}

// ---------------------------------------------------------------------------
// Media viewer – vertical + horizontal scroll mode
// ---------------------------------------------------------------------------

let _cvScrollObserver = null;

function _cvSetScrollButtons() {
    document.getElementById('cv-scroll-btn').classList.toggle('active', _cv.scroll && _cv.scrollDir === 'v');
    document.getElementById('cv-hscroll-btn').classList.toggle('active', _cv.scroll && _cv.scrollDir === 'h');
    document.getElementById('cv-spread-btn').disabled = !!_cv.scroll;
}

function cvToggleScroll() {
    if (_cv.scroll && _cv.scrollDir === 'v') {
        _cv.scroll = false; cvExitScrollView();
    } else {
        if (_cv.scroll) cvExitScrollView();
        _cv.scrollDir = 'v'; _cv.scroll = true; cvBuildScrollView();
    }
    _cvSetScrollButtons();
}

function cvToggleHScroll() {
    if (_cv.scroll && _cv.scrollDir === 'h') {
        _cv.scroll = false; cvExitScrollView();
    } else {
        if (_cv.scroll) cvExitScrollView();
        _cv.scrollDir = 'h'; _cv.scroll = true; cvBuildScrollView();
    }
    _cvSetScrollButtons();
}

function cvApplyScrollZoom(newSize, event) {
    const stage = document.getElementById('cv-stage');
    const btn   = document.getElementById('cv-zoom-reset-btn');
    if (_cv.scrollDir === 'h') {
        if (newSize !== undefined) {
            _cv.scrollHeight = Math.max(20, Math.min(300, newSize));
        }
        if (stage) {
            // Always anchor: use cursor position when available, stage centre otherwise.
            const rect = stage.getBoundingClientRect();
            const cx = event ? (event.clientX - rect.left) : rect.width / 2;
            const anchor = stage.scrollWidth > 0
                ? { ratio: (stage.scrollLeft + cx) / stage.scrollWidth, cx }
                : null;
            if (_cv.scrollHeight >= 100) {
                stage.style.removeProperty('--cv-scroll-height'); // let CSS default (100%) fill the stage
            } else {
                stage.style.setProperty('--cv-scroll-height', `${_cv.scrollHeight}vh`);
            }
            if (anchor) requestAnimationFrame(() => {
                stage.scrollTo({ left: anchor.ratio * stage.scrollWidth - anchor.cx, behavior: 'instant' });
            });
        }
        if (btn) { btn.textContent = Math.round(_cv.scrollHeight) + '%'; btn.style.visibility = _cv.scrollHeight >= 100 ? 'hidden' : ''; }
    } else {
        if (newSize !== undefined) _cv.scrollWidth = Math.max(20, Math.min(300, newSize));
        if (stage) {
            // Always anchor: use cursor position when available, stage centre otherwise.
            const rect = stage.getBoundingClientRect();
            const cy = event ? (event.clientY - rect.top) : rect.height / 2;
            const anchor = stage.scrollHeight > 0
                ? { ratio: (stage.scrollTop + cy) / stage.scrollHeight, cy }
                : null;
            stage.style.setProperty('--cv-scroll-width', `${_cv.scrollWidth}%`);
            if (anchor) requestAnimationFrame(() => {
                stage.scrollTo({ top: anchor.ratio * stage.scrollHeight - anchor.cy, behavior: 'instant' });
            });
        }
        if (btn) { btn.textContent = Math.round(_cv.scrollWidth) + '%'; btn.style.visibility = _cv.scrollWidth === 100 ? 'hidden' : ''; }
    }
}

function cvBuildScrollView() {
    const stage     = document.getElementById('cv-stage');
    const container = document.getElementById('cv-pages');

    stage.classList.add(_cv.scrollDir === 'h' ? 'cv-hscroll-mode' : 'cv-scroll-mode');
    container.style.transform = 'none';
    // RTL in horizontal mode: reverse the row so first page is on the right
    if (_cv.scrollDir === 'h' && _cv.rtl) container.style.flexDirection = 'row-reverse';
    container.innerHTML = '';

    _cv.pages.forEach((_name, i) => {
        const url = cvPageUrl(i);
        const img = document.createElement('img');
        img.className = 'cv-page';
        img.dataset.page = i;
        img.src = url;
        img.alt = `page ${i + 1}`;
        img.loading = 'lazy';
        container.appendChild(img);
    });

    // Track which page is most visible and update status + thumbnail strip
    _cvScrollObserver = new IntersectionObserver(entries => {
        let best = -1, bestRatio = 0;
        entries.forEach(entry => {
            if (entry.intersectionRatio > bestRatio) {
                bestRatio = entry.intersectionRatio;
                best = Number(entry.target.dataset.page);
            }
        });
        if (best >= 0 && best !== _cv.current) {
            _cv.current = best;
            cvUpdateThumbActive(best);
            document.getElementById('cv-status').textContent = `${best + 1} / ${_cv.pages.length}`;
        }
    }, { root: stage, threshold: [0, 0.25, 0.5, 0.75, 1.0] });

    container.querySelectorAll('img.cv-page[data-page]').forEach(img => {
        _cvScrollObserver.observe(img);
    });

    cvApplyScrollZoom();

    // Scroll to the page that was open before entering scroll mode
    const target = container.querySelector(`img.cv-page[data-page="${_cv.current}"]`);
    if (target) {
        requestAnimationFrame(() => {
            if (_cv.scrollDir === 'h') {
                target.scrollIntoView({ inline: 'start', block: 'nearest' });
            } else {
                target.scrollIntoView({ block: 'start' });
            }
        });
    }
    document.getElementById('cv-status').textContent =
        `${_cv.current + 1} / ${_cv.pages.length}`;
}

function cvExitScrollView() {
    if (_cvScrollObserver) { _cvScrollObserver.disconnect(); _cvScrollObserver = null; }
    const stage = document.getElementById('cv-stage');
    stage.classList.remove('cv-scroll-mode');
    stage.classList.remove('cv-hscroll-mode');
    stage.style.removeProperty('--cv-scroll-width');
    stage.style.removeProperty('--cv-scroll-height');
    const container = document.getElementById('cv-pages');
    container.style.transform = '';
    container.style.flexDirection = '';
    container.innerHTML = '';
    document.getElementById('cv-spread-btn').disabled = false;
    _cv.scrollWidth = 100; _cv.scrollHeight = 100;
    cvShowPage(_cv.current);
}

// ---------------------------------------------------------------------------
// Media viewer – zoom / pan
// ---------------------------------------------------------------------------

const _cvDrag = { active: false, moved: false, startX: 0, startY: 0, startPanX: 0, startPanY: 0 };
let _cvPinchStart = null;  // { dist, zoom, midX, midY }

function cvApplyTransform() {
    if (_cv.scroll) return;  // scroll mode uses width-based zoom, not CSS transform
    const container = document.getElementById('cv-pages');
    if (container) {
        container.style.transform =
            `translate(${_cv.panX}px, ${_cv.panY}px) scale(${_cv.zoom})`;
    }
    const stage = document.getElementById('cv-stage');
    if (stage) stage.style.cursor = _cv.zoom > 1 ? 'grab' : '';
    const btn = document.getElementById('cv-zoom-reset-btn');
    if (btn) {
        const pct = Math.round(_cv.zoom * 100);
        btn.textContent = pct + '%';
        btn.style.visibility = _cv.zoom === 1 ? 'hidden' : '';
    }
}

function cvResetZoom() {
    if (_cv.scroll) { cvApplyScrollZoom(_cv.scrollDir === 'h' ? 90 : 100); return; }
    _cv.zoom = 1; _cv.panX = 0; _cv.panY = 0;
    cvApplyTransform();
}

function cvClampPan() {
    const stage = document.getElementById('cv-stage');
    if (!stage) return;
    // Allow panning up to ~80% of the scaled content half-size
    const maxX = stage.clientWidth  * _cv.zoom * 0.6;
    const maxY = stage.clientHeight * _cv.zoom * 0.6;
    _cv.panX = Math.max(-maxX, Math.min(maxX, _cv.panX));
    _cv.panY = Math.max(-maxY, Math.min(maxY, _cv.panY));
}

function cvZoomTo(newZoom, originX, originY) {
    const clamped = Math.max(0.5, Math.min(10, newZoom));
    const dz = clamped / _cv.zoom;
    // zoom toward (originX, originY) relative to stage centre
    _cv.panX = originX * (1 - dz) + _cv.panX * dz;
    _cv.panY = originY * (1 - dz) + _cv.panY * dz;
    _cv.zoom = clamped;
    cvClampPan();
    cvApplyTransform();
}

function cvZoomIn()  {
    if (_cv.scroll) { cvApplyScrollZoom((_cv.scrollDir === 'h' ? _cv.scrollHeight : _cv.scrollWidth) * 1.25); return; }
    cvZoomTo(_cv.zoom * 1.25, 0, 0);
}
function cvZoomOut() {
    if (_cv.scroll) { cvApplyScrollZoom((_cv.scrollDir === 'h' ? _cv.scrollHeight : _cv.scrollWidth) / 1.25); return; }
    cvZoomTo(_cv.zoom / 1.25, 0, 0);
}

function _cvInitStageEvents() {
    const stage = document.getElementById('cv-stage');

    // Wheel / trackpad in normal (non-scroll) mode:
    // - Pinch-to-zoom (reported as Ctrl+wheel by browsers) → zoom
    // - Two-finger pan (deltaX / deltaY without Ctrl) → pan when zoomed, or ignore
    stage.addEventListener('wheel', e => {
        if (document.getElementById('media-viewer').hidden) return;
        if (_cv.scroll) {
            if (e.ctrlKey || e.metaKey) {
                e.preventDefault();
                const factor = Math.exp(-e.deltaY / 300);
                const cur = _cv.scrollDir === 'h' ? _cv.scrollHeight : _cv.scrollWidth;
                cvApplyScrollZoom(cur * factor, e);
            }
            // Otherwise: let the browser scroll natively
            return;
        }
        e.preventDefault();
        if (e.ctrlKey || e.metaKey) {
            // Pinch-to-zoom or Ctrl+scroll → zoom towards cursor
            const rect  = stage.getBoundingClientRect();
            const ox = e.clientX - rect.left  - rect.width  / 2;
            const oy = e.clientY - rect.top   - rect.height / 2;
            const factor = Math.exp(-e.deltaY / 300);
            cvZoomTo(_cv.zoom * factor, ox, oy);
        } else {
            // Two-finger pan → pan (only meaningful when zoomed)
            if (_cv.zoom > 1) {
                _cv.panX -= e.deltaX;
                _cv.panY -= e.deltaY;
                cvClampPan();
                cvApplyTransform();
            }
        }
    }, { passive: false });

    // Mousedown: start drag when zoomed (not in scroll mode)
    stage.addEventListener('mousedown', e => {
        if (document.getElementById('media-viewer').hidden) return;
        if (_cv.scroll) return;
        _cvDrag.moved   = false;
        _cvDrag.startX  = e.clientX;  _cvDrag.startY  = e.clientY;
        _cvDrag.startPanX = _cv.panX; _cvDrag.startPanY = _cv.panY;
        if (_cv.zoom > 1) {
            _cvDrag.active = true;
            stage.style.cursor = 'grabbing';
            e.preventDefault();
        }
    });

    window.addEventListener('mousemove', e => {
        if (!_cvDrag.active) return;
        const dx = e.clientX - _cvDrag.startX;
        const dy = e.clientY - _cvDrag.startY;
        if (Math.abs(dx) > 3 || Math.abs(dy) > 3) _cvDrag.moved = true;
        _cv.panX = _cvDrag.startPanX + dx;
        _cv.panY = _cvDrag.startPanY + dy;
        cvClampPan();
        cvApplyTransform();
    });

    window.addEventListener('mouseup', () => {
        if (_cvDrag.active) {
            _cvDrag.active = false;
            const stage2 = document.getElementById('cv-stage');
            if (stage2) stage2.style.cursor = _cv.zoom > 1 ? 'grab' : '';
        }
    });

    // Double-click in the middle zone (30%–70%): zoom to 2× at cursor, or reset if already zoomed.
    // Double-click in the nav zones (left <30%, right >70%) is ignored so rapid page-turning works.
    stage.addEventListener('dblclick', e => {
        if (document.getElementById('media-viewer').hidden) return;
        const x = e.clientX / window.innerWidth;
        if (x < 0.3 || x > 0.7) return;  // nav zone — ignore
        e.preventDefault();
        if (_cv.zoom > 1) {
            cvResetZoom();
        } else {
            const rect = stage.getBoundingClientRect();
            const ox = e.clientX - rect.left  - rect.width  / 2;
            const oy = e.clientY - rect.top   - rect.height / 2;
            cvZoomTo(2, ox, oy);
        }
    });

    // Touch: pinch-to-zoom
    stage.addEventListener('touchstart', e => {
        if (e.touches.length === 2) {
            const dx = e.touches[1].clientX - e.touches[0].clientX;
            const dy = e.touches[1].clientY - e.touches[0].clientY;
            const rect = stage.getBoundingClientRect();
            _cvPinchStart = {
                dist:  Math.hypot(dx, dy),
                zoom:  _cv.zoom,
                midX: (e.touches[0].clientX + e.touches[1].clientX) / 2 - rect.left  - rect.width  / 2,
                midY: (e.touches[0].clientY + e.touches[1].clientY) / 2 - rect.top   - rect.height / 2,
            };
            e.preventDefault();
        }
    }, { passive: false });

    stage.addEventListener('touchmove', e => {
        if (_cvPinchStart && e.touches.length === 2) {
            const dx   = e.touches[1].clientX - e.touches[0].clientX;
            const dy   = e.touches[1].clientY - e.touches[0].clientY;
            const dist = Math.hypot(dx, dy);
            cvZoomTo(_cvPinchStart.zoom * (dist / _cvPinchStart.dist),
                     _cvPinchStart.midX, _cvPinchStart.midY);
            e.preventDefault();
        }
    }, { passive: false });

    stage.addEventListener('touchend', e => {
        if (e.touches.length < 2) _cvPinchStart = null;
    });
}

const _cvExpandIcon = '<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="15 3 21 3 21 9"/><polyline points="9 21 3 21 3 15"/><line x1="21" y1="3" x2="14" y2="10"/><line x1="3" y1="21" x2="10" y2="14"/></svg>';
const _cvCompressIcon = '<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="4 14 10 14 10 20"/><polyline points="20 10 14 10 14 4"/><line x1="10" y1="14" x2="3" y2="21"/><line x1="21" y1="3" x2="14" y2="10"/></svg>';

function cvToggleFullscreen() {
    const overlay = document.getElementById('media-viewer');
    if (!document.fullscreenElement) {
        overlay.requestFullscreen().catch(() => {});
    } else {
        document.exitFullscreen();
    }
}

document.addEventListener('fullscreenchange', () => {
    const btn = document.getElementById('cv-fs-btn');
    if (!btn) return;
    const inFS = !!document.fullscreenElement;
    btn.innerHTML = inFS ? _cvCompressIcon : _cvExpandIcon;
    btn.title = inFS ? 'Exit full screen (F)' : 'Full screen (F)';
    const overlay = document.getElementById('media-viewer');
    overlay.classList.toggle('cv-fs', inFS);
    if (!inFS) {
        const toolbar = overlay.querySelector('.cv-toolbar');
        if (toolbar) toolbar.classList.remove('cv-toolbar-peek');
        if (_cvFsHideTimer) { clearTimeout(_cvFsHideTimer); _cvFsHideTimer = null; }
    }
});

// ---------------------------------------------------------------------------
// Fullscreen toolbar overlay (hide until mouse nears the top)
// ---------------------------------------------------------------------------

let _cvFsHideTimer = null;

function _cvShowFsToolbar() {
    const toolbar = document.querySelector('#media-viewer .cv-toolbar');
    if (!toolbar) return;
    toolbar.classList.add('cv-toolbar-peek');
    if (_cvFsHideTimer) { clearTimeout(_cvFsHideTimer); _cvFsHideTimer = null; }
    _cvFsHideTimer = setTimeout(() => {
        _cvFsHideTimer = null;
        if (!toolbar.matches(':hover')) toolbar.classList.remove('cv-toolbar-peek');
    }, 3000);
}

function _cvScheduleHideFsToolbar() {
    if (_cvFsHideTimer) { clearTimeout(_cvFsHideTimer); _cvFsHideTimer = null; }
    _cvFsHideTimer = setTimeout(() => {
        _cvFsHideTimer = null;
        const toolbar = document.querySelector('#media-viewer .cv-toolbar');
        if (toolbar) toolbar.classList.remove('cv-toolbar-peek');
    }, 600);
}

function _cvInitFsToolbar() {
    const overlay = document.getElementById('media-viewer');
    const toolbar = overlay.querySelector('.cv-toolbar');

    overlay.addEventListener('mousemove', e => {
        if (!overlay.classList.contains('cv-fs')) return;
        if (e.clientY < 80) _cvShowFsToolbar();
    });

    toolbar.addEventListener('mouseenter', () => {
        if (!overlay.classList.contains('cv-fs')) return;
        if (_cvFsHideTimer) { clearTimeout(_cvFsHideTimer); _cvFsHideTimer = null; }
    });

    toolbar.addEventListener('mouseleave', () => {
        if (!overlay.classList.contains('cv-fs')) return;
        _cvScheduleHideFsToolbar();
    });
}

function cvShowPage(idx) {
    if (!_cv.pages.length) return;
    idx = Math.max(0, Math.min(idx, _cv.pages.length - 1));
    _cv.current = idx;

    // In scroll mode: scroll to the page instead of replacing content
    if (_cv.scroll) {
        const container = document.getElementById('cv-pages');
        const target = container.querySelector(`img.cv-page[data-page="${idx}"]`);
        if (target) {
            if (_cv.scrollDir === 'h') {
                target.scrollIntoView({ behavior: 'smooth', inline: 'start', block: 'nearest' });
            } else {
                target.scrollIntoView({ behavior: 'smooth', block: 'start' });
            }
        }
        document.getElementById('cv-status').textContent = `${idx + 1} / ${_cv.pages.length}`;
        cvUpdateThumbActive(idx);
        return;
    }

    const container = document.getElementById('cv-pages');
    const url1 = cvPageUrl(idx);
    // In spread mode, RTL shows the next page to the LEFT of the current one
    const url2 = _cv.spread && idx + 1 < _cv.pages.length
        ? cvPageUrl(idx + 1)
        : null;

    cvResetZoom();

    // Build Image elements and wait for decode before swapping into the DOM,
    // so the browser has the bitmap ready and the switch is instantaneous.
    const img1 = new Image();
    img1.className = 'cv-page';
    img1.alt = `page ${idx + 1}`;
    img1.src = url1;

    const img2 = url2 ? new Image() : null;
    if (img2) {
        img2.className = 'cv-page';
        img2.alt = `page ${idx + 2}`;
        img2.src = url2;
    }

    // decode() resolves when the image is fully decoded (or immediately if
    // already cached). Fall back silently if the API isn't available.
    const decodes = [img1.decode ? img1.decode().catch(() => {}) : Promise.resolve()];
    if (img2) decodes.push(img2.decode ? img2.decode().catch(() => {}) : Promise.resolve());

    Promise.all(decodes).then(() => {
        // Only update the DOM if the viewer hasn't moved on to another page
        // while we were decoding (rapid navigation).
        if (_cv.current !== idx) return;
        container.innerHTML = '';
        if (_cv.rtl && img2) {
            container.appendChild(img2);
            container.appendChild(img1);
        } else {
            container.appendChild(img1);
            if (img2) container.appendChild(img2);
        }
    });

    const total = _cv.spread
        ? `${idx + 1}${url2 ? '–' + (idx + 2) : ''} / ${_cv.pages.length}`
        : `${idx + 1} / ${_cv.pages.length}`;
    document.getElementById('cv-status').textContent = total;
    cvUpdateThumbActive(idx);
    _cvPrefetch(idx);
}

function cvNext() {
    // In RTL, the visual "next" page (reading direction forward) is the previous index
    const step = _cv.spread ? 2 : 1;
    if (_cv.rtl) {
        cvShowPage(_cv.current - step);
    } else {
        if (_cv.current + step <= _cv.pages.length - 1) cvShowPage(_cv.current + step);
    }
}
function cvPrev() {
    const step = _cv.spread ? 2 : 1;
    if (_cv.rtl) {
        if (_cv.current + step <= _cv.pages.length - 1) cvShowPage(_cv.current + step);
    } else {
        cvShowPage(_cv.current - step);
    }
}

function cvToggleSpread() {
    _cv.spread = !_cv.spread;
    document.getElementById('cv-spread-btn').classList.toggle('active', _cv.spread);
    cvShowPage(_cv.current);
}

function _cvKeyHandler(e) {
    if (document.getElementById('media-viewer').hidden) return;
    // ArrowRight = forward in reading direction (RTL: lower index; LTR: higher index)
    if (e.key === 'ArrowRight' || e.key === 'ArrowDown' || e.key === ' ') {
        e.preventDefault();
        if (_cv.scroll) _cvScrollNav(_cv.rtl ? -1 : 1);
        else { _cv.rtl ? cvPrev() : cvNext(); }
    } else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') {
        e.preventDefault();
        if (_cv.scroll) _cvScrollNav(_cv.rtl ? 1 : -1);
        else { _cv.rtl ? cvNext() : cvPrev(); }
    } else if (e.key === 'f' || e.key === 'F') cvToggleFullscreen();
    else if (e.key === 't' || e.key === 'T') cvToggleThumbs();
    else if (e.key === 'r' || e.key === 'R') cvToggleRtl();
    else if (e.key === 'v' || e.key === 'V') cvToggleScroll();
    else if (e.key === 'h' || e.key === 'H') cvToggleHScroll();
    else if (e.key === '+' || e.key === '=') cvZoomIn();
    else if (e.key === '-') cvZoomOut();
    else if (e.key === '0') cvResetZoom();
    else if (e.key === 'Escape') {
        if (_cv.zoom > 1 || _cv.scrollWidth !== 100) { cvResetZoom(); }
        else { closeMediaViewer(); }
    }
}

function cvClickNav(e) {
    if (_cv.scroll) return;  // no click-nav in scroll mode
    if (_cv.zoom > 1 || _cvDrag.moved) return;
    const x = e.clientX / window.innerWidth;
    if (_cv.rtl) {
        if (x > 0.7) cvNext();
        else if (x < 0.3) cvPrev();
    } else {
        if (x < 0.3) cvPrev();
        else if (x > 0.7) cvNext();
    }
}

// ---------------------------------------------------------------------------
// Scroll-mode keyboard navigation
// ---------------------------------------------------------------------------

// Scrolls by one viewport-unit in the given direction (+1 forward, -1 back).
// Jumps to the next/previous page when the current page boundary is already visible.
function _cvScrollNav(dir) {
    const stage = document.getElementById('cv-stage');
    if (!stage) return;

    if (_cv.scrollDir === 'h') {
        const step = (_cv.rtl ? -dir : dir) * stage.clientWidth * 0.9;
        const newLeft = stage.scrollLeft + step;
        const maxLeft = stage.scrollWidth - stage.clientWidth;
        stage.scrollTo({ left: Math.max(0, Math.min(maxLeft, newLeft)), behavior: 'smooth' });
        return;
    }

    // Vertical scroll mode: jump to the next/previous page by default.
    // Exception: when the current page is taller than the viewport and there is
    // more of it beyond the fold, scroll within the page instead.
    const stageRect = stage.getBoundingClientRect();
    const vh = stage.clientHeight;
    const curImg = stage.querySelector(`img.cv-page[data-page="${_cv.current}"]`);

    if (dir > 0) {
        const moreBelowFold = curImg && curImg.getBoundingClientRect().bottom > stageRect.bottom + 20;
        if (moreBelowFold) {
            // Current page continues below the fold — show the next chunk of it.
            stage.scrollTo({ top: Math.min(stage.scrollTop + vh * 0.9, stage.scrollHeight - vh), behavior: 'smooth' });
        } else {
            // Current page is fully visible (or no page tracked) — jump to the next page.
            const nextIdx = _cv.current + 1;
            if (nextIdx < _cv.pages.length) {
                const nextImg = stage.querySelector(`img.cv-page[data-page="${nextIdx}"]`);
                if (nextImg) {
                    const block = nextImg.clientHeight <= vh ? 'center' : 'start';
                    nextImg.scrollIntoView({ behavior: 'smooth', block });
                }
            }
        }
    } else {
        const moreAboveFold = curImg && curImg.getBoundingClientRect().top < stageRect.top - 20;
        if (moreAboveFold) {
            // Current page continues above the fold — show the previous chunk of it.
            stage.scrollTo({ top: Math.max(0, stage.scrollTop - vh * 0.9), behavior: 'smooth' });
        } else {
            // Current page top is visible (or no page tracked) — jump to the previous page.
            const prevIdx = _cv.current - 1;
            if (prevIdx >= 0) {
                const prevImg = stage.querySelector(`img.cv-page[data-page="${prevIdx}"]`);
                if (prevImg) {
                    const block = prevImg.clientHeight <= vh ? 'center' : 'end';
                    prevImg.scrollIntoView({ behavior: 'smooth', block });
                }
            }
        }
    }
}
