use serde_json::Value;
use std::fs;
use tempfile::tempdir;
use volmap_sqlite::inspection::InspectionSession;

fn put(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn fixture(pages: u32, auto_vacuum: bool) -> Vec<u8> {
    let mut bytes = vec![0; pages as usize * 512];
    bytes[..16].copy_from_slice(b"SQLite format 3\0");
    bytes[16..18].copy_from_slice(&512_u16.to_be_bytes());
    bytes[18..24].copy_from_slice(&[1, 1, 0, 64, 32, 32]);
    put(&mut bytes, 28, pages);
    put(&mut bytes, 44, 4);
    put(&mut bytes, 52, u32::from(auto_vacuum));
    put(&mut bytes, 56, 1);
    bytes[100] = 13;
    bytes[105..107].copy_from_slice(&512_u16.to_be_bytes());
    bytes
}

fn inspect(bytes: &[u8]) -> Value {
    let directory = tempdir().unwrap();
    let path = directory.path().join("roles.sqlite");
    fs::write(&path, bytes).unwrap();
    serde_json::to_value(&*InspectionSession::open(&path).unwrap().graph().unwrap()).unwrap()
}

#[test]
fn classifies_local_structure_and_leaves_unreferenced_bytes_unknown_without_auto_vacuum() {
    let graph = inspect(&fixture(3, false));
    assert_eq!(graph["pointerMap"]["applicable"], false);
    assert_eq!(
        graph["pointerMap"]["largestRoot"]["evidence"]["range"]["fileOffset"],
        52
    );
    assert_eq!(graph["pages"][0]["classification"]["role"], "table_leaf");
    assert_eq!(graph["pages"][1]["classification"]["role"], "unknown");
    assert_eq!(graph["pages"][1]["classification"]["referenced"], false);
    assert_eq!(
        graph["pages"][1]["classification"]["claims"],
        serde_json::json!([])
    );
}

#[test]
fn inspects_pointer_map_boundaries_and_preserves_typed_entry_evidence() {
    // SQLite format example: U=512 gives 102 entries; map pages are 2 and 105.
    let mut bytes = fixture(106, true);
    put(&mut bytes, 52, 3);
    for offset in (512..1022).step_by(5) {
        bytes[offset] = 2;
    }
    bytes[512] = 1;
    bytes[1024] = 13;
    bytes[1029..1031].copy_from_slice(&512_u16.to_be_bytes());
    bytes[104 * 512] = 2;
    let graph = inspect(&bytes);
    assert_eq!(graph["pages"][1]["classification"]["role"], "pointer_map");
    assert!(graph["pages"][1]["detail"]["header"].is_null());
    assert_eq!(graph["pointerMap"]["pages"][0]["page"]["pageNumber"], 2);
    assert_eq!(graph["pointerMap"]["pages"][1]["page"]["pageNumber"], 105);
    assert_eq!(
        graph["pointerMap"]["locations"],
        serde_json::json!([{ "pageNumber": 2 }, { "pageNumber": 105 }])
    );
    let entries = graph["pointerMap"]["pages"][0]["entries"]
        .as_array()
        .unwrap();
    assert_eq!(entries.len(), 102);
    assert_eq!(entries[0]["kind"], "btree_root");
    assert_eq!(
        entries[0]["target"],
        serde_json::json!({"type":"page","pageNumber":3})
    );
    assert_eq!(
        entries[0]["evidence"]["range"],
        serde_json::json!({"pageOffset":0,"fileOffset":512,"length":5})
    );
    assert_eq!(entries[101]["target"]["pageNumber"], 104);
    assert_eq!(
        graph["pointerMap"]["pages"][1]["entries"][0]["target"]["pageNumber"],
        106
    );
}

#[test]
fn sqlite_generated_auto_vacuum_roles_agree_with_forward_topology() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("sqlite-generated.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "PRAGMA page_size=512; PRAGMA auto_vacuum=INCREMENTAL;
        CREATE TABLE items(id INTEGER PRIMARY KEY, value BLOB);
        WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100)
        INSERT INTO items SELECT x, zeroblob(1200) FROM n;
        DELETE FROM items WHERE id>75;",
    )
    .unwrap();
    drop(db);
    let graph =
        serde_json::to_value(&*InspectionSession::open(&path).unwrap().graph().unwrap()).unwrap();
    let entries: Vec<_> = graph["pointerMap"]["pages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|map| map["entries"].as_array().unwrap())
        .collect();
    for kind in [
        "btree_root",
        "btree_child",
        "freelist",
        "first_overflow",
        "overflow",
    ] {
        assert!(
            entries.iter().any(|entry| entry["kind"] == kind),
            "missing {kind}"
        );
    }
    for entry in entries {
        assert_eq!(entry["state"], "validated", "{entry}");
        assert_eq!(entry["diagnostics"], serde_json::json!([]));
    }
    for page in graph["pages"].as_array().unwrap() {
        assert_ne!(page["classification"]["role"], "unknown", "{page}");
        assert_ne!(page["classification"]["role"], "conflicting", "{page}");
    }
    assert_eq!(graph["diagnostics"], serde_json::json!([]));
}

#[test]
fn contradictory_and_malformed_entries_retain_evidence_and_exclude_links() {
    let mut bytes = fixture(6, true);
    put(&mut bytes, 52, 4);
    bytes[100] = 5; // page 1 points to page 3
    put(&mut bytes, 108, 3);
    for page in [3, 4] {
        bytes[(page - 1) * 512] = 13;
        bytes[(page - 1) * 512 + 5..(page - 1) * 512 + 7].copy_from_slice(&512_u16.to_be_bytes());
    }
    bytes[512] = 5;
    put(&mut bytes, 513, 4); // claims parent 4 instead of 1
    bytes[517] = 1; // root page 4
    bytes[522] = 99; // unknown kind for page 5
    bytes[527] = 3; // illegal zero overflow owner for page 6
    let graph = inspect(&bytes);
    let entries = &graph["pointerMap"]["pages"][0]["entries"];
    assert_eq!(entries[0]["state"], "conflicting");
    assert_eq!(
        entries[0]["parent"],
        serde_json::json!({"type":"page","pageNumber":4})
    );
    assert_eq!(graph["pages"][2]["classification"]["role"], "conflicting");
    assert!(
        graph["pages"][2]["classification"]["claims"]
            .as_array()
            .unwrap()
            .len()
            >= 3
    );
    assert_eq!(entries[1]["state"], "validated");
    assert_eq!(entries[2]["rawKind"], 99);
    assert_eq!(entries[2]["state"], "invalid");
    assert_eq!(entries[3]["state"], "invalid");
    assert_eq!(graph["pages"][4]["classification"]["role"], "unknown");
    assert_eq!(graph["pages"][5]["classification"]["role"], "unknown");
    assert!(
        !graph["relationships"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["target"]["pageNumber"] == 3)
    );
    assert!(graph["traversals"].as_array().unwrap().iter().all(|t| {
        !t["validatedPrefix"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["pageNumber"] == 3)
    }));
}

#[test]
fn reserved_pages_reject_freelist_and_btree_claims_before_content_parsing() {
    let mut bytes = fixture(3, true);
    put(&mut bytes, 32, 2);
    put(&mut bytes, 36, 1);
    bytes[100] = 5;
    put(&mut bytes, 108, 2);
    bytes[512] = 1;
    let graph = inspect(&bytes);
    let page = &graph["pages"][1];
    assert_eq!(page["classification"]["role"], "conflicting");
    assert!(page["detail"]["header"].is_null());
    assert!(page["detail"]["allocationRole"].is_null());
    assert_eq!(graph["freelist"]["trunks"], serde_json::json!([]));
    assert!(
        page["classification"]["claims"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["role"] == "pointer_map")
    );
    assert!(
        page["classification"]["claims"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["role"] == "freelist_trunk")
    );
}

#[test]
fn truncated_pointer_map_preserves_complete_entry_and_readable_partial_entry() {
    let mut bytes = fixture(4, true);
    put(&mut bytes, 52, 3);
    bytes[512] = 1;
    bytes[517] = 5;
    bytes.truncate(519);
    let graph = inspect(&bytes);
    assert_eq!(graph["snapshot"]["geometry"]["pageCount"], 2);
    let map = &graph["pointerMap"]["pages"][0];
    assert_eq!(map["complete"], false);
    assert_eq!(map["entries"][0]["rawKind"], 1);
    assert_eq!(map["entries"][0]["parentValue"], 0);
    assert_eq!(map["entries"][1]["rawKind"], 5);
    assert!(map["entries"][1]["parentValue"].is_null());
    assert_eq!(map["entries"][1]["evidence"]["range"]["length"], 2);
    assert!(
        graph["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "pointer_map_truncated")
    );
}

#[test]
fn sparse_gigabyte_image_identifies_lock_page_without_parsing_its_plausible_header() {
    use std::os::unix::fs::FileExt;
    let directory = tempdir().unwrap();
    let path = directory.path().join("lock.sqlite");
    let mut bytes = fixture(1, false);
    bytes[16..18].copy_from_slice(&1_u16.to_be_bytes()); // 65536
    put(&mut bytes, 28, 16_385);
    bytes[105..107].fill(0);
    let file = fs::File::create(&path).unwrap();
    file.set_len(1_073_807_360).unwrap();
    file.write_all_at(&bytes, 0).unwrap();
    file.write_all_at(&[13, 0, 0, 0, 0, 0, 0, 0], 1_073_741_824)
        .unwrap();
    drop(file);
    let session = InspectionSession::open(&path).unwrap();
    let graph = session.graph().unwrap();
    let lock = &graph.pages[16_384];
    assert_eq!(
        serde_json::to_value(&lock.classification).unwrap()["role"],
        "lock_byte"
    );
    assert!(lock.detail.header.is_none());
    assert!(lock.detail.cells.is_empty());
    assert_eq!(lock.detail.regions[0].range.file_offset, 1_073_741_824);
}

#[test]
fn displaced_pointer_map_geometry_remains_visible_in_a_stopped_inventory() {
    use std::os::unix::fs::FileExt;
    use volmap_sqlite::inspection::ScanControl;
    let directory = tempdir().unwrap();
    let path = directory.path().join("displaced.sqlite");
    let mut bytes = fixture(1, true);
    bytes[16..18].copy_from_slice(&1024_u16.to_be_bytes());
    put(&mut bytes, 28, 1_048_784);
    let file = fs::File::create(&path).unwrap();
    file.set_len(1_073_954_816).unwrap();
    file.write_all_at(&bytes, 0).unwrap();
    drop(file);
    let session = InspectionSession::begin(&path).unwrap();
    session
        .scan(|s| {
            if s.progress.total.is_some() {
                ScanControl::Stop
            } else {
                ScanControl::Continue
            }
        })
        .unwrap();
    let graph = serde_json::to_value(&*session.graph().unwrap()).unwrap();
    assert_eq!(graph["pointerMap"]["lockBytePage"]["pageNumber"], 1_048_577);
    assert_eq!(graph["pointerMap"]["locations"], serde_json::json!([]));
    assert_eq!(graph["pointerMap"]["layout"]["stride"], 205);
    assert_eq!(
        graph["pointerMap"]["layout"]["displacedMap"]["pageNumber"],
        1_048_578
    );
    assert_eq!(
        graph["pointerMap"]["layout"]["nextRegularAfterDisplaced"]["pageNumber"],
        1_048_782
    );
    assert_eq!(graph["pointerMap"]["complete"], false);
}

#[test]
fn invalid_header_and_parent_claims_are_contained_without_disabling_pointer_maps() {
    for parent in [0, 2, 3, 500] {
        let mut bytes = fixture(3, true);
        put(&mut bytes, 52, 500);
        bytes[512] = 5;
        put(&mut bytes, 513, parent);
        let graph = inspect(&bytes);
        assert_eq!(graph["pointerMap"]["applicable"], true);
        assert_eq!(
            graph["pointerMap"]["pages"][0]["entries"][0]["state"],
            "invalid"
        );
        assert_eq!(
            graph["pointerMap"]["pages"][0]["entries"][0]["parentValue"],
            parent
        );
        assert!(
            graph["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["code"] == "pointer_map_invalid_largest_root")
        );
    }
}

#[test]
fn allocation_retains_stale_header_observations_without_validating_them_as_live_roles() {
    let mut bytes = fixture(3, false);
    put(&mut bytes, 32, 2);
    put(&mut bytes, 36, 2);
    put(&mut bytes, 516, 1);
    put(&mut bytes, 520, 3);
    bytes[1024] = 13;
    bytes[1029..1031].copy_from_slice(&512_u16.to_be_bytes());
    let graph = inspect(&bytes);
    assert_eq!(graph["pages"][2]["classification"]["role"], "freelist_leaf");
    let claims = graph["pages"][2]["classification"]["claims"]
        .as_array()
        .unwrap();
    assert!(claims.iter().any(|c| c["source"] == "local_structure"
        && c["role"] == "table_leaf"
        && c["state"] == "unresolved"));
    assert!(
        claims
            .iter()
            .any(|c| c["role"] == "freelist_leaf" && c["state"] == "validated")
    );
}

#[test]
fn physical_source_role_conflicts_stop_dependent_links_but_keep_readable_claims() {
    let mut bytes = fixture(4, true);
    bytes[512] = 2; // relocation metadata claims page 3 is free
    bytes[517] = 5;
    put(&mut bytes, 518, 3);
    bytes[1024] = 5; // independent local page 3 interior -> page 4 leaf
    bytes[1029..1031].copy_from_slice(&512_u16.to_be_bytes());
    put(&mut bytes, 1032, 4);
    bytes[1536] = 13;
    bytes[1541..1543].copy_from_slice(&512_u16.to_be_bytes());
    let graph = inspect(&bytes);
    assert_eq!(graph["pages"][2]["classification"]["role"], "conflicting");
    assert!(
        !graph["relationships"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["source"]["pageNumber"] == 3)
    );
    assert_ne!(
        graph["pointerMap"]["pages"][0]["entries"][1]["state"],
        "validated"
    );
    assert!(
        graph["relationshipClaims"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["source"]["pageNumber"] == 3 && c["target"]["pageNumber"] == 4)
    );
    assert!(graph["traversals"].as_array().unwrap().iter().all(|t| {
        !t["validatedPrefix"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["pageNumber"] == 3)
    }));
}

#[test]
fn root_entry_cannot_silently_exceed_the_header_largest_root_claim() {
    let mut bytes = fixture(3, true); // header says largest root is page 1
    bytes[512] = 1; // pointer map says page 3 is another root
    bytes[1024] = 13;
    bytes[1029..1031].copy_from_slice(&512_u16.to_be_bytes());
    let graph = inspect(&bytes);
    assert_eq!(
        graph["pointerMap"]["pages"][0]["entries"][0]["state"],
        "conflicting"
    );
    let finding = graph["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["code"] == "pointer_map_root_above_largest_root")
        .unwrap();
    assert!(
        finding["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["range"]["fileOffset"] == 52)
    );
    assert!(
        finding["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["range"]["fileOffset"] == 512)
    );
}

#[test]
fn lost_overflow_owner_support_propagates_through_the_entire_chain() {
    let mut bytes = fixture(5, true);
    for (offset, kind, parent) in [(512, 2, 0), (517, 3, 3), (522, 4, 4)] {
        bytes[offset] = kind;
        put(&mut bytes, offset + 1, parent);
    }
    bytes[1024] = 13;
    bytes[1027..1029].copy_from_slice(&1_u16.to_be_bytes());
    bytes[1029..1031].copy_from_slice(&466_u16.to_be_bytes());
    bytes[1032..1034].copy_from_slice(&466_u16.to_be_bytes());
    bytes[1490..1493].copy_from_slice(&[0x87, 0x68, 1]);
    put(&mut bytes, 1532, 4);
    put(&mut bytes, 1536, 5);
    let graph = inspect(&bytes);
    for page in [3, 4] {
        assert_eq!(graph["pages"][page]["classification"]["role"], "unknown");
    }
    assert!(
        !graph["relationships"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["kind"] == "overflow")
    );
    let overflow = graph["traversals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["kind"] == "overflow")
        .unwrap();
    assert_eq!(overflow["validatedPrefix"], serde_json::json!([]));
    assert_eq!(overflow["stop"]["reason"], "conflicting_claim");
    for entry in graph["pointerMap"]["pages"][0]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .skip(1)
    {
        assert_eq!(entry["state"], "unresolved");
    }
}
