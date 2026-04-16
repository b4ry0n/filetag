// ---------------------------------------------------------------------------
// Zip directory: open, refresh, helper, grid + list render
// ---------------------------------------------------------------------------

/** Parse a virtual zip-entry DB path (e.g. "comics/arc.cbz::img.jpg").
 *  Returns {zipPath, entryName} or null. */
function parseZipEntryPath(path) {
    const idx = path ? path.indexOf('::') : -1;
    if (idx === -1) return null;
    return { zipPath: path.slice(0, idx), entryName: path.slice(idx + 2) };
}

async function openZipDir(zipPath) {
    _thumbClearCache();
    state.mode = 'zip';
    state.zipPath = zipPath;
    state.zipEntries = [];
    state.selectedFile = null;
    state.selectedDir = null;
    state.selectedPaths.clear();
    state.selectedFilesData.clear();
    _lastClickedPath = null;
    _armedBulkTag = null;
    // Show a loading indicator immediately so the UI does not appear frozen
    // while large archives are being scanned on the server.
    renderBreadcrumb();
    const el = document.getElementById('content');
    el.className = '';
    el.innerHTML = `<div class="empty-state"><span class="empty-state-icon">🗜️</span><span class="empty-state-text">Loading archive…</span></div>`;
    document.getElementById('entry-count').textContent = '…';
    const data = await api('/api/zip/entries?' + new URLSearchParams({ path: zipPath }) + rootParam('&'));
    state.zipEntries = data.entries || [];
    render();
}

async function refreshZipEntries() {
    if (!state.zipPath) return;
    const data = await api('/api/zip/entries?' + new URLSearchParams({ path: state.zipPath }) + rootParam('&'));
    state.zipEntries = data.entries || [];
    renderContent();
    _thumbInit();
}

function renderZipGrid(entries) {
    let html = '';
    for (const entry of entries) {
        // Entry names may include path components (e.g. "chapter1/img001.jpg")
        const displayName = entry.name.split('/').pop() || entry.name;
        const dbPath = state.zipPath + '::' + entry.name;
        const selected = state.selectedFile && state.selectedFile.path === dbPath ? ' selected' : '';

        let preview;
        if (entry.is_image) {
            const thumbUrl = '/api/zip/thumb?' + new URLSearchParams({ path: state.zipPath, page: entry.image_index }) + rootParam('&');
            preview = `<div class="card-thumb-pending" data-thumb-src="${thumbUrl}" data-name="${esc(displayName)}"></div>`;
        } else {
            preview = `<div class="card-icon">${fileIcon(displayName)}</div>`;
        }

        const tagBadge = entry.tag_count > 0
            ? `<span class="card-tag-count">${entry.tag_count}</span>` : '';
        const dblAttr = entry.is_image
            ? ` ondblclick="openMediaViewer('${jesc(state.zipPath)}', ${entry.image_index})"` : '';

        html += `<div class="card${selected}" data-path="${esc(dbPath)}"
            onclick="selectFile('${jesc(dbPath)}', event)"${dblAttr}>
            ${tagBadge}<div class="card-preview">${preview}</div>
            <div class="card-body"><div class="card-name">${esc(displayName)}</div>
            <div class="card-meta">${formatSize(entry.size)}</div></div>
        </div>`;
    }
    return html;
}

function renderZipList(entries) {
    let html = `<div class="list-header">
        <span></span><span>Name</span><span>Size</span><span>Tags</span>
    </div>`;
    for (const entry of entries) {
        const displayName = entry.name.split('/').pop() || entry.name;
        const dbPath = state.zipPath + '::' + entry.name;
        const selected = state.selectedFile && state.selectedFile.path === dbPath ? ' selected' : '';
        const icon = fileIcon(displayName);
        const size = formatSize(entry.size);
        const tags = entry.tag_count != null ? `${entry.tag_count} tags` : '';
        const dblAttr = entry.is_image
            ? ` ondblclick="openMediaViewer('${jesc(state.zipPath)}', ${entry.image_index})"` : '';
        html += `<div class="list-row${selected}" data-path="${esc(dbPath)}"
            onclick="selectFile('${jesc(dbPath)}', event)"${dblAttr}>
            <span class="icon">${icon}</span>
            <span class="name">${esc(displayName)}</span>
            <span class="size">${size}</span>
            <span class="tags-count">${tags}</span>
        </div>`;
    }
    return html;
}

// ---------------------------------------------------------------------------
// Preview error helpers (named functions, avoid SVG-in-HTML-attribute bugs)
// ---------------------------------------------------------------------------

function _previewImgError(img) {
    const p = img.closest('.detail-preview');
    if (!p) return;
    p.innerHTML = `<div class="no-preview">${fileIcon(img.dataset.name || '')}</div>`;
}

function _previewRawError(img) {
    const p = img.closest('.detail-preview');
    if (!p) return;
    p.innerHTML = `<div class="no-preview">${fileIcon(img.dataset.name || '')}<div class="preview-unavail-msg">Preview unavailable — install dcraw, exiftool, ffmpeg, or ImageMagick</div></div>`;
}

function _previewVideoError(video) {
    const n = video.dataset.name || '';
    const d = document.createElement('div');
    d.className = 'no-preview';
    d.innerHTML = `${fileIcon(n)}<div class="preview-unavail-msg">Browser cannot play this format</div>`;
    video.replaceWith(d);
}

// ---------------------------------------------------------------------------
// Video trickplay
// ---------------------------------------------------------------------------
// On hover over a video card, the single-image sprite sheet returned by
// /api/vthumbs is loaded once and cached.  Moving the cursor left-to-right
// shifts a CSS background-position over the sprite, showing different frames
// without any DOM or src changes (same technique as Jellyfin trickplay).

const _trickplayCache = new Map(); // path → {src, n} | 'loading' | 'failed'

/**
 * Attach trickplay behaviour to the <img> that replaced a .card-thumb-pending
 * for a video card.  Called from _thumbReplace() when data-video-path is set.
 */
function _trickplayAttach(img, path) {
    // Wrap img in a positional container that holds the progress bar.
    // The sprite popup is a fixed-position element on document.body so it can
    // break out of the card boundaries to show the frame at its natural AR.
    const wrap = document.createElement('div');
    wrap.className = 'card-trickplay';
    img.replaceWith(wrap);
    wrap.appendChild(img);
    const bar = document.createElement('div');
    bar.className = 'card-trickplay-bar';
    wrap.appendChild(bar);
    // Use the enclosing .card as the hover target so the sprite stays visible
    // when moving between the thumbnail area and the title/meta area below it.
    const card = wrap.closest('.card') || wrap;

    let spriteEl = null;
    let cacheEntry = null;

    function ensureSprite() {
        const cached = _trickplayCache.get(path);
        if (cached === 'loading' || cached === 'failed') return;
        if (cached) { cacheEntry = cached; return; }

        _trickplayCache.set(path, 'loading');
        const minN = state.settings.sprite_min ?? 8;
        const maxN = state.settings.sprite_max ?? 16;
        const src = '/api/vthumbs?' + new URLSearchParams({ path, min_n: minN, max_n: maxN })
            + rootParam('&');
        const preload = new Image();
        preload.onload = () => {
            // Each frame is scaled to 320 px wide by the server; derive n from
            // sprite width so the client doesn't need to pass or receive n.
            const n = Math.max(1, Math.round(preload.naturalWidth / 320));
            const entry = { src, n, natW: preload.naturalWidth, natH: preload.naturalHeight };
            _trickplayCache.set(path, entry);
            cacheEntry = entry;
            if (card.matches(':hover')) buildOverlay();
        };
        preload.onerror = () => {
            // Remove from cache so the next hover retries (server may have been busy).
            _trickplayCache.delete(path);
        };
        preload.src = src;
    }

    function buildOverlay() {
        if (spriteEl || !cacheEntry) return;
        const cardRect = wrap.getBoundingClientRect();
        if (!cardRect.width) return; // not yet laid out

        // Natural aspect ratio of a single tile.
        const frameW = cacheEntry.natW / cacheEntry.n;
        const frameH = cacheEntry.natH;
        const ar = frameW / frameH;
        const isPortrait = ar < 1;

        // For landscape: keep card height, expand width (max 16:9).
        // For portrait: keep card width, expand height (max 9:16 = height ≤ width * 16/9).
        let popupW, popupH;
        if (isPortrait) {
            const clampedAR = Math.max(ar, 9 / 16);
            popupW = cardRect.width;
            popupH = popupW / clampedAR;
        } else {
            const clampedAR = Math.min(ar, 16 / 9);
            popupH = cardRect.height;
            popupW = popupH * clampedAR;
        }
        popupW = Math.round(popupW);
        popupH = Math.round(popupH);

        // Horizontal: center on card.  Vertical: center for landscape, top for portrait.
        let left = cardRect.left + (cardRect.width - popupW) / 2;
        let top  = cardRect.top + (cardRect.height - popupH) / 2;

        // Clamp to viewport so the popup doesn't escape the screen.
        left = Math.max(4, Math.min(left, window.innerWidth  - popupW - 4));
        top  = Math.max(4, Math.min(top,  window.innerHeight - popupH - 4));

        spriteEl = document.createElement('div');
        spriteEl.className = 'card-trickplay-sprite';
        Object.assign(spriteEl.style, {
            position:        'fixed',
            zIndex:          '1000',
            pointerEvents:   'none',
            width:           popupW + 'px',
            height:          popupH + 'px',
            left:            left.toFixed(1) + 'px',
            top:             top.toFixed(1)  + 'px',
            backgroundImage: `url(${JSON.stringify(cacheEntry.src)})`,
            backgroundRepeat:'no-repeat',
            backgroundSize:  'auto 100%',
        });
        document.body.appendChild(spriteEl);
        window.addEventListener('scroll', reposition, { passive: true, capture: true });
        showFrame(0);
    }

    /** Recompute spriteEl position from the card's current viewport rect. */
    function reposition() {
        if (!spriteEl) return;
        const cardRect = wrap.getBoundingClientRect();
        const popupW = parseFloat(spriteEl.style.width);
        const popupH = parseFloat(spriteEl.style.height);
        let left = cardRect.left + (cardRect.width  - popupW) / 2;
        let top  = cardRect.top  + (cardRect.height - popupH) / 2;
        left = Math.max(4, Math.min(left, window.innerWidth  - popupW - 4));
        top  = Math.max(4, Math.min(top,  window.innerHeight - popupH - 4));
        spriteEl.style.left = left.toFixed(1) + 'px';
        spriteEl.style.top  = top.toFixed(1)  + 'px';
    }

    /** Jump to a discrete frame; pixel-offset preserves native AR and centers. */
    function showFrame(idx) {
        if (!spriteEl || !cacheEntry) return;
        const popupH = parseFloat(spriteEl.style.height);
        const popupW = parseFloat(spriteEl.style.width);
        const scale  = popupH / cacheEntry.natH;
        const tileW  = (cacheEntry.natW / cacheEntry.n) * scale;
        const x      = popupW / 2 - tileW * (idx + 0.5);
        spriteEl.style.backgroundPosition = `${x.toFixed(1)}px 0`;
    }

    /** Update frame and bar from a MouseEvent. Uses the card rect, not the
     *  popup rect, so the full [0, N-1] range is reachable across the card width. */
    function onMove(e) {
        if (!cacheEntry || !spriteEl) return;
        const rect = wrap.getBoundingClientRect();
        const frac = Math.max(0, Math.min(0.9999, (e.clientX - rect.left) / rect.width));
        const idx  = Math.min(cacheEntry.n - 1, Math.floor(frac * cacheEntry.n));
        showFrame(idx);
        bar.style.width = (frac * 100).toFixed(1) + '%';
    }

    function teardown() {
        if (spriteEl) {
            spriteEl.remove();
            spriteEl = null;
            window.removeEventListener('scroll', reposition, { capture: true });
        }
        bar.style.width = '0';
    }

    card.addEventListener('mouseenter', () => {
        ensureSprite();
        if (cacheEntry) buildOverlay();
    }, { passive: true });

    card.addEventListener('mouseleave', teardown);

    card.addEventListener('mousemove', e => {
        ensureSprite();
        if (!cacheEntry) return;
        if (!spriteEl) buildOverlay();
        onMove(e);
    }, { passive: true });
}

// ---------------------------------------------------------------------------
// Thumbnail queue: serial loader, visible-first via IntersectionObserver
// ---------------------------------------------------------------------------
// Cards render with <div class="card-thumb-pending" data-thumb-src="...">
// (or use a cached blob URL directly if available).
// _thumbInit() enqueues pending placeholders after each render.
// The IntersectionObserver promotes visible items to the front in
// top-to-bottom order. A single async worker processes one thumbnail at a
// time (matching the server-side semaphore of 1).

const _thumbQueue = [];
let _thumbBusy = false;
const _thumbCache = new Map(); // thumb URL → blob URL

const _thumbObserver = new IntersectionObserver((entries) => {
    // Collect all newly-visible elements from this batch.
    const newly = [];
    for (const e of entries) {
        if (!e.isIntersecting) continue;
        const el = e.target;
        _thumbObserver.unobserve(el);
        const i = _thumbQueue.indexOf(el);
        // i === -1 means the element was already shifted by _thumbRun and is
        // currently being fetched — don't re-queue it.
        if (i === -1) continue;
        // Remove from wherever it sits (including index 0) so we can re-insert
        // in proper sorted order below.
        _thumbQueue.splice(i, 1);
        newly.push(el);
    }
    if (newly.length > 0) {
        // Sort by vertical position so top-most cards are processed first
        // (standard document-order lazy loading, same as native loading="lazy").
        newly.sort((a, b) => a.getBoundingClientRect().top - b.getBoundingClientRect().top);
        _thumbQueue.unshift(...newly);
    }
    _thumbRun();
}, { rootMargin: '150px' });

function _thumbFlush() {
    // Remove orphaned (disconnected) entries and stop observing them.
    for (let i = _thumbQueue.length - 1; i >= 0; i--) {
        if (!_thumbQueue[i].isConnected) _thumbQueue.splice(i, 1);
    }
}

function _thumbInit() {
    _thumbFlush();
    document.querySelectorAll('.card-thumb-pending[data-thumb-src]').forEach(el => {
        const src = el.dataset.thumbSrc;
        // If we already have this thumbnail cached, replace immediately.
        if (_thumbCache.has(src)) {
            _thumbReplace(el, _thumbCache.get(src));
            return;
        }
        if (_thumbQueue.includes(el)) return;
        _thumbQueue.push(el);
        _thumbObserver.observe(el);
    });
    _thumbRun();
}

function _thumbReplace(el, blobUrl) {
    const img = document.createElement('img');
    img.src = blobUrl;
    if (el.dataset.cls) img.className = el.dataset.cls;
    img.alt = '';
    img.dataset.name = el.dataset.name || '';
    el.replaceWith(img);

    // Attach trickplay for video cards.
    if (el.dataset.videoPath) {
        _trickplayAttach(img, el.dataset.videoPath);
    }
}

async function _thumbRun() {
    if (_thumbBusy) return;
    _thumbBusy = true;
    while (_thumbQueue.length > 0) {
        const el = _thumbQueue.shift();
        if (!el.isConnected) continue;
        const src = el.dataset.thumbSrc;
        if (!src) continue;
        // Check cache (may have been filled by another element with the same URL).
        if (_thumbCache.has(src)) {
            _thumbReplace(el, _thumbCache.get(src));
            continue;
        }
        try {
            const resp = await fetch(src);
            if (!el.isConnected) continue;
            if (resp.ok) {
                const blob = await resp.blob();
                const url = URL.createObjectURL(blob);
                _thumbCache.set(src, url);
                if (el.isConnected) _thumbReplace(el, url);
            } else if (resp.status === 503) {
                // Server busy: re-queue at back.
                if (el.isConnected) {
                    _thumbQueue.push(el);
                    _thumbObserver.observe(el);
                }
            }
        } catch (_) { /* network error: leave placeholder */ }
    }
    _thumbBusy = false;
}

function _thumbClearCache() {
    for (const url of _thumbCache.values()) URL.revokeObjectURL(url);
    _thumbCache.clear();
    _thumbQueue.length = 0;
    _trickplayCache.clear();
}

// _cardThumbError is still used by detail-panel preview images (not thumb queue).
function _cardThumbError(img) {
    const name = img.dataset.name || '';
    const wrap = img.closest('.card-preview');
    if (wrap) wrap.innerHTML = `<div class="card-icon">${fileIcon(name)}</div>`;
}

// ---------------------------------------------------------------------------
// Render: Detail panel
// ---------------------------------------------------------------------------

function renderDetail() {
    const panel = document.getElementById('detail');

    // Clean up previous selection.

    // Multi-select bulk panel
    if (state.selectedPaths.size > 1) {
        const count = state.selectedPaths.size;
        const bulkTags = aggregateBulkTags();
        const chipsHtml = renderBulkTagChips(bulkTags, count);
        const paths = [...state.selectedPaths];
        const hasAiTagsBulk = bulkTags.some(t => t.name.startsWith('ai/'));
        const aiClearBulkBtn = hasAiTagsBulk
            ? `<button class="ai-clear-btn" onclick="aiClearTags(${JSON.stringify(paths)})">Verwijder alle ai/-tags</button>`
            : '';
        panel.innerHTML = `
            <div class="detail-header">
                <h3>${count} files selected</h3>
                <button class="detail-close" onclick="clearSelection()" title="Clear selection">&times;</button>
            </div>
            <div class="bulk-tag-section">
                ${bulkTags.length > 0 ? `<p class="bulk-section-label">Tags on selected files</p>
                <div class="bulk-tag-chips" id="bulk-tag-chips">${chipsHtml}</div>` : ''}
                <p class="bulk-section-label" style="margin-top:12px">Add tag</p>
                <div class="tag-add-form">
                    <input type="text" id="bulk-tag-input" placeholder="Tag (e.g. genre/rock)">
                    <button onclick="doBulkAddTag()">Add</button>
                </div>
                ${aiClearBulkBtn}
                <div id="bulk-status" class="bulk-status"></div>
            </div>`;
        attachTagAutocomplete(document.getElementById('bulk-tag-input'), () => doBulkAddTag());
        return;
    }

    if (!state.selectedFile && !state.selectedDir && state.selectedRoot == null) {
        panel.innerHTML = '<div class="detail-empty">Select a file or folder to see details</div>';
        return;
    }

    // Root card selected
    if (state.selectedRoot != null) {
        const rootMeta = state.roots.find(r => r.id === state.selectedRoot);
        const info = state.selectedRootInfo;
        const name = rootMeta ? rootMeta.name : `Root ${state.selectedRoot}`;
        const path = rootMeta ? rootMeta.path : '';
        const infoRows = info ? `
            <div class="detail-meta-row"><span class="detail-meta-label">Files</span><span class="detail-meta-value">${info.files.toLocaleString()}</span></div>
            <div class="detail-meta-row"><span class="detail-meta-label">Tags</span><span class="detail-meta-value">${info.tags.toLocaleString()}</span></div>
            <div class="detail-meta-row"><span class="detail-meta-label">Assignments</span><span class="detail-meta-value">${info.assignments.toLocaleString()}</span></div>
            <div class="detail-meta-row"><span class="detail-meta-label">Total size</span><span class="detail-meta-value">${formatSize(info.total_size)}</span></div>` : '<div class="detail-meta-row">Loading…</div>';
        panel.innerHTML = `
            <div class="detail-header">
                <h3>${esc(name)}</h3>
                <button class="detail-close" onclick="clearSelection()" title="Close">&times;</button>
            </div>
            <div class="detail-preview">
                <div class="no-preview" style="color:var(--primary)">${ICONS.root}</div>
            </div>
            <div class="detail-meta">
                <div class="detail-meta-row"><span class="detail-meta-label">Path</span><span class="detail-meta-value" style="word-break:break-all">${esc(path)}</span></div>
                ${infoRows}
            </div>
            <div style="padding:8px 12px">
                <button class="tag-action-btn" onclick="enterRoot(${state.selectedRoot})">Open database</button>
            </div>`;
        return;
    }

    // Directory selected
    if (state.selectedDir) {
        const d = state.selectedDir;
        panel.innerHTML = `
            <div class="detail-header">
                <h3>${esc(d.name)}</h3>
                <button class="detail-close" onclick="closeDetail()" title="Close">&times;</button>
            </div>
            <div class="detail-preview">
                <div class="no-preview" style="color:#fab005">${ICONS.folder}</div>
            </div>
            <div class="detail-meta">
                <div class="detail-meta-row"><span class="detail-meta-label">Path</span><span class="detail-meta-value">${esc(d.path)}</span></div>
                <div class="detail-meta-row"><span class="detail-meta-label">Items</span><span class="detail-meta-value">${d.file_count}</span></div>
            </div>`;
        return;
    }

    const f = state.selectedFile;
    const zipEntry = parseZipEntryPath(f.path);
    const name = zipEntry ? (zipEntry.entryName.split('/').pop() || zipEntry.entryName) : f.path.split('/').pop();
    const type_ = zipEntry ? fileType(zipEntry.entryName) : fileType(name);
    const previewUrl = '/preview/' + encodeURI(f.path) + rootParam('?');

    let preview;
    if (zipEntry) {
        // Entry inside a zip archive
        const entry = state.zipEntries.find(e => e.name === zipEntry.entryName);
        if (entry && entry.is_image && entry.image_index !== null) {
            const thumbUrl = '/api/zip/thumb?' + new URLSearchParams({ path: zipEntry.zipPath, page: entry.image_index }) + rootParam('&');
            preview = `<a class="preview-zoomable" onclick="openMediaViewer('${jesc(zipEntry.zipPath)}', ${entry.image_index})" title="Click to open in viewer">` +
                      `<img src="${thumbUrl}" alt="${esc(name)}" onerror="_cardThumbError(this)"></a>`;
        } else {
            preview = `<div class="no-preview">${fileIcon(name)}</div>`;
        }
    } else if (type_ === 'image') {
        preview = `<a class="preview-zoomable" onclick="openFileInDirViewer('${jesc(f.path)}')" title="Click to open in viewer">` +
                  `<img src="${previewUrl}" alt="${esc(name)}" data-name="${esc(name)}"` +
                  ` onerror="_previewImgError(this)"></a>`;
    } else if (type_ === 'raw') {
        preview = `<a class="preview-zoomable" onclick="openFileInDirViewer('${jesc(f.path)}')" title="Click to open in viewer">` +
                  `<img src="${previewUrl}" alt="${esc(name)}" data-name="${esc(name)}"` +
                  ` onerror="_previewRawError(this)"></a>`;
    } else if (type_ === 'audio') {
        preview = `<audio controls preload="metadata" src="${previewUrl}" ondblclick="openLightbox('${jesc(f.path)}','audio')"></audio>`;
    } else if (type_ === 'video') {
        preview = `<video controls preload="metadata" src="${previewUrl}" data-name="${esc(name)}"` +
                  ` onerror="_previewVideoError(this)"></video>`;
    } else if (type_ === 'pdf') {
        preview = `<iframe class="preview-pdf" src="${previewUrl}" title="${esc(name)}"></iframe>` +
                  `<div style="text-align:center;padding:4px 0"><button class="tag-action-btn" onclick="openLightbox('${jesc(f.path)}','pdf')">Full-size PDF</button></div>`;
    } else if (type_ === 'markdown') {
        preview = `<div class="preview-markdown" id="preview-md-content" ondblclick="openLightbox('${jesc(f.path)}','markdown')"` +
                  ` title="Double-click to enlarge">Loading…</div>`;
    } else if (type_ === 'text') {
        preview = `<pre class="preview-text" id="preview-text-content" ondblclick="openLightbox('${jesc(f.path)}','text')"` +
                  ` title="Double-click to enlarge">Loading…</pre>`;
    } else if (type_ === 'zip') {
        preview = `<div class="zip-cover-wrap">
            <img src="/thumb/${encodeURI(f.path)}${rootParam('?')}" alt="${esc(name)}" class="zip-cover"
                 onerror="this.style.display='none'">
            <button class="tag-action-btn" onclick="openMediaViewer('${jesc(f.path)}')">Open in viewer</button>
        </div>`;
    } else {
        preview = `<div class="no-preview">${fileIcon(name)}</div>`;
    }

    const covered = f.covered !== false;

    const hasAiTags = covered && f.tags.some(t => t.name.startsWith('ai/'));
    const tagChips = f.tags.length === 0
        ? '<span class="no-tags">No tags assigned</span>'
        : f.tags.map(t => {
            const tagStr = formatTag(t);
            const stateTag = state.tags.find(st => st.name === t.name);
            const chipColor = stateTag?.color ? ` style="border-left: 3px solid ${stateTag.color}"` : '';
            if (!covered) {
                return `<span class="tag-chip tag-chip--readonly"${chipColor}>${esc(tagStr)}</span>`;
            }
            const promoteBtn = t.name.startsWith('ai/')
                ? `<button class="promote" title="Bewaar zonder ai/-prefix" onclick="aiPromoteTag('${jesc(f.path)}','${jesc(t.name)}','${jesc(t.value || '')}')">&uarr;</button>`
                : '';
            return `<span class="tag-chip"${chipColor}>${esc(tagStr)}${promoteBtn}<button class="remove" onclick="doRemoveTag('${jesc(f.path)}','${jesc(tagStr)}')">&times;</button></span>`;
        }).join('');

    const tagAddSection = covered
        ? `<div class="tag-add-form">
                <input type="text" id="tag-input" placeholder="Add tag (e.g. genre/rock)">
                <button onclick="doAddTag()">Add</button>
            </div>`
        : `<div class="uncovered-notice">This file is on a different filesystem. Tags cannot be added here.</div>`;

    const isAnalysable = covered && (type_ === 'image' || type_ === 'raw' || type_ === 'zip');
    const isAnalysing = state.aiAnalysing.has(f.path);
    const aiClearBtn = hasAiTags
        ? `<button class="ai-clear-btn" onclick="aiClearTags(['${jesc(f.path)}'])">Verwijder ai/-tags</button>`
        : '';
    const aiBtn = isAnalysable || hasAiTags
        ? `<div class="ai-analyse-row">${isAnalysable ? `<button class="ai-analyse-btn" id="ai-analyse-single-btn" onclick="aiAnalyseSingle('${jesc(f.path)}')" ${isAnalysing ? 'disabled' : ''}>${isAnalysing ? 'Analyseren…' : '✨ Analyse (AI)'}</button>` : ''}${aiClearBtn}</div>`
        : '';

    panel.innerHTML = `
        <div class="detail-top">
        <div class="detail-header">
            <h3>${esc(name)}</h3>
            <button class="detail-close" onclick="closeDetail()" title="Close">&times;</button>
        </div>
        <div class="detail-preview">${preview}</div>
        <div class="detail-meta">
            ${zipEntry
                ? `<div class="detail-meta-row"><span class="detail-meta-label">Archive</span><span class="detail-meta-value">${esc(zipEntry.zipPath.split('/').pop())}</span></div>
                   <div class="detail-meta-row"><span class="detail-meta-label">Entry</span><span class="detail-meta-value">${esc(zipEntry.entryName)}</span></div>`
                : `<div class="detail-meta-row"><span class="detail-meta-label">Path</span><span class="detail-meta-value">${esc(f.path)}</span></div>
                   <div class="detail-meta-row"><span class="detail-meta-label">Size</span><span class="detail-meta-value">${formatSize(f.size)}</span></div>
                   ${f.indexed_at ? `<div class="detail-meta-row"><span class="detail-meta-label">Indexed</span><span class="detail-meta-value">${esc(f.indexed_at)}</span></div>` : ''}`
            }
        </div>
        </div>
        <div class="detail-v-handle" id="detail-v-handle"></div>
        <div class="detail-tags-section">
            <h4>Tags</h4>
            <div class="detail-tags">${tagChips}</div>
            ${tagAddSection}
            ${aiBtn}
        </div>`;

    if (covered) attachTagAutocomplete(document.getElementById('tag-input'), () => doAddTag());
    initDetailVHandle(document.getElementById('detail-v-handle'));

    // Async-fetch text/markdown content after DOM is set
    if (type_ === 'text') {
        const el = document.getElementById('preview-text-content');
        if (el) {
            fetch(previewUrl).then(r => {
                if (!r.ok) throw new Error(r.statusText);
                return r.text();
            }).then(txt => {
                const clipped = txt.length > 60000 ? txt.slice(0, 60000) + '\n…' : txt;
                if (el) el.innerHTML = highlightCode(clipped, name);
            }).catch(() => {
                if (el) el.textContent = '(Could not load preview)';
            });
        }
    } else if (type_ === 'markdown') {
        const el = document.getElementById('preview-md-content');
        if (el) {
            fetch(previewUrl).then(r => {
                if (!r.ok) throw new Error(r.statusText);
                return r.text();
            }).then(txt => {
                if (el) el.innerHTML = renderMarkdown(txt);
            }).catch(() => {
                if (el) el.textContent = '(Could not load preview)';
            });
        }
    }
}

// Update only the tag chips in the detail panel, leaving the preview (video/audio/image) untouched.
function renderDetailTagsOnly() {
    if (!state.selectedFile) return;
    const tagsEl = document.querySelector('#detail .detail-tags');
    if (!tagsEl) return;
    const f = state.selectedFile;
    const tagChips = f.tags.length === 0
        ? '<span class="no-tags">No tags assigned</span>'
        : f.tags.map(t => {
            const tagStr = formatTag(t);
            const stateTag = state.tags.find(st => st.name === t.name);
            const chipColor = stateTag?.color ? ` style="border-left: 3px solid ${stateTag.color}"` : '';
            const promoteBtn = t.name.startsWith('ai/')
                ? `<button class="promote" title="Bewaar zonder ai/-prefix" onclick="aiPromoteTag('${jesc(f.path)}','${jesc(t.name)}','${jesc(t.value || '')}')">&uarr;</button>`
                : '';
            return `<span class="tag-chip"${chipColor}>${promoteBtn}${esc(tagStr)}<button class="remove" onclick="doRemoveTag('${jesc(f.path)}','${jesc(tagStr)}')">&times;</button></span>`;
        }).join('');
    tagsEl.innerHTML = tagChips;
}

// ---------------------------------------------------------------------------
// Bulk tag helpers (multi-select)
// ---------------------------------------------------------------------------

function aggregateBulkTags() {
    const counts = new Map(); // tagStr → count of selected files that have it
    for (const [path, data] of state.selectedFilesData) {
        if (!state.selectedPaths.has(path)) continue;
        for (const t of (data.tags || [])) {
            const str = formatTag(t);
            counts.set(str, (counts.get(str) || 0) + 1);
        }
    }
    return [...counts.entries()]
        .map(([tagStr, count]) => ({ tagStr, count }))
        .sort((a, b) => b.count - a.count || a.tagStr.localeCompare(b.tagStr));
}

function renderBulkTagChips(bulkTags, total) {
    if (bulkTags.length === 0) return '';
    return bulkTags.map(({ tagStr, count }) => {
        const stateTag = state.tags.find(st => st.name === tagStr || st.name === tagStr.split('=')[0]);
        const chipBorder = stateTag?.color ? ` style="border-left: 3px solid ${stateTag.color}"` : '';
        const countBadge = count < total
            ? `<span class="bulk-chip-count">${count}/${total}</span>`
            : '';
        const isArmed = _armedBulkTag === tagStr;
        if (isArmed) {
            return `<span class="bulk-chip armed"${chipBorder}>
                <span class="bulk-chip-label">${esc(tagStr)}${countBadge}</span>
                <button class="bulk-chip-cancel" onclick="armBulkTag('${jesc(tagStr)}')" title="Cancel">&#8617;</button>
                <button class="bulk-chip-fire" onclick="doBulkRemoveTagChip('${jesc(tagStr)}')">Remove</button>
            </span>`;
        }
        return `<span class="bulk-chip"${chipBorder}>
            <span class="bulk-chip-label">${esc(tagStr)}${countBadge}</span>
            <button class="bulk-chip-arm" onclick="armBulkTag('${jesc(tagStr)}')" title="Remove from selection">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14H6L5 6"/><path d="M10 11v6M14 11v6"/><path d="M9 6V4h6v2"/></svg>
            </button>
        </span>`;
    }).join('');
}

function armBulkTag(tagStr) {
    _armedBulkTag = _armedBulkTag === tagStr ? null : tagStr;
    const el = document.getElementById('bulk-tag-chips');
    if (el) {
        const total = state.selectedPaths.size;
        el.innerHTML = renderBulkTagChips(aggregateBulkTags(), total);
    }
}

async function doBulkRemoveTagChip(tagStr) {
    _armedBulkTag = null;
    // Only remove from files that actually have this tag
    const paths = [...state.selectedPaths].filter(p => {
        const data = state.selectedFilesData.get(p);
        return data && data.tags.some(t => formatTag(t) === tagStr);
    });
    await Promise.all(paths.map(p => apiPost('/api/untag', { path: p, tags: [tagStr], root_id: state.currentRootId })));
    // Update cache locally
    for (const p of paths) {
        const d = state.selectedFilesData.get(p);
        if (d) d.tags = d.tags.filter(t => formatTag(t) !== tagStr);
    }
    await loadTags();
    if (state.mode === 'browse') await loadFiles(state.currentPath);
    const status = document.getElementById('bulk-status');
    if (status) status.textContent = `Removed "${tagStr}" from ${paths.length} file${paths.length === 1 ? '' : 's'}.`;
    const el = document.getElementById('bulk-tag-chips');
    if (el) el.innerHTML = renderBulkTagChips(aggregateBulkTags(), state.selectedPaths.size);
    renderTags();
    renderContent();
    _thumbInit();
}

// ---------------------------------------------------------------------------
// Render: DB info header
// ---------------------------------------------------------------------------

function renderInfo() {
    if (!state.info) return;
    const i = state.info;
    document.getElementById('db-info').textContent =
        `${i.files} files, ${i.tags} tags, ${formatSize(i.total_size)}`;
}

// ---------------------------------------------------------------------------
// Full render
// ---------------------------------------------------------------------------

function render() {
    renderTags();
    renderBreadcrumb();
    renderContent();
    renderDetail();
    renderInfo();
    _thumbInit();
    _kbRestoreFocus();
}
