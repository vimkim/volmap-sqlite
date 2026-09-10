use serde_json::{Value, json};
use std::fs;
use tempfile::tempdir;
use volmap_sqlite::inspection::InspectionSession;

fn put(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn fixture() -> Vec<u8> {
    let mut bytes = vec![0; 512 * 5];
    bytes[..16].copy_from_slice(b"SQLite format 3\0");
    bytes[16..18].copy_from_slice(&512_u16.to_be_bytes());
    bytes[18..24].copy_from_slice(&[1, 1, 0, 64, 32, 32]);
    put(&mut bytes, 28, 5);
    put(&mut bytes, 32, 2);
    put(&mut bytes, 36, 4);
    put(&mut bytes, 44, 4);
    put(&mut bytes, 56, 1);
    bytes[100] = 13;
    bytes[105..107].copy_from_slice(&512_u16.to_be_bytes());
    put(&mut bytes, 512, 4); // trunk 2 -> trunk 4
    put(&mut bytes, 516, 1);
    put(&mut bytes, 520, 3); // leaf 3
    put(&mut bytes, 1540, 1);
    put(&mut bytes, 1544, 5); // leaf 5
    // Freed pages may still contain plausible B-tree headers.
    for base in [1024, 2048] {
        bytes[base] = 13;
        bytes[base + 5..base + 7].copy_from_slice(&512_u16.to_be_bytes());
    }
    bytes
}

fn inspect(bytes: &[u8]) -> Value {
    let directory = tempdir().unwrap();
    let path = directory.path().join("freelist.sqlite");
    fs::write(&path, bytes).unwrap();
    serde_json::to_value(&*InspectionSession::open(&path).unwrap().graph().unwrap()).unwrap()
}

#[test]
fn navigates_header_to_multiple_trunks_and_leaves_without_interpreting_stale_cells() {
    let graph = inspect(&fixture());
    assert_eq!(graph["freelist"]["firstTrunk"]["value"], 2);
    assert_eq!(graph["freelist"]["declaredCount"]["value"], 4);
    assert_eq!(
        graph["freelist"]["declaredCount"]["evidence"]["range"],
        json!({"pageOffset":36,"fileOffset":36,"length":4})
    );
    assert_eq!(graph["freelist"]["coverage"]["reason"], "complete");
    assert_eq!(graph["freelist"]["coverage"]["evaluatedPages"], 4);
    assert_eq!(
        graph["pages"][1]["detail"]["allocationRole"],
        "freelist_trunk"
    );
    assert_eq!(
        graph["pages"][2]["detail"]["allocationRole"],
        "freelist_leaf"
    );
    assert_eq!(graph["pages"][2]["detail"]["kind"], Value::Null);
    assert_eq!(graph["pages"][2]["detail"]["cells"], json!([]));
    let leaf = graph["relationshipClaims"]
        .as_array()
        .unwrap()
        .iter()
        .find(|claim| claim["kind"] == "freelist_leaf" && claim["target"]["pageNumber"] == 3)
        .unwrap();
    assert_eq!(
        leaf["evidence"]["range"],
        json!({"pageOffset":8,"fileOffset":520,"length":4})
    );
    assert_eq!(leaf["state"], "validated");
    assert_eq!(graph["relationships"].as_array().unwrap().len(), 4);
    let traversal = graph["traversals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["kind"] == "freelist")
        .unwrap();
    assert_eq!(
        traversal["validatedPrefix"],
        json!([{"pageNumber":2},{"pageNumber":4}])
    );
    assert_eq!(traversal["stop"], Value::Null);
    assert!(graph["diagnostics"].as_array().unwrap().is_empty());
}

#[test]
fn impossible_leaf_count_stops_that_trunk_without_reading_unused_entries() {
    let mut bytes = fixture();
    put(&mut bytes, 516, 127); // 512 usable bytes allow at most 126 leaves.
    let graph = inspect(&bytes);
    assert_eq!(graph["freelist"]["coverage"]["reason"], "invalid_structure");
    assert_eq!(graph["freelist"]["coverage"]["remainder"], Value::Null);
    assert!(
        graph["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "freelist_leaf_count_exceeds_capacity")
    );
    assert!(
        !graph["relationshipClaims"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["kind"] == "freelist_leaf")
    );
    let traversal = graph["traversals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["kind"] == "freelist")
        .unwrap();
    assert_eq!(traversal["validatedPrefix"], json!([{"pageNumber":2}]));
    assert_eq!(traversal["stop"]["reason"], "invalid_reference");
}

#[test]
fn repeated_leaf_claims_are_conflicting_while_independent_trunks_remain_navigable() {
    let mut bytes = fixture();
    put(&mut bytes, 516, 2);
    put(&mut bytes, 524, 3);
    let graph = inspect(&bytes);
    let claims: Vec<_> = graph["relationshipClaims"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["target"]["pageNumber"] == 3)
        .collect();
    assert_eq!(claims.len(), 2);
    assert!(claims.iter().all(|c| c["state"] == "conflicting"));
    assert_eq!(graph["pages"][2]["detail"]["allocationRole"], "conflicting");
    assert!(
        !graph["relationships"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["target"]["pageNumber"] == 3)
    );
    assert!(
        graph["relationships"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["target"]["pageNumber"] == 4)
    );
    assert_eq!(
        graph["diagnostics"][0]["affectedRelationships"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn empty_and_damaged_totals_report_evidence_without_hiding_trunks() {
    let mut bytes = fixture();
    put(&mut bytes, 32, 0);
    put(&mut bytes, 36, 0);
    let empty = inspect(&bytes);
    assert_eq!(empty["freelist"]["coverage"]["reason"], "complete");
    assert_eq!(empty["freelist"]["coverage"]["evaluatedPages"], 0);
    assert_eq!(empty["freelist"]["trunks"], json!([]));
    for (first, count) in [(0, 4), (2, 0), (2, 99), (2, 3)] {
        put(&mut bytes, 32, first);
        put(&mut bytes, 36, count);
        let graph = inspect(&bytes);
        assert_eq!(
            graph["freelist"]["coverage"]["reason"], "invalid_structure",
            "first={first} count={count}"
        );
        let diagnostic = graph["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["code"] == "freelist_total_mismatch")
            .unwrap();
        assert!(
            diagnostic["evidence"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["range"]["fileOffset"] == 36)
        );
        assert_eq!(
            graph["freelist"]["coverage"]["evaluatedPages"],
            if first == 0 { 0 } else { 4 }
        );
    }
}

#[test]
fn cycle_retains_the_trunk_prefix_and_broken_leaf_does_not_hide_siblings() {
    let mut bytes = fixture();
    put(&mut bytes, 1536, 2);
    let graph = inspect(&bytes);
    let traversal = graph["traversals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["kind"] == "freelist")
        .unwrap();
    assert_eq!(
        traversal["validatedPrefix"],
        json!([{"pageNumber":2},{"pageNumber":4}])
    );
    assert_eq!(traversal["stop"]["reason"], "cycle");
    assert_eq!(traversal["stop"]["intendedTarget"]["pageNumber"], 2);
    assert_eq!(graph["freelist"]["coverage"]["evaluatedPages"], 4);

    let mut bytes = fixture();
    put(&mut bytes, 520, 99);
    let graph = inspect(&bytes);
    let missing = graph["relationshipClaims"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["target"]["pageNumber"] == 99)
        .unwrap();
    assert_eq!(missing["state"], "unresolved");
    assert!(
        graph["relationships"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["target"]["pageNumber"] == 5)
    );
    assert_eq!(graph["pages"].as_array().unwrap().len(), 5);
}

#[test]
fn capacity_uses_usable_bytes_accepts_modern_tail_slots_and_ignores_unused_slots() {
    let mut bytes = fixture();
    put(&mut bytes, 524, u32::MAX); // outside the declared one-leaf array
    put(&mut bytes, 1020, u32::MAX); // unused historical compatibility slot
    assert!(
        inspect(&bytes)["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    bytes.resize(512 * 128, 0);
    put(&mut bytes, 28, 128);
    put(&mut bytes, 36, 127);
    put(&mut bytes, 512, 0);
    put(&mut bytes, 516, 126);
    for (offset, page) in (520..1024).step_by(4).zip(3..=128) {
        put(&mut bytes, offset, page);
    }
    let graph = inspect(&bytes);
    assert_eq!(graph["freelist"]["trunks"][0]["capacity"], 126);
    assert_eq!(graph["freelist"]["trunks"][0]["compatibilityCapacity"], 120);
    assert_eq!(graph["freelist"]["coverage"]["reason"], "complete");
    assert_eq!(
        graph["pages"][127]["detail"]["allocationRole"],
        "freelist_leaf"
    );
    bytes[20] = 32; // 480 usable bytes -> 118 entries, 112 compatibility entries
    let graph = inspect(&bytes);
    assert_eq!(graph["freelist"]["trunks"][0]["capacity"], 118);
    assert_eq!(graph["freelist"]["coverage"]["reason"], "invalid_structure");
    assert_eq!(graph["freelist"]["coverage"]["evaluatedPages"], 1);
}

#[test]
fn budget_stops_at_the_next_leaf_without_promoting_unvisited_claims() {
    use volmap_sqlite::inspection::TraversalBudget;
    let directory = tempdir().unwrap();
    let path = directory.path().join("budget.sqlite");
    fs::write(&path, fixture()).unwrap();
    let session = InspectionSession::open_with_traversal_budget(
        &path,
        TraversalBudget::with_total_pages(100, 100, 1),
    )
    .unwrap();
    let graph = serde_json::to_value(&*session.graph().unwrap()).unwrap();
    let traversal = graph["traversals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["kind"] == "freelist")
        .unwrap();
    assert_eq!(traversal["stop"]["intendedTarget"]["pageNumber"], 3);
    assert_eq!(traversal["stop"]["reason"], "budget");
    assert_eq!(graph["freelist"]["coverage"]["reason"], "budget");
    assert_eq!(graph["topologyCoverage"]["phase"], "freelist_inspection");
    assert!(graph["diagnostics"].as_array().unwrap().is_empty());
}

#[test]
fn real_sqlite_deletion_agrees_with_sqlites_freelist_count() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("deleted.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA page_size=512; PRAGMA secure_delete=OFF; CREATE TABLE items(value); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<500) INSERT INTO items SELECT zeroblob(600) FROM n; DELETE FROM items;").unwrap();
    let expected: u32 = db
        .query_row("PRAGMA freelist_count", [], |row| row.get(0))
        .unwrap();
    assert!(expected > 126);
    drop(db);
    let graph =
        serde_json::to_value(&*InspectionSession::open(&path).unwrap().graph().unwrap()).unwrap();
    assert_eq!(graph["freelist"]["declaredCount"]["value"], expected);
    assert_eq!(graph["freelist"]["coverage"]["evaluatedPages"], expected);
    assert_eq!(graph["freelist"]["coverage"]["reason"], "complete");
    assert!(graph["freelist"]["trunks"].as_array().unwrap().len() > 1);
    assert!(graph["diagnostics"].as_array().unwrap().is_empty());
}

#[test]
fn stopped_inventory_and_truncated_targets_do_not_claim_complete_allocation() {
    use volmap_sqlite::inspection::ScanControl;
    let directory = tempdir().unwrap();
    let path = directory.path().join("partial.sqlite");
    fs::write(&path, fixture()).unwrap();
    let session = InspectionSession::begin(&path).unwrap();
    session.advance().unwrap(); // geometry
    session.advance().unwrap();
    session.advance().unwrap();
    session.stop(ScanControl::Stop).unwrap();
    let graph = serde_json::to_value(&*session.graph().unwrap()).unwrap();
    assert_eq!(graph["freelist"]["coverage"]["reason"], "coverage_stop");
    assert_eq!(graph["freelist"]["coverage"]["remainder"], Value::Null);
    assert_eq!(graph["pages"].as_array().unwrap().len(), 2);
    assert!(
        !graph["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "freelist_total_mismatch")
    );

    let mut truncated = fixture();
    truncated.truncate(512 * 4);
    put(&mut truncated, 28, 0); // geometry uses the actual, whole-page file length
    let graph = inspect(&truncated);
    assert_eq!(graph["freelist"]["coverage"]["reason"], "invalid_structure");
    assert!(
        graph["relationshipClaims"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["target"]["pageNumber"] == 5 && c["state"] == "unresolved")
    );
}

#[test]
fn trunk_byte_map_distinguishes_declared_pointers_from_unused_and_reserved_bytes() {
    let graph = inspect(&fixture());
    let regions = graph["pages"][1]["detail"]["regions"].as_array().unwrap();
    let pointers = regions
        .iter()
        .find(|r| r["kind"] == "freelist_leaf_pointers")
        .unwrap();
    assert_eq!(
        pointers["range"],
        json!({"pageOffset":8,"fileOffset":520,"length":4})
    );
    let unused = regions
        .iter()
        .find(|r| r["kind"] == "freelist_unused")
        .unwrap();
    assert_eq!(
        unused["range"],
        json!({"pageOffset":12,"fileOffset":524,"length":500})
    );
}

#[test]
fn independently_supported_storage_links_conflict_with_allocation_claims_on_both_sides() {
    for overflow in [false, true] {
        let mut bytes = fixture();
        if overflow {
            bytes[103..105].copy_from_slice(&1_u16.to_be_bytes());
            bytes[105..107].copy_from_slice(&466_u16.to_be_bytes());
            bytes[108..110].copy_from_slice(&466_u16.to_be_bytes());
            bytes[466..469].copy_from_slice(&[0x87, 0x68, 0x01]);
            put(&mut bytes, 508, 3);
        } else {
            bytes[100] = 5;
            put(&mut bytes, 108, 3);
        }
        let graph = inspect(&bytes);
        assert_eq!(graph["pages"][2]["detail"]["allocationRole"], "conflicting");
        let claims: Vec<_> = graph["relationshipClaims"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|claim| claim["target"]["pageNumber"] == 3)
            .collect();
        assert_eq!(claims.len(), 2);
        assert!(claims.iter().all(|claim| claim["state"] == "conflicting"));
        assert!(
            !graph["relationships"]
                .as_array()
                .unwrap()
                .iter()
                .any(|relationship| relationship["target"]["pageNumber"] == 3)
        );
        let diagnostic = graph["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["code"] == "freelist_storage_role_conflict")
            .unwrap();
        assert_eq!(
            diagnostic["affectedRelationships"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(diagnostic["evidence"].as_array().unwrap().len(), 2);
        assert_eq!(graph["freelist"]["coverage"]["reason"], "invalid_structure");
    }
}

#[test]
fn trunk_chain_budget_retains_the_prefix_and_does_not_claim_a_count_mismatch() {
    use volmap_sqlite::inspection::{OperationalBudget, ScanControl};
    for ceiling in [0, 1, 2] {
        let directory = tempdir().unwrap();
        let path = directory.path().join("chain.sqlite");
        fs::write(&path, fixture()).unwrap();
        let session = InspectionSession::begin(&path)
            .unwrap()
            .with_operational_budget(OperationalBudget {
                max_freelist_trunks: ceiling,
                ..OperationalBudget::default()
            });
        session.scan(|_| ScanControl::Continue).unwrap();
        let graph = serde_json::to_value(&*session.graph().unwrap()).unwrap();
        assert_eq!(
            graph["freelist"]["trunks"].as_array().unwrap().len(),
            ceiling as usize
        );
        assert_eq!(
            graph["freelist"]["coverage"]["reason"],
            if ceiling < 2 { "budget" } else { "complete" }
        );
        assert!(!graph["diagnostics"].to_string().contains("count_mismatch"));
        assert_eq!(graph["coverage"]["reason"], "complete");
    }
}
