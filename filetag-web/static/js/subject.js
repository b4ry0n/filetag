// subject.js: subject helpers for creation if missing

import { apiPost } from './state.js';

export async function ensureSubjectExists(subject) {
    if (!subject) return;
    // Check if subject already exists in state.subjects
    if (window.state && window.state.subjects && window.state.subjects.some(s => s.name === subject)) {
        return;
    }
    // Try to create subject
    try {
        await apiPost('/api/subject/add', { name: subject, dir: currentAbsDir() });
        // Optionally reload subjects
        if (window.loadSubjects) await window.loadSubjects();
    } catch (e) {
        // Ignore if already exists, else show error
        if (!String(e.message).includes('already exists')) {
            if (window.showToast) window.showToast('Subject aanmaken mislukt: ' + e.message, 4000);
        }
    }
}
