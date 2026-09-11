mod collections;
mod deep;
mod metadata;
mod page_collections;
mod schema;
mod traversals;
pub use deep::{DeepJob, DeepLimits};

use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;
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
    CellBudget,
    ResidentMemoryBudget,
    StorageBudget,
    StorageFailure,
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
// These activity flags overlap (verification and schema/sidecar work can run during topology publication).
#[allow(clippy::struct_excessive_bools)]
pub struct Progress {
    pub unit: &'static str,
    pub completed: u32,
    pub total: Option<u32>,
    pub verifying: bool,
    pub building_topology: bool,
    pub building_sidecars: bool,
    #[serde(rename = "buildingSchema")]
    pub building_schema: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatus {
    pub storage_budget: super::StorageBudget,
    pub storage: super::StorageStatus,
    pub operational_budget: super::OperationalBudget,
    pub work_progress: Option<super::WorkProgress>,
    pub work_coverage: Vec<super::WorkProgress>,
    pub traversal_budget: super::TraversalBudget,
    pub schema_budget: super::SchemaBudget,
    pub sidecar_budget: super::SidecarBudget,
    pub semantic_budget: Option<crate::semantic::SemanticBudget>,
    pub deep_limits: DeepLimits,
    pub session_id: String,
    pub available_revisions: Vec<u64>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opaque_range: Option<OpaqueRange>,
}

/// Readable input whose standard geometry cannot be established. No bytes are disclosed.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpaqueRange {
    pub file_offset: String,
    pub length: String,
}

/// Observations retained after invalidation have no revision identity.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedEvidence {
    pub sidecars: Vec<super::SidecarEvidence>,
    pub snapshot: DatabaseSnapshot,
    pub pages: Vec<PageEntity>,
    pub coverage: Coverage,
    pub relationship_claims: Vec<super::RelationshipClaim>,
    pub relationships: Vec<super::Relationship>,
    pub traversals: Vec<super::Traversal>,
    pub diagnostics: Vec<super::StructuralDiagnostic>,
    pub topology_coverage: super::TopologyCoverage,
    pub freelist: super::FreelistEvidence,
    pub pointer_map: super::PointerMapEvidence,
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
    digest: Result<Option<[u8; 32]>, io::ErrorKind>,
}

impl AcceptedInput {
    fn sidecar_evidence(&self) -> super::SidecarEvidence {
        let mut evidence = super::SidecarEvidence::discovered(
            self.kind,
            &self.path,
            self.length(),
            self.stamp.is_some(),
        );
        if self.digest.is_err() {
            evidence.unreadable();
        }
        evidence
    }
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
    let mut hash = blake3::Hasher::new();
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
    Ok(Some(*hash.finalize().as_bytes()))
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
                    digest: Ok(None),
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
            let result = digest(&input.path, input.length(), &AtomicBool::new(false));
            if input.kind == "main" {
                input.digest = Ok(result.map_err(InspectionError::Open)?);
            } else {
                input.digest = result.map_err(|error| error.kind());
            }
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
                if result.map_err(|error| error.kind()) != input.digest
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
    processed_cells: u64,
    inputs: Arc<Inputs>,
    status: SessionStatus,
    snapshot: Option<DatabaseSnapshot>,
    pages: super::storage::PageStore,
    published: Option<Arc<InspectionGraph>>,
    revisions: std::collections::BTreeMap<u64, Arc<InspectionGraph>>,
    traversal_budget: super::TraversalBudget,
    sidecar_budget: super::SidecarBudget,
    schema_budget: super::SchemaBudget,
    topology_active: bool,
    topology_cancellation: Arc<super::topology::WorkControl>,
    pending_topology_stop: Option<(SessionState, CoverageReason)>,
    observed_topology: Option<super::topology::Topology>,
    sidecars: Vec<super::SidecarEvidence>,
}

impl SessionData {
    fn pointer_map_evidence(
        &self,
        geometry: &super::DatabaseGeometry,
    ) -> Option<super::PointerMapEvidence> {
        if let Some(published) = &self.published {
            let mut evidence = published.pointer_map.clone();
            evidence.pages = self.pages.pointer_maps.materialize().ok()?;
            evidence.locations = evidence
                .pages
                .iter()
                .map(|page| page.page.clone())
                .collect();
            Some(evidence)
        } else if let Some(maps) = self
            .observed_topology
            .as_ref()
            .and_then(|topology| topology.pointer_map.as_ref())
        {
            maps.materialize().ok()
        } else {
            Some(super::PointerMapEvidence::header(geometry))
        }
    }

    fn freelist_evidence(&self) -> super::FreelistEvidence {
        let published = self.published.as_deref();
        let observed = self.observed_topology.as_ref();
        let mut freelist = published.map_or_else(
            || {
                observed.map_or_else(super::FreelistEvidence::uninspected, |topology| {
                    topology.freelist.clone()
                })
            },
            |revision| revision.freelist.clone(),
        );
        freelist.trunks = published.map_or_else(
            || {
                observed.map_or_else(Vec::new, |topology| {
                    topology.freelist_trunks.iter().collect()
                })
            },
            |_| self.pages.freelist_trunks.iter().collect(),
        );
        freelist
    }

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
        self.status.diagnostic = Some(Diagnostic { code: "input_changed", opaque_range: None,
            message: "Accepted input changed; retained observations are diagnostic evidence, not a coherent snapshot.".into(),
            affected_inputs: changes });
    }

    fn publish(
        &mut self,
        state: SessionState,
        reason: CoverageReason,
        mut topology: super::topology::Topology,
        schema: super::schema::StoredSchema,
        semantic_metadata: crate::semantic::SemanticMetadata,
    ) {
        let coverage = self.coverage(reason);
        if let Some(snapshot) = &self.snapshot {
            self.pages.publish(
                topology.classifications,
                std::mem::take(&mut topology.page_overrides),
            );
            self.pages.freelist_trunks = Arc::new(topology.freelist_trunks);
            self.pages.diagnostics = Arc::new(topology.diagnostics);
            self.pages.claims = Arc::new(topology.claims);
            self.pages.relationships = Arc::new(topology.relationships);
            self.pages.traversals = Arc::new(topology.traversals);
            self.published = Some(Arc::new(InspectionGraph {
                work_coverage: self.topology_cancellation.receipts(),
                operational_budget: self.status.operational_budget,
                semantic_metadata,
                deep_inspections: vec![],
                schema: {
                    let header = schema.header.clone();
                    self.pages.schema = Some(Arc::new(schema));
                    header
                },
                sidecars: self.sidecars.clone(),
                revision: 1,
                coverage: coverage.clone(),
                snapshot: snapshot.clone(),
                pages: Vec::new(),
                relationship_claims: Vec::new(),
                relationships: Vec::new(),
                traversals: Vec::new(),
                diagnostics: Vec::new(),
                topology_coverage: topology.coverage,
                freelist: topology.freelist,
                pointer_map: if let Some(maps) = topology.pointer_map {
                    self.pages.pointer_maps = Arc::new(maps.pages);
                    maps.header
                } else {
                    super::PointerMapEvidence::header(&snapshot.geometry)
                },
            }));
            self.revisions
                .insert(1, Arc::clone(self.published.as_ref().unwrap()));
            self.status.revision = Some(1);
        }
        self.status.coverage = Some(coverage);
        self.status.state = state;
    }
}

/// Owns accepted input handles and publishes only immutable, validated revisions.
pub struct InspectionSession {
    semantic_metadata: Option<crate::semantic::SemanticBudget>,
    deep_limits: DeepLimits,
    shutting_down: AtomicBool,
    deep_jobs: Mutex<std::collections::BTreeMap<String, Arc<DeepJob>>>,
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
        Self::begin_with_budgets(path, traversal_budget, super::SidecarBudget::default())
    }

    /// Accepts frozen inputs with explicit traversal and sidecar evidence ceilings.
    /// # Errors
    /// Returns input-access or invalidation errors during acceptance.
    pub fn begin_with_budgets(
        path: &Path,
        traversal_budget: super::TraversalBudget,
        sidecar_budget: super::SidecarBudget,
    ) -> Result<Self, InspectionError> {
        Self::begin_with_schema_budget(
            path,
            traversal_budget,
            sidecar_budget,
            super::SchemaBudget::default(),
        )
    }

    /// Accepts frozen inputs with explicit traversal, sidecar and schema decode ceilings.
    /// # Errors
    /// Returns input-access or invalidation errors during acceptance.
    pub fn begin_with_schema_budget(
        path: &Path,
        traversal_budget: super::TraversalBudget,
        sidecar_budget: super::SidecarBudget,
        schema_budget: super::SchemaBudget,
    ) -> Result<Self, InspectionError> {
        let inputs = Arc::new(Inputs::accept(path)?);
        let sidecars = inputs
            .accepted
            .iter()
            .skip(1)
            .map(AcceptedInput::sidecar_evidence)
            .collect();
        Ok(Self {
            semantic_metadata: None,
            deep_limits: DeepLimits::default(),
            shutting_down: AtomicBool::new(false),
            deep_jobs: Mutex::new(std::collections::BTreeMap::new()),
            active_checks: AtomicUsize::new(0),
            cancel_verification: AtomicBool::new(false),
            data: Mutex::new(SessionData {
                processed_cells: 0,
                inputs,
                status: SessionStatus {
                    storage_budget: super::StorageBudget::default(),
                    storage: super::StorageStatus::default(),
                    operational_budget: super::OperationalBudget::default(),
                    work_progress: None,
                    work_coverage: vec![],
                    traversal_budget,
                    schema_budget,
                    sidecar_budget,
                    semantic_budget: None,
                    deep_limits: DeepLimits::default(),
                    session_id: Uuid::new_v4().to_string(),
                    available_revisions: vec![],
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
                        building_sidecars: false,
                        building_schema: false,
                    },
                    revision: None,
                    coverage: None,
                    diagnostic: None,
                },
                snapshot: None,
                pages: super::storage::PageStore::new(super::StorageBudget::default()),
                published: None,
                revisions: std::collections::BTreeMap::new(),
                traversal_budget,
                sidecar_budget,
                schema_budget,
                topology_active: false,
                topology_cancellation: Arc::new(super::topology::WorkControl::new()),
                pending_topology_stop: None,
                observed_topology: None,
                sidecars,
            }),
        })
    }

    /// Sets private structural-storage ceilings before scanning starts.
    /// # Panics
    /// Panics if geometry or any page has already been inspected.
    #[must_use]
    pub fn with_storage_budget(self, budget: super::StorageBudget) -> Self {
        {
            let mut data = self.lock();
            assert!(
                data.snapshot.is_none(),
                "storage must be configured before scanning"
            );
            data.status.storage_budget = budget;
            data.pages = super::storage::PageStore::new(budget);
        }
        self
    }

    /// Observes structural work at deterministic boundaries outside the session lock.
    /// Return Cancel or Stop to stop this fast inspection at the observed boundary.
    #[must_use]
    pub fn with_work_observer(
        self,
        observe: impl FnMut(&super::WorkProgress) -> ScanControl + Send + 'static,
    ) -> Self {
        self.lock()
            .topology_cancellation
            .set_observer(Box::new(observe));
        self
    }

    /// Sets resource ceilings before any inspection work is started.
    #[must_use]
    pub fn with_operational_budget(mut self, budget: super::OperationalBudget) -> Self {
        self.data
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .status
            .operational_budget = budget;
        self
    }

    /// Opts into descriptive metadata from a bounded private subprocess of this executable.
    /// Call before scanning; unavailable enrichment never changes physical evidence.
    #[must_use]
    pub fn with_semantic_metadata(self) -> Self {
        self.with_semantic_metadata_budget(crate::semantic::SemanticBudget::default())
    }

    /// Enables semantic descriptions with operational budgets capped by fixed security ceilings.
    #[must_use]
    pub fn with_semantic_metadata_budget(
        mut self,
        budget: crate::semantic::SemanticBudget,
    ) -> Self {
        self.semantic_metadata = Some(budget.bounded());
        self.lock().status.semantic_budget = self.semantic_metadata;
        self
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
        mut checkpoint: impl FnMut(),
    ) -> Result<(), InspectionError> {
        self.verify(&mut checkpoint)?;
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
            d.topology_cancellation
                .set_phase_limit(d.status.operational_budget.max_phase_units);
            d.topology_cancellation
                .set_memory_limit(d.status.operational_budget.max_resident_bytes);
            d.topology_cancellation
                .set_freelist_limit(d.status.operational_budget.max_freelist_trunks);
            (
                Arc::clone(&d.inputs),
                d.snapshot.clone(),
                Arc::new(d.pages.clone()),
                d.traversal_budget,
                Arc::clone(&d.topology_cancellation),
            )
        };
        let mut topology = snapshot.as_ref().map_or_else(
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
        self.contain_storage_stop(&pages, &mut topology, true)?;
        let schema = self.inspect_schema(
            &inputs,
            snapshot.as_ref(),
            &pages,
            &topology,
            &cancellation,
            &mut checkpoint,
        );
        self.inspect_sidecars(
            &inputs,
            snapshot.as_ref().map_or(0, |s| s.geometry.page_size),
            reason,
            &cancellation,
            &mut checkpoint,
        );
        let semantic_metadata = self.inspect_semantic(&inputs, &schema, reason, &cancellation);
        self.contain_storage_stop(&pages, &mut topology, false)?;
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
                let state = cancellation.session_state(state);
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
        d.publish(
            publish_state,
            publish_reason,
            topology,
            schema,
            semantic_metadata,
        );
        Ok(())
    }

    fn contain_storage_stop(
        &self,
        pages: &super::storage::PageStore,
        topology: &mut super::topology::Topology,
        topology_incomplete: bool,
    ) -> Result<(), InspectionError> {
        match pages.check() {
            Ok(()) => {}
            Err(super::storage::StorageError::Budget) => {
                if topology_incomplete {
                    topology.classifications = super::index::Sequence::default();
                    topology.page_overrides = super::index_map::IndexMap::default();
                    topology.relationships.clear();
                    topology.traversals.clear();
                }
                pages.context.clear_budget_stop();
            }
            Err(super::storage::StorageError::Unavailable) => {
                let mut data = self.lock();
                data.topology_active = false;
                data.status.state = SessionState::Fatal;
                data.status.coverage = Some(data.coverage(CoverageReason::StorageFailure));
                data.status.diagnostic = Some(Diagnostic {
                    opaque_range: None,
                    code: "private_storage_unavailable",
                    message: "Private structural evidence could not be read.".into(),
                    affected_inputs: Vec::new(),
                });
                return Err(InspectionError::StorageUnavailable);
            }
        }
        Ok(())
    }

    fn inspect_semantic(
        &self,
        inputs: &Inputs,
        schema: &super::schema::StoredSchema,
        reason: CoverageReason,
        cancellation: &super::topology::WorkControl,
    ) -> crate::semantic::SemanticMetadata {
        match self.semantic_metadata {
            Some(budget) if reason == CoverageReason::Complete => {
                cancellation.begin_phase(super::TopologyPhase::SemanticHelper, None);
                let projection =
                    schema.metadata(budget.bounded().max_schema_records.saturating_add(1));
                let mut metadata =
                    crate::semantic::enrich(&inputs.file, &projection, budget, |coverage| {
                        if coverage.phase == crate::semantic::SemanticPhase::Copy {
                            cancellation.set_extent(
                                coverage.evaluated_bytes.div_ceil(16384),
                                coverage.total_bytes.map(|n| n.div_ceil(16384)),
                            );
                        } else {
                            cancellation.set_extent(cancellation.progress().evaluated, None);
                        }
                        cancellation.traversal_reason().is_some()
                    });
                if let Some(reason) = cancellation.traversal_reason() {
                    metadata.coverage.reason = match reason {
                        super::TraversalStopReason::Budget => {
                            crate::semantic::SemanticReason::WorkBudget
                        }
                        super::TraversalStopReason::OperatorStop => {
                            crate::semantic::SemanticReason::OperatorStop
                        }
                        _ => crate::semantic::SemanticReason::Cancelled,
                    };
                } else {
                    use crate::semantic::SemanticReason;
                    let limit = match metadata.coverage.reason {
                        SemanticReason::CopyByteBudget => Some(super::BudgetKind::HelperCopyBytes),
                        SemanticReason::SchemaRecordBudget => {
                            Some(super::BudgetKind::HelperRecords)
                        }
                        SemanticReason::TimeBudget => Some(super::BudgetKind::HelperTime),
                        SemanticReason::OutputByteBudget => {
                            Some(super::BudgetKind::HelperOutputBytes)
                        }
                        _ => None,
                    };
                    if let Some(limit) = limit {
                        cancellation.mark_local_budget(limit);
                    } else if metadata.coverage.reason == SemanticReason::Complete {
                        cancellation.finish_phase(super::TopologyPhase::SemanticHelper);
                    } else {
                        cancellation.mark_unavailable();
                    }
                }
                metadata
            }
            _ => crate::semantic::SemanticMetadata::unavailable(),
        }
    }

    fn inspect_schema(
        &self,
        inputs: &Inputs,
        snapshot: Option<&DatabaseSnapshot>,
        pages: &super::storage::PageStore,
        topology: &super::topology::Topology,
        cancellation: &super::topology::WorkControl,
        checkpoint: &mut impl FnMut(),
    ) -> super::schema::StoredSchema {
        let schema_budget = {
            let mut data = self.lock();
            data.status.progress.building_schema = true;
            data.schema_budget
        };
        let schema = snapshot.map_or_else(
            || super::schema::StoredSchema::unavailable(schema_budget, pages),
            |snapshot| {
                super::schema::inspect(
                    &inputs.file,
                    &snapshot.geometry,
                    pages,
                    topology,
                    cancellation,
                    schema_budget,
                    checkpoint,
                )
            },
        );
        self.lock().status.progress.building_schema = false;
        schema
    }

    fn inspect_sidecars(
        &self,
        inputs: &Inputs,
        page_size: u32,
        reason: CoverageReason,
        cancellation: &super::topology::WorkControl,
        checkpoint: &mut impl FnMut(),
    ) {
        let sidecar_budget = {
            let mut d = self.lock();
            d.status.progress.building_sidecars = true;
            d.sidecar_budget
        };
        let sidecars = inputs
            .accepted
            .iter()
            .skip(1)
            .map(|input| {
                let evidence = input.sidecar_evidence();
                if reason != CoverageReason::Complete || cancellation.traversal_reason().is_some() {
                    evidence
                } else {
                    super::sidecar::inspect(
                        &input.path,
                        evidence,
                        page_size,
                        cancellation,
                        checkpoint,
                        sidecar_budget,
                    )
                }
            })
            .collect();
        {
            let mut d = self.lock();
            d.sidecars = sidecars;
            d.status.progress.building_sidecars = false;
        }
    }

    /// Observes progress independently of a navigable graph and rechecks accepted inputs.
    #[must_use]
    pub fn status(&self) -> SessionStatus {
        let mut data = self.lock();
        data.validate();
        let mut status = data.status.clone();
        status.storage = data.pages.status();
        status.available_revisions = data.revisions.keys().copied().collect();
        status.progress.verifying = self.active_checks.load(Ordering::Acquire) > 0;
        status.progress.building_topology = data.topology_active;
        status.work_coverage = data.topology_cancellation.receipts();
        status.work_progress = data
            .topology_active
            .then(|| data.topology_cancellation.progress());
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
                        code: if matches!(error, InspectionError::InvalidMagic) {
                            "unsupported_format"
                        } else {
                            "fatal_geometry"
                        },
                        message: if matches!(error, InspectionError::InvalidMagic) {
                            "Readable input preserved as opaque evidence; standard SQLite geometry is unavailable".into()
                        } else {
                            error.to_string()
                        },
                        affected_inputs: vec!["main"],
                        opaque_range: Some(OpaqueRange {
                            file_offset: "0".into(),
                            length: d.inputs.accepted[0].length().to_string(),
                        }),
                    });
                    return Err(error);
                }
            }
        } else if Some(d.status.progress.completed) == d.status.progress.total {
            drop(d);
            self.finish(SessionState::Published, CoverageReason::Complete, || {})?;
        } else {
            return self.advance_page(d);
        }
        Ok(())
    }

    fn advance_page(&self, mut d: MutexGuard<'_, SessionData>) -> Result<(), InspectionError> {
        // Reserve headroom for the largest bounded page interpretation before reading cells.
        let page_size = d
            .snapshot
            .as_ref()
            .ok_or(InspectionError::NotScanning)?
            .geometry
            .page_size;
        if !super::budget::memory_available(
            d.status.operational_budget.max_resident_bytes,
            u64::from(page_size) * 2,
        ) {
            drop(d);
            return self.finish(
                SessionState::Stopped,
                CoverageReason::ResidentMemoryBudget,
                || {},
            );
        }
        let number = d.status.progress.completed + 1;
        let geometry = &d
            .snapshot
            .as_ref()
            .ok_or(InspectionError::NotScanning)?
            .geometry;
        let remaining = d
            .status
            .operational_budget
            .max_processed_cells
            .saturating_sub(d.processed_cells);
        let detail = super::roles::reserved_detail(geometry, number).map_or_else(
            || {
                super::btree::read_page_with_cell_budget(
                    &d.inputs.file,
                    number,
                    geometry,
                    remaining,
                    d.status.operational_budget.max_resident_bytes,
                )
            },
            Ok,
        );
        let detail = match detail {
            Ok(detail) => detail,
            Err(reason) => {
                drop(d);
                return self.finish(SessionState::Stopped, reason, || {});
            }
        };
        let classification = super::roles::initial(geometry, number, &detail);
        let cells = detail.cells.len() as u64;
        if let Err(error) = d.pages.push(PageEntity {
            number,
            classification,
            detail,
        }) {
            let reason = match error {
                super::storage::StorageError::Budget => CoverageReason::StorageBudget,
                super::storage::StorageError::Unavailable => CoverageReason::StorageFailure,
            };
            drop(d);
            return self.finish(SessionState::Stopped, reason, || {});
        }
        d.processed_cells += cells;
        d.status.progress.completed = number;
        if !d.validate() {
            return Err(InspectionError::Invalidated);
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
            if !d.revisions.contains_key(&revision) {
                return Err(InspectionError::RevisionUnavailable);
            }
        }
        self.verify(|| {})?;
        let mut d = self.lock();
        if !d.validate() {
            return Err(InspectionError::Invalidated);
        }
        let stored = d
            .revisions
            .get(&revision)
            .ok_or(InspectionError::RevisionUnavailable)?;
        let reservation = d
            .pages
            .export_reservation()
            .map_err(|_| InspectionError::StorageUnavailable)?
            .saturating_add(deep::revision_reservation(stored));
        if reservation > 0
            && !super::budget::memory_available(
                d.status.operational_budget.max_resident_bytes,
                reservation,
            )
        {
            return Err(InspectionError::CollectionBudget);
        }
        let mut graph = (**stored).clone();
        graph.pages = d
            .pages
            .materialize()
            .map_err(|_| InspectionError::StorageUnavailable)?;
        if let Some(schema) = &d.pages.schema {
            graph.schema = schema
                .materialize()
                .map_err(|_| InspectionError::StorageUnavailable)?;
        }
        graph.pointer_map.pages = d
            .pages
            .pointer_maps
            .materialize()
            .map_err(|_| InspectionError::StorageUnavailable)?;
        graph.pointer_map.locations = graph
            .pointer_map
            .pages
            .iter()
            .map(|page| page.page.clone())
            .collect();
        graph.freelist.trunks = d.pages.freelist_trunks.iter().collect();
        graph.diagnostics = d.pages.diagnostics.iter().collect();
        graph.relationship_claims = d.pages.claims.iter().collect();
        graph.relationships = d.pages.relationships.iter().collect();
        graph.traversals = d.pages.traversals.iter().collect();
        d.pages
            .check()
            .map_err(|_| InspectionError::StorageUnavailable)?;
        Ok(Arc::new(graph))
    }

    /// Reads fixed-size metadata without loading any page or relationship collection.
    /// # Errors
    /// Rejects unknown revisions and changed inputs.
    pub fn revision_summary(
        &self,
        revision: u64,
    ) -> Result<super::RevisionSummary, InspectionError> {
        self.verify(|| {})?;
        let mut data = self.lock();
        if !data.validate() {
            return Err(InspectionError::Invalidated);
        }
        let graph = data
            .revisions
            .get(&revision)
            .ok_or(InspectionError::RevisionUnavailable)?;
        Ok(metadata::summary(graph, &data.pages))
    }

    /// Reads at most 256 consecutive pages from one immutable revision.
    /// # Errors
    /// Rejects invalid ranges, unavailable revisions, changed inputs or storage failures.
    pub fn page_batch(
        &self,
        revision: u64,
        first_page: u32,
        limit: u32,
    ) -> Result<super::PageBatch, InspectionError> {
        if first_page == 0 || limit == 0 || limit > 256 {
            return Err(InspectionError::InvalidPageRange);
        }
        self.verify(|| {})?;
        let mut data = self.lock();
        if !data.validate() {
            return Err(InspectionError::Invalidated);
        }
        if !data.revisions.contains_key(&revision) {
            return Err(InspectionError::RevisionUnavailable);
        }
        let total =
            u32::try_from(data.pages.len()).map_err(|_| InspectionError::StorageUnavailable)?;
        let end = first_page
            .saturating_add(limit)
            .min(total.saturating_add(1));
        let mut pages = Vec::new();
        let mut retained = 0_u64;
        let mut next = first_page;
        for number in first_page..end {
            let reservation = data
                .pages
                .estimated_size((number - 1) as usize)
                .map_err(|_| InspectionError::StorageUnavailable)?;
            // A single dense 64 KiB page can exceed the normal decoded window.
            // Estimates include 4x serialized bytes; admit one larger page up to
            // the 8 MiB wire envelope while still enforcing resident headroom.
            let window_budget = if pages.is_empty() {
                32 * 1024 * 1024
            } else {
                8 * 1024 * 1024
            };
            if retained.saturating_add(reservation) > window_budget
                || !super::budget::memory_available(
                    data.status.operational_budget.max_resident_bytes,
                    reservation,
                )
            {
                if pages.is_empty() {
                    return Err(InspectionError::CollectionBudget);
                }
                break;
            }
            let page = data
                .pages
                .get((number - 1) as usize)
                .ok_or(InspectionError::StorageUnavailable)?;
            pages.push(page);
            retained += reservation;
            next = number + 1;
        }
        data.pages
            .check()
            .map_err(|_| InspectionError::StorageUnavailable)?;
        Ok(super::PageBatch {
            revision,
            first_page,
            total,
            next_page: (next <= total && next > first_page).then_some(next),
            pages,
        })
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
        let mut reservation = d.pages.export_reservation().ok()?;
        if published.is_none()
            && let Some(topology) = observed
        {
            reservation = reservation.saturating_add(topology.export_reservation().ok()?);
        }
        if reservation > 0
            && !super::budget::memory_available(
                d.status.operational_budget.max_resident_bytes,
                reservation,
            )
        {
            return None;
        }
        let inventory = d.pages.materialize().ok()?;
        Some(ObservedEvidence {
            sidecars: d.sidecars.clone(),
            pointer_map: d.pointer_map_evidence(&snapshot.geometry)?,
            freelist: d.freelist_evidence(),
            coverage: d.coverage(CoverageReason::InputChanged),
            snapshot,
            pages: {
                let mut pages = inventory;
                if published.is_none()
                    && let Some(topology) = observed
                {
                    for number in topology.page_overrides.keys() {
                        if let Some(page) = topology.page_overrides.get(&number) {
                            pages[(number - 1) as usize] = page;
                        }
                    }
                    for (page, classification) in
                        pages.iter_mut().zip(topology.classifications.iter())
                    {
                        page.classification = classification.clone();
                    }
                }
                pages
            },
            relationship_claims: published.map_or_else(
                || observed.map_or_else(Vec::new, |topology| topology.claims.iter().collect()),
                |_| d.pages.claims.iter().collect(),
            ),
            relationships: published.map_or_else(
                || {
                    observed
                        .map_or_else(Vec::new, |topology| topology.relationships.iter().collect())
                },
                |_| d.pages.relationships.iter().collect(),
            ),
            traversals: published.map_or_else(
                || observed.map_or_else(Vec::new, |topology| topology.traversals.iter().collect()),
                |_| d.pages.traversals.iter().collect(),
            ),
            diagnostics: published.map_or_else(
                || observed.map_or_else(Vec::new, |topology| topology.diagnostics.iter().collect()),
                |_| d.pages.diagnostics.iter().collect(),
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
