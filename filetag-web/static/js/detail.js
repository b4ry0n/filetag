// Global helper for the image meta-info overlay in the detail panel.
window.toggleMetaOverlay = function(e) {
    e.stopPropagation();
    const overlay = document.getElementById('meta-overlay');
    if (!overlay) return;
    overlay.style.display = (overlay.style.display === 'none' || !overlay.style.display) ? 'flex' : 'none';
};

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

async function openZipDir(zipPath, dir) {
    _thumbClearCache();
    state.mode = 'zip';
    state.zipPath = zipPath;
    // Resolve the root that owns this zip so all subsequent requests (thumb,
    // tag, refresh) target the correct database, even when the zip lives on a
    // non-active root (e.g. face-results or tag-filter search results).
    state.zipDir = dir || searchDirForPath(zipPath);
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
    const data = await api('/api/zip/entries?' + new URLSearchParams({ path: zipPath, dir: state.zipDir }));
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
    const effectiveDir = state.zipDir || searchDirForPath(state.zipPath);
    const data = await api('/api/zip/entries?' + new URLSearchParams({ path: state.zipPath, dir: effectiveDir }));
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
            const zipThumbDir = state.zipDir || searchDirForPath(state.zipPath);
            const thumbUrl = '/api/zip/thumb?' + new URLSearchParams({ path: state.zipPath, page: entry.image_index, dir: zipThumbDir });
            preview = `<div class="card-icon" data-thumb-src="${thumbUrl}" data-name="${esc(displayName)}" data-thumb-hover="1">${fileIcon(displayName)}</div>`;
        } else {
            preview = `<div class="card-icon">${fileIcon(displayName)}</div>`;
        }

        const tagBadge = entry.tag_count > 0
            ? `<span class="card-tag-count">${entry.tag_count}</span>` : '';
        const checkmark = state.selectedPaths.size > 1 && state.selectedPaths.has(dbPath)
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

/** MIME type for a video extension — mirrors the Rust mime_for_ext mapping.
 *  Only includes formats that browsers typically cannot play natively. */
function _videoMimeForExt(ext) {
    const map = {
        wmv: 'video/x-ms-wmv', avi: 'video/x-msvideo', flv: 'video/x-flv',
        asf: 'video/x-ms-asf', mpg: 'video/mpeg', mpeg: 'video/mpeg',
        m2ts: 'video/mp2t', mts: 'video/mp2t', mkv: 'video/x-matroska',
    };
    return map[ext] || null;
}

/** Return true if the browser reports it can likely play this video extension. */
function _browserCanPlayVideoExt(ext) {
    const mime = _videoMimeForExt(ext);
    if (!mime) return true; // unknown → let browser try
    return document.createElement('video').canPlayType(mime) !== '';
}

/** Render the transcode UI immediately (no play attempt needed). */
function _transcodeImmediately(btn, transcodeUrl, name) {
    _startTranscode(btn.closest('.no-preview'), transcodeUrl, name);
}

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
    const previewUrl = video.dataset.previewUrl || '';
    const transcodeUrl = previewUrl.replace(/^\/preview\//, '/transcode/');
    const d = document.createElement('div');
    d.className = 'no-preview';
    d.innerHTML = fileIcon(n) + '<div class="preview-unavail-msg">Browser kan dit formaat niet direct afspelen.</div>';
    if (transcodeUrl && transcodeUrl !== previewUrl) {
        const btn = document.createElement('button');
        btn.className = 'transcode-btn';
        btn.textContent = 'Transcoderen voor afspelen';
        btn.addEventListener('click', () => _startTranscode(d, transcodeUrl, n));
        d.appendChild(btn);
    }
    video.replaceWith(d);
}

function _startTranscode(container, transcodeUrl, name) {
    container.innerHTML = fileIcon(name) +
        '<div class="preview-unavail-msg">Transcoderen\u2026 even geduld.</div>';
    const vid = document.createElement('video');
    vid.controls = true;
    vid.preload = 'metadata';
    vid.src = transcodeUrl;
    vid.dataset.name = name;
    vid.dataset.previewUrl = transcodeUrl;
    vid.onerror = () => {
        container.innerHTML = fileIcon(name) +
            '<div class="preview-unavail-msg">Transcodering mislukt.</div>';
        container.classList.add('no-preview');
    };
    container.replaceWith(vid);
}

// ---------------------------------------------------------------------------
// Video trickplay
// ---------------------------------------------------------------------------
// On hover over a video card, the single-image sprite sheet returned by
// /api/vthumbs is loaded once and cached.  Moving the cursor left-to-right
// shifts a CSS background-position over the sprite, showing different frames
// without any DOM or src changes (same technique as Jellyfin trickplay).

const _trickplayCache  = new Map(); // path → {src, n, natW, natH} | 'loading' | 'failed'
// Tracks per-path WebM-full readiness for the webm-seek tile mode.
// Values: 'pending' (generation triggered) | 'ready' (tile_full.webm exists).
const _webmFullStatus  = new Map();

/**
 * Convert viewport-relative (vpLeft, vpTop) coordinates to absolute
 * coordinates within the #content scroll container.  All overlay popups are
 * appended to #content so they move with the grid when the user scrolls.
 * Returns { sc: HTMLElement, left: number, top: number }.
 */
function _overlayAbsolute(vpLeft, vpTop) {
    const sc = document.getElementById('content');
    const r  = sc.getBoundingClientRect();
    return { sc, left: vpLeft - r.left + sc.scrollLeft, top: vpTop - r.top + sc.scrollTop };
}

// ---------------------------------------------------------------------------
// Autoplay vtile download pool — limits concurrent vtile video downloads.
// Without this, every video card fires its own /api/vtile request immediately
// after the thumbnail loads, causing dozens of parallel video streams.
// ---------------------------------------------------------------------------
const VTILE_AP_CONCURRENCY = 3;
let   _vtileApActive = 0;
const _vtileApQueue  = []; // { v: HTMLVideoElement, startFn: Function }

/** Request a vtile download slot; startFn is called when a slot is free. */
function _vtileApAcquire(v, startFn) {
    if (_vtileApActive < VTILE_AP_CONCURRENCY) {
        _vtileApActive++;
        startFn();
    } else {
        _vtileApQueue.push({ v, startFn });
    }
}

/** Release a vtile slot and start the next queued download if any. */
function _vtileApRelease() {
    while (_vtileApQueue.length > 0) {
        const { v: qv, startFn } = _vtileApQueue.shift();
        if (!qv.isConnected) continue; // card removed from DOM — skip
        startFn();                      // slot count unchanged (transferred)
        return;
    }
    _vtileApActive--;                   // queue empty or all entries disconnected
}

// ---------------------------------------------------------------------------
// Autoplay sprite fetch pool — limits concurrent /api/vthumbs requests.
// ---------------------------------------------------------------------------
const SPRITE_AP_CONCURRENCY = 6;
let   _spriteApActive = 0;
const _spriteApQueue  = []; // pending sprite fetch functions

/** Wrap a sprite fetch call with pool concurrency control. */
function _spriteApRun(fetchFn) {
    if (_spriteApActive < SPRITE_AP_CONCURRENCY) {
        _spriteApActive++;
        fetchFn();
    } else {
        _spriteApQueue.push(fetchFn);
    }
}

/** Call when a pooled sprite fetch completes (success or failure). */
function _spriteApDone() {
    if (_spriteApQueue.length > 0) {
        _spriteApQueue.shift()(); // count unchanged (transferred)
    } else {
        _spriteApActive--;
    }
}

/**
 * Fetch a sprite sheet URL, read the X-Sprite-N response header to get the
 * frame count, then load the image to obtain its natural dimensions.
 * On success, calls onEntry({src, n, natW, natH}).
 * On failure (missing header, network error, decode error), calls onFail().
 * The caller is responsible for setting _trickplayCache to 'loading' first.
 */
function _fetchSpriteEntry(src, onEntry, onFail) {
    fetch(src, { credentials: 'same-origin' })
        .then(resp => {
            const n = parseInt(resp.headers.get('X-Sprite-N') || '0', 10);
            if (n < 1) { onFail(); return; }
            return resp.blob().then(blob => {
                const blobUrl = URL.createObjectURL(blob);
                const img = new Image();
                img.onload = () => {
                    URL.revokeObjectURL(blobUrl);
                    onEntry({ src, n, natW: img.naturalWidth, natH: img.naturalHeight });
                };
                img.onerror = () => { URL.revokeObjectURL(blobUrl); onFail(); };
                img.src = blobUrl;
            });
        })
        .catch(onFail);
}

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

    // ---- Video-based tile preview branches (webm / webm-seek / autoplay) ----
    const tileMode = state.settings.tile_preview_mode ?? 'sprite';

    // Shared helper: compute popup geometry from a video element's natural
    // dimensions (or fall back to card dimensions if not yet loaded).
    // Returns { popupW, popupH, left, top } in pixels.
    function _videoPopupGeometry(v, cardRect) {
        const vw = v.videoWidth  || cardRect.width;
        const vh = v.videoHeight || cardRect.height;
        const ar = vw / vh;
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
        let left = cardRect.left + (cardRect.width  - popupW) / 2;
        let top  = cardRect.top  + (cardRect.height - popupH) / 2;
        left = Math.max(4, Math.min(left, window.innerWidth  - popupW - 4));
        top  = Math.max(4, Math.min(top,  window.innerHeight - popupH - 4));
        return { popupW, popupH, left, top };
    }

    // Shared helper: un-pin any video pinned to another card.
    function _unpinOtherCards() {
        document.querySelectorAll('.card-trickplay-pinned').forEach(el => {
            if (!wrap.contains(el)) {
                const v = el.tagName === 'VIDEO' ? el : el.querySelector('video');
                if (v) v.pause();
                el.remove();
            }
        });
    }

    // ---- mode: "webm" — looping clip, aspect-ratio-correct popup ----
    if (tileMode === 'webm') {
        let floatVideo = null;
        let pinnedVideo = null;

        function makeWebmVideo() {
            const v = document.createElement('video');
            v.src = '/api/vtile?' + new URLSearchParams({ path }) + dirParam('&');
            v.muted = true;
            v.loop = true;
            v.autoplay = true;
            v.playsInline = true;
            v.style.pointerEvents = 'none';
            return v;
        }

        function buildWebmOverlay() {
            if (floatVideo || pinnedVideo) return;
            if (!wrap.getBoundingClientRect().width) return;

            floatVideo = makeWebmVideo();
            floatVideo.className = 'card-trickplay-sprite';
            // Hide until we know the video's dimensions.
            Object.assign(floatVideo.style, {
                position: 'absolute', zIndex: '1000',
                display: 'none', objectFit: 'cover', borderRadius: '4px',
            });

            const positionOverlay = () => {
                if (!floatVideo) return;
                const freshRect = wrap.getBoundingClientRect();
                const g = _videoPopupGeometry(floatVideo, freshRect);
                const abs = _overlayAbsolute(g.left, g.top);
                Object.assign(floatVideo.style, {
                    width:   g.popupW + 'px', height: g.popupH + 'px',
                    left:    abs.left.toFixed(1) + 'px',
                    top:     abs.top.toFixed(1)  + 'px',
                    display: '',
                });
            };
            floatVideo.addEventListener('loadedmetadata', positionOverlay, { once: true });
            // Fallback if metadata stalls (e.g. server slow to respond).
            setTimeout(() => { if (floatVideo && floatVideo.style.display === 'none') positionOverlay(); }, 800);

            const sc = document.getElementById('content');
            sc.appendChild(floatVideo);
            floatVideo.play().catch(() => {});
        }

        function buildWebmInline() {
            if (pinnedVideo && pinnedVideo.isConnected) return;
            teardownWebmOverlay();
            pinnedVideo = makeWebmVideo();
            pinnedVideo.className = 'card-trickplay-pinned';
            Object.assign(pinnedVideo.style, {
                objectFit: 'cover', width: '100%', height: '100%',
                opacity: '0', transition: 'opacity 0.4s ease',
            });
            wrap.appendChild(pinnedVideo);
            pinnedVideo.play().catch(() => {});
            pinnedVideo.addEventListener('canplay', () => {
                if (pinnedVideo) pinnedVideo.style.opacity = '1';
            }, { once: true });
            // Retry if the tile is not yet cached.
            let _pr = 0;
            pinnedVideo.addEventListener('error', function _pRetry() {
                if (!pinnedVideo || _pr >= 8) { if (pinnedVideo) pinnedVideo.removeEventListener('error', _pRetry); return; }
                const d = Math.min(2000 * (_pr + 1), 10000); _pr++;
                setTimeout(() => { if (pinnedVideo) { pinnedVideo.load(); pinnedVideo.play().catch(() => {}); } }, d);
            });
        }

        function teardownWebmOverlay() {
            if (floatVideo) { floatVideo.pause(); floatVideo.remove(); floatVideo = null; }
        }

        function teardownWebmInline() {
            if (pinnedVideo) { pinnedVideo.pause(); pinnedVideo.remove(); pinnedVideo = null; }
        }

        card.addEventListener('mouseenter', () => {
            if (!pinnedVideo) buildWebmOverlay();
        }, { passive: true });

        card.addEventListener('mouseleave', () => {
            teardownWebmOverlay();
            teardownWebmInline();
        });

        card.addEventListener('click', e => {
            if (e.target.closest('button, a')) return;
            teardownWebmOverlay();
            _unpinOtherCards();
            if (pinnedVideo && pinnedVideo.isConnected) {
                teardownWebmInline();
            } else {
                pinnedVideo = null;
                buildWebmInline();
            }
        });

        return;
    }
    // ---- end mode: "webm" ----

    // ---- mode: "webm-seek" — hover seeks through a full-length VP8/WebM re-encode.
    //          While the WebM is being generated in the background, the sprite-sheet
    //          hover preview is shown as a fallback.  Once the job completes the card
    //          seamlessly switches to the seek behaviour without a page reload. ----
    if (tileMode === 'webm-seek') {
        // -- WebM seek state --
        let floatVideo = null;
        let pinnedVideo = null;
        let seekTimer  = null;

        // -- Sprite fallback state (used while WebM is not yet ready) --
        let spriteCacheEntry = null;
        let spriteEl         = null;

        // ---------- WebM seek helpers ----------

        function makeSeekVideo(inline) {
            const v = document.createElement('video');
            v.src = '/api/vtile-full?' + new URLSearchParams({ path }) + dirParam('&');
            v.muted = true;
            v.preload = 'auto';
            v.playsInline = true;
            v.style.pointerEvents = 'none';
            if (inline) v.style.display = 'none'; // shown after metadata
            return v;
        }

        function seekToFrac(v, frac) {
            if (!v || !v.duration) return;
            v.currentTime = Math.max(0, Math.min(1, frac)) * v.duration;
        }

        function buildSeekOverlay() {
            if (floatVideo || pinnedVideo) return;
            if (!wrap.getBoundingClientRect().width) return;

            floatVideo = makeSeekVideo(false);
            floatVideo.className = 'card-trickplay-sprite';
            Object.assign(floatVideo.style, {
                position: 'absolute', zIndex: '1000',
                display: 'none', objectFit: 'cover', borderRadius: '4px',
            });

            const positionOverlay = () => {
                if (!floatVideo) return;
                const freshRect = wrap.getBoundingClientRect();
                const g = _videoPopupGeometry(floatVideo, freshRect);
                const abs = _overlayAbsolute(g.left, g.top);
                Object.assign(floatVideo.style, {
                    width:   g.popupW + 'px', height: g.popupH + 'px',
                    left:    abs.left.toFixed(1) + 'px',
                    top:     abs.top.toFixed(1)  + 'px',
                    display: '',
                });
            };
            floatVideo.addEventListener('loadedmetadata', positionOverlay, { once: true });
            setTimeout(() => { if (floatVideo && floatVideo.style.display === 'none') positionOverlay(); }, 800);
            const sc = document.getElementById('content');
            sc.appendChild(floatVideo);
        }

        function buildSeekInline(clientX) {
            if (pinnedVideo && pinnedVideo.isConnected) return;
            teardownSeekOverlay();
            pinnedVideo = makeSeekVideo(true);
            pinnedVideo.className = 'card-trickplay-pinned';
            Object.assign(pinnedVideo.style, { objectFit: 'cover', width: '100%', height: '100%' });
            pinnedVideo.addEventListener('loadedmetadata', () => {
                pinnedVideo.style.display = '';
                if (clientX !== undefined) {
                    const rect = wrap.getBoundingClientRect();
                    seekToFrac(pinnedVideo, (clientX - rect.left) / rect.width);
                }
            }, { once: true });
            wrap.appendChild(pinnedVideo);
        }

        function teardownSeekOverlay() {
            clearTimeout(seekTimer); seekTimer = null;
            if (floatVideo) { floatVideo.pause(); floatVideo.remove(); floatVideo = null; }
        }

        function teardownSeekInline() {
            clearTimeout(seekTimer); seekTimer = null;
            if (pinnedVideo) { pinnedVideo.pause(); pinnedVideo.remove(); pinnedVideo = null; }
        }

        function onSeekMouseMove(e) {
            const rect = wrap.getBoundingClientRect();
            const frac = (e.clientX - rect.left) / rect.width;
            if (floatVideo && !floatVideo.paused) floatVideo.pause();
            if (pinnedVideo && !pinnedVideo.paused) pinnedVideo.pause();
            if (floatVideo)  seekToFrac(floatVideo,  frac);
            if (pinnedVideo) seekToFrac(pinnedVideo, frac);
            clearTimeout(seekTimer);
            seekTimer = setTimeout(() => {
                if (floatVideo)  floatVideo.play().catch(() => {});
                if (pinnedVideo) pinnedVideo.play().catch(() => {});
            }, 300);
        }

        // ---------- Sprite fallback helpers ----------

        function ensureSpriteFallback() {
            if (_trickplayCache.get(path) === 'loading' || _trickplayCache.get(path) === 'failed') return;
            const cached = _trickplayCache.get(path);
            if (cached && typeof cached === 'object') { spriteCacheEntry = cached; return; }
            _trickplayCache.set(path, 'loading');
            const src = '/api/vthumbs?' + new URLSearchParams({
                path,
                min_n: state.settings.sprite_min ?? 8,
                max_n: state.settings.sprite_max ?? 16,
            }) + dirParam('&');
            _fetchSpriteEntry(src,
                (entry) => {
                    _trickplayCache.set(path, entry);
                    spriteCacheEntry = entry;
                    if (card.matches(':hover') && _webmFullStatus.get(path) !== 'ready') {
                        buildSpriteOverlay();
                    }
                },
                () => {
                    _trickplayCache.set(path, 'failed');
                    setTimeout(() => {
                        if (_trickplayCache.get(path) === 'failed') _trickplayCache.delete(path);
                    }, 3000);
                }
            );
        }

        function buildSpriteOverlay() {
            if (spriteEl || !spriteCacheEntry) return;
            const cardRect = wrap.getBoundingClientRect();
            if (!cardRect.width) return;
            const frameW   = spriteCacheEntry.natW / spriteCacheEntry.n;
            const ar       = frameW / spriteCacheEntry.natH;
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
            let left = cardRect.left + (cardRect.width  - popupW) / 2;
            let top  = cardRect.top  + (cardRect.height - popupH) / 2;
            left = Math.max(4, Math.min(left, window.innerWidth  - popupW - 4));
            top  = Math.max(4, Math.min(top,  window.innerHeight - popupH - 4));
            const abs = _overlayAbsolute(left, top);
            spriteEl = document.createElement('div');
            spriteEl.className = 'card-trickplay-sprite';
            Object.assign(spriteEl.style, {
                position:        'absolute',
                zIndex:          '1000',
                pointerEvents:   'none',
                width:           popupW + 'px',
                height:          popupH + 'px',
                left:            abs.left.toFixed(1) + 'px',
                top:             abs.top.toFixed(1)  + 'px',
                backgroundImage: `url(${JSON.stringify(spriteCacheEntry.src)})`,
                backgroundRepeat: 'no-repeat',
            });
            abs.sc.appendChild(spriteEl);
            showSpriteFrame(0);
        }

        function showSpriteFrame(frac) {
            if (!spriteEl || !spriteCacheEntry) return;
            const idx    = Math.min(spriteCacheEntry.n - 1, Math.floor(Math.max(0, Math.min(0.9999, frac)) * spriteCacheEntry.n));
            const popupH = parseFloat(spriteEl.style.height);
            const popupW = parseFloat(spriteEl.style.width);
            const frameW = spriteCacheEntry.natW / spriteCacheEntry.n;
            const frameH = spriteCacheEntry.natH;
            const scale  = Math.max(popupW / frameW, popupH / frameH);
            const bsW    = Math.round(spriteCacheEntry.natW * scale);
            const bsH    = Math.round(spriteCacheEntry.natH * scale);
            const tileW  = frameW * scale;
            const tileH  = frameH * scale;
            const x      = popupW / 2 - tileW * (idx + 0.5);
            const y      = (popupH - tileH) / 2;
            spriteEl.style.backgroundSize     = `${bsW}px ${bsH}px`;
            spriteEl.style.backgroundPosition = `${x.toFixed(1)}px ${y.toFixed(1)}px`;
        }

        function teardownSpriteOverlay() {
            if (spriteEl) { spriteEl.remove(); spriteEl = null; }
        }

        // ---------- WebM readiness trigger ----------

        async function triggerWebmFull() {
            const status = _webmFullStatus.get(path);
            if (status === 'ready' || status === 'pending') return;
            _webmFullStatus.set(path, 'pending');
            try {
                const res = await apiPost('/api/vtile-full' + dirParam('?'), { path });
                if (res?.ready) {
                    _webmFullStatus.set(path, 'ready');
                    // Switch immediately if the user is still hovering.
                    if (card.matches(':hover') && !pinnedVideo) {
                        teardownSpriteOverlay();
                        buildSeekOverlay();
                    }
                } else if (res?.job_id) {
                    onJobSubmitted(res.job_id);
                    whenJobDone(res.job_id, () => {
                        _webmFullStatus.set(path, 'ready');
                        if (card.matches(':hover') && !pinnedVideo) {
                            teardownSpriteOverlay();
                            buildSeekOverlay();
                        }
                    });
                }
            } catch (err) {
                console.error('[webm-seek] vtile-full trigger failed:', err);
                // Allow retry on next hover.
                _webmFullStatus.delete(path);
            }
        }

        // ---------- Event listeners ----------

        card.addEventListener('mouseenter', () => {
            triggerWebmFull();
            if (_webmFullStatus.get(path) === 'ready') {
                if (!pinnedVideo) buildSeekOverlay();
            } else {
                ensureSpriteFallback();
                if (spriteCacheEntry) buildSpriteOverlay();
            }
        }, { passive: true });

        card.addEventListener('mouseleave', () => {
            teardownSeekOverlay();
            teardownSeekInline();
            teardownSpriteOverlay();
        });

        card.addEventListener('mousemove', e => {
            if (_webmFullStatus.get(path) === 'ready') {
                onSeekMouseMove(e);
            } else if (spriteEl) {
                const rect = wrap.getBoundingClientRect();
                showSpriteFrame((e.clientX - rect.left) / rect.width);
            }
        }, { passive: true });

        card.addEventListener('click', e => {
            if (e.target.closest('button, a')) return;
            if (_webmFullStatus.get(path) === 'ready') {
                teardownSeekOverlay();
                _unpinOtherCards();
                if (pinnedVideo && pinnedVideo.isConnected) {
                    teardownSeekInline();
                } else {
                    pinnedVideo = null;
                    buildSeekInline(e.clientX);
                }
            } else {
                // Sprite fallback visible — click dismisses it.
                teardownSpriteOverlay();
            }
        });

        return;
    }
    // ---- end mode: "webm-seek" ----

    // ---- mode: "autoplay" — video always playing inline; hover expands to AR popup ----
    // Priority order:
    //   1. Static thumbnail (already loaded before _trickplayAttach is called)
    //   2. Sprite sheet scrub-on-hover (loaded proactively, same behaviour as sprite mode)
    //   3. WebM tile playing inline + AR-correct popup on hover (once tile is ready)
    if (tileMode === 'autoplay') {
        // -- Phase 2: sprite sheet machinery (identical to sprite mode, no click-to-pin) --
        let _spriteEl  = null;
        let _spriteCE  = null; // cacheEntry for this card

        function _apEnsureSprite() {
            const cached = _trickplayCache.get(path);
            if (cached === 'loading' || cached === 'failed') return;
            if (cached) { _spriteCE = cached; return; }

            _trickplayCache.set(path, 'loading');
            const minN = state.settings.sprite_min ?? 8;
            const maxN = state.settings.sprite_max ?? 16;
            const src = '/api/vthumbs?' + new URLSearchParams({ path, min_n: minN, max_n: maxN })
                + dirParam('&');
            _spriteApRun(() => _fetchSpriteEntry(src,
                (entry) => {
                    _spriteApDone();
                    _trickplayCache.set(path, entry);
                    _spriteCE = entry;
                    // Build overlay immediately if the user is already hovering and
                    // the video is not yet ready.
                    if (card.matches(':hover') && !_videoReady) _apBuildOverlay();
                },
                () => {
                    _spriteApDone();
                    _trickplayCache.set(path, 'failed');
                    setTimeout(() => {
                        if (_trickplayCache.get(path) === 'failed') _trickplayCache.delete(path);
                    }, 3000);
                }
            ));
        }

        function _apBuildOverlay() {
            if (_spriteEl || !_spriteCE) return;
            const cardRect = wrap.getBoundingClientRect();
            if (!cardRect.width) return;

            const frameW = _spriteCE.natW / _spriteCE.n;
            const frameH = _spriteCE.natH;
            const ar = frameW / frameH;
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
            let left = cardRect.left + (cardRect.width  - popupW) / 2;
            let top  = cardRect.top  + (cardRect.height - popupH) / 2;
            left = Math.max(4, Math.min(left, window.innerWidth  - popupW - 4));
            top  = Math.max(4, Math.min(top,  window.innerHeight - popupH - 4));
            const abs = _overlayAbsolute(left, top);

            _spriteEl = document.createElement('div');
            _spriteEl.className = 'card-trickplay-sprite';
            Object.assign(_spriteEl.style, {
                position:        'absolute',
                zIndex:          '1000',
                pointerEvents:   'none',
                width:           popupW + 'px',
                height:          popupH + 'px',
                left:            abs.left.toFixed(1) + 'px',
                top:             abs.top.toFixed(1)  + 'px',
                backgroundImage: `url(${JSON.stringify(_spriteCE.src)})`,
                backgroundRepeat:'no-repeat',
                backgroundSize:  'auto 100%',
            });
            abs.sc.appendChild(_spriteEl);
            _apShowFrame(0);
        }

        function _apShowFrame(idx) {
            if (!_spriteEl || !_spriteCE) return;
            const popupH = parseFloat(_spriteEl.style.height);
            const popupW = parseFloat(_spriteEl.style.width);
            const frameW = _spriteCE.natW / _spriteCE.n;
            const frameH = _spriteCE.natH;
            const scale  = Math.max(popupW / frameW, popupH / frameH);
            const bsW    = Math.round(_spriteCE.natW * scale);
            const bsH    = Math.round(_spriteCE.natH * scale);
            const tileW  = frameW * scale;
            const tileH  = frameH * scale;
            const x      = popupW / 2 - tileW * (idx + 0.5);
            const y      = (popupH - tileH) / 2;
            _spriteEl.style.backgroundSize     = `${bsW}px ${bsH}px`;
            _spriteEl.style.backgroundPosition = `${x.toFixed(1)}px ${y.toFixed(1)}px`;
        }

        function _apTeardown() {
            if (_spriteEl) {
                _spriteEl.remove();
                _spriteEl = null;
            }
        }

        // -- Phase 3: WebM video element (background generation + polling) --
        const v = document.createElement('video');
        // v.src is set lazily via _vtileApAcquire to cap concurrent downloads.
        v.muted = true;
        v.loop = true;
        v.autoplay = true;
        v.playsInline = true;
        v.className = 'card-trickplay-pinned';
        // Start invisible — static thumbnail + sprite sheet show first.
        v.style.opacity    = '0';
        v.style.transition = 'opacity 0.4s ease';
        wrap.appendChild(v);

        let _videoReady = false;
        let _slotHeld   = false;

        function _releaseVtileSlot() {
            if (!_slotHeld) return;
            _slotHeld = false;
            _vtileApRelease();
        }

        v.addEventListener('canplay', () => {
            _videoReady   = true;
            _videoVisible = true;
            v.style.opacity = '1';
            // Remove sprite overlay; playing video replaces it.
            _apTeardown();
            _releaseVtileSlot(); // free pool slot for the next card
        }, { once: true });

        // Retry when the tile is not yet cached (backend returns 202 or 422).
        let _retries = 0;
        v.addEventListener('error', function _retry() {
            if (_retries >= 12 || !v.isConnected) {
                v.removeEventListener('error', _retry);
                _releaseVtileSlot(); // give up; free the pool slot
                return;
            }
            const delay = Math.min(2000 * (_retries + 1), 15000);
            _retries++;
            setTimeout(() => {
                if (!v.isConnected) { _releaseVtileSlot(); return; }
                v.load(); v.play().catch(() => {});
            }, delay);
        });

        // Acquire a pool slot; starts immediately if under the cap, otherwise queues.
        _vtileApAcquire(v, () => {
            _slotHeld = true;
            v.src = '/api/vtile?' + new URLSearchParams({ path }) + dirParam('&');
            v.play().catch(() => {});
        });

        // -- Event handlers: sprite behaviour while video not ready, video popup once ready --
        let _videoVisible = false; // toggled by click once video has loaded

        // Moves the in-card <video> element into #content as an AR-correct overlay.
        // Reusing the same node means the clip keeps playing seamlessly (no restart).
        function _apBuildVideoOverlay() {
            if (v.parentNode !== wrap) return; // already floating
            const cardRect = wrap.getBoundingClientRect();
            if (!cardRect.width) return;
            const g = _videoPopupGeometry(v, cardRect);
            const abs = _overlayAbsolute(g.left, g.top);
            Object.assign(v.style, {
                position:      'absolute',
                inset:         'auto',   // override .card-trickplay-pinned's inset:0
                zIndex:        '1000',
                objectFit:     'cover',
                borderRadius:  '4px',
                pointerEvents: 'none',
                width:         g.popupW + 'px',
                height:        g.popupH + 'px',
                left:          abs.left.toFixed(1) + 'px',
                top:           abs.top.toFixed(1)  + 'px',
                opacity:       '1',
                transition:    'none',
            });
            abs.sc.appendChild(v);
        }

        function _apTeardownVideoOverlay() {
            if (v.parentNode === wrap) return; // not floating
            Object.assign(v.style, {
                position:      '',
                inset:         '',
                zIndex:        '',
                objectFit:     '',
                borderRadius:  '',
                pointerEvents: '',
                width:         '',
                height:        '',
                left:          '',
                top:           '',
                opacity:       _videoVisible ? '1' : '0',
                transition:    'opacity 0.4s ease',
            });
            wrap.appendChild(v);
        }

        card.addEventListener('mouseenter', () => {
            if (_videoReady && _videoVisible) {
                _apBuildVideoOverlay();
            } else {
                _apEnsureSprite();
                if (_spriteCE) _apBuildOverlay();
            }
        }, { passive: true });

        card.addEventListener('mousemove', e => {
            if (_videoReady && _videoVisible) return;
            if (!_spriteCE) return;
            if (!_spriteEl) _apBuildOverlay();
            const rect = wrap.getBoundingClientRect();
            const frac = Math.max(0, Math.min(0.9999, (e.clientX - rect.left) / rect.width));
            _apShowFrame(Math.min(_spriteCE.n - 1, Math.floor(frac * _spriteCE.n)));
        }, { passive: true });

        card.addEventListener('mouseleave', () => {
            _apTeardownVideoOverlay();
            _apTeardown();
        });

        // Click: toggle video visibility.  When hidden, hover shows the sprite fallback
        // instead; clicking again while hidden restores the video.
        card.addEventListener('click', e => {
            if (e.target.closest('button, a')) return;
            _apTeardownVideoOverlay();
            _apTeardown();
            if (_videoReady) {
                _videoVisible = !_videoVisible;
                v.style.opacity = _videoVisible ? '1' : '0';
                if (_videoVisible) v.play().catch(() => {});
            }
        });

        // Proactively load the sprite sheet as soon as the thumbnail is ready.
        // Viewport priority is provided automatically: _trickplayAttach() is called
        // from _thumbReplace() which fires in IntersectionObserver-priority order,
        // so cards visible in the viewport have their sprites loaded first.
        _apEnsureSprite();

        // Queue vtile pregeneration for this card in viewport-entry order.
        // _queueVtilePregen (actions.js) batches requests with a short debounce
        // so that a screenful of cards submits one job instead of N individual jobs.
        if (typeof _queueVtilePregen === 'function') _queueVtilePregen(path);

        return;
    }
    // ---- end mode: "autoplay" ----

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
        // Use fetch() so we can read the X-Sprite-N response header to get the
        // exact frame count (frame width is no longer constant because we scale
        // by the shortest side rather than a fixed width).
        _fetchSpriteEntry(src,
            (entry) => {
                _trickplayCache.set(path, entry);
                cacheEntry = entry;
                if (card.matches(':hover') && !pinnedEl) buildOverlay();
                if (wantPin) buildInline();
            },
            () => {
                // Mark as failed; schedule a retry after 3 s so the next hover
                // can try again without hammering a busy server.
                _trickplayCache.set(path, 'failed');
                setTimeout(() => {
                    if (_trickplayCache.get(path) === 'failed') {
                        _trickplayCache.delete(path);
                    }
                }, 3000);
            }
        );
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

        const abs = _overlayAbsolute(left, top);
        spriteEl = document.createElement('div');
        spriteEl.className = 'card-trickplay-sprite';
        Object.assign(spriteEl.style, {
            position:        'absolute',
            zIndex:          '1000',
            pointerEvents:   'none',
            width:           popupW + 'px',
            height:          popupH + 'px',
            left:            abs.left.toFixed(1) + 'px',
            top:             abs.top.toFixed(1)  + 'px',
            backgroundImage: `url(${JSON.stringify(cacheEntry.src)})`,
            backgroundRepeat:'no-repeat',
            backgroundSize:  'auto 100%',
        });
        abs.sc.appendChild(spriteEl);
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

        const abs = _overlayAbsolute(left, top);
        popupEl = document.createElement('div');
        popupEl.className = 'card-thumb-popup';
        Object.assign(popupEl.style, {
            width:  popupW + 'px',
            height: popupH + 'px',
            left:   abs.left.toFixed(1) + 'px',
            top:    abs.top.toFixed(1)  + 'px',
        });
        const popupImg = document.createElement('img');
        popupImg.src = blobUrl;
        popupImg.alt = '';
        popupEl.appendChild(popupImg);
        abs.sc.appendChild(popupEl);
        // Register globally so keyboard shortcuts can dismiss the popup even
        // when the card DOM node is replaced (and mouseleave never fires).
        _activeThumbPopupTeardown = teardownPopup;
    }

    function teardownPopup() {
        if (popupEl) {
            popupEl.remove();
            popupEl = null;
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
// Thumbnail queue: serial loader + parallel dir-thumb pool
// ---------------------------------------------------------------------------
// Regular thumbnails (images, videos, PDFs, archives) are handled by a serial
// queue (_thumbQueue) — one at a time, respecting the server-side semaphore.
//
// Directory-preview thumbnails use a separate parallel pool (_dirThumbQueue /
// _dirThumbActive, cap DIR_THUMB_CONCURRENCY).  The server starts a background
// collage-generation task on first request and returns 202 Accepted
// immediately, so multiple requests can be in-flight without serialising.
//
// Ordering strategy:
//   1. IntersectionObserver (rootMargin 150 px) fires first for visible
//      directories → added to the front of _dirThumbQueue.
//   2. When a slot becomes free and the queue is empty,
//      _dirThumbEnqueueRemaining() collects all unqueued dir-thumb elements
//      and sorts them by distance from the viewport midpoint (nearest first),
//      so the page fills in concentric rings rather than random order.

const DIR_THUMB_CONCURRENCY = 6; // max parallel dir-thumb fetches / pollers
const THUMB_CONCURRENCY = 6;     // max parallel regular-thumb fetches

const _thumbQueue    = [];
let   _thumbBusy     = false;    // kept for back-compat; no longer used as a gate
const _thumbCache    = new Map();    // thumb URL → blob URL  (null = permanent miss)
const _thumbSalient  = new Map();    // blob URL → {cx, cy} — populated from X-Salient-* headers
const _thumbActive   = new Set();    // regular-thumb elements currently in flight

const _dirThumbQueue  = [];          // ordered: visible-first, then by proximity
const _dirThumbActive = new Set();   // dir-thumb elements currently being fetched
let   _dirThumbAbortCtrl = new AbortController(); // aborted on navigation to cancel in-flight fetches

const _thumbObserver = new IntersectionObserver((entries) => {
    const newlyRegular = [];
    for (const e of entries) {
        if (!e.isIntersecting) continue;
        const el = e.target;
        _thumbObserver.unobserve(el);
        const src = el.dataset.thumbSrc || '';
        if (src.includes('/api/dir-thumbs?')) {
            // Dir-thumb: promote to the front of the parallel pool queue.
            if (!_dirThumbActive.has(el) && !_dirThumbQueue.includes(el)) {
                _dirThumbQueue.unshift(el);
            }
        } else {
            // Regular thumb: add to the front of the queue (lazy-gate).
            // Items are not pre-queued, so indexOf will be -1; the else-if
            // branch handles the normal first-entry case.
            const i = _thumbQueue.indexOf(el);
            if (i !== -1) {
                _thumbQueue.splice(i, 1);
                newlyRegular.push(el);
            } else if (!_thumbActive.has(el) && src) {
                newlyRegular.push(el);
            }
        }
    }
    if (newlyRegular.length > 0) {
        newlyRegular.sort((a, b) => a.getBoundingClientRect().top - b.getBoundingClientRect().top);
        _thumbQueue.unshift(...newlyRegular);
    }
    _thumbRun();
    _dirThumbSchedule();
}, { rootMargin: '150px' });

function _thumbFlush() {
    for (let i = _thumbQueue.length - 1; i >= 0; i--) {
        if (!_thumbQueue[i].isConnected) _thumbQueue.splice(i, 1);
    }
    for (let i = _dirThumbQueue.length - 1; i >= 0; i--) {
        if (!_dirThumbQueue[i].isConnected) _dirThumbQueue.splice(i, 1);
    }
}

/** Re-sort both thumbnail queues by distance from the current viewport centre.
 *
 *  The IntersectionObserver promotes elements to the front of the queue when
 *  they first enter the viewport, then unobserves them.  After that, if the
 *  user scrolls away and back again, those elements are already somewhere in
 *  the queue but are no longer observed — so they won't be re-promoted
 *  automatically.  By re-sorting on scroll we ensure that whatever is visible
 *  right now is always processed first, regardless of scroll history.
 *
 *  getBoundingClientRect() is viewport-relative and does NOT force a full
 *  layout reflow when called inside a requestAnimationFrame callback.
 */
let _thumbViewportSortPending = false;
function _thumbViewportSort() {
    _thumbViewportSortPending = false;
    if (_thumbQueue.length < 2 && _dirThumbQueue.length < 2) return;
    const centre = window.innerHeight / 2;
    const dist = el => {
        if (!el.isConnected) return Infinity;
        const r = el.getBoundingClientRect();
        return Math.abs(r.top + r.height / 2 - centre);
    };
    if (_thumbQueue.length > 1) {
        _thumbQueue.sort((a, b) => dist(a) - dist(b));
    }
    if (_dirThumbQueue.length > 1) {
        _dirThumbQueue.sort((a, b) => dist(a) - dist(b));
    }
}
window.addEventListener('scroll', () => {
    if (_thumbViewportSortPending) return;
    _thumbViewportSortPending = true;
    requestAnimationFrame(_thumbViewportSort);
}, { passive: true });

// #content is the real scroll container (overflow-y:auto; body has overflow:hidden).
// Listen here too so viewport-priority sorting fires when scrolling the file grid.
document.getElementById('content')?.addEventListener('scroll', () => {
    if (_thumbViewportSortPending) return;
    _thumbViewportSortPending = true;
    requestAnimationFrame(_thumbViewportSort);
}, { passive: true });

function _thumbInit() {
    _thumbFlush();
    document.querySelectorAll('.card-icon[data-thumb-src]').forEach(el => {
        const src = el.dataset.thumbSrc;
        if (_thumbCache.has(src)) {
            const cached = _thumbCache.get(src);
            if (cached) _thumbReplace(el, cached); else _thumbShowFailed(el);
            return;
        }
        if (src.includes('/api/dir-thumbs?')) {
            // Dir-thumbs: only observe for now; the pool picks them up when
            // they become visible, or via _dirThumbEnqueueRemaining afterwards.
            if (!_dirThumbActive.has(el) && !_dirThumbQueue.includes(el)) {
                _thumbObserver.observe(el);
            }
        } else {
            if (_thumbQueue.includes(el) || _thumbActive.has(el)) return;
            // Only observe; the card is queued when it enters the viewport.
            _thumbObserver.observe(el);
        }
    });
    _thumbRun();
}

function _thumbReplace(el, blobUrl, revokeOnLoad = false) {
    // Directory preview: inline sprite cycling (no floating popup).
    if (el.dataset.dirPath) {
        _dirPreviewReplace(el, blobUrl);
        // Sprite BlobURL is transferred to a CSS background-image; revoke after
        // a short delay so the browser has time to decode it.
        if (revokeOnLoad) setTimeout(() => URL.revokeObjectURL(blobUrl), 5000);
        return;
    }
    const img = document.createElement('img');
    img.src = blobUrl;
    if (el.dataset.cls) img.className = el.dataset.cls;
    img.alt = '';
    img.dataset.name = el.dataset.name || '';
    // Crop position: use detected salient point when available; otherwise
    // apply North gravity for portrait images so heads stay in frame.
    const _salient = _thumbSalient.get(blobUrl);
    img.addEventListener('load', () => {
        if (_salient) {
            // Subtract 10 percentage points so the focal point sits lower in
            // the card, leaving breathing room above the detected head/body.
            const cy_display = Math.max(5, Math.min(90, _salient.cy * 100 - 10));
            img.style.objectPosition =
                `${(_salient.cx * 100).toFixed(1)}% ${cy_display.toFixed(1)}%`;
        } else if (img.naturalHeight > img.naturalWidth) {
            img.style.objectPosition = 'top';
        }
    }, { once: true });
    if (revokeOnLoad) {
        img.addEventListener('load',  () => URL.revokeObjectURL(blobUrl), { once: true });
        img.addEventListener('error', () => URL.revokeObjectURL(blobUrl), { once: true });
    }
    const _card = el.closest('.card');
    el.replaceWith(img);
    if (_card) _card.classList.add('has-thumb');

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
        if (card) card.classList.add('has-thumb');
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

/** Fill free slots in the regular-thumb parallel pool from _thumbQueue. */
function _thumbRun() {
    _thumbFlush();
    while (_thumbActive.size < THUMB_CONCURRENCY && _thumbQueue.length > 0) {
        const el = _thumbQueue.shift();
        if (!el.isConnected || !el.dataset.thumbSrc || _thumbActive.has(el)) continue;
        _thumbActive.add(el);
        _thumbFetchOne(el).finally(() => {
            _thumbActive.delete(el);
            _thumbRun();
        });
    }
}

async function _thumbFetchOne(el) {
    const src = el.dataset.thumbSrc;
    if (!src) return;
    // Check cache before fetching (another element may have populated it).
    if (_thumbCache.has(src)) {
        const cached = _thumbCache.get(src);
        if (cached) _thumbReplace(el, cached); else _thumbShowFailed(el);
        return;
    }
    try {
        const resp = await fetch(src);
        if (!el.isConnected) return;
        if (resp.status === 200) {
            const salientCx = resp.headers.get('x-salient-cx');
            const salientCy = resp.headers.get('x-salient-cy');
            const blob = await resp.blob();
            const url = URL.createObjectURL(blob);
            _thumbCache.set(src, url);
            if (salientCx !== null && salientCy !== null) {
                _thumbSalient.set(url, { cx: parseFloat(salientCx), cy: parseFloat(salientCy) });
            }
            if (el.isConnected) _thumbReplace(el, url);
        } else if (resp.status === 202 || resp.status === 503) {
            // Fallback: server indicated not yet ready (should rarely fire since
            // thumb_handler now waits asynchronously for a generation slot).
            await new Promise(resolve => setTimeout(resolve, 500));
            if (el.isConnected) {
                _thumbQueue.push(el);
                _thumbObserver.observe(el);
            }
        } else if (resp.status === 204) {
            _thumbCache.set(src, null);
            if (el.isConnected) _thumbShowFailed(el);
        } else {
            if (el.isConnected) _thumbShowFailed(el);
        }
    } catch (_) {
        if (el.isConnected) _thumbShowFailed(el);
    }
}

// ---------------------------------------------------------------------------
// Dir-thumb parallel pool
// ---------------------------------------------------------------------------

/** Collect not-yet-queued dir-thumb elements and append them to _dirThumbQueue
 *  sorted by distance from the viewport midpoint (nearest first). */
function _dirThumbEnqueueRemaining() {
    const mid = window.scrollY + window.innerHeight / 2;
    const pending = [...document.querySelectorAll('.card-icon[data-thumb-src]')]
        .filter(el => {
            const src = el.dataset.thumbSrc || '';
            return src.includes('/api/dir-thumbs?')
                && !_dirThumbActive.has(el)
                && !_dirThumbQueue.includes(el);
        });
    if (pending.length === 0) return;
    pending.sort((a, b) => {
        const ay = a.getBoundingClientRect().top + window.scrollY + a.offsetHeight / 2;
        const by = b.getBoundingClientRect().top + window.scrollY + b.offsetHeight / 2;
        return Math.abs(ay - mid) - Math.abs(by - mid);
    });
    _dirThumbQueue.push(...pending);
}

/** Fill free slots in the parallel dir-thumb pool from _dirThumbQueue. */
function _dirThumbSchedule() {
    // If queue is empty, try to find remaining off-screen elements.
    if (_dirThumbQueue.length === 0) _dirThumbEnqueueRemaining();
    while (_dirThumbActive.size < DIR_THUMB_CONCURRENCY && _dirThumbQueue.length > 0) {
        const el = _dirThumbQueue.shift();
        if (!el.isConnected || !el.dataset.thumbSrc || _dirThumbActive.has(el)) continue;
        _dirThumbActive.add(el);
        _dirThumbFetch(el).finally(() => {
            _dirThumbActive.delete(el);
            _dirThumbSchedule();
        });
    }
}

/** Fetch one dir-thumb.  The server now blocks the response until the
 *  thumbnail is ready, so this is normally a single-shot fetch.  A small
 *  retry budget is kept as a safety net for the rare case where the backend
 *  returns 202 (e.g. a generation timeout on very large directories).
 *  The AbortController signal cancels both the fetch and any retry delay
 *  immediately when the user navigates away. */
async function _dirThumbFetch(el) {
    const src = el.dataset.thumbSrc;
    if (!src) return;
    const signal = _dirThumbAbortCtrl.signal;
    for (let attempt = 0; attempt < 5; attempt++) {
        if (!el.isConnected || signal.aborted) return;
        if (_thumbCache.has(src)) {
            const cached = _thumbCache.get(src);
            delete el.dataset.thumbSrc;
            if (cached && el.isConnected) _thumbReplace(el, cached);
            return;
        }
        try {
            const resp = await fetch(src, { signal });
            if (!el.isConnected || signal.aborted) return;
            if (resp.status === 200) {
                const blob = await resp.blob();
                const url = URL.createObjectURL(blob);
                _thumbCache.set(src, url);
                delete el.dataset.thumbSrc;
                if (el.isConnected) _thumbReplace(el, url);
                return;
            } else if (resp.status === 202 || resp.status === 503) {
                // Fallback: server not yet ready; wait before retrying.
                await new Promise(resolve => {
                    const t = setTimeout(resolve, 3000);
                    signal.addEventListener('abort', () => { clearTimeout(t); resolve(); }, { once: true });
                });
            } else if (resp.status === 204) {
                if (el.isConnected) _thumbShowFailed(el);
                return;
            } else {
                if (el.isConnected) _thumbShowFailed(el);
                return;
            }
        } catch (err) {
            if (err.name === 'AbortError') return;
            if (el.isConnected) _thumbShowFailed(el);
            return;
        }
    }
    if (el.isConnected) _thumbShowFailed(el);
}

/** Cancel all in-flight dir-thumb fetches and their retry waits, then reset
 *  the queue.  Called on navigation so the new directory gets pool slots
 *  immediately instead of waiting for the previous directory to drain. */
function _dirThumbAbort() {
    _dirThumbAbortCtrl.abort();
    _dirThumbAbortCtrl = new AbortController();
    _dirThumbQueue.length = 0;
    // _dirThumbActive is NOT cleared here: aborted coroutines exit via
    // AbortError, then free their own slot via the .finally() callback in
    // _dirThumbSchedule.  This keeps active.size accurate at all times.
}

function _thumbClearCache() {
    for (const url of _thumbCache.values()) URL.revokeObjectURL(url);
    _thumbCache.clear();
    _thumbQueue.length = 0;
    _thumbActive.clear();
    _dirThumbAbort(); // abort in-flight dir-thumb fetches + clear queue
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

/// Build the AI analyse/accept/clear button row HTML for a single file.
/// Returns empty string if the file is not analysable and has no AI tags.
/// Called by renderDetail() and renderDetailTagsSectionOnly().
function _renderAiBtn(f, type_, covered) {
    const hasAiTags = covered && f.tags.some(tag => tag.name.startsWith('ai/'));
    const isAnalysable = covered && (type_ === 'image' || type_ === 'raw' || type_ === 'zip' || type_ === 'video');
    if (!isAnalysable && !hasAiTags) return '';
    const isAnalysing = state.aiAnalysing.has(f.path);
    const aiAcceptBtn = hasAiTags
        ? `<button class="ai-clear-btn" onclick="aiAcceptAllTags(['${jesc(f.path)}'])">${esc(t('ai.accept-all'))}</button>`
        : '';
    const aiClearBtn = hasAiTags
        ? `<button class="ai-clear-btn" onclick="aiClearTags(['${jesc(f.path)}'])">${esc(t('ai.clear-tags'))}</button>`
        : '';
    return `<div class="ai-analyse-row" id="ai-analyse-row">
            ${isAnalysable ? `
            <div class="ai-analyse-controls">
                <div class="ai-analyse-btn-row">
                    <button class="ai-analyse-btn" id="ai-analyse-single-btn" onclick="aiAnalyseSingle('${jesc(f.path)}')" ${isAnalysing ? 'disabled' : ''}>${isAnalysing ? esc(t('ai.analysing')) : esc(t('ai.analyse-btn'))}</button>
                    ${type_ === 'video' ? `
                    <label class="ai-frames-label" title="${esc(t('ai.frames-auto-title'))}"><input type="checkbox" id="ai-frames-auto" ${state.aiVideoFramesAuto ? 'checked' : ''} onchange="aiSetVideoFramesAuto(this.checked)"><span>${esc(t('ai.frames-auto-label'))}</span></label>
                    <label class="ai-frames-label" title="${esc(t('ai.frames-title'))}"><input type="number" id="ai-frames-input" class="ai-frames-input" value="${state.aiVideoFrames}" min="2" max="256" step="1" onchange="aiSetVideoFrames(this.value)" ${state.aiVideoFramesAuto ? 'disabled' : ''}><span>${esc(t('ai.frames-label'))}</span></label>
                    ` : ''}
                </div>
            </div>
            <small class="ai-analyse-note">${type_ === 'video' ? esc(t('ai.video-note')) : ''}</small>` : ''}
            ${(aiAcceptBtn || aiClearBtn) ? `<div class="ai-action-row">${aiAcceptBtn}${aiClearBtn}</div>` : ''}
           </div>`;
}

/// Partial update: refresh only the tag chips and AI button row in the
/// detail panel, without touching the preview element (keeps video playing).
function _updateSubjectLabelHighlight() {
    const input = document.getElementById('tag-subject');
    const activeSubj = input ? input.value.trim() : '';
    document.querySelectorAll('#detail .subject-group .subject-label').forEach(el => {
        el.classList.toggle('subject-label--active', !!activeSubj && el.textContent.trim() === activeSubj);
    });
}

function renderDetailTagsSectionOnly() {
    if (!state.selectedFile) return;
    const f = state.selectedFile;
    const covered = f.covered !== false;

    const tagsEl = document.querySelector('#detail .detail-tags');
    if (tagsEl) tagsEl.innerHTML = renderFileTagChips(f, covered);
    _updateSubjectLabelHighlight();

    const zipEntry = parseZipEntryPath(f.path);
    const name = zipEntry ? (zipEntry.entryName.split('/').pop() || zipEntry.entryName) : f.path.split('/').pop();
    const type_ = zipEntry ? fileType(zipEntry.entryName) : fileType(name);
    const newAiHtml = _renderAiBtn(f, type_, covered);
    const existingAiRow = document.getElementById('ai-analyse-row');
    if (existingAiRow) {
        if (newAiHtml) {
            existingAiRow.outerHTML = newAiHtml;
        } else {
            existingAiRow.remove();
        }
    } else if (newAiHtml) {
        const tagsSection = document.querySelector('#detail .detail-tags-section');
        if (tagsSection) tagsSection.insertAdjacentHTML('beforeend', newAiHtml);
    }
}

function renderDetail() {
    const panel = document.getElementById('detail');

    // Clean up previous selection.

    // Multi-select bulk panel
    if (state.selectedPaths.size > 1) {
        const count = state.selectedPaths.size;

        // Show a spinner while file details are loading for a new range selection.
        if (state.selectionLoading) {
            panel.innerHTML = `<div class="bulk-tag-section" style="display:flex;flex-direction:column;align-items:center;justify-content:center;min-height:160px;gap:12px;">
                <div class="nav-loading-spinner"></div>
                <span style="font-size:12px;color:var(--text-secondary)">${t('bulk.n-selected', {n: count})}</span>
            </div>`;
            return;
        }

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
        const hasFaceImages = state.faceConfig && state.faceConfig.enabled && state.faceConfig.models_ready
            && paths.some(p => { const n = p.split('/').pop() || p; const ft = fileType(n); return ft === 'image' || ft === 'raw'; });
        const faceBulkSection = hasFaceImages ? `
            <div class="bulk-ai-section">
                <div class="bulk-ai-row">
                    <button class="ai-analyse-btn bulk-ai-btn" onclick="faceDetectSelection()">${esc(t('face.detect-selection-btn'))}</button>
                </div>
            </div>` : '';
        const COMIC_EXTS = new Set(['cbz', 'cbr', 'cb7']);
        const comicPaths = paths.filter(p => {
            const ext = (p.split('.').pop() || '').toLowerCase();
            return COMIC_EXTS.has(ext);
        });
        const comicBulkSection = comicPaths.length > 0 ? `
            <div class="bulk-ai-section">
                <div class="bulk-ai-row">
                    <button class="ai-analyse-btn bulk-ai-btn" id="comic-import-bulk-btn" onclick="comicImportSelection()">${esc(t('comic.import-selection-btn'))}</button>
                </div>
            </div>` : '';
        panel.innerHTML = `
            <div class="detail-header">
                <h3>${t('bulk.n-selected', {n: count})}</h3>
                <div class="detail-header-actions">
                    <button class="detail-icon-btn${state.tagPickerMode ? ' active' : ''}" id="tag-picker-toggle" onclick="enterTagPickerMode()" title="Tag files">
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20.59 13.41l-7.17 7.17a2 2 0 0 1-2.83 0L2 12V2h10l8.59 8.59a2 2 0 0 1 0 2.82z"/><line x1="7" y1="7" x2="7.01" y2="7"/></svg>
                    </button>
                    <button class="detail-icon-btn" onclick="openChat()" title="Chat">
                        <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M14 10.5A1.5 1.5 0 0 1 12.5 12H4l-3 3V3.5A1.5 1.5 0 0 1 2.5 2h10A1.5 1.5 0 0 1 14 3.5z"/></svg>
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
                    <div class="tag-input-row">
                        <div class="tag-input-wrap">
                            <input type="text" id="bulk-tag-input" placeholder="${esc(t('bulk.tag-input'))}">
                        </div>
                        <button onclick="doBulkAddTag()">${esc(t('bulk.add-btn'))}</button>
                    </div>
                    <div class="tag-input-wrap">
                        <input type="text" id="bulk-tag-subject" class="tag-subject-input" placeholder="${esc(t('detail.subject-placeholder'))}">
                    </div>
                </div>
                ${aiAcceptBulkBtn}
                ${aiClearBulkBtn}
                ${aiBulkSection}
                ${faceBulkSection}
                ${comicBulkSection}
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
            const _savedRec = localStorage.getItem('dirTag:recursive') === '1';
            const _savedArc = localStorage.getItem('dirTag:archives') === '1';
            tagInputHtml = `<div class="tag-add-form">
                <input type="text" id="dir-tag-input" placeholder="${esc(t('detail.tag-add'))}">
                <button onclick="doDirAddTag()">${esc(t('detail.tag-add-btn'))}</button>
            </div>
            <div class="tag-add-options">
                <label class="tag-add-option">
                    <input type="checkbox" id="dir-tag-recursive" onchange="_dirRecursiveToggle()" ${_savedRec ? 'checked' : ''}> Recursief
                </label>
                <label class="tag-add-option" id="dir-tag-archives-wrap" ${_savedRec ? '' : 'hidden'}>
                    <input type="checkbox" id="dir-tag-archives" onchange="_dirArchivesToggle()" ${_savedArc ? 'checked' : ''}> Incl. archieven
                </label>
            </div>
            <div id="dir-recursive-status" class="recursive-status"></div>`;
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
                ${d.file_count != null ? `<div class="detail-meta-row"><span class="detail-meta-label">${t('detail.items')}</span><span class="detail-meta-value">${d.file_count}</span></div>` : ''}
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
    // Use root_path from the file-detail response when available (search mode
    // may return files from roots other than the currently browsed directory).
    const _previewDir = f.root_path || currentAbsDir();
    const previewUrl = '/preview/' + encodePath(f.path) + (_previewDir ? '?dir=' + encodeURIComponent(_previewDir) : '');

    let preview;
    if (zipEntry) {
        // Entry inside a zip archive
        const entry = state.zipEntries.find(e => e.name === zipEntry.entryName);
        if (entry && entry.is_image && entry.image_index !== null) {
            const thumbUrl = '/api/zip/thumb?' + new URLSearchParams({ path: zipEntry.zipPath, page: entry.image_index, dir: _previewDir });
            preview = `<a class="preview-zoomable" onclick="openMediaViewer('${jesc(zipEntry.zipPath)}', ${entry.image_index})" title="Click to open in viewer">` +
                      `<img src="${thumbUrl}" alt="${esc(name)}" onerror="_cardThumbError(this)"></a>`;
        } else {
            // Entries not loaded yet (e.g. selected from search results);
            // show placeholder and async-fetch entries to render the real preview.
            preview = `<div class="no-preview" id="zip-entry-preview-placeholder">${fileIcon(name)}</div>`;
        }
    } else if (type_ === 'image') {
        preview = `<div class="preview-zoomable" onclick="openMediaViewer('${jesc(f.path)}')">` +
                  `<img class="detail-img-zoomable" src="${previewUrl}" alt="${esc(name)}" data-name="${esc(name)}" onerror="_previewImgError(this)">` +
                  `<div class="preview-viewer-hover-zone"><button class="preview-viewer-overlay-btn" onclick="event.stopPropagation();openMediaViewer('${jesc(f.path)}')" tabindex="-1">Open in viewer</button></div>` +
                  `</div>`;
    } else if (type_ === 'raw') {
        preview = `<div class="preview-zoomable" onclick="if(!state.selectedPaths.has('${jesc(f.path)}'))selectFile('${jesc(f.path)}',event);openFileInDirViewer('${jesc(f.path)}')">` +
                  `<img src="${previewUrl}" alt="${esc(name)}" data-name="${esc(name)}"` +
                  ` onerror="_previewRawError(this)">` +
                  `<div class="preview-viewer-hover-zone"><button class="preview-viewer-overlay-btn" onclick="event.stopPropagation();if(!state.selectedPaths.has('${jesc(f.path)}'))selectFile('${jesc(f.path)}',event);openFileInDirViewer('${jesc(f.path)}')" tabindex="-1">Open in viewer</button></div>` +
                  `</div>`;
    } else if (type_ === 'audio') {
        preview = `<audio controls preload="metadata" src="${previewUrl}" ondblclick="openLightbox('${jesc(f.path)}','audio')" onclick="if(!state.selectedPaths.has('${jesc(f.path)}'))selectFile('${jesc(f.path)}',event);"></audio>`;
    } else if (type_ === 'video') {
        const ext = name.split('.').pop().toLowerCase();
        const transcodeUrl = previewUrl.replace(/^\/preview\//, '/transcode/');
        if (!_browserCanPlayVideoExt(ext) && transcodeUrl !== previewUrl) {
            // Browser cannot play this format natively — skip the doomed play
            // attempt (which would produce console errors) and go straight to
            // the transcode option.
            preview = `<div class="no-preview">${fileIcon(name)}`
                    + `<div class="preview-unavail-msg">Browser kan dit formaat niet direct afspelen.</div>`
                    + `<button class="transcode-btn" onclick="_transcodeImmediately(this,'${jesc(transcodeUrl)}','${jesc(name)}')">Transcoderen voor afspelen</button>`
                    + `</div>`;
        } else {
            preview = `<video controls preload="metadata" src="${previewUrl}" data-name="${esc(name)}" data-preview-url="${previewUrl}"`
                    + ` onerror="_previewVideoError(this)" onclick="if(!state.selectedPaths.has('${jesc(f.path)}'))selectFile('${jesc(f.path)}',event);"></video>`;
        }
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
        const _zipCoverDir = _previewDir ? '?dir=' + encodeURIComponent(_previewDir) : '';
        preview = `<div class="zip-cover-wrap" onclick="openMediaViewer('${jesc(f.path)}')">
            <img src="/thumb/${encodePath(f.path)}${_zipCoverDir}" alt="${esc(name)}" class="zip-cover"
                 onerror="this.style.display='none'">
            <div class="preview-viewer-hover-zone"><button class="preview-viewer-overlay-btn" onclick="event.stopPropagation();openMediaViewer('${jesc(f.path)}')" tabindex="-1">Open in viewer</button></div>
        </div>`;
    } else {
        preview = `<div class="no-preview">${fileIcon(name)}</div>`;
    }

    const covered = f.covered !== false;

    const hasAiTags = covered && f.tags.some(tag => tag.name.startsWith('ai/'));
    const tagChips = renderFileTagChips(f, covered);

    const tagAddSection = covered
        ? `<div class="tag-add-form">
                <div class="tag-input-row">
                    <div class="tag-input-wrap">
                        <input type="text" id="tag-input" placeholder="${esc(t('detail.tag-add'))}">
                    </div>
                    <button onclick="doAddTag()">${esc(t('detail.tag-add-btn'))}</button>
                </div>
                <div class="tag-input-wrap">
                    <input type="text" id="tag-subject" class="tag-subject-input" placeholder="${esc(t('detail.subject-placeholder'))}">
                </div>
            </div>`
        : `<div class="uncovered-notice">${esc(t('detail.uncovered'))}</div>`;

    const aiBtn = _renderAiBtn(f, type_, covered);

    // ComicInfo import button — shown for CBZ/CBR/CB7 archives only.
    const COMIC_EXTS = new Set(['cbz', 'cbr', 'cb7']);
    const isComicArchive = covered && type_ === 'zip'
        && COMIC_EXTS.has((name.split('.').pop() || '').toLowerCase());
    const comicBtn = isComicArchive
        ? `<div class="comic-import-row" id="comic-import-row">
               <button class="ai-analyse-btn" id="comic-import-btn" onclick="comicImportMetadata('${jesc(f.path)}')">${esc(t('comic.import-btn'))}</button>
           </div>`
        : '';

    const viewerBtnRow = '';

    panel.innerHTML = `
        <div class="detail-top">
        <div class="detail-header">
            <h3>${esc(name)}</h3>
            <div class="detail-header-actions">
                <button class="detail-icon-btn${state.tagPickerMode ? ' active' : ''}" id="tag-picker-toggle" onclick="enterTagPickerMode()" title="Tag file">
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20.59 13.41l-7.17 7.17a2 2 0 0 1-2.83 0L2 12V2h10l8.59 8.59a2 2 0 0 1 0 2.82z"/><line x1="7" y1="7" x2="7.01" y2="7"/></svg>
                </button>
                <button class="detail-icon-btn" onclick="openChat()" title="Chat">
                    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M14 10.5A1.5 1.5 0 0 1 12.5 12H4l-3 3V3.5A1.5 1.5 0 0 1 2.5 2h10A1.5 1.5 0 0 1 14 3.5z"/></svg>
                </button>
            </div>
        </div>
        <div class="detail-preview">
            <button class="meta-info-btn" title="File info" onclick="toggleMetaOverlay(event)">i</button>
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
            ${comicBtn}
            <div class="detail-similar-section" id="detail-similar">
                <div class="detail-similar-header">
                    <button class="detail-similar-toggle" onclick="toggleSimilarSection('${jesc(f.path)}')">Similar</button>
                </div>
                <div class="detail-similar-results" id="detail-similar-results" hidden></div>
            </div>
        </div>`;
    if (covered) {
        attachTagAutocomplete(document.getElementById('tag-input'), () => doAddTag());
        const subjInput = document.getElementById('tag-subject');
        attachSubjectAutocomplete(subjInput, collectSingleFileSubjects);
        if (subjInput) subjInput.addEventListener('input', _updateSubjectLabelHighlight);
    }
    initDetailVHandle(document.getElementById('detail-v-handle'));

    // Picture-in-Picture event listeners: hide surrounding UI while PiP is active
    if (type_ === 'video') {
        const vid = panel.querySelector('.detail-preview video');
        if (vid) {
            vid.addEventListener('enterpictureinpicture', () => panel.classList.add('pip-active'));
            vid.addEventListener('leavepictureinpicture', () => panel.classList.remove('pip-active'));
        }
    }

    // Face detection overlay (images only)
    if (typeof faceOnDetailRendered === 'function' && (type_ === 'image' || type_ === 'raw')) {
        faceOnDetailRendered(f.path, type_);
    }

    // Async-fetch zip entry preview when entries are not cached (e.g. selected from search results)
    if (zipEntry && !state.zipEntries.find(e => e.name === zipEntry.entryName)) {
        const _selectedPath = f.path;
        const _zipEntryName = zipEntry.entryName;
        const _zipPath = zipEntry.zipPath;
        const _entryName = name;
        api('/api/zip/entries?' + new URLSearchParams({ path: _zipPath, dir: _previewDir }))
            .then(data => {
                if (state.selectedFile?.path !== _selectedPath) return;
                const entry = (data.entries || []).find(e => e.name === _zipEntryName);
                const placeholder = document.getElementById('zip-entry-preview-placeholder');
                if (!placeholder || !entry || !entry.is_image || entry.image_index === null) return;
                const thumbUrl = '/api/zip/thumb?' + new URLSearchParams({ path: _zipPath, page: entry.image_index, dir: _previewDir });
                const anchor = document.createElement('a');
                anchor.className = 'preview-zoomable';
                anchor.title = 'Click to open in viewer';
                anchor.onclick = () => openMediaViewer(_zipPath, entry.image_index);
                const imgEl = document.createElement('img');
                imgEl.src = thumbUrl;
                imgEl.alt = _entryName;
                imgEl.onerror = function() { _cardThumbError(this); };
                anchor.appendChild(imgEl);
                placeholder.replaceWith(anchor);
                // Now that the real preview image is in the DOM, apply face
                // detection overlays (detections were already fetched by
                // faceOnDetailRendered but found no image to wrap at that point).
                if (typeof _faceWrapPreviewImg === 'function') {
                    _faceWrapPreviewImg();
                    if (typeof _faceRenderOverlays === 'function') {
                        _faceRenderOverlays(_selectedPath);
                    }
                }
            })
            .catch(() => {});
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

// ---------------------------------------------------------------------------
// ComicInfo.xml import
// ---------------------------------------------------------------------------

/** Import ComicInfo.xml metadata from a comic archive and apply as tags. */
async function comicImportMetadata(path) {
    const btn = document.getElementById('comic-import-btn');
    if (btn) { btn.disabled = true; btn.textContent = t('comic.importing'); }
    try {
        const result = await apiPost('/api/comic/import-metadata', { path, dir: currentAbsDir() });
        await loadTags();
        if (state.selectedFile && state.selectedFile.path === path) {
            await loadFileDetail(path);
        }
        ftEmit('ft:file-tags', { paths: [path] });
        showToast(t('comic.imported', { n: result.imported }), 4000);
    } catch (e) {
        const msg = e.message || String(e);
        if (msg.toLowerCase().includes('no comicinfo')) {
            showToast(t('comic.not-found'), 4000);
        } else {
            showToast(t('comic.error') + ': ' + msg, 5000);
        }
        if (btn) { btn.disabled = false; btn.textContent = t('comic.import-btn'); }
    }
}

/** Import ComicInfo.xml for all comic archives in the current selection. */
async function comicImportSelection() {
    const COMIC_EXTS = new Set(['cbz', 'cbr', 'cb7']);
    const paths = [...state.selectedPaths].filter(p => {
        const ext = (p.split('.').pop() || '').toLowerCase();
        return COMIC_EXTS.has(ext);
    });
    if (paths.length === 0) return;

    const btn = document.getElementById('comic-import-bulk-btn');
    if (btn) { btn.disabled = true; btn.textContent = t('comic.importing'); }
    try {
        let imported = 0;
        for (const path of paths) {
            try {
                const result = await apiPost('/api/comic/import-metadata', { path, dir: searchDirForPath(path) });
                imported += result.imported ?? 0;
            } catch (_) {
                // skip archives without ComicInfo.xml silently
            }
        }
        await loadTags();
        if (state.selectedFile && paths.includes(state.selectedFile.path)) {
            await loadFileDetail(state.selectedFile.path);
        }
        ftEmit('ft:file-tags', { paths });
        showToast(t('comic.imported-bulk', { n: paths.length }), 4000);
    } finally {
        if (btn) { btn.disabled = false; btn.textContent = t('comic.import-selection-btn'); }
    }
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
        // Haal subject dynamisch uit het formulier bij promote, maar geef null door als subject leeg is
        const promoteBtn = tag.name.startsWith('ai/')
            ? `<button class=\"promote\" title=\"${esc(t('detail.promote-title'))}\" onclick=\"(function(){
                var subjInput = document.getElementById('tag-subject');
                var subj = subjInput && subjInput.value.trim() ? subjInput.value.trim() : (${subjArg});
                if (!subj) subj = null;
                aiPromoteTag('${jesc(f.path)}','${jesc(tag.name)}','${jesc(tag.value || '')}', subj);
            })()\">&uarr;</button>`
            : '';
        return `<span class="tag-chip"${chipColor} draggable="true" ondragstart="detailChipDragStart(event,'${jesc(f.path)}','${jesc(tagStr)}',${subjArg})">${esc(tagStr)}${promoteBtn}<button class="remove" onclick="doRemoveTag('${jesc(f.path)}','${jesc(tagStr)}',${subjArg})">&times;</button></span>`;
    }

    let html = '';
    // Render no-subject tags first — wrapped in a drop zone.
    const noSubj = groups.get('');
    if (noSubj) {
        html += `<div class="no-subject-zone" ondragover="detailSubjectDragOver(event)" ondragleave="detailSubjectDragLeave(event)" ondrop="detailSubjectDrop(event,'${jesc(f.path)}',null)">`;
        html += noSubj.map(chipHtml).join('');
        html += `</div>`;
    }

    // Render subject groups.
    for (const [subj, tags] of groups) {
        if (subj === '') continue;
        const explicitTags = tags.filter(t => !t.implicit);
        const implicitTags = tags.filter(t => t.implicit);
        const hasExplicit = explicitTags.length > 0;
        const hasImplicit = implicitTags.length > 0;
        if (!hasExplicit && !hasImplicit) continue;
        html += `<div class="subject-group" ondragover="detailSubjectDragOver(event)" ondragleave="detailSubjectDragLeave(event)" ondrop="detailSubjectDrop(event,'${jesc(f.path)}','${jesc(subj)}')">`;
        html += `<span class="subject-label" title="Click to fill subject field" onclick="toggleSubjectInput('${jesc(subj)}')">${esc(subj)}</span>`;
        if (covered) {
            html += `<button class="subject-rename" title="Rename subject" onclick="event.stopPropagation();startSubjectRename(this.parentElement,'${jesc(f.path)}','${jesc(subj)}')" tabindex="-1">&#9998;</button>`;
        }
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
    // Group paths by root and issue one bulk request per root.
    const byDir = new Map();
    for (const p of paths) { const d = searchDirForPath(p); if (!byDir.has(d)) byDir.set(d, []); byDir.get(d).push(p); }
    await Promise.all([...byDir.entries()].map(([d, ps]) => apiPost('/api/tag-bulk', { paths: ps, tags: [tagStr], dir: d })));
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
    const byDir = new Map();
    for (const p of paths) { const d = searchDirForPath(p); if (!byDir.has(d)) byDir.set(d, []); byDir.get(d).push(p); }
    await Promise.all([...byDir.entries()].map(([d, ps]) => apiPost('/api/untag-bulk', { paths: ps, tags: [tagStr], dir: d })));
    // Update local cache immediately so the chip list refreshes right away,
    // before the slower loadTags() / loadFiles() network calls complete.
    for (const p of paths) {
        const d = state.selectedFilesData.get(p);
        if (d) d.tags = d.tags.filter(t => formatTag(t) !== tagStr);
    }
    const statusEl = document.getElementById('bulk-status');
    if (statusEl) statusEl.textContent = t('bulk.removed', {tag: tagStr, n: paths.length, plural: paths.length !== 1 ? t('bulk.removed-plural') : ''});
    const chipsEl = document.getElementById('bulk-tag-chips');
    if (chipsEl) chipsEl.innerHTML = renderBulkTagChips(aggregateBulkTags(), state.selectedPaths.size);
    // Reload server-side tag counts; if this fails the local update already
    // kept the UI correct so swallow the error.
    try {
        await loadTags();
        if (state.mode === 'browse') await loadFiles(state.currentPath);
    } catch (_) { /* local state already updated */ }
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
    // Render breadcrumb and file grid first so the first chunk of items paints
    // as quickly as possible.  The tag sidebar and detail panel can be heavy
    // (many tags, many subjects), so defer them until after the first frame.
    renderBreadcrumb();
    try {
        renderContent(); // first _RENDER_INITIAL items painted synchronously
    } catch (e) {
        console.error('renderContent failed:', e);
    }
    // Yield to the browser so it can paint the content before building the
    // tag sidebar and detail panel.  _thumbInit() runs inside the callback so
    // it picks up the first chunk that was just painted.
    requestAnimationFrame(() => {
        renderTags();
        renderDetail();
        renderInfo();
        renderFiletree();
        _thumbInit();
        _dirThumbInit();
        _kbRestoreFocus();
    });
}

// ---------------------------------------------------------------------------
// Similar-files section
// ---------------------------------------------------------------------------

async function toggleSimilarSection(path) {
    const results = document.getElementById('detail-similar-results');
    if (!results) return;
    if (!results.hidden) { results.hidden = true; return; }
    results.hidden = false;
    _loadSimilarResults(path, results);
}

async function _loadSimilarResults(path, resultsEl) {
    resultsEl.innerHTML = '<div class="detail-similar-loading">Searching…</div>';
    try {
        const data = await loadSimilarFiles(path, 20);
        if (!data.results?.length) {
            resultsEl.innerHTML = '<div class="detail-similar-empty">Geen vergelijkbare bestanden gevonden.</div>';
            return;
        }
        resultsEl.innerHTML = `<div class="detail-similar-grid">${data.results.map(r => {
            const p = r.abs_path || r.path;
            const thumbUrl = '/thumb/' + encodePath(p) + dirParam('?');
            const score = Math.round((r.score || 0) * 100);
            return `<div class="detail-similar-item" onclick="selectFile('${jesc(p)}',null)" title="${esc(p)} (${score}%)">
                <img src="${thumbUrl}" loading="lazy" onerror="this.style.display='none'">
                <span class="detail-similar-score">${score}%</span>
            </div>`;
        }).join('')}</div>`;
    } catch (e) {
        resultsEl.innerHTML = `<div class="detail-similar-empty">Fout: ${esc(String(e))}</div>`;
    }
}
