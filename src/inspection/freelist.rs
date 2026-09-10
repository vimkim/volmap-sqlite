//! Allocation evidence is read before topology: freed leaves may contain stale B-trees.
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllocationRole {
    FreelistTrunk,
    FreelistLeaf,
    Conflicting,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FreelistField {
    pub value: u32,
    pub evidence: PhysicalEvidence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
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
}

impl FreelistEvidence {
    pub(super) fn uninspected() -> Self {
        Self {
            first_trunk: None,
            declared_count: None,
            trunks: Vec::new(),
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
    pages: &'a super::storage::PageStore,
    control: &'a WorkControl,
    result: Topology,
    incoming: super::index_map::IndexMap<u32, usize>,
    prefix: Vec<PageIdentity>,
    stop: Option<TraversalStop>,
}

pub(super) fn inspect(
    file: &File,
    geometry: &DatabaseGeometry,
    pages: &super::storage::PageStore,
    budget: TraversalBudget,
    control: &WorkControl,
) -> Topology {
    let mut inspector = Inspector {
        file,
        geometry,
        pages,
        control,
        result: Topology {
            page_overrides: super::index_map::IndexMap::new(std::sync::Arc::clone(&pages.context)),
            freelist_trunks: super::index::Sequence::new(std::sync::Arc::clone(&pages.context)),
            diagnostics: super::index::Sequence::new(std::sync::Arc::clone(&pages.context)),
            claims: super::index::Sequence::new(std::sync::Arc::clone(&pages.context)),
            relationships: super::index::Sequence::new(std::sync::Arc::clone(&pages.context)),
            traversals: super::index::Sequence::new(std::sync::Arc::clone(&pages.context)),
            ..Topology::empty(budget)
        },
        incoming: super::index_map::IndexMap::new(std::sync::Arc::clone(&pages.context)),
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
                validation_rule: std::borrow::Cow::Borrowed(rule),
            },
        })
    }

    fn read_failure(&mut self, index: usize) {
        self.fail(
            index,
            "freelist_read_failed",
            TraversalStopReason::MissingTarget,
        );
        if !self
            .result
            .claims
            .edit(index, |stored| stored.state = RelationshipState::Unresolved)
        {
            return;
        }
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

    fn read_header(&mut self) -> Option<FreelistField> {
        let Some(first) = self.field(1, 32, "sqlite_header_first_freelist_trunk") else {
            self.result.freelist.coverage.reason = FreelistCoverageReason::Unreadable;
            return None;
        };
        let Some(count) = self.field(1, 36, "sqlite_header_freelist_page_count") else {
            self.result.freelist.coverage.reason = FreelistCoverageReason::Unreadable;
            return None;
        };
        self.result.freelist.first_trunk = Some(first.clone());
        self.result.freelist.declared_count = Some(count);
        self.result.freelist.coverage.reason = FreelistCoverageReason::Complete;
        Some(first)
    }

    fn stop_for_trunk_budget(&mut self, index: usize) -> bool {
        if (self.prefix.len() as u64) < self.control.freelist_limit() {
            return false;
        }
        self.control
            .mark_local_budget(super::BudgetKind::FreelistTrunks);
        self.result.claims.edit(index, |claim| {
            claim.stop_reason = Some(TraversalStopReason::Budget);
        });
        self.result.freelist.coverage.reason = FreelistCoverageReason::Budget;
        self.stop_at(index);
        true
    }

    fn run(&mut self, budget: TraversalBudget) {
        self.control
            .begin_phase(TopologyPhase::FreelistInspection, None);
        let Some(first) = self.read_header() else {
            return;
        };
        let mut index = self.claim(&first, RelationshipKind::FreelistTrunk);
        'trunks: while (match self.result.claims.get(index) {
            Some(claim) => claim,
            None => return,
        })
        .target
        .is_some()
        {
            if self.stop_for_trunk_budget(index) {
                break;
            }
            if !self.validate(index, AllocationRole::FreelistTrunk, budget) {
                self.stop_at(index);
                break;
            }
            let page = (match self.result.claims.get(index) {
                Some(claim) => claim,
                None => return,
            })
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
            self.result.freelist_trunks.push(FreelistTrunk {
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
                let Some(mut diagnostic) = self.result.diagnostics.last_mut() else {
                    return;
                };
                diagnostic.evidence.push(count.evidence.clone());
                drop(diagnostic);
                if let Some(mut stored) = self.result.page_overrides.get_mut(&page) {
                    stored.detail.coverage = LocalCoverage::Partial;
                }
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
                        (match self.result.claims.get(leaf_index) {
                            Some(claim) => claim,
                            None => return,
                        })
                        .stop_reason,
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
                code: std::borrow::Cow::Borrowed("freelist_total_mismatch"),
                severity: DiagnosticSeverity::Error,
                evidence: vec![first.evidence.clone(), declared.evidence.clone()],
                affected_relationships: vec![
                    (match self.result.claims.get(0) {
                        Some(claim) => claim,
                        None => return,
                    })
                    .id
                    .clone(),
                ],
                containment: Containment::RelationshipExcluded,
            });
            self.result.freelist.coverage.reason = FreelistCoverageReason::InvalidStructure;
        }
        for position in 0..self.prefix.len() {
            let Some(incoming) = self.incoming.get(&self.prefix[position].page_number) else {
                return;
            };
            if (match self.result.claims.get(incoming) {
                Some(claim) => claim,
                None => return,
            })
            .state
                != RelationshipState::Validated
            {
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
        let Some(mut stored) = self.result.page_overrides.get_mut(&page) else {
            return;
        };
        let regions = &mut stored.detail.regions;
        for (kind, start, length) in [
            ("freelist_header", 0, 8),
            ("freelist_leaf_pointers", 8, count * 4),
            ("freelist_unused", end, self.geometry.usable_size - end),
        ] {
            regions.push(super::btree::Region {
                kind: std::borrow::Cow::Borrowed(kind),
                range: ByteRange {
                    page_offset: start,
                    file_offset: u64::from(page - 1) * u64::from(self.geometry.page_size)
                        + u64::from(start),
                    length,
                },
            });
        }
    }

    fn validate_reference(
        &mut self,
        index: usize,
        role: AllocationRole,
        budget: TraversalBudget,
    ) -> Option<u32> {
        let target = self
            .result
            .claims
            .get(index)?
            .target
            .as_ref()
            .map_or(0, |p| p.page_number);
        if let Some(reason) = self.control.traversal_reason() {
            if !self
                .result
                .claims
                .edit(index, |stored| stored.stop_reason = Some(reason))
            {
                return None;
            }
            self.result.freelist.coverage.reason = match reason {
                TraversalStopReason::Cancelled => FreelistCoverageReason::Cancelled,
                TraversalStopReason::Budget => FreelistCoverageReason::Budget,
                _ => FreelistCoverageReason::OperatorStop,
            };
            return None;
        }
        if u64::from(self.result.freelist.coverage.evaluated_pages) >= budget.max_total_pages() {
            self.control.mark_aggregate_budget_exhausted();
            if !self.result.claims.edit(index, |stored| {
                stored.stop_reason = Some(TraversalStopReason::Budget);
            }) {
                return None;
            }
            self.result.freelist.coverage.reason = FreelistCoverageReason::Budget;
            return None;
        }
        if target <= 1 {
            self.fail(
                index,
                "freelist_invalid_page",
                TraversalStopReason::InvalidReference,
            );
            return None;
        }
        if target > self.geometry.page_count {
            self.fail(
                index,
                "freelist_page_out_of_range",
                TraversalStopReason::OutOfRange,
            );
            if !self
                .result
                .claims
                .edit(index, |stored| stored.state = RelationshipState::Unresolved)
            {
                return None;
            }
            return None;
        }
        if super::roles::reserved_role(self.geometry, target).is_some() {
            self.fail(
                index,
                "freelist_reserved_page_conflict",
                TraversalStopReason::ConflictingClaim,
            );
            if !self.result.claims.edit(index, |stored| {
                stored.state = RelationshipState::Conflicting;
            }) {
                return None;
            }
            return None;
        }
        if let Some(previous) = self.incoming.get(&target) {
            self.repeated_claim(index, role, target, previous);
            return None;
        }
        Some(target)
    }

    fn validate(&mut self, index: usize, role: AllocationRole, budget: TraversalBudget) -> bool {
        let Some(target) = self.validate_reference(index, role, budget) else {
            return false;
        };
        let Some(page) = self.pages.get((target - 1) as usize) else {
            self.result.freelist.coverage.stopping_claim.get_or_insert(
                match self.result.claims.get(index) {
                    Some(claim) => claim.id,
                    None => return false,
                },
            );
            if !self.result.claims.edit(index, |stored| {
                stored.stop_reason = Some(TraversalStopReason::CoverageStop);
            }) {
                return false;
            }
            self.result.freelist.coverage.reason = FreelistCoverageReason::CoverageStop;
            return false;
        };
        if page.detail.diagnostics.contains(&"page_read_failed".into()) {
            self.read_failure(index);
            return false;
        }
        // Preserve the original inventory without copying its cells. Only allocation
        // pages receive a structural projection with stale contents withheld.
        self.result.page_overrides.insert(
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
                        .filter(|region| {
                            matches!(region.kind.as_ref(), "usable_space" | "opaque_reserved")
                        })
                        .cloned()
                        .collect(),
                    coverage: LocalCoverage::Complete,
                },
            },
        );
        self.incoming.insert(target, index);
        if !self
            .result
            .claims
            .edit(index, |stored| stored.state = RelationshipState::Validated)
        {
            return false;
        }
        self.result.freelist.coverage.evaluated_pages += 1;
        self.control.advance(TopologyPhase::FreelistInspection);
        true
    }

    fn repeated_claim(&mut self, index: usize, role: AllocationRole, target: u32, previous: usize) {
        let cycle = role == AllocationRole::FreelistTrunk
            && (match self.result.claims.get(previous) {
                Some(claim) => claim,
                None => return,
            })
            .kind
                == RelationshipKind::FreelistTrunk;
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
        if !self.result.claims.edit(index, |stored| {
            stored.state = RelationshipState::Conflicting;
        }) {
            return;
        }
        if !cycle {
            if !self.result.claims.edit(previous, |stored| {
                stored.state = RelationshipState::Conflicting;
            }) {
                return;
            }
            if !self
                .result
                .claims
                .edit(previous, |stored| stored.stop_reason = Some(reason))
            {
                return;
            }
            if let Some(mut stored) = self.result.page_overrides.get_mut(&target) {
                stored.detail.allocation_role = Some(AllocationRole::Conflicting);
                stored.detail.coverage = LocalCoverage::Partial;
            }
        }
        let prior = &(match self.result.claims.get(previous) {
            Some(claim) => claim,
            None => return,
        });
        let Some(mut diagnostic) = self.result.diagnostics.last_mut() else {
            return;
        };
        diagnostic.evidence.push(prior.evidence.clone());
        diagnostic.affected_relationships.push(prior.id.clone());
    }

    fn fail(&mut self, index: usize, code: &'static str, reason: TraversalStopReason) {
        let Some(mut claim) = self.result.claims.get_mut(index) else {
            return;
        };
        claim.state = RelationshipState::Invalid;
        claim.stop_reason = Some(reason);
        self.result.diagnostics.push(StructuralDiagnostic {
            code: std::borrow::Cow::Borrowed(code),
            severity: DiagnosticSeverity::Error,
            evidence: vec![claim.evidence.clone()],
            affected_relationships: vec![claim.id.clone()],
            containment: Containment::TraversalStopped,
        });
        if let Some(mut page) = self
            .result
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
        let claim = &(match self.result.claims.get(index) {
            Some(claim) => claim,
            None => return,
        });
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
    storage: &mut super::index::Sequence<RelationshipClaim>,
    traversals: &mut super::index::Sequence<Traversal>,
    control: &WorkControl,
) {
    control.begin_phase(TopologyPhase::AllocationReconciliation, None);
    let Some(incoming) = super::topology::index_claim_targets(
        &allocation.claims,
        control,
        TopologyPhase::AllocationReconciliation,
    ) else {
        return;
    };
    let mut conflicts = allocation.claims.map();
    for position in 0..storage.len() {
        let Some(mut claim) = storage.get_mut(position) else {
            return;
        };
        if control.traversal_reason().is_some() {
            break;
        }
        control.advance(TopologyPhase::AllocationReconciliation);
        let Some(target) = claim.target.clone() else {
            continue;
        };
        let Some(mut page) = allocation.page_overrides.get_mut(&target.page_number) else {
            continue;
        };
        if claim.stop_reason != Some(TraversalStopReason::TypeMismatch) {
            continue;
        }
        let indexes = control
            .read_index(&incoming, &target.page_number)
            .unwrap_or_default();
        if control.traversal_reason().is_some()
            || !control.reserve_memory(
                (indexes.len() as u64)
                    .saturating_add(1)
                    .saturating_mul(2048),
            )
        {
            return;
        }
        claim.state = RelationshipState::Conflicting;
        claim.stop_reason = Some(TraversalStopReason::ConflictingClaim);
        conflicts.insert(claim.id.clone(), ());
        page.detail.allocation_role = Some(AllocationRole::Conflicting);
        page.detail.coverage = LocalCoverage::Partial;
        let mut evidence = vec![claim.evidence.clone()];
        let mut affected = vec![claim.id.clone()];
        for index in &indexes {
            let Some(mut free) = allocation.claims.get_mut(*index) else {
                return;
            };
            free.state = RelationshipState::Conflicting;
            free.stop_reason = Some(TraversalStopReason::ConflictingClaim);
            conflicts.insert(free.id.clone(), ());
            evidence.push(free.evidence.clone());
            affected.push(free.id.clone());
        }
        allocation.diagnostics.push(StructuralDiagnostic {
            code: std::borrow::Cow::Borrowed("freelist_storage_role_conflict"),
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
    allocation.relationships.retain(
        |relationship| !conflicts.contains_key(&relationship.claim_id),
        || control.traversal_reason().is_none(),
    );
    reconcile_traversal_stops(allocation, traversals, &incoming, &conflicts, control);
    if control.traversal_reason().is_none() {
        control.finish_phase(TopologyPhase::AllocationReconciliation);
    }
}

fn reconcile_traversal_stops(
    allocation: &mut Topology,
    traversals: &mut super::index::Sequence<Traversal>,
    incoming: &super::index_map::IndexMap<u32, Vec<usize>>,
    conflicts: &super::index_map::IndexMap<String, ()>,
    control: &WorkControl,
) {
    for position in 0..traversals.len() {
        if control.traversal_reason().is_some() {
            return;
        }
        let Some(mut traversal) = traversals.get_mut(position) else {
            return;
        };
        if let Some(stop) = &mut traversal.stop
            && conflicts.contains_key(&stop.claim_id)
        {
            stop.reason = TraversalStopReason::ConflictingClaim;
        }
    }
    for position in 0..allocation.traversals.len() {
        if control.traversal_reason().is_some() {
            return;
        }
        let Some(mut traversal) = allocation.traversals.get_mut(position) else {
            return;
        };
        if let Some(position) = traversal.validated_prefix.iter().position(|page| {
            control
                .read_index(incoming, &page.page_number)
                .unwrap_or_default()
                .iter()
                .any(|index| {
                    allocation
                        .claims
                        .get(*index)
                        .is_some_and(|claim| conflicts.contains_key(&claim.id))
                })
        }) {
            let page = traversal.validated_prefix[position].page_number;
            let Some(index) = incoming
                .get(&page)
                .and_then(|indexes| indexes.first().copied())
            else {
                return;
            };
            let Some(claim) = allocation.claims.get(index) else {
                return;
            };
            traversal.validated_prefix.truncate(position);
            traversal.stop = Some(TraversalStop {
                reason: TraversalStopReason::ConflictingClaim,
                claim_id: claim.id.clone(),
                intended_target: claim.target.clone(),
            });
            allocation.freelist.coverage.stopping_claim = Some(claim.id.clone());
        }
    }
}
