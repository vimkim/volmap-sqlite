use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    DatabaseSnapshot, InspectionError, InspectionGraph, PageEntity, SourceIdentity, read_snapshot,
    sanitized_display_name,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Scanning,
    Published,
    Cancelled,
    Stopped,
    Invalidated,
    Fatal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanControl {
    Continue,
    Cancel,
    Stop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageReason {
    Complete,
    Cancelled,
    OperatorStop,
    InputChanged,
    FatalGeometry,
    AllocationFailure,
}

/// Coverage is explicitly limited to page inventory; later structural passes have their own scope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    pub scope: &'static str,
    pub evaluated: u32,
    pub total: Option<u32>,
    pub next_page: Option<u32>,
    pub reason: CoverageReason,
    pub remainder: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Progress {
    pub unit: &'static str,
    pub completed: u32,
    pub total: Option<u32>,
    pub verifying: bool,
    pub building_topology: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatus {
    pub snapshot_id: String,
    pub source: SourceIdentity,
    pub state: SessionState,
    pub progress: Progress,
    pub revision: Option<u64>,
    pub coverage: Option<Coverage>,
    pub diagnostic: Option<Diagnostic>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub code: &'static str,
    pub message: String,
    pub affected_inputs: Vec<&'static str>,
}

/// Observations retained after invalidation have no revision identity.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedEvidence {
    pub snapshot: DatabaseSnapshot,
    pub pages: Vec<PageEntity>,
    pub coverage: Coverage,
    pub relationship_claims: Vec<super::RelationshipClaim>,
    pub relationships: Vec<super::Relationship>,
    pub traversals: Vec<super::Traversal>,
    pub diagnostics: Vec<super::StructuralDiagnostic>,
    pub topology_coverage: super::TopologyCoverage,
}

/// Metadata excludes access time, so inspection reads cannot invalidate themselves.
/// Device/inode detect replacement; nanosecond mtime/ctime detect content and metadata changes.
#[derive(Eq, PartialEq)]
struct Stamp {
    dev: u64,
    ino: u64,
    len: u64,
    mode: u32,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

impl From<Metadata> for Stamp {
    fn from(m: Metadata) -> Self {
        Self {
            dev: m.dev(),
            ino: m.ino(),
            len: m.len(),
            mode: m.mode(),
            mtime: (m.mtime(), m.mtime_nsec()),
            ctime: (m.ctime(), m.ctime_nsec()),
        }
    }
}

#[derive(Eq, PartialEq)]
struct InputStamp {
    link: Stamp,
    target: Option<Stamp>,
}

fn stamp(path: &Path) -> io::Result<Option<InputStamp>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(InputStamp {
            target: if metadata.is_symlink() {
                Some(fs::metadata(path)?.into())
            } else {
                None
            },
            link: metadata.into(),
        })),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

struct Inputs {
    accepted: Vec<AcceptedInput>,
    file: File,
    descriptor: Stamp,
}

struct AcceptedInput {
    kind: &'static str,
    path: PathBuf,
    stamp: Option<InputStamp>,
    digest: Option<[u8; 32]>,
}

impl AcceptedInput {
    fn length(&self) -> u64 {
        self.stamp
            .as_ref()
            .map_or(0, |stamp| stamp.target.as_ref().unwrap_or(&stamp.link).len)
    }
}

fn digest(path: &Path, length: u64, cancelled: &AtomicBool) -> io::Result<Option<[u8; 32]>> {
    match fs::metadata(path) {
        Ok(metadata) if !metadata.is_file() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "inspection inputs must be regular files",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
        Ok(_) => {}
    }
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "inspection inputs must be regular files",
        ));
    }
    let mut hash = Sha256::new();
    // Never chase a growing file's EOF. A later metadata comparison detects growth.
    let mut file = file.take(length);
    let mut buffer = [0_u8; 16_384];
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "verification cancelled",
            ));
        }
        let read = match file.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(Some(hash.finalize().into()))
}

impl Inputs {
    fn accept(path: &Path) -> Result<Self, InspectionError> {
        let sidecar = |suffix: &str| {
            let mut p = path.as_os_str().to_owned();
            p.push(suffix);
            PathBuf::from(p)
        };
        let paths = [
            ("main", path.to_owned()),
            ("wal", sidecar("-wal")),
            ("journal", sidecar("-journal")),
            ("shm", sidecar("-shm")),
        ];
        let mut accepted = paths
            .into_iter()
            .map(|(kind, path)| {
                Ok(AcceptedInput {
                    stamp: stamp(&path)?,
                    kind,
                    path,
                    digest: None,
                })
            })
            .collect::<io::Result<Vec<_>>>()
            .map_err(InspectionError::Open)?;
        if !fs::metadata(path).map_err(InspectionError::Open)?.is_file() {
            return Err(InspectionError::Open(io::Error::new(
                io::ErrorKind::InvalidInput,
                "inspection inputs must be regular files",
            )));
        }
        let file = File::open(path).map_err(InspectionError::Open)?;
        let descriptor = file.metadata().map_err(InspectionError::Open)?.into();
        for input in &mut accepted {
            input.digest = digest(&input.path, input.length(), &AtomicBool::new(false))
                .map_err(InspectionError::Open)?;
        }
        let inputs = Self {
            accepted,
            file,
            descriptor,
        };
        if !inputs.changes().is_empty() {
            return Err(InspectionError::Invalidated);
        }
        Ok(inputs)
    }

    fn changes(&self) -> Vec<&'static str> {
        let mut changes: Vec<_> = self
            .accepted
            .iter()
            .filter_map(|input| match stamp(&input.path) {
                Ok(current) if current == input.stamp => None,
                _ => Some(input.kind),
            })
            .collect();
        let descriptor_matches = self
            .file
            .metadata()
            .is_ok_and(|m| Stamp::from(m) == self.descriptor);
        let accepted_matches = self.accepted[0]
            .stamp
            .as_ref()
            .is_some_and(|s| s.target.as_ref().unwrap_or(&s.link) == &self.descriptor);
        if (!descriptor_matches || !accepted_matches) && !changes.contains(&"main") {
            changes.push("main");
        }
        changes
    }

    fn verify(&self, cancelled: &AtomicBool) -> Result<Vec<&'static str>, InspectionError> {
        let mut changes = self.changes();
        if changes.is_empty() {
            for input in &self.accepted {
                let result = digest(&input.path, input.length(), cancelled);
                if result
                    .as_ref()
                    .is_err_and(|error| error.kind() == io::ErrorKind::Interrupted)
                {
                    return Err(InspectionError::NotScanning);
                }
                if !result.is_ok_and(|current| current == input.digest)
                    && !changes.contains(&input.kind)
                {
                    changes.push(input.kind);
                }
            }
            // Catch replacement or writes that occur while the streaming digest is read.
            for name in self.changes() {
                if !changes.contains(&name) {
                    changes.push(name);
                }
            }
        }
        Ok(changes)
    }
}

struct SessionData {
    inputs: Arc<Inputs>,
    status: SessionStatus,
    snapshot: Option<DatabaseSnapshot>,
    pages: Arc<Vec<PageEntity>>,
    published: Option<Arc<InspectionGraph>>,
    traversal_budget: super::TraversalBudget,
    topology_active: bool,
    topology_cancellation: Arc<super::topology::WorkControl>,
    pending_topology_stop: Option<(SessionState, CoverageReason)>,
    observed_topology: Option<super::topology::Topology>,
}

impl SessionData {
    fn coverage(&self, reason: CoverageReason) -> Coverage {
        let p = &self.status.progress;
        Coverage {
            scope: "page_inventory",
            evaluated: p.completed,
            total: p.total,
            next_page: p
                .total
                .filter(|total| p.completed < *total)
                .map(|_| p.completed + 1),
            reason,
            remainder: p.total.map(|total| total - p.completed),
        }
    }

    fn validate(&mut self) -> bool {
        if self.status.state == SessionState::Invalidated {
            return false;
        }
        let changes = self.inputs.changes();
        if changes.is_empty() {
            return true;
        }
        self.invalidate(changes);
        false
    }

    fn invalidate(&mut self, changes: Vec<&'static str>) {
        self.topology_cancellation.cancel();
        self.status.state = SessionState::Invalidated;
        self.status.revision = None;
        self.status.coverage = Some(self.coverage(CoverageReason::InputChanged));
        self.status.diagnostic = Some(Diagnostic { code: "input_changed",
            message: "Accepted input changed; retained observations are diagnostic evidence, not a coherent snapshot.".into(),
            affected_inputs: changes });
    }

    fn publish(
        &mut self,
        state: SessionState,
        reason: CoverageReason,
        topology: super::topology::Topology,
    ) {
        let coverage = self.coverage(reason);
        if let Some(snapshot) = &self.snapshot {
            let pages = Arc::try_unwrap(std::mem::replace(&mut self.pages, Arc::new(Vec::new())))
                .unwrap_or_else(|pages| (*pages).clone());
            self.published = Some(Arc::new(InspectionGraph {
                revision: 1,
                coverage: coverage.clone(),
                snapshot: snapshot.clone(),
                pages,
                relationship_claims: topology.claims,
                relationships: topology.relationships,
                traversals: topology.traversals,
                diagnostics: topology.diagnostics,
                topology_coverage: topology.coverage,
            }));
            self.status.revision = Some(1);
        }
        self.status.coverage = Some(coverage);
        self.status.state = state;
    }
}

/// Owns accepted input handles and publishes only immutable, validated revisions.
pub struct InspectionSession {
    data: Mutex<SessionData>,
    active_checks: AtomicUsize,
    cancel_verification: AtomicBool,
}

struct ActiveCheck<'a>(&'a AtomicUsize);

impl Drop for ActiveCheck<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl std::fmt::Debug for InspectionSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InspectionSession")
            .field("status", &self.lock().status)
            .finish_non_exhaustive()
    }
}

impl InspectionSession {
    /// Accepts inputs without publishing any graph.
    /// # Errors
    /// Returns an input-access or invalidation error if acceptance cannot establish a stable input set.
    pub fn begin(path: &Path) -> Result<Self, InspectionError> {
        Self::begin_with_traversal_budget(path, super::TraversalBudget::default())
    }

    /// Accepts inputs with explicit relationship traversal ceilings without publishing a graph.
    /// # Errors
    /// Returns an input-access or invalidation error if acceptance cannot establish a stable input set.
    pub fn begin_with_traversal_budget(
        path: &Path,
        traversal_budget: super::TraversalBudget,
    ) -> Result<Self, InspectionError> {
        Ok(Self {
            active_checks: AtomicUsize::new(0),
            cancel_verification: AtomicBool::new(false),
            data: Mutex::new(SessionData {
                inputs: Arc::new(Inputs::accept(path)?),
                status: SessionStatus {
                    snapshot_id: Uuid::new_v4().to_string(),
                    source: SourceIdentity {
                        id: Uuid::new_v4().to_string(),
                        display_name: sanitized_display_name(path),
                    },
                    state: SessionState::Scanning,
                    progress: Progress {
                        unit: "pages",
                        completed: 0,
                        total: None,
                        verifying: false,
                        building_topology: false,
                    },
                    revision: None,
                    coverage: None,
                    diagnostic: None,
                },
                snapshot: None,
                pages: Arc::new(Vec::new()),
                published: None,
                traversal_budget,
                topology_active: false,
                topology_cancellation: Arc::new(super::topology::WorkControl::new()),
                pending_topology_stop: None,
                observed_topology: None,
            }),
        })
    }

    /// Accepts inputs and finishes the page inventory synchronously.
    /// # Errors
    /// Returns geometry, input-access, or invalidation errors.
    pub fn open(path: &Path) -> Result<Self, InspectionError> {
        let session = Self::begin(path)?;
        session.scan(|_| ScanControl::Continue)?;
        Ok(session)
    }

    /// Accepts inputs and finishes the inventory with explicit traversal ceilings.
    /// # Errors
    /// Returns geometry, input-access, or invalidation errors.
    pub fn open_with_traversal_budget(
        path: &Path,
        traversal_budget: super::TraversalBudget,
    ) -> Result<Self, InspectionError> {
        let session = Self::begin_with_traversal_budget(path, traversal_budget)?;
        session.scan(|_| ScanControl::Continue)?;
        Ok(session)
    }

    fn lock(&self) -> MutexGuard<'_, SessionData> {
        self.data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    // Expensive reads run outside the state mutex. A stop can abort between buffers.
    fn verify(&self, checkpoint: impl FnOnce()) -> Result<(), InspectionError> {
        let inputs = {
            let mut d = self.lock();
            if !d.validate() {
                return Err(InspectionError::Invalidated);
            }
            self.active_checks.fetch_add(1, Ordering::AcqRel);
            Arc::clone(&d.inputs)
        };
        let _active = ActiveCheck(&self.active_checks);
        checkpoint();
        let result = inputs.verify(&self.cancel_verification);
        let changes = result?;
        let mut d = self.lock();
        if !changes.is_empty() {
            d.invalidate(changes);
        }
        if !d.validate() {
            return Err(InspectionError::Invalidated);
        }
        Ok(())
    }

    fn finish(
        &self,
        state: SessionState,
        reason: CoverageReason,
        checkpoint: impl FnOnce(),
    ) -> Result<(), InspectionError> {
        self.verify(checkpoint)?;
        let (inputs, snapshot, pages, budget, cancellation) = {
            let mut d = self.lock();
            if !d.validate() {
                return Err(InspectionError::Invalidated);
            }
            if d.status.state != SessionState::Scanning {
                return Err(InspectionError::NotScanning);
            }
            if d.topology_active {
                return Err(InspectionError::NotScanning);
            }
            d.topology_active = true;
            d.topology_cancellation.reset();
            (
                Arc::clone(&d.inputs),
                d.snapshot.clone(),
                Arc::clone(&d.pages),
                d.traversal_budget,
                Arc::clone(&d.topology_cancellation),
            )
        };
        let topology = snapshot.as_ref().map_or_else(
            || super::topology::Topology::empty(budget),
            |snapshot| {
                super::topology::inspect(
                    &inputs.file,
                    &snapshot.geometry,
                    &pages,
                    budget,
                    &cancellation,
                )
            },
        );
        drop(pages);
        let (publish_state, publish_reason, pending_stop) = {
            let mut d = self.lock();
            d.topology_active = false;
            if d.status.state == SessionState::Invalidated {
                d.observed_topology = Some(topology);
                return Err(InspectionError::Invalidated);
            }
            if let Some((pending_state, pending_reason)) = d.pending_topology_stop.take() {
                (pending_state, pending_reason, true)
            } else if d.status.state == SessionState::Scanning {
                (state, reason, false)
            } else {
                return Ok(());
            }
        };
        // Topology follows overflow pointers with additional positional reads.
        // Verify content again before publishing those facts as one revision.
        if let Err(error) = self.verify(|| {}) {
            if matches!(error, InspectionError::Invalidated) {
                self.lock().observed_topology = Some(topology);
            }
            return Err(error);
        }
        let mut d = self.lock();
        if !d.validate() {
            d.observed_topology = Some(topology);
            return Err(InspectionError::Invalidated);
        }
        if (!pending_stop && d.status.state != SessionState::Scanning)
            || (pending_stop && d.status.state != publish_state)
        {
            return Err(InspectionError::NotScanning);
        }
        d.publish(publish_state, publish_reason, topology);
        Ok(())
    }

    /// Observes progress independently of a navigable graph and rechecks accepted inputs.
    #[must_use]
    pub fn status(&self) -> SessionStatus {
        let mut data = self.lock();
        data.validate();
        let mut status = data.status.clone();
        status.progress.verifying = self.active_checks.load(Ordering::Acquire) > 0;
        status.progress.building_topology = data.topology_active;
        status
    }

    /// Runs one bounded inventory unit. Publication itself is a separate, validated boundary.
    /// # Errors
    /// Returns an error for invalid geometry, changed inputs, or work requested after a terminal state.
    pub fn advance(&self) -> Result<(), InspectionError> {
        let mut d = self.lock();
        if !d.validate() {
            return Err(InspectionError::Invalidated);
        }
        if d.status.state != SessionState::Scanning {
            return Err(InspectionError::NotScanning);
        }
        if d.topology_active {
            return Err(InspectionError::NotScanning);
        }
        if d.snapshot.is_none() {
            let result = read_snapshot(
                &d.inputs.file,
                &d.status.snapshot_id,
                d.status.source.clone(),
            );
            if !d.validate() {
                return Err(InspectionError::Invalidated);
            }
            match result {
                Ok(snapshot) => {
                    d.status.progress.total = Some(snapshot.geometry.page_count);
                    d.snapshot = Some(snapshot);
                }
                Err(error) => {
                    d.status.state = SessionState::Fatal;
                    d.status.coverage = Some(d.coverage(CoverageReason::FatalGeometry));
                    d.status.diagnostic = Some(Diagnostic {
                        code: "fatal_geometry",
                        message: error.to_string(),
                        affected_inputs: vec!["main"],
                    });
                    return Err(error);
                }
            }
        } else if Some(d.status.progress.completed) == d.status.progress.total {
            drop(d);
            self.finish(SessionState::Published, CoverageReason::Complete, || {})?;
        } else {
            if Arc::make_mut(&mut d.pages).try_reserve(1).is_err() {
                drop(d);
                return self.finish(
                    SessionState::Stopped,
                    CoverageReason::AllocationFailure,
                    || {},
                );
            }
            let number = d.status.progress.completed + 1;
            let geometry = &d
                .snapshot
                .as_ref()
                .ok_or(InspectionError::NotScanning)?
                .geometry;
            let detail = super::btree::read_page(&d.inputs.file, number, geometry);
            Arc::make_mut(&mut d.pages).push(PageEntity { number, detail });
            d.status.progress.completed = number;
            if !d.validate() {
                return Err(InspectionError::Invalidated);
            }
        }
        Ok(())
    }

    /// Observes every deterministic boundary outside the session lock, allowing cancellation
    /// and controlled synchronization without blocking concurrent status queries.
    /// # Errors
    /// Returns geometry or invalidation errors, or rejects a second scan on a terminal session.
    pub fn scan(
        &self,
        mut observe: impl FnMut(&SessionStatus) -> ScanControl,
    ) -> Result<(), InspectionError> {
        if self.status().state != SessionState::Scanning {
            return Err(if self.status().state == SessionState::Invalidated {
                InspectionError::Invalidated
            } else {
                InspectionError::NotScanning
            });
        }
        loop {
            let status = self.status();
            if status.state == SessionState::Invalidated {
                return Err(InspectionError::Invalidated);
            }
            if status.state != SessionState::Scanning {
                return Ok(());
            }
            match observe(&status) {
                ScanControl::Continue
                    if status.progress.total == Some(status.progress.completed) =>
                {
                    let result =
                        self.finish(SessionState::Published, CoverageReason::Complete, || {
                            let control = observe(&self.status());
                            if control != ScanControl::Continue {
                                let _ = self.stop(control);
                            }
                        });
                    if let Err(error) = result {
                        return if self.status().state == SessionState::Cancelled
                            || self.status().state == SessionState::Stopped
                        {
                            Ok(())
                        } else {
                            Err(error)
                        };
                    }
                    return Ok(());
                }
                ScanControl::Continue => self.advance()?,
                control => {
                    self.stop(control)?;
                    return Ok(());
                }
            }
        }
    }

    /// Publishes only the validated prefix, labelled with its precise stop reason.
    /// # Errors
    /// Rejects changed inputs and terminal sessions.
    pub fn stop(&self, control: ScanControl) -> Result<(), InspectionError> {
        let mut d = self.lock();
        if !d.validate() {
            return Err(InspectionError::Invalidated);
        }
        if d.status.state != SessionState::Scanning {
            return Err(InspectionError::NotScanning);
        }
        let (state, reason) = match control {
            ScanControl::Continue => return Ok(()),
            ScanControl::Cancel => (SessionState::Cancelled, CoverageReason::Cancelled),
            ScanControl::Stop => (SessionState::Stopped, CoverageReason::OperatorStop),
        };
        if self.active_checks.load(Ordering::Acquire) > 0 {
            self.cancel_verification.store(true, Ordering::Release);
            d.status.state = state;
            d.status.coverage = Some(d.coverage(reason));
            // Integrity was not established, so this stop has no navigable revision.
            return Ok(());
        }
        if d.topology_active {
            match control {
                ScanControl::Cancel => d.topology_cancellation.cancel(),
                ScanControl::Stop => d.topology_cancellation.stop(),
                ScanControl::Continue => unreachable!("continue returned above"),
            }
            let inventory_reason = if d.status.progress.total == Some(d.status.progress.completed) {
                CoverageReason::Complete
            } else {
                reason
            };
            d.pending_topology_stop = Some((state, inventory_reason));
            d.status.state = state;
            d.status.coverage = Some(d.coverage(inventory_reason));
            return Ok(());
        }
        drop(d);
        self.finish(state, reason, || {})
    }

    /// Resolves a published revision after validating input stability. This is also the
    /// prerequisite for later enrichment work; callers must revalidate before publishing.
    /// # Errors
    /// Rejects invalidated snapshots, unpublished graphs and unknown revisions.
    pub fn revision(&self, revision: u64) -> Result<Arc<InspectionGraph>, InspectionError> {
        {
            let mut d = self.lock();
            if !d.validate() {
                return Err(InspectionError::Invalidated);
            }
            if d.published.as_ref().is_none_or(|r| r.revision != revision) {
                return Err(InspectionError::RevisionUnavailable);
            }
        }
        self.verify(|| {})?;
        let mut d = self.lock();
        if !d.validate() {
            return Err(InspectionError::Invalidated);
        }
        d.published
            .as_ref()
            .filter(|r| r.revision == revision)
            .cloned()
            .ok_or(InspectionError::RevisionUnavailable)
    }

    /// Returns the initial published graph.
    /// # Errors
    /// Rejects unpublished or invalidated snapshots.
    pub fn graph(&self) -> Result<Arc<InspectionGraph>, InspectionError> {
        self.revision(1)
    }

    /// Retained observations are available only after invalidation, never as a revision.
    #[must_use]
    pub fn evidence(&self) -> Option<ObservedEvidence> {
        let mut d = self.lock();
        d.validate();
        if d.status.state != SessionState::Invalidated {
            return None;
        }
        let snapshot = d.snapshot.clone()?;
        let published = d.published.as_deref();
        let observed = d.observed_topology.as_ref();
        Some(ObservedEvidence {
            coverage: d.coverage(CoverageReason::InputChanged),
            snapshot,
            pages: published.map_or_else(|| (*d.pages).clone(), |revision| revision.pages.clone()),
            relationship_claims: published.map_or_else(
                || observed.map_or_else(Vec::new, |topology| topology.claims.clone()),
                |revision| revision.relationship_claims.clone(),
            ),
            relationships: published.map_or_else(
                || observed.map_or_else(Vec::new, |topology| topology.relationships.clone()),
                |revision| revision.relationships.clone(),
            ),
            traversals: published.map_or_else(
                || observed.map_or_else(Vec::new, |topology| topology.traversals.clone()),
                |revision| revision.traversals.clone(),
            ),
            diagnostics: published.map_or_else(
                || observed.map_or_else(Vec::new, |topology| topology.diagnostics.clone()),
                |revision| revision.diagnostics.clone(),
            ),
            topology_coverage: published.map_or_else(
                || {
                    observed.map_or(
                        super::TopologyCoverage {
                            reason: super::TopologyCoverageReason::Cancelled,
                            phase: super::TopologyPhase::BtreeClaimCollection,
                            evaluated: 0,
                            total: None,
                            next: None,
                            remainder: None,
                            next_phase: None,
                            traversal_budget: d.traversal_budget,
                        },
                        |topology| topology.coverage,
                    )
                },
                |revision| revision.topology_coverage,
            ),
        })
    }
}
