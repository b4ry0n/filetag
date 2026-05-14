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

// Kind → emoji icon
const KIND_ICONS = {
    'tag-dir':    '🏷',
    'ai-batch':   '🤖',
    'face-scan':  '👤',
    'similarity': '🔗',
    'download':   '⬇',
};

// ---------------------------------------------------------------------------
// Polling
// ---------------------------------------------------------------------------

function startJobPolling() {
    if (_pollTimer !== null) return;
    _pollTimer = setInterval(_pollJobs, 1000);
    _pollJobs(); // immediate first fetch
}

function stopJobPolling() {
    if (_pollTimer !== null) {
        clearInterval(_pollTimer);
        _pollTimer = null;
    }
}

async function _pollJobs() {
    try {
        const data = await api('/api/jobs');
        state.jobs = data.jobs || [];
    } catch (_) {
        // Silently ignore network errors during polling.
    }
    renderJobsBar();
    if (_panelOpen) renderJobsList();

    const hasActive = (state.jobs || []).some(j =>
        j.status === 'pending' || j.status === 'running');
    if (!hasActive && !_panelOpen) {
        stopJobPolling();
    }
}

// ---------------------------------------------------------------------------
// Status-bar button
// ---------------------------------------------------------------------------

function renderJobsBar() {
    const btn = document.getElementById('jobs-btn');
    if (!btn) return;
    const jobs = state.jobs || [];
    if (jobs.length === 0) {
        btn.classList.add('hidden');
        return;
    }
    btn.classList.remove('hidden');

    const active = jobs.filter(j => j.status === 'pending' || j.status === 'running');
    btn.classList.toggle('jobs-btn-running', active.length > 0);

    const countEl = document.getElementById('jobs-btn-count');
    if (countEl) {
        countEl.textContent = active.length > 0
            ? `${active.length} actief`
            : `${jobs.length} klaar`;
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
        renderJobsList();
        startJobPolling();
    }
}

function renderJobsList() {
    const list = document.getElementById('jobs-list');
    if (!list) return;
    const jobs = state.jobs || [];
    if (jobs.length === 0) {
        list.innerHTML = '<div class="jobs-empty">Geen actieve jobs</div>';
        return;
    }
    list.innerHTML = jobs.map(renderJobItem).join('');
}

function renderJobItem(job) {
    const icon = KIND_ICONS[job.kind] || '⚙';
    const pct  = (job.total > 0) ? Math.round(100 * job.done / job.total) : 0;
    const indeterminate = job.status === 'running' && job.total === 0;

    const barFill = indeterminate
        ? `<div class="job-progress-fill indeterminate"></div>`
        : `<div class="job-progress-fill" style="width:${pct}%"></div>`;

    const currentHtml = (job.current && job.status === 'running')
        ? `<div class="job-item-current">${escapeHtml(job.current)}</div>`
        : '';

    let statusHtml = '';
    if (job.status === 'done') {
        statusHtml = `<span class="job-status-done">✓ Klaar${job.total > 0 ? ' (' + job.total + ')' : ''}</span>`;
    } else if (job.status === 'failed') {
        statusHtml = `<span class="job-status-failed">✗ Mislukt</span>`;
    } else if (job.status === 'running' && job.total > 0) {
        statusHtml = `<span class="job-item-count">${job.done} / ${job.total}</span>`;
    }

    const errorHtml = (job.error && job.status === 'failed')
        ? `<div class="job-item-error">${escapeHtml(job.error)}</div>`
        : '';

    const canDismiss = job.status === 'done' || job.status === 'failed';
    const dismissBtn = (canDismiss && !job.id.startsWith('__'))
        ? `<button class="job-item-dismiss" onclick="dismissJob('${escapeHtml(job.id)}')" title="Sluiten">✕</button>`
        : '';

    const progressHtml = (job.status === 'running' || (job.status === 'pending'))
        ? `<div class="job-progress-bar">${barFill}</div>`
        : '';

    return `<div class="job-item">
  <div class="job-item-top">
    <span class="job-kind-icon">${icon}</span>
    <span class="job-item-label" title="${escapeHtml(job.label)}">${escapeHtml(job.label)}</span>
    ${statusHtml}
    ${dismissBtn}
  </div>
  ${progressHtml}
  ${currentHtml}
  ${errorHtml}
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

// ---------------------------------------------------------------------------
// Called externally when a new background job is submitted
// ---------------------------------------------------------------------------

function onJobSubmitted(jobId) {
    startJobPolling();
    const btn = document.getElementById('jobs-btn');
    if (btn) btn.classList.remove('hidden');
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
window.onJobSubmitted    = onJobSubmitted;
window.renderJobsBar     = renderJobsBar;

})();
