const TAG_COLORS = [
    '#ef4444', '#f97316', '#f59e0b', '#eab308', '#84cc16',
    '#22c55e', '#14b8a6', '#06b6d4', '#3b82f6', '#6366f1',
    '#8b5cf6', '#a855f7', '#d946ef', '#ec4899', '#f43f5e',
];

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

function colorDot(color) {
    if (color) {
        return `<span class="tag-color-dot" style="background:${color}"></span>`;
    }
    return `<span class="tag-color-dot tag-color-dot-empty"></span>`;
}

// ---------------------------------------------------------------------------
// Tag tree building and rendering (sidebar)
// ---------------------------------------------------------------------------

/**
 * Build a recursive tree from a flat tag list.
 * Returns Map<segment, TreeNode> where TreeNode = { segment, fullPath, tag, children }.
 */
function buildTagTree(tags) {
    const root = new Map();
    for (const tag of tags) {
        const parts = tag.name.split('/');
        let nodeMap = root;
        for (let i = 0; i < parts.length; i++) {
            const seg = parts[i];
            const fullPath = parts.slice(0, i + 1).join('/');
            if (!nodeMap.has(seg)) {
                nodeMap.set(seg, { segment: seg, fullPath, tag: null, children: new Map() });
            }
            const node = nodeMap.get(seg);
            if (i === parts.length - 1) node.tag = tag;
            nodeMap = node.children;
        }
    }
    return root;
}

function _nodeCount(node) {
    let n = node.tag ? node.tag.count : 0;
    for (const child of node.children.values()) n += _nodeCount(child);
    return n;
}

/** Returns true if this node or any descendant matches the filter string. */
function _nodeMatchesFilter(node, f) {
    if (node.fullPath.toLowerCase().includes(f)) return true;
    for (const child of node.children.values()) {
        if (_nodeMatchesFilter(child, f)) return true;
    }
    return false;
}

/** Wrap matching substring in a highlight span (safe, operates on escaped text). */
function _highlightMatch(text, f) {
    if (!f) return esc(text);
    const lower = text.toLowerCase();
    const idx = lower.indexOf(f);
    if (idx === -1) return esc(text);
    return esc(text.slice(0, idx))
        + '<mark class="tag-filter-match">' + esc(text.slice(idx, idx + f.length)) + '</mark>'
        + esc(text.slice(idx + f.length));
}

function _anyDescendantActive(nodeMap) {
    for (const node of nodeMap.values()) {
        if (node.tag && state.activeTags.has(node.fullPath)) return true;
        if (_anyDescendantActive(node.children)) return true;
    }
    return false;
}

/**
 * Render children of a node map as HTML.
 * depth=0: top-level; respects tagSortMode.
 * depth>0: inside a group; pure alphabetical by segment.
 */
function renderTagTreeNodes(nodeMap, depth) {
    const f = state.tagFilter.toLowerCase();
    let nodes = [...nodeMap.values()];
    if (f) {
        nodes = nodes.filter(n => _nodeMatchesFilter(n, f));
    }
    const mode = state.tagSortMode;
    if (mode === 'count') {
        nodes.sort((a, b) => _nodeCount(b) - _nodeCount(a) || a.segment.localeCompare(b.segment));
    } else if (depth === 0 && mode === 'groups-first' && !f) {
        nodes.sort((a, b) => {
            const ag = a.children.size > 0 ? 0 : 1;
            const bg = b.children.size > 0 ? 0 : 1;
            if (ag !== bg) return ag - bg;
            return a.segment.localeCompare(b.segment);
        });
    } else {
        nodes.sort((a, b) => a.segment.localeCompare(b.segment));
    }
    return nodes.map(n => renderTagTreeNode(n, depth)).join('');
}

function renderTagTreeNode(node, depth) {
    const { segment, fullPath, tag, children } = node;
    const hasChildren = children.size > 0;
    // Each sub-group level adds 12 px of left margin (accumulated through nesting).
    const marginStyle = depth > 0 ? ' style="margin-left:12px"' : '';

    // --- Leaf node ---
    if (!hasChildren) {
        if (!tag) return '';
        const f = state.tagFilter.toLowerCase();
        if (tag.has_values) return _renderKvNode(tag, segment, marginStyle, f);
        const active = state.activeTags.has(fullPath) ? ' active' : '';
        const synBadge = (tag.synonyms || []).length
            ? ` <span class="tag-synonym-badge" title="Synonyms: ${(tag.synonyms || []).map(esc).join(', ')}">&#8801;</span>` : '';
        const cls = depth === 0 ? 'tag-item tag-standalone' : 'tag-item';
        const check = active ? '<svg class="tag-check" viewBox="0 0 12 12" width="12" height="12"><polyline points="1.5,6 4.5,9.5 10.5,2.5" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>' : '<span class="tag-check-placeholder"></span>';
        return `<button class="${cls}${active}" onclick="toggleTagFilter('${jesc(fullPath)}')" oncontextmenu="showTagMenu(event,'${jesc(fullPath)}')">${check}${colorDot(tag.color)}${_highlightMatch(segment, f)}${synBadge} <span class="count">${tag.count}</span></button>`;
    }

    // --- Group node (has children; may also have a tag at this exact path) ---
    const f = state.tagFilter.toLowerCase();
    const totalCount = _nodeCount(node);
    const expanded = state.expandedGroups.has(fullPath) || (!!f && _anyDescendantActive(children));
    // When a filter is active, auto-expand groups that have matching children.
    const filterExpand = !!f;
    const expandedClass = (expanded || filterExpand) ? ' expanded' : '';
    const anyActive = (tag && state.activeTags.has(fullPath)) || _anyDescendantActive(children);
    const searchQuery = tag ? `${fullPath} or ${fullPath}/*` : `${fullPath}/*`;
    const groupActiveClass =
        (state.mode === 'search' && state.searchQuery === searchQuery) || anyActive
            ? ' active' : '';
    const groupColor = tag ? tag.color : null;
    const rootContextMenu = tag ? ` oncontextmenu="showTagMenu(event,'${jesc(fullPath)}')"` : '';
    const synBadge = tag && (tag.synonyms || []).length
        ? ` <span class="tag-synonym-badge" title="Synonyms: ${(tag.synonyms || []).map(esc).join(', ')}">&#8801;</span>` : '';
    const showOpen = expanded || filterExpand;
    return `<div class="tag-group"${marginStyle}>
        <div class="tag-group-label${groupActiveClass}${expandedClass}">
            <button class="tag-group-chevron" onclick="toggleTagGroup('${jesc(fullPath)}')" title="Expand/collapse">
                <svg class="chevron-icon" viewBox="0 0 12 12"><polyline points="2,3 6,8 10,3" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>
            </button>
            <button class="tag-group-name" onclick="doTagGroupSearch('${jesc(fullPath)}')"${rootContextMenu}>${colorDot(groupColor)}${_highlightMatch(segment, f)}${synBadge} <span class="count">${totalCount}</span></button>
        </div>
        <div class="tag-group-items${showOpen ? ' open' : ''}">
            ${showOpen ? renderTagTreeNodes(children, depth + 1) : ''}
        </div>
    </div>`;
}

/** Render a k=v tag as an expandable group-like element. */
function _renderKvNode(tag, segment, marginStyle, f = '') {
    const active = state.activeTags.has(tag.name) ? ' active' : '';
    const synBadge = (tag.synonyms || []).length
        ? ` <span class="tag-synonym-badge" title="Synonyms: ${(tag.synonyms || []).map(esc).join(', ')}">&#8801;</span>` : '';
    const kvKey = '\x01kv:' + tag.name;
    const expanded = state.expandedGroups.has(kvKey);
    const expandedClass = expanded ? ' expanded' : '';
    const values = state.kvValueCache[tag.name] || [];
    let html = `<div class="tag-group tag-kv-group"${marginStyle}>
        <div class="tag-group-label${expandedClass}${active}">
            <button class="tag-group-chevron" onclick="toggleKvExpand('${jesc(tag.name)}')" title="Show values">
                <svg class="chevron-icon" viewBox="0 0 12 12"><polyline points="2,3 6,8 10,3" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>
            </button>
            <button class="tag-group-name" onclick="toggleTagFilter('${jesc(tag.name)}')" oncontextmenu="showTagMenu(event,'${jesc(tag.name)}')">
                ${active ? '<svg class="tag-check" viewBox="0 0 12 12" width="12" height="12"><polyline points="1.5,6 4.5,9.5 10.5,2.5" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>' : '<span class="tag-check-placeholder"></span>'}${colorDot(tag.color)}${_highlightMatch(segment, f)}${synBadge} <span class="tag-kv-badge">k=v</span> <span class="count">${tag.count}</span>
            </button>
        </div>`;
    if (expanded) {
        html += `<div class="tag-group-items open">`;
        if (values.length) {
            for (const v of values) {
                const valFilter = `${tag.name}=${v.value}`;
                const valActive = state.activeTags.has(valFilter) ? ' active' : '';
                html += `<button class="tag-item tag-kv-value${valActive}"
                    onclick="toggleTagFilter('${jesc(valFilter)}')"
                    oncontextmenu="showKvValueMenu(event,'${jesc(tag.name)}','${jesc(v.value)}')"
                    title="${esc(tag.name)}=${esc(v.value)}">
                    <span class="tag-kv-eq">=</span>${esc(v.value)} <span class="count">${v.count}</span>
                </button>`;
            }
        } else {
            html += `<div class="tag-item-loading">Loading\u2026</div>`;
        }
        html += `</div>`;
    }
    html += `</div>`;
    return html;
}

function renderTags() {
    const el = document.getElementById('tag-list');
    if (!state.tags.length) {
        el.innerHTML = '<div class="empty-state"><span class="empty-state-text">No tags</span></div>';
        return;
    }

    // Update clear-button visibility for the tag search filter
    const tagSearchClear = document.getElementById('tag-search-clear');
    if (tagSearchClear) tagSearchClear.hidden = !state.tagFilter;

    const tree = buildTagTree(state.tags);
    const listHtml = renderTagTreeNodes(tree, 0);

    // Compact "Clear filters" bar — only shown when tags are active, takes minimal space
    const clearBar = state.activeTags.size > 0
        ? `<div class="active-filters-bar"><span class="active-filters-count">${state.activeTags.size} active</span><button class="active-filters-clear" onclick="clearTagFilters()">Clear</button></div>`
        : '';

    el.innerHTML = clearBar + (listHtml || (state.tagFilter
        ? `<div class="empty-state"><span class="empty-state-text">No tags match \u201c${esc(state.tagFilter)}\u201d</span></div>`
        : ''));
    renderSubjects();
}

/**
 * Render the subjects section below the tag list.
 * Each subject is a clickable button that triggers a `subject:name` search.
 * Subjects with a `/` separator are shown in a collapsible hierarchy.
 */
function renderSubjects() {
    const el = document.getElementById('subject-list');
    if (!el) return;
    if (!state.subjects || !state.subjects.length) {
        el.innerHTML = '';
        return;
    }

    // Build a prefix tree from subjects (reuse the same logic as tags).
    // state.subjects is [{ name, count }], same shape expected by buildTagTree
    // except without color/synonyms/has_values.  We adapt by remapping.
    const subjectTags = state.subjects.map(s => ({
        name: s.name,
        count: s.count,
        color: null,
        synonyms: [],
        has_values: false,
    }));
    const tree = buildTagTree(subjectTags);

    let html = `<div class="subject-section-header">Subjects</div>`;
    html += renderSubjectTreeNodes(tree, 0);
    el.innerHTML = html;
}

function renderSubjectTreeNodes(nodes, depth) {
    return nodes.map(n => renderSubjectTreeNode(n, depth)).join('');
}

function renderSubjectTreeNode(node, depth) {
    const { segment, fullPath, tag, children } = node;
    const marginStyle = depth > 0 ? ` style="margin-left:${depth * 12}px"` : '';
    const isActive = state.mode === 'search' && state.searchQuery === ('subject:' + fullPath);

    if (!children.length) {
        // Leaf node
        const activeClass = isActive ? ' active' : '';
        const count = tag ? ` <span class="count">${tag.count}</span>` : '';
        return `<button class="subject-item${activeClass}"${marginStyle}
            onclick="doSubjectSearch('${jesc(fullPath)}')"
            title="subject:${esc(fullPath)}">${esc(segment)}${count}</button>`;
    }

    // Group node
    const groupKey = '\x01subj:' + fullPath;
    const expanded = state.expandedGroups.has(groupKey);
    const expandedClass = expanded ? ' expanded' : '';
    const activeClass = isActive ? ' active' : '';
    const totalCount = tag
        ? tag.count
        : children.reduce((acc, c) => acc + (c.tag ? c.tag.count : 0), 0);
    return `<div class="tag-group subject-group"${marginStyle}>
        <div class="tag-group-label${expandedClass}${activeClass}">
            <button class="tag-group-chevron" onclick="toggleSubjectGroup('${jesc(fullPath)}')" title="Expand/collapse">
                <svg class="chevron-icon" viewBox="0 0 12 12"><polyline points="2,3 6,8 10,3" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>
            </button>
            <button class="tag-group-name" onclick="doSubjectSearch('${jesc(fullPath)}')">${esc(segment)} <span class="count">${totalCount}</span></button>
        </div>
        <div class="tag-group-items${expanded ? ' open' : ''}">
            ${expanded ? renderSubjectTreeNodes(children, depth + 1) : ''}
        </div>
    </div>`;
}

function toggleSubjectGroup(fullPath) {
    const key = '\x01subj:' + fullPath;
    if (state.expandedGroups.has(key)) {
        state.expandedGroups.delete(key);
    } else {
        state.expandedGroups.add(key);
    }
    renderSubjects();
}

function doSubjectSearch(subject) {
    state.searchQuery = 'subject:' + subject;
    searchFiles(state.searchQuery).then(() => render());
}

async function toggleKvExpand(tagName) {
    const kvKey = '\x01kv:' + tagName;
    if (state.expandedGroups.has(kvKey)) {
        state.expandedGroups.delete(kvKey);
        renderTags();
        return;
    }
    state.expandedGroups.add(kvKey);
    if (!state.kvValueCache[tagName]) {
        renderTags(); // show loading state
        try {
            const values = await api(
                '/api/tag-values?' + new URLSearchParams({ name: tagName }) + dirParam('&')
            );
            state.kvValueCache[tagName] = values;
        } catch (_) {
            state.kvValueCache[tagName] = [];
        }
    }
    renderTags();
}

// ---------------------------------------------------------------------------
// Tag context menu (right-click in sidebar)
// ---------------------------------------------------------------------------

function showTagMenu(e, tagName) {
    e.preventDefault();
    e.stopPropagation();
    closeTagMenu();

    const tag = state.tags.find(t => t.name === tagName);
    const currentColor = tag?.color || null;

    let swatches = TAG_COLORS.map(c => {
        const sel = c === currentColor ? ' selected' : '';
        return `<button class="tag-menu-swatch${sel}" style="background:${c}" onclick="setTagColor('${jesc(tagName)}','${c}')"></button>`;
    }).join('');
    const noSel = !currentColor ? ' selected' : '';
    swatches = `<button class="tag-menu-swatch tag-menu-swatch-none${noSel}" onclick="setTagColor('${jesc(tagName)}', null)" title="No color">\u2715</button>` + swatches;

    const menu = document.createElement('div');
    menu.id = 'tag-context-menu';
    menu.className = 'tag-context-menu';

    const synonyms = tag?.synonyms || [];
    const synonymRows = synonyms.map(a =>
        `<span class="tag-menu-synonym-row">${esc(a)}<button class="tag-menu-synonym-remove" onclick="removeSynonym('${jesc(tagName)}','${jesc(a)}')" title="Remove synonym">\u2715</button></span>`
    ).join('');

    menu.innerHTML = `
        <div class="tag-menu-header" id="tag-menu-header">${esc(tagName)}</div>
        <div class="tag-menu-section">
            <div class="tag-menu-label">Color</div>
            <div class="tag-menu-swatches">${swatches}</div>
        </div>
        <div class="tag-menu-divider"></div>
        <div class="tag-menu-section">
            <div class="tag-menu-label">Synonyms${synonyms.length ? '' : ' <span style="font-weight:normal;opacity:.6">(none)</span>'}</div>
            ${synonymRows ? `<div class="tag-menu-synonyms">${synonymRows}</div>` : ''}
            <div class="tag-menu-synonym-add">
                <input id="tag-menu-synonym-input" class="tag-menu-rename-input" type="text" placeholder="Add alias\u2026" onclick="event.stopPropagation()">
                <button class="tag-menu-action" onclick="addSynonymFromInput('${jesc(tagName)}')">Add</button>
            </div>
        </div>
        <div class="tag-menu-divider"></div>
        <button class="tag-menu-action" onclick="startTagRename('${jesc(tagName)}')">Rename\u2026</button>
        <button class="tag-menu-action" onclick="closeTagMenu(); showTagManager('${jesc(tagName)}')">Manage tag\u2026</button>
        <button class="tag-menu-action tag-menu-delete" onclick="deleteTag('${jesc(tagName)}')">Delete tag</button>
    `;
    document.body.appendChild(menu);

    const rect = menu.getBoundingClientRect();
    let x = e.clientX;
    let y = e.clientY;
    if (x + rect.width > window.innerWidth) x = window.innerWidth - rect.width - 8;
    if (y + rect.height > window.innerHeight) y = window.innerHeight - rect.height - 8;
    menu.style.left = x + 'px';
    menu.style.top = y + 'px';

    requestAnimationFrame(() => {
        document.addEventListener('click', closeTagMenu, { once: true });
    });
}

function showKvValueMenu(e, tagName, value) {
    e.preventDefault();
    e.stopPropagation();
    closeTagMenu();

    const menu = document.createElement('div');
    menu.id = 'tag-context-menu';
    menu.className = 'tag-context-menu';
    menu.innerHTML = `
        <div class="tag-menu-header">${esc(tagName)}=<strong>${esc(value)}</strong></div>
        <button class="tag-menu-action" onclick="startKvValueRename('${jesc(tagName)}','${jesc(value)}')">Rename value\u2026</button>
        <button class="tag-menu-action tag-menu-delete" onclick="deleteKvValue('${jesc(tagName)}','${jesc(value)}')">Remove from all files</button>
    `;
    document.body.appendChild(menu);

    const rect = menu.getBoundingClientRect();
    let x = e.clientX;
    let y = e.clientY;
    if (x + rect.width > window.innerWidth) x = window.innerWidth - rect.width - 8;
    if (y + rect.height > window.innerHeight) y = window.innerHeight - rect.height - 8;
    menu.style.left = x + 'px';
    menu.style.top = y + 'px';

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
    await apiPost('/api/tag-color', { name: tagName, color, dir: currentAbsDir() });
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
    await apiPost('/api/delete-tag', { name: tagName, dir: currentAbsDir() });
    await loadTags();
    if (state.selectedFile) await loadFileDetail(state.selectedFile.path);
    render();
}

function startTagRename(tagName) {
    const header = document.getElementById('tag-menu-header');
    if (!header) return;

    const input = document.createElement('input');
    input.type = 'text';
    input.value = tagName;
    input.className = 'tag-menu-rename-input';
    input.onclick = e => e.stopPropagation();
    input.onkeydown = async e => {
        if (e.key === 'Enter') {
            await renameTag(tagName, input.value.trim());
        } else if (e.key === 'Escape') {
            closeTagMenu();
        }
    };

    header.replaceWith(input);
    input.id = 'tag-menu-header';
    input.select();

    document.removeEventListener('click', closeTagMenu);
    requestAnimationFrame(() => {
        document.addEventListener('click', closeTagMenu, { once: true });
    });
}

async function renameTag(oldName, newName) {
    if (!newName || newName === oldName) { closeTagMenu(); return; }
    closeTagMenu();
    const res = await apiPost('/api/rename-tag', { name: oldName, new_name: newName, dir: currentAbsDir() });
    if (res && res.merged) {
        showToast(`Tags merged into "${newName}".`);
    }
    await loadTags();
    if (state.selectedFile) await loadFileDetail(state.selectedFile.path);
    render();
}

async function addSynonymFromInput(canonical) {
    const input = document.getElementById('tag-menu-synonym-input');
    const alias = input ? input.value.trim() : '';
    if (!alias) return;
    closeTagMenu();
    try {
        await apiPost('/api/synonym/add', { alias, canonical, dir: currentAbsDir() });
        showToast(`Added synonym "${alias}" \u2192 "${canonical}".`);
    } catch (e) {
        alert(`Could not add synonym: ${e.message || e}`);
    }
    await loadTags();
    render();
}

async function removeSynonym(canonical, alias) {
    closeTagMenu();
    await apiPost('/api/synonym/remove', { alias, dir: currentAbsDir() });
    showToast(`Removed synonym "${alias}".`);
    await loadTags();
    render();
}

function startKvValueRename(tagName, oldValue) {
    closeTagMenu();
    const newValue = prompt(`Rename value "${oldValue}" for tag "${tagName}" to:`, oldValue);
    if (!newValue || newValue === oldValue) return;
    renameTag(`${tagName}=${oldValue}`, `${tagName}=${newValue}`);
}

async function deleteKvValue(tagName, value) {
    closeTagMenu();
    if (!confirm(`Remove "${tagName}=${value}" from all files?`)) return;
    await apiPost('/api/delete-tag', { name: `${tagName}=${value}`, dir: currentAbsDir() });
    delete state.kvValueCache[tagName];
    await loadTags();
    if (state.selectedFile) await loadFileDetail(state.selectedFile.path);
    render();
}

// ---------------------------------------------------------------------------
// Tag Manager modal
// ---------------------------------------------------------------------------

let _tmSelectedTag = null;
let _tmSearchQuery = '';
let _tmCollapsedGroups = new Set();

function tmToggleGroup(prefix) {
    if (_tmCollapsedGroups.has(prefix)) {
        _tmCollapsedGroups.delete(prefix);
    } else {
        _tmCollapsedGroups.add(prefix);
    }
    renderTmList();
}

function showTagManager(selectTag) {
    if (document.getElementById('tag-manager-overlay')) return;

    const overlay = document.createElement('div');
    overlay.id = 'tag-manager-overlay';
    overlay.className = 'tm-overlay';
    overlay.innerHTML = `
        <div class="tm-modal" onclick="event.stopPropagation()">
            <div class="tm-header">
                <span class="tm-title">Tag Manager</span>
                <button class="tm-prune-btn" onclick="pruneUnusedTags()" title="Remove all tags with no file assignments">Prune unused</button>
                <button class="tm-close" onclick="closeTagManager()" title="Close">\u2715</button>
            </div>
            <div class="tm-search-row">
                <input id="tm-search" class="tm-search-input" type="text"
                    placeholder="Filter tags\u2026" oninput="tmSearch(this.value)"
                    onkeydown="if(event.key==='Escape') closeTagManager()">
            </div>
            <div class="tm-body">
                <div class="tm-list" id="tm-list"></div>
                <div class="tm-detail" id="tm-detail">
                    <div class="tm-detail-placeholder">Select a tag to edit it.</div>
                </div>
            </div>
        </div>
    `;
    overlay.addEventListener('click', closeTagManager);
    document.body.appendChild(overlay);

    _tmSelectedTag = selectTag || null;
    _tmSearchQuery = '';
    _tmCollapsedGroups = new Set();
    renderTmList();
    if (_tmSelectedTag) renderTmDetail(_tmSelectedTag);

    requestAnimationFrame(() => {
        document.getElementById('tm-search')?.focus();
    });
}

function closeTagManager() {
    const el = document.getElementById('tag-manager-overlay');
    if (el) el.remove();
    _tmSelectedTag = null;
}

async function pruneUnusedTags() {
    if (!confirm('Remove all tags that have no file assignments?')) return;
    try {
        const res = await apiPost('/api/prune-tags', { dir: currentAbsDir() });
        const n = res.removed ?? 0;
        if (n === 0) {
            showToast('No unused tags found.');
        } else {
            showToast(`Removed ${n} unused tag${n === 1 ? '' : 's'}.`);
            await loadTags();
            renderTmList();
        }
    } catch (e) {
        showToast('Error: ' + e.message, 'error');
    }
}

function tmSearch(q) {
    _tmSearchQuery = q.toLowerCase();
    renderTmList();
}

function renderTmList() {
    const el = document.getElementById('tm-list');
    if (!el) return;

    const q = _tmSearchQuery;
    const filtered = q
        ? state.tags.filter(t => t.name.toLowerCase().includes(q))
        : state.tags;

    if (!filtered.length) {
        el.innerHTML = '<div class="tm-empty">No tags found.</div>';
        return;
    }

    // Group by hierarchy prefix
    const groups = {};
    const standalone = [];
    for (const tag of filtered) {
        const slash = tag.name.indexOf('/');
        if (slash > 0) {
            const prefix = tag.name.slice(0, slash);
            if (!groups[prefix]) groups[prefix] = { root: null, children: [] };
            groups[prefix].children.push(tag);
        } else {
            standalone.push(tag);
        }
    }
    for (const tag of standalone) {
        if (groups[tag.name] && !groups[tag.name].root) {
            groups[tag.name].root = tag;
        }
    }
    const standaloneOnly = standalone.filter(t => !groups[t.name]);

    let html = '';

    for (const prefix of Object.keys(groups).sort()) {
        const { root, children } = groups[prefix];
        const totalCount = children.reduce((s, c) => s + c.count, 0) + (root?.count || 0);
        const rootSel = _tmSelectedTag === prefix ? ' selected' : '';
        // When searching or a child is selected, always expand.
        const hasSelectedChild = children.some(c => c.name === _tmSelectedTag);
        const collapsed = _tmCollapsedGroups.has(prefix) && !q && !hasSelectedChild;
        const chevronClass = collapsed ? 'tm-chevron' : 'tm-chevron tm-chevron-open';
        html += `<div class="tm-group">
            <div class="tm-group-header${rootSel}">
                <button class="${chevronClass}" onclick="tmToggleGroup('${jesc(prefix)}')" title="Expand/collapse">
                    <svg viewBox="0 0 12 12" width="12" height="12"><polyline points="2,3 6,8 10,3" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>
                </button>
                <span class="tm-group-label" onclick="tmSelectTag('${jesc(prefix)}')">
                    ${colorDot(root?.color || null)}<span class="tm-group-name">${esc(prefix)}</span>
                    <span class="tm-count">${totalCount}</span>
                </span>
            </div>`;
        if (!collapsed) {
            for (const child of children.sort((a, b) => a.name.localeCompare(b.name))) {
                const suffix = child.name.slice(prefix.length + 1);
                const sel = _tmSelectedTag === child.name ? ' selected' : '';
                const synBadge = (child.synonyms || []).length ? ` <span class="tm-syn-badge">\u2261</span>` : '';
                const kvBadge = child.has_values ? ` <span class="tm-kv-badge">k=v</span>` : '';
                html += `<div class="tm-tag-row tm-tag-child${sel}" onclick="tmSelectTag('${jesc(child.name)}')">
                    ${colorDot(child.color)}${esc(suffix)}${kvBadge}${synBadge}
                    <span class="tm-count">${child.count}</span>
                </div>`;
            }
        }
        html += `</div>`;
    }

    for (const tag of standaloneOnly.sort((a, b) => a.name.localeCompare(b.name))) {
        const sel = _tmSelectedTag === tag.name ? ' selected' : '';
        const synBadge = (tag.synonyms || []).length ? ` <span class="tm-syn-badge">\u2261</span>` : '';
        const kvBadge = tag.has_values ? ` <span class="tm-kv-badge">k=v</span>` : '';
        html += `<div class="tm-tag-row tm-tag-standalone${sel}" onclick="tmSelectTag('${jesc(tag.name)}')">
            ${colorDot(tag.color)}${esc(tag.name)}${kvBadge}${synBadge}
            <span class="tm-count">${tag.count}</span>
        </div>`;
    }

    el.innerHTML = html;
}

async function tmSelectTag(name) {
    _tmSelectedTag = name;
    renderTmList();
    await renderTmDetail(name);
}

async function renderTmDetail(name) {
    const panel = document.getElementById('tm-detail');
    if (!panel) return;

    const tag = state.tags.find(t => t.name === name);
    if (!tag) {
        // Could be a group prefix that has no root tag of its own — show a
        // group summary with rename-prefix and child list.
        const children = state.tags.filter(t => t.name.startsWith(name + '/'));
        if (children.length) {
            const totalCount = children.reduce((s, c) => s + c.count, 0);
            const childRows = children.sort((a, b) => a.name.localeCompare(b.name)).map(c =>
                `<div class="tm-val-row">
                    <span class="tm-val-name" onclick="tmSelectTag('${jesc(c.name)}')">${esc(c.name.slice(name.length + 1))}</span>
                    <span class="tm-val-count">${c.count}</span>
                </div>`
            ).join('');
            panel.innerHTML = `
                <div class="tm-detail-header">
                    <div class="tm-detail-name">${esc(name)}/</div>
                    <div class="tm-detail-meta">${children.length} child tag${children.length !== 1 ? 's' : ''} \u00b7 ${totalCount} file${totalCount !== 1 ? 's' : ''} total</div>
                </div>
                <section class="tm-section">
                    <div class="tm-section-title">Child tags</div>
                    <div class="tm-val-list">${childRows}</div>
                </section>
                <section class="tm-section tm-ops">
                    <div class="tm-section-title">Operations</div>
                    <div class="tm-op-row">
                        <label class="tm-op-label">Rename group prefix to</label>
                        <div class="tm-op-inputs">
                            <input id="tm-rename-input" class="tm-input" type="text" value="${esc(name)}"
                                placeholder="New prefix\u2026"
                                onkeydown="if(event.key==='Enter') tmDoRenamePrefix('${jesc(name)}')">
                            <button class="tm-btn" onclick="tmDoRenamePrefix('${jesc(name)}')">Rename</button>
                        </div>
                        <div class="tm-op-hint">Renames all <em>${esc(name)}/*</em> tags to use the new prefix.</div>
                    </div>
                </section>
            `;
            return;
        }
        panel.innerHTML = `<div class="tm-detail-placeholder">Tag not found.</div>`;
        return;
    }

    // Load k/v values if needed
    let values = [];
    if (tag.has_values) {
        if (state.kvValueCache[name]) {
            values = state.kvValueCache[name];
        } else {
            try {
                values = await api('/api/tag-values?' + new URLSearchParams({ name }) + dirParam('&'));
                state.kvValueCache[name] = values;
            } catch (_) { /* ignore */ }
        }
    }

    const currentColor = tag.color || null;
    let swatches = TAG_COLORS.map(c => {
        const sel = c === currentColor ? ' selected' : '';
        return `<button class="tag-menu-swatch${sel}" style="background:${c}"
            onclick="tmSetColor('${jesc(name)}','${c}')" title="${c}"></button>`;
    }).join('');
    const noSel = !currentColor ? ' selected' : '';
    swatches = `<button class="tag-menu-swatch tag-menu-swatch-none${noSel}"
        onclick="tmSetColor('${jesc(name)}',null)" title="No color">\u2715</button>` + swatches;

    const synonyms = tag.synonyms || [];
    const synRows = synonyms.map(a => `
        <span class="tm-syn-row">
            <span class="tm-syn-name">${esc(a)}</span>
            <button class="tm-syn-remove" onclick="tmRemoveSynonym('${jesc(name)}','${jesc(a)}')" title="Remove">\u2715</button>
        </span>`).join('');

    const suffix = name.includes('/') ? name.slice(name.indexOf('/') + 1) : name;
    const defaultPrefix = name.includes('/') ? name.split('/')[0] : '';

    const valRows = values.map(v => `
        <div class="tm-val-row">
            <span class="tm-val-name" onclick="tmValueSearch('${jesc(name)}','${jesc(v.value)}')" title="Search ${esc(name)}=${esc(v.value)}">${esc(v.value)}</span>
            <span class="tm-val-count">${v.count}</span>
            <button class="tm-val-rename" onclick="tmRenameValue('${jesc(name)}','${jesc(v.value)}')" title="Rename value">\u270e</button>
        </div>`).join('');

    panel.innerHTML = `
        <div class="tm-detail-header">
            <div class="tm-detail-name">${colorDot(currentColor)}${esc(name)}</div>
            <div class="tm-detail-meta">${tag.count} file${tag.count !== 1 ? 's' : ''}${tag.has_values ? ' \u00b7 key=value tag' : ''}</div>
        </div>

        <section class="tm-section">
            <div class="tm-section-title">Color</div>
            <div class="tag-menu-swatches">${swatches}</div>
        </section>

        <section class="tm-section">
            <div class="tm-section-title">Synonyms</div>
            ${synonyms.length ? `<div class="tm-syn-list">${synRows}</div>` : `<div class="tm-empty-hint">No synonyms defined.</div>`}
            <div class="tm-syn-add">
                <input id="tm-syn-input" class="tm-input" type="text" placeholder="Add alias for this tag\u2026"
                    onkeydown="if(event.key==='Enter') tmAddSynonym('${jesc(name)}')">
                <button class="tm-btn" onclick="tmAddSynonym('${jesc(name)}')">Add</button>
            </div>
        </section>

        ${tag.has_values ? `
        <section class="tm-section">
            <div class="tm-section-title">Values <span class="tm-section-hint">(click to search)</span></div>
            ${values.length
                ? `<div class="tm-val-list">${valRows}</div>`
                : `<div class="tm-empty-hint">No values loaded.</div>`}
        </section>` : ''}

        <section class="tm-section tm-ops">
            <div class="tm-section-title">Operations</div>

            <div class="tm-op-row">
                <label class="tm-op-label">Rename to</label>
                <div class="tm-op-inputs">
                    <input id="tm-rename-input" class="tm-input" type="text" value="${esc(name)}"
                        placeholder="New name\u2026"
                        onkeydown="if(event.key==='Enter') tmDoRename('${jesc(name)}')">
                    <button class="tm-btn" onclick="tmDoRename('${jesc(name)}')">Rename</button>
                </div>
                <div class="tm-op-hint">Renaming to an existing tag merges them. Use <code>key=value</code> form to change tag style.</div>
            </div>

            <div class="tm-op-row">
                <label class="tm-op-label">Merge into</label>
                <div class="tm-op-inputs">
                    <input id="tm-merge-input" class="tm-input" type="text"
                        placeholder="Target tag name\u2026" list="tm-merge-datalist"
                        onkeydown="if(event.key==='Enter') tmDoMerge('${jesc(name)}')">
                    <datalist id="tm-merge-datalist">
                        ${state.tags.filter(t => t.name !== name).map(t => `<option value="${esc(t.name)}">`).join('')}
                    </datalist>
                    <button class="tm-btn tm-btn-warn" onclick="tmDoMerge('${jesc(name)}')">Merge</button>
                </div>
                <div class="tm-op-hint">All files tagged <em>${esc(name)}</em> will also receive the target tag.</div>
            </div>

            <div class="tm-op-row">
                <label class="tm-op-label">Move to group</label>
                <div class="tm-op-inputs">
                    <input id="tm-move-input" class="tm-input" type="text"
                        placeholder="Prefix (e.g. genre)" value="${esc(defaultPrefix)}">
                    <button class="tm-btn" onclick="tmDoMove('${jesc(name)}')">Move</button>
                </div>
                <div class="tm-op-hint">Renames to <em>prefix/${esc(suffix)}</em>.</div>
            </div>
        </section>

        <div class="tm-danger-zone">
            <button class="tm-btn tm-btn-danger" onclick="tmDeleteTag('${jesc(name)}')">
                Delete tag (${tag.count} assignment${tag.count !== 1 ? 's' : ''})
            </button>
        </div>
    `;
}

// Tag Manager operations

async function tmSetColor(name, color) {
    await apiPost('/api/tag-color', { name, color, dir: currentAbsDir() });
    await loadTags();
    renderTmList();
    await renderTmDetail(name);
    render();
}

async function tmAddSynonym(canonical) {
    const input = document.getElementById('tm-syn-input');
    const alias = input ? input.value.trim() : '';
    if (!alias) return;
    try {
        await apiPost('/api/synonym/add', { alias, canonical, dir: currentAbsDir() });
        showToast(`Added synonym "${alias}" \u2192 "${canonical}".`);
        if (input) input.value = '';
    } catch (e) {
        alert(`Could not add synonym: ${e.message || e}`);
        return;
    }
    await loadTags();
    renderTmList();
    await renderTmDetail(canonical);
}

async function tmRemoveSynonym(canonical, alias) {
    await apiPost('/api/synonym/remove', { alias, dir: currentAbsDir() });
    showToast(`Removed synonym "${alias}".`);
    await loadTags();
    renderTmList();
    await renderTmDetail(canonical);
}

async function tmDoRename(oldName) {
    const input = document.getElementById('tm-rename-input');
    const newName = input ? input.value.trim() : '';
    if (!newName || newName === oldName) return;
    const existingTarget = state.tags.find(t => t.name === newName);
    if (!confirm(`Rename "${oldName}" to "${newName}"?${existingTarget ? '\n\nTarget tag already exists \u2014 they will be merged.' : ''}`)) return;
    const res = await apiPost('/api/rename-tag', { name: oldName, new_name: newName, dir: currentAbsDir() });
    if (res && res.merged) showToast(`Merged into "${newName}".`);
    else showToast(`Renamed "${oldName}" to "${newName}".`);
    await loadTags();
    _tmSelectedTag = newName;
    renderTmList();
    await renderTmDetail(newName);
    if (state.selectedFile) await loadFileDetail(state.selectedFile.path);
    render();
}

async function tmDoMerge(sourceName) {
    const input = document.getElementById('tm-merge-input');
    const targetName = input ? input.value.trim() : '';
    if (!targetName || targetName === sourceName) return;
    const srcTag = state.tags.find(t => t.name === sourceName);
    if (!confirm(`Merge "${sourceName}" into "${targetName}"?\nThis will retag all ${srcTag?.count || 0} file(s) and remove "${sourceName}".`)) return;
    const res = await apiPost('/api/rename-tag', { name: sourceName, new_name: targetName, dir: currentAbsDir() });
    if (res && res.merged) showToast(`Merged "${sourceName}" into "${targetName}".`);
    else showToast(`Renamed and merged.`);
    await loadTags();
    _tmSelectedTag = targetName;
    renderTmList();
    await renderTmDetail(targetName);
    if (state.selectedFile) await loadFileDetail(state.selectedFile.path);
    render();
}

async function tmDoMove(tagName) {
    const input = document.getElementById('tm-move-input');
    const newPrefix = input ? input.value.trim() : '';
    if (!newPrefix) return;
    const suffix = tagName.includes('/') ? tagName.slice(tagName.indexOf('/') + 1) : tagName;
    const newName = `${newPrefix}/${suffix}`;
    if (newName === tagName) return;
    if (!confirm(`Rename "${tagName}" to "${newName}"?`)) return;
    const res = await apiPost('/api/rename-tag', { name: tagName, new_name: newName, dir: currentAbsDir() });
    if (res && res.merged) showToast(`Merged into "${newName}".`);
    else showToast(`Moved to "${newName}".`);
    await loadTags();
    _tmSelectedTag = newName;
    renderTmList();
    await renderTmDetail(newName);
    if (state.selectedFile) await loadFileDetail(state.selectedFile.path);
    render();
}

async function tmDoRenamePrefix(oldPrefix) {
    const input = document.getElementById('tm-rename-input');
    const newPrefix = input ? input.value.trim() : '';
    if (!newPrefix || newPrefix === oldPrefix) return;
    // Find all children and rename each one
    const children = state.tags.filter(t => t.name.startsWith(oldPrefix + '/'));
    if (!children.length) return;
    if (!confirm(`Rename group prefix "${oldPrefix}" to "${newPrefix}"?\nThis renames ${children.length} tag${children.length !== 1 ? 's' : ''}.`)) return;
    for (const child of children) {
        const suffix = child.name.slice(oldPrefix.length + 1);
        await apiPost('/api/rename-tag', {
            name: child.name,
            new_name: `${newPrefix}/${suffix}`,
            dir: currentAbsDir(),
        });
    }
    showToast(`Renamed group "${oldPrefix}" to "${newPrefix}".`);
    await loadTags();
    _tmSelectedTag = newPrefix;
    renderTmList();
    await renderTmDetail(newPrefix);
    if (state.selectedFile) await loadFileDetail(state.selectedFile.path);
    render();
}

async function tmDeleteTag(name) {
    const tag = state.tags.find(t => t.name === name);
    const count = tag?.count || 0;
    if (!confirm(`Delete tag "${name}"?\nThis removes it from ${count} file(s). This cannot be undone.`)) return;
    await apiPost('/api/delete-tag', { name, dir: currentAbsDir() });
    showToast(`Deleted "${name}".`);
    await loadTags();
    _tmSelectedTag = null;
    renderTmList();
    const panel = document.getElementById('tm-detail');
    if (panel) panel.innerHTML = `<div class="tm-detail-placeholder">Tag deleted.</div>`;
    if (state.selectedFile) await loadFileDetail(state.selectedFile.path);
    render();
}

function tmValueSearch(tagName, value) {
    closeTagManager();
    const query = `${tagName}=${value}`;
    document.getElementById('search-input').value = query;
    searchFiles(query).then(() => {
        document.getElementById('search-clear').hidden = false;
        render();
    });
}

async function tmRenameValue(tagName, oldValue) {
    const newValue = prompt(`Rename value "${oldValue}" (tag "${tagName}") to:`, oldValue);
    if (!newValue || newValue === oldValue) return;
    const res = await apiPost('/api/rename-tag', {
        name: `${tagName}=${oldValue}`,
        new_name: `${tagName}=${newValue}`,
        dir: currentAbsDir(),
    });
    if (res && res.merged) showToast(`Values merged.`);
    else showToast(`Value renamed.`);
    delete state.kvValueCache[tagName];
    await loadTags();
    renderTmList();
    await renderTmDetail(tagName);
    render();
}
