// ---------------------------------------------------------------------------
// Lightbox
// ---------------------------------------------------------------------------

// Zoom/pan state for images
const _lb = { scale: 1, dx: 0, dy: 0, dragging: false, sx: 0, sy: 0, isImg: false };

function _lbApplyTransform() {
    const img = document.querySelector('#lightbox-content img');
    if (img) img.style.transform = `translate(${_lb.dx}px,${_lb.dy}px) scale(${_lb.scale})`;
}

function _lbWheel(e) {
    if (!_lb.isImg) return;
    e.preventDefault();
    const factor = e.deltaY < 0 ? 1.15 : 1 / 1.15;
    _lb.scale = Math.min(Math.max(_lb.scale * factor, 0.5), 12);
    _lbApplyTransform();
}

function _lbMouseDown(e) {
    if (!_lb.isImg || _lb.scale <= 1) return;
    _lb.dragging = true;
    _lb.sx = e.clientX - _lb.dx;
    _lb.sy = e.clientY - _lb.dy;
    e.preventDefault();
}

function _lbMouseMove(e) {
    if (!_lb.dragging) return;
    _lb.dx = e.clientX - _lb.sx;
    _lb.dy = e.clientY - _lb.sy;
    _lbApplyTransform();
}

function _lbMouseUp() { _lb.dragging = false; }

function _lbDblClick(e) {
    if (!_lb.isImg) return;
    if (_lb.scale !== 1) {
        _lb.scale = 1; _lb.dx = 0; _lb.dy = 0;
    } else {
        _lb.scale = 2;
    }
    _lbApplyTransform();
    e.stopPropagation();
}

function _lbAttachZoom() {
    const el = document.getElementById('lightbox');
    el.addEventListener('wheel', _lbWheel, { passive: false });
    el.addEventListener('mousedown', _lbMouseDown);
    el.addEventListener('mousemove', _lbMouseMove);
    el.addEventListener('mouseup', _lbMouseUp);
    el.addEventListener('mouseleave', _lbMouseUp);
    el.addEventListener('dblclick', _lbDblClick);
}

function _lbDetachZoom() {
    const el = document.getElementById('lightbox');
    if (!el) return;
    el.removeEventListener('wheel', _lbWheel);
    el.removeEventListener('mousedown', _lbMouseDown);
    el.removeEventListener('mousemove', _lbMouseMove);
    el.removeEventListener('mouseup', _lbMouseUp);
    el.removeEventListener('mouseleave', _lbMouseUp);
    el.removeEventListener('dblclick', _lbDblClick);
}

// Dispatch: images open in the directory viewer; everything else in the lightbox.
function cvOpenFile(path, type) {
    if (type === 'image' || type === 'raw') { openFileInDirViewer(path); }
    else { openLightbox(path, type); }
}

function openLightbox(path, type, duration) {
    // Video plays natively in the detail panel with its own fullscreen button.
    // No lightbox needed for video.
    if (type === 'video') return;

    // Pause any audio playing in the detail panel.
    document.querySelectorAll('#detail audio').forEach(m => m.pause());

    const url = '/preview/' + encodePath(path) + dirParam('?');
    const lb = document.getElementById('lightbox');
    const content = document.getElementById('lightbox-content');
    _lb.scale = 1; _lb.dx = 0; _lb.dy = 0; _lb.dragging = false;
    _lb.isImg = (type === 'image' || type === 'raw');

    let html = '';
    if (type === 'image' || type === 'raw') {
        html = `<img src="${url}" alt="${esc(path.split('/').pop())}"
                     onerror="this.replaceWith(Object.assign(document.createElement('p'),{textContent:'Preview unavailable',className:'lightbox-error'}))">`;
    } else if (type === 'audio') {
        html = `<audio controls autoplay src="${url}"></audio>`;
    } else if (type === 'pdf') {
        html = `<iframe class="lightbox-pdf" src="${url}" title="${esc(path.split('/').pop())}"></iframe>`;
    } else if (type === 'text' || type === 'markdown') {
        html = `<pre class="lightbox-text hl-dark" id="lightbox-text-pre">Loading…</pre>`;
    }
    content.innerHTML = html;

    lb.hidden = false;

    // Zoom hint for images
    if (_lb.isImg) {
        _lbAttachZoom();
        const hint = document.createElement('div');
        hint.className = 'lightbox-hint';
        hint.textContent = 'Scroll to zoom · drag to pan · double-click to reset';
        lb.appendChild(hint);
        setTimeout(() => hint.remove(), 2500);
    }

    document.addEventListener('keydown', _lightboxKeyHandler, { once: true });

    if (type === 'text') {
        const filename = path.split('/').pop();
        fetch(url).then(r => r.text()).then(txt => {
            const el = document.getElementById('lightbox-text-pre');
            if (el) el.innerHTML = highlightCode(txt, filename);
        }).catch(() => {
            const el = document.getElementById('lightbox-text-pre');
            if (el) el.textContent = '(Could not load file)';
        });
    } else if (type === 'markdown') {
        fetch(url).then(r => r.text()).then(txt => {
            const pre = document.getElementById('lightbox-text-pre');
            if (!pre) return;
            const div = document.createElement('div');
            div.className = 'lightbox-markdown';
            div.innerHTML = renderMarkdown(txt);
            pre.replaceWith(div);
        }).catch(() => {
            const el = document.getElementById('lightbox-text-pre');
            if (el) el.textContent = '(Could not load file)';
        });
    }
}

function closeLightbox(event) {
    if (event && event.target !== document.getElementById('lightbox') &&
        !event.target.classList.contains('lightbox-close')) return;
    _lbDetachZoom();
    const lb = document.getElementById('lightbox');
    const content = document.getElementById('lightbox-content');
    content.querySelectorAll('video, audio').forEach(m => m.pause());
    content.innerHTML = '';
    lb.hidden = true;
    lb.querySelectorAll('.lightbox-hint').forEach(h => h.remove());
}

function _lightboxKeyHandler(e) {
    if (e.key === 'Escape') {
        _lbDetachZoom();
        const lb = document.getElementById('lightbox');
        const content = document.getElementById('lightbox-content');
        content.querySelectorAll('video, audio').forEach(m => m.pause());
        content.innerHTML = '';
        lb.hidden = true;
        lb.querySelectorAll('.lightbox-hint').forEach(h => h.remove());
    } else {
        document.addEventListener('keydown', _lightboxKeyHandler, { once: true });
    }
}
