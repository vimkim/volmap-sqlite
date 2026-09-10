//! Physical page-role evidence and reconciliation; adapters never classify bytes.
use serde::Serialize;

use super::{
    BtreeKind, ByteRange, DatabaseGeometry, PageDetail, PageIdentity, PhysicalEvidence,
    RelationshipState,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageRole {
    Btree,
    TableLeaf,
    TableInterior,
    IndexLeaf,
    IndexInterior,
    Freelist,
    FreelistTrunk,
    FreelistLeaf,
    Overflow,
    PointerMap,
    LockByte,
    Unknown,
    Conflicting,
}

impl From<BtreeKind> for PageRole {
    fn from(kind: BtreeKind) -> Self {
        match kind {
            BtreeKind::TableLeaf => Self::TableLeaf,
            BtreeKind::TableInterior => Self::TableInterior,
            BtreeKind::IndexLeaf => Self::IndexLeaf,
            BtreeKind::IndexInterior => Self::IndexInterior,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleClaim {
    pub role: PageRole,
    pub source: std::borrow::Cow<'static, str>,
    pub evidence: PhysicalEvidence,
    pub state: RelationshipState,
    pub relationship_id: Option<String>,
    pub relationship_stop: Option<super::TraversalStopReason>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageClassification {
    pub role: PageRole,
    pub claims: Vec<RoleClaim>,
    /// A relationship names this page, independently of whether it validated.
    pub referenced: bool,
    pub reconciled: bool,
}

pub(super) fn local(number: u32, detail: &PageDetail) -> PageClassification {
    let claims = detail
        .kind
        .zip(detail.header.as_ref())
        .map(|(kind, header)| RoleClaim {
            role: kind.into(),
            source: std::borrow::Cow::Borrowed("local_structure"),
            evidence: PhysicalEvidence {
                page: PageIdentity {
                    page_number: number,
                },
                range: header.range.clone(),
                validation_rule: std::borrow::Cow::Borrowed("sqlite_btree_header"),
            },
            state: RelationshipState::Validated,
            relationship_id: None,
            relationship_stop: None,
        })
        .into_iter()
        .collect();
    PageClassification {
        role: detail.kind.map_or(PageRole::Unknown, Into::into),
        claims,
        referenced: false,
        reconciled: false,
    }
}

pub(super) fn evidence(
    geometry: &DatabaseGeometry,
    page: u32,
    offset: u32,
    length: u32,
    rule: &'static str,
) -> PhysicalEvidence {
    PhysicalEvidence {
        page: PageIdentity { page_number: page },
        range: ByteRange {
            page_offset: offset,
            file_offset: u64::from(page - 1) * u64::from(geometry.page_size) + u64::from(offset),
            length,
        },
        validation_rule: std::borrow::Cow::Borrowed(rule),
    }
}

/// The standard `SQLite` pending-byte page is reserved even on VFSes that never use it.
pub(super) fn lock_page(geometry: &DatabaseGeometry) -> u32 {
    0x4000_0000 / geometry.page_size + 1
}

/// `SQLite` ptrmapPageno: keep the periodic sequence anchored at page 2 when the
/// single map at the pending-byte page is displaced by one page.
pub(super) fn map_for(geometry: &DatabaseGeometry, target: u32) -> Option<u32> {
    if geometry.largest_root_page == 0 || target < 2 {
        return None;
    }
    map_number(geometry.page_size, geometry.usable_size, target)
}

pub(super) fn map_number(page_size: u32, usable_size: u32, target: u32) -> Option<u32> {
    let stride = usable_size.checked_div(5)?.checked_add(1)?;
    let nominal = target
        .checked_sub(2)?
        .checked_div(stride)?
        .checked_mul(stride)?
        .checked_add(2)?;
    nominal.checked_add(u32::from(nominal == 0x4000_0000 / page_size + 1))
}

pub(super) fn reserved_role(geometry: &DatabaseGeometry, number: u32) -> Option<PageRole> {
    if number == lock_page(geometry) {
        Some(PageRole::LockByte)
    } else if map_for(geometry, number) == Some(number) {
        Some(PageRole::PointerMap)
    } else {
        None
    }
}

pub(super) fn reserved_detail(geometry: &DatabaseGeometry, number: u32) -> Option<PageDetail> {
    let role = reserved_role(geometry, number)?;
    Some(PageDetail {
        allocation_role: None,
        kind: None,
        header: None,
        cells: vec![],
        freeblocks: vec![],
        diagnostics: vec![],
        coverage: if geometry.file_length < u64::from(number) * u64::from(geometry.page_size) {
            super::LocalCoverage::Partial
        } else {
            super::LocalCoverage::Complete
        },
        regions: vec![super::btree::Region {
            kind: if role == PageRole::LockByte {
                std::borrow::Cow::Borrowed("lock_byte")
            } else {
                std::borrow::Cow::Borrowed("pointer_map")
            },
            range: evidence(
                geometry,
                number,
                0,
                u32::try_from(
                    geometry
                        .file_length
                        .saturating_sub(u64::from(number - 1) * u64::from(geometry.page_size))
                        .min(u64::from(geometry.page_size)),
                )
                .expect("page-bounded length"),
                "sqlite_reserved_page_geometry",
            )
            .range,
        }],
    })
}

pub(super) fn initial(
    geometry: &DatabaseGeometry,
    number: u32,
    detail: &PageDetail,
) -> PageClassification {
    if let Some(role) = reserved_role(geometry, number) {
        PageClassification {
            role,
            referenced: false,
            reconciled: false,
            claims: vec![RoleClaim {
                role,
                source: std::borrow::Cow::Borrowed("database_geometry"),
                evidence: evidence(
                    geometry,
                    1,
                    if role == PageRole::PointerMap { 52 } else { 16 },
                    if role == PageRole::PointerMap { 4 } else { 2 },
                    "sqlite_reserved_page_geometry",
                ),
                state: RelationshipState::Validated,
                relationship_id: None,
                relationship_stop: None,
            }],
        }
    } else {
        local(number, detail)
    }
}

fn merge(left: PageRole, right: PageRole) -> PageRole {
    use PageRole::{
        Btree, Conflicting, Freelist, FreelistLeaf, FreelistTrunk, IndexInterior, IndexLeaf,
        TableInterior, TableLeaf, Unknown,
    };
    match (left, right) {
        (Unknown, role) | (role, Unknown) => role,
        (a, b) if a == b => a,
        (Btree, specific @ (TableLeaf | TableInterior | IndexLeaf | IndexInterior))
        | (specific @ (TableLeaf | TableInterior | IndexLeaf | IndexInterior), Btree)
        | (Freelist, specific @ (FreelistTrunk | FreelistLeaf))
        | (specific @ (FreelistTrunk | FreelistLeaf), Freelist) => specific,
        _ => Conflicting,
    }
}

pub(super) fn reconcile(
    geometry: &DatabaseGeometry,
    pages: &super::storage::PageStore,
    topology: &mut super::topology::Topology,
    control: &super::topology::WorkControl,
) -> bool {
    control.begin_phase(super::TopologyPhase::RoleReconciliation, None);
    let mut classifications = super::index::Sequence::new(std::sync::Arc::clone(&pages.context));
    let complete = collect_claims(geometry, pages, topology, &mut classifications, control)
        && resolve_claims(&mut classifications, topology, control)
        && contain_conflicts(&mut classifications, topology, control)
        && (0..classifications.len()).all(|index| {
            let Some(mut classification) = control.edit_record(&mut classifications, index) else {
                return false;
            };
            if !checkpoint(control) {
                return false;
            }
            classification.reconciled = true;
            true
        });
    if matches!(pages.check(), Err(super::storage::StorageError::Budget)) {
        control.stop_for_storage_budget();
    }
    topology.classifications = classifications;
    if complete {
        control.finish_phase(super::TopologyPhase::RoleReconciliation);
    }
    complete
}

fn checkpoint(control: &super::topology::WorkControl) -> bool {
    if control.traversal_reason().is_some() {
        return false;
    }
    control.advance(super::TopologyPhase::RoleReconciliation);
    true
}

fn collect_claims(
    geometry: &DatabaseGeometry,
    pages: &super::storage::PageStore,
    topology: &mut super::topology::Topology,
    classifications: &mut super::index::Sequence<PageClassification>,
    control: &super::topology::WorkControl,
) -> bool {
    for page in pages.iter() {
        if !checkpoint(control) {
            return false;
        }
        let mut result = page.classification.clone();
        if topology.page_overrides.contains_key(&page.number) {
            // Preserve stale headers as observations, never as live storage roles.
            result.role = PageRole::Unknown;
            for claim in &mut result.claims {
                claim.state = RelationshipState::Unresolved;
            }
        }
        classifications.push(result);
        if pages.check().is_err() {
            return false;
        }
    }
    for claim in &topology.claims {
        if !checkpoint(control) {
            return false;
        }
        let Some(mut target) = claim
            .target
            .as_ref()
            .and_then(|p| p.page_number.checked_sub(1))
            .and_then(|n| control.edit_record(classifications, n as usize))
        else {
            continue;
        };
        target.referenced = true;
        let role = match claim.kind {
            super::RelationshipKind::BtreeChild => PageRole::Btree,
            super::RelationshipKind::Overflow => PageRole::Overflow,
            super::RelationshipKind::FreelistTrunk => PageRole::FreelistTrunk,
            super::RelationshipKind::FreelistLeaf => PageRole::FreelistLeaf,
        };
        target.claims.push(RoleClaim {
            role,
            source: std::borrow::Cow::Borrowed("relationship"),
            evidence: claim.evidence.clone(),
            state: claim.state,
            relationship_id: Some(claim.id.clone()),
            relationship_stop: claim.stop_reason,
        });
    }
    collect_pointer_map_claims(geometry, classifications, topology, control)
}

fn resolve_claims(
    classifications: &mut super::index::Sequence<PageClassification>,
    topology: &mut super::topology::Topology,
    control: &super::topology::WorkControl,
) -> bool {
    for index in 0..classifications.len() {
        let Some(mut classification) = control.edit_record(classifications, index) else {
            return false;
        };
        if !checkpoint(control) {
            return false;
        }
        let classification = &mut *classification;
        classification.role = PageRole::Unknown;
        for claim in &classification.claims {
            if !checkpoint(control) {
                return false;
            }
            if claim.state == RelationshipState::Conflicting
                && claim.relationship_stop != Some(super::TraversalStopReason::Cycle)
            {
                classification.role = PageRole::Conflicting;
            }
            if matches!(
                claim.state,
                RelationshipState::Validated | RelationshipState::Conflicting
            ) {
                classification.role = merge(classification.role, claim.role);
            }
        }
        if classification.role == PageRole::Conflicting {
            let mut evidence = Vec::new();
            let mut affected_relationships = Vec::new();
            for claim in &classification.claims {
                if !checkpoint(control) {
                    return false;
                }
                if matches!(
                    claim.state,
                    RelationshipState::Validated | RelationshipState::Conflicting
                ) {
                    evidence.push(claim.evidence.clone());
                }
                if let Some(id) = &claim.relationship_id {
                    affected_relationships.push(id.clone());
                }
            }
            topology.diagnostics.push(super::StructuralDiagnostic {
                code: std::borrow::Cow::Borrowed("page_role_conflict"),
                severity: super::DiagnosticSeverity::Error,
                evidence,
                affected_relationships,
                containment: super::Containment::RelationshipExcluded,
            });
        }
    }
    true
}

fn collect_pointer_map_claims(
    geometry: &DatabaseGeometry,
    classifications: &mut super::index::Sequence<PageClassification>,
    topology: &mut super::topology::Topology,
    control: &super::topology::WorkControl,
) -> bool {
    if let Some(maps) = &topology.pointer_map {
        for code in &maps.diagnostics {
            topology.diagnostics.push(super::StructuralDiagnostic {
                code: std::borrow::Cow::Borrowed(code),
                severity: super::DiagnosticSeverity::Error,
                evidence: vec![
                    maps.largest_root.evidence.clone(),
                    maps.incremental_vacuum.evidence.clone(),
                ],
                affected_relationships: vec![],
                containment: super::Containment::RelationshipExcluded,
            });
        }
        for map in &maps.pages {
            if !checkpoint(control) {
                return false;
            }
            for code in &map.diagnostics {
                topology.diagnostics.push(super::StructuralDiagnostic {
                    code: code.clone(),
                    severity: super::DiagnosticSeverity::Error,
                    evidence: vec![evidence(
                        geometry,
                        map.page.page_number,
                        0,
                        u32::try_from(
                            geometry
                                .file_length
                                .saturating_sub(
                                    u64::from(map.page.page_number - 1)
                                        * u64::from(geometry.page_size),
                                )
                                .min(u64::from(geometry.usable_size)),
                        )
                        .expect("page length"),
                        "sqlite_pointer_map_extent",
                    )],
                    affected_relationships: vec![],
                    containment: super::Containment::RelationshipExcluded,
                });
            }
            for entry in &map.entries {
                if !checkpoint(control) {
                    return false;
                }
                let super::EntityIdentity::Page { page_number } = entry.target else {
                    continue;
                };
                for code in &entry.diagnostics {
                    topology.diagnostics.push(super::StructuralDiagnostic {
                        code: code.clone(),
                        severity: super::DiagnosticSeverity::Error,
                        evidence: if *code == "pointer_map_root_above_largest_root" {
                            vec![entry.evidence.clone(), maps.largest_root.evidence.clone()]
                        } else {
                            vec![entry.evidence.clone()]
                        },
                        affected_relationships: vec![],
                        containment: super::Containment::RelationshipExcluded,
                    });
                }
                let Some(mut target) =
                    control.edit_record(classifications, (page_number - 1) as usize)
                else {
                    continue;
                };
                target.referenced = true;
                if let Some(kind) = entry.kind {
                    target.claims.push(RoleClaim {
                        role: kind.role(),
                        source: std::borrow::Cow::Borrowed("pointer_map"),
                        evidence: entry.evidence.clone(),
                        state: entry.state,
                        relationship_id: None,
                        relationship_stop: None,
                    });
                }
            }
        }
    }
    true
}

fn contain_conflicts(
    classifications: &mut super::index::Sequence<PageClassification>,
    topology: &mut super::topology::Topology,
    control: &super::topology::WorkControl,
) -> bool {
    let Some(mut dependencies) = Dependencies::collect(classifications, topology, control) else {
        return false;
    };
    if !dependencies.propagate(classifications, topology, control) {
        return false;
    }
    let mut physical = classifications.sibling();
    for (index, support) in dependencies.support.into_iter().enumerate() {
        let Some(mut classification) = control.edit_record(classifications, index) else {
            return false;
        };
        if !checkpoint(control) {
            return false;
        }
        let role = support.physical();
        physical.push(role == PageRole::Conflicting);
        classification.role = if support.conflicts > 0 {
            PageRole::Conflicting
        } else {
            role
        };
    }
    for index in 0..topology.claims.len() {
        let Some(mut claim) = topology.claims.get_mut(index) else {
            return false;
        };
        if !checkpoint(control) {
            return false;
        }
        if claim.state == RelationshipState::Validated
            && claim
                .target
                .as_ref()
                .and_then(|p| p.page_number.checked_sub(1))
                .and_then(|n| control.read_record(classifications, n as usize))
                .is_some_and(|c| c.role == PageRole::Conflicting)
        {
            claim.state = RelationshipState::Conflicting;
            claim.stop_reason = Some(super::TraversalStopReason::ConflictingClaim);
        }
    }
    normalize_after_conflicts(classifications, &physical, topology, control)
}

const CLAIM_ROLES: [PageRole; 11] = [
    PageRole::Btree,
    PageRole::TableLeaf,
    PageRole::TableInterior,
    PageRole::IndexLeaf,
    PageRole::IndexInterior,
    PageRole::Freelist,
    PageRole::FreelistTrunk,
    PageRole::FreelistLeaf,
    PageRole::Overflow,
    PageRole::PointerMap,
    PageRole::LockByte,
];

#[derive(Clone, Default, Serialize, serde::Deserialize)]
struct RoleSupport {
    counts: [usize; PageRole::Conflicting as usize + 1],
    conflicts: usize,
}
impl super::index::StoredRecord for RoleSupport {}

impl RoleSupport {
    fn physical(&self) -> PageRole {
        CLAIM_ROLES
            .into_iter()
            .filter(|&role| self.counts[role as usize] > 0)
            .fold(PageRole::Unknown, merge)
    }

    fn add(&mut self, claim: &RoleClaim) {
        if credible(claim.state) {
            self.counts[claim.role as usize] += 1;
            self.conflicts += usize::from(
                claim.state == RelationshipState::Conflicting
                    && claim.relationship_stop != Some(super::TraversalStopReason::Cycle),
            );
        }
    }

    fn remove(&mut self, claim: &RoleClaim) {
        self.counts[claim.role as usize] -= 1;
        self.conflicts -= usize::from(
            claim.state == RelationshipState::Conflicting
                && claim.relationship_stop != Some(super::TraversalStopReason::Cycle),
        );
    }
}

fn credible(state: RelationshipState) -> bool {
    matches!(
        state,
        RelationshipState::Validated | RelationshipState::Conflicting
    )
}

#[derive(Clone, Copy, Serialize, serde::Deserialize)]
enum Requirement {
    Header,
    Btree,
    Overflow,
    FreelistTrunk,
}

impl Requirement {
    fn accepts(self, role: PageRole) -> bool {
        match self {
            Self::Header => true,
            Self::Btree => matches!(
                role,
                PageRole::Btree
                    | PageRole::TableLeaf
                    | PageRole::TableInterior
                    | PageRole::IndexLeaf
                    | PageRole::IndexInterior
            ),
            Self::Overflow => role == PageRole::Overflow,
            Self::FreelistTrunk => role == PageRole::FreelistTrunk,
        }
    }
}

#[derive(Clone, Copy, Serialize, serde::Deserialize)]
enum DependentEvidence {
    Relationship(usize),
    PointerMap(usize, usize),
}

#[derive(Clone, Copy, Serialize, serde::Deserialize)]
struct Dependency {
    slot: (usize, usize),
    evidence: DependentEvidence,
    requirement: Requirement,
}

struct Dependencies {
    support: super::index::Sequence<RoleSupport>,
    outgoing: super::index_map::IndexMap<u32, Vec<Dependency>>,
}

impl Dependencies {
    fn collect(
        classifications: &super::index::Sequence<PageClassification>,
        topology: &super::topology::Topology,
        control: &super::topology::WorkControl,
    ) -> Option<Self> {
        use super::{
            EntityIdentity as Entity, PointerMapKind as Map, RelationshipKind as Relation,
        };
        let mut support = classifications.sibling();
        let mut relationship_slots = classifications.map();
        let mut map_slots = classifications.map();
        for (page, classification) in classifications.iter().enumerate() {
            if !checkpoint(control) {
                return None;
            }
            let mut counts = RoleSupport::default();
            for (index, claim) in classification.claims.iter().enumerate() {
                if !checkpoint(control) {
                    return None;
                }
                counts.add(claim);
                if let Some(id) = &claim.relationship_id {
                    relationship_slots.insert(id.clone(), (page, index));
                }
                if claim.source == "pointer_map" {
                    map_slots.insert(
                        (
                            claim.evidence.page.page_number,
                            claim.evidence.range.page_offset,
                        ),
                        (page, index),
                    );
                }
            }
            support.push(counts);
        }
        // Physical source order also stabilizes diagnostics and cancellation prefixes.
        let mut outgoing: super::index_map::IndexMap<u32, Vec<Dependency>> = classifications.map();
        for (index, claim) in topology.claims.iter().enumerate() {
            if !checkpoint(control) {
                return None;
            }
            let Some(slot) = relationship_slots.get(&claim.id) else {
                continue;
            };
            let source = match claim.source {
                Entity::Page { page_number } | Entity::Cell { page_number, .. } => page_number,
            };
            let requirement = match claim.kind {
                Relation::BtreeChild => Requirement::Btree,
                Relation::Overflow if matches!(claim.source, Entity::Cell { .. }) => {
                    Requirement::Btree
                }
                Relation::Overflow => Requirement::Overflow,
                Relation::FreelistTrunk if source == 1 => Requirement::Header,
                Relation::FreelistTrunk | Relation::FreelistLeaf => Requirement::FreelistTrunk,
            };
            let mut dependencies = control.read_index(&outgoing, &source).unwrap_or_default();
            if !checkpoint(control) {
                return None;
            }
            dependencies.push(Dependency {
                slot,
                requirement,
                evidence: DependentEvidence::Relationship(index),
            });
            outgoing.insert(source, dependencies);
        }
        if let Some(maps) = &topology.pointer_map {
            for (map_index, map) in maps.pages.iter().enumerate() {
                if !checkpoint(control) {
                    return None;
                }
                for (index, entry) in map.entries.iter().enumerate() {
                    if !checkpoint(control) {
                        return None;
                    }
                    let Some(slot) =
                        map_slots.get(&(map.page.page_number, entry.evidence.range.page_offset))
                    else {
                        continue;
                    };
                    let requirement = match entry.kind {
                        Some(Map::BtreeChild | Map::FirstOverflow) => Requirement::Btree,
                        Some(Map::Overflow) => Requirement::Overflow,
                        _ => continue,
                    };
                    if let Some(parent) = entry.parent_value {
                        let mut dependencies =
                            control.read_index(&outgoing, &parent).unwrap_or_default();
                        if !checkpoint(control) {
                            return None;
                        }
                        dependencies.push(Dependency {
                            slot,
                            requirement,
                            evidence: DependentEvidence::PointerMap(map_index, index),
                        });
                        outgoing.insert(parent, dependencies);
                    }
                }
            }
        }
        Some(Self { support, outgoing })
    }

    // Support only decreases: enqueue a source again when its physical role
    // changes, not once per lost incoming claim. Each dependency is revoked once.
    fn propagate(
        &mut self,
        classifications: &mut super::index::Sequence<PageClassification>,
        topology: &mut super::topology::Topology,
        control: &super::topology::WorkControl,
    ) -> bool {
        let mut queue = classifications.sibling();
        let mut front = 0;
        let mut queued = classifications.map();
        for source in self.outgoing.keys() {
            if !checkpoint(control) {
                return false;
            }
            queue.push(source);
            queued.insert(source, ());
        }
        while let Some(source) = queue.get(front) {
            front += 1;
            if !checkpoint(control) {
                return false;
            }
            queued.remove(&source);
            let role = source
                .checked_sub(1)
                .and_then(|n| self.support.get(n as usize))
                .map_or(PageRole::Unknown, |support| support.physical());
            for dependency in control
                .read_index(&self.outgoing, &source)
                .unwrap_or_default()
            {
                if !checkpoint(control) {
                    return false;
                }
                let (page, index) = dependency.slot;
                let Some(mut classification) = control.edit_record(classifications, page) else {
                    return false;
                };
                let claim = &mut classification.claims[index];
                if dependency.requirement.accepts(role) || !credible(claim.state) {
                    continue;
                }
                let Some(mut support) = self.support.get_mut(page) else {
                    return false;
                };
                let previous = support.physical();
                support.remove(claim);
                claim.state = RelationshipState::Unresolved;
                claim.relationship_stop = Some(super::TraversalStopReason::ConflictingClaim);
                invalidate_dependency(dependency, topology);
                let target = u32::try_from(page + 1).expect("page inventory index");
                if previous != support.physical()
                    && self.outgoing.contains_key(&target)
                    && queued.insert(target, ()).is_none()
                {
                    queue.push(target);
                }
            }
        }
        true
    }
}

fn invalidate_dependency(dependency: Dependency, topology: &mut super::topology::Topology) {
    match dependency.evidence {
        DependentEvidence::Relationship(index) => {
            let Some(mut claim) = topology.claims.get_mut(index) else {
                return;
            };
            claim.state = RelationshipState::Unresolved;
            claim.stop_reason = Some(super::TraversalStopReason::ConflictingClaim);
        }
        DependentEvidence::PointerMap(map, index) => {
            let Some(mut stored) = topology
                .pointer_map
                .as_mut()
                .expect("map dependency")
                .pages
                .get_mut(map)
            else {
                return;
            };
            let entry = &mut stored.entries[index];
            entry.state = RelationshipState::Unresolved;
            entry
                .diagnostics
                .push("pointer_map_parent_role_conflict".into());
            topology.diagnostics.push(super::StructuralDiagnostic {
                code: std::borrow::Cow::Borrowed("pointer_map_parent_role_conflict"),
                severity: super::DiagnosticSeverity::Error,
                evidence: vec![entry.evidence.clone()],
                affected_relationships: vec![],
                containment: super::Containment::RelationshipExcluded,
            });
        }
    }
}

fn normalize_after_conflicts(
    classifications: &super::index::Sequence<PageClassification>,
    physical: &super::index::Sequence<bool>,
    topology: &mut super::topology::Topology,
    control: &super::topology::WorkControl,
) -> bool {
    let mut valid = classifications.map();
    for claim in &topology.claims {
        if !checkpoint(control) {
            return false;
        }
        if claim.state == RelationshipState::Validated {
            valid.insert(claim.id.clone(), ());
        }
    }
    let empty = topology.relationships.sibling();
    let relationships = std::mem::replace(&mut topology.relationships, empty);
    for relationship in relationships {
        if !checkpoint(control) {
            return false;
        }
        if valid.contains_key(&relationship.claim_id) {
            topology.relationships.push(relationship);
        }
    }
    let empty = topology.traversals.sibling();
    let traversals = std::mem::replace(&mut topology.traversals, empty);
    for mut traversal in traversals {
        if !checkpoint(control) {
            return false;
        }
        if let super::EntityIdentity::Cell {
            page_number,
            cell_index,
        } = traversal.origin
        {
            let id = format!("overflow:cell:{page_number}:{cell_index}");
            if !traversal.validated_prefix.is_empty() && !valid.contains_key(&id) {
                let target = traversal.validated_prefix.first().cloned();
                traversal.validated_prefix.clear();
                traversal.stop = Some(super::TraversalStop {
                    reason: super::TraversalStopReason::ConflictingClaim,
                    claim_id: id,
                    intended_target: target,
                });
            }
        }
        let mut boundary = None;
        for (position, page) in traversal.validated_prefix.iter().enumerate() {
            if !checkpoint(control) {
                return false;
            }
            let index = (page.page_number - 1) as usize;
            let independent_origin = position == 0
                && !physical.get(index).unwrap_or(true)
                && matches!(traversal.origin, super::EntityIdentity::Page { page_number } if page_number == page.page_number);
            let Some(classification) = control.read_record(classifications, index) else {
                return false;
            };
            if classification.role == PageRole::Conflicting && !independent_origin {
                boundary = Some((position, page.clone()));
                break;
            }
        }
        if let Some((position, target)) = boundary {
            let Some(classification) =
                control.read_record(classifications, (target.page_number - 1) as usize)
            else {
                return false;
            };
            let claim_id = classification
                .claims
                .iter()
                .find_map(|c| c.relationship_id.clone())
                .unwrap_or_else(|| format!("role:page:{}", target.page_number));
            traversal.validated_prefix.truncate(position);
            traversal.stop = Some(super::TraversalStop {
                reason: super::TraversalStopReason::ConflictingClaim,
                claim_id,
                intended_target: Some(target),
            });
        }
        topology.traversals.push(traversal);
    }
    true
}
