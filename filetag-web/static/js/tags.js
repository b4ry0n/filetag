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

        if (state.tagPickerMode) {
            const checked = state.tagPickerPicks.has(fullPath);
            const checkedCls = checked ? ' picker-checked' : '';
            const checkIcon = checked
                ? '<svg class="tag-check" viewBox="0 0 12 12" width="12" height="12"><polyline points="1.5,6 4.5,9.5 10.5,2.5" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>'
                : '<span class="tag-check-placeholder"></span>';
            return `<button class="${cls}${checkedCls}" draggable="true" ondragstart="tagDragStart(event,'${jesc(fullPath)}')" onclick="toggleTagPick('${jesc(fullPath)}')" oncontextmenu="showTagMenu(event,'${jesc(fullPath)}')" ondragover="tagDragOver(event)" ondragleave="tagDragLeave(event)" ondrop="tagDrop(event,'${jesc(fullPath)}')">${checkIcon}${colorDot(tag.color)}${_highlightMatch(segment, f)}${synBadge} <span class="count">${tag.count}</span></button>`;
        }

        const check = active ? '<svg class="tag-check" viewBox="0 0 12 12" width="12" height="12"><polyline points="1.5,6 4.5,9.5 10.5,2.5" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>' : '<span class="tag-check-placeholder"></span>';
        return `<button class="${cls}${active}" draggable="true" ondragstart="tagDragStart(event,'${jesc(fullPath)}')" onclick="toggleTagFilter('${jesc(fullPath)}')" oncontextmenu="showTagMenu(event,'${jesc(fullPath)}')" ondragover="tagDragOver(event)" ondragleave="tagDragLeave(event)" ondrop="tagDrop(event,'${jesc(fullPath)}')">${check}${colorDot(tag.color)}${_highlightMatch(segment, f)}${synBadge} <span class="count">${tag.count}</span></button>`;
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
    const groupNameClick = state.tagPickerMode
        ? `toggleTagGroupPick('${jesc(fullPath)}')`
        : `doTagGroupSearch('${jesc(fullPath)}')`;
    const groupPickedCls = state.tagPickerMode && _anyDescendantPicked(children) ? ' picker-checked' : '';
    const groupDrag = tag ? ` draggable="true" ondragstart="tagDragStart(event,'${jesc(fullPath)}')"` : '';
    return `<div class="tag-group"${marginStyle}>
        <div class="tag-group-label${groupActiveClass}${expandedClass}">
            <button class="tag-group-chevron" onclick="toggleTagGroup('${jesc(fullPath)}')" title="Expand/collapse">
                <svg class="chevron-icon" viewBox="0 0 12 12"><polyline points="2,3 6,8 10,3" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>
            </button>
            <button class="tag-group-name${groupPickedCls}" onclick="${groupNameClick}"${rootContextMenu}${groupDrag} ondragover="tagDragOver(event)" ondragleave="tagDragLeave(event)" ondrop="tagDrop(event,'${jesc(fullPath)}')">${colorDot(groupColor)}${_highlightMatch(segment, f)}${synBadge} <span class="count">${totalCount}</span></button>
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
            <button class="tag-group-name" draggable="true" ondragstart="tagDragStart(event,'${jesc(tag.name)}')" onclick="toggleTagFilter('${jesc(tag.name)}')" oncontextmenu="showTagMenu(event,'${jesc(tag.name)}')" ondragover="tagDragOver(event)" ondragleave="tagDragLeave(event)" ondrop="tagDrop(event,'${jesc(tag.name)}')">
                ${colorDot(tag.color)}${_highlightMatch(segment, f)}${synBadge} <span class="tag-kv-badge">k=v</span> <span class="count">${tag.count}</span>
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
    const hasPeople = typeof renderPeopleSection === 'function' && state.faceConfig != null;
    if (!state.tags.length && (!state.subjects || !state.subjects.length) && !hasPeople) {
        el.innerHTML = '<div class="empty-state"><span class="empty-state-text">No tags</span></div>';
        return;
    }

    // Update clear-button visibility for the tag search filter
    const tagSearchClear = document.getElementById('tag-search-clear');
    if (tagSearchClear) tagSearchClear.hidden = !state.tagFilter;

    const tree = buildTagTree(state.tags);
    const listHtml = state.tags.length ? renderTagTreeNodes(tree, 0) : '';
    const subjectsHtml = _renderSubjectsInline();

    if (state.tagPickerMode) {
        // In picker mode: show sticky apply/cancel bar at the bottom.
        const added   = [...state.tagPickerPicks].filter(t => !state.tagPickerOriginal.has(t)).length;
        const removed = [...state.tagPickerOriginal].filter(t => !state.tagPickerPicks.has(t)).length;
        const subjectChanged = state.tagPickerSubject !== state.tagPickerOriginalSubject;
        const delta = added + removed + (subjectChanged ? 1 : 0);
        const targets = state.selectedPaths.size > 0 ? state.selectedPaths.size : (state.selectedFile ? 1 : 0);
        const targetLabel = targets > 1 ? `${targets} files` : (state.selectedFile ? '1 file' : 'no file');
        const parts = [];
        if (added > 0) parts.push(`+${added}`);
        if (removed > 0) parts.push(`-${removed}`);
        if (subjectChanged) parts.push(state.tagPickerSubject ? `subject: ${state.tagPickerSubject}` : 'remove subject');
        const applyLabel = delta > 0
            ? `Apply (${parts.join(', ')}) to ${targetLabel}`
            : `No changes — ${targetLabel} selected`;
        const applyDisabled = delta === 0 || targets === 0 ? ' disabled' : '';
        const pickerBar = `<div class="tag-picker-bar">
            <button class="tag-picker-apply"${applyDisabled} onclick="applyTagPicker()">${applyLabel}</button>
            <button class="tag-picker-cancel" onclick="cancelTagPickerMode()">Cancel</button>
        </div>`;
        el.innerHTML = (listHtml || `<div class="empty-state"><span class="empty-state-text">No tags match</span></div>`)
            + subjectsHtml + pickerBar;
    } else {
        // Normal mode: compact "Clear filters" bar
        const clearBar = state.activeTags.size > 0
            ? `<div class="active-filters-bar"><span class="active-filters-count">${state.activeTags.size} active</span><button class="active-filters-clear" onclick="clearTagFilters()">Clear</button></div>`
            : '';
        const peopleHtml = typeof renderPeopleSection === 'function' ? renderPeopleSection() : '';
        el.innerHTML = clearBar + (listHtml || (state.tagFilter
            ? `<div class="empty-state"><span class="empty-state-text">No tags match \u201c${esc(state.tagFilter)}\u201d</span></div>`
            : '')) + subjectsHtml + peopleHtml;
    }
    // Clear the old separate subject container (no longer used for content).
    const subjEl = document.getElementById('subject-list');
    if (subjEl) subjEl.innerHTML = '';
}

/**
 * Render the subjects section as inline HTML to be appended to #tag-list.
 * Uses the same visual language as tags.
 */
function _renderSubjectsInline() {
    if (!state.subjects || !state.subjects.length) return '';
    const subjectTags = state.subjects.map(s => ({
        name: s.name, count: s.count, color: null, synonyms: [], has_values: false,
    }));
    const tree = buildTagTree(subjectTags);
    return `<div class="subjects-section-divider">Subjects</div>`
        + renderSubjectTreeNodes(tree, 0);
}

/**
 * @deprecated — kept so old callers don't break; renderTags now handles subjects inline.
 */
function renderSubjects() {
    renderTags();
}

function renderSubjectTreeNodes(nodeMap, depth) {
    const nodes = [...nodeMap.values()].sort((a, b) => a.segment.localeCompare(b.segment));
    return nodes.map(n => renderSubjectTreeNode(n, depth)).join('');
}

function renderSubjectTreeNode(node, depth) {
    const { segment, fullPath, tag, children } = node;
    const marginStyle = depth > 0 ? ` style="margin-left:${depth * 12}px"` : '';
    const isActive = state.mode === 'search' && state.searchQuery === ('subject:' + quoteTag(fullPath));

    if (!children.size) {
        // Leaf node
        const activeClass = isActive ? ' active' : '';
        const count = tag ? ` <span class="count">${tag.count}</span>` : '';
        const clickFn = state.tagPickerMode
            ? `toggleSubjectPick('${jesc(fullPath)}')`
            : `doSubjectSearch('${jesc(fullPath)}')`;
        // In picker mode: radio-style indicator (filled circle = selected).
        // In normal mode: no indicator (subjects have no checkmark column).
        const indicator = state.tagPickerMode
            ? (state.tagPickerSubject === fullPath
                ? '<svg class="tag-check" viewBox="0 0 12 12" width="12" height="12"><circle cx="6" cy="6" r="4" fill="currentColor"/></svg>'
                : '<span class="tag-check-placeholder"></span>')
            : '';
        const pickedCls = state.tagPickerMode && state.tagPickerSubject === fullPath ? ' picker-checked' : '';
        const subjectPadding = 22 + depth * 12;
        return `<button class="tag-item tag-subject-item${activeClass}${pickedCls}" style="padding-left:${subjectPadding}px"
            onclick="${clickFn}"
            ondragover="tagDragOver(event)" ondragleave="tagDragLeave(event)" ondrop="subjectDrop(event,'${jesc(fullPath)}')"
            title="subject:${esc(fullPath)}">${indicator}${esc(segment)}${count}</button>`;
    }

    // Group node
    const groupKey = '\x01subj:' + fullPath;
    const expanded = state.expandedGroups.has(groupKey);
    const expandedClass = expanded ? ' expanded' : '';
    const activeClass = isActive ? ' active' : '';
    const totalCount = tag
        ? tag.count
        : [...children.values()].reduce((acc, c) => acc + (c.tag ? c.tag.count : 0), 0);
    const groupClickFn = state.tagPickerMode
        ? `toggleSubjectPick('${jesc(fullPath)}')`
        : `doSubjectSearch('${jesc(fullPath)}')`;
    const groupPickedCls = state.tagPickerMode && state.tagPickerSubject === fullPath ? ' picker-checked' : '';
    return `<div class="tag-group subject-tree-group"${marginStyle}>
        <div class="tag-group-label${expandedClass}${activeClass}">
            <button class="tag-group-chevron" onclick="toggleSubjectGroup('${jesc(fullPath)}')" title="Expand/collapse">
                <svg class="chevron-icon" viewBox="0 0 12 12"><polyline points="2,3 6,8 10,3" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>
            </button>
            <button class="tag-group-name${groupPickedCls}" onclick="${groupClickFn}" ondragover="tagDragOver(event)" ondragleave="tagDragLeave(event)" ondrop="subjectDrop(event,'${jesc(fullPath)}')">${esc(segment)} <span class="count">${totalCount}</span></button>
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
    renderTags();
}

// In picker mode: radio-select a subject (click again to deselect).
function toggleSubjectPick(subjectName) {
    state.tagPickerSubject = state.tagPickerSubject === subjectName ? null : subjectName;
    renderTags();
}

async function doSubjectSearch(subject) {
    const q = 'subject:' + quoteTag(subject);
    // Toggle: clicking an already-active subject clears the search.
    if (state.mode === 'search' && state.searchQuery === q) {
        doClearSearch();
        return;
    }
    document.getElementById('search-input').value = q;
    document.getElementById('search-clear').hidden = false;
    await searchFiles(q);
    render();
}

async function applySubjectToSelection(subjectName) {
    const paths = state.selectedPaths.size > 0
        ? [...state.selectedPaths]
        : state.selectedFile ? [state.selectedFile.path] : [];
    if (!paths.length) return;
    const dir = currentAbsDir();
    // Re-apply every existing tag on each file with the new subject.
    for (const p of paths) {
        const data = state.selectedFilesData.get(p) || (state.selectedFile?.path === p ? state.selectedFile : null);
        const tags = data?.tags || [];
        if (!tags.length) continue;
        await Promise.all(tags.map(t =>
            apiPost('/api/tag', {
                path: p,
                tags: [t.value ? `${t.name}=${t.value}` : t.name],
                subject: subjectName,
                dir,
            })
        ));
    }
    showToast(`Assigned subject "${subjectName}" to ${paths.length} file${paths.length === 1 ? '' : 's'}.`);
    if (state.selectedFile) await loadFileDetail(state.selectedFile.path);
    renderDetailTagsOnly();
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
        ${(state.selectedFile || state.selectedPaths.size > 0)
            ? `<div class="tag-menu-divider"></div>
        <button class="tag-menu-action tag-menu-apply" onclick="applyTagToSelection('${jesc(tagName)}')">Apply to selection</button>`
            : ''}
        <div class="tag-menu-divider"></div>
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

async function applyTagToSelection(tagName) {
    closeTagMenu();
    const paths = state.selectedPaths.size > 0
        ? [...state.selectedPaths]
        : state.selectedFile ? [state.selectedFile.path] : [];
    if (!paths.length) return;
    await Promise.all(paths.map(p =>
        apiPost('/api/tag', { path: p, tags: [tagName], dir: currentAbsDir() })
    ));
    showToast(`Applied "${tagName}" to ${paths.length} file${paths.length === 1 ? '' : 's'}.`);
    await loadTags();
    if (state.selectedFile) await loadFileDetail(state.selectedFile.path);
    renderDetailTagsOnly();
    renderTags();
    _updateCardTagBadges();
}

// ---------------------------------------------------------------------------
// Drag-and-drop: file cards → sidebar tag/subject items
// ---------------------------------------------------------------------------

function tagDragStart(event, tagName) {
    event.stopPropagation();
    event.dataTransfer.effectAllowed = 'move';
    event.dataTransfer.setData('text/filetag-tag', tagName);
}

function tagDragOver(event) {
    const hasFiles = event.dataTransfer.types.includes('text/filetag-paths');
    const hasTag   = event.dataTransfer.types.includes('text/filetag-tag');
    if (!hasFiles && !hasTag) return;
    event.preventDefault();
    event.currentTarget.classList.add('tag-drag-over');
}

function tagDragLeave(event) {
    event.currentTarget.classList.remove('tag-drag-over');
}

async function tagDrop(event, tagName) {
    event.preventDefault();
    event.stopPropagation();
    event.currentTarget.classList.remove('tag-drag-over');

    // Tag-to-tag: move dragged tag under the drop target.
    const draggedTag = event.dataTransfer.getData('text/filetag-tag');
    if (draggedTag) {
        if (draggedTag === tagName) return;
        if (tagName.startsWith(draggedTag + '/')) return; // can't nest under own descendant
        const segment = draggedTag.split('/').pop();
        const newName = tagName + '/' + segment;
        if (newName === draggedTag) return;
        await renameTag(draggedTag, newName);
        return;
    }

    // File-to-tag: apply the tag to the dropped files.
    const raw = event.dataTransfer.getData('text/filetag-paths');
    if (!raw) return;
    const paths = JSON.parse(raw);
    await Promise.all(paths.map(p =>
        apiPost('/api/tag', { path: p, tags: [tagName], dir: currentAbsDir() })
    ));
    showToast(`Applied "${tagName}" to ${paths.length} file${paths.length === 1 ? '' : 's'}.`);
    await loadTags();
    if (state.selectedFile) await loadFileDetail(state.selectedFile.path);
    renderDetailTagsOnly();
    renderTags();
    _updateCardTagBadges();
}

async function subjectDrop(event, subjectName) {
    event.preventDefault();
    event.currentTarget.classList.remove('tag-drag-over');
    const raw = event.dataTransfer.getData('text/filetag-paths');
    if (!raw) return;
    const paths = JSON.parse(raw);
    const dir = currentAbsDir();
    for (const p of paths) {
        const data = state.selectedFilesData.get(p) || (state.selectedFile?.path === p ? state.selectedFile : null);
        const tags = data?.tags || [];
        if (!tags.length) continue;
        await Promise.all(tags.map(t =>
            apiPost('/api/tag', {
                path: p,
                tags: [t.value ? `${t.name}=${t.value}` : t.name],
                subject: subjectName,
                dir,
            })
        ));
    }
    showToast(`Assigned subject "${subjectName}" to ${paths.length} file${paths.length === 1 ? '' : 's'}.`);
    if (state.selectedFile) await loadFileDetail(state.selectedFile.path);
    renderDetailTagsOnly();
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
    // Clear kv value caches for affected tags so the sidebar reflects the rename.
    for (const n of [oldName, newName]) {
        const eq = n.indexOf('=');
        delete state.kvValueCache[eq > 0 ? n.slice(0, eq) : n];
    }
    const res = await apiPost('/api/rename-tag', { name: oldName, new_name: newName, dir: currentAbsDir() });
    if (res && res.merged) {
        showToast(`Tags merged into "${newName}".`);
    }
    await loadTags();
    if (state.selectedFile) await loadFileDetail(state.selectedFile.path);
    render();
    if (document.getElementById('tag-manager-overlay')) renderTmList();
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

let _tmTab = 'tags'; // 'tags' | 'subjects'
let _tmSelectedSubject = null;
let _tmSubjectSearch = '';

function showTagManager(selectTag) {
    if (document.getElementById('tag-manager-overlay')) return;

    const overlay = document.createElement('div');
    overlay.id = 'tag-manager-overlay';
    overlay.className = 'tm-overlay';
    overlay.innerHTML = `
        <div class="tm-modal" onclick="event.stopPropagation()">
            <div class="tm-header">
                <span class="tm-title">Tag Manager</span>
                <div class="tm-tabs">
                    <button class="tm-tab active" id="tm-tab-tags" onclick="tmSwitchTab('tags')">Tags</button>
                    <button class="tm-tab" id="tm-tab-subjects" onclick="tmSwitchTab('subjects')">Subjects</button>
                </div>
                <button class="tm-prune-btn" id="tm-prune-btn" onclick="pruneUnusedTags()" title="Remove all tags with no file assignments">Prune unused</button>
                <button class="tm-close" onclick="closeTagManager()" title="Close">\u2715</button>
            </div>
            <div class="tm-search-row" id="tm-search-row">
                <input id="tm-search" class="tm-search-input" type="text"
                    placeholder="Filter tags\u2026" oninput="tmSearch(this.value)"
                    onkeydown="if(event.key==='Escape') closeTagManager()">
            </div>
            <div class="tm-body" id="tm-body-tags">
                <div class="tm-list-col">
                    <div class="tm-list-header">
                        <button class="sidebar-sort-btn active" id="tm-sort-btn" onclick="toggleTagSortMode()" title="Sort: groups first">
                            <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                <line x1="2" y1="4" x2="14" y2="4"/><line x1="2" y1="8" x2="10" y2="8"/><line x1="2" y1="12" x2="6" y2="12"/>
                            </svg>
                            <span class="sort-label">Groups first</span>
                        </button>
                    </div>
                    <div class="tm-list" id="tm-list"></div>
                </div>
                <div class="tm-detail" id="tm-detail">
                    <div class="tm-detail-placeholder">Select a tag to edit it.</div>
                </div>
            </div>
            <div class="tm-body" id="tm-body-subjects" style="display:none">
                <div class="tm-list" id="tm-subject-list">
                    <div class="tm-new-row">
                        <input id="tm-new-subject-input" class="tm-input" type="text"
                            placeholder="New subject name\u2026"
                            onkeydown="if(event.key==='Enter') tmCreateSubject()">
                        <button class="tm-btn" onclick="tmCreateSubject()">+</button>
                    </div>
                </div>
                <div class="tm-detail" id="tm-subject-detail">
                    <div class="tm-detail-placeholder">Select a subject to edit it.</div>
                </div>
            </div>
        </div>
    `;
    overlay.addEventListener('click', closeTagManager);
    document.body.appendChild(overlay);

    _tmTab = 'tags';
    _tmSelectedTag = selectTag || null;
    _tmSearchQuery = '';
    _tmCollapsedGroups = new Set();
    _tmSelectedSubject = null;
    _tmSubjectSearch = '';
    renderTmList();
    if (_tmSelectedTag) renderTmDetail(_tmSelectedTag);

    requestAnimationFrame(() => {
        document.getElementById('tm-search')?.focus();
    });
}

function tmSwitchTab(tab) {
    _tmTab = tab;
    document.getElementById('tm-tab-tags').classList.toggle('active', tab === 'tags');
    document.getElementById('tm-tab-subjects').classList.toggle('active', tab === 'subjects');
    document.getElementById('tm-body-tags').style.display = tab === 'tags' ? '' : 'none';
    document.getElementById('tm-body-subjects').style.display = tab === 'subjects' ? '' : 'none';
    document.getElementById('tm-prune-btn').style.display = tab === 'tags' ? '' : 'none';
    const searchInput = document.getElementById('tm-search');
    if (searchInput) {
        searchInput.placeholder = tab === 'tags' ? 'Filter tags\u2026' : 'Filter subjects\u2026';
        searchInput.value = tab === 'tags' ? _tmSearchQuery : _tmSubjectSearch;
        searchInput.oninput = tab === 'tags'
            ? (e => tmSearch(e.target.value))
            : (e => tmSubjectSearch(e.target.value));
    }
    if (tab === 'subjects') renderTmSubjectList();
}

function closeTagManager() {
    const el = document.getElementById('tag-manager-overlay');
    if (el) el.remove();
    _tmSelectedTag = null;
    _tmSelectedSubject = null;
    _tmTab = 'tags';
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

// ---------------------------------------------------------------------------
// Tag Manager tree renderer — same CSS classes as sidebar, TM interaction
// ---------------------------------------------------------------------------

function renderTmTagTreeNodes(nodeMap, depth) {
    const q = _tmSearchQuery;
    let nodes = [...nodeMap.values()];
    if (q) nodes = nodes.filter(n => _nodeMatchesFilter(n, q));
    const mode = state.tagSortMode;
    if (mode === 'count') {
        nodes.sort((a, b) => _nodeCount(b) - _nodeCount(a) || a.segment.localeCompare(b.segment));
    } else if (depth === 0 && mode === 'groups-first' && !q) {
        nodes.sort((a, b) => {
            const ag = a.children.size > 0 ? 0 : 1;
            const bg = b.children.size > 0 ? 0 : 1;
            if (ag !== bg) return ag - bg;
            return a.segment.localeCompare(b.segment);
        });
    } else {
        nodes.sort((a, b) => a.segment.localeCompare(b.segment));
    }
    return nodes.map(n => renderTmTagTreeNode(n, depth)).join('');
}

function renderTmTagTreeNode(node, depth) {
    const { segment, fullPath, tag, children } = node;
    const hasChildren = children.size > 0;
    const marginStyle = depth > 0 ? ' style="margin-left:12px"' : '';
    const q = _tmSearchQuery;

    // --- Leaf node ---
    if (!hasChildren) {
        if (!tag) return '';
        const sel = _tmSelectedTag === fullPath ? ' active' : '';
        const synBadge = (tag.synonyms || []).length
            ? ` <span class="tag-synonym-badge" title="Synonyms: ${(tag.synonyms||[]).map(esc).join(', ')}">&#8801;</span>` : '';
        const kvBadge = tag.has_values ? ` <span class="tag-kv-badge">k=v</span>` : '';
        // depth=0: tag-standalone (padding-left:6px) → dot at 6+12+4 = 22px.
        // depth>0: inside a .tag-group div that already carries margin-left:12px,
        //   so tag-item (padding-left:22px) gives dot at 12+22+12+4 = 50px — same as sidebar.
        //   No extra margin or padding needed; the parent div handles the indentation.
        const cls = depth === 0 ? 'tag-item tag-standalone' : 'tag-item';
        return `<button class="${cls}${sel}" onclick="tmSelectTag('${jesc(fullPath)}')" draggable="true" ondragstart="tagDragStart(event,'${jesc(fullPath)}')" ondragover="tagDragOver(event)" ondragleave="tagDragLeave(event)" ondrop="tagDrop(event,'${jesc(fullPath)}')" ><span class="tag-check-placeholder"></span>${colorDot(tag.color)}${_highlightMatch(segment, q)}${kvBadge}${synBadge} <span class="count">${tag.count}</span></button>`;
    }

    // --- Group node ---
    const totalCount = _nodeCount(node);
    const expanded = !_tmCollapsedGroups.has(fullPath) || !!q;
    const expandedClass = expanded ? ' expanded' : '';
    const sel = _tmSelectedTag === fullPath ? ' active' : '';
    const groupColor = tag ? tag.color : null;
    const synBadge = tag && (tag.synonyms || []).length
        ? ` <span class="tag-synonym-badge" title="Synonyms: ${(tag.synonyms||[]).map(esc).join(', ')}">&#8801;</span>` : '';
    const kvBadge = tag && tag.has_values ? ` <span class="tag-kv-badge">k=v</span>` : '';
    const groupDrag = tag ? ` draggable="true" ondragstart="tagDragStart(event,'${jesc(fullPath)}')"` : '';
    return `<div class="tag-group"${marginStyle}>
        <div class="tag-group-label${expandedClass}${sel}">
            <button class="tag-group-chevron" onclick="tmToggleGroup('${jesc(fullPath)}')" title="Expand/collapse">
                <svg class="chevron-icon" viewBox="0 0 12 12"><polyline points="2,3 6,8 10,3" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>
            </button>
            <button class="tag-group-name"${groupDrag} onclick="tmSelectTag('${jesc(fullPath)}')" ondragover="tagDragOver(event)" ondragleave="tagDragLeave(event)" ondrop="tagDrop(event,'${jesc(fullPath)}')">${colorDot(groupColor)}${_highlightMatch(segment, q)}${kvBadge}${synBadge} <span class="count">${totalCount}</span></button>
        </div>
        <div class="tag-group-items${expanded ? ' open' : ''}">
            ${expanded ? renderTmTagTreeNodes(children, depth + 1) : ''}
        </div>
    </div>`;
}

function _updateTmSortBtn() {
    const labels = { 'groups-first': 'Groups first', 'alpha': 'A\u2013Z', 'count': 'By count' };
    const titles = { 'groups-first': 'Sort: groups first', 'alpha': 'Sort: A\u2013Z', 'count': 'Sort: by count' };
    const btn = document.getElementById('tm-sort-btn');
    if (!btn) return;
    btn.title = titles[state.tagSortMode];
    btn.classList.toggle('active', state.tagSortMode !== 'alpha');
    const label = btn.querySelector('.sort-label');
    if (label) label.textContent = labels[state.tagSortMode];
}

function renderTmList() {
    const el = document.getElementById('tm-list');
    if (!el) return;

    _updateTmSortBtn();

    const q = _tmSearchQuery;
    const filtered = q
        ? state.tags.filter(t => t.name.toLowerCase().includes(q))
        : state.tags;

    if (!filtered.length) {
        el.innerHTML = '<div class="tm-empty">No tags found.</div>';
        return;
    }

    const tree = buildTagTree(filtered);
    el.innerHTML = renderTmTagTreeNodes(tree, 0);
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

// ---------------------------------------------------------------------------
// Subject Manager (tab inside Tag Manager)
// ---------------------------------------------------------------------------

function tmSubjectSearch(q) {
    _tmSubjectSearch = q.toLowerCase();
    renderTmSubjectList();
}

function renderTmSubjectList() {
    const el = document.getElementById('tm-subject-list');
    if (!el) return;

    const q = _tmSubjectSearch;
    const filtered = q
        ? state.subjects.filter(s => s.name.toLowerCase().includes(q))
        : state.subjects;

    // Preserve the new-subject input row (first child) if it exists
    const newRow = el.querySelector('.tm-new-row');
    const savedValue = newRow ? newRow.querySelector('input')?.value ?? '' : '';

    let html = `
        <div class="tm-new-row">
            <input id="tm-new-subject-input" class="tm-input" type="text"
                placeholder="New subject name\u2026"
                onkeydown="if(event.key==='Enter') tmCreateSubject()">
            <button class="tm-btn" onclick="tmCreateSubject()">+</button>
        </div>`;

    if (!filtered.length) {
        html += '<div class="tm-empty">No subjects found.</div>';
    } else {
        for (const s of [...filtered].sort((a, b) => a.name.localeCompare(b.name))) {
            const sel = _tmSelectedSubject === s.name ? ' selected' : '';
            html += `<div class="tm-tag-row tm-tag-standalone${sel}" onclick="tmSelectSubject('${jesc(s.name)}')">
                ${esc(s.name)}
                <span class="tm-count">${s.count}</span>
            </div>`;
        }
    }
    el.innerHTML = html;

    // Restore typed value if user was mid-input
    if (savedValue) {
        const inp = el.querySelector('#tm-new-subject-input');
        if (inp) inp.value = savedValue;
    }
}

async function tmCreateSubject() {
    const input = document.getElementById('tm-new-subject-input');
    const name = input ? input.value.trim() : '';
    if (!name) return;
    try {
        await apiPost('/api/create-subject', { name, dir: currentAbsDir() });
    } catch (e) {
        showToast('Error: ' + e.message);
        return;
    }
    if (input) input.value = '';
    await loadTags();
    _tmSelectedSubject = name;
    renderTmSubjectList();
    renderTmSubjectDetail(name);
}

function tmSelectSubject(name) {
    _tmSelectedSubject = name;
    renderTmSubjectList();
    renderTmSubjectDetail(name);
}

async function renderTmSubjectDetail(name) {
    const panel = document.getElementById('tm-subject-detail');
    if (!panel) return;

    const subj = state.subjects.find(s => s.name === name);
    if (!subj) {
        panel.innerHTML = `<div class="tm-detail-placeholder">Subject not found.</div>`;
        return;
    }

    // Show skeleton while loading
    panel.innerHTML = `
        <div class="tm-detail-header">
            <div class="tm-detail-name">${esc(name)}</div>
            <div class="tm-detail-meta">${subj.count} file${subj.count !== 1 ? 's' : ''}</div>
        </div>
        <div class="tm-detail-placeholder" style="padding:12px 16px">Loading\u2026</div>
    `;

    // Load file-level tags and entity properties in parallel
    const [subjTags, subjProps] = await Promise.all([
        api('/api/subject/tags?' + new URLSearchParams({ name }) + dirParam('&')).catch(() => []),
        api('/api/subject/props?' + new URLSearchParams({ name }) + dirParam('&')).catch(() => []),
    ]);

    if (document.getElementById('tm-subject-detail') !== panel) return; // closed meanwhile

    const tagRows = subjTags.map(t => `
        <div class="tm-val-row">
            <span class="tm-val-name" onclick="tmSubjectTagSearch('${jesc(name)}','${jesc(t.value)}')"
                title="Search ${esc(name)} and ${esc(t.value)}">${esc(t.value)}</span>
            <span class="tm-val-count">${t.count}</span>
            <button class="tm-val-rename" onclick="tmSubjectRemoveTag('${jesc(name)}','${jesc(t.value)}')"
                title="Remove this tag from all files in subject">\u2715</button>
        </div>`).join('');

    const propRows = subjProps.map(p => {
        const label = p.value ? `${esc(p.tag)} = ${esc(p.value)}` : esc(p.tag);
        return `
        <div class="tm-val-row">
            <span class="tm-val-name">${label}</span>
            <button class="tm-val-rename" onclick="tmSubjectRemoveProp('${jesc(name)}','${jesc(p.tag)}','${jesc(p.value)}')"
                title="Remove property">\u2715</button>
        </div>`;
    }).join('');

    panel.innerHTML = `
        <div class="tm-detail-header">
            <div class="tm-detail-name">${esc(name)}</div>
            <div class="tm-detail-meta">${subj.count} file${subj.count !== 1 ? 's' : ''}</div>
        </div>

        <section class="tm-section">
            <div class="tm-section-title">Properties
                <span class="tm-section-hint">(describe what this subject <em>is</em>)</span>
            </div>
            ${subjProps.length
                ? `<div class="tm-val-list">${propRows}</div>`
                : `<div class="tm-empty-hint">No properties yet.</div>`}
            <div class="tm-syn-add" style="margin-top:6px">
                <input id="tm-subj-prop-tag" class="tm-input" type="text"
                    placeholder="Property (e.g. geslacht, geboren\u2026)"
                    list="tm-subj-prop-datalist"
                    style="flex:1.5"
                    onkeydown="if(event.key==='Enter') tmSubjectSetProp('${jesc(name)}')">
                <datalist id="tm-subj-prop-datalist">
                    ${state.tags.map(t => `<option value="${esc(t.name)}">`).join('')}
                </datalist>
                <input id="tm-subj-prop-val" class="tm-input" type="text"
                    placeholder="Value (optional)"
                    style="flex:1"
                    onkeydown="if(event.key==='Enter') tmSubjectSetProp('${jesc(name)}')">
                <button class="tm-btn" onclick="tmSubjectSetProp('${jesc(name)}')">Add</button>
            </div>
        </section>

        <section class="tm-section">
            <div class="tm-section-title">File tags
                <span class="tm-section-hint">(tags on files under this subject; click to search, \u2715 to remove)</span>
            </div>
            ${subjTags.length
                ? `<div class="tm-val-list">${tagRows}</div>`
                : `<div class="tm-empty-hint">No tags assigned yet.</div>`}
            <div class="tm-syn-add" style="margin-top:6px">
                <input id="tm-subj-tag-input" class="tm-input" type="text"
                    placeholder="Add tag to all files in subject\u2026"
                    list="tm-subj-tag-datalist"
                    onkeydown="if(event.key==='Enter') tmSubjectAddTag('${jesc(name)}')">
                <datalist id="tm-subj-tag-datalist">
                    ${state.tags.map(t => `<option value="${esc(t.name)}">`).join('')}
                </datalist>
                <button class="tm-btn" onclick="tmSubjectAddTag('${jesc(name)}')">Add</button>
            </div>
        </section>

        <section class="tm-section tm-ops">
            <div class="tm-section-title">Operations</div>

            <div class="tm-op-row">
                <label class="tm-op-label">Rename to</label>
                <div class="tm-op-inputs">
                    <input id="tm-subj-rename-input" class="tm-input" type="text" value="${esc(name)}"
                        placeholder="New name\u2026"
                        onkeydown="if(event.key==='Enter') tmDoRenameSubject('${jesc(name)}')">
                    <button class="tm-btn" onclick="tmDoRenameSubject('${jesc(name)}')">Rename</button>
                </div>
                <div class="tm-op-hint">All tag assignments with this subject label are updated.</div>
            </div>

            <div class="tm-op-row">
                <label class="tm-op-label">Clone as</label>
                <div class="tm-op-inputs">
                    <input id="tm-subj-clone-input" class="tm-input" type="text"
                        placeholder="New subject name\u2026"
                        onkeydown="if(event.key==='Enter') tmCloneSubject('${jesc(name)}')">
                    <button class="tm-btn" onclick="tmCloneSubject('${jesc(name)}')">Clone</button>
                </div>
                <div class="tm-op-hint">Copies all file tag assignments to a new subject name.</div>
            </div>
        </section>

        <div class="tm-danger-zone">
            <button class="tm-btn tm-btn-danger" onclick="tmDeleteSubject('${jesc(name)}')">
                Remove subject (${subj.count} file${subj.count !== 1 ? 's' : ''})
            </button>
        </div>
    `;
}

async function tmSubjectAddTag(subject) {
    const input = document.getElementById('tm-subj-tag-input');
    const tag = input ? input.value.trim() : '';
    if (!tag) return;
    try {
        const res = await apiPost('/api/subject/add-tag', { subject, tag, dir: currentAbsDir() });
        showToast(`Added "${tag}" to ${res.inserted ?? 0} file(s) in subject "${subject}".`);
        if (input) input.value = '';
    } catch (e) {
        showToast('Error: ' + e.message);
        return;
    }
    await loadTags();
    renderTmSubjectList();
    await renderTmSubjectDetail(subject);
    renderSubjects();
}

async function tmSubjectRemoveTag(subject, tag) {
    if (!confirm(`Remove tag "${tag}" from all files in subject "${subject}"?\nThis deletes ${tag} on files where it was assigned under this subject.`)) return;
    try {
        const res = await apiPost('/api/subject/remove-tag', { subject, tag, dir: currentAbsDir() });
        showToast(`Removed "${tag}" from ${res.removed ?? 0} file(s) in subject "${subject}".`);
    } catch (e) {
        showToast('Error: ' + e.message);
        return;
    }
    await loadTags();
    renderTmSubjectList();
    await renderTmSubjectDetail(subject);
    renderSubjects();
}

async function tmCloneSubject(name) {
    const input = document.getElementById('tm-subj-clone-input');
    const newName = input ? input.value.trim() : '';
    if (!newName || newName === name) return;
    if (state.subjects.find(s => s.name === newName)) {
        if (!confirm(`Subject "${newName}" already exists. Merge into it?`)) return;
    }
    try {
        const res = await apiPost('/api/clone-subject', { name, new_name: newName, dir: currentAbsDir() });
        showToast(`Cloned "${name}" to "${newName}" (${res.inserted ?? 0} rows).`);
    } catch (e) {
        showToast('Error: ' + e.message);
        return;
    }
    await loadTags();
    _tmSelectedSubject = newName;
    renderTmSubjectList();
    await renderTmSubjectDetail(newName);
    renderSubjects();
}

function tmSubjectTagSearch(subject, tag) {
    closeTagManager();
    const query = `subject:${subject} and ${tag}`;
    document.getElementById('search-input').value = query;
    searchFiles(query).then(() => {
        document.getElementById('search-clear').hidden = false;
        render();
    });
}

async function tmSubjectSetProp(subject) {
    const tagInput = document.getElementById('tm-subj-prop-tag');
    const valInput = document.getElementById('tm-subj-prop-val');
    const tag = tagInput ? tagInput.value.trim() : '';
    const value = valInput ? valInput.value.trim() : '';
    if (!tag) return;
    try {
        await apiPost('/api/subject/set-prop', { subject, tag, value, dir: currentAbsDir() });
        showToast(`Added property "${tag}${value ? '=' + value : ''}" to "${subject}".`);
        if (tagInput) tagInput.value = '';
        if (valInput) valInput.value = '';
    } catch (e) {
        showToast('Error: ' + e.message);
        return;
    }
    await renderTmSubjectDetail(subject);
}

async function tmSubjectRemoveProp(subject, tag, value) {
    try {
        await apiPost('/api/subject/remove-prop', { subject, tag, value, dir: currentAbsDir() });
        showToast(`Removed property "${tag}${value ? '=' + value : ''}" from "${subject}".`);
    } catch (e) {
        showToast('Error: ' + e.message);
        return;
    }
    await renderTmSubjectDetail(subject);
}

async function tmDoRenameSubject(oldName) {
    const input = document.getElementById('tm-subj-rename-input');
    const newName = input ? input.value.trim() : '';
    if (!newName || newName === oldName) return;
    if (!confirm(`Rename subject "${oldName}" to "${newName}"?`)) return;
    await apiPost('/api/rename-subject', { name: oldName, new_name: newName, dir: currentAbsDir() });
    showToast(`Renamed subject "${oldName}" to "${newName}".`);
    await loadTags();
    _tmSelectedSubject = newName;
    renderTmSubjectList();
    renderTmSubjectDetail(newName);
    renderSubjects();
}

async function tmDeleteSubject(name) {
    const subj = state.subjects.find(s => s.name === name);
    const count = subj?.count || 0;
    if (!confirm(`Remove subject "${name}"?\nThis clears the subject label from ${count} file(s). The tags themselves are not removed.`)) return;
    await apiPost('/api/delete-subject', { name, dir: currentAbsDir() });
    showToast(`Subject "${name}" removed.`);
    await loadTags();
    _tmSelectedSubject = null;
    renderTmSubjectList();
    const panel = document.getElementById('tm-subject-detail');
    if (panel) panel.innerHTML = `<div class="tm-detail-placeholder">Subject removed.</div>`;
    renderSubjects();
}
