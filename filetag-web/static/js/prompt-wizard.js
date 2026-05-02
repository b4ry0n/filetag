// prompt-wizard.js — Fully-automatic Prompt Optimisation Wizard
// Two passes: generate → refine. No user interaction needed after opening.

(function () {
    'use strict';

    // ---------------------------------------------------------------------------
    // Open / close
    // ---------------------------------------------------------------------------

    window.openPromptWizard = function () {
        const goals = (document.getElementById('ai-subject') || {}).value || '';

        // Reset state
        document.getElementById('pw-progress-area').hidden = false;
        document.getElementById('pw-result-area').hidden = true;
        document.getElementById('pw-error-area').hidden = true;
        document.getElementById('pw-title').textContent = '\u2728 Optimising prompts\u2026';
        document.getElementById('pw-close-btn').disabled = true;
        _setStepState(1, 'pending');
        _setStepState(2, 'pending');

        document.getElementById('prompt-wizard-modal').hidden = false;

        _runWizard(goals);
    };

    window.closePromptWizard = function () {
        document.getElementById('prompt-wizard-modal').hidden = true;
    };

    // ---------------------------------------------------------------------------
    // Automatic two-pass pipeline
    // ---------------------------------------------------------------------------

    async function _runWizard(goals) {
        try {
            // Pass 1 — generate initial prompts
            _setStepState(1, 'active');
            const draft = await _callWizard(goals, null);
            _setStepState(1, 'done');

            // Pass 2 — review and refine
            _setStepState(2, 'active');
            const final = await _callWizard(goals, draft);
            _setStepState(2, 'done');

            _showResults(final);

        } catch (err) {
            _showError(err.message);
        } finally {
            document.getElementById('pw-close-btn').disabled = false;
        }
    }

    async function _callWizard(goals, draft) {
        const body = {
            dir: (typeof currentAbsDir === 'function' ? currentAbsDir() : null) || '',
            goals,
            draft: draft || null,
        };
        const r = await fetch('/api/ai/prompt-wizard', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(body),
        });
        if (!r.ok) {
            const msg = await r.text().catch(() => r.statusText);
            throw new Error(msg || 'HTTP ' + r.status);
        }
        const data = await r.json();
        return data.prompts;
    }

    // ---------------------------------------------------------------------------
    // Apply results to settings
    // ---------------------------------------------------------------------------

    window.pwApplyToSettings = function () {
        const map = [
            ['pw-result-subject',       'ai-subject'],
            ['pw-result-image',         'ai-prompt-image'],
            ['pw-result-video',         'ai-prompt-video'],
            ['pw-result-archive',       'ai-prompt-archive'],
            ['pw-result-output-format', 'ai-output-format'],
        ];
        for (const [src, dst] of map) {
            const val = (document.getElementById(src) || {}).value || '';
            const el  = document.getElementById(dst);
            if (el && val.trim()) el.value = val.trim();
        }
        closePromptWizard();
    };

    // ---------------------------------------------------------------------------
    // UI helpers
    // ---------------------------------------------------------------------------

    function _showResults(prompts) {
        document.getElementById('pw-result-subject').value        = prompts.subject        || '';
        document.getElementById('pw-result-image').value          = prompts.prompt_image   || '';
        document.getElementById('pw-result-video').value          = prompts.prompt_video   || '';
        document.getElementById('pw-result-archive').value        = prompts.prompt_archive || '';
        document.getElementById('pw-result-output-format').value  = prompts.output_format  || '';

        document.getElementById('pw-progress-area').hidden = true;
        document.getElementById('pw-result-area').hidden   = false;
        document.getElementById('pw-title').textContent    = '\u2728 Optimised prompts';
    }

    function _showError(msg) {
        const el = document.getElementById('pw-error-area');
        el.textContent = 'Error: ' + msg;
        el.hidden = false;
        document.getElementById('pw-title').textContent = '\u2728 Optimisation failed';
    }

    /** state: 'pending' | 'active' | 'done' | 'error' */
    function _setStepState(n, stepState) {
        const step = document.getElementById('pw-step-' + n);
        if (!step) return;
        step.className = 'pw-step pw-step-' + stepState;
        const icon = step.querySelector('.pw-step-icon');
        if (!icon) return;
        if (stepState === 'active') {
            icon.innerHTML = '<span class="pw-spinner-sm"></span>';
        } else if (stepState === 'done') {
            icon.innerHTML = '<svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="var(--accent)" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="2,7 6,11 12,3"/></svg>';
        } else if (stepState === 'error') {
            icon.innerHTML = '<svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="var(--danger)" stroke-width="2" stroke-linecap="round"><line x1="2" y1="2" x2="12" y2="12"/><line x1="12" y1="2" x2="2" y2="12"/></svg>';
        } else {
            icon.innerHTML = '';
        }
    }

    // Close on overlay click (only when not busy)
    document.addEventListener('DOMContentLoaded', function () {
        const modal = document.getElementById('prompt-wizard-modal');
        if (modal) {
            modal.addEventListener('click', function (e) {
                if (e.target === this && !document.getElementById('pw-close-btn').disabled) {
                    closePromptWizard();
                }
            });
        }
    });

}());
