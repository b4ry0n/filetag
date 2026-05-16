/* =========================================================
 * jobs.js — Background job store, polling, and rendering
 * ========================================================= */

'use strict';

(function () {

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

let _pollTimer = null;
let _panelOpen = false;
let _lastJobsJson = '';  // last serialised snapshot; avoids redundant DOM updates
let _doneExpanded = false; // whether the collapsed "done" section is open
let _evtSource = null;   // EventSource for SSE job updates

// Callbacks to fire when a specific job reaches 'done' status.
// Map<job_id, Array<Function>>
const _jobDoneCallbacks = new Map();

// Kind → emoji icon
const KIND_ICONS = {
    'tag-dir':      '🏷',
    'sprites':      '🎞',
    'tile-preview': '🎬',
    'ai-batch':     '🤖',
    'face-scan':    '👤',
    'similarity':   '🔗',
    'download':     '⬇',
};

// ---------------------------------------------------------------------------
// SSE connection + data handling
// ---------------------------------------------------------------------------

function startJobPolling() {
    if (_evtSource !== null) return;  // already open
    _evtSource = new EventSource('/api/jobs/stream');
    _evtSource.onmessage = (evt) => {
        let data;
        try { data = JSON.parse(evt.data); } catch (_) { return; }
        _applyJobsData(data);
    };
    // Browser auto-reconnects on transient errors.  If the source permanently
    // closes (page going offline), null it so the next call can reopen it.
    _evtSource.onerror = () => {
        if (_evtSource && _evtSource.readyState === EventSource.CLOSED) {
            _evtSource = null;
        }
    };
}

function stopJobPolling() {
    // Kept for backward compatibility; the SSE connection stays open.
}

function _applyJobsData(data) {
    state.jobs = data.jobs || [];

    const newJson = JSON.stringify(state.jobs);
    const changed = newJson !== _lastJobsJson;
    _lastJobsJson = newJson;

    // Fire done callbacks before re-rendering so callbacks can update DOM immediately.
    if (_jobDoneCallbacks.size > 0) {
        (state.jobs || []).forEach(j => {
            if (j.status === 'done' && _jobDoneCallbacks.has(j.id)) {
                const cbs = _jobDoneCallbacks.get(j.id);
                _jobDoneCallbacks.delete(j.id);
                cbs.forEach(cb => { try { cb(j); } catch (_) {} });
            }
        });
    }
    if (changed) renderJobsBar();
    if (_panelOpen && changed) renderJobsList();
}

// ---------------------------------------------------------------------------
// Status-bar button
// ---------------------------------------------------------------------------

function renderJobsBar() {
    // The header jobs button is removed; the status bar is the sole indicator.
    renderStatusBar();
}

// ---------------------------------------------------------------------------
// Status bar
// ---------------------------------------------------------------------------

function renderStatusBar() {
    const sbBtn   = document.getElementById('statusbar-jobs');
    const sbLabel = document.getElementById('statusbar-jobs-label');
    if (!sbBtn || !sbLabel) return;

    const jobs   = state.jobs || [];
    const active = jobs.filter(j => j.status === 'pending' || j.status === 'running');
    const done   = jobs.filter(j => j.status === 'done' || j.status === 'failed');

    if (active.length > 0) {
        sbBtn.className = 'statusbar-jobs jobs-active';
        const kinds = [...new Set(active.map(j => KIND_ICONS[j.kind] || '⚙'))];
        sbLabel.textContent = `${kinds.join('')} ${active.length} bezig`;
    } else if (jobs.length > 0) {
        sbBtn.className = 'statusbar-jobs jobs-done';
        sbLabel.textContent = `✓ ${done.length} klaar`;
    } else {
        sbBtn.className = 'statusbar-jobs';
        sbLabel.textContent = '';
    }
}

// ---------------------------------------------------------------------------
// Panel
// ---------------------------------------------------------------------------

function toggleJobsPanel() {
    _panelOpen = !_panelOpen;
    const panel = document.getElementById('jobs-panel');
    if (panel) panel.classList.toggle('hidden', !_panelOpen);
    if (_panelOpen) {
        _lastJobsJson = '';  // force a full render on open
        renderJobsList();
        startJobPolling();
    }
}

function renderJobsList() {
    const list = document.getElementById('jobs-list');
    if (!list) return;
    const jobs     = state.jobs || [];
    const active   = jobs.filter(j => j.status !== 'done' && j.status !== 'failed');
    const finished = jobs.filter(j => j.status === 'done'  || j.status === 'failed');

    if (active.length === 0 && finished.length === 0) {
        list.innerHTML = '<div class="jobs-empty">Geen actieve jobs</div>';
        return;
    }

    let html = active.map(renderJobItem).join('');

    if (finished.length > 0) {
        const arrow = _doneExpanded ? '\u25be' : '\u25b8'; // ▾ / ▸
        const inner = _doneExpanded
            ? `<div class="jobs-done-list">${finished.map(renderJobItem).join('')}</div>`
            : '';
        html += `<div class="jobs-done-section">
  <button class="jobs-done-toggle" onclick="toggleDoneJobs()">${arrow}\u2002${finished.length} afgerond</button>${inner ? '\n  ' + inner : ''}
</div>`;
    }

    list.innerHTML = html;
}

function renderJobItem(job) {
    const icon = KIND_ICONS[job.kind] || '\u2699'; // ⚙
    const pct  = (job.total > 0) ? Math.round(100 * job.done / job.total) : 0;
    const isActive      = job.status === 'running' || job.status === 'pending';
    const indeterminate = job.status === 'running' && job.total === 0;

    // Progress fill: translucent colour expanding behind row content.
    let progressBg = '';
    if (isActive) {
        const cls   = indeterminate ? ' indeterminate' : '';
        const style = (!indeterminate) ? ` style="width:${pct}%"` : '';
        progressBg = `<div class="job-progress-bg${cls}"${style}></div>\n  `;
    }

    let statusHtml = '';
    if (job.status === 'done') {
        statusHtml = `<span class="job-status-done">\u2713${job.total > 0 ? '\u202f' + job.total : ''}</span>`;
    } else if (job.status === 'failed') {
        statusHtml = `<span class="job-status-failed">\u2717</span>`;
    } else if (isActive && job.total > 0) {
        statusHtml = `<span class="job-item-count">${job.done}/${job.total}</span>`;
    }

    const canDismiss = job.status === 'done' || job.status === 'failed';
    const dismissBtn = (canDismiss && !job.id.startsWith('__'))
        ? `<button class="job-item-dismiss" onclick="dismissJob('${escapeHtml(job.id)}')" title="Sluiten">\u2715</button>`
        : '';

    const currentHtml = (job.current && job.status === 'running')
        ? `\n  <div class="job-item-current">${escapeHtml(job.current)}</div>`
        : '';
    const errorHtml = (job.error && job.status === 'failed')
        ? `\n  <div class="job-item-error">${escapeHtml(job.error)}</div>`
        : '';

    return `<div class="job-item">
  ${progressBg}<div class="job-item-top">
    <span class="job-kind-icon">${icon}</span>
    <span class="job-item-label" title="${escapeHtml(job.label)}">${escapeHtml(job.label)}</span>
    ${statusHtml}
    ${dismissBtn}
  </div>${currentHtml}${errorHtml}
</div>`;
}

// ---------------------------------------------------------------------------
// Dismiss
// ---------------------------------------------------------------------------

async function dismissJob(id) {
    try {
        await fetch(`/api/jobs/${encodeURIComponent(id)}`, { method: 'DELETE' });
    } catch (_) {}
    state.jobs = (state.jobs || []).filter(j => j.id !== id);
    renderJobsBar();
    if (_panelOpen) renderJobsList();
}

async function dismissAllJobs() {
    try {
        await fetch('/api/jobs', { method: 'DELETE' });
    } catch (_) {}
    state.jobs = (state.jobs || []).filter(
        j => j.status === 'pending' || j.status === 'running');
    renderJobsBar();
    if (_panelOpen) renderJobsList();
}

function toggleDoneJobs() {
    _doneExpanded = !_doneExpanded;
    renderJobsList();
}

// ---------------------------------------------------------------------------
// Called externally when a new background job is submitted
// ---------------------------------------------------------------------------

function onJobSubmitted(jobId) {
    startJobPolling();
    const btn = document.getElementById('jobs-btn');
    if (btn) btn.classList.remove('hidden');
}

/**
 * Register a callback to be invoked once when the given job reaches 'done'
 * status.  If the job is already done in the current snapshot the callback
 * fires on the next poll tick.  Polling is started automatically.
 */
function whenJobDone(jobId, callback) {
    if (!_jobDoneCallbacks.has(jobId)) _jobDoneCallbacks.set(jobId, []);
    _jobDoneCallbacks.get(jobId).push(callback);
    startJobPolling();
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

function escapeHtml(str) {
    if (!str) return '';
    return String(str)
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;');
}

// ---------------------------------------------------------------------------
// Exports (attach to window for inline event handlers)
// ---------------------------------------------------------------------------

window.startJobPolling   = startJobPolling;
window.stopJobPolling    = stopJobPolling;
window.toggleJobsPanel   = toggleJobsPanel;
window.dismissJob        = dismissJob;
window.dismissAllJobs    = dismissAllJobs;
window.toggleDoneJobs    = toggleDoneJobs;
window.onJobSubmitted    = onJobSubmitted;
window.whenJobDone       = whenJobDone;
window.renderJobsBar     = renderJobsBar;
window.renderStatusBar   = renderStatusBar;

// Open the SSE connection immediately so pre-existing jobs are visible
// as soon as the page loads, without waiting for a user interaction.
startJobPolling();

})();
