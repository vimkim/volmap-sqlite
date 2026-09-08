use rusqlite::Connection;
use serde_json::{Value, json};
use tempfile::tempdir;
use volmap_sqlite::inspection::InspectionSession;

#[test]
fn table_leaf_structure_and_page_one_coordinates_do_not_disclose_values() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("leaf.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA page_size=512; CREATE TABLE t(value TEXT); INSERT INTO t(rowid,value) VALUES(-7,'PRIVATE-PAYLOAD');").unwrap();
    drop(db);
    let session = InspectionSession::open(&path).unwrap();
    let graph = serde_json::to_value(&*session.graph().unwrap()).unwrap();
    let first = &graph["pages"][0]["detail"];
    assert_eq!(first["kind"], "table_leaf");
    assert_eq!(
        first["header"]["range"],
        json!({"pageOffset":100,"fileOffset":100,"length":8})
    );
    assert_eq!(first["regions"][0]["kind"], "database_header");
    let leaf = &graph["pages"][1]["detail"];
    assert_eq!(leaf["kind"], "table_leaf");
    assert_eq!(leaf["header"]["range"]["fileOffset"], 512);
    let cell = &leaf["cells"][0];
    assert_eq!(cell["identity"], json!({"pageNumber":2,"index":0}));
    assert_eq!(cell["rowid"], "-7");
    assert_eq!(cell["record"]["serialTypes"], json!(["43"]));
    assert_eq!(cell["record"]["state"], "complete");
    assert_eq!(cell["range"]["length"], 27);
    assert_eq!(leaf["diagnostics"], json!([]));
    assert!(!graph.to_string().contains("PRIVATE-PAYLOAD"));
}

#[test]
fn all_four_btree_kinds_have_bounded_cells_across_sizes_and_encodings() {
    for (size, encoding) in [(512, "UTF-8"), (1024, "UTF-16le"), (4096, "UTF-16be")] {
        let dir = tempdir().unwrap();
        let path = dir.path().join("trees.sqlite");
        let mut db = Connection::open(&path).unwrap();
        db.execute_batch(&format!("PRAGMA page_size={size}; PRAGMA encoding='{encoding}'; CREATE TABLE t(value TEXT); CREATE INDEX idx ON t(value);")).unwrap();
        let tx = db.transaction().unwrap();
        for index in 0..1500 {
            tx.execute(
                "INSERT INTO t VALUES(?1)",
                [format!("PRIVATE-{index:04}-{}", "x".repeat(80))],
            )
            .unwrap();
        }
        tx.commit().unwrap();
        drop(db);
        let session = InspectionSession::open(&path).unwrap();
        let graph = serde_json::to_value(&*session.graph().unwrap()).unwrap();
        for kind in [
            "table_leaf",
            "table_interior",
            "index_leaf",
            "index_interior",
        ] {
            let pages: Vec<_> = graph["pages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|page| page["detail"]["kind"] == kind)
                .collect();
            assert!(!pages.is_empty(), "{kind} at {size}");
            for page in pages {
                assert_eq!(page["detail"]["diagnostics"], json!([]), "{page}");
                for cell in page["detail"]["cells"].as_array().unwrap() {
                    assert!(cell["range"].is_object(), "{cell}");
                    assert!(cell["diagnostic"].is_null(), "{cell}");
                    if kind != "table_interior" {
                        assert_eq!(cell["record"]["state"], "complete", "{cell}");
                    }
                }
            }
        }
        assert!(!graph.to_string().contains("PRIVATE-"));
    }
}

// Hand-authored main-file image: two 512-byte pages, two 4-byte table cells.
// Each record is [header-size=2, serial-type=8] (integer zero, no body).
fn small_image() -> Vec<u8> {
    let mut bytes = vec![0; 1024];
    bytes[..16].copy_from_slice(b"SQLite format 3\0");
    bytes[16..18].copy_from_slice(&512_u16.to_be_bytes());
    bytes[28..32].copy_from_slice(&2_u32.to_be_bytes());
    bytes[44..48].copy_from_slice(&4_u32.to_be_bytes());
    bytes[56..60].copy_from_slice(&1_u32.to_be_bytes());
    bytes[100] = 13;
    bytes[105..107].copy_from_slice(&512_u16.to_be_bytes());
    bytes[512] = 13;
    bytes[515..517].copy_from_slice(&2_u16.to_be_bytes());
    bytes[517..519].copy_from_slice(&504_u16.to_be_bytes());
    bytes[520..524].copy_from_slice(&[1, 248, 1, 252]);
    bytes[1016..1024].copy_from_slice(&[2, 1, 2, 8, 2, 2, 2, 8]);
    bytes
}

fn inspect_bytes(bytes: &[u8]) -> Value {
    let dir = tempdir().unwrap();
    let path = dir.path().join("fixture.sqlite");
    std::fs::write(&path, bytes).unwrap();
    let session = InspectionSession::open(&path).unwrap();
    serde_json::to_value(&*session.graph().unwrap()).unwrap()
}

#[test]
fn overlapping_cells_lose_dependent_facts_but_keep_physical_identities() {
    let mut bytes = small_image();
    bytes[522..524].copy_from_slice(&504_u16.to_be_bytes());
    let graph = inspect_bytes(&bytes);
    let page = &graph["pages"][1]["detail"];
    assert_eq!(page["kind"], "table_leaf");
    assert_eq!(page["header"]["cellCount"], 2);
    for (index, cell) in page["cells"].as_array().unwrap().iter().enumerate() {
        assert_eq!(cell["identity"]["index"], index);
        assert_eq!(cell["diagnostic"], "overlapping_allocation");
        assert!(cell["range"].is_null());
        assert!(cell["record"].is_null());
        assert!(cell["rowid"].is_null());
    }
    assert_eq!(page["coverage"], "partial");
    assert_eq!(graph["pages"][0]["detail"]["coverage"], "complete");
}

#[test]
fn malformed_cell_boundaries_preserve_independent_siblings() {
    for (offset, replacement, diagnostic) in [
        (520, vec![0, 7], "invalid_cell_pointer"),
        (522, vec![1, 255], "truncated_varint"),
        (1020, vec![127], "invalid_cell_extent"),
        (1022, vec![3], "invalid_record"),
        (1023, vec![127], "invalid_record"),
    ] {
        let mut bytes = small_image();
        bytes[offset..offset + replacement.len()].copy_from_slice(&replacement);
        if diagnostic == "truncated_varint" {
            bytes[1023] = 0x80;
        }
        let graph = inspect_bytes(&bytes);
        let page = &graph["pages"][1]["detail"];
        let damaged = usize::from(offset != 520);
        assert_eq!(page["cells"][damaged]["diagnostic"], diagnostic);
        assert_eq!(page["cells"][1 - damaged]["record"]["state"], "complete");
        assert_eq!(page["coverage"], "partial");
    }
}

#[test]
fn freeblock_chain_and_fragments_are_bounded_allocation_evidence() {
    let mut bytes = small_image();
    // Move the first cell to 491, leave a 1-byte fragment then two linked blocks.
    bytes[517..519].copy_from_slice(&491_u16.to_be_bytes());
    bytes[520..522].copy_from_slice(&491_u16.to_be_bytes());
    bytes[1003..1007].copy_from_slice(&[2, 1, 2, 8]);
    bytes[513..515].copy_from_slice(&496_u16.to_be_bytes());
    bytes[519] = 1;
    bytes[1008..1012].copy_from_slice(&[1, 248, 0, 8]);
    bytes[1016..1020].copy_from_slice(&[0, 0, 0, 4]);
    let graph = inspect_bytes(&bytes);
    let page = &graph["pages"][1]["detail"];
    assert_eq!(page["coverage"], "complete");
    assert_eq!(page["freeblocks"].as_array().unwrap().len(), 2);
    assert_eq!(page["freeblocks"][0]["range"]["length"], 8);
    assert!(page["regions"].as_array().unwrap().contains(
        &json!({"kind":"fragment","range":{"pageOffset":495,"fileOffset":1007,"length":1}})
    ));
    // A backward link preserves the first block but stops the chain there.
    bytes[1008..1010].copy_from_slice(&496_u16.to_be_bytes());
    let graph = inspect_bytes(&bytes);
    let page = &graph["pages"][1]["detail"];
    assert_eq!(page["freeblocks"].as_array().unwrap().len(), 1);
    assert_eq!(page["diagnostics"], json!(["invalid_freeblock_link"]));
    assert_eq!(page["cells"][1]["record"]["state"], "complete");
    assert_eq!(page["coverage"], "partial");
}

#[test]
fn overflow_spanning_record_header_is_partial_not_malformed_or_decoded() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wide.sqlite");
    let db = Connection::open(&path).unwrap();
    let columns = (0..600)
        .map(|n| format!("c{n}"))
        .collect::<Vec<_>>()
        .join(",");
    let values = vec!["0"; 600].join(",");
    db.execute_batch(&format!(
        "PRAGMA page_size=512; CREATE TABLE t({columns}); INSERT INTO t VALUES({values});"
    ))
    .unwrap();
    drop(db);
    let session = InspectionSession::open(&path).unwrap();
    let graph = serde_json::to_value(&*session.graph().unwrap()).unwrap();
    let page = &graph["pages"][1]["detail"];
    let cell = &page["cells"][0];
    assert_eq!(cell["payloadSize"], 602);
    assert_eq!(cell["localPayload"]["length"], 94);
    assert_eq!(cell["record"]["headerSize"], 602);
    assert_eq!(cell["record"]["state"], "needs_overflow");
    assert!(cell["diagnostic"].is_null());
    assert!(cell["range"].is_object());
    assert_eq!(page["coverage"], "partial");
}

#[test]
fn empty_64k_page_and_opaque_reserved_region_keep_distinct_coordinates() {
    let mut bytes = small_image()[..512].to_vec();
    bytes.resize(65_536, 0);
    bytes[16..18].copy_from_slice(&1_u16.to_be_bytes());
    bytes[28..32].copy_from_slice(&1_u32.to_be_bytes());
    bytes[105..107].copy_from_slice(&0_u16.to_be_bytes());
    let graph = inspect_bytes(&bytes);
    assert_eq!(
        graph["pages"][0]["detail"]["header"]["contentStart"],
        65_536
    );
    assert_eq!(graph["pages"][0]["detail"]["coverage"], "complete");
    bytes[20] = 16;
    bytes[105..107].copy_from_slice(&65_520_u16.to_be_bytes());
    bytes[65_520..].copy_from_slice(b"PRIVATE-RESERVED");
    let graph = inspect_bytes(&bytes);
    let regions = graph["pages"][0]["detail"]["regions"].as_array().unwrap();
    assert!(regions.contains(&json!({"kind":"opaque_reserved","range":{"pageOffset":65520,"fileOffset":65520,"length":16}})));
    assert!(!graph.to_string().contains("PRIVATE-RESERVED"));
}

#[test]
fn invalid_headers_do_not_enable_dependent_parsing() {
    for (offset, value) in [(515, vec![255, 255]), (517, vec![0, 0]), (519, vec![61])] {
        let mut bytes = small_image();
        bytes[offset..offset + value.len()].copy_from_slice(&value);
        let graph = inspect_bytes(&bytes);
        let page = &graph["pages"][1]["detail"];
        assert!(page["kind"].is_null());
        assert!(page["header"].is_null());
        assert_eq!(page["cells"], json!([]));
        assert_eq!(page["diagnostics"], json!(["invalid_btree_header"]));
        assert_eq!(graph["pages"][0]["detail"]["coverage"], "complete");
    }
}

#[test]
fn freeblock_damage_is_contained_and_overlap_with_a_cell_is_not_trusted() {
    for (first, size, expected) in [
        (511_u16, 0_u16, "invalid_freeblock_extent"),
        (504, 3, "invalid_freeblock_extent"),
        (504, 9, "invalid_freeblock_extent"),
    ] {
        let mut bytes = small_image();
        bytes[513..515].copy_from_slice(&first.to_be_bytes());
        if first == 504 {
            bytes[1018..1020].copy_from_slice(&size.to_be_bytes());
        }
        let graph = inspect_bytes(&bytes);
        let page = &graph["pages"][1]["detail"];
        assert_eq!(page["diagnostics"][0], expected);
        assert_eq!(page["cells"][1]["record"]["state"], "complete");
    }
    let mut bytes = small_image();
    bytes[513..515].copy_from_slice(&504_u16.to_be_bytes());
    // A valid-looking freeblock overlapping both cell pointer targets.
    bytes[1016..1020].copy_from_slice(&[0, 0, 0, 8]);
    let graph = inspect_bytes(&bytes);
    let page = &graph["pages"][1]["detail"];
    assert!(page["freeblocks"][0]["range"].is_null());
    assert!(page["cells"][1]["range"].is_null());
    assert_eq!(page["cells"][1]["diagnostic"], "overlapping_allocation");
    assert_eq!(page["coverage"], "partial");
}
