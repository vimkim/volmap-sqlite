use std::fs::File;
use std::os::unix::fs::FileExt;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use serde::{Serialize, Serializer};

use super::{ByteRange, DatabaseGeometry, PageEntity};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageIdentity {
    pub page_number: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EntityIdentity {
    #[serde(rename_all = "camelCase")]
    Page { page_number: u32 },
    #[serde(rename_all = "camelCase")]
    Cell { page_number: u32, cell_index: u16 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipKind {
    BtreeChild,
    FreelistTrunk,
    FreelistLeaf,
    Overflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipState {
    Validated,
    Unresolved,
    Invalid,
    Conflicting,
    Terminal,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhysicalEvidence {
    pub page: PageIdentity,
    pub range: ByteRange,
    pub validation_rule: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationshipClaim {
    pub id: String,
    pub kind: RelationshipKind,
    pub source: EntityIdentity,
    pub target: Option<PageIdentity>,
    pub evidence: PhysicalEvidence,
    pub state: RelationshipState,
    #[serde(skip)]
    pub(super) stop_reason: Option<TraversalStopReason>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Relationship {
    pub claim_id: String,
    pub kind: RelationshipKind,
    pub source: EntityIdentity,
    pub target: PageIdentity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraversalKind {
    Btree,
    Freelist,
    Overflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraversalStopReason {
    MissingTarget,
    OutOfRange,
    CoverageStop,
    InvalidReference,
    TypeMismatch,
    Cycle,
    ConflictingClaim,
    OverlappingExtent,
    Budget,
    Cancelled,
    OperatorStop,
}

/// Explicit ceilings for relationship traversal. Validation of the physical
/// claims remains independent from how far a traversal is allowed to follow.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraversalBudget {
    #[serde(rename = "maxBtreePages")]
    btree_depth: u32,
    #[serde(rename = "maxOverflowPages")]
    overflow_depth: u32,
    #[serde(rename = "maxTotalPages", serialize_with = "serialize_u64_decimal")]
    allocation_ceiling: u64,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // `serialize_with` requires `&T`.
fn serialize_u64_decimal<S>(value: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&value.to_string())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TopologyCoverageReason {
    Complete,
    Budget,
    Cancelled,
    OperatorStop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum TopologyPhase {
    BtreeClaimCollection = 1,
    BtreeClaimValidation,
    BtreeParentReconciliation,
    BtreeCycleReconciliation,
    BtreeRelationshipNormalization,
    BtreeTraversal,
    OverflowInspection,
    OverflowReconciliation,
    OverflowRelationshipNormalization,
    FreelistInspection,
    AllocationReconciliation,
    Complete,
}

pub(super) struct WorkControl {
    stop: AtomicU8,
    phase: AtomicU8,
    evaluated: AtomicU64,
    total: AtomicU64,
    total_known: AtomicU8,
    phase_complete: AtomicU8,
    budget_exhausted: AtomicU8,
    aggregate_budget_exhausted: AtomicU8,
}

impl WorkControl {
    pub(super) fn new() -> Self {
        Self {
            stop: AtomicU8::new(0),
            phase: AtomicU8::new(TopologyPhase::BtreeClaimCollection as u8),
            evaluated: AtomicU64::new(0),
            total: AtomicU64::new(0),
            total_known: AtomicU8::new(0),
            phase_complete: AtomicU8::new(0),
            budget_exhausted: AtomicU8::new(0),
            aggregate_budget_exhausted: AtomicU8::new(0),
        }
    }

    pub(super) fn reset(&self) {
        self.stop.store(0, Ordering::Release);
        self.phase
            .store(TopologyPhase::BtreeClaimCollection as u8, Ordering::Release);
        self.evaluated.store(0, Ordering::Release);
        self.total.store(0, Ordering::Release);
        self.total_known.store(0, Ordering::Release);
        self.phase_complete.store(0, Ordering::Release);
        self.budget_exhausted.store(0, Ordering::Release);
        self.aggregate_budget_exhausted.store(0, Ordering::Release);
    }

    pub(super) fn cancel(&self) {
        self.stop.store(1, Ordering::Release);
    }

    pub(super) fn stop(&self) {
        self.stop.store(2, Ordering::Release);
    }

    fn stop_reason(&self) -> Option<TopologyCoverageReason> {
        match self.stop.load(Ordering::Acquire) {
            1 => Some(TopologyCoverageReason::Cancelled),
            2 => Some(TopologyCoverageReason::OperatorStop),
            _ => None,
        }
    }

    pub(super) fn traversal_reason(&self) -> Option<TraversalStopReason> {
        match self.stop_reason() {
            Some(TopologyCoverageReason::Cancelled) => Some(TraversalStopReason::Cancelled),
            Some(TopologyCoverageReason::OperatorStop) => Some(TraversalStopReason::OperatorStop),
            _ => None,
        }
    }

    fn stopped(&self) -> bool {
        self.stop_reason().is_some()
    }

    fn mark_budget_exhausted(&self) {
        self.budget_exhausted.store(1, Ordering::Release);
    }

    pub(super) fn mark_aggregate_budget_exhausted(&self) {
        self.mark_budget_exhausted();
        self.aggregate_budget_exhausted.store(1, Ordering::Release);
    }

    fn aggregate_budget_exhausted(&self) -> bool {
        self.aggregate_budget_exhausted.load(Ordering::Acquire) != 0
    }

    pub(super) fn begin_phase(&self, phase: TopologyPhase, total: Option<u64>) {
        if self.stopped() {
            return;
        }
        self.phase.store(phase as u8, Ordering::Release);
        self.evaluated.store(0, Ordering::Release);
        self.total.store(total.unwrap_or(0), Ordering::Release);
        self.total_known
            .store(u8::from(total.is_some()), Ordering::Release);
        self.phase_complete.store(0, Ordering::Release);
    }

    pub(super) fn advance(&self, phase: TopologyPhase) {
        if self.phase.load(Ordering::Acquire) == phase as u8 {
            self.evaluated.fetch_add(1, Ordering::AcqRel);
        }
    }

    pub(super) fn finish_phase(&self, phase: TopologyPhase) {
        if self.phase.load(Ordering::Acquire) == phase as u8 {
            self.phase_complete.store(1, Ordering::Release);
        }
    }

    fn complete(&self) {
        if !self.stopped() && !self.aggregate_budget_exhausted() {
            self.begin_phase(TopologyPhase::Complete, Some(0));
            self.finish_phase(TopologyPhase::Complete);
        }
    }

    fn coverage(&self, budget: TraversalBudget) -> TopologyCoverage {
        let evaluated = self.evaluated.load(Ordering::Acquire);
        let phase = decode_phase(self.phase.load(Ordering::Acquire));
        let total = (self.total_known.load(Ordering::Acquire) != 0)
            .then(|| self.total.load(Ordering::Acquire));
        let phase_complete = self.phase_complete.load(Ordering::Acquire) != 0
            || total.is_some_and(|total| evaluated >= total);
        TopologyCoverage {
            reason: self.stop_reason().unwrap_or_else(|| {
                if self.budget_exhausted.load(Ordering::Acquire) != 0 {
                    TopologyCoverageReason::Budget
                } else {
                    TopologyCoverageReason::Complete
                }
            }),
            phase,
            evaluated,
            total,
            next: (!phase_complete && phase != TopologyPhase::Complete)
                .then_some(evaluated + 1)
                .filter(|_| total.is_none_or(|total| evaluated < total)),
            remainder: total.map(|total| total.saturating_sub(evaluated)),
            next_phase: phase_complete.then(|| next_phase(phase)).flatten(),
            traversal_budget: budget,
        }
    }
}

fn decode_phase(value: u8) -> TopologyPhase {
    match value {
        1 => TopologyPhase::BtreeClaimCollection,
        2 => TopologyPhase::BtreeClaimValidation,
        3 => TopologyPhase::BtreeParentReconciliation,
        4 => TopologyPhase::BtreeCycleReconciliation,
        5 => TopologyPhase::BtreeRelationshipNormalization,
        6 => TopologyPhase::BtreeTraversal,
        7 => TopologyPhase::OverflowInspection,
        8 => TopologyPhase::OverflowReconciliation,
        9 => TopologyPhase::OverflowRelationshipNormalization,
        10 => TopologyPhase::FreelistInspection,
        11 => TopologyPhase::AllocationReconciliation,
        _ => TopologyPhase::Complete,
    }
}

fn next_phase(phase: TopologyPhase) -> Option<TopologyPhase> {
    match phase {
        TopologyPhase::BtreeClaimCollection => Some(TopologyPhase::BtreeClaimValidation),
        TopologyPhase::BtreeClaimValidation => Some(TopologyPhase::BtreeParentReconciliation),
        TopologyPhase::BtreeParentReconciliation => Some(TopologyPhase::BtreeCycleReconciliation),
        TopologyPhase::BtreeCycleReconciliation => {
            Some(TopologyPhase::BtreeRelationshipNormalization)
        }
        TopologyPhase::BtreeRelationshipNormalization => Some(TopologyPhase::BtreeTraversal),
        TopologyPhase::BtreeTraversal => Some(TopologyPhase::OverflowInspection),
        TopologyPhase::OverflowInspection => Some(TopologyPhase::OverflowReconciliation),
        TopologyPhase::OverflowReconciliation => {
            Some(TopologyPhase::OverflowRelationshipNormalization)
        }
        TopologyPhase::OverflowRelationshipNormalization => {
            Some(TopologyPhase::AllocationReconciliation)
        }
        TopologyPhase::AllocationReconciliation => Some(TopologyPhase::Complete),
        TopologyPhase::FreelistInspection => Some(TopologyPhase::BtreeClaimCollection),
        TopologyPhase::Complete => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopologyCoverage {
    pub reason: TopologyCoverageReason,
    /// The phase containing the stop boundary, or Complete after all phases.
    pub phase: TopologyPhase,
    /// Fully evaluated units in this phase. Units are pages, claims, cells, or
    /// traversal work items according to the named phase.
    pub evaluated: u64,
    /// Total units in this phase when known before the phase finishes.
    pub total: Option<u64>,
    /// The next one-based unit in this phase, if one remains or the total is
    /// not yet knowable.
    pub next: Option<u64>,
    /// Unevaluated units in this phase when the total is known.
    pub remainder: Option<u64>,
    /// The next phase when a stop landed exactly after this phase completed.
    pub next_phase: Option<TopologyPhase>,
    pub traversal_budget: TraversalBudget,
}

impl TraversalBudget {
    const DEFAULT_MAX_TOTAL_PAGES: u64 = 1_000_000;

    #[must_use]
    /// Creates per-path traversal ceilings.
    ///
    /// # Panics
    ///
    /// Panics when `max_btree_pages` is zero because a root boundary cannot
    /// otherwise be represented by the traversal evidence model.
    pub const fn new(max_btree_pages: u32, max_overflow_pages: u32) -> Self {
        assert!(
            max_btree_pages > 0,
            "B-tree traversal budget must retain at least the root page"
        );
        Self {
            btree_depth: max_btree_pages,
            overflow_depth: max_overflow_pages,
            allocation_ceiling: Self::DEFAULT_MAX_TOTAL_PAGES,
        }
    }

    #[must_use]
    /// Creates per-path and aggregate traversal ceilings.
    ///
    /// # Panics
    ///
    /// Panics when `max_btree_pages` or `max_total_pages` is zero because a
    /// root boundary cannot otherwise be represented by the traversal
    /// evidence model.
    pub const fn with_total_pages(
        max_btree_pages: u32,
        max_overflow_pages: u32,
        max_total_pages: u64,
    ) -> Self {
        assert!(
            max_btree_pages > 0,
            "B-tree traversal budget must retain at least the root page"
        );
        assert!(
            max_total_pages > 0,
            "aggregate traversal budget must retain at least one page identity"
        );
        Self {
            btree_depth: max_btree_pages,
            overflow_depth: max_overflow_pages,
            allocation_ceiling: max_total_pages,
        }
    }

    #[must_use]
    pub const fn max_btree_pages(self) -> u32 {
        self.btree_depth
    }

    #[must_use]
    pub const fn max_overflow_pages(self) -> u32 {
        self.overflow_depth
    }

    #[must_use]
    pub const fn max_total_pages(self) -> u64 {
        self.allocation_ceiling
    }
}

impl Default for TraversalBudget {
    fn default() -> Self {
        Self::new(u32::MAX, u32::MAX)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraversalStop {
    pub reason: TraversalStopReason,
    pub claim_id: String,
    pub intended_target: Option<PageIdentity>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Traversal {
    pub kind: TraversalKind,
    pub origin: EntityIdentity,
    pub validated_prefix: Vec<PageIdentity>,
    pub stop: Option<TraversalStop>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Containment {
    TraversalStopped,
    RelationshipExcluded,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StructuralDiagnostic {
    pub code: &'static str,
    pub severity: DiagnosticSeverity,
    pub evidence: Vec<PhysicalEvidence>,
    pub affected_relationships: Vec<String>,
    pub containment: Containment,
}

pub(super) struct Topology {
    pub freelist: super::FreelistEvidence,
    pub claims: Vec<RelationshipClaim>,
    pub relationships: Vec<Relationship>,
    pub traversals: Vec<Traversal>,
    pub diagnostics: Vec<StructuralDiagnostic>,
    pub coverage: TopologyCoverage,
}

impl Topology {
    pub(super) fn empty(budget: TraversalBudget) -> Self {
        Self {
            freelist: super::FreelistEvidence::uninspected(),
            claims: Vec::new(),
            relationships: Vec::new(),
            traversals: Vec::new(),
            diagnostics: Vec::new(),
            coverage: TopologyCoverage {
                reason: TopologyCoverageReason::Complete,
                phase: TopologyPhase::Complete,
                evaluated: 0,
                total: Some(0),
                next: None,
                remainder: Some(0),
                next_phase: None,
                traversal_budget: budget,
            },
        }
    }
}

struct OverflowTopology {
    claims: Vec<RelationshipClaim>,
    traversals: Vec<Traversal>,
    diagnostics: Vec<StructuralDiagnostic>,
    page_claims: std::collections::HashMap<u32, usize>,
    traversal_pages: u64,
}

impl OverflowTopology {
    fn empty(traversal_pages: u64) -> Self {
        Self {
            claims: Vec::new(),
            traversals: Vec::new(),
            diagnostics: Vec::new(),
            page_claims: std::collections::HashMap::new(),
            traversal_pages,
        }
    }
}

/// Topology sees allocation projections without cloning the structural inventory.
struct PageInventory<'a> {
    pages: &'a [PageEntity],
    overrides: &'a std::collections::HashMap<u32, PageEntity>,
}

impl PageInventory<'_> {
    fn len(&self) -> usize {
        self.pages.len()
    }

    fn get(&self, index: usize) -> Option<&PageEntity> {
        self.pages
            .get(index)
            .map(|page| self.overrides.get(&page.number).unwrap_or(page))
    }

    fn iter(&self) -> impl Iterator<Item = &PageEntity> {
        self.pages
            .iter()
            .map(|page| self.overrides.get(&page.number).unwrap_or(page))
    }
}

impl std::ops::Index<usize> for PageInventory<'_> {
    type Output = PageEntity;

    fn index(&self, index: usize) -> &Self::Output {
        self.get(index)
            .expect("index is bounded by the inspected inventory")
    }
}

struct OverflowSource<'a> {
    file: &'a File,
    geometry: &'a DatabaseGeometry,
    pages: &'a PageInventory<'a>,
    max_pages: u32,
    max_total_pages: u64,
    cancelled: &'a WorkControl,
}

pub(super) fn inspect(
    file: &File,
    geometry: &DatabaseGeometry,
    pages: &[PageEntity],
    budget: TraversalBudget,
    cancelled: &WorkControl,
) -> Topology {
    let mut allocation = super::freelist::inspect(file, geometry, pages, budget, cancelled);
    if cancelled.stopped() || cancelled.aggregate_budget_exhausted() {
        allocation.coverage = cancelled.coverage(budget);
        return allocation;
    }
    let inventory = PageInventory {
        pages,
        overrides: &allocation.freelist.page_overrides,
    };
    let pages = &inventory;
    let allocation_pages = u64::from(allocation.freelist.coverage.evaluated_pages);
    let mut claims = collect_btree_claims(pages, cancelled);
    let mut diagnostics = validate_btree_claims(geometry.page_count, pages, &mut claims, cancelled);
    let (mut relationships, btree_relationships_complete) = normalize_relationships(
        &claims,
        TopologyPhase::BtreeRelationshipNormalization,
        cancelled,
    );
    let (mut traversals, traversal_pages) = if btree_relationships_complete && !cancelled.stopped()
    {
        btree_traversals(
            pages,
            &claims,
            pages.len() == geometry.page_count as usize,
            budget.max_btree_pages(),
            budget.max_total_pages().saturating_sub(allocation_pages),
            cancelled,
        )
    } else {
        (Vec::new(), 0)
    };
    let mut overflow = if cancelled.stopped() || cancelled.aggregate_budget_exhausted() {
        OverflowTopology::empty(traversal_pages)
    } else {
        inspect_overflow(
            file,
            geometry,
            pages,
            budget,
            traversal_pages + allocation_pages,
            cancelled,
        )
    };
    let mut overflow_relationships =
        if cancelled.stopped() || cancelled.aggregate_budget_exhausted() {
            Vec::new()
        } else {
            normalize_relationships(
                &overflow.claims,
                TopologyPhase::OverflowRelationshipNormalization,
                cancelled,
            )
            .0
        };
    relationships.append(&mut overflow_relationships);
    claims.append(&mut overflow.claims);
    traversals.append(&mut overflow.traversals);
    diagnostics.append(&mut overflow.diagnostics);
    if !cancelled.stopped() && !cancelled.aggregate_budget_exhausted() {
        super::freelist::reconcile_storage(
            &mut allocation,
            &mut claims,
            &mut traversals,
            cancelled,
        );
    }
    relationships.append(&mut allocation.relationships);
    claims.append(&mut allocation.claims);
    traversals.append(&mut allocation.traversals);
    diagnostics.append(&mut allocation.diagnostics);
    cancelled.complete();
    Topology {
        freelist: allocation.freelist,
        claims,
        relationships,
        traversals,
        diagnostics,
        coverage: cancelled.coverage(budget),
    }
}

fn normalize_relationships(
    claims: &[RelationshipClaim],
    phase: TopologyPhase,
    cancelled: &WorkControl,
) -> (Vec<Relationship>, bool) {
    cancelled.begin_phase(phase, Some(claims.len() as u64));
    let mut relationships = Vec::new();
    for claim in claims {
        if cancelled.stopped() {
            return (relationships, false);
        }
        if claim.state == RelationshipState::Validated {
            relationships.push(Relationship {
                claim_id: claim.id.clone(),
                kind: claim.kind,
                source: claim.source.clone(),
                target: claim.target.clone().expect("validated claim has a target"),
            });
        }
        cancelled.advance(phase);
    }
    cancelled.finish_phase(phase);
    (relationships, true)
}

fn collect_btree_claims(
    pages: &PageInventory<'_>,
    cancelled: &WorkControl,
) -> Vec<RelationshipClaim> {
    let mut claims = Vec::new();
    cancelled.begin_phase(
        TopologyPhase::BtreeClaimCollection,
        Some(pages.len() as u64),
    );
    for page in pages.iter() {
        if cancelled.stopped() {
            return claims;
        }
        if matches!(
            page.detail.kind,
            Some(super::BtreeKind::TableInterior | super::BtreeKind::IndexInterior)
        ) {
            for cell in &page.detail.cells {
                let Some(target) = cell.left_child else {
                    continue;
                };
                claims.push(RelationshipClaim {
                    id: format!("btree:page:{}:cell:{}", page.number, cell.identity.index),
                    kind: RelationshipKind::BtreeChild,
                    source: EntityIdentity::Cell {
                        page_number: page.number,
                        cell_index: cell.identity.index,
                    },
                    target: Some(PageIdentity {
                        page_number: target,
                    }),
                    evidence: PhysicalEvidence {
                        page: PageIdentity {
                            page_number: page.number,
                        },
                        range: cell
                            .left_child_pointer
                            .clone()
                            .expect("left-child value has source evidence"),
                        validation_rule: "sqlite_btree_interior_left_child",
                    },
                    state: RelationshipState::Unresolved,
                    stop_reason: None,
                });
            }
            if let Some(header) = &page.detail.header
                && let Some(target) = header.rightmost_child
            {
                let page_offset = header.range.page_offset + 8;
                claims.push(RelationshipClaim {
                    id: format!("btree:page:{}:rightmost", page.number),
                    kind: RelationshipKind::BtreeChild,
                    source: EntityIdentity::Page {
                        page_number: page.number,
                    },
                    target: Some(PageIdentity {
                        page_number: target,
                    }),
                    evidence: PhysicalEvidence {
                        page: PageIdentity {
                            page_number: page.number,
                        },
                        range: ByteRange {
                            page_offset,
                            file_offset: header.range.file_offset + 8,
                            length: 4,
                        },
                        validation_rule: "sqlite_btree_interior_rightmost_child",
                    },
                    state: RelationshipState::Unresolved,
                    stop_reason: None,
                });
            }
        }
        cancelled.advance(TopologyPhase::BtreeClaimCollection);
    }
    cancelled.finish_phase(TopologyPhase::BtreeClaimCollection);
    claims
}

fn validate_btree_claims(
    page_count: u32,
    pages: &PageInventory<'_>,
    claims: &mut [RelationshipClaim],
    cancelled: &WorkControl,
) -> Vec<StructuralDiagnostic> {
    cancelled.begin_phase(
        TopologyPhase::BtreeClaimValidation,
        Some(claims.len() as u64),
    );
    let mut diagnostics = Vec::new();
    for index in 0..claims.len() {
        if cancelled.stopped() {
            downgrade_validated_claims(claims, RelationshipKind::BtreeChild);
            return diagnostics;
        }
        let claim = &mut claims[index];
        if let Some(diagnostic) = validate_btree_claim(page_count, pages, claim) {
            diagnostics.push(diagnostic);
        }
        cancelled.advance(TopologyPhase::BtreeClaimValidation);
    }
    cancelled.finish_phase(TopologyPhase::BtreeClaimValidation);
    if !cancelled.stopped() {
        diagnostics.extend(mark_duplicate_btree_parents(pages.len(), claims, cancelled));
    }
    if !cancelled.stopped() {
        diagnostics.extend(mark_btree_cycles(pages.len(), claims, cancelled));
    }
    if cancelled.stopped() {
        downgrade_validated_claims(claims, RelationshipKind::BtreeChild);
    }
    diagnostics
}

fn downgrade_validated_claims(claims: &mut [RelationshipClaim], kind: RelationshipKind) {
    for claim in claims {
        if claim.kind == kind && claim.state == RelationshipState::Validated {
            claim.state = RelationshipState::Unresolved;
            claim.stop_reason = None;
        }
    }
}

fn validate_btree_claim(
    page_count: u32,
    pages: &PageInventory<'_>,
    claim: &mut RelationshipClaim,
) -> Option<StructuralDiagnostic> {
    let source_number = source_page(&claim.source);
    let source_cell = match claim.source {
        EntityIdentity::Cell { cell_index, .. } => pages[(source_number - 1) as usize]
            .detail
            .cells
            .get(cell_index as usize),
        EntityIdentity::Page { .. } => None,
    };
    let (state, reason, code) =
        if source_cell.is_some_and(|cell| cell.diagnostic == Some("overlapping_allocation")) {
            (
                RelationshipState::Invalid,
                TraversalStopReason::OverlappingExtent,
                "btree_child_overlapping_source",
            )
        } else {
            let target = claim.target.as_ref().expect("B-tree claim has a target");
            if target.page_number == 0 {
                (
                    RelationshipState::Invalid,
                    TraversalStopReason::InvalidReference,
                    "btree_child_invalid_page",
                )
            } else if target.page_number > page_count {
                (
                    RelationshipState::Unresolved,
                    TraversalStopReason::OutOfRange,
                    "btree_child_out_of_range",
                )
            } else if let Some(target_page) = pages.get((target.page_number - 1) as usize) {
                if target_page.detail.diagnostics.contains(&"page_read_failed") {
                    claim.state = RelationshipState::Unresolved;
                    claim.stop_reason = Some(TraversalStopReason::MissingTarget);
                    return Some(diagnostic(
                        "btree_child_missing",
                        claim,
                        Containment::TraversalStopped,
                    ));
                }
                let source_kind = pages[(source_number - 1) as usize].detail.kind;
                if compatible_btree_kinds(source_kind, target_page.detail.kind) {
                    claim.state = RelationshipState::Validated;
                    return None;
                }
                (
                    RelationshipState::Invalid,
                    TraversalStopReason::TypeMismatch,
                    "btree_child_type_mismatch",
                )
            } else {
                claim.state = RelationshipState::Unresolved;
                claim.stop_reason = Some(TraversalStopReason::CoverageStop);
                return None;
            }
        };
    claim.state = state;
    claim.stop_reason = Some(reason);
    Some(diagnostic(code, claim, Containment::TraversalStopped))
}

fn mark_duplicate_btree_parents(
    page_count: usize,
    claims: &mut [RelationshipClaim],
    cancelled: &WorkControl,
) -> Vec<StructuralDiagnostic> {
    cancelled.begin_phase(TopologyPhase::BtreeParentReconciliation, None);
    let mut incoming = vec![Vec::new(); page_count + 1];
    for (index, claim) in claims.iter().enumerate() {
        if cancelled.stopped() {
            return Vec::new();
        }
        if claim.state == RelationshipState::Validated {
            incoming[claim.target.as_ref().unwrap().page_number as usize].push(index);
        }
        cancelled.advance(TopologyPhase::BtreeParentReconciliation);
    }
    let mut diagnostics = Vec::new();
    for indexes in incoming {
        if cancelled.stopped() {
            return diagnostics;
        }
        cancelled.advance(TopologyPhase::BtreeParentReconciliation);
        if indexes.len() > 1 {
            let Some(diagnostic) = conflicting_claims_bounded(
                claims,
                &indexes,
                "btree_duplicate_parent",
                TraversalStopReason::ConflictingClaim,
                Containment::RelationshipExcluded,
                cancelled,
                TopologyPhase::BtreeParentReconciliation,
            ) else {
                return diagnostics;
            };
            diagnostics.push(diagnostic);
        }
    }
    cancelled.finish_phase(TopologyPhase::BtreeParentReconciliation);
    diagnostics
}

fn mark_btree_cycles(
    page_count: usize,
    claims: &mut [RelationshipClaim],
    cancelled: &WorkControl,
) -> Vec<StructuralDiagnostic> {
    cancelled.begin_phase(TopologyPhase::BtreeCycleReconciliation, None);
    let cycles = btree_cycles(page_count, claims, cancelled);
    if cancelled.stopped() {
        return Vec::new();
    }
    let mut cycle_by_page = vec![None; page_count + 1];
    for (cycle_index, cycle) in cycles.iter().enumerate() {
        for page in cycle {
            if cancelled.stopped() {
                return Vec::new();
            }
            cycle_by_page[*page as usize] = Some(cycle_index);
            cancelled.advance(TopologyPhase::BtreeCycleReconciliation);
        }
    }
    let mut indexes_by_cycle = vec![Vec::new(); cycles.len()];
    for (index, claim) in claims.iter().enumerate() {
        if cancelled.stopped() {
            return Vec::new();
        }
        if claim.state == RelationshipState::Validated {
            let source_cycle = cycle_by_page[source_page(&claim.source) as usize];
            let target_cycle = cycle_by_page[claim.target.as_ref().unwrap().page_number as usize];
            if let Some(cycle) = source_cycle
                && source_cycle == target_cycle
            {
                indexes_by_cycle[cycle].push(index);
            }
        }
        cancelled.advance(TopologyPhase::BtreeCycleReconciliation);
    }
    let mut diagnostics = Vec::new();
    for indexes in indexes_by_cycle {
        let Some(diagnostic) = conflicting_claims_bounded(
            claims,
            &indexes,
            "btree_cycle",
            TraversalStopReason::Cycle,
            Containment::TraversalStopped,
            cancelled,
            TopologyPhase::BtreeCycleReconciliation,
        ) else {
            return diagnostics;
        };
        diagnostics.push(diagnostic);
    }
    cancelled.finish_phase(TopologyPhase::BtreeCycleReconciliation);
    diagnostics
}

fn conflicting_claims_bounded(
    claims: &mut [RelationshipClaim],
    indexes: &[usize],
    code: &'static str,
    reason: TraversalStopReason,
    containment: Containment,
    cancelled: &WorkControl,
    phase: TopologyPhase,
) -> Option<StructuralDiagnostic> {
    let mut evidence = Vec::with_capacity(indexes.len());
    let mut affected_relationships = Vec::with_capacity(indexes.len());
    for index in indexes {
        if cancelled.stopped() {
            return None;
        }
        evidence.push(claims[*index].evidence.clone());
        affected_relationships.push(claims[*index].id.clone());
        cancelled.advance(phase);
    }
    let mut changed: Vec<usize> = Vec::with_capacity(indexes.len());
    for index in indexes {
        if cancelled.stopped() {
            for changed_index in changed {
                claims[changed_index].state = RelationshipState::Validated;
                claims[changed_index].stop_reason = None;
            }
            return None;
        }
        claims[*index].state = RelationshipState::Conflicting;
        claims[*index].stop_reason = Some(reason);
        changed.push(*index);
        cancelled.advance(phase);
    }
    Some(StructuralDiagnostic {
        code,
        severity: DiagnosticSeverity::Error,
        evidence,
        affected_relationships,
        containment,
    })
}

fn inspect_overflow(
    file: &File,
    geometry: &DatabaseGeometry,
    pages: &PageInventory<'_>,
    budget: TraversalBudget,
    traversal_pages: u64,
    cancelled: &WorkControl,
) -> OverflowTopology {
    let mut result = OverflowTopology::empty(traversal_pages);
    let source = OverflowSource {
        file,
        geometry,
        pages,
        max_pages: budget.max_overflow_pages(),
        max_total_pages: budget.max_total_pages(),
        cancelled,
    };
    let Some(cell_count) = count_overflow_cells(pages, cancelled) else {
        return result;
    };
    cancelled.begin_phase(TopologyPhase::OverflowInspection, Some(cell_count));
    for page in pages.iter() {
        if cancelled.aggregate_budget_exhausted() {
            downgrade_validated_claims(&mut result.claims, RelationshipKind::Overflow);
            return result;
        }
        if cancelled.stopped() {
            downgrade_validated_claims(&mut result.claims, RelationshipKind::Overflow);
            return result;
        }
        for cell in &page.detail.cells {
            if cancelled.aggregate_budget_exhausted() {
                downgrade_validated_claims(&mut result.claims, RelationshipKind::Overflow);
                return result;
            }
            if cancelled.stopped() {
                downgrade_validated_claims(&mut result.claims, RelationshipKind::Overflow);
                return result;
            }
            if inspect_overflow_cell(&source, page.number, cell, &mut result) {
                cancelled.advance(TopologyPhase::OverflowInspection);
            } else {
                downgrade_validated_claims(&mut result.claims, RelationshipKind::Overflow);
                return result;
            }
        }
    }
    cancelled.finish_phase(TopologyPhase::OverflowInspection);
    let reconciled = !cancelled.stopped() && reconcile_overflow_owners(&mut result, cancelled);
    if !reconciled {
        downgrade_validated_claims(&mut result.claims, RelationshipKind::Overflow);
    }
    result
}

fn count_overflow_cells(pages: &PageInventory<'_>, cancelled: &WorkControl) -> Option<u64> {
    cancelled.begin_phase(TopologyPhase::OverflowInspection, None);
    let mut cell_count = 0_u64;
    for page in pages.iter() {
        if cancelled.stopped() {
            return None;
        }
        cell_count = cell_count.saturating_add(page.detail.cells.len() as u64);
        cancelled.advance(TopologyPhase::OverflowInspection);
    }
    Some(cell_count)
}

fn inspect_overflow_cell(
    source: &OverflowSource<'_>,
    page_number: u32,
    cell: &super::CellDetail,
    topology: &mut OverflowTopology,
) -> bool {
    let (Some(first_page), Some(pointer)) = (cell.overflow_page, cell.overflow_pointer.clone())
    else {
        return true;
    };
    let origin = EntityIdentity::Cell {
        page_number,
        cell_index: cell.identity.index,
    };
    let mut first = RelationshipClaim {
        id: format!("overflow:cell:{page_number}:{}", cell.identity.index),
        kind: RelationshipKind::Overflow,
        source: origin.clone(),
        target: (first_page != 0).then_some(PageIdentity {
            page_number: first_page,
        }),
        evidence: PhysicalEvidence {
            page: PageIdentity { page_number },
            range: pointer,
            validation_rule: "sqlite_btree_first_overflow_page",
        },
        state: RelationshipState::Unresolved,
        stop_reason: None,
    };
    if cell.diagnostic == Some("overlapping_allocation") {
        invalidate_overflow_claim(
            &mut first,
            RelationshipState::Invalid,
            TraversalStopReason::OverlappingExtent,
            "overflow_overlapping_source",
            &mut topology.diagnostics,
        );
    } else {
        validate_overflow_target(
            source.geometry.page_count,
            source.pages,
            &mut first,
            "overflow_first_page_missing",
            "overflow_first_page_out_of_range",
            &mut topology.diagnostics,
        );
    }
    let first_claim_index = topology.claims.len();
    topology.claims.push(first);
    let claim = &topology.claims[first_claim_index];
    if claim.state != RelationshipState::Validated {
        topology
            .traversals
            .push(stopped_overflow_traversal(origin, Vec::new(), claim));
        return true;
    }
    let aggregate_budget_exhausted = topology.traversal_pages >= source.max_total_pages;
    let external_stop_reason = source.cancelled.traversal_reason();
    if source.max_pages == 0 || aggregate_budget_exhausted || external_stop_reason.is_some() {
        if aggregate_budget_exhausted && external_stop_reason.is_none() {
            source.cancelled.mark_aggregate_budget_exhausted();
        } else if source.max_pages == 0 && external_stop_reason.is_none() {
            source.cancelled.mark_budget_exhausted();
        }
        topology
            .traversals
            .push(stopped_overflow_traversal_with_reason(
                origin,
                Vec::new(),
                claim,
                external_stop_reason.unwrap_or(TraversalStopReason::Budget),
            ));
        return !aggregate_budget_exhausted && external_stop_reason.is_none();
    }
    let remaining = cell
        .payload_size
        .zip(cell.local_payload.as_ref())
        .map_or(0, |(total, local)| {
            total.saturating_sub(u64::from(local.length))
        });
    follow_overflow_chain(
        source,
        origin,
        first_page,
        first_claim_index,
        remaining,
        topology,
    )
}

fn follow_overflow_chain(
    source: &OverflowSource<'_>,
    origin: EntityIdentity,
    first_page: u32,
    first_claim_index: usize,
    mut remaining: u64,
    topology: &mut OverflowTopology,
) -> bool {
    let mut current = first_page;
    let mut incoming_claim_index = first_claim_index;
    let mut prefix = Vec::new();
    let mut visited = std::collections::HashSet::new();
    loop {
        if source.cancelled.stopped() {
            record_cancelled_overflow(source, origin, prefix, incoming_claim_index, topology);
            return false;
        }
        if topology.traversal_pages >= source.max_total_pages {
            stop_at_overflow_aggregate_budget(
                source,
                origin,
                prefix,
                incoming_claim_index,
                topology,
            );
            return false;
        }
        topology.traversal_pages += 1;
        assert!(
            visited.insert(current),
            "overflow cycle escaped containment"
        );
        prefix.push(PageIdentity {
            page_number: current,
        });
        remaining = remaining.saturating_sub(u64::from(source.geometry.usable_size - 4));
        let claim_index = overflow_next_claim(source, current, topology);
        let mut claim = topology.claims[claim_index].clone();
        match claim.state {
            RelationshipState::Terminal if remaining == 0 => {
                topology.traversals.push(complete_overflow(origin, prefix));
                return true;
            }
            RelationshipState::Terminal => {
                record_overflow_stop(
                    topology,
                    origin,
                    prefix,
                    &claim,
                    TraversalStopReason::InvalidReference,
                    "overflow_chain_truncated",
                );
                return true;
            }
            RelationshipState::Validated if remaining == 0 => {
                record_overflow_stop(
                    topology,
                    origin,
                    prefix,
                    &claim,
                    TraversalStopReason::InvalidReference,
                    "overflow_chain_too_long",
                );
                return true;
            }
            RelationshipState::Validated => {
                let next = claim
                    .target
                    .as_ref()
                    .expect("validated overflow claim has a target")
                    .page_number;
                if visited.contains(&next) {
                    claim.state = RelationshipState::Conflicting;
                    claim.stop_reason = Some(TraversalStopReason::Cycle);
                    topology.claims[claim_index] = claim.clone();
                    record_overflow_stop(
                        topology,
                        origin,
                        prefix,
                        &claim,
                        TraversalStopReason::Cycle,
                        "overflow_cycle",
                    );
                    return true;
                }
                if prefix.len() >= source.max_pages as usize {
                    source.cancelled.mark_budget_exhausted();
                    topology
                        .traversals
                        .push(stopped_overflow_traversal_with_reason(
                            origin,
                            prefix,
                            &claim,
                            TraversalStopReason::Budget,
                        ));
                    return true;
                }
                current = next;
                incoming_claim_index = claim_index;
            }
            _ => {
                topology
                    .traversals
                    .push(stopped_overflow_traversal(origin, prefix, &claim));
                return true;
            }
        }
    }
}

fn stop_at_overflow_aggregate_budget(
    source: &OverflowSource<'_>,
    origin: EntityIdentity,
    prefix: Vec<PageIdentity>,
    claim_index: usize,
    topology: &mut OverflowTopology,
) {
    source.cancelled.mark_aggregate_budget_exhausted();
    let traversal = stopped_overflow_traversal_with_reason(
        origin,
        prefix,
        &topology.claims[claim_index],
        TraversalStopReason::Budget,
    );
    topology.traversals.push(traversal);
}

fn record_cancelled_overflow(
    source: &OverflowSource<'_>,
    origin: EntityIdentity,
    prefix: Vec<PageIdentity>,
    claim_index: usize,
    topology: &mut OverflowTopology,
) {
    let traversal = stopped_overflow_traversal_with_reason(
        origin,
        prefix,
        &topology.claims[claim_index],
        source
            .cancelled
            .traversal_reason()
            .expect("stopped topology has a reason"),
    );
    topology.traversals.push(traversal);
}

fn complete_overflow(origin: EntityIdentity, prefix: Vec<PageIdentity>) -> Traversal {
    Traversal {
        kind: TraversalKind::Overflow,
        origin,
        validated_prefix: prefix,
        stop: None,
    }
}

fn record_overflow_stop(
    topology: &mut OverflowTopology,
    origin: EntityIdentity,
    prefix: Vec<PageIdentity>,
    claim: &RelationshipClaim,
    reason: TraversalStopReason,
    code: &'static str,
) {
    topology
        .diagnostics
        .push(diagnostic(code, claim, Containment::TraversalStopped));
    topology
        .traversals
        .push(stopped_overflow_traversal_with_reason(
            origin, prefix, claim, reason,
        ));
}

fn overflow_next_claim(
    source: &OverflowSource<'_>,
    current: u32,
    topology: &mut OverflowTopology,
) -> usize {
    if let Some(index) = topology.page_claims.get(&current) {
        return *index;
    }
    let evidence = PhysicalEvidence {
        page: PageIdentity {
            page_number: current,
        },
        range: ByteRange {
            page_offset: 0,
            file_offset: u64::from(current - 1) * u64::from(source.geometry.page_size),
            length: 4,
        },
        validation_rule: "sqlite_overflow_next_page",
    };
    let next = read_u32(source.file, evidence.range.file_offset);
    let mut claim = RelationshipClaim {
        id: format!("overflow:page:{current}"),
        kind: RelationshipKind::Overflow,
        source: EntityIdentity::Page {
            page_number: current,
        },
        target: next
            .filter(|page_number| *page_number != 0)
            .map(|page_number| PageIdentity { page_number }),
        evidence,
        state: RelationshipState::Unresolved,
        stop_reason: None,
    };
    match next {
        None => invalidate_overflow_claim(
            &mut claim,
            RelationshipState::Invalid,
            TraversalStopReason::InvalidReference,
            "overflow_pointer_read_failed",
            &mut topology.diagnostics,
        ),
        Some(0) => {
            claim.state = RelationshipState::Terminal;
        }
        Some(_) => {
            validate_overflow_target(
                source.geometry.page_count,
                source.pages,
                &mut claim,
                "overflow_next_page_missing",
                "overflow_next_page_out_of_range",
                &mut topology.diagnostics,
            );
        }
    }
    let index = topology.claims.len();
    topology.claims.push(claim);
    topology.page_claims.insert(current, index);
    index
}

fn reconcile_overflow_owners(topology: &mut OverflowTopology, cancelled: &WorkControl) -> bool {
    cancelled.begin_phase(TopologyPhase::OverflowReconciliation, None);
    let mut incoming = std::collections::BTreeMap::<u32, Vec<usize>>::new();
    for (index, claim) in topology.claims.iter().enumerate() {
        if cancelled.stopped() {
            return false;
        }
        if claim.state == RelationshipState::Validated {
            incoming
                .entry(claim.target.as_ref().unwrap().page_number)
                .or_default()
                .push(index);
        }
        cancelled.advance(TopologyPhase::OverflowReconciliation);
    }
    let mut cell_boundaries = std::collections::HashMap::new();
    let mut page_boundaries = std::collections::HashMap::new();
    for indexes in incoming.into_values().filter(|indexes| indexes.len() > 1) {
        let Some(diagnostic) = conflicting_claims_bounded(
            &mut topology.claims,
            &indexes,
            "overflow_duplicate_owner",
            TraversalStopReason::ConflictingClaim,
            Containment::RelationshipExcluded,
            cancelled,
            TopologyPhase::OverflowReconciliation,
        ) else {
            return false;
        };
        topology.diagnostics.push(diagnostic);
        for index in &indexes {
            if cancelled.stopped() {
                return false;
            }
            let claim = &topology.claims[*index];
            match claim.source {
                EntityIdentity::Cell {
                    page_number,
                    cell_index,
                } => {
                    cell_boundaries.insert((page_number, cell_index), *index);
                }
                EntityIdentity::Page { page_number } => {
                    page_boundaries.insert(page_number, *index);
                }
            }
            cancelled.advance(TopologyPhase::OverflowReconciliation);
        }
    }
    for traversal in &mut topology.traversals {
        if cancelled.stopped() {
            return false;
        }
        let origin_boundary = match traversal.origin {
            EntityIdentity::Cell {
                page_number,
                cell_index,
            } => cell_boundaries
                .get(&(page_number, cell_index))
                .copied()
                .map(|index| (0, index)),
            EntityIdentity::Page { .. } => None,
        };
        let mut page_boundary = None;
        for (position, page) in traversal.validated_prefix.iter().enumerate() {
            if cancelled.stopped() {
                return false;
            }
            cancelled.advance(TopologyPhase::OverflowReconciliation);
            if let Some(index) = page_boundaries.get(&page.page_number) {
                page_boundary = Some((position + 1, *index));
                break;
            }
        }
        if let Some((boundary, index)) = origin_boundary.or(page_boundary) {
            let claim = &topology.claims[index];
            traversal.validated_prefix.truncate(boundary);
            traversal.stop = Some(TraversalStop {
                reason: TraversalStopReason::ConflictingClaim,
                claim_id: claim.id.clone(),
                intended_target: claim.target.clone(),
            });
        }
        cancelled.advance(TopologyPhase::OverflowReconciliation);
    }
    cancelled.finish_phase(TopologyPhase::OverflowReconciliation);
    true
}

fn read_u32(file: &File, offset: u64) -> Option<u32> {
    let mut bytes = [0; 4];
    file.read_exact_at(&mut bytes, offset).ok()?;
    Some(u32::from_be_bytes(bytes))
}

fn validate_overflow_target(
    page_count: u32,
    pages: &PageInventory<'_>,
    claim: &mut RelationshipClaim,
    missing_code: &'static str,
    out_of_range_code: &'static str,
    diagnostics: &mut Vec<StructuralDiagnostic>,
) {
    let Some(target) = &claim.target else {
        invalidate_overflow_claim(
            claim,
            RelationshipState::Invalid,
            TraversalStopReason::InvalidReference,
            "overflow_invalid_page",
            diagnostics,
        );
        return;
    };
    if target.page_number > page_count {
        invalidate_overflow_claim(
            claim,
            RelationshipState::Unresolved,
            TraversalStopReason::OutOfRange,
            out_of_range_code,
            diagnostics,
        );
        return;
    }
    let Some(page) = pages.get((target.page_number - 1) as usize) else {
        claim.state = RelationshipState::Unresolved;
        claim.stop_reason = Some(TraversalStopReason::CoverageStop);
        return;
    };
    if page.detail.diagnostics.contains(&"page_read_failed") {
        invalidate_overflow_claim(
            claim,
            RelationshipState::Unresolved,
            TraversalStopReason::MissingTarget,
            missing_code,
            diagnostics,
        );
        return;
    }
    if page.detail.kind.is_some() || page.detail.allocation_role.is_some() {
        invalidate_overflow_claim(
            claim,
            RelationshipState::Conflicting,
            TraversalStopReason::TypeMismatch,
            "overflow_page_type_conflict",
            diagnostics,
        );
    } else {
        claim.state = RelationshipState::Validated;
    }
}

fn invalidate_overflow_claim(
    claim: &mut RelationshipClaim,
    state: RelationshipState,
    reason: TraversalStopReason,
    code: &'static str,
    diagnostics: &mut Vec<StructuralDiagnostic>,
) {
    claim.state = state;
    claim.stop_reason = Some(reason);
    diagnostics.push(diagnostic(code, claim, Containment::TraversalStopped));
}

fn stopped_overflow_traversal(
    origin: EntityIdentity,
    validated_prefix: Vec<PageIdentity>,
    claim: &RelationshipClaim,
) -> Traversal {
    Traversal {
        kind: TraversalKind::Overflow,
        origin,
        validated_prefix,
        stop: Some(TraversalStop {
            reason: stop_reason(claim),
            claim_id: claim.id.clone(),
            intended_target: claim.target.clone(),
        }),
    }
}

fn stopped_overflow_traversal_with_reason(
    origin: EntityIdentity,
    validated_prefix: Vec<PageIdentity>,
    claim: &RelationshipClaim,
    reason: TraversalStopReason,
) -> Traversal {
    Traversal {
        kind: TraversalKind::Overflow,
        origin,
        validated_prefix,
        stop: Some(TraversalStop {
            reason,
            claim_id: claim.id.clone(),
            intended_target: claim.target.clone(),
        }),
    }
}

fn diagnostic(
    code: &'static str,
    claim: &RelationshipClaim,
    containment: Containment,
) -> StructuralDiagnostic {
    StructuralDiagnostic {
        code,
        severity: DiagnosticSeverity::Error,
        evidence: vec![claim.evidence.clone()],
        affected_relationships: vec![claim.id.clone()],
        containment,
    }
}

fn btree_cycles(
    page_count: usize,
    claims: &[RelationshipClaim],
    cancelled: &WorkControl,
) -> Vec<Vec<u32>> {
    let mut parent = vec![None; page_count + 1];
    for claim in claims {
        if cancelled.stopped() {
            return Vec::new();
        }
        if claim.state == RelationshipState::Validated {
            parent[claim.target.as_ref().unwrap().page_number as usize] =
                Some(source_page(&claim.source));
        }
        cancelled.advance(TopologyPhase::BtreeCycleReconciliation);
    }
    let mut done = vec![false; page_count + 1];
    let mut cycles = Vec::new();
    for start in 1..=page_count {
        if cancelled.stopped() {
            return cycles;
        }
        if done[start] {
            cancelled.advance(TopologyPhase::BtreeCycleReconciliation);
            continue;
        }
        let mut path = Vec::new();
        let mut positions = std::collections::HashMap::new();
        let mut current = u32::try_from(start).expect("page inventory is bounded by u32");
        while current != 0 && !done[current as usize] {
            if cancelled.stopped() {
                return cycles;
            }
            cancelled.advance(TopologyPhase::BtreeCycleReconciliation);
            if let Some(position) = positions.insert(current, path.len()) {
                cycles.push(path[position..].to_vec());
                break;
            }
            path.push(current);
            current = parent[current as usize].unwrap_or(0);
        }
        for page in path {
            if cancelled.stopped() {
                return cycles;
            }
            done[page as usize] = true;
            cancelled.advance(TopologyPhase::BtreeCycleReconciliation);
        }
        cancelled.advance(TopologyPhase::BtreeCycleReconciliation);
    }
    cycles
}

fn btree_traversals(
    pages: &PageInventory<'_>,
    claims: &[RelationshipClaim],
    inventory_complete: bool,
    max_pages: u32,
    max_total_pages: u64,
    cancelled: &WorkControl,
) -> (Vec<Traversal>, u64) {
    cancelled.begin_phase(TopologyPhase::BtreeTraversal, None);
    let mut traversal_pages = 0;
    let Some((incoming, outgoing)) = btree_adjacency(pages.len(), claims, cancelled) else {
        return (Vec::new(), traversal_pages);
    };
    let mut traversals = Vec::new();
    for root in pages.iter().filter(|page| {
        page.detail.kind.is_some()
            && !incoming[page.number as usize]
            && (inventory_complete || page.number == 1)
    }) {
        if cancelled.stopped() {
            return (traversals, traversal_pages);
        }
        if !reserve_traversal_pages(&mut traversal_pages, 1, max_total_pages) {
            cancelled.mark_aggregate_budget_exhausted();
            return (traversals, traversal_pages);
        }
        let origin = EntityIdentity::Page {
            page_number: root.number,
        };
        let mut pending = vec![(
            root.number,
            vec![PageIdentity {
                page_number: root.number,
            }],
        )];
        while let Some((page_number, prefix)) = pending.pop() {
            if cancelled.stopped() {
                if let Some(index) = outgoing[page_number as usize].first() {
                    let claim = &claims[*index];
                    traversals.push(Traversal {
                        kind: TraversalKind::Btree,
                        origin: origin.clone(),
                        validated_prefix: prefix,
                        stop: Some(TraversalStop {
                            reason: cancelled
                                .traversal_reason()
                                .expect("stopped topology has a reason"),
                            claim_id: claim.id.clone(),
                            intended_target: claim.target.clone(),
                        }),
                    });
                }
                return (traversals, traversal_pages);
            }
            cancelled.advance(TopologyPhase::BtreeTraversal);
            if outgoing[page_number as usize].is_empty() {
                traversals.push(Traversal {
                    kind: TraversalKind::Btree,
                    origin: origin.clone(),
                    validated_prefix: prefix,
                    stop: None,
                });
                continue;
            }
            let walk = BtreeWalk {
                claims,
                outgoing: &outgoing,
                max_pages,
                max_total_pages,
                cancelled,
            };
            if !walk_btree_children(
                &walk,
                &mut traversals,
                &mut traversal_pages,
                &origin,
                prefix,
                &mut pending,
            ) {
                return (traversals, traversal_pages);
            }
        }
    }
    cancelled.finish_phase(TopologyPhase::BtreeTraversal);
    (traversals, traversal_pages)
}

fn btree_adjacency(
    page_count: usize,
    claims: &[RelationshipClaim],
    cancelled: &WorkControl,
) -> Option<(Vec<bool>, Vec<Vec<usize>>)> {
    let mut incoming = vec![false; page_count + 1];
    let mut outgoing = vec![Vec::new(); page_count + 1];
    for (index, claim) in claims.iter().enumerate() {
        if cancelled.stopped() {
            return None;
        }
        outgoing[source_page(&claim.source) as usize].push(index);
        if claim.state == RelationshipState::Validated {
            incoming[claim.target.as_ref().unwrap().page_number as usize] = true;
        }
        cancelled.advance(TopologyPhase::BtreeTraversal);
    }
    Some((incoming, outgoing))
}

struct BtreeWalk<'a> {
    claims: &'a [RelationshipClaim],
    outgoing: &'a [Vec<usize>],
    max_pages: u32,
    max_total_pages: u64,
    cancelled: &'a WorkControl,
}

fn walk_btree_children(
    walk: &BtreeWalk<'_>,
    traversals: &mut Vec<Traversal>,
    traversal_pages: &mut u64,
    origin: &EntityIdentity,
    prefix: Vec<PageIdentity>,
    pending: &mut Vec<(u32, Vec<PageIdentity>)>,
) -> bool {
    let page_number = prefix
        .last()
        .expect("B-tree prefix is non-empty")
        .page_number;
    for index in walk.outgoing[page_number as usize].iter().rev() {
        let claim = &walk.claims[*index];
        if walk.cancelled.stopped() {
            traversals.push(stopped_btree_traversal(
                origin.clone(),
                prefix,
                claim,
                walk.cancelled
                    .traversal_reason()
                    .expect("stopped topology has a reason"),
            ));
            return false;
        }
        if claim.state != RelationshipState::Validated {
            if !reserve_traversal_pages(traversal_pages, prefix.len() as u64, walk.max_total_pages)
            {
                record_aggregate_btree_stop(walk.cancelled, traversals, origin, prefix, claim);
                return false;
            }
            traversals.push(stopped_btree_traversal(
                origin.clone(),
                prefix.clone(),
                claim,
                stop_reason(claim),
            ));
            continue;
        }
        if prefix.len() >= walk.max_pages as usize {
            if !reserve_traversal_pages(traversal_pages, prefix.len() as u64, walk.max_total_pages)
            {
                record_aggregate_btree_stop(walk.cancelled, traversals, origin, prefix, claim);
                return false;
            }
            walk.cancelled.mark_budget_exhausted();
            traversals.push(stopped_btree_traversal(
                origin.clone(),
                prefix.clone(),
                claim,
                TraversalStopReason::Budget,
            ));
            continue;
        }
        if !reserve_traversal_pages(
            traversal_pages,
            (prefix.len() + 1) as u64,
            walk.max_total_pages,
        ) {
            record_aggregate_btree_stop(walk.cancelled, traversals, origin, prefix, claim);
            return false;
        }
        let mut next_prefix = prefix.clone();
        let target = claim.target.clone().expect("validated target");
        next_prefix.push(target.clone());
        pending.push((target.page_number, next_prefix));
    }
    true
}

fn record_aggregate_btree_stop(
    cancelled: &WorkControl,
    traversals: &mut Vec<Traversal>,
    origin: &EntityIdentity,
    prefix: Vec<PageIdentity>,
    claim: &RelationshipClaim,
) {
    cancelled.mark_aggregate_budget_exhausted();
    traversals.push(stopped_btree_traversal(
        origin.clone(),
        prefix,
        claim,
        TraversalStopReason::Budget,
    ));
}

fn reserve_traversal_pages(used: &mut u64, requested: u64, maximum: u64) -> bool {
    let Some(updated) = used.checked_add(requested) else {
        return false;
    };
    if updated > maximum {
        return false;
    }
    *used = updated;
    true
}

fn stopped_btree_traversal(
    origin: EntityIdentity,
    validated_prefix: Vec<PageIdentity>,
    claim: &RelationshipClaim,
    reason: TraversalStopReason,
) -> Traversal {
    Traversal {
        kind: TraversalKind::Btree,
        origin,
        validated_prefix,
        stop: Some(TraversalStop {
            reason,
            claim_id: claim.id.clone(),
            intended_target: claim.target.clone(),
        }),
    }
}

fn stop_reason(claim: &RelationshipClaim) -> TraversalStopReason {
    claim
        .stop_reason
        .expect("non-validated claim has a stop reason")
}

fn source_page(source: &EntityIdentity) -> u32 {
    match source {
        EntityIdentity::Page { page_number } | EntityIdentity::Cell { page_number, .. } => {
            *page_number
        }
    }
}

fn compatible_btree_kinds(
    source: Option<super::BtreeKind>,
    target: Option<super::BtreeKind>,
) -> bool {
    matches!(
        (source, target),
        (
            Some(super::BtreeKind::TableInterior),
            Some(super::BtreeKind::TableInterior | super::BtreeKind::TableLeaf)
        ) | (
            Some(super::BtreeKind::IndexInterior),
            Some(super::BtreeKind::IndexInterior | super::BtreeKind::IndexLeaf)
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_control_reports_an_interrupted_unit_boundary() {
        let control = WorkControl::new();
        control.begin_phase(TopologyPhase::BtreeClaimCollection, Some(3));
        control.advance(TopologyPhase::BtreeClaimCollection);
        control.stop();

        let coverage = control.coverage(TraversalBudget::default());
        assert_eq!(coverage.reason, TopologyCoverageReason::OperatorStop);
        assert_eq!(coverage.phase, TopologyPhase::BtreeClaimCollection);
        assert_eq!(coverage.evaluated, 1);
        assert_eq!(coverage.next, Some(2));
        assert_eq!(coverage.remainder, Some(2));
        assert_eq!(coverage.next_phase, None);
    }

    #[test]
    fn work_control_reports_a_stop_between_completed_phases() {
        let control = WorkControl::new();
        control.begin_phase(TopologyPhase::BtreeClaimValidation, Some(2));
        control.advance(TopologyPhase::BtreeClaimValidation);
        control.advance(TopologyPhase::BtreeClaimValidation);
        control.finish_phase(TopologyPhase::BtreeClaimValidation);
        control.cancel();

        let coverage = control.coverage(TraversalBudget::default());
        assert_eq!(coverage.reason, TopologyCoverageReason::Cancelled);
        assert_eq!(coverage.evaluated, 2);
        assert_eq!(coverage.next, None);
        assert_eq!(coverage.remainder, Some(0));
        assert_eq!(
            coverage.next_phase,
            Some(TopologyPhase::BtreeParentReconciliation)
        );
    }

    #[test]
    fn completed_topology_discloses_budget_exhaustion() {
        let control = WorkControl::new();
        control.mark_budget_exhausted();
        control.complete();

        let coverage = control.coverage(TraversalBudget::default());
        assert_eq!(coverage.reason, TopologyCoverageReason::Budget);
        assert_eq!(coverage.phase, TopologyPhase::Complete);
    }
}
