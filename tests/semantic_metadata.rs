use rusqlite::Connection;
use serde_json::Value;
use std::io::Write;
use volmap_sqlite::inspection::{InspectionSession, ScanControl};

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--private-semantic-helper") {
        let bytes = std::fs::read("snapshot.sqlite").unwrap();
        if u32::from_be_bytes(bytes[68..72].try_into().unwrap()) == 112 {
            let parent_id = u32::from_be_bytes(bytes[60..64].try_into().unwrap());
            std::fs::write(invocation_marker(parent_id), b"helper invoked").unwrap();
        }
        match u32::from_be_bytes(bytes[68..72].try_into().unwrap()) {
            101 => std::process::exit(1),
            102 => std::process::abort(),
            103 => {
                std::thread::sleep(std::time::Duration::from_secs(10));
            }
            104 => {
                print!("not json");
                std::process::exit(0);
            }
            105 => {
                std::io::stdout().write_all(&vec![b' '; 300_000]).unwrap();
                std::process::exit(0);
            }
            106 => {
                print!("{{\"version\":1,\"queryMask\":31,\"tables\":[]}}");
                std::process::exit(0);
            }
            _ => (),
        }
        std::process::exit(volmap_sqlite::semantic::run_private_helper());
    }
    protocol_checks();
    hostile_schemas();
    ignores_original_wal();
    operational_budgets();
    incomplete_schema_refuses_helper();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT) STRICT; INSERT INTO items VALUES(1,'APPLICATION_SECRET');").unwrap();
    drop(db);
    let session = InspectionSession::begin(&path)
        .unwrap()
        .with_semantic_metadata();
    session.scan(|_| ScanControl::Continue).unwrap();
    let graph = session.graph().unwrap();
    let json = serde_json::to_value(&*graph).unwrap();
    assert_eq!(json["semanticMetadata"]["state"], "available");
    assert_eq!(json["semanticMetadata"]["tables"][0]["columnCount"], 2);
    assert_eq!(json["semanticMetadata"]["tables"][0]["strict"], true);
    assert!(!json.to_string().contains("APPLICATION_SECRET"));
    let plain = InspectionSession::open(&path).unwrap();
    assert_eq!(
        physical(json),
        physical(serde_json::to_value(&*plain.graph().unwrap()).unwrap())
    );
    for mode in 100..=106 {
        Connection::open(&path)
            .unwrap()
            .execute_batch(&format!("PRAGMA application_id={mode}"))
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        let plain = InspectionSession::open(&path).unwrap();
        let enriched = InspectionSession::begin(&path)
            .unwrap()
            .with_semantic_metadata();
        let start = std::time::Instant::now();
        enriched.scan(|_| ScanControl::Continue).unwrap();
        assert!(start.elapsed() < std::time::Duration::from_secs(4));
        let result = serde_json::to_value(&*enriched.graph().unwrap()).unwrap();
        assert_eq!(
            result["semanticMetadata"]["state"],
            if mode == 100 {
                "available"
            } else {
                "unavailable"
            },
            "fault {mode}"
        );
        assert_eq!(
            physical(result),
            physical(serde_json::to_value(&*plain.graph().unwrap()).unwrap())
        );
        assert_eq!(
            selected_values(&std::sync::Arc::new(plain)),
            selected_values(&std::sync::Arc::new(enriched))
        );
        assert_eq!(before, std::fs::read(&path).unwrap());
    }
    println!("semantic metadata: success, bounded failures and disclosure passed");
}

fn physical(mut graph: Value) -> Value {
    graph.as_object_mut().unwrap().remove("semanticMetadata");
    graph["snapshot"]["id"] = Value::Null;
    graph["snapshot"]["source"]["id"] = Value::Null;
    graph
}

fn protocol_checks() {
    use std::process::{Command, Stdio};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("snapshot.sqlite");
    Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE t(a,b); CREATE TABLE keys(k TEXT PRIMARY KEY, v INTEGER GENERATED ALWAYS AS(length(k)) STORED) WITHOUT ROWID;")
        .unwrap();
    let invoke = |input: &[u8]| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_volmap-sqlite"))
            .arg("--private-semantic-helper")
            .current_dir(dir.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        child.wait_with_output().unwrap()
    };
    let output = invoke(b"VOLMAP-SEMANTIC-1\n");
    assert!(output.status.success());
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    let tables = json["tables"].as_array().unwrap();
    assert_eq!(tables.len(), 2);
    assert!(
        tables
            .iter()
            .any(|table| table["without_rowid"] == true && table["column_count"] == 2)
    );
    assert_eq!(
        json["queryMask"], 31,
        "SQLite trace must observe exactly the fixed setup and metadata queries"
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains(dir.path().to_str().unwrap()));
    for input in [
        b"SELECT * FROM t".as_slice(),
        b"VOLMAP-SEMANTIC-2\n",
        b"VOLMAP-SEMANTIC-1\nextra",
    ] {
        let output = invoke(input);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
    std::fs::write(dir.path().join("snapshot.sqlite-wal"), b"hostile sidecar").unwrap();
    let output = invoke(b"VOLMAP-SEMANTIC-1\n");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

fn selected_values(session: &std::sync::Arc<InspectionSession>) -> Value {
    use volmap_sqlite::inspection::{DeepBudget, DeepSelector, DeepState};
    let status = session.status();
    let target = DeepSelector {
        session_id: status.session_id,
        snapshot_id: status.snapshot_id,
        revision: 1,
        page_number: 2,
        cell_index: 0,
    };
    let job = session.request_deep(target.clone(), DeepBudget::default());
    assert_eq!(job.wait().state, DeepState::Completed);
    let result = session.deep_result(&job.id, &target).unwrap();
    serde_json::to_value(result).unwrap()["values"].clone()
}

fn hostile_schemas() {
    let cases = [
        (
            "CREATE VIEW dangerous AS SELECT load_extension('/should-never-open');",
            "unavailable",
        ),
        (
            "CREATE VIRTUAL TABLE dangerous USING fts5(value);",
            "unavailable",
        ),
        (
            "PRAGMA writable_schema=ON; INSERT INTO sqlite_schema VALUES('table','ghost','ghost',0,'CREATE /* comment */ VIRTUAL TABLE ghost USING absent_module(value)');",
            "unavailable",
        ),
        (
            "CREATE TRIGGER dangerous AFTER INSERT ON items BEGIN SELECT load_extension('/should-never-open'); END;",
            "available",
        ),
        (
            "PRAGMA writable_schema=ON; UPDATE sqlite_schema SET sql='unsupported SQL' WHERE name='items';",
            "unavailable",
        ),
    ];
    for (sql, state) in cases {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hostile.sqlite");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE items(value); INSERT INTO items VALUES('APPLICATION_SECRET');",
        )
        .unwrap();
        db.execute_batch(sql).unwrap();
        drop(db);
        let before = std::fs::read(&path).unwrap();
        let plain = InspectionSession::open(&path).unwrap();
        let session = InspectionSession::begin(&path)
            .unwrap()
            .with_semantic_metadata();
        session.scan(|_| ScanControl::Continue).unwrap();
        let graph = serde_json::to_value(&*session.graph().unwrap()).unwrap();
        assert_eq!(graph["semanticMetadata"]["state"], state, "{sql}");
        assert!(!graph.to_string().contains("APPLICATION_SECRET"));
        assert_eq!(
            physical(graph),
            physical(serde_json::to_value(&*plain.graph().unwrap()).unwrap())
        );
        assert_eq!(before, std::fs::read(&path).unwrap());
    }
    // A legitimate schema exceeding the helper's column ceiling is only unavailable metadata.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("limited.sqlite");
    let columns = (0..257)
        .map(|i| format!("c{i}"))
        .collect::<Vec<_>>()
        .join(",");
    Connection::open(&path)
        .unwrap()
        .execute_batch(&format!("CREATE TABLE wide({columns});"))
        .unwrap();
    let session = InspectionSession::begin(&path)
        .unwrap()
        .with_semantic_metadata();
    session.scan(|_| ScanControl::Continue).unwrap();
    assert_eq!(
        session.graph().unwrap().semantic_metadata.state,
        "unavailable"
    );
    assert_eq!(
        physical(serde_json::to_value(&*session.graph().unwrap()).unwrap()),
        physical(
            serde_json::to_value(&*InspectionSession::open(&path).unwrap().graph().unwrap())
                .unwrap()
        )
    );
}

fn ignores_original_wal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE items(a,b); PRAGMA journal_mode=WAL; ALTER TABLE items ADD COLUMN wal_only; INSERT INTO items VALUES('WAL_SECRET',2,3);").unwrap();
    let inputs = [
        path.clone(),
        path.with_extension("sqlite-wal"),
        path.with_extension("sqlite-shm"),
    ];
    let before: Vec<_> = inputs.iter().map(|p| std::fs::read(p).unwrap()).collect();
    let session = InspectionSession::begin(&path)
        .unwrap()
        .with_semantic_metadata();
    session.scan(|_| ScanControl::Continue).unwrap();
    let graph = session.graph().unwrap();
    assert_eq!(graph.semantic_metadata.state, "available");
    assert_eq!(graph.semantic_metadata.tables[0].column_count, 2);
    let json = serde_json::to_value(&*graph).unwrap();
    assert!(!json.to_string().contains("WAL_SECRET"));
    assert!(!json.to_string().contains("wal_only"));
    assert_eq!(
        physical(json),
        physical(
            serde_json::to_value(&*InspectionSession::open(&path).unwrap().graph().unwrap())
                .unwrap()
        )
    );
    for (input, bytes) in inputs.iter().zip(before) {
        assert_eq!(std::fs::read(input).unwrap(), bytes);
    }
    drop(db);
}

fn operational_budgets() {
    use volmap_sqlite::semantic::SemanticBudget;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("budget.sqlite");
    Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE items(a,b);")
        .unwrap();
    let plain = physical(
        serde_json::to_value(&*InspectionSession::open(&path).unwrap().graph().unwrap()).unwrap(),
    );
    for budget in [
        SemanticBudget {
            max_copy_bytes: 0,
            ..SemanticBudget::default()
        },
        SemanticBudget {
            max_schema_records: 0,
            ..SemanticBudget::default()
        },
        SemanticBudget {
            timeout_ms: 0,
            ..SemanticBudget::default()
        },
        SemanticBudget {
            max_output_bytes: 0,
            ..SemanticBudget::default()
        },
    ] {
        let session = InspectionSession::begin(&path)
            .unwrap()
            .with_semantic_metadata_budget(budget);
        session.scan(|_| ScanControl::Continue).unwrap();
        let graph = serde_json::to_value(&*session.graph().unwrap()).unwrap();
        assert_eq!(graph["semanticMetadata"]["state"], "unavailable");
        assert_eq!(physical(graph), plain);
    }
}

fn invocation_marker(parent_id: u32) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("volmap-helper-invocation-{parent_id}"))
}

fn incomplete_schema_refuses_helper() {
    use volmap_sqlite::inspection::{SchemaBudget, SidecarBudget, TraversalBudget};
    use volmap_sqlite::semantic::SemanticBudget;
    let marker = invocation_marker(std::process::id());
    let _ = std::fs::remove_file(&marker);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("incomplete.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch(&format!(
        "CREATE TABLE items(a); PRAGMA application_id=112; PRAGMA user_version={};",
        std::process::id()
    ))
    .unwrap();
    drop(db);
    let session = InspectionSession::begin_with_schema_budget(
        &path,
        TraversalBudget::default(),
        SidecarBudget::default(),
        SchemaBudget {
            max_decoded_bytes: 0,
        },
    )
    .unwrap()
    .with_semantic_metadata_budget(SemanticBudget {
        max_schema_records: 1,
        ..SemanticBudget::default()
    });
    session.scan(|_| ScanControl::Continue).unwrap();
    let invoked = marker.exists();
    let _ = std::fs::remove_file(&marker);
    assert!(
        !invoked,
        "partial direct schema must not launch a helper under a possibly undercounted record budget"
    );
    assert_eq!(
        session.graph().unwrap().semantic_metadata.state,
        "unavailable"
    );
    Connection::open(&path)
        .unwrap()
        .execute_batch("DROP TABLE items; PRAGMA application_id=0;")
        .unwrap();
    let session = InspectionSession::begin(&path)
        .unwrap()
        .with_semantic_metadata_budget(SemanticBudget {
            max_schema_records: 0,
            ..SemanticBudget::default()
        });
    session.scan(|_| ScanControl::Continue).unwrap();
    assert_eq!(
        session.graph().unwrap().semantic_metadata.state,
        "unavailable",
        "zero budget must refuse even an empty schema"
    );
}
