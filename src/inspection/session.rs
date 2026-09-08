use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
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
pub struct ObservedEvidence {
    pub snapshot: DatabaseSnapshot,
    pub pages: Vec<PageEntity>,
    pub coverage: Coverage,
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
    paths: [(&'static str, PathBuf); 4],
    accepted: Vec<Option<InputStamp>>,
    file: File,
    descriptor: Stamp,
    digests: Vec<Option<[u8; 32]>>,
}

fn digest(path: &Path) -> io::Result<Option<[u8; 32]>> {
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
    let mut file = match File::open(path) {
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
    let mut buffer = [0_u8; 16_384];
    loop {
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
        let accepted = paths
            .iter()
            .map(|(_, p)| stamp(p))
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
        let digests = paths
            .iter()
            .map(|(_, p)| digest(p))
            .collect::<io::Result<Vec<_>>>()
            .map_err(InspectionError::Open)?;
        let inputs = Self {
            paths,
            accepted,
            file,
            descriptor,
            digests,
        };
        if !inputs.changes(false).is_empty() {
            return Err(InspectionError::Invalidated);
        }
        Ok(inputs)
    }

    fn changes(&self, verify_content: bool) -> Vec<&'static str> {
        let mut changes: Vec<_> = self
            .paths
            .iter()
            .zip(&self.accepted)
            .filter_map(|((name, path), accepted)| match stamp(path) {
                Ok(current) if &current == accepted => None,
                _ => Some(*name),
            })
            .collect();
        let descriptor_matches = self
            .file
            .metadata()
            .is_ok_and(|m| Stamp::from(m) == self.descriptor);
        let accepted_matches = self.accepted[0]
            .as_ref()
            .is_some_and(|s| s.target.as_ref().unwrap_or(&s.link) == &self.descriptor);
        if (!descriptor_matches || !accepted_matches) && !changes.contains(&"main") {
            changes.push("main");
        }
        if verify_content {
            for ((name, path), accepted) in self.paths.iter().zip(&self.digests) {
                if !digest(path).is_ok_and(|current| &current == accepted)
                    && !changes.contains(name)
                {
                    changes.push(name);
                }
            }
            // Catch replacement or writes that occur while the streaming digest is read.
            for name in self.changes(false) {
                if !changes.contains(&name) {
                    changes.push(name);
                }
            }
        }
        changes
    }
}

struct SessionData {
    inputs: Inputs,
    status: SessionStatus,
    snapshot: Option<DatabaseSnapshot>,
    pages: Vec<PageEntity>,
    published: Option<Arc<InspectionGraph>>,
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

    fn validate(&mut self, verify_content: bool) -> bool {
        if self.status.state == SessionState::Invalidated {
            return false;
        }
        let changes = self.inputs.changes(verify_content);
        if changes.is_empty() {
            return true;
        }
        self.status.state = SessionState::Invalidated;
        self.status.revision = None;
        self.status.coverage = Some(self.coverage(CoverageReason::InputChanged));
        self.status.diagnostic = Some(Diagnostic { code: "input_changed",
            message: "Accepted input changed; retained observations are diagnostic evidence, not a coherent snapshot.".into(),
            affected_inputs: changes });
        false
    }

    fn publish(&mut self, state: SessionState, reason: CoverageReason) {
        let coverage = self.coverage(reason);
        if let Some(snapshot) = &self.snapshot {
            self.published = Some(Arc::new(InspectionGraph {
                revision: 1,
                coverage: coverage.clone(),
                snapshot: snapshot.clone(),
                pages: std::mem::take(&mut self.pages),
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
        Ok(Self {
            data: Mutex::new(SessionData {
                inputs: Inputs::accept(path)?,
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
                    },
                    revision: None,
                    coverage: None,
                    diagnostic: None,
                },
                snapshot: None,
                pages: Vec::new(),
                published: None,
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

    fn lock(&self) -> MutexGuard<'_, SessionData> {
        self.data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Observes progress independently of a navigable graph and rechecks accepted inputs.
    #[must_use]
    pub fn status(&self) -> SessionStatus {
        let mut data = self.lock();
        let terminal = data.status.state != SessionState::Scanning;
        data.validate(terminal);
        data.status.clone()
    }

    /// Runs one bounded inventory unit. Publication itself is a separate, validated boundary.
    /// # Errors
    /// Returns an error for invalid geometry, changed inputs, or work requested after a terminal state.
    pub fn advance(&self) -> Result<(), InspectionError> {
        let mut d = self.lock();
        if !d.validate(false) {
            return Err(InspectionError::Invalidated);
        }
        if d.status.state != SessionState::Scanning {
            return Err(InspectionError::NotScanning);
        }
        if d.snapshot.is_none() {
            let result = read_snapshot(
                &d.inputs.file,
                &d.status.snapshot_id,
                d.status.source.clone(),
            );
            if !d.validate(false) {
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
            if !d.validate(true) {
                return Err(InspectionError::Invalidated);
            }
            d.publish(SessionState::Published, CoverageReason::Complete);
        } else {
            if d.pages.try_reserve(1).is_err() {
                if !d.validate(true) {
                    return Err(InspectionError::Invalidated);
                }
                d.publish(SessionState::Stopped, CoverageReason::AllocationFailure);
                return Ok(());
            }
            let number = d.status.progress.completed + 1;
            d.pages.push(PageEntity { number });
            d.status.progress.completed = number;
            if !d.validate(false) {
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
        if !d.validate(true) {
            return Err(InspectionError::Invalidated);
        }
        if d.status.state != SessionState::Scanning {
            return Err(InspectionError::NotScanning);
        }
        match control {
            ScanControl::Continue => {}
            ScanControl::Cancel => d.publish(SessionState::Cancelled, CoverageReason::Cancelled),
            ScanControl::Stop => d.publish(SessionState::Stopped, CoverageReason::OperatorStop),
        }
        Ok(())
    }

    /// Resolves a published revision after validating input stability. This is also the
    /// prerequisite for later enrichment work; callers must revalidate before publishing.
    /// # Errors
    /// Rejects invalidated snapshots, unpublished graphs and unknown revisions.
    pub fn revision(&self, revision: u64) -> Result<Arc<InspectionGraph>, InspectionError> {
        let mut d = self.lock();
        let published = d.published.is_some();
        if !d.validate(published) {
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
        d.validate(true);
        if d.status.state != SessionState::Invalidated {
            return None;
        }
        let snapshot = d.snapshot.clone()?;
        Some(ObservedEvidence {
            coverage: d.coverage(CoverageReason::InputChanged),
            snapshot,
            pages: d
                .published
                .as_ref()
                .map_or_else(|| d.pages.clone(), |r| r.pages.clone()),
        })
    }
}
