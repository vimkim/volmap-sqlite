use std::fs::File;
use std::io::Write;
use std::os::unix::fs::FileExt;
use std::path::Path;

use serde_json::{Value, json};
use tempfile::tempdir;
use volmap_sqlite::inspection::{
    InspectionSession, ScanControl, TopologyCoverage, TopologyCoverageReason, TopologyPhase,
    TraversalBudget,
};

const PAGE_SIZE: usize = 512;

fn database_header(bytes: &mut [u8], page_count: u32) {
    bytes[..16].copy_from_slice(b"SQLite format 3\0");
    bytes[16..18].copy_from_slice(&u16::try_from(PAGE_SIZE).unwrap().to_be_bytes());
    bytes[18] = 1;
    bytes[19] = 1;
    bytes[21] = 64;
    bytes[22] = 32;
    bytes[23] = 32;
    bytes[24..28].copy_from_slice(&1_u32.to_be_bytes());
    bytes[28..32].copy_from_slice(&page_count.to_be_bytes());
    bytes[44..48].copy_from_slice(&4_u32.to_be_bytes());
    bytes[56..60].copy_from_slice(&1_u32.to_be_bytes());
    bytes[92..96].copy_from_slice(&1_u32.to_be_bytes());
}

fn empty_table_leaf(bytes: &mut [u8], page_number: u32) {
    let base = (page_number as usize - 1) * PAGE_SIZE;
    bytes[base] = 13;
    bytes[base + 5..base + 7].copy_from_slice(&u16::try_from(PAGE_SIZE).unwrap().to_be_bytes());
}

fn table_interior(bytes: &mut [u8], page_number: u32, left: u32, right: u32) {
    let page = (page_number as usize - 1) * PAGE_SIZE;
    let header = page + usize::from(page_number == 1) * 100;
    let cell = page + 507;
    bytes[header] = 5;
    bytes[header + 3..header + 5].copy_from_slice(&1_u16.to_be_bytes());
    bytes[header + 5..header + 7].copy_from_slice(&507_u16.to_be_bytes());
    bytes[header + 8..header + 12].copy_from_slice(&right.to_be_bytes());
    bytes[header + 12..header + 14].copy_from_slice(&507_u16.to_be_bytes());
    bytes[cell..cell + 4].copy_from_slice(&left.to_be_bytes());
    bytes[cell + 4] = 1;
}

fn valid_table_btree() -> Vec<u8> {
    let mut bytes = vec![0; PAGE_SIZE * 3];
    database_header(&mut bytes, 3);

    // Page 1 is an interior table page with one left child and a right-most child.
    bytes[100] = 5;
    bytes[103..105].copy_from_slice(&1_u16.to_be_bytes());
    bytes[105..107].copy_from_slice(&507_u16.to_be_bytes());
    bytes[108..112].copy_from_slice(&3_u32.to_be_bytes());
    bytes[112..114].copy_from_slice(&507_u16.to_be_bytes());
    bytes[507..511].copy_from_slice(&2_u32.to_be_bytes());
    bytes[511] = 1;

    empty_table_leaf(&mut bytes, 2);
    empty_table_leaf(&mut bytes, 3);
    bytes
}

fn valid_overflow_chain() -> Vec<u8> {
    let mut bytes = vec![0; PAGE_SIZE * 3];
    database_header(&mut bytes, 3);
    bytes[100] = 13;
    bytes[103..105].copy_from_slice(&1_u16.to_be_bytes());
    bytes[105..107].copy_from_slice(&466_u16.to_be_bytes());
    bytes[108..110].copy_from_slice(&466_u16.to_be_bytes());
    bytes[466..469].copy_from_slice(&[0x87, 0x68, 0x01]); // 1000-byte payload, rowid 1
    bytes[508..512].copy_from_slice(&2_u32.to_be_bytes());
    bytes[512..516].copy_from_slice(&3_u32.to_be_bytes());
    bytes[1024..1028].copy_from_slice(&0_u32.to_be_bytes());
    bytes
}

fn duplicate_overflow_roots() -> Vec<u8> {
    let mut bytes = valid_overflow_chain();
    bytes[103..105].copy_from_slice(&2_u16.to_be_bytes());
    bytes[105..107].copy_from_slice(&420_u16.to_be_bytes());
    bytes[108..110].copy_from_slice(&420_u16.to_be_bytes());
    bytes[110..112].copy_from_slice(&466_u16.to_be_bytes());
    // This owner needs one overflow page, while cell 1 needs two. The shared
    // physical next-pointer claim must not inherit either owner's chain length.
    bytes[420..423].copy_from_slice(&[0x83, 0x74, 0x01]);
    bytes[462..466].copy_from_slice(&2_u32.to_be_bytes());
    bytes
}

fn large_table_btree(interior_pages: u32) -> Vec<u8> {
    let page_count = interior_pages * 2 + 1;
    let mut bytes = vec![0; PAGE_SIZE * page_count as usize];
    database_header(&mut bytes, page_count);
    for page in 1..=interior_pages {
        let left = if page < interior_pages {
            page + 1
        } else {
            page_count
        };
        table_interior(&mut bytes, page, left, interior_pages + page);
    }
    for page in interior_pages + 1..=page_count {
        empty_table_leaf(&mut bytes, page);
    }
    bytes
}

fn inspect(path: &Path, bytes: &[u8]) -> Value {
    File::create(path).unwrap().write_all(bytes).unwrap();
    let session = InspectionSession::open(path).unwrap();
    serde_json::to_value(&*session.graph().unwrap()).unwrap()
}

fn assert_exact_topology_stop(coverage: TopologyCoverage, reason: TopologyCoverageReason) {
    assert_eq!(coverage.reason, reason);
    assert!(
        coverage.phase == TopologyPhase::Complete
            || coverage.next.is_some()
            || coverage.next_phase.is_some()
    );
    assert_eq!(
        coverage.remainder,
        coverage
            .total
            .map(|total| total.saturating_sub(coverage.evaluated))
    );
    assert_eq!(
        coverage.next,
        match (coverage.next_phase, coverage.total) {
            (Some(_), _) => None,
            (None, _) if coverage.phase == TopologyPhase::Complete => None,
            (None, Some(total)) if coverage.evaluated >= total => None,
            (None, _) => Some(coverage.evaluated + 1),
        }
    );
}

#[test]
fn aggregate_traversal_budget_serializes_without_javascript_precision_loss() {
    let budget = TraversalBudget::with_total_pages(u32::MAX, u32::MAX, u64::MAX);

    assert_eq!(
        serde_json::to_value(budget).unwrap()["maxTotalPages"],
        u64::MAX.to_string()
    );
}

#[test]
fn traversal_budget_rejects_zero_for_limits_without_a_representable_boundary() {
    assert!(std::panic::catch_unwind(|| TraversalBudget::new(0, 1)).is_err());
    assert!(std::panic::catch_unwind(|| TraversalBudget::with_total_pages(1, 1, 0)).is_err());
}

#[test]
fn valid_btree_claims_resolve_with_bidirectional_navigation_and_prefixes() {
    let directory = tempdir().unwrap();
    let graph = inspect(&directory.path().join("tree.sqlite"), &valid_table_btree());

    assert_eq!(
        graph["relationshipClaims"],
        json!([
            {
                "id": "btree:page:1:cell:0",
                "kind": "btree_child",
                "source": { "type": "cell", "pageNumber": 1, "cellIndex": 0 },
                "target": { "pageNumber": 2 },
                "evidence": {
                    "page": { "pageNumber": 1 },
                    "range": { "pageOffset": 507, "fileOffset": 507, "length": 4 },
                    "validationRule": "sqlite_btree_interior_left_child"
                },
                "state": "validated"
            },
            {
                "id": "btree:page:1:rightmost",
                "kind": "btree_child",
                "source": { "type": "page", "pageNumber": 1 },
                "target": { "pageNumber": 3 },
                "evidence": {
                    "page": { "pageNumber": 1 },
                    "range": { "pageOffset": 108, "fileOffset": 108, "length": 4 },
                    "validationRule": "sqlite_btree_interior_rightmost_child"
                },
                "state": "validated"
            }
        ])
    );
    assert_eq!(
        graph["relationships"],
        json!([
            {
                "claimId": "btree:page:1:cell:0",
                "kind": "btree_child",
                "source": { "type": "cell", "pageNumber": 1, "cellIndex": 0 },
                "target": { "pageNumber": 2 }
            },
            {
                "claimId": "btree:page:1:rightmost",
                "kind": "btree_child",
                "source": { "type": "page", "pageNumber": 1 },
                "target": { "pageNumber": 3 }
            }
        ])
    );
    assert_eq!(
        graph["traversals"],
        json!([
            {
                "kind": "btree",
                "origin": { "type": "page", "pageNumber": 1 },
                "validatedPrefix": [{ "pageNumber": 1 }, { "pageNumber": 2 }],
                "stop": null
            },
            {
                "kind": "btree",
                "origin": { "type": "page", "pageNumber": 1 },
                "validatedPrefix": [{ "pageNumber": 1 }, { "pageNumber": 3 }],
                "stop": null
            }
        ])
    );
    assert_eq!(graph["diagnostics"], json!([]));
    assert_eq!(graph["topologyCoverage"]["reason"], "complete");
    assert_eq!(graph["topologyCoverage"]["phase"], "complete");
    assert_eq!(graph["topologyCoverage"]["evaluated"], 0);
    assert_eq!(graph["topologyCoverage"]["total"], 0);
    assert_eq!(graph["topologyCoverage"]["next"], Value::Null);
    assert_eq!(graph["topologyCoverage"]["remainder"], 0);
    assert_eq!(graph["topologyCoverage"]["nextPhase"], Value::Null);
    assert_eq!(
        graph["topologyCoverage"]["traversalBudget"],
        json!({
            "maxBtreePages": u32::MAX,
            "maxOverflowPages": u32::MAX,
            "maxTotalPages": "1000000"
        })
    );
}

#[test]
fn an_out_of_range_child_stops_only_its_path_and_preserves_the_claim() {
    let directory = tempdir().unwrap();
    let mut bytes = valid_table_btree();
    bytes[507..511].copy_from_slice(&9_u32.to_be_bytes());
    let graph = inspect(&directory.path().join("broken.sqlite"), &bytes);

    assert_eq!(graph["relationshipClaims"][0]["target"]["pageNumber"], 9);
    assert_eq!(
        graph["relationshipClaims"][0]["evidence"]["range"],
        json!({ "pageOffset": 507, "fileOffset": 507, "length": 4 })
    );
    assert_eq!(graph["relationshipClaims"][0]["state"], "unresolved");
    assert_eq!(graph["relationshipClaims"][1]["state"], "validated");
    assert_eq!(
        graph["relationships"],
        json!([{
            "claimId": "btree:page:1:rightmost",
            "kind": "btree_child",
            "source": { "type": "page", "pageNumber": 1 },
            "target": { "pageNumber": 3 }
        }])
    );
    assert_eq!(
        graph["traversals"][0],
        json!({
            "kind": "btree",
            "origin": { "type": "page", "pageNumber": 1 },
            "validatedPrefix": [{ "pageNumber": 1 }],
            "stop": {
            "reason": "out_of_range",
                "claimId": "btree:page:1:cell:0",
                "intendedTarget": { "pageNumber": 9 }
            }
        })
    );
    assert_eq!(
        graph["traversals"][1]["validatedPrefix"],
        json!([{ "pageNumber": 1 }, { "pageNumber": 3 }])
    );
    assert_eq!(
        graph["diagnostics"],
        json!([{
            "code": "btree_child_out_of_range",
            "severity": "error",
            "evidence": [{
                "page": { "pageNumber": 1 },
                "range": { "pageOffset": 507, "fileOffset": 507, "length": 4 },
                "validationRule": "sqlite_btree_interior_left_child"
            }],
            "affectedRelationships": ["btree:page:1:cell:0"],
            "containment": "traversal_stopped"
        }])
    );
    assert_eq!(graph["pages"][2]["detail"]["coverage"], "complete");
}

#[test]
fn type_mismatch_excludes_only_the_incompatible_relationship() {
    let directory = tempdir().unwrap();
    let mut bytes = valid_table_btree();
    bytes[PAGE_SIZE] = 10;
    let graph = inspect(&directory.path().join("mismatch.sqlite"), &bytes);

    assert_eq!(graph["relationshipClaims"][0]["state"], "invalid");
    assert_eq!(graph["relationshipClaims"][1]["state"], "validated");
    assert_eq!(graph["traversals"][0]["stop"]["reason"], "type_mismatch");
    assert_eq!(graph["diagnostics"][0]["code"], "btree_child_type_mismatch");
    assert_eq!(graph["diagnostics"][0]["severity"], "error");
    assert_eq!(
        graph["diagnostics"][0]["affectedRelationships"],
        json!(["btree:page:1:cell:0"])
    );
    assert_eq!(graph["diagnostics"][0]["containment"], "traversal_stopped");
    assert_eq!(graph["pages"][2]["detail"]["coverage"], "complete");
}

#[test]
fn traversal_prefixes_span_multiple_interior_levels() {
    let directory = tempdir().unwrap();
    let mut bytes = vec![0; PAGE_SIZE * 5];
    database_header(&mut bytes, 5);
    table_interior(&mut bytes, 1, 2, 5);
    table_interior(&mut bytes, 2, 3, 4);
    empty_table_leaf(&mut bytes, 3);
    empty_table_leaf(&mut bytes, 4);
    empty_table_leaf(&mut bytes, 5);
    let graph = inspect(&directory.path().join("deep.sqlite"), &bytes);

    assert_eq!(
        graph["traversals"],
        json!([
            {
                "kind": "btree",
                "origin": { "type": "page", "pageNumber": 1 },
                "validatedPrefix": [
                    { "pageNumber": 1 },
                    { "pageNumber": 2 },
                    { "pageNumber": 3 }
                ],
                "stop": null
            },
            {
                "kind": "btree",
                "origin": { "type": "page", "pageNumber": 1 },
                "validatedPrefix": [
                    { "pageNumber": 1 },
                    { "pageNumber": 2 },
                    { "pageNumber": 4 }
                ],
                "stop": null
            },
            {
                "kind": "btree",
                "origin": { "type": "page", "pageNumber": 1 },
                "validatedPrefix": [{ "pageNumber": 1 }, { "pageNumber": 5 }],
                "stop": null
            }
        ])
    );
}

#[test]
fn duplicate_parents_are_conflicts_and_the_target_remains_independently_visible() {
    let directory = tempdir().unwrap();
    let mut bytes = valid_table_btree();
    bytes[108..112].copy_from_slice(&2_u32.to_be_bytes());
    let graph = inspect(&directory.path().join("duplicate.sqlite"), &bytes);

    assert_eq!(graph["relationshipClaims"][0]["state"], "conflicting");
    assert_eq!(graph["relationshipClaims"][1]["state"], "conflicting");
    assert_eq!(graph["relationships"], json!([]));
    assert_eq!(graph["diagnostics"][0]["code"], "btree_duplicate_parent");
    assert_eq!(
        graph["diagnostics"][0]["affectedRelationships"],
        json!(["btree:page:1:cell:0", "btree:page:1:rightmost"])
    );
    assert!(
        graph["traversals"]
            .as_array()
            .unwrap()
            .iter()
            .any(|traversal| {
                traversal["validatedPrefix"] == json!([{ "pageNumber": 2 }])
                    && traversal["stop"].is_null()
            })
    );
}

#[test]
fn cycles_are_diagnostic_stops_without_hiding_valid_sibling_paths() {
    let directory = tempdir().unwrap();
    let mut bytes = vec![0; PAGE_SIZE * 4];
    database_header(&mut bytes, 4);
    table_interior(&mut bytes, 1, 2, 3);
    table_interior(&mut bytes, 2, 1, 4);
    empty_table_leaf(&mut bytes, 3);
    empty_table_leaf(&mut bytes, 4);
    let graph = inspect(&directory.path().join("cycle.sqlite"), &bytes);

    assert_eq!(graph["diagnostics"][0]["code"], "btree_cycle");
    assert_eq!(
        graph["diagnostics"][0]["affectedRelationships"],
        json!(["btree:page:1:cell:0", "btree:page:2:cell:0"])
    );
    assert!(
        graph["traversals"]
            .as_array()
            .unwrap()
            .iter()
            .any(|traversal| {
                traversal["validatedPrefix"] == json!([{ "pageNumber": 1 }])
                    && traversal["stop"]["reason"] == "cycle"
            })
    );
    assert!(
        graph["traversals"]
            .as_array()
            .unwrap()
            .iter()
            .any(|traversal| {
                traversal["validatedPrefix"] == json!([{ "pageNumber": 1 }, { "pageNumber": 3 }])
                    && traversal["stop"].is_null()
            })
    );
    assert!(
        graph["traversals"]
            .as_array()
            .unwrap()
            .iter()
            .any(|traversal| {
                traversal["validatedPrefix"] == json!([{ "pageNumber": 2 }, { "pageNumber": 4 }])
                    && traversal["stop"].is_null()
            })
    );
}

#[test]
fn overlapping_cells_retain_untrusted_claim_evidence_without_authorizing_links() {
    let directory = tempdir().unwrap();
    let mut bytes = valid_table_btree();
    bytes[103..105].copy_from_slice(&2_u16.to_be_bytes());
    bytes[114..116].copy_from_slice(&507_u16.to_be_bytes());
    let graph = inspect(&directory.path().join("overlap.sqlite"), &bytes);

    assert_eq!(graph["relationshipClaims"][0]["state"], "invalid");
    assert_eq!(graph["relationshipClaims"][1]["state"], "invalid");
    assert_eq!(graph["relationshipClaims"][2]["state"], "validated");
    assert_eq!(
        graph["relationshipClaims"][0]["evidence"]["range"],
        json!({ "pageOffset": 507, "fileOffset": 507, "length": 4 })
    );
    assert_eq!(
        graph["diagnostics"][0]["code"],
        "btree_child_overlapping_source"
    );
    assert_eq!(
        graph["traversals"][0]["stop"]["reason"],
        "overlapping_extent"
    );
    assert_eq!(graph["pages"][1]["detail"]["coverage"], "complete");
}

#[test]
fn overflow_claims_trace_only_linkage_bytes_and_publish_the_validated_prefix() {
    let directory = tempdir().unwrap();
    let graph = inspect(
        &directory.path().join("overflow.sqlite"),
        &valid_overflow_chain(),
    );
    assert_eq!(
        graph["relationshipClaims"],
        json!([
            {
                "id": "overflow:cell:1:0",
                "kind": "overflow",
                "source": { "type": "cell", "pageNumber": 1, "cellIndex": 0 },
                "target": { "pageNumber": 2 },
                "evidence": {
                    "page": { "pageNumber": 1 },
                    "range": { "pageOffset": 508, "fileOffset": 508, "length": 4 },
                    "validationRule": "sqlite_btree_first_overflow_page"
                },
                "state": "validated"
            },
            {
                "id": "overflow:page:2",
                "kind": "overflow",
                "source": { "type": "page", "pageNumber": 2 },
                "target": { "pageNumber": 3 },
                "evidence": {
                    "page": { "pageNumber": 2 },
                    "range": { "pageOffset": 0, "fileOffset": 512, "length": 4 },
                    "validationRule": "sqlite_overflow_next_page"
                },
                "state": "validated"
            },
            {
                "id": "overflow:page:3",
                "kind": "overflow",
                "source": { "type": "page", "pageNumber": 3 },
                "target": null,
                "evidence": {
                    "page": { "pageNumber": 3 },
                    "range": { "pageOffset": 0, "fileOffset": 1024, "length": 4 },
                    "validationRule": "sqlite_overflow_next_page"
                },
                "state": "terminal"
            }
        ])
    );
    assert_eq!(
        graph["relationships"],
        json!([
            {
                "claimId": "overflow:cell:1:0",
                "kind": "overflow",
                "source": { "type": "cell", "pageNumber": 1, "cellIndex": 0 },
                "target": { "pageNumber": 2 }
            },
            {
                "claimId": "overflow:page:2",
                "kind": "overflow",
                "source": { "type": "page", "pageNumber": 2 },
                "target": { "pageNumber": 3 }
            }
        ])
    );
    assert!(graph["traversals"].as_array().unwrap().contains(&json!({
        "kind": "overflow",
        "origin": { "type": "cell", "pageNumber": 1, "cellIndex": 0 },
        "validatedPrefix": [{ "pageNumber": 2 }, { "pageNumber": 3 }],
        "stop": null
    })));
    assert_eq!(graph["diagnostics"], json!([]));
    assert!(!graph.to_string().contains("payloadBytes"));
}

#[test]
fn an_out_of_range_overflow_target_retains_the_intended_page_and_stops_at_the_cell() {
    let directory = tempdir().unwrap();
    let mut bytes = valid_overflow_chain();
    bytes[508..512].copy_from_slice(&9_u32.to_be_bytes());
    let graph = inspect(&directory.path().join("missing-overflow.sqlite"), &bytes);
    let claim = graph["relationshipClaims"]
        .as_array()
        .unwrap()
        .iter()
        .find(|claim| claim["id"] == "overflow:cell:1:0")
        .unwrap();
    assert_eq!(claim["state"], "unresolved");
    assert_eq!(claim["target"]["pageNumber"], 9);
    let traversal = graph["traversals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|traversal| traversal["kind"] == "overflow")
        .unwrap();
    assert_eq!(traversal["validatedPrefix"], json!([]));
    assert_eq!(traversal["stop"]["reason"], "out_of_range");
    assert_eq!(
        graph["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|diagnostic| diagnostic["code"] == "overflow_first_page_out_of_range")
            .unwrap()["evidence"][0]["range"],
        json!({ "pageOffset": 508, "fileOffset": 508, "length": 4 })
    );
}

#[test]
fn cyclic_overflow_retains_the_prefix_before_the_repeated_page() {
    let directory = tempdir().unwrap();
    let mut bytes = valid_overflow_chain();
    bytes[512..516].copy_from_slice(&2_u32.to_be_bytes());
    let graph = inspect(&directory.path().join("cyclic-overflow.sqlite"), &bytes);
    let traversal = graph["traversals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|traversal| traversal["kind"] == "overflow")
        .unwrap();

    assert_eq!(traversal["validatedPrefix"], json!([{ "pageNumber": 2 }]));
    assert_eq!(traversal["stop"]["reason"], "cycle");
    assert_eq!(traversal["stop"]["intendedTarget"]["pageNumber"], 2);
    assert!(
        graph["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "overflow_cycle"
                && diagnostic["affectedRelationships"] == json!(["overflow:page:2"]))
    );
}

#[test]
fn early_overflow_termination_is_truncated_not_complete() {
    let directory = tempdir().unwrap();
    let mut bytes = valid_overflow_chain();
    bytes[512..516].copy_from_slice(&0_u32.to_be_bytes());
    let graph = inspect(&directory.path().join("truncated-overflow.sqlite"), &bytes);
    let traversal = graph["traversals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|traversal| traversal["kind"] == "overflow")
        .unwrap();

    assert_eq!(traversal["validatedPrefix"], json!([{ "pageNumber": 2 }]));
    assert_eq!(traversal["stop"]["reason"], "invalid_reference");
    assert_eq!(traversal["stop"]["claimId"], "overflow:page:2");
    assert!(
        graph["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |diagnostic| diagnostic["code"] == "overflow_chain_truncated"
                    && diagnostic["containment"] == "traversal_stopped"
            )
    );
}

#[test]
fn an_overflow_target_with_a_btree_header_is_an_incompatible_claim() {
    let directory = tempdir().unwrap();
    let mut bytes = valid_overflow_chain();
    bytes[512..520].fill(0);
    bytes[512] = 13;
    bytes[517..519].copy_from_slice(&u16::try_from(PAGE_SIZE).unwrap().to_be_bytes());
    let graph = inspect(
        &directory.path().join("conflicting-overflow.sqlite"),
        &bytes,
    );
    let claim = graph["relationshipClaims"]
        .as_array()
        .unwrap()
        .iter()
        .find(|claim| claim["id"] == "overflow:cell:1:0")
        .unwrap();

    assert_eq!(claim["state"], "conflicting");
    assert!(
        graph["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |diagnostic| diagnostic["code"] == "overflow_page_type_conflict"
                    && diagnostic["severity"] == "error"
                    && diagnostic["containment"] == "traversal_stopped"
            )
    );
    assert_eq!(graph["pages"][2]["detail"]["coverage"], "unsupported");
}

#[test]
fn duplicate_overflow_owners_conflict_without_duplicating_page_claims() {
    let directory = tempdir().unwrap();
    let graph = inspect(
        &directory.path().join("duplicate-overflow.sqlite"),
        &duplicate_overflow_roots(),
    );
    let claims = graph["relationshipClaims"].as_array().unwrap();

    assert_eq!(
        claims
            .iter()
            .filter(|claim| claim["id"] == "overflow:page:2")
            .count(),
        1
    );
    assert_eq!(
        claims
            .iter()
            .filter(|claim| claim["id"]
                .as_str()
                .unwrap()
                .starts_with("overflow:cell:1:"))
            .map(|claim| claim["state"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["conflicting", "conflicting"]
    );
    let shared = claims
        .iter()
        .find(|claim| claim["id"] == "overflow:page:2")
        .unwrap();
    assert_eq!(shared["state"], "validated");
    assert!(
        graph["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |diagnostic| diagnostic["code"] == "overflow_duplicate_owner"
                    && diagnostic["affectedRelationships"]
                        == json!(["overflow:cell:1:0", "overflow:cell:1:1"])
            )
    );
    let overflow_traversals: Vec<_> = graph["traversals"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|traversal| traversal["kind"] == "overflow")
        .collect();
    assert_eq!(overflow_traversals.len(), 2);
    assert!(overflow_traversals.iter().all(|traversal| {
        traversal["validatedPrefix"] == json!([])
            && traversal["stop"]["reason"] == "conflicting_claim"
    }));
}

#[test]
fn malformed_interior_payload_keeps_an_independently_readable_child_claim() {
    let directory = tempdir().unwrap();
    let mut bytes = valid_table_btree();
    bytes[105..107].copy_from_slice(&508_u16.to_be_bytes());
    bytes[112..114].copy_from_slice(&508_u16.to_be_bytes());
    bytes[508..512].copy_from_slice(&2_u32.to_be_bytes());
    let graph = inspect(&directory.path().join("malformed-cell.sqlite"), &bytes);

    assert_eq!(
        graph["pages"][0]["detail"]["cells"][0]["diagnostic"],
        "truncated_varint"
    );
    assert_eq!(graph["relationshipClaims"][0]["target"]["pageNumber"], 2);
    assert_eq!(graph["relationshipClaims"][0]["state"], "validated");
    assert_eq!(
        graph["relationshipClaims"][0]["evidence"]["range"],
        json!({ "pageOffset": 508, "fileOffset": 508, "length": 4 })
    );
}

#[test]
fn cancelled_inventory_uses_coverage_stops_without_corruption_diagnostics() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("partial.sqlite");
    File::create(&path)
        .unwrap()
        .write_all(&valid_table_btree())
        .unwrap();
    let session = InspectionSession::begin(&path).unwrap();
    session.advance().unwrap();
    session.advance().unwrap();
    session.stop(ScanControl::Cancel).unwrap();
    let graph = session.graph().unwrap();

    assert_eq!(graph.coverage.evaluated, 1);
    assert!(graph.diagnostics.is_empty());
    assert!(
        graph
            .relationship_claims
            .iter()
            .all(|claim| claim.state == volmap_sqlite::inspection::RelationshipState::Unresolved)
    );
    assert!(
        graph
            .traversals
            .iter()
            .all(|traversal| traversal
                .stop
                .as_ref()
                .is_some_and(|stop| stop.reason
                    == volmap_sqlite::inspection::TraversalStopReason::CoverageStop))
    );
}

#[test]
fn partial_inventory_does_not_infer_a_root_from_an_unparented_prefix_page() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("unknown-parent.sqlite");
    let mut bytes = vec![0; PAGE_SIZE * 3];
    database_header(&mut bytes, 3);
    empty_table_leaf(&mut bytes, 2);
    empty_table_leaf(&mut bytes, 3);
    File::create(&path).unwrap().write_all(&bytes).unwrap();
    let session = InspectionSession::begin(&path).unwrap();
    session.advance().unwrap();
    session.advance().unwrap();
    session.advance().unwrap();
    session.stop(ScanControl::Cancel).unwrap();
    let graph = session.graph().unwrap();

    assert_eq!(graph.coverage.evaluated, 2);
    assert!(graph.traversals.is_empty());
}

#[test]
fn configured_traversal_budget_stops_at_the_last_validated_prefix() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("budget.sqlite");
    File::create(&path)
        .unwrap()
        .write_all(&valid_table_btree())
        .unwrap();
    let session =
        InspectionSession::open_with_traversal_budget(&path, TraversalBudget::new(1, 1)).unwrap();
    let graph = session.graph().unwrap();

    assert!(graph.diagnostics.is_empty());
    assert_eq!(
        graph.topology_coverage.reason,
        TopologyCoverageReason::Budget
    );
    assert!(
        graph
            .traversals
            .iter()
            .filter(|traversal| traversal.kind == volmap_sqlite::inspection::TraversalKind::Btree)
            .all(|traversal| {
                traversal.validated_prefix.len() == 1
                    && traversal.stop.as_ref().is_some_and(|stop| {
                        stop.reason == volmap_sqlite::inspection::TraversalStopReason::Budget
                    })
            })
    );
}

#[test]
fn aggregate_traversal_budget_bounds_prefix_allocation_and_reports_its_boundary() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("aggregate-budget.sqlite");
    File::create(&path)
        .unwrap()
        .write_all(&valid_table_btree())
        .unwrap();
    let session = InspectionSession::open_with_traversal_budget(
        &path,
        TraversalBudget::with_total_pages(u32::MAX, u32::MAX, 1),
    )
    .unwrap();
    let graph = session.graph().unwrap();

    assert_eq!(
        graph.topology_coverage.reason,
        TopologyCoverageReason::Budget
    );
    assert_eq!(graph.topology_coverage.phase, TopologyPhase::BtreeTraversal);
    assert_eq!(graph.topology_coverage.next, Some(4));
    assert_eq!(
        graph.topology_coverage.traversal_budget.max_total_pages(),
        1
    );
    assert!(
        graph
            .traversals
            .iter()
            .any(|traversal| traversal.stop.as_ref().is_some_and(|stop| {
                stop.reason == volmap_sqlite::inspection::TraversalStopReason::Budget
            }))
    );
    assert!(
        graph
            .traversals
            .iter()
            .map(|traversal| traversal.validated_prefix.len() as u64)
            .sum::<u64>()
            <= 1
    );
}

#[test]
fn aggregate_traversal_budget_is_shared_with_overflow_prefixes() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("aggregate-overflow-budget.sqlite");
    File::create(&path)
        .unwrap()
        .write_all(&duplicate_overflow_roots())
        .unwrap();
    let session = InspectionSession::open_with_traversal_budget(
        &path,
        TraversalBudget::with_total_pages(u32::MAX, u32::MAX, 1),
    )
    .unwrap();
    let graph = session.graph().unwrap();
    assert_eq!(
        graph.topology_coverage.reason,
        TopologyCoverageReason::Budget
    );
    assert_eq!(
        graph.topology_coverage.phase,
        TopologyPhase::OverflowInspection
    );
    assert_eq!(graph.topology_coverage.evaluated, 0);
    assert_eq!(graph.topology_coverage.total, Some(2));
    assert_eq!(graph.topology_coverage.next, Some(1));
    assert_eq!(graph.topology_coverage.remainder, Some(2));
    assert!(
        graph
            .relationship_claims
            .iter()
            .filter(|claim| claim.kind == volmap_sqlite::inspection::RelationshipKind::Overflow)
            .all(|claim| claim.state != volmap_sqlite::inspection::RelationshipState::Validated)
    );
    let stopped = graph
        .traversals
        .iter()
        .find(|traversal| {
            traversal.origin
                == volmap_sqlite::inspection::EntityIdentity::Cell {
                    page_number: 1,
                    cell_index: 0,
                }
        })
        .unwrap();
    assert!(stopped.validated_prefix.is_empty());
    assert_eq!(
        stopped.stop.as_ref().unwrap().reason,
        volmap_sqlite::inspection::TraversalStopReason::Budget
    );
    assert_eq!(stopped.stop.as_ref().unwrap().claim_id, "overflow:cell:1:0");
    assert_eq!(
        stopped.stop.as_ref().unwrap().intended_target,
        Some(volmap_sqlite::inspection::PageIdentity { page_number: 2 })
    );
}

#[test]
fn aggregate_overflow_stop_names_the_cached_edge_that_was_not_followed() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("cached-overflow-budget.sqlite");
    File::create(&path)
        .unwrap()
        .write_all(&duplicate_overflow_roots())
        .unwrap();
    let session = InspectionSession::open_with_traversal_budget(
        &path,
        TraversalBudget::with_total_pages(u32::MAX, u32::MAX, 3),
    )
    .unwrap();
    let graph = session.graph().unwrap();
    let stopped = graph
        .traversals
        .iter()
        .find(|traversal| {
            traversal.origin
                == volmap_sqlite::inspection::EntityIdentity::Cell {
                    page_number: 1,
                    cell_index: 1,
                }
                && traversal.stop.as_ref().is_some_and(|stop| {
                    stop.reason == volmap_sqlite::inspection::TraversalStopReason::Budget
                })
        })
        .unwrap();

    assert_eq!(graph.topology_coverage.evaluated, 1);
    assert_eq!(graph.topology_coverage.next, Some(2));
    assert_eq!(
        stopped.validated_prefix,
        vec![volmap_sqlite::inspection::PageIdentity { page_number: 2 }]
    );
    assert_eq!(stopped.stop.as_ref().unwrap().claim_id, "overflow:page:2");
    assert_eq!(
        stopped.stop.as_ref().unwrap().intended_target,
        Some(volmap_sqlite::inspection::PageIdentity { page_number: 3 })
    );
}

#[test]
fn configured_overflow_budget_stops_before_following_the_next_link() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("overflow-budget.sqlite");
    File::create(&path)
        .unwrap()
        .write_all(&valid_overflow_chain())
        .unwrap();
    let session =
        InspectionSession::open_with_traversal_budget(&path, TraversalBudget::new(u32::MAX, 1))
            .unwrap();
    let graph = session.graph().unwrap();
    let traversal = graph
        .traversals
        .iter()
        .find(|traversal| traversal.kind == volmap_sqlite::inspection::TraversalKind::Overflow)
        .unwrap();

    assert_eq!(
        graph.topology_coverage.reason,
        TopologyCoverageReason::Budget
    );
    assert_eq!(traversal.validated_prefix[0].page_number, 2);
    assert_eq!(traversal.validated_prefix.len(), 1);
    assert_eq!(
        traversal.stop.as_ref().unwrap().reason,
        volmap_sqlite::inspection::TraversalStopReason::Budget
    );
    assert_eq!(traversal.stop.as_ref().unwrap().claim_id, "overflow:page:2");
    assert!(graph.diagnostics.is_empty());
}

#[test]
fn invalidation_retains_published_relationship_observations_as_evidence() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("retained.sqlite");
    File::create(&path)
        .unwrap()
        .write_all(&valid_table_btree())
        .unwrap();
    let session = InspectionSession::open(&path).unwrap();
    let graph = session.graph().unwrap();
    assert!(!graph.relationship_claims.is_empty());

    std::fs::remove_file(&path).unwrap();
    let evidence = session.evidence().unwrap();
    assert_eq!(evidence.relationship_claims, graph.relationship_claims);
    assert_eq!(evidence.relationships, graph.relationships);
    assert_eq!(evidence.traversals, graph.traversals);
    assert_eq!(evidence.diagnostics, graph.diagnostics);
}

#[test]
fn cancelling_active_topology_publishes_a_coherent_bounded_revision() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("cancel-topology.sqlite");
    let interior_pages = 5_000_u32;
    let page_count = interior_pages * 2 + 1;
    File::create(&path)
        .unwrap()
        .write_all(&large_table_btree(interior_pages))
        .unwrap();

    let session = std::sync::Arc::new(InspectionSession::begin(&path).unwrap());
    session.advance().unwrap();
    for _ in 0..page_count {
        session.advance().unwrap();
    }
    let worker_session = std::sync::Arc::clone(&session);
    let worker = std::thread::spawn(move || worker_session.advance());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !session.status().progress.building_topology {
        assert!(
            std::time::Instant::now() < deadline,
            "topology did not start"
        );
        std::thread::yield_now();
    }
    session.stop(ScanControl::Cancel).unwrap();
    worker.join().unwrap().unwrap();

    let status = session.status();
    assert_eq!(
        status.state,
        volmap_sqlite::inspection::SessionState::Cancelled
    );
    assert_eq!(
        status.coverage.unwrap().reason,
        volmap_sqlite::inspection::CoverageReason::Complete
    );
    let graph = session.graph().unwrap();
    assert_exact_topology_stop(graph.topology_coverage, TopologyCoverageReason::Cancelled);
    for relationship in &graph.relationships {
        assert!(graph.relationship_claims.iter().any(|claim| {
            claim.id == relationship.claim_id
                && claim.state == volmap_sqlite::inspection::RelationshipState::Validated
        }));
    }
}

#[test]
fn stopping_active_topology_records_operator_intent_and_an_exact_boundary() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("stop-topology.sqlite");
    let interior_pages = 5_000_u32;
    let page_count = interior_pages * 2 + 1;
    File::create(&path)
        .unwrap()
        .write_all(&large_table_btree(interior_pages))
        .unwrap();

    let session = std::sync::Arc::new(InspectionSession::begin(&path).unwrap());
    session.advance().unwrap();
    for _ in 0..page_count {
        session.advance().unwrap();
    }
    let worker_session = std::sync::Arc::clone(&session);
    let worker = std::thread::spawn(move || worker_session.advance());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !session.status().progress.building_topology {
        assert!(
            std::time::Instant::now() < deadline,
            "topology did not start"
        );
        std::thread::yield_now();
    }
    session.stop(ScanControl::Stop).unwrap();
    worker.join().unwrap().unwrap();

    assert_eq!(
        session.status().state,
        volmap_sqlite::inspection::SessionState::Stopped
    );
    assert_exact_topology_stop(
        session.graph().unwrap().topology_coverage,
        TopologyCoverageReason::OperatorStop,
    );
}

#[test]
fn invalidation_interrupts_active_topology_and_retains_its_observations() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("invalidate-topology.sqlite");
    let interior_pages = 5_000_u32;
    let page_count = interior_pages * 2 + 1;
    File::create(&path)
        .unwrap()
        .write_all(&large_table_btree(interior_pages))
        .unwrap();

    let session = std::sync::Arc::new(InspectionSession::begin(&path).unwrap());
    session.advance().unwrap();
    for _ in 0..page_count {
        session.advance().unwrap();
    }
    let worker_session = std::sync::Arc::clone(&session);
    let worker = std::thread::spawn(move || worker_session.advance());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !session.status().progress.building_topology {
        assert!(
            std::time::Instant::now() < deadline,
            "topology did not start"
        );
        std::thread::yield_now();
    }

    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .write_all_at(&[13], PAGE_SIZE as u64)
        .unwrap();
    assert_eq!(
        session.status().state,
        volmap_sqlite::inspection::SessionState::Invalidated
    );
    assert!(matches!(
        worker.join().unwrap(),
        Err(volmap_sqlite::inspection::InspectionError::Invalidated)
    ));
    assert_eq!(
        session.evidence().unwrap().topology_coverage.reason,
        volmap_sqlite::inspection::TopologyCoverageReason::Cancelled
    );
}
