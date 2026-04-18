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
            groups[prefix].children.push({ suffix, fullName: tag.name, count: tag.count, color: tag.color, synonyms: tag.synonyms || [] });
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

    // Active filter chips (shown when one or more tags are selected)
    if (state.activeTags.size > 0) {
        html += '<div class="active-filters">';
        for (const t of state.activeTags) {
            html += `<button class="filter-chip" onclick="toggleTagFilter('${jesc(t)}')" title="Remove filter">${esc(t)} ×</button>`;
        }
        html += `<button class="active-filters-clear" onclick="clearTagFilters()">Clear all</button>`;
        html += '</div>';
    }

    // Grouped tags
    const groupNames = Object.keys(groups).sort();
    for (const prefix of groupNames) {
        const { root, children } = groups[prefix];
        const items = children.sort((a, b) => a.suffix.localeCompare(b.suffix));
        const rootCount = root ? root.count : 0;
        const totalCount = items.reduce((s, i) => s + i.count, 0) + rootCount;
        const groupQuery = root ? `${prefix} or ${prefix}/*` : `${prefix}/*`;
        const groupActiveClass = (state.mode === 'search' && state.searchQuery === groupQuery)
            || items.some(i => state.activeTags.has(i.fullName))
            ? ' active' : '';
        const groupColor = root ? root.color : null;
        const expanded = state.expandedGroups.has(prefix);
        const expandedClass = expanded ? ' expanded' : '';
        const rootContextMenu = root ? ` oncontextmenu="showTagMenu(event,'${jesc(prefix)}')"` : '';
        html += `<div class="tag-group">
            <div class="tag-group-label${groupActiveClass}${expandedClass}">
                <button class="tag-group-chevron" onclick="toggleTagGroup('${jesc(prefix)}')" title="Expand/collapse">
                    <svg class="chevron-icon" viewBox="0 0 12 12"><polyline points="2,3 6,8 10,3" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>
                </button>
                <button class="tag-group-name" onclick="doTagGroupSearch('${jesc(prefix)}')"${rootContextMenu}>${colorDot(groupColor)}${esc(prefix)} <span class="count">${totalCount}</span></button>
            </div>
            <div class="tag-group-items${expanded ? ' open' : ''}">`;
        for (const item of items) {
            const active = state.activeTags.has(item.fullName) ? ' active' : '';
            const synBadge = item.synonyms.length ? ` <span class="tag-synonym-badge" title="Synonyms: ${item.synonyms.map(esc).join(', ')}">≡</span>` : '';
            html += `<button class="tag-item${active}" onclick="toggleTagFilter('${jesc(item.fullName)}')" oncontextmenu="showTagMenu(event, '${jesc(item.fullName)}')">
                ${colorDot(item.color)}${esc(item.suffix)}${synBadge} <span class="count">${item.count}</span>
            </button>`;
        }
        html += '</div></div>';
    }

    // Standalone tags (those that are not a prefix of any group)
    for (const tag of trulyStandalone.sort((a, b) => a.name.localeCompare(b.name))) {
        const active = state.activeTags.has(tag.name) ? ' active' : '';
        const synBadge = (tag.synonyms || []).length ? ` <span class="tag-synonym-badge" title="Synonyms: ${(tag.synonyms || []).map(esc).join(', ')}">≡</span>` : '';
        html += `<button class="tag-item tag-standalone${active}" onclick="toggleTagFilter('${jesc(tag.name)}')" oncontextmenu="showTagMenu(event, '${jesc(tag.name)}')">
            ${colorDot(tag.color)}${esc(tag.name)}${synBadge} <span class="count">${tag.count}</span>
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
        return `<button class="tag-menu-swatch${sel}" style="background:${c}" onclick="setTagColor('${jesc(tagName)}','${c}')"></button>`;
    }).join('');
    // "no color" swatch
    const noSel = !currentColor ? ' selected' : '';
    swatches = `<button class="tag-menu-swatch tag-menu-swatch-none${noSel}" onclick="setTagColor('${jesc(tagName)}', null)" title="No color">✕</button>` + swatches;

    const menu = document.createElement('div');
    menu.id = 'tag-context-menu';
    menu.className = 'tag-context-menu';

    const synonyms = tag?.synonyms || [];
    const synonymRows = synonyms.map(a =>
        `<span class="tag-menu-synonym-row">${esc(a)}<button class="tag-menu-synonym-remove" onclick="removeSynonym('${jesc(tagName)}','${jesc(a)}')" title="Remove synonym">✕</button></span>`
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
                <input id="tag-menu-synonym-input" class="tag-menu-rename-input" type="text" placeholder="Add alias…" onclick="event.stopPropagation()">
                <button class="tag-menu-action" onclick="addSynonymFromInput('${jesc(tagName)}')">Add</button>
            </div>
        </div>
        <div class="tag-menu-divider"></div>
        <button class="tag-menu-action" onclick="startTagRename('${jesc(tagName)}')">Rename tag</button>
        <button class="tag-menu-action tag-menu-delete" onclick="deleteTag('${jesc(tagName)}')">Delete tag</button>
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

    // Prevent the outside-click listener from closing the menu immediately
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
        showToast(`Added synonym "${alias}" → "${canonical}".`);
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

