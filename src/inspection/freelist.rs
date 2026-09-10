//! Allocation evidence is read before topology: freed leaves may contain stale B-trees.
use std::collections::HashMap;
use std::fs::File;
use std::os::unix::fs::FileExt;

use serde::Serialize;

use super::topology::{Topology, WorkControl};
use super::{
    ByteRange, Containment, DatabaseGeometry, DiagnosticSeverity, EntityIdentity, LocalCoverage,
    PageEntity, PageIdentity, PhysicalEvidence, Relationship, RelationshipClaim, RelationshipKind,
    RelationshipState, StructuralDiagnostic, TopologyPhase, Traversal, TraversalBudget,
    TraversalKind, TraversalStop, TraversalStopReason,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AllocationRole {
    FreelistTrunk,
    FreelistLeaf,
    Conflicting,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FreelistField {
    pub value: u32,
    pub evidence: PhysicalEvidence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FreelistTrunk {
    pub page: PageIdentity,
    pub leaf_count: FreelistField,
    pub capacity: u32,
    pub compatibility_capacity: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FreelistCoverageReason {
    Complete,
    InvalidStructure,
    CoverageStop,
    Budget,
    Cancelled,
    OperatorStop,
    NotInspected,
    Unreadable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FreelistCoverage {
    pub reason: FreelistCoverageReason,
    pub stopping_claim: Option<String>,
    pub evaluated_pages: u32,
    /// A damaged declaration is not a trustworthy remaining-work total.
    pub remainder: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FreelistEvidence {
    pub first_trunk: Option<FreelistField>,
    pub declared_count: Option<FreelistField>,
    pub trunks: Vec<FreelistTrunk>,
    pub coverage: FreelistCoverage,
    #[serde(skip)]
    pub(super) page_overrides: HashMap<u32, PageEntity>,
}

impl FreelistEvidence {
    pub(super) fn uninspected() -> Self {
        Self {
            first_trunk: None,
            declared_count: None,
            trunks: Vec::new(),
            page_overrides: HashMap::new(),
            coverage: FreelistCoverage {
                reason: FreelistCoverageReason::NotInspected,
                stopping_claim: None,
                evaluated_pages: 0,
                remainder: None,
            },
        }
    }
}

struct Inspector<'a> {
    file: &'a File,
    geometry: &'a DatabaseGeometry,
    pages: &'a [PageEntity],
    control: &'a WorkControl,
    result: Topology,
    incoming: HashMap<u32, usize>,
    prefix: Vec<PageIdentity>,
    stop: Option<TraversalStop>,
}

pub(super) fn inspect(
    file: &File,
    geometry: &DatabaseGeometry,
    pages: &[PageEntity],
    budget: TraversalBudget,
    control: &WorkControl,
) -> Topology {
    let mut inspector = Inspector {
        file,
        geometry,
        pages,
        control,
        result: Topology::empty(budget),
        incoming: HashMap::new(),
        prefix: Vec::new(),
        stop: None,
    };
    inspector.run(budget);
    inspector.result
}

impl Inspector<'_> {
    fn field(&self, page: u32, offset: u32, rule: &'static str) -> Option<FreelistField> {
        let file_offset =
            u64::from(page - 1) * u64::from(self.geometry.page_size) + u64::from(offset);
        let mut bytes = [0; 4];
        self.file.read_exact_at(&mut bytes, file_offset).ok()?;
        Some(FreelistField {
            value: u32::from_be_bytes(bytes),
            evidence: PhysicalEvidence {
                page: PageIdentity { page_number: page },
                range: ByteRange {
                    page_offset: offset,
                    file_offset,
                    length: 4,
                },
                validation_rule: rule,
            },
        })
    }

    fn read_failure(&mut self, index: usize) {
        self.fail(
            index,
            "freelist_read_failed",
            TraversalStopReason::MissingTarget,
        );
        self.result.claims[index].state = RelationshipState::Unresolved;
        self.result.freelist.coverage.reason = FreelistCoverageReason::Unreadable;
        self.stop_at(index);
    }

    fn required_field(
        &mut self,
        page: u32,
        offset: u32,
        rule: &'static str,
        index: usize,
    ) -> Option<FreelistField> {
        let field = self.field(page, offset, rule);
        if field.is_none() {
            self.read_failure(index);
        }
        field
    }

    fn claim(&mut self, field: &FreelistField, kind: RelationshipKind) -> usize {
        let page = field.evidence.page.page_number;
        let index = self.result.claims.len();
        self.result.claims.push(RelationshipClaim {
            id: format!(
                "freelist:page:{page}:byte:{}",
                field.evidence.range.page_offset
            ),
            kind,
            source: EntityIdentity::Page { page_number: page },
            target: (field.value != 0 || kind == RelationshipKind::FreelistLeaf).then_some(
                PageIdentity {
                    page_number: field.value,
                },
            ),
            evidence: field.evidence.clone(),
            state: if field.value == 0 && kind == RelationshipKind::FreelistTrunk {
                RelationshipState::Terminal
            } else {
                RelationshipState::Unresolved
            },
            stop_reason: None,
        });
        index
    }

    fn run(&mut self, budget: TraversalBudget) {
        self.control
            .begin_phase(TopologyPhase::FreelistInspection, None);
        let Some(first) = self.field(1, 32, "sqlite_header_first_freelist_trunk") else {
            self.result.freelist.coverage.reason = FreelistCoverageReason::Unreadable;
            return;
        };
        let Some(count) = self.field(1, 36, "sqlite_header_freelist_page_count") else {
            self.result.freelist.coverage.reason = FreelistCoverageReason::Unreadable;
            return;
        };
        self.result.freelist.first_trunk = Some(first.clone());
        self.result.freelist.declared_count = Some(count);
        self.result.freelist.coverage.reason = FreelistCoverageReason::Complete;
        let mut index = self.claim(&first, RelationshipKind::FreelistTrunk);
        'trunks: while self.result.claims[index].target.is_some() {
            if self.prefix.len() as u64 >= self.control.freelist_limit() {
                self.control
                    .mark_local_budget(super::BudgetKind::FreelistTrunks);
                self.result.claims[index].stop_reason = Some(TraversalStopReason::Budget);
                self.result.freelist.coverage.reason = FreelistCoverageReason::Budget;
                self.stop_at(index);
                break;
            }
            if !self.validate(index, AllocationRole::FreelistTrunk, budget) {
                self.stop_at(index);
                break;
            }
            let page = self.result.claims[index]
                .target
                .as_ref()
                .unwrap()
                .page_number;
            self.prefix.push(PageIdentity { page_number: page });
            let Some(next) = self.required_field(page, 0, "sqlite_freelist_next_trunk", index)
            else {
                break;
            };
            let Some(count) = self.required_field(page, 4, "sqlite_freelist_leaf_count", index)
            else {
                break;
            };
            let capacity = self.geometry.usable_size / 4 - 2;
            self.result.freelist.trunks.push(FreelistTrunk {
                page: PageIdentity { page_number: page },
                leaf_count: count.clone(),
                capacity,
                compatibility_capacity: capacity - 6,
            });
            let next_index = self.claim(&next, RelationshipKind::FreelistTrunk);
            if count.value > capacity {
                self.fail(
                    next_index,
                    "freelist_leaf_count_exceeds_capacity",
                    TraversalStopReason::InvalidReference,
                );
                self.result
                    .diagnostics
                    .last_mut()
                    .unwrap()
                    .evidence
                    .push(count.evidence.clone());
                self.result
                    .freelist
                    .page_overrides
                    .get_mut(&page)
                    .unwrap()
                    .detail
                    .coverage = LocalCoverage::Partial;
                self.stop_at(next_index);
                break;
            }
            self.trunk_regions(page, count.value);
            for leaf in 0..count.value {
                let Some(field) = self.required_field(
                    page,
                    8 + 4 * leaf,
                    "sqlite_freelist_leaf_page",
                    next_index,
                ) else {
                    break 'trunks;
                };
                let leaf_index = self.claim(&field, RelationshipKind::FreelistLeaf);
                if !self.validate(leaf_index, AllocationRole::FreelistLeaf, budget)
                    && matches!(
                        self.result.claims[leaf_index].stop_reason,
                        Some(
                            TraversalStopReason::Budget
                                | TraversalStopReason::Cancelled
                                | TraversalStopReason::OperatorStop
                        )
                    )
                {
                    self.stop_at(leaf_index);
                    break 'trunks;
                }
            }
            index = next_index;
        }
        self.finish(&first);
    }

    fn finish(&mut self, first: &FreelistField) {
        let declared = self.result.freelist.declared_count.as_ref().unwrap();
        if self.result.freelist.coverage.reason == FreelistCoverageReason::Complete
            && declared.value != self.result.freelist.coverage.evaluated_pages
        {
            self.result.diagnostics.push(StructuralDiagnostic {
                code: "freelist_total_mismatch",
                severity: DiagnosticSeverity::Error,
                evidence: vec![first.evidence.clone(), declared.evidence.clone()],
                affected_relationships: vec![self.result.claims[0].id.clone()],
                containment: Containment::RelationshipExcluded,
            });
            self.result.freelist.coverage.reason = FreelistCoverageReason::InvalidStructure;
        }
        for position in 0..self.prefix.len() {
            let incoming = self.incoming[&self.prefix[position].page_number];
            if self.result.claims[incoming].state != RelationshipState::Validated {
                self.prefix.truncate(position);
                self.stop_at(incoming);
                break;
            }
        }
        if first.value != 0 {
            self.result.traversals.push(Traversal {
                kind: TraversalKind::Freelist,
                origin: EntityIdentity::Page { page_number: 1 },
                validated_prefix: std::mem::take(&mut self.prefix),
                stop: self.stop.take(),
            });
        }
        for claim in &self.result.claims {
            if claim.state == RelationshipState::Validated {
                self.result.relationships.push(Relationship {
                    claim_id: claim.id.clone(),
                    kind: claim.kind,
                    source: claim.source.clone(),
                    target: claim.target.clone().unwrap(),
                });
            }
        }
        if self.result.freelist.coverage.reason == FreelistCoverageReason::Complete {
            self.result.freelist.coverage.remainder = Some(0);
        }
        if !matches!(
            self.result.freelist.coverage.reason,
            FreelistCoverageReason::Budget
                | FreelistCoverageReason::Cancelled
                | FreelistCoverageReason::OperatorStop
        ) {
            self.control.finish_phase(TopologyPhase::FreelistInspection);
        }
    }

    fn trunk_regions(&mut self, page: u32, count: u32) {
        let end = 8 + count * 4;
        let regions = &mut self
            .result
            .freelist
            .page_overrides
            .get_mut(&page)
            .unwrap()
            .detail
            .regions;
        for (kind, start, length) in [
            ("freelist_header", 0, 8),
            ("freelist_leaf_pointers", 8, count * 4),
            ("freelist_unused", end, self.geometry.usable_size - end),
        ] {
            regions.push(super::btree::Region {
                kind,
                range: ByteRange {
                    page_offset: start,
                    file_offset: u64::from(page - 1) * u64::from(self.geometry.page_size)
                        + u64::from(start),
                    length,
                },
            });
        }
    }

    fn validate(&mut self, index: usize, role: AllocationRole, budget: TraversalBudget) -> bool {
        let target = self.result.claims[index]
            .target
            .as_ref()
            .map_or(0, |p| p.page_number);
        if let Some(reason) = self.control.traversal_reason() {
            self.result.claims[index].stop_reason = Some(reason);
            self.result.freelist.coverage.reason = match reason {
                TraversalStopReason::Cancelled => FreelistCoverageReason::Cancelled,
                TraversalStopReason::Budget => FreelistCoverageReason::Budget,
                _ => FreelistCoverageReason::OperatorStop,
            };
            return false;
        }
        if u64::from(self.result.freelist.coverage.evaluated_pages) >= budget.max_total_pages() {
            self.control.mark_aggregate_budget_exhausted();
            self.result.claims[index].stop_reason = Some(TraversalStopReason::Budget);
            self.result.freelist.coverage.reason = FreelistCoverageReason::Budget;
            return false;
        }
        if target <= 1 {
            self.fail(
                index,
                "freelist_invalid_page",
                TraversalStopReason::InvalidReference,
            );
            return false;
        }
        if target > self.geometry.page_count {
            self.fail(
                index,
                "freelist_page_out_of_range",
                TraversalStopReason::OutOfRange,
            );
            self.result.claims[index].state = RelationshipState::Unresolved;
            return false;
        }
        if super::roles::reserved_role(self.geometry, target).is_some() {
            self.fail(
                index,
                "freelist_reserved_page_conflict",
                TraversalStopReason::ConflictingClaim,
            );
            self.result.claims[index].state = RelationshipState::Conflicting;
            return false;
        }
        if let Some(previous) = self.incoming.get(&target).copied() {
            self.repeated_claim(index, role, target, previous);
            return false;
        }
        let Some(page) = self.pages.get((target - 1) as usize) else {
            self.result
                .freelist
                .coverage
                .stopping_claim
                .get_or_insert_with(|| self.result.claims[index].id.clone());
            self.result.claims[index].stop_reason = Some(TraversalStopReason::CoverageStop);
            self.result.freelist.coverage.reason = FreelistCoverageReason::CoverageStop;
            return false;
        };
        if page.detail.diagnostics.contains(&"page_read_failed") {
            self.read_failure(index);
            return false;
        }
        // Preserve the original inventory without copying its cells. Only allocation
        // pages receive a structural projection with stale contents withheld.
        self.result.freelist.page_overrides.insert(
            target,
            PageEntity {
                number: target,
                classification: page.classification.clone(),
                detail: super::PageDetail {
                    allocation_role: Some(role),
                    kind: None,
                    header: None,
                    cells: Vec::new(),
                    freeblocks: Vec::new(),
                    diagnostics: Vec::new(),
                    regions: page
                        .detail
                        .regions
                        .iter()
                        .filter(|region| matches!(region.kind, "usable_space" | "opaque_reserved"))
                        .cloned()
                        .collect(),
                    coverage: LocalCoverage::Complete,
                },
            },
        );
        self.incoming.insert(target, index);
        self.result.claims[index].state = RelationshipState::Validated;
        self.result.freelist.coverage.evaluated_pages += 1;
        self.control.advance(TopologyPhase::FreelistInspection);
        true
    }

    fn repeated_claim(&mut self, index: usize, role: AllocationRole, target: u32, previous: usize) {
        let cycle = role == AllocationRole::FreelistTrunk
            && self.result.claims[previous].kind == RelationshipKind::FreelistTrunk;
        let reason = if cycle {
            TraversalStopReason::Cycle
        } else {
            TraversalStopReason::ConflictingClaim
        };
        self.fail(
            index,
            if cycle {
                "freelist_trunk_cycle"
            } else {
                "freelist_repeated_claim"
            },
            reason,
        );
        self.result.claims[index].state = RelationshipState::Conflicting;
        if !cycle {
            self.result.claims[previous].state = RelationshipState::Conflicting;
            self.result.claims[previous].stop_reason = Some(reason);
            self.result
                .freelist
                .page_overrides
                .get_mut(&target)
                .unwrap()
                .detail
                .allocation_role = Some(AllocationRole::Conflicting);
            self.result
                .freelist
                .page_overrides
                .get_mut(&target)
                .unwrap()
                .detail
                .coverage = LocalCoverage::Partial;
        }
        let prior = &self.result.claims[previous];
        let diagnostic = self.result.diagnostics.last_mut().unwrap();
        diagnostic.evidence.push(prior.evidence.clone());
        diagnostic.affected_relationships.push(prior.id.clone());
    }

    fn fail(&mut self, index: usize, code: &'static str, reason: TraversalStopReason) {
        let claim = &mut self.result.claims[index];
        claim.state = RelationshipState::Invalid;
        claim.stop_reason = Some(reason);
        self.result.diagnostics.push(StructuralDiagnostic {
            code,
            severity: DiagnosticSeverity::Error,
            evidence: vec![claim.evidence.clone()],
            affected_relationships: vec![claim.id.clone()],
            containment: Containment::TraversalStopped,
        });
        if let Some(page) = self
            .result
            .freelist
            .page_overrides
            .get_mut(&claim.evidence.page.page_number)
        {
            page.detail.coverage = LocalCoverage::Partial;
        }
        self.result.freelist.coverage.reason = FreelistCoverageReason::InvalidStructure;
        self.result
            .freelist
            .coverage
            .stopping_claim
            .get_or_insert_with(|| claim.id.clone());
    }

    fn stop_at(&mut self, index: usize) {
        let claim = &self.result.claims[index];
        self.result.freelist.coverage.stopping_claim = Some(claim.id.clone());
        self.stop = Some(TraversalStop {
            reason: claim
                .stop_reason
                .unwrap_or(TraversalStopReason::InvalidReference),
            claim_id: claim.id.clone(),
            intended_target: claim.target.clone(),
        });
    }
}

/// An incoming storage pointer is independent evidence; a stale local type byte is not.
pub(super) fn reconcile_storage(
    allocation: &mut Topology,
    storage: &mut [RelationshipClaim],
    traversals: &mut [Traversal],
    control: &WorkControl,
) {
    control.begin_phase(TopologyPhase::AllocationReconciliation, None);
    let mut incoming = HashMap::<u32, Vec<usize>>::new();
    for (index, claim) in allocation.claims.iter().enumerate() {
        if control.traversal_reason().is_some() {
            return;
        }
        control.advance(TopologyPhase::AllocationReconciliation);
        if let Some(target) = &claim.target {
            incoming.entry(target.page_number).or_default().push(index);
        }
    }
    let mut conflicts = std::collections::HashSet::new();
    for claim in storage {
        if control.traversal_reason().is_some() {
            break;
        }
        control.advance(TopologyPhase::AllocationReconciliation);
        let Some(target) = &claim.target else {
            continue;
        };
        let Some(page) = allocation
            .freelist
            .page_overrides
            .get_mut(&target.page_number)
        else {
            continue;
        };
        if claim.stop_reason != Some(TraversalStopReason::TypeMismatch) {
            continue;
        }
        claim.state = RelationshipState::Conflicting;
        claim.stop_reason = Some(TraversalStopReason::ConflictingClaim);
        conflicts.insert(claim.id.clone());
        page.detail.allocation_role = Some(AllocationRole::Conflicting);
        page.detail.coverage = LocalCoverage::Partial;
        let mut evidence = vec![claim.evidence.clone()];
        let mut affected = vec![claim.id.clone()];
        for index in &incoming[&target.page_number] {
            let free = &mut allocation.claims[*index];
            free.state = RelationshipState::Conflicting;
            free.stop_reason = Some(TraversalStopReason::ConflictingClaim);
            conflicts.insert(free.id.clone());
            evidence.push(free.evidence.clone());
            affected.push(free.id.clone());
        }
        allocation.diagnostics.push(StructuralDiagnostic {
            code: "freelist_storage_role_conflict",
            severity: DiagnosticSeverity::Error,
            evidence,
            affected_relationships: affected,
            containment: Containment::RelationshipExcluded,
        });
        if allocation.freelist.coverage.reason == FreelistCoverageReason::Complete {
            allocation.freelist.coverage.reason = FreelistCoverageReason::InvalidStructure;
            allocation.freelist.coverage.remainder = None;
        }
    }
    allocation
        .relationships
        .retain(|relationship| !conflicts.contains(&relationship.claim_id));
    for traversal in traversals {
        if let Some(stop) = &mut traversal.stop
            && conflicts.contains(&stop.claim_id)
        {
            stop.reason = TraversalStopReason::ConflictingClaim;
        }
    }
    for traversal in &mut allocation.traversals {
        if let Some(position) = traversal.validated_prefix.iter().position(|page| {
            incoming[&page.page_number]
                .iter()
                .any(|index| conflicts.contains(&allocation.claims[*index].id))
        }) {
            let page = traversal.validated_prefix[position].page_number;
            let claim = &allocation.claims[incoming[&page][0]];
            traversal.validated_prefix.truncate(position);
            traversal.stop = Some(TraversalStop {
                reason: TraversalStopReason::ConflictingClaim,
                claim_id: claim.id.clone(),
                intended_target: claim.target.clone(),
            });
            allocation.freelist.coverage.stopping_claim = Some(claim.id.clone());
        }
    }
    if control.traversal_reason().is_none() {
        control.finish_phase(TopologyPhase::AllocationReconciliation);
    }
}
