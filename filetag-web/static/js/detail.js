// Globale functie voor info-overlay
window.toggleMetaOverlay = function(e) {
    e.stopPropagation();
    const overlay = document.getElementById('meta-overlay');
    if (!overlay) return;
    overlay.style.display = (overlay.style.display === 'none' || !overlay.style.display) ? 'flex' : 'none';
};
// ---------------------------------------------------------------------------
// Zoom & pan functionaliteit voor afbeelding in detail-preview
// ---------------------------------------------------------------------------

function enableDetailPreviewZoomPan() {
    const preview = document.querySelector('.detail-preview');
    if (!preview) return;
    const img = preview.querySelector('img');
    if (!img) return;
    let zoom = 1;
    let panX = 0, panY = 0;
    let dragging = false, lastX = 0, lastY = 0;

    function update() {
        img.style.transform = `scale(${zoom}) translate(${panX/zoom}px,${panY/zoom}px)`;
        img.style.cursor = zoom > 1 ? 'grab' : 'zoom-in';
    }

    img.addEventListener('dblclick', e => {
        if (zoom === 1) {
            zoom = 2;
            img.style.cursor = 'grab';
        } else {
            zoom = 1; panX = 0; panY = 0;
        }
        update();
    });

    img.addEventListener('mousedown', e => {
        if (zoom === 1) return;
        dragging = true;
        lastX = e.clientX; lastY = e.clientY;
        img.style.cursor = 'grabbing';
        e.preventDefault();
    });
    window.addEventListener('mousemove', e => {
        if (!dragging) return;
        panX += e.clientX - lastX;
        panY += e.clientY - lastY;
        lastX = e.clientX; lastY = e.clientY;
        // Clamp zodat je niet buiten het frame kunt pannen
        const rect = img.getBoundingClientRect();
        const pRect = preview.getBoundingClientRect();
        const maxX = Math.max(0, (rect.width - pRect.width) / 2);
        const maxY = Math.max(0, (rect.height - pRect.height) / 2);
        panX = Math.max(-maxX, Math.min(maxX, panX));
        panY = Math.max(-maxY, Math.min(maxY, panY));
        update();
    });
    window.addEventListener('mouseup', () => {
        if (dragging) img.style.cursor = 'grab';
        dragging = false;
    });
    // Reset bij nieuwe afbeelding
    img.addEventListener('load', () => { zoom = 1; panX = 0; panY = 0; update(); });
    update();
}

// Activeer na elke render
setTimeout(enableDetailPreviewZoomPan, 0);
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
    state.zipSubdir = '';
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
    const data = await api('/api/zip/entries?' + new URLSearchParams({ path: zipPath }) + dirParam('&'));
    state.zipEntries = data.entries || [];

    // If archive root contains exactly one folder and no files, jump into it
    // immediately. Users can still navigate back via the breadcrumb.
    const rootContents = getZipDirContents(state.zipEntries, '');
    if (rootContents.folders.length === 1 && rootContents.files.length === 0) {
        state.zipSubdir = rootContents.folders[0] + '/';
    }

    render();
    _navPush();
}

async function refreshZipEntries() {
    if (!state.zipPath) return;
    const data = await api('/api/zip/entries?' + new URLSearchParams({ path: state.zipPath }) + dirParam('&'));
    state.zipEntries = data.entries || [];
    renderContent();
    _thumbInit();
    _dirThumbInit();
    _kbRestoreFocus();
}

/**
 * Navigate to a sub-path within the currently open archive.
 * Pass '' to go back to the archive root.
 */
function enterZipSubdir(subdir) {
    state.zipSubdir = subdir;
    state.selectedFile = null;
    state.selectedDir = null;
    state.selectedPaths.clear();
    state.selectedFilesData.clear();
    _lastClickedPath = null;
    _kbCursor = -1;
    renderBreadcrumb();
    renderContent();
    _thumbInit();
    _dirThumbInit();
    _kbRestoreFocus();
    _navPush();
}

/**
 * Split zip entries into the immediate sub-folders and files visible at
 * the current sub-path (`subdir`).  Returns { folders: string[], files: ZipEntry[] }.
 */
function getZipDirContents(entries, subdir) {
    const folders = new Set();
    const files = [];
    for (const entry of entries) {
        if (!entry.name.startsWith(subdir)) continue;
        const rest = entry.name.slice(subdir.length);
        if (!rest) continue;
        const slash = rest.indexOf('/');
        if (slash !== -1) {
            folders.add(rest.slice(0, slash));
        } else {
            files.push(entry);
        }
    }
    return { folders: [...folders].sort(), files };
}

function renderZipGrid(entries) {
    const { folders, files } = getZipDirContents(entries, state.zipSubdir);
    let html = '';

    // Folder entries (navigate into zip sub-directory)
    for (const folder of folders) {
        const target = state.zipSubdir + folder + '/';
        html += `<div class="card folder" data-zip-folder="${esc(folder)}"
            ondblclick="enterZipSubdir('${jesc(target)}')">
            <div class="card-preview"><div class="card-icon">${ICONS.folder}</div></div>
            <div class="card-body"><div class="card-name">${esc(folder)}</div>
            <div class="card-meta">folder</div></div>
        </div>`;
    }

    // File entries
    for (const entry of files) {
        const displayName = entry.name.split('/').pop() || entry.name;
        const dbPath = state.zipPath + '::' + entry.name;
        const selected = state.selectedPaths.has(dbPath) ? ' selected' : '';

        let preview;
        if (entry.is_image) {
            const thumbUrl = '/api/zip/thumb?' + new URLSearchParams({ path: state.zipPath, page: entry.image_index }) + dirParam('&');
            preview = `<div class="card-icon" data-thumb-src="${thumbUrl}" data-name="${esc(displayName)}" data-thumb-hover="1">${fileIcon(displayName)}</div>`;
        } else {
            preview = `<div class="card-icon">${fileIcon(displayName)}</div>`;
        }

        const tagBadge = entry.tag_count > 0
            ? `<span class="card-tag-count">${entry.tag_count}</span>` : '';
        const checkmark = state.selectedPaths.has(dbPath)
            ? '<span class="card-check">&#10003;</span>' : '';
        const dblAttr = entry.is_image
            ? ` ondblclick="openMediaViewer('${jesc(state.zipPath)}', ${entry.image_index})"` : '';

        html += `<div class="card${selected}" data-path="${esc(dbPath)}"
            onclick="selectFile('${jesc(dbPath)}', event)"${dblAttr}>
            ${checkmark}${tagBadge}<div class="card-preview">${preview}</div>
            <div class="card-body"><div class="card-name">${esc(displayName)}</div>
            <div class="card-meta">${formatSize(entry.size)}</div></div>
        </div>`;
    }
    return html;
}

function renderZipList(entries) {
    const { folders, files } = getZipDirContents(entries, state.zipSubdir);
    let html = `<div class="list-header">
        <span></span><span>Name</span><span>Size</span><span>Tags</span>
    </div>`;

    // Folder entries
    for (const folder of folders) {
        const target = state.zipSubdir + folder + '/';
        html += `<div class="list-row folder" data-zip-folder="${esc(folder)}"
            ondblclick="enterZipSubdir('${jesc(target)}')">
            <span class="icon">${ICONS.folder}</span>
            <span class="name">${esc(folder)}</span>
            <span class="size"></span>
            <span class="tags-count"></span>
        </div>`;
    }

    // File entries
    for (const entry of files) {
        const displayName = entry.name.split('/').pop() || entry.name;
        const dbPath = state.zipPath + '::' + entry.name;
        const selected = state.selectedPaths.has(dbPath) ? ' selected' : '';
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
    p.innerHTML = `<div class="no-preview">${fileIcon(img.dataset.name || '')}<div class="preview-unavail-msg">Preview unavailable — install dcraw, ffmpeg, or ImageMagick and enable the corresponding feature in Settings</div></div>`;
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
    // The floating sprite popup is a fixed-position element on document.body so
    // it can break out of the card boundaries.  When the card is clicked the
    // sprite is shown inline (pinned) directly inside the card instead.
    const wrap = document.createElement('div');
    wrap.className = 'card-trickplay';
    img.replaceWith(wrap);
    wrap.appendChild(img);
    // Use the enclosing .card as the hover target so the sprite stays visible
    // when moving between the thumbnail area and the title/meta area below it.
    const card = wrap.closest('.card') || wrap;

    let spriteEl  = null; // floating popup (hover)
    let pinnedEl  = null; // inline pinned (after click)
    let wantPin   = false; // pin requested before cacheEntry loaded
    let cacheEntry = null;

    function ensureSprite() {
        const cached = _trickplayCache.get(path);
        if (cached === 'loading' || cached === 'failed') return;
        if (cached) { cacheEntry = cached; return; }

        _trickplayCache.set(path, 'loading');
        const minN = state.settings.sprite_min ?? 8;
        const maxN = state.settings.sprite_max ?? 16;
        const src = '/api/vthumbs?' + new URLSearchParams({ path, min_n: minN, max_n: maxN })
            + dirParam('&');
        const preload = new Image();
        preload.onload = () => {
            // Each frame is scaled to 320 px wide by the server; derive n from
            // sprite width so the client doesn't need to pass or receive n.
            const n = Math.max(1, Math.round(preload.naturalWidth / 320));
            const entry = { src, n, natW: preload.naturalWidth, natH: preload.naturalHeight };
            _trickplayCache.set(path, entry);
            cacheEntry = entry;
            if (card.matches(':hover') && !pinnedEl) buildOverlay();
            if (wantPin) buildInline();
        };
        preload.onerror = () => {
            // Mark as failed; schedule a retry after 3 s so the next hover
            // can try again without hammering a busy server.
            _trickplayCache.set(path, 'failed');
            setTimeout(() => {
                if (_trickplayCache.get(path) === 'failed') {
                    _trickplayCache.delete(path);
                }
            }, 3000);
        };
        preload.src = src;
    }

    function buildOverlay() {
        if (spriteEl || pinnedEl || !cacheEntry) return;
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

    /** Build an inline pinned sprite that fills the card preview area.
     *  @param {number} [clientX] - mouse X from the triggering event, used to
     *  show the correct frame immediately rather than defaulting to frame 0. */
    function buildInline(clientX) {
        if (!cacheEntry) { wantPin = true; return; }
        wantPin = false;
        if (pinnedEl && pinnedEl.isConnected) return; // already pinned
        pinnedEl = document.createElement('div');
        pinnedEl.className = 'card-trickplay-pinned';
        Object.assign(pinnedEl.style, {
            backgroundImage:  `url(${JSON.stringify(cacheEntry.src)})`,
            backgroundRepeat: 'no-repeat',
            backgroundSize:   'auto 100%',
        });
        wrap.appendChild(pinnedEl);
        if (clientX !== undefined) {
            const rect = wrap.getBoundingClientRect();
            const frac = Math.max(0, Math.min(0.9999, (clientX - rect.left) / rect.width));
            const idx  = Math.min(cacheEntry.n - 1, Math.floor(frac * cacheEntry.n));
            showPinnedFrame(idx);
        } else {
            showPinnedFrame(0);
        }
    }

    function teardownInline() {
        wantPin = false;
        if (pinnedEl) { pinnedEl.remove(); pinnedEl = null; }
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

    /** Jump to a discrete frame in the floating overlay. */
    function showFrame(idx) {
        if (!spriteEl || !cacheEntry) return;
        const popupH = parseFloat(spriteEl.style.height);
        const popupW = parseFloat(spriteEl.style.width);
        const frameW = cacheEntry.natW / cacheEntry.n;
        const frameH = cacheEntry.natH;
        // Cover scaling: ensure the tile fully covers the popup so adjacent
        // frames never bleed in from the sides or top/bottom.
        const scale  = Math.max(popupW / frameW, popupH / frameH);
        const bsW    = Math.round(cacheEntry.natW * scale);
        const bsH    = Math.round(cacheEntry.natH * scale);
        const tileW  = frameW * scale;
        const tileH  = frameH * scale;
        const x      = popupW / 2 - tileW * (idx + 0.5);
        const y      = (popupH - tileH) / 2;
        spriteEl.style.backgroundSize     = `${bsW}px ${bsH}px`;
        spriteEl.style.backgroundPosition = `${x.toFixed(1)}px ${y.toFixed(1)}px`;
    }

    /** Jump to a discrete frame in the inline pinned element. */
    function showPinnedFrame(idx) {
        if (!pinnedEl || !cacheEntry) return;
        const h      = wrap.offsetHeight || 140;
        const w      = wrap.offsetWidth  || 140;
        const frameW = cacheEntry.natW / cacheEntry.n;
        const frameH = cacheEntry.natH;
        // Cover scaling: ensure the tile fully covers the card area so adjacent
        // frames never bleed in from the sides or top/bottom.
        const scale  = Math.max(w / frameW, h / frameH);
        const bsW    = Math.round(cacheEntry.natW * scale);
        const bsH    = Math.round(cacheEntry.natH * scale);
        const tileW  = frameW * scale;
        const tileH  = frameH * scale;
        const x      = w / 2 - tileW * (idx + 0.5);
        const y      = (h - tileH) / 2;
        pinnedEl.style.backgroundSize     = `${bsW}px ${bsH}px`;
        pinnedEl.style.backgroundPosition = `${x.toFixed(1)}px ${y.toFixed(1)}px`;
    }

    /** Update frame from a MouseEvent. Uses the card rect, not the
     *  popup rect, so the full [0, N-1] range is reachable across the card width. */
    function onMove(e) {
        if (!cacheEntry) return;
        const rect = wrap.getBoundingClientRect();
        const frac = Math.max(0, Math.min(0.9999, (e.clientX - rect.left) / rect.width));
        const idx  = Math.min(cacheEntry.n - 1, Math.floor(frac * cacheEntry.n));
        if (spriteEl)  showFrame(idx);
        if (pinnedEl)  showPinnedFrame(idx);
    }

    function teardown() {
        if (spriteEl) {
            spriteEl.remove();
            spriteEl = null;
            window.removeEventListener('scroll', reposition, { capture: true });
        }
    }

    card.addEventListener('mouseenter', () => {
        ensureSprite();
        if (cacheEntry && !pinnedEl) buildOverlay();
    }, { passive: true });

    card.addEventListener('mouseleave', () => {
        teardown();
        teardownInline();
    });

    card.addEventListener('mousemove', e => {
        ensureSprite();
        if (!cacheEntry) return;
        if (!spriteEl && !pinnedEl) buildOverlay();
        onMove(e);
    }, { passive: true });

    // Click: show inline trickplay while hovering over the card.
    card.addEventListener('click', e => {
        if (e.target.closest('button, a')) return;
        teardown(); // dismiss floating overlay
        // Unpin any other card that was previously pinned.
        document.querySelectorAll('.card-trickplay-pinned').forEach(el => {
            if (!wrap.contains(el)) el.remove();
        });
        // Toggle pinned state for this card.
        if (pinnedEl && pinnedEl.isConnected) {
            teardownInline();
        } else {
            pinnedEl = null; // clear stale reference if detached by external cleanup
            ensureSprite();
            buildInline(e.clientX);
        }
    });
}

// ---------------------------------------------------------------------------
// Thumb hover popup: full-image floating preview for images / archives
// ---------------------------------------------------------------------------
// On hover the thumbnail is shown as a fixed-position popup sized to the
// image's natural aspect ratio (similar to video trickplay but without
// frame scrubbing).  Clicking pins the image inline inside the card;
// clicking again removes the pinned view.

/** Teardown function for the currently visible hover popup (if any). */
let _activeThumbPopupTeardown = null;

/** Remove any visible hover popup immediately (called on re-render, keydown, etc.). */
function dismissThumbPopup() {
    if (_activeThumbPopupTeardown) {
        _activeThumbPopupTeardown();
        _activeThumbPopupTeardown = null;
    }
}

// Dismiss the popup on any keydown — keyboard shortcuts (Cmd+A, Escape, etc.)
// can trigger a re-render that replaces card DOM nodes without firing mouseleave.
document.addEventListener('keydown', dismissThumbPopup, { passive: true, capture: true });

/**
 * Attach hover-popup behaviour to an <img> whose blob URL is already known.
 * @param {HTMLImageElement} img     The thumbnail image element inside .card-preview.
 * @param {string}           blobUrl The already-loaded blob URL for this thumb.
 */
function _thumbHoverAttach(img, blobUrl) {
    const wrap = document.createElement('div');
    wrap.className = 'card-trickplay'; // reuse positioning wrapper
    img.replaceWith(wrap);
    wrap.appendChild(img);
    const card = wrap.closest('.card') || wrap;

    let popupEl  = null;
    // When the user clicks while a popup is showing, suppress the popup
    // until the mouse leaves and re-enters the card.
    let _suppressPopup = false;

    // Natural dimensions become known once the img has loaded.
    function getNatAR() {
        if (img.naturalWidth && img.naturalHeight) return img.naturalWidth / img.naturalHeight;
        return 1;
    }

    function buildPopup() {
        if (popupEl || _suppressPopup) return;
        const cardRect = wrap.getBoundingClientRect();
        if (!cardRect.width) return;
        const ar = getNatAR();
        const isPortrait = ar < 1;

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

        let left = cardRect.left + (cardRect.width - popupW) / 2;
        let top  = cardRect.top  + (cardRect.height - popupH) / 2;
        left = Math.max(4, Math.min(left, window.innerWidth  - popupW - 4));
        top  = Math.max(4, Math.min(top,  window.innerHeight - popupH - 4));

        popupEl = document.createElement('div');
        popupEl.className = 'card-thumb-popup';
        Object.assign(popupEl.style, {
            width:  popupW + 'px',
            height: popupH + 'px',
            left:   left.toFixed(1) + 'px',
            top:    top.toFixed(1)  + 'px',
        });
        const popupImg = document.createElement('img');
        popupImg.src = blobUrl;
        popupImg.alt = '';
        popupEl.appendChild(popupImg);
        document.body.appendChild(popupEl);
        window.addEventListener('scroll', repositionPopup, { passive: true, capture: true });
        // Register globally so keyboard shortcuts can dismiss the popup even
        // when the card DOM node is replaced (and mouseleave never fires).
        _activeThumbPopupTeardown = teardownPopup;
    }

    function repositionPopup() {
        if (!popupEl) return;
        const cardRect = wrap.getBoundingClientRect();
        const popupW = parseFloat(popupEl.style.width);
        const popupH = parseFloat(popupEl.style.height);
        let left = cardRect.left + (cardRect.width  - popupW) / 2;
        let top  = cardRect.top  + (cardRect.height - popupH) / 2;
        left = Math.max(4, Math.min(left, window.innerWidth  - popupW - 4));
        top  = Math.max(4, Math.min(top,  window.innerHeight - popupH - 4));
        popupEl.style.left = left.toFixed(1) + 'px';
        popupEl.style.top  = top.toFixed(1)  + 'px';
    }

    function teardownPopup() {
        if (popupEl) {
            popupEl.remove();
            popupEl = null;
            window.removeEventListener('scroll', repositionPopup, { capture: true });
        }
        if (_activeThumbPopupTeardown === teardownPopup) {
            _activeThumbPopupTeardown = null;
        }
    }

    card.addEventListener('mouseenter', () => {
        _suppressPopup = false;
        buildPopup();
    }, { passive: true });

    card.addEventListener('mouseleave', () => {
        _suppressPopup = false;
        teardownPopup();
    });

    // Click: if popup is showing, hide it (suppress until mouse re-enters).
    // If popup is hidden (suppressed), re-show it.
    card.addEventListener('click', e => {
        if (e.target.closest('button, a')) return;
        if (popupEl) {
            teardownPopup();
            _suppressPopup = true;
        } else {
            _suppressPopup = false;
            buildPopup();
        }
    });
}

// ---------------------------------------------------------------------------
// Directory thumbnail init (delegates to _thumbInit via data-thumb-src)
// ---------------------------------------------------------------------------

/**
 * Called after every grid render.  Dir thumbnails now use data-thumb-src and
 * are processed by the regular _thumbInit queue, so this is a no-op.
 */
function _dirThumbInit() {
    // No-op: dir cards use data-thumb-src → handled by _thumbInit.
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
    document.querySelectorAll('.card-icon[data-thumb-src]').forEach(el => {
        const src = el.dataset.thumbSrc;
        // If we already have this thumbnail cached, replace immediately.
        if (_thumbCache.has(src)) {
            const cached = _thumbCache.get(src);
            if (cached) _thumbReplace(el, cached); else _thumbShowFailed(el);
            return;
        }
        if (_thumbQueue.includes(el)) return;
        _thumbQueue.push(el);
        _thumbObserver.observe(el);
    });
    _thumbRun();
}

function _thumbReplace(el, blobUrl) {
    // Directory preview: inline sprite cycling (no floating popup).
    if (el.dataset.dirPath) {
        _dirPreviewReplace(el, blobUrl);
        return;
    }
    const img = document.createElement('img');
    img.src = blobUrl;
    if (el.dataset.cls) img.className = el.dataset.cls;
    img.alt = '';
    img.dataset.name = el.dataset.name || '';
    el.replaceWith(img);

    // Attach trickplay for video cards.
    if (el.dataset.videoPath) {
        _trickplayAttach(img, el.dataset.videoPath);
    } else if (el.dataset.thumbHover) {
        // Image / archive cards: hover shows full-thumb popup.
        _thumbHoverAttach(img, blobUrl);
    }
}

/** Mark a card-icon element as permanently failed: strip data-thumb-src so it
 * is never re-queued. The element already shows the right icon. */
function _thumbShowFailed(el) {
    el.removeAttribute('data-thumb-src');
}

/**
 * Replace a directory `.card-icon` with an inline sprite div.
 * The sprite sheet contains N square frames (240 × 240 px each) side-by-side.
 * Frame 0 is shown by default; hovering the card cycles through frames.
 * Because `.card-icon` is removed the card switches automatically from the
 * stacked icon layout to the full-bleed image layout via the CSS
 * `.card:has(.card-icon)` selector ceasing to match.
 */
function _dirPreviewReplace(el, blobUrl) {
    const probe = new Image();
    probe.onload = () => {
        if (!el.isConnected) return;
        const n = Math.max(1, Math.round(probe.naturalWidth / 240));
        const sprite = document.createElement('div');
        sprite.className = 'card-dir-sprite';
        sprite.style.backgroundImage = `url(${JSON.stringify(blobUrl)})`;
        const card = el.closest('.card');
        el.replaceWith(sprite);
        if (n > 1 && card) {
            let timer = null;
            let frame = 0;
            const showFrame = f => {
                // Each sprite frame is square, so scaled-frame-width === container height.
                const h = sprite.offsetHeight || card.offsetHeight || 140;
                sprite.style.backgroundPosition = `-${f * h}px 0`;
            };
            card.addEventListener('mouseenter', () => {
                frame = 0;
                showFrame(0);
                timer = setInterval(() => {
                    frame = (frame + 1) % n;
                    showFrame(frame);
                }, 700);
            }, { passive: true });
            card.addEventListener('mouseleave', () => {
                clearInterval(timer);
                timer = null;
                showFrame(0);
            });
        }
    };
    probe.src = blobUrl;
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
        // null = permanent failure (e.g. 422): skip without fetching again.
        if (_thumbCache.has(src)) {
            const cached = _thumbCache.get(src);
            if (cached) _thumbReplace(el, cached);
            continue;
        }
        try {
            const resp = await fetch(src);
            if (!el.isConnected) continue;
            if (resp.ok && resp.status !== 204) {
                const blob = await resp.blob();
                const url = URL.createObjectURL(blob);
                _thumbCache.set(src, url);
                if (el.isConnected) _thumbReplace(el, url);
            } else if (resp.status === 503) {
                // Server busy: re-queue at back.
                await new Promise(resolve => setTimeout(resolve, 250));
                if (el.isConnected) {
                    _thumbQueue.push(el);
                    _thumbObserver.observe(el);
                }
            } else if (resp.status === 204) {
                // No thumbnail available for this URL. Cache that result for
                // the current page session so unsupported files are not refetched.
                // Directory contents can change while the app is open, so folder
                // misses stay retryable on the next render.
                if (!src.includes('/api/dir-thumbs?')) _thumbCache.set(src, null);
                if (el.isConnected) _thumbShowFailed(el);
            } else {
                // Other failures can be transient (tool not ready, stale cache,
                // corrupt sampled candidate). Show the placeholder for this DOM
                // node but do not cache failure globally; future renders may retry.
                if (el.isConnected) _thumbShowFailed(el);
            }
        } catch (_) {
            // Network error: show placeholder so shimmer does not run forever.
            if (el.isConnected) _thumbShowFailed(el);
        }
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
        const bulkSubjects = aggregateBulkSubjects();
        const chipsHtml = renderBulkTagChips(bulkTags, count);
        const paths = [...state.selectedPaths];
        const hasAiTagsBulk = bulkTags.some(t => t.tagStr.startsWith('ai/'));
        const pathsJson = esc(JSON.stringify(paths));
        const aiAcceptBulkBtn = hasAiTagsBulk
            ? `<button class="ai-clear-btn" onclick="aiAcceptAllTags(${pathsJson})">${esc(t('ai.accept-all'))}</button>`
            : '';
        const aiClearBulkBtn = hasAiTagsBulk
            ? `<button class="ai-clear-btn" onclick="aiClearTags(${pathsJson})">${esc(t('ai.clear-tags-bulk'))}</button>`
            : '';
        const hasAnalysable = paths.some(p => isAiImage(p));
        const aiBulkSection = hasAnalysable ? `
            <div class="bulk-ai-section">
                <p class="bulk-section-label">${esc(t('ai.analysis-label'))}</p>
                <div class="bulk-ai-row">
                    <button class="ai-analyse-btn bulk-ai-btn" onclick="aiAnalyseSelected()" title="${esc(t('ai.analyse-per-file'))}">${esc(t('ai.analyse-per-file'))}</button>
                    <button class="ai-analyse-btn bulk-ai-btn" onclick="aiAnalyseCommonTraits()" title="${esc(t('ai.analyse-common'))}">${esc(t('ai.analyse-common'))}</button>
                </div>
                <small class="ai-analyse-note">${esc(t('ai.bulk-note'))}</small>
            </div>` : '';
        panel.innerHTML = `
            <div class="detail-header">
                <h3>${t('bulk.n-selected', {n: count})}</h3>
                <div class="detail-header-actions">
                    <button class="detail-tag-picker-btn${state.tagPickerMode ? ' active' : ''}" id="tag-picker-toggle" onclick="enterTagPickerMode()" title="Apply multiple tags to selection">&#x2714; Tag files</button>
                    <button class="detail-chat-btn" onclick="openChat()" title="Chat about selected files">
                        <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M14 10.5A1.5 1.5 0 0 1 12.5 12H4l-3 3V3.5A1.5 1.5 0 0 1 2.5 2h10A1.5 1.5 0 0 1 14 3.5z"/></svg>
                        Chat
                    </button>
                </div>
            </div>
            <div class="bulk-tag-section">
                ${bulkSubjects.length > 0 ? `<p class="bulk-section-label">${esc(t('bulk.subjects-label'))}</p>
                <div class="bulk-subject-chips">${bulkSubjects.map(({ name, count: sc }) => {
                    const badge = sc < count ? ` <span class="bulk-chip-count">${sc}/${count}</span>` : '';
                    return `<span class="subject-label bulk-subject-chip">${esc(name)}${badge}</span>`;
                }).join('')}</div>` : ''}
                ${bulkTags.length > 0 ? `<p class="bulk-section-label"${bulkSubjects.length > 0 ? ' style="margin-top:10px"' : ''}>${esc(t('bulk.tags-label'))}</p>
                <div class="bulk-tag-chips" id="bulk-tag-chips">${chipsHtml}</div>` : ''}
                <p class="bulk-section-label" style="margin-top:12px">${esc(t('bulk.add-label'))}</p>
                <div class="tag-add-form">
                    <div class="tag-input-wrap">
                        <input type="text" id="bulk-tag-input" placeholder="${esc(t('bulk.tag-input'))}">
                    </div>
                    <div class="tag-input-wrap">
                        <input type="text" id="bulk-tag-subject" class="tag-subject-input" placeholder="${esc(t('detail.subject-placeholder'))}">
                    </div>
                    <button onclick="doBulkAddTag()">${esc(t('bulk.add-btn'))}</button>
                </div>
                ${aiAcceptBulkBtn}
                ${aiClearBulkBtn}
                ${aiBulkSection}
                <div id="bulk-status" class="bulk-status"></div>
            </div>`;
        attachTagAutocomplete(document.getElementById('bulk-tag-input'), () => doBulkAddTag());
        attachSubjectAutocomplete(document.getElementById('bulk-tag-subject'), collectBulkSubjects);
        return;
    }

    if (!state.selectedFile && !state.selectedDir && state.selectedRoot == null) {
        panel.innerHTML = `<div class="detail-empty">${esc(t('detail.empty'))}</div>`;
        return;
    }

    // Root card selected
    if (state.selectedRoot != null) {
        const rootMeta = state.roots.find(r => r.path === state.selectedRoot);
        const info = state.selectedRootInfo;
        const name = rootMeta ? rootMeta.name : state.selectedRoot.split('/').pop();
        const path = state.selectedRoot;
        const infoRows = info ? `
            <div class="detail-meta-row"><span class="detail-meta-label">${t('detail.files')}</span><span class="detail-meta-value">${info.files.toLocaleString()}</span></div>
            <div class="detail-meta-row"><span class="detail-meta-label">${t('detail.tags')}</span><span class="detail-meta-value">${info.tags.toLocaleString()}</span></div>
            <div class="detail-meta-row"><span class="detail-meta-label">${t('detail.assignments')}</span><span class="detail-meta-value">${info.assignments.toLocaleString()}</span></div>
            <div class="detail-meta-row"><span class="detail-meta-label">${t('detail.total-size')}</span><span class="detail-meta-value">${formatSize(info.total_size)}</span></div>` : `<div class="detail-meta-row">${esc(t('detail.loading'))}</div>`;
        panel.innerHTML = `
            <div class="detail-header">
                <h3>${esc(name)}</h3>
            </div>
            <div class="detail-preview">
                <div class="no-preview" style="color:var(--primary)">${ICONS.root}</div>
            </div>
            <div class="detail-meta">
                <div class="detail-meta-row"><span class="detail-meta-label">${t('detail.path')}</span><span class="detail-meta-value" style="word-break:break-all">${esc(path)}</span></div>
                ${infoRows}
            </div>
            <div style="padding:8px 12px">
                <button class="tag-action-btn" onclick="enterRoot('${jesc(state.selectedRoot)}')">${esc(t('detail.open-db'))}</button>
            </div>`;
        return;
    }

    // Directory selected
    if (state.selectedDir) {
        const d = state.selectedDir;
        let tagsHtml;
        let tagInputHtml = '';
        if (d.tags === null) {
            tagsHtml = `<span class="no-tags">${esc(t('detail.loading'))}</span>`;
        } else {
            tagsHtml = d.tags.length === 0
                ? `<span class="no-tags">${esc(t('detail.no-tags'))}</span>`
                : d.tags.map(tag => {
                    const tagStr = formatTag(tag);
                    const stateTag = state.tags.find(st => st.name === tag.name);
                    const chipColor = stateTag?.color ? ` style="border-left: 3px solid ${stateTag.color}"` : '';
                    return `<span class="tag-chip"${chipColor}>${esc(tagStr)}<button class="remove" onclick="removeTagFromDir('${jesc(d.path)}','${jesc(tagStr)}')">&times;</button></span>`;
                }).join('');
            tagInputHtml = `<div class="tag-add-form">
                <input type="text" id="dir-tag-input" placeholder="${esc(t('detail.tag-add'))}">
                <button onclick="doDirAddTag()">${esc(t('detail.tag-add-btn'))}</button>
            </div>`;
        }
        panel.innerHTML = `
            <div class="detail-top">
            <div class="detail-header">
                <h3>${esc(d.name)}</h3>
            </div>
            <div class="detail-preview">
                <div class="no-preview" style="color:#fab005">${ICONS.folder}</div>
            </div>
            <div class="detail-meta">
                <div class="detail-meta-row"><span class="detail-meta-label">${t('detail.path')}</span><span class="detail-meta-value">${esc(d.path)}</span></div>
                <div class="detail-meta-row"><span class="detail-meta-label">${t('detail.items')}</span><span class="detail-meta-value">${d.file_count}</span></div>
            </div>
            </div>
            <div class="detail-v-handle" id="detail-v-handle"></div>
            <div class="detail-tags-section">
                <h4>${t('detail.tags')}</h4>
                <div class="detail-tags">${tagsHtml}</div>
                ${tagInputHtml}
            </div>`;
        if (d.tags !== null) {
            const inp = document.getElementById('dir-tag-input');
            if (inp) attachTagAutocomplete(inp, () => doDirAddTag());
            initDetailVHandle(document.getElementById('detail-v-handle'));
        }
        return;
    }

    const f = state.selectedFile;
    const zipEntry = parseZipEntryPath(f.path);
    const name = zipEntry ? (zipEntry.entryName.split('/').pop() || zipEntry.entryName) : f.path.split('/').pop();
    const type_ = zipEntry ? fileType(zipEntry.entryName) : fileType(name);
    const previewUrl = '/preview/' + encodePath(f.path) + dirParam('?');

    let preview;
    if (zipEntry) {
        // Entry inside a zip archive
        const entry = state.zipEntries.find(e => e.name === zipEntry.entryName);
        if (entry && entry.is_image && entry.image_index !== null) {
            const thumbUrl = '/api/zip/thumb?' + new URLSearchParams({ path: zipEntry.zipPath, page: entry.image_index }) + dirParam('&');
            preview = `<a class="preview-zoomable" onclick="openMediaViewer('${jesc(zipEntry.zipPath)}', ${entry.image_index})" title="Click to open in viewer">` +
                      `<img src="${thumbUrl}" alt="${esc(name)}" onerror="_cardThumbError(this)"></a>`;
        } else {
            preview = `<div class="no-preview">${fileIcon(name)}</div>`;
        }
    } else if (type_ === 'image') {
        preview = `<img class="detail-img-zoomable" src="${previewUrl}" alt="${esc(name)}" data-name="${esc(name)}" onerror="_previewImgError(this)">`;
    } else if (type_ === 'raw') {
        preview = `<a class="preview-zoomable" onclick="if(!state.selectedPaths.has('${jesc(f.path)}'))selectFile('${jesc(f.path)}',event);openFileInDirViewer('${jesc(f.path)}')" title="Click to open in viewer">` +
                  `<img src="${previewUrl}" alt="${esc(name)}" data-name="${esc(name)}"` +
                  ` onerror="_previewRawError(this)"></a>`;
    } else if (type_ === 'audio') {
        preview = `<audio controls preload="metadata" src="${previewUrl}" ondblclick="openLightbox('${jesc(f.path)}','audio')" onclick="if(!state.selectedPaths.has('${jesc(f.path)}'))selectFile('${jesc(f.path)}',event);"></audio>`;
    } else if (type_ === 'video') {
        preview = `<video controls preload="metadata" src="${previewUrl}" data-name="${esc(name)}"` +
                  ` onerror="_previewVideoError(this)" onclick="if(!state.selectedPaths.has('${jesc(f.path)}'))selectFile('${jesc(f.path)}',event);"></video>`;
    } else if (type_ === 'pdf') {
        preview = `<iframe class="preview-pdf" src="${previewUrl}" title="${esc(name)}" onclick="if(!state.selectedPaths.has('${jesc(f.path)}'))selectFile('${jesc(f.path)}',event);"></iframe>` +
                  `<div style="text-align:center;padding:4px 0"><button class="tag-action-btn" onclick="openLightbox('${jesc(f.path)}','pdf')">Full-size PDF</button></div>`;
    } else if (type_ === 'markdown') {
        preview = `<div class="preview-markdown" id="preview-md-content" ondblclick="openLightbox('${jesc(f.path)}','markdown')" onclick="if(!state.selectedPaths.has('${jesc(f.path)}'))selectFile('${jesc(f.path)}',event);"` +
                  ` title="Double-click to enlarge">Loading…</div>`;
    } else if (type_ === 'text') {
        preview = `<pre class="preview-text" id="preview-text-content" ondblclick="openLightbox('${jesc(f.path)}','text')" onclick="if(!state.selectedPaths.has('${jesc(f.path)}'))selectFile('${jesc(f.path)}',event);"` +
                  ` title="Double-click to enlarge">Loading…</pre>`;
    } else if (type_ === 'zip') {
        preview = `<div class="zip-cover-wrap" onclick="if(!state.selectedPaths.has('${jesc(f.path)}'))selectFile('${jesc(f.path)}',event);">
            <img src="/thumb/${encodePath(f.path)}${dirParam('?')}" alt="${esc(name)}" class="zip-cover"
                 onerror="this.style.display='none'">
            <button class="tag-action-btn" onclick="openMediaViewer('${jesc(f.path)}')">Open in viewer</button>
        </div>`;
    } else {
        preview = `<div class="no-preview">${fileIcon(name)}</div>`;
    }

    const covered = f.covered !== false;

    const hasAiTags = covered && f.tags.some(tag => tag.name.startsWith('ai/'));
    const tagChips = renderFileTagChips(f, covered);

    const tagAddSection = covered
        ? `<div class="tag-add-form">
                <div class="tag-input-wrap">
                    <input type="text" id="tag-input" placeholder="${esc(t('detail.tag-add'))}">
                </div>
                <div class="tag-input-wrap">
                    <input type="text" id="tag-subject" class="tag-subject-input" placeholder="${esc(t('detail.subject-placeholder'))}">
                </div>
                <button onclick="doAddTag()">${esc(t('detail.tag-add-btn'))}</button>
            </div>`
        : `<div class="uncovered-notice">${esc(t('detail.uncovered'))}</div>`;

    const isAnalysable = covered && (type_ === 'image' || type_ === 'raw' || type_ === 'zip' || type_ === 'video');
    const isAnalysing = state.aiAnalysing.has(f.path);
    const aiAcceptBtn = hasAiTags
        ? `<button class="ai-clear-btn" onclick="aiAcceptAllTags(['${jesc(f.path)}'])">${esc(t('ai.accept-all'))}</button>`
        : '';
    const aiClearBtn = hasAiTags
        ? `<button class="ai-clear-btn" onclick="aiClearTags(['${jesc(f.path)}'])">${esc(t('ai.clear-tags'))}</button>`
        : '';
    const aiBtn = isAnalysable || hasAiTags
        ? `<div class="ai-analyse-row">
            ${isAnalysable ? `
            <div class="ai-analyse-controls">
                <button class="ai-analyse-btn" id="ai-analyse-single-btn" onclick="aiAnalyseSingle('${jesc(f.path)}')" ${isAnalysing ? 'disabled' : ''}>${isAnalysing ? esc(t('ai.analysing')) : esc(t('ai.analyse-btn'))}</button>
                ${type_ === 'video' ? `<div class="ai-frames-row">
                    <label class="ai-frames-label" title="${esc(t('ai.frames-auto-title'))}"><input type="checkbox" id="ai-frames-auto" ${state.aiVideoFramesAuto ? 'checked' : ''} onchange="aiSetVideoFramesAuto(this.checked)"><span>${esc(t('ai.frames-auto-label'))}</span></label>
                    <label class="ai-frames-label" title="${esc(t('ai.frames-title'))}"><input type="number" id="ai-frames-input" class="ai-frames-input" value="${state.aiVideoFrames}" min="2" max="256" step="1" onchange="aiSetVideoFrames(this.value)" ${state.aiVideoFramesAuto ? 'disabled' : ''}><span>${esc(t('ai.frames-label'))}</span></label>
                </div>` : ''}
            </div>
            <small class="ai-analyse-note">${type_ === 'video' ? esc(t('ai.video-note')) : ''}</small>` : ''}
            ${(aiAcceptBtn || aiClearBtn) ? `<div class="ai-action-row">${aiAcceptBtn}${aiClearBtn}</div>` : ''}
           </div>`
        : '';

    panel.innerHTML = `
        <div class="detail-top">
        <div class="detail-header">
            <h3>${esc(name)}</h3>
            <div class="detail-header-actions">
                <button class="detail-tag-picker-btn${state.tagPickerMode ? ' active' : ''}" id="tag-picker-toggle" onclick="enterTagPickerMode()" title="Apply multiple tags to this file">&#x2714; Tag file</button>
                <button class="detail-chat-btn" onclick="openChat()" title="Chat about this file">
                    <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M14 10.5A1.5 1.5 0 0 1 12.5 12H4l-3 3V3.5A1.5 1.5 0 0 1 2.5 2h10A1.5 1.5 0 0 1 14 3.5z"/></svg>
                    Chat
                </button>
            </div>
        </div>
        <div class="detail-preview">
            <button class="meta-info-btn" title="Toon info" onclick="toggleMetaOverlay(event)">i</button>
            ${preview}
            <div class="meta-overlay" id="meta-overlay" style="display:none;">
                <div class="meta-overlay-content">
                    <button class="meta-overlay-close" onclick="toggleMetaOverlay(event)">&times;</button>
                    ${zipEntry
                        ? `<div class=\"detail-meta-row\"><span class=\"detail-meta-label\">${esc(t('detail.archive'))}</span><span class=\"detail-meta-value\">${esc(zipEntry.zipPath.split('/').pop())}</span></div>
                           <div class=\"detail-meta-row\"><span class=\"detail-meta-label\">${esc(t('detail.entry'))}</span><span class=\"detail-meta-value\">${esc(zipEntry.entryName)}</span></div>`
                        : `<div class=\"detail-meta-row\"><span class=\"detail-meta-label\">${esc(t('detail.path'))}</span><span class=\"detail-meta-value\">${esc(f.path)}</span></div>
                           <div class=\"detail-meta-row\"><span class=\"detail-meta-label\">${esc(t('detail.size'))}</span><span class=\"detail-meta-value\">${formatSize(f.size)}</span></div>
                           ${f.indexed_at ? `<div class=\"detail-meta-row\"><span class=\"detail-meta-label\">${esc(t('detail.indexed'))}</span><span class=\"detail-meta-value\">${esc(f.indexed_at)}</span></div>` : ''}`
                    }
                </div>
            </div>
        </div>

        ${(type_ === 'image' || type_ === 'raw') ? '<div id="face-toolbar-row"></div>' : ''}
        </div>
        <div class="detail-v-handle" id="detail-v-handle"></div>
        <div class="detail-tags-section">
            <h4>${t('detail.tags')}</h4>
            <div class="detail-tags">${tagChips}</div>
            ${tagAddSection}
            ${aiBtn}
        </div>`;
    if (covered) {
        attachTagAutocomplete(document.getElementById('tag-input'), () => doAddTag());
        attachSubjectAutocomplete(document.getElementById('tag-subject'), collectSingleFileSubjects);
    }
    initDetailVHandle(document.getElementById('detail-v-handle'));

    // Face detection overlay (images only)
    if (typeof faceOnDetailRendered === 'function' && (type_ === 'image' || type_ === 'raw')) {
        faceOnDetailRendered(f.path, type_);
    }

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
                if (el) el.textContent = t('detail.preview-error');
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
                if (el) el.textContent = t('detail.preview-error');
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
    const covered = f.covered !== false;
    tagsEl.innerHTML = renderFileTagChips(f, covered);
}

// Render tag chips for a file, grouped by subject.
function renderFileTagChips(f, covered) {
    const visibleTags = f.tags.filter(tag => !(tag.subject && tag.name === tag.subject));
    if (visibleTags.length === 0 && !f.tags.some(t => t.implicit))
        return `<span class="no-tags">${esc(t('detail.no-tags'))}</span>`;

    // Track which subjects are explicitly linked (via the hidden linkage tag).
    const linkedSubjects = new Set(
        f.tags.filter(t => t.subject && t.name === t.subject).map(t => t.subject)
    );

    // Separate tags into subject groups (empty string = no subject).
    // Skip the helper linkage tag (same name as the subject).
    const groups = new Map(); // subject -> tag[]
    for (const tag of f.tags) {
        if (tag.subject && tag.name === tag.subject) continue; // hidden linkage tag
        const subj = tag.subject || '';
        if (!groups.has(subj)) groups.set(subj, []);
        groups.get(subj).push(tag);
    }

    function chipHtml(tag) {
        const tagStr = formatTag(tag);
        const stateTag = state.tags.find(st => st.name === tag.name);
        const chipColor = stateTag?.color ? ` style="border-left: 3px solid ${stateTag.color}"` : '';
        if (tag.implicit) {
            // Implicit (subject-entity) tags: read-only, visually distinct.
            return `<span class="tag-chip tag-chip--implicit"${chipColor} title="Subject tag (edit in Subjects manager)">${esc(tagStr)}</span>`;
        }
        if (!covered) {
            return `<span class="tag-chip tag-chip--readonly"${chipColor}>${esc(tagStr)}</span>`;
        }
        const subjArg = tag.subject ? `'${jesc(tag.subject)}'` : 'null';
        // Haal subject dynamisch uit het formulier bij promote
        const promoteBtn = tag.name.startsWith('ai/')
            ? `<button class="promote" title="${esc(t('detail.promote-title'))}" onclick="(function(){
                const subjInput = document.getElementById('tag-subject');
                const subj = subjInput && subjInput.value.trim() ? subjInput.value.trim() : (tag.subject || null);
                aiPromoteTag('${jesc(f.path)}','${jesc(tag.name)}','${jesc(tag.value || '')}', subj);
            })()">&uarr;</button>`
            : '';
        return `<span class="tag-chip"${chipColor}>${esc(tagStr)}${promoteBtn}<button class="remove" onclick="doRemoveTag('${jesc(f.path)}','${jesc(tagStr)}',${subjArg})">&times;</button></span>`;
    }

    let html = '';
    // Render no-subject tags first.
    const noSubj = groups.get('');
    if (noSubj) html += noSubj.map(chipHtml).join('');

    // Render subject groups.
    for (const [subj, tags] of groups) {
        if (subj === '') continue;
        const explicitTags = tags.filter(t => !t.implicit);
        const implicitTags = tags.filter(t => t.implicit);
        const hasExplicit = explicitTags.length > 0;
        const hasImplicit = implicitTags.length > 0;
        if (!hasExplicit && !hasImplicit) continue;
        html += `<div class="subject-group">`;
        html += `<span class="subject-label" title="Click to fill subject field" onclick="toggleSubjectInput('${jesc(subj)}')">${esc(subj)}</span>`;
        html += explicitTags.map(chipHtml).join('');
        if (hasImplicit) {
            html += `<span class="subject-implicit-sep" title="Subject tags (read-only)"></span>`;
            html += implicitTags.map(chipHtml).join('');
        }
        if (covered && (hasExplicit || linkedSubjects.has(subj))) {
            html += `<button class="subject-remove" title="${esc(t('detail.subject-remove-title'))}" onclick="doRemoveSubject('${jesc(f.path)}','${jesc(subj)}')">&times;</button>`;
        }
        html += `</div>`;
    }
    return html;
}

// ---------------------------------------------------------------------------
// Bulk tag helpers (multi-select)
// ---------------------------------------------------------------------------

function aggregateBulkTags() {
    const counts = new Map(); // tagStr → count of selected files that have it
    for (const [path, data] of state.selectedFilesData) {
        if (!state.selectedPaths.has(path)) continue;
        for (const t of (data.tags || [])) {
            // Skip linkage tags (subject name == tag name); they are shown in the subjects section.
            if (t.subject && t.name === t.subject) continue;
            const str = formatTag(t);
            counts.set(str, (counts.get(str) || 0) + 1);
        }
    }
    return [...counts.entries()]
        .map(([tagStr, count]) => ({ tagStr, count }))
        .sort((a, b) => b.count - a.count || a.tagStr.localeCompare(b.tagStr));
}

function aggregateBulkSubjects() {
    const counts = new Map(); // subject name → count of selected files that have it
    for (const [path, data] of state.selectedFilesData) {
        if (!state.selectedPaths.has(path)) continue;
        for (const tag of (data.tags || [])) {
            if (tag.subject && tag.name === tag.subject) {
                counts.set(tag.name, (counts.get(tag.name) || 0) + 1);
            }
        }
    }
    return [...counts.entries()]
        .map(([name, count]) => ({ name, count }))
        .sort((a, b) => b.count - a.count || a.name.localeCompare(b.name));
}

function renderBulkTagChips(bulkTags, total) {
    if (bulkTags.length === 0) return '';
    return bulkTags.map(({ tagStr, count }) => {
        const stateTag = state.tags.find(st => st.name === tagStr || st.name === tagStr.split('=')[0]);
        const chipBorder = stateTag?.color ? ` style="border-left: 3px solid ${stateTag.color}"` : '';
        const isPartial = count < total;
        const countBadge = isPartial
            ? `<span class="bulk-chip-count">${count}/${total}</span>`
            : '';
        const applyBtn = isPartial
            ? `<button class="bulk-chip-apply" onclick="doBulkApplyTagToAll('${jesc(tagStr)}')" title="${esc(t('bulk.apply-title', {n: total}))}">+</button>`
            : '';
        const isArmed = _armedBulkTag === tagStr;
        const hoverIn  = `bulkChipHoverEnter('${jesc(tagStr)}')`;
        const hoverOut = `bulkChipHoverLeave()`;
        if (isArmed) {
            return `<span class="bulk-chip armed"${chipBorder} onmouseenter="${hoverIn}" onmouseleave="${hoverOut}">
                <span class="bulk-chip-label">${esc(tagStr)}${countBadge}</span>
                <button class="bulk-chip-cancel" onclick="armBulkTag('${jesc(tagStr)}')" title="${esc(t('bulk.cancel'))}">&#8617;</button>
                <button class="bulk-chip-fire" onclick="doBulkRemoveTagChip('${jesc(tagStr)}')">${esc(t('bulk.remove'))}</button>
            </span>`;
        }
        return `<span class="bulk-chip"${chipBorder} onmouseenter="${hoverIn}" onmouseleave="${hoverOut}">
            <span class="bulk-chip-label">${esc(tagStr)}${countBadge}</span>
            ${applyBtn}
            <button class="bulk-chip-arm" onclick="armBulkTag('${jesc(tagStr)}')" title="Remove from selection">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14H6L5 6"/><path d="M10 11v6M14 11v6"/><path d="M9 6V4h6v2"/></svg>
            </button>
        </span>`;
    }).join('');
}

// Highlight grid/list cards that have the given tag among the selected files.
function bulkChipHoverEnter(tagStr) {
    const hasPaths = new Set();
    for (const [path, data] of state.selectedFilesData) {
        if (state.selectedPaths.has(path) && (data.tags || []).some(t => formatTag(t) === tagStr)) {
            hasPaths.add(path);
        }
    }
    document.querySelectorAll('.card[data-path], .list-row[data-path]').forEach(el => {
        el.classList.toggle('bulk-tag-lit', hasPaths.has(el.dataset.path));
    });
}

function bulkChipHoverLeave() {
    document.querySelectorAll('.bulk-tag-lit').forEach(el => el.classList.remove('bulk-tag-lit'));
}

// Apply tagStr to every selected file that does not yet have it.
async function doBulkApplyTagToAll(tagStr) {
    const paths = [...state.selectedPaths].filter(p => {
        const data = state.selectedFilesData.get(p);
        return data && !(data.tags || []).some(t => formatTag(t) === tagStr);
    });
    if (!paths.length) return;
    await Promise.all(paths.map(p => apiPost('/api/tag', { path: p, tags: [tagStr], dir: currentAbsDir() })));
    // Update local cache
    const eqIdx = tagStr.indexOf('=');
    const tName  = eqIdx !== -1 ? tagStr.slice(0, eqIdx) : tagStr;
    const tValue = eqIdx !== -1 ? tagStr.slice(eqIdx + 1) : '';
    for (const p of paths) {
        const d = state.selectedFilesData.get(p);
        if (d) d.tags.push({ name: tName, value: tValue });
    }
    await loadTags();
    if (state.mode === 'browse') await loadFiles(state.currentPath);
    const status = document.getElementById('bulk-status');
    if (status) status.textContent = t('bulk.applied', {tag: tagStr, n: paths.length, plural: paths.length !== 1 ? t('bulk.applied-plural') : ''});
    const el = document.getElementById('bulk-tag-chips');
    if (el) el.innerHTML = renderBulkTagChips(aggregateBulkTags(), state.selectedPaths.size);
    renderTags();
    renderContent();
    _thumbInit();
    _dirThumbInit();
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
    await Promise.all(paths.map(p => apiPost('/api/untag', { path: p, tags: [tagStr], dir: currentAbsDir() })));
    // Update cache locally
    for (const p of paths) {
        const d = state.selectedFilesData.get(p);
        if (d) d.tags = d.tags.filter(t => formatTag(t) !== tagStr);
    }
    await loadTags();
    if (state.mode === 'browse') await loadFiles(state.currentPath);
    const status = document.getElementById('bulk-status');
    if (status) status.textContent = t('bulk.removed', {tag: tagStr, n: paths.length, plural: paths.length !== 1 ? t('bulk.removed-plural') : ''});
    const el = document.getElementById('bulk-tag-chips');
    if (el) el.innerHTML = renderBulkTagChips(aggregateBulkTags(), state.selectedPaths.size);
    renderTags();
    renderContent();
    _thumbInit();
    _dirThumbInit();
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
    _dirThumbInit();
    _kbRestoreFocus();
}
