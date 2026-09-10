//! Session-owned deep jobs: values are returned only through exact-selector lookup.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use super::{InspectionSession, SessionState};
use crate::inspection::deep::{
    self, DeepBudget, DeepCoverage, DeepResult, DeepSelector, DeepState, DeepStatus,
};
use crate::inspection::{PageRole, ScanControl};

/// Session-wide admission ceilings; rejected work is not retained.
#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeepLimits {
    pub max_jobs: u32,
    pub max_concurrent_jobs: u32,
    pub per_job: DeepBudget,
}

impl Default for DeepLimits {
    fn default() -> Self {
        Self {
            max_jobs: 64,
            max_concurrent_jobs: 4,
            per_job: DeepBudget::default(),
        }
    }
}

struct JobData {
    status: DeepStatus,
    result: Option<DeepResult>,
    reservation: u64,
}

pub struct DeepJob {
    pub id: String,
    target: DeepSelector,
    data: Mutex<JobData>,
    ready: Condvar,
    cancelled: AtomicBool,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl DeepJob {
    fn new(target: DeepSelector, budget: DeepBudget) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        Self {
            id: id.clone(),
            target: target.clone(),
            data: Mutex::new(JobData {
                status: DeepStatus {
                    id,
                    target,
                    budget,
                    state: DeepState::Pending,
                    coverage: DeepCoverage::default(),
                    result_revision: None,
                },
                result: None,
                reservation: 0,
            }),
            ready: Condvar::new(),
            cancelled: AtomicBool::new(false),
            worker: Mutex::new(None),
        }
    }

    fn lock(&self) -> MutexGuard<'_, JobData> {
        self.data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Waits for this job's terminal receipt; it never returns application values.
    #[must_use]
    pub fn wait(&self) -> DeepStatus {
        let mut data = self.lock();
        while data.status.state == DeepState::Pending {
            data = self
                .ready
                .wait(data)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        let status = data.status.clone();
        drop(data);
        // Serialize joiners as well as terminal receipts: every waiter sees cleanup completed.
        let mut worker = self
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(handle) = worker.take() {
            let _ = handle.join();
        }
        status
    }

    #[must_use]
    pub fn status(&self) -> DeepStatus {
        self.lock().status.clone()
    }

    /// True once cancellation was requested for this worker.
    #[must_use]
    pub fn cancellation_requested(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Requests cancellation without stopping unrelated jobs or invalidating a revision.
    pub fn cancel(&self) {
        let data = self.lock();
        if data.status.state == DeepState::Pending {
            self.cancelled.store(true, Ordering::Release);
        }
    }

    fn finish(&self, state: DeepState, reason: &'static str) {
        let mut data = self.lock();
        data.status.state = state;
        data.status.coverage.reason = reason;
        data.result = None;
        self.ready.notify_all();
    }
}

impl InspectionSession {
    /// Closes deep admission, requests cancellation and joins every admitted deep worker.
    /// The adapter joins its fast-scan worker after this returns.
    pub fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::Release);
        let _ = self.stop(ScanControl::Cancel);
        let jobs: Vec<_> = self
            .deep_jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        for job in &jobs {
            job.cancel();
        }
        for job in jobs {
            let _ = job.wait();
        }
    }

    /// Starts asynchronous reconstruction for an explicit current-revision cell selector.
    #[must_use]
    pub fn request_deep(
        self: &Arc<Self>,
        target: DeepSelector,
        budget: DeepBudget,
    ) -> Arc<DeepJob> {
        self.request_deep_observed(target, budget, |_| ScanControl::Continue)
    }

    /// Observes deterministic job boundaries without holding session or job locks.
    /// Observations contain structural progress only, and can cancel this one job.
    #[must_use]
    pub fn request_deep_observed(
        self: &Arc<Self>,
        target: DeepSelector,
        budget: DeepBudget,
        mut observe: impl FnMut(&DeepStatus) -> ScanControl + Send + 'static,
    ) -> Arc<DeepJob> {
        let job = Arc::new(DeepJob::new(target, budget));
        let target = &job.target;
        let graph = {
            let mut data = self.lock();
            if !data.validate() {
                job.finish(DeepState::InvalidatedSnapshot, "snapshot_invalidated");
                return job;
            }
            if target.session_id != data.status.session_id
                || target.snapshot_id != data.status.snapshot_id
            {
                job.finish(DeepState::InvalidTarget, "selector_scope_mismatch");
                return job;
            }
            let Some(graph) = data.published.as_ref() else {
                job.finish(DeepState::InvalidTarget, "revision_unavailable");
                return job;
            };
            if graph.revision != target.revision {
                job.finish(DeepState::StaleRevision, "stale_revision");
                return job;
            }
            let valid = target
                .page_number
                .checked_sub(1)
                .and_then(|index| graph.pages.get(index as usize))
                .filter(|page| {
                    page.classification.reconciled
                        && matches!(
                            page.classification.role,
                            PageRole::TableLeaf | PageRole::IndexLeaf | PageRole::IndexInterior
                        )
                })
                .and_then(|page| page.detail.cells.get(usize::from(target.cell_index)))
                .is_some_and(|cell| {
                    cell.range.is_some()
                        && cell.diagnostic.is_none()
                        && cell.local_payload.is_some()
                });
            if !valid {
                job.finish(DeepState::InvalidTarget, "cell_not_available");
                return job;
            }
            Arc::clone(graph)
        };
        if !self.admit_deep(&job, budget, &graph) {
            return job;
        }
        let session = Arc::clone(self);
        let worker = Arc::clone(&job);
        let mut handle = job
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let spawned = std::thread::Builder::new()
            .name("selected-cell".into())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    session.run_deep(&worker, &graph, budget, &mut observe);
                }));
                if outcome.is_err() {
                    worker.finish(DeepState::Failed, "worker_failed");
                }
            });
        match spawned {
            Ok(worker) => *handle = Some(worker),
            Err(_) => job.finish(DeepState::Failed, "worker_unavailable"),
        }
        drop(handle);
        job
    }

    /// Configures admission before sharing the session with workers.
    #[must_use]
    pub fn with_deep_limits(mut self, limits: DeepLimits) -> Self {
        self.deep_limits = limits;
        self.lock().status.deep_limits = limits;
        self
    }

    fn admit_deep(
        &self,
        job: &Arc<DeepJob>,
        budget: DeepBudget,
        graph: &crate::inspection::InspectionGraph,
    ) -> bool {
        let payload = graph.pages[(job.target.page_number - 1) as usize]
            .detail
            .cells[usize::from(job.target.cell_index)]
        .payload_size
        .unwrap_or(0);
        {
            let mut data = job.lock();
            data.status.coverage.payload_bytes = Some(payload.to_string());
            data.status.coverage.remainder_bytes = Some(payload.to_string());
            data.status.coverage.stopping_page = Some(job.target.page_number);
            data.status.coverage.stopping_payload_offset = Some("0".into());
        }
        let mut jobs = self
            .deep_jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.shutting_down.load(Ordering::Acquire) {
            job.finish(DeepState::Cancelled, "session_shutdown");
            return false;
        }
        let limits = self.deep_limits;
        let active = jobs
            .values()
            .filter(|job| job.lock().status.state == DeepState::Pending)
            .count();
        let reason = if jobs.len() >= limits.max_jobs as usize {
            Some("session_job_budget")
        } else if active >= limits.max_concurrent_jobs as usize {
            Some("concurrent_job_budget")
        } else if budget.max_payload_bytes > limits.per_job.max_payload_bytes
            || budget.max_overflow_pages > limits.per_job.max_overflow_pages
            || budget.max_values > limits.per_job.max_values
            || budget.max_decoded_bytes > limits.per_job.max_decoded_bytes
        {
            Some("session_per_job_budget")
        } else {
            None
        };
        if let Some(reason) = reason {
            job.finish(DeepState::BudgetStopped, reason);
            return false;
        }
        let reservation = payload
            .saturating_mul(8)
            .saturating_add(
                u64::from(budget.max_values)
                    .min(payload)
                    .saturating_mul(1024),
            )
            .saturating_add(revision_reservation(graph));
        let reserved = jobs.values().fold(0_u64, |n, prior| {
            let data = prior.lock();
            if data.status.state == DeepState::Pending {
                n.saturating_add(data.reservation)
            } else {
                n
            }
        });
        let ceiling = self.lock().status.operational_budget.max_resident_bytes;
        if !crate::inspection::budget::memory_available(
            ceiling,
            reserved.saturating_add(reservation),
        ) {
            job.finish(DeepState::BudgetStopped, "resident_memory_budget");
            return false;
        }
        job.lock().reservation = reservation;
        jobs.insert(job.id.clone(), Arc::clone(job));
        true
    }

    fn verify_deep(&self, job: &DeepJob) -> bool {
        let inputs = Arc::clone(&self.lock().inputs);
        match inputs.verify(&job.cancelled) {
            Ok(changes) if changes.is_empty() => true,
            Ok(changes) => {
                self.lock().invalidate(changes);
                job.finish(DeepState::InvalidatedSnapshot, "snapshot_invalidated");
                false
            }
            Err(_) => {
                job.finish(DeepState::Cancelled, "cancelled");
                false
            }
        }
    }

    fn run_deep(
        &self,
        job: &DeepJob,
        graph: &crate::inspection::InspectionGraph,
        budget: DeepBudget,
        observe: &mut impl FnMut(&DeepStatus) -> ScanControl,
    ) {
        if observe(&job.status()) != ScanControl::Continue {
            job.cancel();
        }
        if !self.verify_deep(job) {
            return;
        }
        let inputs = Arc::clone(&self.lock().inputs);
        let ceiling = self.lock().status.operational_budget.max_resident_bytes;
        let payload_bytes = graph.pages[(job.target.page_number - 1) as usize]
            .detail
            .cells[usize::from(job.target.cell_index)]
        .payload_size
        .unwrap_or(0);
        // Payload storage, UTF-16 conversion scratch, decoded text and retained evidence coexist.
        let reservation = payload_bytes.saturating_mul(8).saturating_add(
            u64::from(budget.max_values)
                .min(payload_bytes)
                .saturating_mul(1024),
        );
        if !crate::inspection::budget::memory_available(ceiling, reservation) {
            job.finish(DeepState::BudgetStopped, "resident_memory_budget");
            return;
        }
        let result = deep::inspect(&inputs.file, graph, &job.target, budget, &mut |coverage| {
            job.lock().status.coverage = coverage.clone();
            if observe(&job.status()) != ScanControl::Continue {
                job.cancel();
            }
            !job.cancelled.load(Ordering::Acquire) && self.lock().validate()
        });
        let decoded = match result {
            Ok(decoded) => decoded,
            Err(failure) => {
                job.lock().status.coverage = (*failure.coverage).clone();
                if self.lock().status.state == SessionState::Invalidated {
                    job.finish(DeepState::InvalidatedSnapshot, "snapshot_invalidated");
                } else {
                    job.finish(failure.state, failure.coverage.reason);
                }
                return;
            }
        };
        // Preparation holds no publication locks: cancellation can win during the copy.
        // Copying a revision duplicates its relationship and page evidence as well as the payload.
        let graph_reservation = revision_reservation(graph);
        if !crate::inspection::budget::memory_available(ceiling, graph_reservation) {
            job.lock().status.coverage = decoded.evidence.coverage;
            job.finish(DeepState::BudgetStopped, "resident_memory_budget");
            return;
        }
        let mut enriched = graph.clone();
        enriched
            .deep_inspections
            .retain(|prior| prior.cell != decoded.evidence.cell);
        enriched.deep_inspections.push(decoded.evidence.clone());
        job.lock().status.coverage = decoded.evidence.coverage.clone();
        if observe(&job.status()) != ScanControl::Continue {
            job.cancel();
        }
        if !self.verify_deep(job) {
            return;
        }
        let mut data = self.lock();
        if !data.validate() {
            job.finish(DeepState::InvalidatedSnapshot, "snapshot_invalidated");
            return;
        }
        if data
            .published
            .as_ref()
            .is_none_or(|current| current.revision != job.target.revision)
        {
            job.finish(DeepState::StaleRevision, "stale_revision");
            return;
        }
        let mut outcome = job.lock();
        if job.cancelled.load(Ordering::Acquire) {
            outcome.status.state = DeepState::Cancelled;
            outcome.status.coverage.reason = "cancelled";
        } else if let Some(revision) = graph.revision.checked_add(1) {
            enriched.revision = revision;
            let enriched = Arc::new(enriched);
            data.revisions.insert(revision, Arc::clone(&enriched));
            data.published = Some(enriched);
            data.status.revision = Some(revision);
            outcome.status.state = DeepState::Completed;
            outcome.status.result_revision = Some(revision);
            outcome.result = Some(DeepResult {
                target: job.target.clone(),
                revision,
                evidence: decoded.evidence,
                values: decoded.values,
            });
        } else {
            outcome.status.state = DeepState::Failed;
            outcome.status.coverage.reason = "revision_limit";
        }
        job.ready.notify_all();
    }

    fn deep_job(&self, id: &str) -> Option<Arc<DeepJob>> {
        self.deep_jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .cloned()
    }

    /// Returns structural job status, withholding completed values from routine responses.
    #[must_use]
    pub fn deep_status(&self, id: &str) -> Option<DeepStatus> {
        let job = self.deep_job(id)?;
        let mut status = job.status();
        if !self.lock().validate() {
            status.state = DeepState::InvalidatedSnapshot;
            status.result_revision = None;
            status.coverage.reason = "snapshot_invalidated";
        }
        Some(status)
    }

    #[must_use]
    pub fn cancel_deep(&self, id: &str) -> Option<DeepStatus> {
        self.deep_job(id)?.cancel();
        self.deep_status(id)
    }

    /// Returns values only for the exact selector that authorized this job.
    /// # Errors
    /// Rejects mismatched selectors, unfinished jobs, or changed frozen inputs.
    pub fn deep_result(&self, id: &str, target: &DeepSelector) -> Result<DeepResult, DeepState> {
        let job = self.deep_job(id).ok_or(DeepState::InvalidTarget)?;
        if job.target != *target {
            return Err(DeepState::InvalidTarget);
        }
        let state = job.status().state;
        if state != DeepState::Completed {
            return Err(state);
        }
        self.verify(|| {})
            .map_err(|_| DeepState::InvalidatedSnapshot)?;
        let mut data = self.lock();
        if !data.validate() {
            return Err(DeepState::InvalidatedSnapshot);
        }
        job.lock().result.clone().ok_or(DeepState::InvalidTarget)
    }
}

fn revision_reservation(graph: &crate::inspection::InspectionGraph) -> u64 {
    graph
        .pages
        .iter()
        .fold(0_u64, |n, page| {
            n.saturating_add(4096)
                .saturating_add((page.detail.cells.len() as u64).saturating_mul(4096))
        })
        .saturating_add((graph.relationship_claims.len() as u64).saturating_mul(2048))
        .saturating_add(graph.traversals.iter().fold(0_u64, |n, t| {
            n.saturating_add((t.validated_prefix.len() as u64).saturating_mul(64))
        }))
}
