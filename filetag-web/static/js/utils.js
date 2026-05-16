// ---------------------------------------------------------------------------
// Global fetch interceptor
// Handles two cases transparently for all API calls:
//   401 Unauthorised  — session expired or server restarted; redirect to /login.
//   X-Build-Id change — server restarted with new code; reload to pick up new JS.
// ---------------------------------------------------------------------------
{
    const _origFetch = window.fetch.bind(window);
    let _knownBuildId = null;
    let _navigating   = false;

    window.fetch = async function(input, init) {
        const resp = await _origFetch(input, init);
        if (_navigating) return resp;

        // Auth expired or server restarted and wiped the in-memory session store.
        if (resp.status === 401) {
            _navigating = true;
            window.location.href = '/login';
            return resp;
        }

        // Server was restarted with a new build — reload so stale JS is replaced.
        const bid = resp.headers.get('x-build-id');
        if (bid) {
            if (_knownBuildId === null) {
                _knownBuildId = bid;
            } else if (_knownBuildId !== bid) {
                _navigating = true;
                window.location.reload();
            }
        }

        return resp;
    };
}

// ---------------------------------------------------------------------------
// Icons (inline SVG)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Lightweight event bus
// Two events are defined:
//   'ft:tags-meta'  — tag metadata changed (name, colour, synonyms)
//   'ft:file-tags'  — file↔tag associations changed (tag/untag operations)
// Emitters load all required data BEFORE calling ftEmit so that by the time
// subscribers run, state.tags / state.selectedFile are already up to date.
// Subscribers are responsible only for rendering.
// ---------------------------------------------------------------------------

const _ftBus = {};

/** Register a handler for an event. */
function ftOn(event, handler) {
    (_ftBus[event] ??= []).push(handler);
}
/** Remove a previously registered handler. */
function ftOff(event, handler) {
    if (_ftBus[event]) _ftBus[event] = _ftBus[event].filter(h => h !== handler);
}
/** Emit an event, calling all registered handlers synchronously. */
function ftEmit(event, detail) {
    (_ftBus[event] || []).forEach(h => h(detail));
}

const ICONS = {
    // Lucide-style icons (stroke-based, outline)
    folder:   '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/></svg>',
    file:     '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/></svg>',
    image:    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21"/></svg>',
    audio:    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>',
    video:    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m22 8-6 4 6 4V8z"/><rect width="14" height="12" x="2" y="6" rx="2"/></svg>',
    pdf:      '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="M10 9H8"/><path d="M16 13H8"/><path d="M16 17H8"/></svg>',
    text:     '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="M10 9H8"/><path d="M16 13H8"/><path d="M16 17H8"/></svg>',
    markdown: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="M7 13h2l2 4 2-4h2"/></svg>',
    raw:      '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21"/><path d="M17 3l4 4"/><path d="m21 3-4 4"/></svg>',
    zip:      '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="20" height="5" x="2" y="3" rx="1"/><path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8"/><path d="M10 12h4"/></svg>',
    gotoDir:  '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M1 4.5v7A1.5 1.5 0 002.5 13h11A1.5 1.5 0 0015 11.5V6a1.5 1.5 0 00-1.5-1.5H7L5.5 3H2.5A1.5 1.5 0 001 4.5z"/><polyline points="9 8 11 10 9 12"/><line x1="6" y1="10" x2="11" y2="10"/></svg>',
    root:     '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M3 5v14a9 3 0 0 0 18 0V5"/><path d="M3 12a9 3 0 0 0 18 0"/></svg>',
};

// ---------------------------------------------------------------------------
// File type detection
// ---------------------------------------------------------------------------

const EXT_MAP = {
    image:    ['jpg','jpeg','png','gif','webp','svg','bmp','ico','tiff','tif','avif'],
    audio:    ['mp3','flac','wav','ogg','opus','aac','m4a','wma','aiff','alac'],
    video:    ['mp4','webm','mkv','avi','mov','wmv','flv','m4v','3gp','f4v','mpg','mpeg',
               'm2v','m2ts','mts','mxf','rm','rmvb','divx','vob','ogv','ogg','dv','asf','amv',
               'mpe','m1v','mpv','qt'],
    pdf:      ['pdf'],
    markdown: ['md','markdown'],
    zip:      ['zip','cbz','rar','cbr','7z','cb7'],
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
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;');
}

// ---------------------------------------------------------------------------
// Syntax highlighting (no external dependencies)
// ---------------------------------------------------------------------------

function hlLang(filename) {
    const ext = (filename.includes('.') ? filename.split('.').pop() : '').toLowerCase();
    return ({
        js:'js', jsx:'js', mjs:'js', cjs:'js', ts:'js', tsx:'js',
        py:'py', pyw:'py',
        rb:'rb',
        rs:'rs',
        go:'go',
        java:'java', kt:'java',
        c:'c', h:'c', cpp:'c', cc:'c', cxx:'c', hpp:'c', hxx:'c',
        sh:'sh', bash:'sh', zsh:'sh', fish:'sh', ksh:'sh',
        sql:'sql',
        json:'json',
        yaml:'yaml', yml:'yaml',
        toml:'toml',
        xml:'xml', html:'xml', htm:'xml', svg:'xml',
        css:'css', scss:'css', less:'css',
        diff:'diff', patch:'diff',
        ini:'ini', cfg:'ini', conf:'ini', env:'ini',
    })[ext] || null;
}

function _hlKw(words) {
    return ['kw', new RegExp('\\b(' + words.join('|') + ')\\b', 'y')];
}

function hlPatterns(lang) {
    const dbl  = ['str', /"(?:[^"\\]|\\.)*"/y];
    const sgl  = ['str', /'(?:[^'\\]|\\.)*'/y];
    const tpl  = ['str', /`(?:[^`\\]|\\.)*`/y];
    const blkC = ['comment', /\/\*[\s\S]*?\*\//y];
    const num  = ['num', /\b0x[\da-fA-F]+\b|\b\d+\.?\d*(?:[eE][+-]?\d+)?\b/y];
    const lineC = pfx => ['comment', new RegExp(pfx + '[^\\n]*', 'y')];

    switch (lang) {
        case 'json':
            return [dbl, num, _hlKw(['true','false','null'])];

        case 'yaml':
            return [lineC('#'), dbl, sgl,
                    _hlKw(['true','false','null','yes','no','on','off']), num];

        case 'toml':
            return [lineC('#'),
                    ['str', /"""[\s\S]*?"""/y], ['str', /'''[\s\S]*?'''/y],
                    dbl, sgl, num, _hlKw(['true','false']),
                    ['section', /^\[+[^\]\n]+\]+/ym]];

        case 'xml':
            return [['comment', /<!--[\s\S]*?-->/y],
                    ['tag', /<[!?\/]?[a-zA-Z][a-zA-Z0-9:._-]*(?:\s[^>]*)?\s*\/?>/y],
                    dbl, sgl, num];

        case 'css':
            return [blkC, dbl, sgl, num,
                    ['at', /@[a-zA-Z-]+/y],
                    _hlKw(['important','inherit','initial','unset','none','auto','normal'])];

        case 'diff':
            return [
                ['meta', /^(?:\+\+\+|---|\\ No newline at end of file|diff |index |new file|deleted file|rename |Binary |From |commit )[^\n]*/ym],
                ['info', /^@@[^@\n]*@@[^\n]*/ym],
                ['add',  /^\+[^\n]*/ym],
                ['del',  /^-[^\n]*/ym],
            ];

        case 'sh':
            return [lineC('#'), dbl, sgl,
                    ['var', /\$\{?[a-zA-Z_][a-zA-Z0-9_]*\}?/y],
                    _hlKw(['if','then','else','elif','fi','for','while','do','done',
                            'case','esac','in','function','return','export','local',
                            'source','readonly','true','false']),
                    num];

        case 'sql':
            return [lineC('--'), blkC, sgl,
                    _hlKw(['SELECT','INSERT','UPDATE','DELETE','FROM','WHERE','JOIN',
                            'LEFT','RIGHT','INNER','OUTER','FULL','CROSS','ON','GROUP',
                            'ORDER','BY','HAVING','LIMIT','OFFSET','AS','AND','OR','NOT',
                            'IN','IS','NULL','LIKE','CREATE','DROP','ALTER','TABLE',
                            'INDEX','VIEW','DATABASE','BEGIN','COMMIT','ROLLBACK',
                            'WITH','DISTINCT','UNION','VALUES','SET','INTO','CASE',
                            'WHEN','THEN','ELSE','END','COUNT','SUM','AVG','MIN','MAX',
                            'select','insert','update','delete','from','where','join',
                            'left','right','inner','outer','full','cross','on','group',
                            'order','by','having','limit','offset','as','and','or','not',
                            'in','is','null','like','create','drop','alter','table',
                            'index','view','database','begin','commit','rollback',
                            'with','distinct','union','values','set','into','case',
                            'when','then','else','end','count','sum','avg','min','max']),
                    num];

        case 'js':
            return [blkC, lineC('//'), dbl, sgl, tpl,
                    _hlKw(['abstract','as','async','await','break','case','catch','class',
                            'const','continue','debugger','default','delete','do','else',
                            'enum','export','extends','false','finally','for','from',
                            'function','get','if','implements','import','in','instanceof',
                            'interface','let','new','null','of','package','private',
                            'protected','public','return','set','static','super','switch',
                            'this','throw','true','try','type','typeof','undefined','var',
                            'void','while','with','yield']),
                    num];

        case 'py':
            return [['str', /"""[\s\S]*?"""/y], ['str', /'''[\s\S]*?'''/y],
                    lineC('#'), dbl, sgl,
                    _hlKw(['False','None','True','and','as','assert','async','await',
                            'break','class','continue','def','del','elif','else','except',
                            'finally','for','from','global','if','import','in','is',
                            'lambda','nonlocal','not','or','pass','raise','return','try',
                            'while','with','yield']),
                    ['builtin', /\b(print|len|range|enumerate|zip|map|filter|isinstance|type|open|super|property|staticmethod|classmethod|abs|all|any|bool|dict|float|int|list|set|str|tuple)\b/y],
                    num];

        case 'rb':
            return [lineC('#'), dbl, sgl,
                    _hlKw(['BEGIN','END','alias','and','begin','break','case','class',
                            'def','defined','do','else','elsif','end','ensure','false',
                            'for','if','in','module','next','nil','not','or','redo',
                            'rescue','retry','return','self','super','then','true','undef',
                            'unless','until','when','while','yield']),
                    num];

        case 'rs':
            return [blkC, lineC('//'),
                    ['str', /r#+"[^"]*"+#+/y], dbl,
                    ['str', /'(?:[^'\\]|\\.)'/y],   // char literal before lifetime
                    ['lifetime', /'[a-zA-Z_][a-zA-Z0-9_]*/y],
                    _hlKw(['as','async','await','break','const','continue','crate','dyn',
                            'else','enum','extern','false','fn','for','if','impl','in',
                            'let','loop','match','mod','move','mut','pub','ref','return',
                            'self','Self','static','struct','super','trait','true','type',
                            'unsafe','use','where','while']),
                    ['type_', /\b[A-Z][a-zA-Z0-9_]*\b/y],
                    ['macro_', /\b[a-z_][a-z0-9_]*!/y],
                    num];

        case 'go':
            return [blkC, lineC('//'), dbl,
                    ['str', /`[^`]*`/y],
                    _hlKw(['break','case','chan','const','continue','default','defer',
                            'else','fallthrough','for','func','go','goto','if','import',
                            'interface','map','package','range','return','select','struct',
                            'switch','type','var','true','false','nil']),
                    ['type_', /\b[A-Z][a-zA-Z0-9_]*\b/y],
                    num];

        case 'java':
            return [blkC, lineC('//'), dbl, sgl,
                    _hlKw(['abstract','assert','boolean','break','byte','case','catch',
                            'char','class','const','continue','default','do','double',
                            'else','enum','extends','false','final','finally','float','for',
                            'goto','if','implements','import','instanceof','int','interface',
                            'long','native','new','null','package','private','protected',
                            'public','return','short','static','super','switch',
                            'synchronized','this','throw','throws','transient','true','try',
                            'var','void','volatile','while']),
                    ['type_', /\b[A-Z][a-zA-Z0-9_]*\b/y],
                    num];

        case 'c':
            return [blkC, lineC('//'),
                    ['preproc', /#\s*(?:include|define|undef|if|ifdef|ifndef|elif|else|endif|error|pragma|warning)[^\n]*/y],
                    dbl, sgl,
                    _hlKw(['auto','break','case','char','const','continue','default','do',
                            'double','else','enum','extern','float','for','goto','if',
                            'inline','int','long','register','restrict','return','short',
                            'signed','sizeof','static','struct','switch','typedef','union',
                            'unsigned','void','volatile','while','NULL','true','false']),
                    num];

        case 'ini':
            return [['comment', /[#;][^\n]*/y],
                    ['section', /^\[[^\]\n]*\]/ym],
                    dbl, sgl, num];

        default:
            return [num];
    }
}

function highlightCode(text, filename) {
    const lang = hlLang(filename);
    if (!lang) return esc(text);
    const patterns = hlPatterns(lang);
    const html = [];
    let pos = 0;
    let plain = '';
    while (pos < text.length) {
        let matched = false;
        for (const [type, rx] of patterns) {
            rx.lastIndex = pos;
            const m = rx.exec(text);
            if (m && m.index === pos) {
                if (plain) { html.push(esc(plain)); plain = ''; }
                html.push(`<span class="hl-${type}">${esc(m[0])}</span>`);
                pos += m[0].length;
                matched = true;
                break;
            }
        }
        if (!matched) plain += text[pos++];
    }
    if (plain) html.push(esc(plain));
    return html.join('');
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Escape HTML
// ---------------------------------------------------------------------------

function esc(s) {
    if (!s) return '';
    return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
            .replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}

// Escape a value for use as a JS string argument inside a single-quoted string
// literal that is itself inside an HTML attribute (e.g. onclick="fn('...')").
// Browsers HTML-decode attribute values before running JS, so &#39; → ' would
// break the JS string. Instead we use JS backslash-escaping for ' and \, and
// HTML-encode " to avoid ending the outer double-quoted attribute.
function jesc(s) {
    if (!s) return '';
    return s.replace(/\\/g, '\\\\')   // backslash first
             .replace(/'/g, "\\'")     // JS-escape single quote
             .replace(/&/g, '&amp;')
             .replace(/</g, '&lt;')
             .replace(/>/g, '&gt;')
             .replace(/"/g, '&quot;'); // HTML-encode " to avoid breaking attribute
}

// Encode a relative file path for use as a URL path component.
// Unlike encodeURI, this encodes '#' and '?' which would otherwise be
// interpreted as URL fragment / query separators by the browser.
function encodePath(p) {
    return p.split('/').map(encodeURIComponent).join('/');
}
