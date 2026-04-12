// ---------------------------------------------------------------------------
// Icons (inline SVG)
// ---------------------------------------------------------------------------

const ICONS = {
    folder: '<svg viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="0.5" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z"/></svg>',
    file:   '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>',
    image:  '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="8.5" cy="8.5" r="1.5"/><polyline points="21 15 16 10 5 21"/></svg>',
    audio:  '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>',
    video:  '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="23 7 16 12 23 17 23 7"/><rect x="1" y="5" width="15" height="14" rx="2"/></svg>',
    pdf:    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="9" y1="13" x2="15" y2="13"/><line x1="9" y1="17" x2="15" y2="17"/><polyline points="9 9 10 9"/></svg>',
    text:   '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="8" y1="13" x2="16" y2="13"/><line x1="8" y1="17" x2="14" y2="17"/></svg>',
    markdown:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="18" rx="2"/><path d="M7 15V9l3 3 3-3v6"/><line x1="16" y1="9" x2="16" y2="15"/><polyline points="13.5 15 16 15"/></svg>',
    raw:    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="8.5" cy="8.5" r="1.5"/><polyline points="21 15 16 10 5 21"/><line x1="15" y1="3" x2="21" y2="3"/><line x1="18" y1="0" x2="18" y2="6"/></svg>',
    zip:    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="12" y1="11" x2="12" y2="17"/><line x1="10" y1="11" x2="14" y2="11"/></svg>',
    gotoDir:'<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M1 4.5v7A1.5 1.5 0 002.5 13h11A1.5 1.5 0 0015 11.5V6a1.5 1.5 0 00-1.5-1.5H7L5.5 3H2.5A1.5 1.5 0 001 4.5z"/><polyline points="9 8 11 10 9 12"/><line x1="6" y1="10" x2="11" y2="10"/></svg>',
};

// ---------------------------------------------------------------------------
// File type detection
// ---------------------------------------------------------------------------

const EXT_MAP = {
    image:    ['jpg','jpeg','png','gif','webp','svg','bmp','ico','tiff','tif','avif'],
    audio:    ['mp3','flac','wav','ogg','opus','aac','m4a','wma','aiff','alac'],
    video:    ['mp4','webm','mkv','avi','mov','wmv','flv','m4v','ts','3gp','f4v'],
    pdf:      ['pdf'],
    markdown: ['md','markdown'],
    zip:      ['zip','cbz'],
    text:     ['txt','rst','csv','tsv','log','ini','cfg','conf',
               'json','yaml','yml','toml','xml','html','htm','css','js','ts',
               'jsx','tsx','py','rb','rs','go','java','c','cpp','h','hpp',
               'sh','bash','zsh','fish','sql','diff','patch','gitignore','env'],
    raw:      ['arw','cr2','cr3','nef','orf','rw2','dng','raf','pef','srw',
               'raw','3fr','x3f','rwl','iiq','mef','mos','heic','heif',
               'psd','psb','xcf','ai','eps'],
};

function fileType(name) {
    const ext = name.split('.').pop().toLowerCase();
    for (const [type_, exts] of Object.entries(EXT_MAP)) {
        if (exts.includes(ext)) return type_;
    }
    return 'file';
}

function fileIcon(name) {
    const type_ = fileType(name);
    return ICONS[type_] || ICONS.file;
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

function formatSize(bytes) {
    if (bytes == null) return '';
    if (bytes < 1024) return bytes + ' B';
    const units = ['KiB', 'MiB', 'GiB', 'TiB'];
    let size = bytes / 1024;
    for (const unit of units) {
        if (size < 1024) return size.toFixed(1) + ' ' + unit;
        size /= 1024;
    }
    return size.toFixed(1) + ' PiB';
}

function formatDate(mtimeNs) {
    if (!mtimeNs) return '';
    const ms = Math.floor(mtimeNs / 1_000_000);
    const d = new Date(ms);
    return d.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}

function formatTag(tag) {
    if (tag.value) return tag.name + '=' + tag.value;
    return tag.name;
}

// ---------------------------------------------------------------------------
// Markdown renderer (local, no external deps)
// ---------------------------------------------------------------------------

function renderMarkdown(src) {
    // Protect fenced code blocks first
    const fenced = [];
    src = src.replace(/```([\w]*)\n?([\s\S]*?)```/g, (_, lang, code) => {
        const i = fenced.length;
        const langClass = lang ? ` class="lang-${escMd(lang)}"` : '';
        fenced.push(`<pre class="md-pre"><code${langClass}>${escMd(code.replace(/\n$/, ''))}</code></pre>`);
        return `\x00F${i}\x00`;
    });
    // Inline code
    src = src.replace(/`([^`\n]+)`/g, (_, c) => `<code class="md-code">${escMd(c)}</code>`);

    // Headings
    src = src.replace(/^(#{1,6}) +(.+)$/gm, (_, h, t) =>
        `<h${h.length} class="md-h md-h${h.length}">${t.trim()}</h${h.length}>`);

    // Horizontal rule
    src = src.replace(/^[ \t]*(?:-{3,}|\*{3,}|_{3,})[ \t]*$/gm, '<hr class="md-hr">');

    // Bold + italic combined
    src = src.replace(/\*{3}(.+?)\*{3}/g, '<strong><em>$1</em></strong>');
    src = src.replace(/_{3}(.+?)_{3}/g, '<strong><em>$1</em></strong>');
    // Bold
    src = src.replace(/\*{2}(.+?)\*{2}/g, '<strong>$1</strong>');
    src = src.replace(/_{2}(.+?)_{2}/g, '<strong>$1</strong>');
    // Italic
    src = src.replace(/\*([^*\n]+)\*/g, '<em>$1</em>');
    src = src.replace(/_([^_\n]+)_/g, '<em>$1</em>');
    // Strikethrough
    src = src.replace(/~~(.+?)~~/g, '<del>$1</del>');

    // Images — render as placeholder (no external fetching)
    src = src.replace(/!\[([^\]]*)\]\([^)]*\)/g,
        (_, alt) => `<span class="md-img">[image${alt ? ': ' + escMd(alt) : ''}]</span>`);
    // Links — keep text, discard href (safer for local preview)
    src = src.replace(/\[([^\]]+)\]\([^)]+\)/g, '<span class="md-link">$1</span>');
    // Auto-links
    src = src.replace(/https?:\/\/\S+/g, url => `<span class="md-link">${escMd(url)}</span>`);

    // Blockquotes
    src = src.replace(/^(>[ \t]*.+\n?)+/gm, m => {
        const inner = m.replace(/^>[ \t]?/gm, '').trim();
        return `<blockquote class="md-bq">${inner}</blockquote>\n`;
    });

    // Unordered lists (simple, single-level)
    src = src.replace(/^[ \t]*[-*+] (.+)$/gm, '<li>$1</li>');
    src = src.replace(/(<li>.*<\/li>\n?)+/g, m => `<ul class="md-ul">${m}</ul>`);

    // Ordered lists
    src = src.replace(/^[ \t]*\d+\. (.+)$/gm, '<li>$1</li>');

    // Paragraphs: blank-line-separated
    const paras = src.split(/\n{2,}/);
    src = paras.map(p => {
        p = p.trim();
        if (!p) return '';
        // Don't wrap block-level elements
        if (/^<(h[1-6]|ul|ol|li|blockquote|pre|hr)/.test(p)) return p;
        return `<p class="md-p">${p.replace(/\n/g, '<br>')}</p>`;
    }).join('\n');

    // Restore fenced blocks
    src = src.replace(/\x00F(\d+)\x00/g, (_, i) => fenced[+i]);
    return src;
}

function escMd(s) {
    return String(s)
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;');
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

const state = {
    mode: 'browse', // browse | search | zip
    currentPath: '',
    viewMode: 'grid',
    showHidden: false,
    tags: [],
    entries: [],
    searchQuery: '',
    searchResults: [],
    zipPath: null,         // path to the currently browsed zip archive
    zipEntries: [],        // [{name, size, is_image, image_index, tag_count}]
    selectedFile: null,  // { path, size, file_id, mtime, indexed_at, tags } | null
    selectedDir: null,   // { path, name, file_count } | null
    selectedPaths: new Set(), // multi-select: Set of paths
    selectedFilesData: new Map(), // path → file detail (for tag aggregation)
    info: null,
    detailOpen: true,
    expandedGroups: new Set(), // tag group prefixes that are expanded
};

let _lastClickedPath = null; // for shift-range selection
let _armedBulkTag = null;    // two-step delete: which tag is armed

// ---------------------------------------------------------------------------
// API
// ---------------------------------------------------------------------------

async function api(url) {
    const res = await fetch(url);
    if (!res.ok) {
        const body = await res.json().catch(() => ({}));
        throw new Error(body.error || res.statusText);
    }
    return res.json();
}

async function apiPost(url, body) {
    const res = await fetch(url, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
    });
    if (!res.ok) {
        const b = await res.json().catch(() => ({}));
        throw new Error(b.error || res.statusText);
    }
    return res.json();
}

async function loadInfo() {
    state.info = await api('/api/info');
}

async function loadTags() {
    state.tags = await api('/api/tags');
}

async function loadFiles(path) {
    const url = '/api/files?path=' + encodeURIComponent(path) + (state.showHidden ? '&show_hidden=true' : '');
    const data = await api(url);
    state.currentPath = data.path;
    state.entries = data.entries;
    state.mode = 'browse';
    state.searchQuery = '';
    state.zipPath = null;
    state.zipEntries = [];
    sessionStorage.setItem('ft_path', state.currentPath);
}

async function searchFiles(query) {
    try {
        const data = await api('/api/search?q=' + encodeURIComponent(query));
        state.searchQuery = query;
        state.searchResults = data.results;
        state.mode = 'search';
        state.selectedFile = null;
    } catch (e) {
        state.searchQuery = query;
        state.searchResults = [];
        state.mode = 'search';
        state.selectedFile = null;
    }
    state.selectedPaths.clear();
    state.selectedFilesData.clear();
    _lastClickedPath = null;
    _armedBulkTag = null;
}

async function loadFileDetail(path) {
    state.selectedFile = await api('/api/file?path=' + encodeURIComponent(path));
    state.selectedDir = null;
}

function selectDir(path, name, fileCount) {
    const anchor = saveScrollAnchor(path);
    state.selectedDir = { path, name, file_count: fileCount };
    state.selectedFile = null;
    if (!state.detailOpen) {
        state.detailOpen = true;
        document.querySelector('.layout').classList.remove('detail-collapsed');
        document.getElementById('detail-toggle').classList.add('active');
    }
    renderDetail();
    restoreScrollAnchor(anchor);
}

// Timer to distinguish single click (select) from double click (navigate) on directories.
let _dirClickTimer = null;
let _zipClickTimer = null;

function handleDirClick(path, name, fileCount) {
    if (_dirClickTimer) {
        clearTimeout(_dirClickTimer);
        _dirClickTimer = null;
        navigateTo(path); // double click
    } else {
        _dirClickTimer = setTimeout(() => {
            _dirClickTimer = null;
            selectDir(path, name, fileCount); // single click
        }, 250);
    }
}

function handleZipClick(path, event) {
    if (_zipClickTimer) {
        clearTimeout(_zipClickTimer);
        _zipClickTimer = null;
        openZipDir(path); // double click
    } else {
        _zipClickTimer = setTimeout(() => {
            _zipClickTimer = null;
            selectFile(path, event); // single click
        }, 250);
    }
}

async function addTagToFile(path, tagStr) {
    await apiPost('/api/tag', { path, tags: [tagStr] });
    await loadFileDetail(path);
    await loadTags();
    if (state.mode === 'browse') await loadFiles(state.currentPath);
    if (state.mode === 'zip')    await refreshZipEntries();
}

async function removeTagFromFile(path, tagStr) {
    await apiPost('/api/untag', { path, tags: [tagStr] });
    await loadFileDetail(path);
    await loadTags();
    if (state.mode === 'browse') await loadFiles(state.currentPath);
    if (state.mode === 'zip')    await refreshZipEntries();
}

// ---------------------------------------------------------------------------
// Full path helper
// ---------------------------------------------------------------------------

function fullPath(entry) {
    if (state.mode === 'search') return entry.path;
    if (state.currentPath) return state.currentPath + '/' + entry.name;
    return entry.name;
}

// ---------------------------------------------------------------------------
// Render: Sidebar tags
// ---------------------------------------------------------------------------

const TAG_COLORS = [
    '#ef4444', '#f97316', '#f59e0b', '#eab308', '#84cc16',
    '#22c55e', '#14b8a6', '#06b6d4', '#3b82f6', '#6366f1',
    '#8b5cf6', '#a855f7', '#d946ef', '#ec4899', '#f43f5e',
];

function colorDot(color) {
    if (color) {
        return `<span class="tag-color-dot" style="background:${color}"></span>`;
    }
    return `<span class="tag-color-dot tag-color-dot-empty"></span>`;
}

function renderTags() {
    const el = document.getElementById('tag-list');
    if (!state.tags.length) {
        el.innerHTML = '<div class="empty-state"><span class="empty-state-text">No tags</span></div>';
        return;
    }

    // Group tags by prefix
    const groups = {};
    const standalone = [];
    for (const tag of state.tags) {
        const slash = tag.name.indexOf('/');
        if (slash > 0) {
            const prefix = tag.name.slice(0, slash);
            const suffix = tag.name.slice(slash + 1);
            if (!groups[prefix]) groups[prefix] = { root: null, children: [] };
            groups[prefix].children.push({ suffix, fullName: tag.name, count: tag.count, color: tag.color });
        } else {
            standalone.push(tag);
        }
    }

    // Merge standalone tags that share a name with a group prefix into that group
    const trulyStandalone = [];
    for (const tag of standalone) {
        if (groups[tag.name]) {
            groups[tag.name].root = tag;
        } else {
            trulyStandalone.push(tag);
        }
    }

    let html = '';

    // Grouped tags
    const groupNames = Object.keys(groups).sort();
    for (const prefix of groupNames) {
        const { root, children } = groups[prefix];
        const items = children.sort((a, b) => a.suffix.localeCompare(b.suffix));
        const rootCount = root ? root.count : 0;
        const totalCount = items.reduce((s, i) => s + i.count, 0) + rootCount;
        const groupQuery = root ? `${prefix} or ${prefix}/*` : `${prefix}/*`;
        const groupActive = state.mode === 'search' && state.searchQuery === groupQuery ? ' active' : '';
        const groupColor = root ? root.color : null;
        const expanded = state.expandedGroups.has(prefix);
        const expandedClass = expanded ? ' expanded' : '';
        const rootContextMenu = root ? ` oncontextmenu="showTagMenu(event,'${esc(prefix)}')"` : '';
        html += `<div class="tag-group">
            <div class="tag-group-label${groupActive}${expandedClass}">
                <button class="tag-group-chevron" onclick="toggleTagGroup('${esc(prefix)}')" title="Expand/collapse">
                    <svg class="chevron-icon" viewBox="0 0 12 12"><polyline points="2,3 6,8 10,3" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>
                </button>
                <button class="tag-group-name" onclick="doTagGroupSearch('${esc(prefix)}')"${rootContextMenu}>${colorDot(groupColor)}${esc(prefix)} <span class="count">${totalCount}</span></button>
            </div>
            <div class="tag-group-items${expanded ? ' open' : ''}">`;
        for (const item of items) {
            const q = quoteTag(item.fullName);
            const active = state.mode === 'search' && state.searchQuery === q ? ' active' : '';
            html += `<button class="tag-item${active}" onclick="doTagSearch('${esc(item.fullName)}')" oncontextmenu="showTagMenu(event, '${esc(item.fullName)}')">
                ${colorDot(item.color)}${esc(item.suffix)} <span class="count">${item.count}</span>
            </button>`;
        }
        html += '</div></div>';
    }

    // Standalone tags (those that are not a prefix of any group)
    for (const tag of trulyStandalone.sort((a, b) => a.name.localeCompare(b.name))) {
        const q = quoteTag(tag.name);
        const active = state.mode === 'search' && state.searchQuery === q ? ' active' : '';
        html += `<button class="tag-item tag-standalone${active}" onclick="doTagSearch('${esc(tag.name)}')" oncontextmenu="showTagMenu(event, '${esc(tag.name)}')">
            ${colorDot(tag.color)}${esc(tag.name)} <span class="count">${tag.count}</span>
        </button>`;
    }

    el.innerHTML = html;
}

// ---------------------------------------------------------------------------
// Tag context menu
// ---------------------------------------------------------------------------

function showTagMenu(e, tagName) {
    e.preventDefault();
    e.stopPropagation();
    closeTagMenu();

    const tag = state.tags.find(t => t.name === tagName);
    const currentColor = tag?.color || null;

    let swatches = TAG_COLORS.map(c => {
        const sel = c === currentColor ? ' selected' : '';
        return `<button class="tag-menu-swatch${sel}" style="background:${c}" onclick="setTagColor('${esc(tagName)}','${c}')"></button>`;
    }).join('');
    // "no color" swatch
    const noSel = !currentColor ? ' selected' : '';
    swatches = `<button class="tag-menu-swatch tag-menu-swatch-none${noSel}" onclick="setTagColor('${esc(tagName)}', null)" title="No color">✕</button>` + swatches;

    const menu = document.createElement('div');
    menu.id = 'tag-context-menu';
    menu.className = 'tag-context-menu';
    menu.innerHTML = `
        <div class="tag-menu-header">${esc(tagName)}</div>
        <div class="tag-menu-section">
            <div class="tag-menu-label">Color</div>
            <div class="tag-menu-swatches">${swatches}</div>
        </div>
        <div class="tag-menu-divider"></div>
        <button class="tag-menu-action tag-menu-delete" onclick="deleteTag('${esc(tagName)}')">Delete tag</button>
    `;
    document.body.appendChild(menu);

    // Position near the click
    const rect = menu.getBoundingClientRect();
    let x = e.clientX;
    let y = e.clientY;
    if (x + rect.width > window.innerWidth) x = window.innerWidth - rect.width - 8;
    if (y + rect.height > window.innerHeight) y = window.innerHeight - rect.height - 8;
    menu.style.left = x + 'px';
    menu.style.top = y + 'px';

    // Close on outside click (wait a tick so this click doesn't close it)
    requestAnimationFrame(() => {
        document.addEventListener('click', closeTagMenu, { once: true });
    });
}

function closeTagMenu() {
    const menu = document.getElementById('tag-context-menu');
    if (menu) menu.remove();
}

async function setTagColor(tagName, color) {
    closeTagMenu();
    await apiPost('/api/tag-color', { name: tagName, color });
    await loadTags();
    render();
}

async function deleteTag(tagName) {
    closeTagMenu();
    const tag = state.tags.find(t => t.name === tagName);
    const count = tag?.count || 0;
    if (count > 0 && !confirm(`Delete tag "${tagName}"? It is applied to ${count} file(s).`)) {
        return;
    }
    await apiPost('/api/delete-tag', { name: tagName });
    await loadTags();
    if (state.selectedFile) await loadFileDetail(state.selectedFile.path);
    render();
}

// ---------------------------------------------------------------------------
// Render: Breadcrumb
// ---------------------------------------------------------------------------

function renderBreadcrumb() {
    const el = document.getElementById('breadcrumb');

    if (state.mode === 'search') {
        el.innerHTML = `<span class="breadcrumb-item current">Search: ${esc(state.searchQuery)}</span>`;
        return;
    }

    const rootIsCurrent = state.currentPath === '' && state.mode !== 'zip';
    let html = `<button class="breadcrumb-item${rootIsCurrent ? ' current' : ''}" onclick="navigateTo('')">/</button>`;

    if (state.currentPath) {
        const parts = state.currentPath.split('/');
        let accumulated = '';
        for (let i = 0; i < parts.length; i++) {
            accumulated += (i === 0 ? '' : '/') + parts[i];
            const isCurrent = i === parts.length - 1 && state.mode !== 'zip';
            const path = accumulated;
            if (i > 0) html += `<span class="breadcrumb-sep">/</span>`;
            html += `<button class="breadcrumb-item${isCurrent ? ' current' : ''}" onclick="navigateTo('${esc(path)}')">${esc(parts[i])}</button>`;
        }
    }

    if (state.mode === 'zip') {
        const zipName = state.zipPath.split('/').pop();
        html += `<span class="breadcrumb-sep">/</span><span class="breadcrumb-item current">${esc(zipName)}</span>`;
    }

    el.innerHTML = html;
}

// ---------------------------------------------------------------------------
// Render: File grid
// ---------------------------------------------------------------------------

function renderGrid(items) {
    let html = '';
    for (const entry of items) {
        const isDir = entry.is_dir;
        const name = isDir ? entry.name : (entry.name || entry.path.split('/').pop());
        const path = isDir ? null : fullPath(entry);
        const selected = state.selectedFile && state.selectedFile.path === path ? ' selected' : '';
        const type_ = isDir ? 'folder' : fileType(name);

        let preview = '';
        if (isDir) {
            preview = `<div class="card-icon">${ICONS.folder}</div>`;
        } else if (type_ === 'image' || type_ === 'raw') {
            preview = `<img src="/thumb/${encodeURI(fullPath(entry))}" loading="lazy" alt=""
                data-name="${esc(name)}" onerror="_cardThumbError(this)">`;
        } else if (type_ === 'video') {
            preview = `<img src="/thumb/${encodeURI(fullPath(entry))}" loading="lazy" alt=""
                class="card-thumb-strip" data-name="${esc(name)}" onerror="_cardThumbError(this)">` +
                `<div class="card-filmstrip-badge">${ICONS.video}</div>`;
        } else if (type_ === 'zip') {
            preview = `<img src="/thumb/${encodeURI(fullPath(entry))}" loading="lazy" alt=""
                data-name="${esc(name)}" onerror="_cardThumbError(this)">` +
                `<div class="card-filmstrip-badge">${ICONS.zip || ''}</div>`;
        } else if (type_ === 'pdf') {
            preview = `<img src="/thumb/${encodeURI(fullPath(entry))}" loading="lazy" alt=""
                data-name="${esc(name)}" onerror="_cardThumbError(this)">` +
                `<div class="card-filmstrip-badge">${ICONS.pdf}</div>`;
        } else {
            preview = `<div class="card-icon">${fileIcon(name)}</div>`;
        }

        const meta = isDir ? `${entry.file_count} file${entry.file_count === 1 ? '' : 's'}` : formatSize(entry.size);

        if (isDir) {
            const dirPath = fullPath(entry);
            const dirSelected = state.selectedDir && state.selectedDir.path === dirPath ? ' selected' : '';
            html += `<div class="card folder${dirSelected}" data-path="${esc(dirPath)}" onclick="handleDirClick('${esc(dirPath)}','${esc(name)}',${entry.file_count})">
                <div class="card-preview">${preview}</div>
                <div class="card-body"><div class="card-name">${esc(name)}</div><div class="card-meta">${meta}</div></div>
            </div>`;
        } else {
            const multiSel = state.selectedPaths.has(path) ? ' selected' : '';
            const checkmark = state.selectedPaths.has(path) ? '<span class="card-check">&#10003;</span>' : '';
            const gotoDirBtn = state.mode === 'search'
                ? `<button class="card-goto" onclick="event.stopPropagation();navigateToParent('${esc(path)}')" title="Go to directory">${ICONS.gotoDir}</button>`
                : '';
            if (type_ === 'zip') {
                html += `<div class="card${multiSel}" data-path="${esc(path)}" onclick="handleZipClick('${esc(path)}', event)">
                    ${checkmark}${gotoDirBtn}<div class="card-preview">${preview}</div>
                    <div class="card-body"><div class="card-name">${esc(name)}</div><div class="card-meta">${meta}</div></div>
                </div>`;
            } else {
                const dblFn = `openLightbox('${esc(path)}','${fileType(name)}')`;
                html += `<div class="card${multiSel}" data-path="${esc(path)}" onclick="selectFile('${esc(path)}', event)" ondblclick="${dblFn}">
                    ${checkmark}${gotoDirBtn}<div class="card-preview">${preview}</div>
                    <div class="card-body"><div class="card-name">${esc(name)}</div><div class="card-meta">${meta}</div></div>
                </div>`;
            }
        }
    }
    return html;
}

// ---------------------------------------------------------------------------
// Render: File list
// ---------------------------------------------------------------------------

function renderList(items) {
    let html = `<div class="list-header">
        <span></span><span>Name</span><span>Size</span><span>Modified</span><span>Tags</span>
    </div>`;

    for (const entry of items) {
        const isDir = entry.is_dir;
        const name = isDir ? entry.name : (entry.name || entry.path.split('/').pop());
        const path = isDir ? null : fullPath(entry);
        const selected = state.selectedFile && state.selectedFile.path === path ? ' selected' : '';
        const icon = isDir ? ICONS.folder : fileIcon(name);
        const size = isDir ? '' : formatSize(entry.size);
        const date = isDir ? '' : formatDate(entry.mtime);
        const tags = isDir ? `${entry.file_count} files` : (entry.tag_count != null ? `${entry.tag_count} tags` : '');

        if (isDir) {
            const dirPath = fullPath(entry);
            const dirSelected = state.selectedDir && state.selectedDir.path === dirPath ? ' selected' : '';
            html += `<div class="list-row folder${dirSelected}" data-path="${esc(dirPath)}" onclick="handleDirClick('${esc(dirPath)}','${esc(name)}',${entry.file_count})">
                <span class="icon">${icon}</span>
                <span class="name">${esc(name)}</span>
                <span class="size">${size}</span>
                <span class="date">${date}</span>
                <span class="tags-count">${tags}</span>
            </div>`;
        } else {
            const multiSel = state.selectedPaths.has(path) ? ' selected' : '';
            const gotoDirBtn = state.mode === 'search'
                ? `<button class="goto-dir-btn" onclick="event.stopPropagation();navigateToParent('${esc(path)}')" title="Go to directory">${ICONS.gotoDir}</button>`
                : '';
            if (fileType(name) === 'zip') {
                html += `<div class="list-row${multiSel}" data-path="${esc(path)}" onclick="handleZipClick('${esc(path)}', event)">
                    <span class="icon">${icon}</span>
                    <span class="name">${esc(name)}</span>
                    <span class="size">${size}</span>
                    <span class="date">${date}</span>
                    <span class="tags-count">${tags}${gotoDirBtn}</span>
                </div>`;
            } else {
                const dblFnL = `openLightbox('${esc(path)}','${fileType(name)}')`;
                html += `<div class="list-row${multiSel}" data-path="${esc(path)}" onclick="selectFile('${esc(path)}', event)" ondblclick="${dblFnL}">
                    <span class="icon">${icon}</span>
                    <span class="name">${esc(name)}</span>
                    <span class="size">${size}</span>
                    <span class="date">${date}</span>
                    <span class="tags-count">${tags}${gotoDirBtn}</span>
                </div>`;
            }
        }
    }
    return html;
}

// ---------------------------------------------------------------------------
// Render: Content area
// ---------------------------------------------------------------------------

function renderContent() {
    const el = document.getElementById('content');

    // --- Zip directory mode ---
    if (state.mode === 'zip') {
        const entries = state.zipEntries;
        if (!entries.length) {
            el.className = '';
            el.innerHTML = `<div class="empty-state"><span class="empty-state-icon">🗜️</span><span class="empty-state-text">Empty archive</span></div>`;
            document.getElementById('entry-count').textContent = '0 files';
            return;
        }
        if (state.viewMode === 'grid') {
            el.className = 'file-grid';
            el.innerHTML = renderZipGrid(entries);
        } else {
            el.className = 'file-list';
            el.innerHTML = renderZipList(entries);
        }
        const images = entries.filter(e => e.is_image).length;
        document.getElementById('entry-count').textContent =
            `${entries.length} file${entries.length === 1 ? '' : 's'} (${images} image${images === 1 ? '' : 's'})`;
        return;
    }

    const items = state.mode === 'search' ? state.searchResults : state.entries;

    if (!items.length) {
        el.className = '';
        el.innerHTML = `<div class="empty-state">
            <span class="empty-state-icon">📂</span>
            <span class="empty-state-text">${state.mode === 'search' ? 'No results' : 'No files'}</span>
        </div>`;
        return;
    }

    // For search results, transform to match grid/list entry format
    const displayItems = state.mode === 'search'
        ? items.map(r => ({
            name: r.path.split('/').pop(),
            path: r.path,
            is_dir: false,
            size: null,
            mtime: null,
            tag_count: r.tags.length,
        }))
        : items;

    if (state.viewMode === 'grid') {
        el.className = 'file-grid';
        el.innerHTML = renderGrid(displayItems);
    } else {
        el.className = 'file-list';
        el.innerHTML = renderList(displayItems);
    }

    // Entry count
    const dirs = displayItems.filter(e => e.is_dir).length;
    const files = displayItems.filter(e => !e.is_dir).length;
    const parts = [];
    if (dirs > 0) parts.push(`${dirs} folder${dirs === 1 ? '' : 's'}`);
    if (files > 0) parts.push(`${files} file${files === 1 ? '' : 's'}`);
    document.getElementById('entry-count').textContent = parts.join(', ');
}

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
    state.mode = 'zip';
    state.zipPath = zipPath;
    state.selectedFile = null;
    state.selectedDir = null;
    state.selectedPaths.clear();
    state.selectedFilesData.clear();
    _lastClickedPath = null;
    _armedBulkTag = null;
    const data = await api('/api/zip/entries?' + new URLSearchParams({ path: zipPath }));
    state.zipEntries = data.entries || [];
    render();
}

async function refreshZipEntries() {
    if (!state.zipPath) return;
    const data = await api('/api/zip/entries?' + new URLSearchParams({ path: state.zipPath }));
    state.zipEntries = data.entries || [];
    renderContent();
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
            const thumbUrl = '/api/zip/thumb?' + new URLSearchParams({ path: state.zipPath, page: entry.image_index });
            preview = `<img src="${thumbUrl}" loading="lazy" alt="" data-name="${esc(displayName)}" onerror="_cardThumbError(this)">`;
        } else {
            preview = `<div class="card-icon">${fileIcon(displayName)}</div>`;
        }

        const tagBadge = entry.tag_count > 0
            ? `<span class="card-tag-count">${entry.tag_count}</span>` : '';
        const dblAttr = entry.is_image
            ? ` ondblclick="openComicViewer('${esc(state.zipPath)}', ${entry.image_index})"` : '';

        html += `<div class="card${selected}" data-path="${esc(dbPath)}"
            onclick="selectFile('${esc(dbPath)}', event)"${dblAttr}>
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
            ? ` ondblclick="openComicViewer('${esc(state.zipPath)}', ${entry.image_index})"` : '';
        html += `<div class="list-row${selected}" data-path="${esc(dbPath)}"
            onclick="selectFile('${esc(dbPath)}', event)"${dblAttr}>
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

function _cardThumbError(img) {
    // Fall back to generic file icon when card thumbnail fails to load
    const name = img.dataset.name || '';
    const wrap = img.closest('.card-preview');
    if (wrap) wrap.innerHTML = `<div class="card-icon">${fileIcon(name)}</div>`;
}

// ---------------------------------------------------------------------------
// Render: Detail panel
// ---------------------------------------------------------------------------

function renderDetail() {
    const panel = document.getElementById('detail');

    // Multi-select bulk panel
    if (state.selectedPaths.size > 1) {
        const count = state.selectedPaths.size;
        const bulkTags = aggregateBulkTags();
        const chipsHtml = renderBulkTagChips(bulkTags, count);
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
                <div id="bulk-status" class="bulk-status"></div>
            </div>`;
        attachTagAutocomplete(document.getElementById('bulk-tag-input'), () => doBulkAddTag());
        return;
    }

    if (!state.selectedFile && !state.selectedDir) {
        panel.innerHTML = '<div class="detail-empty">Select a file or folder to see details</div>';
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
    const previewUrl = '/preview/' + encodeURI(f.path);

    let preview;
    if (zipEntry) {
        // Entry inside a zip archive
        const entry = state.zipEntries.find(e => e.name === zipEntry.entryName);
        if (entry && entry.is_image && entry.image_index !== null) {
            const thumbUrl = '/api/zip/thumb?' + new URLSearchParams({ path: zipEntry.zipPath, page: entry.image_index });
            preview = `<a class="preview-zoomable" onclick="openComicViewer('${esc(zipEntry.zipPath)}', ${entry.image_index})" title="Click to open in viewer">` +
                      `<img src="${thumbUrl}" alt="${esc(name)}" onerror="_cardThumbError(this)"></a>`;
        } else {
            preview = `<div class="no-preview">${fileIcon(name)}</div>`;
        }
    } else if (type_ === 'image') {
        preview = `<a class="preview-zoomable" onclick="openLightbox('${esc(f.path)}','image')" title="Click to enlarge">` +
                  `<img src="${previewUrl}" alt="${esc(name)}" data-name="${esc(name)}"` +
                  ` onerror="_previewImgError(this)"></a>`;
    } else if (type_ === 'raw') {
        preview = `<a class="preview-zoomable" onclick="openLightbox('${esc(f.path)}','raw')" title="Click to enlarge">` +
                  `<img src="${previewUrl}" alt="${esc(name)}" data-name="${esc(name)}"` +
                  ` onerror="_previewRawError(this)"></a>`;
    } else if (type_ === 'audio') {
        preview = `<audio controls preload="metadata" src="${previewUrl}" ondblclick="openLightbox('${esc(f.path)}','audio')"></audio>`;
    } else if (type_ === 'video') {
        preview = `<video controls preload="metadata" src="${previewUrl}" data-name="${esc(name)}"` +
                  ` onclick="openLightbox('${esc(f.path)}','video')" style="cursor:zoom-in"` +
                  ` onerror="_previewVideoError(this)"></video>`;
    } else if (type_ === 'pdf') {
        preview = `<iframe class="preview-pdf" src="${previewUrl}" title="${esc(name)}"></iframe>` +
                  `<div style="text-align:center;padding:4px 0"><button class="tag-action-btn" onclick="openLightbox('${esc(f.path)}','pdf')">Full-size PDF</button></div>`;
    } else if (type_ === 'markdown') {
        preview = `<div class="preview-markdown" id="preview-md-content" ondblclick="openLightbox('${esc(f.path)}','markdown')"` +
                  ` title="Double-click to enlarge">Loading…</div>`;
    } else if (type_ === 'text') {
        preview = `<pre class="preview-text" id="preview-text-content" ondblclick="openLightbox('${esc(f.path)}','text')"` +
                  ` title="Double-click to enlarge">Loading…</pre>`;
    } else if (type_ === 'zip') {
        preview = `<div class="zip-cover-wrap">
            <img src="/thumb/${encodeURI(f.path)}" alt="${esc(name)}" class="zip-cover"
                 onerror="this.style.display='none'">
            <button class="tag-action-btn" onclick="openComicViewer('${esc(f.path)}')">Open comic viewer</button>
        </div>`;
    } else {
        preview = `<div class="no-preview">${fileIcon(name)}</div>`;
    }

    const tagChips = f.tags.length === 0
        ? '<span class="no-tags">No tags assigned</span>'
        : f.tags.map(t => {
            const tagStr = formatTag(t);
            const stateTag = state.tags.find(st => st.name === t.name);
            const chipColor = stateTag?.color ? ` style="border-left: 3px solid ${stateTag.color}"` : '';
            return `<span class="tag-chip"${chipColor}>${esc(tagStr)}<button class="remove" onclick="doRemoveTag('${esc(f.path)}','${esc(tagStr)}')">&times;</button></span>`;
        }).join('');

    panel.innerHTML = `
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
        <div class="detail-tags-section">
            <h4>Tags</h4>
            <div class="detail-tags">${tagChips}</div>
            <div class="tag-add-form">
                <input type="text" id="tag-input" placeholder="Add tag (e.g. genre/rock)">
                <button onclick="doAddTag()">Add</button>
            </div>
        </div>`;

    attachTagAutocomplete(document.getElementById('tag-input'), () => doAddTag());

    // Async-fetch text/markdown content after DOM is set
    if (type_ === 'text') {
        const el = document.getElementById('preview-text-content');
        if (el) {
            fetch(previewUrl).then(r => {
                if (!r.ok) throw new Error(r.statusText);
                return r.text();
            }).then(txt => {
                if (el) el.textContent = txt.length > 60000 ? txt.slice(0, 60000) + '\n…' : txt;
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
            return `<span class="tag-chip"${chipColor}>${esc(tagStr)}<button class="remove" onclick="doRemoveTag('${esc(f.path)}','${esc(tagStr)}')">&times;</button></span>`;
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
                <button class="bulk-chip-cancel" onclick="armBulkTag('${esc(tagStr)}')" title="Cancel">&#8617;</button>
                <button class="bulk-chip-fire" onclick="doBulkRemoveTagChip('${esc(tagStr)}')">Remove</button>
            </span>`;
        }
        return `<span class="bulk-chip"${chipBorder}>
            <span class="bulk-chip-label">${esc(tagStr)}${countBadge}</span>
            <button class="bulk-chip-arm" onclick="armBulkTag('${esc(tagStr)}')" title="Remove from selection">
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
    await Promise.all(paths.map(p => apiPost('/api/untag', { path: p, tags: [tagStr] })));
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
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

async function navigateTo(path) {
    state.selectedFile = null;
    state.selectedDir = null;
    state.selectedPaths.clear();
    state.selectedFilesData.clear();
    _lastClickedPath = null;
    _armedBulkTag = null;
    await loadFiles(path);
    render();
}


async function doSearch() {
    const input = document.getElementById('search-input');
    const query = input.value.trim();
    if (!query) return;
    await searchFiles(query);
    document.getElementById('search-clear').hidden = false;
    render();
}

function doClearSearch() {
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

async function selectFile(path, event) {
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
                const data = await api('/api/file?path=' + encodeURIComponent(path));
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
                const data = await api('/api/file?path=' + encodeURIComponent(p));
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
    render();
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
    renderContent();
    renderDetailTagsOnly();
    input.focus();
}

async function doRemoveTag(path, tagStr) {
    await removeTagFromFile(path, tagStr);
    renderTags();
    renderContent();
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

function clearSelection() {
    state.selectedPaths.clear();
    state.selectedFilesData.clear();
    state.selectedFile = null;
    state.selectedDir = null;
    _lastClickedPath = null;
    _armedBulkTag = null;
    render();
}

async function doBulkAddTag() {
    const input = document.getElementById('bulk-tag-input');
    const tagStr = input.value.trim();
    if (!tagStr) return;
    const paths = [...state.selectedPaths];
    const status = document.getElementById('bulk-status');
    status.textContent = 'Adding...';
    await Promise.all(paths.map(p => apiPost('/api/tag', { path: p, tags: [tagStr] })));
    // Refresh cached data for all selected files
    await Promise.all(paths.map(async p => {
        const data = await api('/api/file?path=' + encodeURIComponent(p));
        state.selectedFilesData.set(p, data);
    }));
    await loadTags();
    if (state.mode === 'browse') await loadFiles(state.currentPath);
    input.value = '';
    status.textContent = `Added "${tagStr}" to ${paths.length} file${paths.length === 1 ? '' : 's'}.`;
    renderTags();
    renderContent();
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

    const btn = document.getElementById('cache-clear-page-btn');
    btn.disabled = true;
    try {
        let body = null;
        if (!all) {
            const items = state.mode === 'search' ? state.searchResults : state.entries;
            const paths = (items || [])
                .filter(e => !e.is_dir && e.path)
                .map(e => e.path);
            body = JSON.stringify({ paths });
        }
        await fetch('/api/cache/clear', {
            method: 'POST',
            headers: body ? { 'Content-Type': 'application/json' } : {},
            body: body ?? undefined,
        });
    } catch (_) {
        // Reload regardless
    } finally {
        btn.disabled = false;
        if (state.mode === 'search') {
            await doSearch();
        } else {
            await loadFiles(state.currentPath);
        }
    }
}

function setViewMode(mode) {
    state.viewMode = mode;
    document.getElementById('view-grid').classList.toggle('active', mode === 'grid');
    document.getElementById('view-list').classList.toggle('active', mode === 'list');
    document.getElementById('zoom-slider').style.display = mode === 'grid' ? '' : 'none';
    renderContent();
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
    // Expand group on click
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
    state.detailOpen = false;
    document.querySelector('.layout').classList.add('detail-collapsed');
    document.getElementById('detail-toggle').classList.remove('active');
    render();
    restoreScrollAnchor(anchor);
}

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

function openLightbox(path, type) {
    const url = '/preview/' + encodeURI(path);
    const lb = document.getElementById('lightbox');
    const content = document.getElementById('lightbox-content');
    _lb.scale = 1; _lb.dx = 0; _lb.dy = 0; _lb.dragging = false;
    _lb.isImg = (type === 'image' || type === 'raw');

    let html = '';
    if (type === 'image' || type === 'raw') {
        html = `<img src="${url}" alt="${esc(path.split('/').pop())}"
                     onerror="this.replaceWith(Object.assign(document.createElement('p'),{textContent:'Preview unavailable',className:'lightbox-error'}))">`;
    } else if (type === 'video') {
        html = `<video controls autoplay src="${url}"
                       onerror="this.replaceWith(Object.assign(document.createElement('p'),{textContent:'Cannot play this video format',className:'lightbox-error'}))"></video>`;
    } else if (type === 'audio') {
        html = `<audio controls autoplay src="${url}"></audio>`;
    } else if (type === 'pdf') {
        html = `<iframe class="lightbox-pdf" src="${url}" title="${esc(path.split('/').pop())}"></iframe>`;
    } else if (type === 'text' || type === 'markdown') {
        html = `<pre class="lightbox-text" id="lightbox-text-pre">Loading…</pre>`;
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
        fetch(url).then(r => r.text()).then(txt => {
            const el = document.getElementById('lightbox-text-pre');
            if (el) el.textContent = txt;
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
    lb.hidden = true;
    document.getElementById('lightbox-content').innerHTML = '';
    // Remove leftover hint if any
    lb.querySelectorAll('.lightbox-hint').forEach(h => h.remove());
}

function _lightboxKeyHandler(e) {
    if (e.key === 'Escape') {
        _lbDetachZoom();
        const lb = document.getElementById('lightbox');
        lb.hidden = true;
        document.getElementById('lightbox-content').innerHTML = '';
        lb.querySelectorAll('.lightbox-hint').forEach(h => h.remove());
    } else {
        document.addEventListener('keydown', _lightboxKeyHandler, { once: true });
    }
}

// Scroll anchor helpers — keep an element at the same viewport-Y across reflows
// ---------------------------------------------------------------------------

function saveScrollAnchor(path) {
    const content = document.getElementById('content');
    if (path) {
        const el = content.querySelector(`[data-path="${CSS.escape(path)}"]`);
        if (el) return { path, top: el.getBoundingClientRect().top };
    }
    // fallback: proportional position
    return { ratio: content.scrollHeight > 0 ? content.scrollTop / content.scrollHeight : 0 };
}

function restoreScrollAnchor(anchor) {
    const content = document.getElementById('content');
    requestAnimationFrame(() => {
        if (anchor.path) {
            const el = content.querySelector(`[data-path="${CSS.escape(anchor.path)}"]`);
            if (el) {
                content.scrollTop += el.getBoundingClientRect().top - anchor.top;
                return;
            }
        }
        content.scrollTop = anchor.ratio * content.scrollHeight;
    });
}

// ---------------------------------------------------------------------------
// Comic viewer
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
    scrollHeight: 90, // image height in vh in horizontal scroll mode
    zoom: 1,
    panX: 0,
    panY: 0,
};

async function openComicViewer(path, startPage = 0) {
    const overlay = document.getElementById('comic-viewer');
    overlay.hidden = false;

    _cv.path = path;
    _cv.current = startPage;
    _cv.pages = [];

    document.getElementById('cv-status').textContent = 'Loading…';
    document.getElementById('cv-pages').innerHTML = '';

    const res = await fetch('/api/zip/pages?' + new URLSearchParams({ path }));
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

function closeComicViewer() {
    if (document.fullscreenElement) document.exitFullscreen();
    if (_cv.scroll) cvExitScrollView();
    document.getElementById('comic-viewer').hidden = true;
    document.removeEventListener('keydown', _cvKeyHandler);
    _cv.path = null; _cv.pages = []; _cv.current = 0;
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
        const url = `/api/zip/thumb?${new URLSearchParams({ path: _cv.path, page: i })}`;
        cell.innerHTML = `<img src="${url}" loading="lazy" alt="page ${i + 1}">` +
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
// Comic viewer – vertical + horizontal scroll mode
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

function cvApplyScrollZoom(newSize) {
    const stage = document.getElementById('cv-stage');
    const btn   = document.getElementById('cv-zoom-reset-btn');
    if (_cv.scrollDir === 'h') {
        if (newSize !== undefined) _cv.scrollHeight = Math.max(20, Math.min(200, newSize));
        if (stage) stage.style.setProperty('--cv-scroll-height', `${_cv.scrollHeight}vh`);
        if (btn) { btn.textContent = Math.round(_cv.scrollHeight) + 'vh'; btn.style.display = _cv.scrollHeight === 90 ? 'none' : ''; }
    } else {
        if (newSize !== undefined) _cv.scrollWidth = Math.max(20, Math.min(300, newSize));
        if (stage) stage.style.setProperty('--cv-scroll-width', `${_cv.scrollWidth}%`);
        if (btn) { btn.textContent = Math.round(_cv.scrollWidth) + '%'; btn.style.display = _cv.scrollWidth === 100 ? 'none' : ''; }
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
        const url = `/api/zip/page?${new URLSearchParams({ path: _cv.path, page: i })}`;
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
    _cv.scrollWidth = 100; _cv.scrollHeight = 90;
    cvShowPage(_cv.current);
}

// ---------------------------------------------------------------------------
// Comic viewer – zoom / pan
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
        btn.style.display = _cv.zoom === 1 ? 'none' : '';
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

    // Wheel: zoom towards cursor (or scroll width in scroll mode with Ctrl held)
    stage.addEventListener('wheel', e => {
        if (document.getElementById('comic-viewer').hidden) return;
        if (_cv.scroll) {
            if (e.ctrlKey || e.metaKey) {
                e.preventDefault();
                const factor = e.deltaY < 0 ? 1.15 : 1 / 1.15;
                const cur = _cv.scrollDir === 'h' ? _cv.scrollHeight : _cv.scrollWidth;
                cvApplyScrollZoom(cur * factor);
            }
            // Otherwise: let the browser scroll natively
            return;
        }
        e.preventDefault();
        const rect  = stage.getBoundingClientRect();
        const ox = e.clientX - rect.left  - rect.width  / 2;
        const oy = e.clientY - rect.top   - rect.height / 2;
        const factor = e.deltaY < 0 ? 1.15 : 1 / 1.15;
        cvZoomTo(_cv.zoom * factor, ox, oy);
    }, { passive: false });

    // Mousedown: start drag when zoomed (not in scroll mode)
    stage.addEventListener('mousedown', e => {
        if (document.getElementById('comic-viewer').hidden) return;
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

    // Double-click: zoom to 2× at cursor, or reset if already zoomed
    stage.addEventListener('dblclick', e => {
        if (document.getElementById('comic-viewer').hidden) return;
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
    const overlay = document.getElementById('comic-viewer');
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
});

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
    const url1 = `/api/zip/page?${new URLSearchParams({ path: _cv.path, page: idx })}`;
    // In spread mode, RTL shows the next page to the LEFT of the current one
    const url2 = _cv.spread && idx + 1 < _cv.pages.length
        ? `/api/zip/page?${new URLSearchParams({ path: _cv.path, page: idx + 1 })}`
        : null;

    cvResetZoom();
    // RTL spread: right page (current) first, then left page (next) to its left
    let html;
    if (_cv.rtl && url2) {
        html = `<img class="cv-page" src="${url2}" alt="page ${idx + 2}">` +
               `<img class="cv-page" src="${url1}" alt="page ${idx + 1}">`;
    } else {
        html = `<img class="cv-page" src="${url1}" alt="page ${idx + 1}">`;
        if (url2) html += `<img class="cv-page" src="${url2}" alt="page ${idx + 2}">`;
    }
    container.innerHTML = html;

    const total = _cv.spread
        ? `${idx + 1}${url2 ? '–' + (idx + 2) : ''} / ${_cv.pages.length}`
        : `${idx + 1} / ${_cv.pages.length}`;
    document.getElementById('cv-status').textContent = total;
    cvUpdateThumbActive(idx);
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
    if (document.getElementById('comic-viewer').hidden) return;
    // ArrowRight = forward in reading direction (RTL: lower index; LTR: higher index)
    if (e.key === 'ArrowRight' || e.key === 'ArrowDown' || (e.key === ' ' && !_cv.scroll)) {
        e.preventDefault(); _cv.rtl ? cvPrev() : cvNext();
    } else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') {
        e.preventDefault(); _cv.rtl ? cvNext() : cvPrev();
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
        else { closeComicViewer(); }
    }
}

let _cvClickNavTimer = null;

function cvClickNav(e) {
    if (_cv.scroll) return;  // no click-nav in scroll mode
    if (_cv.zoom > 1 || _cvDrag.moved) return;
    const x = e.clientX / window.innerWidth;
    let action = null;
    if (_cv.rtl) {
        if (x > 0.7) action = 'next';
        else if (x < 0.3) action = 'prev';
    } else {
        if (x < 0.3) action = 'prev';
        else if (x > 0.7) action = 'next';
    }
    if (!action) return;

    // Delay so a double-click fires dblclick (zoom) rather than two nav steps
    if (_cvClickNavTimer) { clearTimeout(_cvClickNavTimer); _cvClickNavTimer = null; return; }
    _cvClickNavTimer = setTimeout(() => {
        _cvClickNavTimer = null;
        if (action === 'next') cvNext(); else cvPrev();
    }, 220);
}

// ---------------------------------------------------------------------------
// Escape HTML
// ---------------------------------------------------------------------------

function esc(s) {
    if (!s) return '';
    return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
            .replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}

// ---------------------------------------------------------------------------
// Event binding
// ---------------------------------------------------------------------------

document.addEventListener('DOMContentLoaded', async () => {
    // Search
    document.getElementById('search-btn').addEventListener('click', doSearch);
    document.getElementById('search-input').addEventListener('keydown', e => {
        if (e.key === 'Enter') doSearch();
    });
    document.getElementById('search-clear').addEventListener('click', doClearSearch);

    // View toggle
    document.getElementById('view-grid').addEventListener('click', () => setViewMode('grid'));
    document.getElementById('view-list').addEventListener('click', () => setViewMode('list'));

    // Zoom slider
    const zoom = document.getElementById('zoom-slider');
    zoom.addEventListener('input', () => setCardSize(zoom.value));
    setCardSize(zoom.value);

    // Close detail panel when clicking empty space in the content area
    document.getElementById('content').addEventListener('click', e => {
        if (e.target === e.currentTarget) closeDetail();
    });

    // Keyboard shortcuts
    document.addEventListener('keydown', e => {
        if (e.key === 'Escape') {
            if (state.selectedPaths.size > 1) { clearSelection(); }
            else if (state.selectedFile) { closeDetail(); }
            else if (state.mode === 'search') { doClearSearch(); }
        }
    });

    // Initial load: restore directory from sessionStorage if present (survives Cmd-R).
    const initialPath = sessionStorage.getItem('ft_path') || '';
    await Promise.all([loadInfo(), loadTags(), loadFiles(initialPath)]);
    render();

    // Comic viewer stage events (wheel, drag, pinch)
    _cvInitStageEvents();
});
