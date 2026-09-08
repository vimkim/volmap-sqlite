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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PointerMapKind {
    BtreeRoot,
    Freelist,
    FirstOverflow,
    Overflow,
    BtreeChild,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PointerMapEntry {
    pub target: EntityIdentity,
    pub raw_kind: u8,
    pub kind: Option<PointerMapKind>,
    pub parent_value: Option<u32>,
    pub parent: Option<EntityIdentity>,
    pub evidence: PhysicalEvidence,
    pub state: RelationshipState,
    pub diagnostics: Vec<&'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PointerMapPage {
    pub page: PageIdentity,
    pub entries: Vec<PointerMapEntry>,
    pub complete: bool,
    pub diagnostics: Vec<&'static str>,
}

pub(super) fn inspect(
    file: &File,
    geometry: &DatabaseGeometry,
    pages: &[super::PageEntity],
    control: &super::topology::WorkControl,
) -> PointerMapEvidence {
    let mut result = PointerMapEvidence::header(geometry);
    if !result.applicable {
        return result;
    }
    control.begin_phase(
        super::TopologyPhase::PointerMapInspection,
        Some(pages.len() as u64),
    );
    for page in pages {
        if control.traversal_reason().is_some() {
            return result;
        }
        if map_for(geometry, page.number) != Some(page.number) {
            control.advance(super::TopologyPhase::PointerMapInspection);
            continue;
        }
        result.locations.push(PageIdentity {
            page_number: page.number,
        });
        result
            .pages
            .push(inspect_page(file, geometry, page.number, control));
        control.advance(super::TopologyPhase::PointerMapInspection);
    }
    if control.traversal_reason().is_none() {
        control.finish_phase(super::TopologyPhase::PointerMapInspection);
    }
    result.complete = control.traversal_reason().is_none()
        && pages.len() == geometry.page_count as usize
        && result.pages.iter().all(|page| page.complete);
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
        map.diagnostics.push("pointer_map_truncated");
    }
    let target_limit = if truncated {
        geometry.page_count.max(geometry.declared_page_count)
    } else {
        geometry.page_count
    };
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
            map.diagnostics.push("pointer_map_read_failed");
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
    maps: &mut PointerMapEvidence,
    control: &super::topology::WorkControl,
) -> bool {
    control.begin_phase(super::TopologyPhase::PointerMapValidation, None);
    for map in &mut maps.pages {
        for entry in &mut map.entries {
            if control.traversal_reason().is_some() {
                return false;
            }
            control.advance(super::TopologyPhase::PointerMapValidation);
            let Some(kind) = entry.kind else {
                entry.state = RelationshipState::Invalid;
                entry.diagnostics.push("pointer_map_invalid_kind");
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
                entry.diagnostics.push("pointer_map_target_out_of_range");
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
                        .push("pointer_map_root_above_largest_root");
                }
            } else {
                entry.state = RelationshipState::Invalid;
                entry.diagnostics.push("pointer_map_invalid_parent");
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
    maps: &mut PointerMapEvidence,
    topology: &mut super::topology::Topology,
    inventory_complete: bool,
    control: &super::topology::WorkControl,
) -> bool {
    use super::{
        EntityIdentity as Entity, RelationshipKind as Relation, RelationshipState as State,
    };
    control.begin_phase(super::TopologyPhase::PointerMapReconciliation, None);
    let mut incoming = std::collections::HashMap::<u32, Vec<usize>>::new();
    for (index, claim) in topology.claims.iter().enumerate() {
        if control.traversal_reason().is_some() {
            return false;
        }
        control.advance(super::TopologyPhase::PointerMapReconciliation);
        if let Some(target) = &claim.target {
            incoming.entry(target.page_number).or_default().push(index);
        }
    }
    for entry in maps.pages.iter_mut().flat_map(|map| &mut map.entries) {
        if control.traversal_reason().is_some() {
            return false;
        }
        control.advance(super::TopologyPhase::PointerMapReconciliation);
        if entry.state != State::Validated {
            continue;
        }
        let Entity::Page {
            page_number: target,
        } = entry.target
        else {
            continue;
        };
        let indexes = incoming.get(&target).map_or(&[][..], Vec::as_slice);
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
            let claim = &topology.claims[index];
            contradicted |=
                matches!(claim.state, State::Validated | State::Conflicting) && !agrees(claim);
            confirmed |= claim.state == State::Validated && agrees(claim);
        }
        let root = kind == PointerMapKind::BtreeRoot;
        let fully_evaluated = inventory_complete
            && topology.coverage.reason == super::TopologyCoverageReason::Complete
            && topology.freelist.coverage.reason == super::FreelistCoverageReason::Complete;
        if contradicted || (!root && !confirmed && fully_evaluated) {
            entry.state = State::Conflicting;
            entry.diagnostics.push(if contradicted {
                "pointer_map_parent_mismatch"
            } else {
                "pointer_map_missing_forward_link"
            });
            for &index in indexes {
                if control.traversal_reason().is_some() {
                    return false;
                }
                control.advance(super::TopologyPhase::PointerMapReconciliation);
                let claim = &mut topology.claims[index];
                if matches!(claim.state, State::Validated | State::Conflicting) {
                    claim.state = State::Conflicting;
                    claim.stop_reason = Some(super::TraversalStopReason::ConflictingClaim);
                }
            }
        } else if !root && !confirmed {
            entry.state = State::Unresolved;
        }
    }
    if control.traversal_reason().is_some() {
        return false;
    }
    control.finish_phase(super::TopologyPhase::PointerMapReconciliation);
    true
}
