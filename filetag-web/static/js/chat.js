// ---------------------------------------------------------------------------
// Chat panel — multi-turn conversation about selected files
// ---------------------------------------------------------------------------

let _chatMessages = [];   // [{role, content}]
let _chatFiles    = [];   // absolute paths of files in context
let _chatSending  = false;

function _initChatResize() {
    const panel = document.getElementById('chat-panel');
    if (!panel) return;

    // --- Restore saved size ---
    const sw = localStorage.getItem('ft-chat-width');
    const sh = localStorage.getItem('ft-chat-height');
    if (sw) panel.style.width  = sw;
    if (sh) panel.style.height = sh;

    // --- Restore saved free position ---
    if (localStorage.getItem('ft-chat-free') === '1') {
        const sl = localStorage.getItem('ft-chat-left');
        const st = localStorage.getItem('ft-chat-top');
        panel.style.right  = 'auto';
        panel.style.bottom = 'auto';
        if (sl) panel.style.left = sl;
        if (st) panel.style.top  = st;
        panel.dataset.free = '1';
    }

    // Switch from CSS right/bottom anchor to explicit left/top so the panel
    // can be positioned freely anywhere on screen.
    function enterFreeMode() {
        if (panel.dataset.free === '1') return;
        const rect = panel.getBoundingClientRect();
        panel.style.right  = 'auto';
        panel.style.bottom = 'auto';
        panel.style.left   = rect.left + 'px';
        panel.style.top    = rect.top  + 'px';
        panel.dataset.free = '1';
        localStorage.setItem('ft-chat-free', '1');
    }

    // --- Create 8 resize handles ---
    for (const dir of ['n', 'ne', 'e', 'se', 's', 'sw', 'w', 'nw']) {
        const el = document.createElement('div');
        el.className  = 'chat-resize-edge';
        el.dataset.dir = dir;
        el.addEventListener('mousedown', (e) => {
            if (e.button !== 0) return;
            e.preventDefault();
            e.stopPropagation();
            enterFreeMode();
            const startX = e.clientX;
            const startY = e.clientY;
            const startW = panel.offsetWidth;
            const startH = panel.offsetHeight;
            const startL = panel.getBoundingClientRect().left;
            const startT = panel.getBoundingClientRect().top;
            const cs     = getComputedStyle(panel);
            const minW   = parseInt(cs.minWidth)  || 260;
            const minH   = parseInt(cs.minHeight) || 260;
            const maxW   = Math.min(parseInt(cs.maxWidth)  || 900, window.innerWidth  - 8);
            const maxH   = Math.min(parseInt(cs.maxHeight) || 900, window.innerHeight - 8);
            document.body.style.userSelect = 'none';
            document.body.style.cursor     = getComputedStyle(el).cursor;
            function onMove(ev) {
                const dx = ev.clientX - startX;
                const dy = ev.clientY - startY;
                let w = startW, h = startH, l = startL, t = startT;
                if (dir.includes('e')) {
                    w = Math.max(minW, Math.min(maxW, startW + dx));
                }
                if (dir.includes('s')) {
                    h = Math.max(minH, Math.min(maxH, startH + dy));
                }
                if (dir.includes('w')) {
                    const nw = Math.max(minW, Math.min(maxW, startW - dx));
                    l = startL + (startW - nw);
                    w = nw;
                }
                if (dir.includes('n')) {
                    const nh = Math.max(minH, Math.min(maxH, startH - dy));
                    t = startT + (startH - nh);
                    h = nh;
                }
                panel.style.width  = w + 'px';
                panel.style.height = h + 'px';
                panel.style.left   = l + 'px';
                panel.style.top    = t + 'px';
                localStorage.setItem('ft-chat-width',  w + 'px');
                localStorage.setItem('ft-chat-height', h + 'px');
                localStorage.setItem('ft-chat-left',   l + 'px');
                localStorage.setItem('ft-chat-top',    t + 'px');
            }
            function onUp() {
                document.body.style.userSelect = '';
                document.body.style.cursor     = '';
                window.removeEventListener('mousemove', onMove);
                window.removeEventListener('mouseup',   onUp);
            }
            window.addEventListener('mousemove', onMove);
            window.addEventListener('mouseup',   onUp);
        });
        panel.appendChild(el);
    }

    // --- Drag: move panel by header ---
    const header = panel.querySelector('.chat-header');
    if (header) {
        header.addEventListener('mousedown', (e) => {
            if (e.button !== 0) return;
            if (e.target.closest('button,input,select,textarea,a')) return;
            e.preventDefault();
            enterFreeMode();
            const startX = e.clientX;
            const startY = e.clientY;
            const startL = panel.getBoundingClientRect().left;
            const startT = panel.getBoundingClientRect().top;
            document.body.style.cursor     = 'grabbing';
            document.body.style.userSelect = 'none';
            function onMove(ev) {
                const l = Math.max(-(panel.offsetWidth - 100),
                              Math.min(window.innerWidth - 100,
                                startL + (ev.clientX - startX)));
                const t = Math.max(0,
                              Math.min(window.innerHeight - 42,
                                startT + (ev.clientY - startY)));
                panel.style.left = l + 'px';
                panel.style.top  = t + 'px';
                localStorage.setItem('ft-chat-left', l + 'px');
                localStorage.setItem('ft-chat-top',  t + 'px');
            }
            function onUp() {
                document.body.style.cursor     = '';
                document.body.style.userSelect = '';
                window.removeEventListener('mousemove', onMove);
                window.removeEventListener('mouseup',   onUp);
            }
            window.addEventListener('mousemove', onMove);
            window.addEventListener('mouseup',   onUp);
        });
    }
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
