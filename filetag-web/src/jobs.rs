//! Background job store for long-running tasks.
//!
//! All long-running operations (recursive directory tagging, face scanning,
//! similarity indexing, AI batch analysis) register a [`Job`] here so the
//! frontend can poll progress via `GET /api/jobs`.
//!
//! Existing progress mechanisms (`AiProgress`, `FaceProgress`, …) continue to
//! work internally.  `GET /api/jobs` surfaces them as *synthetic* jobs so the
//! status panel shows everything in one place without requiring a large
//! refactor of the existing code.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;

// ---------------------------------------------------------------------------
// Job model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Pending,
    Running,
    Done,
    Failed,
}

/// A snapshot of a background job's progress.
#[derive(Debug, Clone, Serialize)]
pub struct Job {
    /// Unique identifier (opaque string).
    pub id: String,
    /// Machine-readable kind (e.g. `"tag-dir"`, `"face-scan"`, `"similarity"`,
    /// `"ai-batch"`, `"download"`).  Used by the frontend to pick an icon.
    pub kind: String,
    /// Human-readable description shown in the job panel.
    pub label: String,
    pub status: JobStatus,
    /// Items processed so far.
    pub done: u64,
    /// Total items to process (0 = indeterminate / unknown).
    pub total: u64,
    /// Path or name of the item currently being processed, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    /// Error message when `status == Failed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Unix timestamp (ms) when the job was created.
    pub created_ms: i64,
    /// Unix timestamp (ms) of the last update.
    pub updated_ms: i64,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Job store
// ---------------------------------------------------------------------------

/// Thread-safe store of all active and recently completed background jobs.
#[derive(Default)]
pub struct JobStore {
    jobs: HashMap<String, Job>,
    /// Insertion-ordered list of IDs so the panel keeps a stable order.
    order: Vec<String>,
    /// Monotonic counter for simple collision-free ID generation.
    counter: u64,
}

impl JobStore {
    /// Register a new job and return its ID.  Starts in [`JobStatus::Pending`].
    pub fn submit(&mut self, kind: impl Into<String>, label: impl Into<String>) -> String {
        self.counter += 1;
        let id = format!("job-{}-{}", now_ms(), self.counter);
        let now = now_ms();
        self.order.push(id.clone());
        self.jobs.insert(
            id.clone(),
            Job {
                id: id.clone(),
                kind: kind.into(),
                label: label.into(),
                status: JobStatus::Pending,
                done: 0,
                total: 0,
                current: None,
                error: None,
                created_ms: now,
                updated_ms: now,
            },
        );
        id
    }

    /// Transition to [`JobStatus::Running`] and record the expected total.
    pub fn start(&mut self, id: &str, total: u64) {
        if let Some(j) = self.jobs.get_mut(id) {
            j.status = JobStatus::Running;
            j.total = total;
            j.updated_ms = now_ms();
        }
    }

    /// Update `done` counter and optionally the currently-processing item name.
    pub fn progress(&mut self, id: &str, done: u64, current: Option<&str>) {
        if let Some(j) = self.jobs.get_mut(id) {
            j.status = JobStatus::Running;
            j.done = done;
            j.current = current.map(|s| s.to_string());
            j.updated_ms = now_ms();
        }
    }

    /// Mark a job as successfully completed.
    pub fn finish(&mut self, id: &str) {
        if let Some(j) = self.jobs.get_mut(id) {
            j.status = JobStatus::Done;
            j.current = None;
            if j.total > 0 {
                j.done = j.total;
            }
            j.updated_ms = now_ms();
        }
    }

    /// Mark a job as failed with an error message.
    pub fn fail(&mut self, id: &str, error: impl Into<String>) {
        if let Some(j) = self.jobs.get_mut(id) {
            j.status = JobStatus::Failed;
            j.error = Some(error.into());
            j.current = None;
            j.updated_ms = now_ms();
        }
    }

    /// Remove a specific job entry (dismiss from the panel).
    pub fn dismiss(&mut self, id: &str) {
        self.jobs.remove(id);
        self.order.retain(|i| i != id);
    }

    /// Remove all jobs that are neither `Pending` nor `Running`.
    pub fn dismiss_finished(&mut self) {
        self.order.retain(|id| {
            let active = self
                .jobs
                .get(id)
                .is_some_and(|j| matches!(j.status, JobStatus::Pending | JobStatus::Running));
            if !active {
                self.jobs.remove(id);
            }
            active
        });
    }

    /// Return all jobs in creation order (oldest first).
    pub fn list(&self) -> Vec<&Job> {
        self.order
            .iter()
            .filter_map(|id| self.jobs.get(id))
            .collect()
    }

    /// Returns `true` when at least one job is `Pending` or `Running`.
    #[allow(dead_code)]
    pub fn has_active(&self) -> bool {
        self.jobs
            .values()
            .any(|j| matches!(j.status, JobStatus::Pending | JobStatus::Running))
    }
}

pub type SharedJobStore = Arc<Mutex<JobStore>>;

// ---------------------------------------------------------------------------
// Routing helper
// ---------------------------------------------------------------------------

/// File count above which a tagging operation should run as a background job.
#[allow(dead_code)]
pub const BG_THRESHOLD: usize = 20;

/// Returns `true` when the estimated number of items warrants dispatching the
/// operation as a background job rather than blocking the request handler.
#[allow(dead_code)]
pub fn needs_background(item_count: usize) -> bool {
    item_count > BG_THRESHOLD
}
