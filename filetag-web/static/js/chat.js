// ---------------------------------------------------------------------------
// Chat panel — multi-turn conversation about selected files
// ---------------------------------------------------------------------------

let _chatMessages = [];   // [{role, content}]
let _chatFiles    = [];   // absolute paths of files in context
let _chatSending  = false;

function _initChatResize() {
    const handle = document.getElementById('chat-resize-handle');
    const panel  = document.getElementById('chat-panel');
    if (!handle || !panel) return;
    handle.addEventListener('mousedown', (e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        const startX = e.clientX;
        const startY = e.clientY;
        const startW = panel.offsetWidth;
        const startH = panel.offsetHeight;
        const minW = parseInt(getComputedStyle(panel).minWidth) || 260;
        const minH = parseInt(getComputedStyle(panel).minHeight) || 260;
        const maxW = parseInt(getComputedStyle(panel).maxWidth) || 760;
        const maxH = parseInt(getComputedStyle(panel).maxHeight) || 900;
        document.body.style.cursor = 'nwse-resize';
        document.body.style.userSelect = 'none';
        function onMove(ev) {
            const w = Math.max(minW, Math.min(maxW, startW - (ev.clientX - startX)));
            const h = Math.max(minH, Math.min(maxH, startH - (ev.clientY - startY)));
            panel.style.width  = w + 'px';
            panel.style.height = h + 'px';
            localStorage.setItem('ft-chat-width',  w + 'px');
            localStorage.setItem('ft-chat-height', h + 'px');
        }
        function onUp() {
            document.body.style.cursor = '';
            document.body.style.userSelect = '';
            window.removeEventListener('mousemove', onMove);
            window.removeEventListener('mouseup', onUp);
        }
        window.addEventListener('mousemove', onMove);
        window.addEventListener('mouseup', onUp);
    });
    // Restore saved size
    const sw = localStorage.getItem('ft-chat-width');
    const sh = localStorage.getItem('ft-chat-height');
    if (sw) panel.style.width  = sw;
    if (sh) panel.style.height = sh;
}

function _updateChatVideoBar() {
    const bar = document.getElementById('chat-video-bar');
    if (!bar) return;
    const hasVideo = _chatFiles.some(p => {
        const ext = p.split('.').pop().toLowerCase();
        return AI_VIDEO_EXTS.has(ext);
    });
    bar.hidden = !hasVideo;
    if (!hasVideo) return;
    const autoEl  = document.getElementById('chat-frames-auto');
    const inputEl = document.getElementById('chat-frames-input');
    if (autoEl)  autoEl.checked  = state.aiVideoFramesAuto;
    if (inputEl) {
        inputEl.value    = state.aiVideoFrames;
        inputEl.disabled = state.aiVideoFramesAuto;
    }
    // Disable controls once the conversation has started — the video context
    // was set with the first message and cannot be changed mid-conversation.
    const started = _chatMessages.some(m => m.role === 'user');
    bar.classList.toggle('chat-video-bar--locked', started);
    bar.querySelectorAll('input').forEach(el => {
        if (started) el.setAttribute('disabled', '');
        else if (el.type === 'number') el.disabled = state.aiVideoFramesAuto;
        else el.removeAttribute('disabled');
    });
}

function openChat() {
    const files = state.selectedPaths.size > 0
        ? [...state.selectedPaths]
        : state.selectedFile ? [state.selectedFile.path] : [];
    if (!files.length) {
        showToast('Select one or more files first');
        return;
    }

    _chatFiles    = files;
    _chatMessages = [];
    _chatSending  = false;

    const menu = document.getElementById('more-menu');
    if (menu) menu.hidden = true;

    const panel = document.getElementById('chat-panel');
    panel.hidden = false;
    _renderChatFiles();
    _updateChatVideoBar();
    _renderChatMessages();
    document.getElementById('chat-input').focus();
}

function closeChat() {
    const panel = document.getElementById('chat-panel');
    panel.hidden = true;
    _chatMessages = [];
    _chatFiles    = [];
}

function chatClearHistory() {
    _chatMessages = [];
    _renderChatMessages();
    _updateChatVideoBar();
    document.getElementById('chat-input').focus();
}

function _renderChatFiles() {
    const el  = document.getElementById('chat-file-thumbs');
    const dir = currentAbsDir();
    const dirQ = dir ? '&dir=' + encodeURIComponent(dir) : '';
    const shown = _chatFiles.slice(0, 8);
    const extra = _chatFiles.length - shown.length;
    el.innerHTML = shown.map(p => {
        const name = p.split('/').pop();
        return `<img class="chat-thumb" src="/api/thumb?path=${encodeURIComponent(p)}${dirQ}" `
             + `alt="${esc(name)}" title="${esc(name)}" onerror="this.style.display='none'">`;
    }).join('')
    + (extra > 0 ? `<span class="chat-thumb-more">+${extra}</span>` : '');
}

function _renderChatMessages() {
    const el = document.getElementById('chat-messages');
    if (!_chatMessages.length) {
        el.innerHTML = '<p class="chat-hint">Ask anything about the selected file(s).</p>';
        return;
    }
    el.innerHTML = _chatMessages.map(m => {
        const cls  = m.role === 'user' ? 'chat-msg-user' : 'chat-msg-assistant';
        const html = esc(m.content).replace(/\n/g, '<br>');
        return `<div class="chat-msg ${cls}"><div class="chat-bubble">${html}</div></div>`;
    }).join('');
    el.scrollTop = el.scrollHeight;
}

async function sendChatMessage() {
    const input = document.getElementById('chat-input');
    const text  = input.value.trim();
    if (!text || _chatSending) return;

    _chatMessages = [..._chatMessages, { role: 'user', content: text }];
    input.value   = '';
    _chatSending  = true;
    document.getElementById('chat-send-btn').disabled = true;

    // Optimistically show user message + loading placeholder
    _chatMessages = [..._chatMessages, { role: 'assistant', content: '…' }];
    _renderChatMessages();

    try {
        const res = await apiPost('/api/ai/chat', {
            dir:      currentAbsDir(),
            files:    _chatFiles,
            messages: _chatMessages.slice(0, -1),  // exclude placeholder
            n_frames: state.aiVideoFramesAuto ? null : (state.aiVideoFrames || null),
        });
        _chatMessages[_chatMessages.length - 1] = { role: 'assistant', content: res.reply };
    } catch (e) {
        _chatMessages[_chatMessages.length - 1] = { role: 'assistant', content: '⚠ ' + e.message };
    } finally {
        _chatSending = false;
        document.getElementById('chat-send-btn').disabled = false;
        _renderChatMessages();
        _updateChatVideoBar();
        input.focus();
    }
}
