use super::roles::{evidence, lock_page, map_for};
use super::{DatabaseGeometry, EntityIdentity, PageIdentity, PhysicalEvidence, RelationshipState};
use serde::Serialize;
use std::fs::File;
use std::os::unix::fs::FileExt;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PointerMapEvidence {
    pub applicable: bool,
    pub diagnostics: Vec<&'static str>,
    /// Locations reached by inspection, not an eagerly expanded whole-file schedule.
    pub locations: Vec<PageIdentity>,
    pub layout: Option<PointerMapLayout>,
    pub lock_byte_page: Option<PageIdentity>,
    pub pages: Vec<PointerMapPage>,
    pub complete: bool,
    pub largest_root: super::FreelistField,
    pub incremental_vacuum: super::FreelistField,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PointerMapLayout {
    pub entries_per_page: u32,
    pub stride: u32,
    pub displaced_map: Option<PageIdentity>,
    pub next_regular_after_displaced: Option<PageIdentity>,
}

impl PointerMapEvidence {
    pub(super) fn header(geometry: &DatabaseGeometry) -> Self {
        let mut diagnostics = Vec::new();
        if geometry.largest_root_page > geometry.page_count
            || super::roles::reserved_role(geometry, geometry.largest_root_page).is_some()
        {
            diagnostics.push("pointer_map_invalid_largest_root");
        }
        if geometry.largest_root_page == 0 && geometry.incremental_vacuum != 0 {
            diagnostics.push("pointer_map_incremental_without_auto_vacuum");
        }
        Self {
            diagnostics,
            locations: Vec::new(),
            layout: (geometry.largest_root_page != 0).then(|| {
                let stride = geometry.usable_size / 5 + 1;
                let lock = lock_page(geometry);
                let displaced = lock.checked_add(1).filter(|&page| {
                    page <= geometry.page_count && map_for(geometry, lock) == Some(page)
                });
                PointerMapLayout {
                    entries_per_page: geometry.usable_size / 5,
                    stride,
                    displaced_map: displaced.map(|page_number| PageIdentity { page_number }),
                    next_regular_after_displaced: displaced
                        .and_then(|_| lock.checked_add(stride))
                        .filter(|&n| n <= geometry.page_count)
                        .map(|page_number| PageIdentity { page_number }),
                }
            }),
            lock_byte_page: (lock_page(geometry) <= geometry.page_count).then_some(PageIdentity {
                page_number: lock_page(geometry),
            }),
            applicable: geometry.largest_root_page != 0,
            pages: vec![],
            complete: geometry.largest_root_page == 0,
            largest_root: super::FreelistField {
                value: geometry.largest_root_page,
                evidence: evidence(geometry, 1, 52, 4, "sqlite_header_largest_root"),
            },
            incremental_vacuum: super::FreelistField {
                value: geometry.incremental_vacuum,
                evidence: evidence(geometry, 1, 64, 4, "sqlite_header_incremental_vacuum"),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointerMapKind {
    BtreeRoot,
    Freelist,
    FirstOverflow,
    Overflow,
    BtreeChild,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PointerMapEntry {
    pub target: EntityIdentity,
    pub raw_kind: u8,
    pub kind: Option<PointerMapKind>,
    pub parent_value: Option<u32>,
    pub parent: Option<EntityIdentity>,
    pub evidence: PhysicalEvidence,
    pub state: RelationshipState,
    pub diagnostics: Vec<std::borrow::Cow<'static, str>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PointerMapPage {
    pub page: PageIdentity,
    pub entries: Vec<PointerMapEntry>,
    pub complete: bool,
    pub diagnostics: Vec<std::borrow::Cow<'static, str>>,
}

pub(super) struct StoredPointerMaps {
    pub header: PointerMapEvidence,
    pub pages: super::index::Sequence<PointerMapPage>,
}
impl std::ops::Deref for StoredPointerMaps {
    type Target = PointerMapEvidence;
    fn deref(&self) -> &Self::Target {
        &self.header
    }
}
impl std::ops::DerefMut for StoredPointerMaps {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.header
    }
}
impl StoredPointerMaps {
    pub(super) fn materialize(&self) -> Result<PointerMapEvidence, super::storage::StorageError> {
        let mut evidence = self.header.clone();
        evidence.pages = self.pages.materialize()?;
        evidence.locations = evidence
            .pages
            .iter()
            .map(|page| page.page.clone())
            .collect();
        Ok(evidence)
    }
}

pub(super) fn inspect(
    file: &File,
    geometry: &DatabaseGeometry,
    pages: &super::storage::PageStore,
    control: &super::topology::WorkControl,
) -> StoredPointerMaps {
    let mut result = StoredPointerMaps {
        header: PointerMapEvidence::header(geometry),
        pages: super::index::Sequence::new(std::sync::Arc::clone(&pages.context)),
    };
    if !result.applicable {
        return result;
    }
    control.begin_phase(
        super::TopologyPhase::PointerMapInspection,
        Some(pages.len() as u64),
    );
    let mut complete = true;
    for number in (1..=geometry.page_count).take(pages.len()) {
        if control.traversal_reason().is_some() {
            return result;
        }
        if map_for(geometry, number) != Some(number) {
            control.advance(super::TopologyPhase::PointerMapInspection);
            continue;
        }
        let map = inspect_page(file, geometry, number, control);
        complete &= map.complete;
        result.pages.push(map);
        control.advance(super::TopologyPhase::PointerMapInspection);
    }
    if control.traversal_reason().is_none() {
        control.finish_phase(super::TopologyPhase::PointerMapInspection);
    }
    result.complete = control.traversal_reason().is_none()
        && pages.len() == geometry.page_count as usize
        && complete;
    result
}

fn inspect_page(
    file: &File,
    geometry: &DatabaseGeometry,
    number: u32,
    control: &super::topology::WorkControl,
) -> PointerMapPage {
    let mut map = PointerMapPage {
        page: PageIdentity {
            page_number: number,
        },
        entries: vec![],
        complete: true,
        diagnostics: vec![],
    };
    let map_end = u64::from(number) * u64::from(geometry.page_size);
    let truncated = geometry.file_length < map_end;
    if truncated {
        map.complete = false;
        map.diagnostics.push("pointer_map_truncated".into());
    }
    let target_limit = if truncated {
        geometry.page_count.max(geometry.declared_page_count)
    } else {
        geometry.page_count
    };
    // Account for entry evidence, diagnostics, growth and the encoded spill
    // record before building the page's entry vector.
    let entries = (geometry.usable_size / 5).min(target_limit.saturating_sub(number));
    if !control.reserve_memory(u64::from(entries).saturating_mul(2048)) {
        map.complete = false;
        return map;
    }
    for index in 0..geometry.usable_size / 5 {
        let Some(target) = number.checked_add(index).and_then(|n| n.checked_add(1)) else {
            break;
        };
        if target > target_limit || map_for(geometry, target) != Some(number) {
            break;
        }
        if target == lock_page(geometry) {
            continue;
        }
        if control.traversal_reason().is_some() {
            map.complete = false;
            break;
        }
        let mut locus = evidence(geometry, number, index * 5, 5, "sqlite_pointer_map_entry");
        let mut bytes = [0; 5];
        let available = usize::try_from(
            geometry
                .file_length
                .saturating_sub(locus.range.file_offset)
                .min(5),
        )
        .expect("entry-bounded length");
        if available == 0 {
            map.complete = false;
            break;
        }
        if file
            .read_exact_at(&mut bytes[..available], locus.range.file_offset)
            .is_err()
        {
            map.complete = false;
            map.diagnostics.push("pointer_map_read_failed".into());
            break;
        }
        locus.range.length = u32::try_from(available).expect("entry length");
        let kind = match bytes[0] {
            1 => Some(PointerMapKind::BtreeRoot),
            2 => Some(PointerMapKind::Freelist),
            3 => Some(PointerMapKind::FirstOverflow),
            4 => Some(PointerMapKind::Overflow),
            5 => Some(PointerMapKind::BtreeChild),
            _ => None,
        };
        let parent = (available == 5)
            .then(|| u32::from_be_bytes(bytes[1..].try_into().expect("four bytes")));
        map.entries.push(PointerMapEntry {
            target: EntityIdentity::Page {
                page_number: target,
            },
            raw_kind: bytes[0],
            kind,
            parent_value: parent,
            parent: parent
                .filter(|p| *p != 0)
                .map(|page_number| EntityIdentity::Page { page_number }),
            evidence: locus,
            state: RelationshipState::Unresolved,
            diagnostics: vec![],
        });
    }
    map
}

impl PointerMapKind {
    pub(super) fn role(self) -> super::PageRole {
        match self {
            Self::BtreeRoot | Self::BtreeChild => super::PageRole::Btree,
            Self::Freelist => super::PageRole::Freelist,
            Self::FirstOverflow | Self::Overflow => super::PageRole::Overflow,
        }
    }
}

pub(super) fn validate(
    geometry: &DatabaseGeometry,
    maps: &mut StoredPointerMaps,
    control: &super::topology::WorkControl,
) -> bool {
    control.begin_phase(super::TopologyPhase::PointerMapValidation, None);
    for index in 0..maps.pages.len() {
        let Some(mut map) = control.edit_record(&mut maps.pages, index) else {
            return false;
        };
        for entry in &mut map.entries {
            if control.traversal_reason().is_some() {
                return false;
            }
            control.advance(super::TopologyPhase::PointerMapValidation);
            let Some(kind) = entry.kind else {
                entry.state = RelationshipState::Invalid;
                entry.diagnostics.push("pointer_map_invalid_kind".into());
                continue;
            };
            let Some(parent) = entry.parent_value else {
                continue;
            };
            let target = match entry.target {
                EntityIdentity::Page { page_number } => page_number,
                EntityIdentity::Cell { .. } => unreachable!("map targets are pages"),
            };
            if target > geometry.page_count {
                entry.state = RelationshipState::Unresolved;
                entry
                    .diagnostics
                    .push("pointer_map_target_out_of_range".into());
                continue;
            }
            let parent_valid =
                if matches!(kind, PointerMapKind::BtreeRoot | PointerMapKind::Freelist) {
                    parent == 0
                } else {
                    parent > 0
                        && parent <= geometry.page_count
                        && parent != target
                        && super::roles::reserved_role(geometry, parent).is_none()
                };
            if parent_valid {
                entry.state = RelationshipState::Validated;
                if kind == PointerMapKind::BtreeRoot && target > geometry.largest_root_page {
                    entry.state = RelationshipState::Conflicting;
                    entry
                        .diagnostics
                        .push("pointer_map_root_above_largest_root".into());
                }
            } else {
                entry.state = RelationshipState::Invalid;
                entry.diagnostics.push("pointer_map_invalid_parent".into());
            }
        }
    }
    if control.traversal_reason().is_some() {
        return false;
    }
    control.finish_phase(super::TopologyPhase::PointerMapValidation);
    true
}

pub(super) fn reconcile(
    maps: &mut StoredPointerMaps,
    topology: &mut super::topology::Topology,
    inventory_complete: bool,
    control: &super::topology::WorkControl,
) -> bool {
    control.begin_phase(super::TopologyPhase::PointerMapReconciliation, None);
    let Some(incoming) = super::topology::index_claim_targets(
        &topology.claims,
        control,
        super::TopologyPhase::PointerMapReconciliation,
    ) else {
        return false;
    };
    for index in 0..maps.pages.len() {
        let Some(mut map) = control.edit_record(&mut maps.pages, index) else {
            return false;
        };
        for entry in &mut map.entries {
            if !reconcile_entry(entry, topology, &incoming, inventory_complete, control) {
                return false;
            }
        }
    }
    if control.traversal_reason().is_some() {
        return false;
    }
    control.finish_phase(super::TopologyPhase::PointerMapReconciliation);
    true
}

fn reconcile_entry(
    entry: &mut PointerMapEntry,
    topology: &mut super::topology::Topology,
    incoming: &super::index_map::IndexMap<u32, Vec<usize>>,
    inventory_complete: bool,
    control: &super::topology::WorkControl,
) -> bool {
    use super::{
        EntityIdentity as Entity, RelationshipKind as Relation, RelationshipState as State,
    };
    if control.traversal_reason().is_some() {
        return false;
    }
    control.advance(super::TopologyPhase::PointerMapReconciliation);
    if entry.state != State::Validated {
        return true;
    }
    let Entity::Page {
        page_number: target,
    } = entry.target
    else {
        return true;
    };
    let indexes = &control.read_index(incoming, &target).unwrap_or_default();
    if control.traversal_reason().is_some() {
        return false;
    }
    let kind = entry.kind.expect("validated kind");
    let agrees = |claim: &super::RelationshipClaim| {
        let source_page = match claim.source {
            Entity::Page { page_number } | Entity::Cell { page_number, .. } => page_number,
        };
        match kind {
            PointerMapKind::BtreeRoot => false,
            PointerMapKind::Freelist => {
                matches!(claim.kind, Relation::FreelistTrunk | Relation::FreelistLeaf)
            }
            PointerMapKind::BtreeChild => {
                claim.kind == Relation::BtreeChild && Some(source_page) == entry.parent_value
            }
            PointerMapKind::FirstOverflow => {
                claim.kind == Relation::Overflow
                    && matches!(claim.source, Entity::Cell { .. })
                    && Some(source_page) == entry.parent_value
            }
            PointerMapKind::Overflow => {
                claim.kind == Relation::Overflow
                    && matches!(claim.source, Entity::Page { .. })
                    && Some(source_page) == entry.parent_value
            }
        }
    };
    let mut contradicted = false;
    let mut confirmed = false;
    for &index in indexes {
        if control.traversal_reason().is_some() {
            return false;
        }
        control.advance(super::TopologyPhase::PointerMapReconciliation);
        let Some(claim) = topology.claims.get(index) else {
            return false;
        };
        contradicted |=
            matches!(claim.state, State::Validated | State::Conflicting) && !agrees(&claim);
        confirmed |= claim.state == State::Validated && agrees(&claim);
    }
    let root = kind == PointerMapKind::BtreeRoot;
    let fully_evaluated = inventory_complete
        && topology.coverage.reason == super::TopologyCoverageReason::Complete
        && topology.freelist.coverage.reason == super::FreelistCoverageReason::Complete;
    if contradicted || (!root && !confirmed && fully_evaluated) {
        entry.state = State::Conflicting;
        entry.diagnostics.push(if contradicted {
            "pointer_map_parent_mismatch".into()
        } else {
            "pointer_map_missing_forward_link".into()
        });
        for &index in indexes {
            if control.traversal_reason().is_some() {
                return false;
            }
            control.advance(super::TopologyPhase::PointerMapReconciliation);
            let Some(mut claim) = topology.claims.get_mut(index) else {
                return false;
            };
            if matches!(claim.state, State::Validated | State::Conflicting) {
                claim.state = State::Conflicting;
                claim.stop_reason = Some(super::TraversalStopReason::ConflictingClaim);
            }
        }
    } else if !root && !confirmed {
        entry.state = State::Unresolved;
    }
    true
}
