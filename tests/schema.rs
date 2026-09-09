use rusqlite::Connection;
use serde_json::Value;
use tempfile::TempDir;
use volmap_sqlite::inspection::InspectionSession;

fn fixture(sql: &str) -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("schema.sqlite");
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("PRAGMA page_size=512;").unwrap();
    connection.execute_batch(sql).unwrap();
    drop(connection);
    (dir, path)
}

fn inspect(path: &std::path::Path) -> Value {
    serde_json::to_value(&*InspectionSession::open(path).unwrap().graph().unwrap()).unwrap()
}

fn object<'a>(graph: &'a Value, name: &str) -> &'a Value {
    graph["schema"]["objects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|object| object["name"] == name)
        .unwrap()
}

#[test]
fn attributes_schema_declarations_to_physical_btrees_without_disclosing_rows() {
    let (_dir, path) = fixture(
        "CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT);
        CREATE INDEX by_value ON items(value);
        INSERT INTO items VALUES(1, 'PRIVATE_APPLICATION_VALUE');",
    );
    let graph = inspect(&path);
    assert_eq!(graph["schema"]["state"], "complete");
    let table = object(&graph, "items");
    assert_eq!(table["objectType"], "table");
    assert_eq!(table["tableName"], "items");
    assert_eq!(table["rootPage"], "2");
    assert_eq!(table["root"]["pageNumber"], 2);
    assert_eq!(table["pages"], serde_json::json!([{ "pageNumber": 2 }]));
    assert_eq!(table["state"], "complete");
    assert_eq!(
        table["declaration"],
        "CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT)"
    );
    assert_eq!(object(&graph, "by_value")["root"]["pageNumber"], 3);
    assert!(!graph.to_string().contains("PRIVATE_APPLICATION_VALUE"));
}

#[test]
fn decodes_overflow_declarations_and_utf_encodings_from_schema_descendants() {
    for encoding in ["UTF-8", "UTF-16le", "UTF-16be"] {
        let sql = format!(
            "PRAGMA encoding='{encoding}'; CREATE TABLE \"長い表\"(value TEXT CHECK(length(value) < 99999) /*{}*/);",
            "declaration ".repeat(150)
        );
        let (_dir, path) = fixture(&sql);
        let connection = Connection::open(&path).unwrap();
        for index in 0..40 {
            connection
                .execute_batch(&format!("CREATE TABLE t{index}(value);"))
                .unwrap();
        }
        drop(connection);
        let graph = inspect(&path);
        let object = object(&graph, "長い表");
        assert_eq!(object["state"], "complete", "{encoding}: {object}");
        assert_eq!(
            object["declaration"],
            sql.split_once("; ").unwrap().1.trim_end_matches(';')
        );
        assert!(object["evidence"].as_array().unwrap().len() > 1);
        assert_eq!(graph["schema"]["objects"].as_array().unwrap().len(), 41);
        assert!(
            graph["schema"]["objects"]
                .as_array()
                .unwrap()
                .iter()
                .any(|object| object["identity"]["pageNumber"] != 1)
        );
    }
}

#[test]
fn keeps_rootless_objects_and_shadow_storage_visible_without_executing_declarations() {
    let (_dir, path) = fixture("CREATE TABLE storage(id TEXT PRIMARY KEY, value) WITHOUT ROWID;
        CREATE INDEX storage_value ON storage(value);
        CREATE TABLE ghost_data(value);
        CREATE VIEW dangerous AS SELECT absent_function(value) FROM ghost_data;
        CREATE TRIGGER storage AFTER INSERT ON ghost_data BEGIN SELECT absent_function(new.value); END;
        PRAGMA writable_schema=ON;
        INSERT INTO sqlite_schema VALUES('table','ghost','ghost',0,'CREATE /* retained comment */ VIRTUAL TABLE ghost USING absent_module(value)');
        PRAGMA writable_schema=OFF;");
    let before = std::fs::read(&path).unwrap();
    let graph = inspect(&path);
    for name in ["dangerous", "ghost"] {
        let object = object(&graph, name);
        assert_eq!(object["state"], "declaration_only", "{object}");
        assert!(object["root"].is_null());
        assert_eq!(object["pages"], serde_json::json!([]));
    }
    let objects = graph["schema"]["objects"].as_array().unwrap();
    let duplicates: Vec<_> = objects.iter().filter(|o| o["name"] == "storage").collect();
    assert_eq!(duplicates.len(), 2);
    assert_ne!(duplicates[0]["identity"], duplicates[1]["identity"]);
    assert_eq!(duplicates[0]["state"], "complete");
    assert_eq!(duplicates[1]["state"], "declaration_only");
    assert_eq!(object(&graph, "ghost_data")["state"], "complete");
    assert_eq!(object(&graph, "storage_value")["state"], "complete");
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn contains_invalid_and_duplicate_root_claims_without_changing_physical_evidence() {
    let (_dir, path) = fixture("CREATE TABLE valid(value); CREATE TABLE duplicate_a(value); CREATE TABLE duplicate_b(value);
        CREATE TABLE negative(value); CREATE TABLE missing(value); CREATE TABLE schema_root(value);
        CREATE TABLE rootless(value); CREATE TABLE fake_virtual(value);
        PRAGMA writable_schema=ON;
        UPDATE sqlite_schema SET rootpage=(SELECT rootpage FROM sqlite_schema WHERE name='duplicate_a') WHERE name='duplicate_b';
        UPDATE sqlite_schema SET rootpage=-2 WHERE name='negative';
        UPDATE sqlite_schema SET rootpage=99999 WHERE name='missing';
        UPDATE sqlite_schema SET rootpage=1 WHERE name='schema_root';
        UPDATE sqlite_schema SET rootpage=0 WHERE name='rootless';
        UPDATE sqlite_schema SET sql='CREATE VIRTUAL TABLE fake_virtual USING absent_module(value)' WHERE name='fake_virtual';
        PRAGMA writable_schema=OFF;");
    let graph = inspect(&path);
    for name in [
        "duplicate_a",
        "duplicate_b",
        "negative",
        "missing",
        "schema_root",
        "rootless",
        "fake_virtual",
    ] {
        let object = object(&graph, name);
        assert_eq!(object["state"], "unavailable", "{name}: {object}");
        assert!(object["root"].is_null());
        assert!(object["pages"].as_array().unwrap().is_empty());
        assert!(!object["diagnostics"].as_array().unwrap().is_empty());
    }
    assert_eq!(object(&graph, "negative")["rootPage"], "-2");
    assert_eq!(object(&graph, "valid")["state"], "complete");
    let session = InspectionSession::begin_with_schema_budget(
        &path,
        volmap_sqlite::inspection::TraversalBudget::default(),
        volmap_sqlite::inspection::SidecarBudget::default(),
        volmap_sqlite::inspection::SchemaBudget {
            max_decoded_bytes: 0,
        },
    )
    .unwrap();
    session
        .scan(|_| volmap_sqlite::inspection::ScanControl::Continue)
        .unwrap();
    let disabled = serde_json::to_value(&*session.graph().unwrap()).unwrap();
    for field in [
        "pages",
        "relationships",
        "relationshipClaims",
        "traversals",
        "diagnostics",
        "freelist",
        "pointerMap",
    ] {
        assert_eq!(graph[field], disabled[field], "physical {field}");
    }
    assert_eq!(disabled["schema"]["state"], "partial");
    assert_eq!(
        disabled["schema"]["diagnostics"],
        serde_json::json!(["schema_decode_budget"])
    );
    assert!(disabled["schema"]["stoppingCell"].is_object());
}

#[test]
fn contains_damaged_records_and_retains_independent_declarations() {
    let (_dir, path) = fixture("CREATE TABLE damaged(value); CREATE TABLE intact(value);");
    let before = inspect(&path);
    let evidence = &object(&before, "damaged")["evidence"][0]["range"];
    let start = usize::try_from(evidence["fileOffset"].as_u64().unwrap()).unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    // The first serial type describes 'table'. Making it reserved breaks this
    // record's boundary without changing its cell extent or the other record.
    bytes[start + 1] = 10;
    std::fs::write(&path, bytes).unwrap();
    let graph = inspect(&path);
    assert_eq!(graph["schema"]["state"], "partial");
    let damaged = &graph["schema"]["objects"][0];
    assert_eq!(damaged["identity"], object(&before, "damaged")["identity"]);
    assert_eq!(damaged["state"], "unavailable");
    assert!(damaged["name"].is_null());
    assert!(!damaged["diagnostics"].as_array().unwrap().is_empty());
    assert_eq!(object(&graph, "intact")["state"], "complete");
    assert_eq!(before["pages"][1], graph["pages"][1]);
    assert_eq!(before["pages"][2], graph["pages"][2]);
}

#[test]
fn follows_validated_descendants_and_stops_at_a_btree_budget() {
    let (_dir, path) = fixture(
        "CREATE TABLE large(value TEXT);
        WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<300)
        INSERT INTO large SELECT printf('%0100d',x) FROM n;",
    );
    let full = inspect(&path);
    let table = object(&full, "large");
    assert_eq!(table["root"]["pageNumber"], 2);
    assert!(table["pages"].as_array().unwrap().len() > 20);
    assert_eq!(table["state"], "complete");
    let session = InspectionSession::open_with_traversal_budget(
        &path,
        volmap_sqlite::inspection::TraversalBudget::new(1, 100),
    )
    .unwrap();
    let partial = serde_json::to_value(&*session.graph().unwrap()).unwrap();
    let table = object(&partial, "large");
    assert_eq!(table["state"], "partial");
    assert_eq!(table["pages"], serde_json::json!([{ "pageNumber": 2 }]));
    assert_eq!(partial["pages"], full["pages"]);
}

#[test]
fn rejects_descendant_pages_as_roots_and_preserves_invalid_integer_claims() {
    let (_dir, path) = fixture(
        "CREATE TABLE large(value TEXT); CREATE TABLE wrong(value);
        WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<60)
        INSERT INTO large SELECT printf('%0100d',x) FROM n;",
    );
    let initial = inspect(&path);
    let child = object(&initial, "large")["pages"][1]["pageNumber"]
        .as_u64()
        .unwrap();
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch(&format!("PRAGMA writable_schema=ON; UPDATE sqlite_schema SET rootpage={child} WHERE name='wrong';")).unwrap();
    drop(connection);
    let graph = inspect(&path);
    assert_eq!(object(&graph, "wrong")["state"], "unavailable");
    assert_eq!(object(&graph, "large")["state"], "complete");
}

#[test]
fn never_decodes_a_schema_overflow_chain_past_a_validation_stop() {
    let (_dir, path) = fixture(&format!(
        "CREATE TABLE long(value /*{}*/); CREATE TABLE intact(value);",
        "comment ".repeat(250)
    ));
    let before = inspect(&path);
    let long = object(&before, "long");
    let page = long["evidence"][1]["page"]["pageNumber"].as_u64().unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    let offset = usize::try_from((page - 1) * 512).unwrap();
    bytes[offset..offset + 4].copy_from_slice(&99999_u32.to_be_bytes());
    std::fs::write(&path, bytes).unwrap();
    let graph = inspect(&path);
    assert_eq!(graph["schema"]["state"], "partial");
    assert_eq!(graph["schema"]["objects"][0]["state"], "unavailable");
    assert_eq!(object(&graph, "intact")["state"], "complete");
    assert!(!graph.to_string().contains("comment comment"));
}

#[test]
fn preserves_real_fts_shadow_tables_when_the_module_is_unavailable() {
    let (_dir, path) = fixture("CREATE VIRTUAL TABLE search USING fts5(body);
        INSERT INTO search VALUES('PRIVATE_FTS_BODY');
        PRAGMA writable_schema=ON;
        UPDATE sqlite_schema SET sql='CREATE VIRTUAL TABLE search USING unavailable_fts(body)' WHERE name='search';
        PRAGMA writable_schema=OFF;");
    let graph = inspect(&path);
    assert_eq!(object(&graph, "search")["state"], "declaration_only");
    for name in [
        "search_data",
        "search_idx",
        "search_content",
        "search_docsize",
        "search_config",
    ] {
        assert_eq!(object(&graph, name)["state"], "complete", "{name}");
    }
    assert!(!graph.to_string().contains("PRIVATE_FTS_BODY"));
}

#[test]
fn cancellation_at_schema_preprocessing_publishes_explicit_partial_metadata() {
    let (_dir, path) = fixture("CREATE TABLE item(value);");
    let session = InspectionSession::begin(&path).unwrap();
    let mut reached_schema = false;
    session
        .scan(|status| {
            let progress = serde_json::to_value(&status.progress).unwrap();
            if progress["buildingSchema"] == true {
                reached_schema = true;
                volmap_sqlite::inspection::ScanControl::Cancel
            } else {
                volmap_sqlite::inspection::ScanControl::Continue
            }
        })
        .unwrap();
    assert!(reached_schema);
    let graph = serde_json::to_value(&*session.graph().unwrap()).unwrap();
    assert_eq!(graph["schema"]["state"], "partial");
    assert_eq!(graph["schema"]["decodedBytes"], "0");
    assert_eq!(
        graph["schema"]["diagnostics"],
        serde_json::json!(["schema_cancelled"])
    );
    assert_eq!(graph["pages"].as_array().unwrap().len(), 2);
}

#[test]
fn cancellation_and_operator_stop_interrupt_schema_preprocessing_after_it_starts() {
    use volmap_sqlite::inspection::ScanControl;
    let (_dir, path) = fixture(
        "CREATE TABLE large(value);
        WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000)
        INSERT INTO large SELECT printf('%0100d',x) FROM n;",
    );
    for (control, reason) in [
        (ScanControl::Cancel, "schema_cancelled"),
        (ScanControl::Stop, "schema_operator_stop"),
    ] {
        let session = InspectionSession::begin(&path).unwrap();
        let mut checkpoints = 0;
        session
            .scan(|status| {
                if status.progress.building_schema {
                    checkpoints += 1;
                }
                if checkpoints == 2 {
                    control
                } else {
                    ScanControl::Continue
                }
            })
            .unwrap();
        assert_eq!(checkpoints, 2);
        let graph = serde_json::to_value(&*session.graph().unwrap()).unwrap();
        assert_eq!(graph["schema"]["decodedBytes"], "0");
        assert_eq!(graph["schema"]["diagnostics"], serde_json::json!([reason]));
        assert!(graph["schema"]["objects"].as_array().unwrap().is_empty());
    }
}
