use std::sync::Arc;

use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use volmap_sqlite::{
    inspection::{InspectionSession, ScanControl},
    terminal::TerminalFlow,
    web::atlas_router,
};

#[tokio::test]
async fn unreadable_standard_geometry_preserves_readable_nonstandard_content_as_opaque() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("main.sqlite");
    std::fs::write(&path, [0xa5; 512]).unwrap();
    let session = Arc::new(InspectionSession::begin(&path).unwrap());
    assert!(session.scan(|_| ScanControl::Continue).is_err());
    let status = serde_json::to_value(session.status()).unwrap();
    assert_eq!(status["diagnostic"]["code"], "unsupported_format");
    assert_eq!(
        status["diagnostic"]["opaqueRange"],
        json!({"fileOffset": "0", "length": "512"})
    );
    assert_eq!(status["coverage"]["total"], Value::Null);
    assert!(session.graph().is_err());
    let response = atlas_router(Arc::clone(&session))
        .oneshot(
            Request::builder()
                .uri(format!("/api/snapshots/{}", session.status().snapshot_id))
                .header("host", "localhost")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let web: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1_000_000).await.unwrap()).unwrap();
    assert_eq!(web["diagnostic"], status["diagnostic"]);
    let mut terminal = TerminalFlow::new(session);
    let screen = terminal.screen(240, 80).join("\n");
    assert!(screen.contains("unsupported_format"));
    assert!(screen.contains("Opaque input: offset 0; length 512 bytes"));
}

#[tokio::test]
async fn unsupported_page_content_retains_an_opaque_extent_without_decoding() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("main.sqlite");
    let mut bytes = vec![0; 1024];
    bytes[..16].copy_from_slice(b"SQLite format 3\0");
    bytes[16..18].copy_from_slice(&512_u16.to_be_bytes());
    bytes[18..20].copy_from_slice(&[1, 1]);
    bytes[21..24].copy_from_slice(&[64, 32, 32]);
    bytes[28..32].copy_from_slice(&2_u32.to_be_bytes());
    bytes[44..48].copy_from_slice(&4_u32.to_be_bytes());
    bytes[56..60].copy_from_slice(&1_u32.to_be_bytes());
    bytes[100] = 13;
    bytes[105..107].copy_from_slice(&512_u16.to_be_bytes());
    bytes[512..].fill(0xa5);
    std::fs::write(&path, &bytes).unwrap();
    let session = Arc::new(InspectionSession::begin(&path).unwrap());
    session.scan(|_| ScanControl::Continue).unwrap();
    let graph = serde_json::to_value(&*session.graph().unwrap()).unwrap();
    let page = &graph["pages"][1]["detail"];
    assert_eq!(page["coverage"], "unsupported");
    assert_eq!(page["diagnostics"], json!([]));
    assert!(page["regions"].as_array().unwrap().contains(&json!({
        "kind": "opaque_content", "range": {"pageOffset": 0, "fileOffset": 512, "length": 512}
    })));
    let response = atlas_router(Arc::clone(&session))
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/snapshots/{}/revisions/1",
                    session.status().snapshot_id
                ))
                .header("host", "localhost")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let web: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1_000_000).await.unwrap()).unwrap();
    assert_eq!(web, graph);
    let mut terminal = TerminalFlow::new(session);
    assert!(!terminal.screen(120, 40).is_empty());
}

#[path = "support/hostile.rs"]
mod hostile;

#[test]
fn helper_reply_decoder_rejects_oversize_and_unrecognized_protocol_data() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("main.sqlite");
    std::fs::write(&path, include_bytes!("corpus/damage/opaque-page.sqlite")).unwrap();
    let session = hostile::begin(&path);
    session.scan(|_| ScanControl::Continue).unwrap();
    let graph = session.graph().unwrap();
    let valid = br#"{"version":1,"queryMask":31,"tables":[]}"#;
    assert!(volmap_sqlite::semantic::decode_helper_reply(&graph.schema, valid).is_some());
    let mut oversized = vec![b' '; 256 * 1024];
    oversized.extend_from_slice(valid);
    assert!(volmap_sqlite::semantic::decode_helper_reply(&graph.schema, &oversized).is_none());
    for bytes in [
        b"RAW_HIDE".as_slice(),
        br#"{"version":1,"queryMask":31,"tables":[],"sql":"RAW_HIDE"}"#,
    ] {
        assert!(volmap_sqlite::semantic::decode_helper_reply(&graph.schema, bytes).is_none());
    }
}

#[tokio::test]
async fn damage_corpus_preserves_boundaries_through_session_and_both_adapters() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/damage");
    let cases: Value =
        serde_json::from_slice(&std::fs::read(root.join("manifest.json")).unwrap()).unwrap();
    for case in cases.as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        eprintln!("damage case: {name}");
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.sqlite");
        let bytes = std::fs::read(root.join(case["file"].as_str().unwrap())).unwrap();
        assert!(bytes.len() <= hostile::MAX_INPUT);
        std::fs::write(&path, &bytes).unwrap();
        for (suffix, file) in case["sidecars"].as_object().unwrap() {
            std::fs::copy(
                root.join(file.as_str().unwrap()),
                directory.path().join(format!("main.sqlite-{suffix}")),
            )
            .unwrap();
        }
        let session = hostile::begin(&path);
        let result = session.scan(|_| ScanControl::Continue);
        assert_eq!(result.is_err(), case["fatal"].as_bool().unwrap(), "{name}");
        let document = hostile::adapters(&session, directory.path()).await;
        hostile::expectations(&document, &case["checks"], name);
        if case["fatal"].as_bool().unwrap() {
            assert_eq!(
                document["status"]["diagnostic"]["opaqueRange"],
                json!({"fileOffset":"0","length":bytes.len().to_string()})
            );
        }
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        if !case["fatal"].as_bool().unwrap() {
            assert_eq!(document["coverage"]["reason"], "complete", "{name}");
            assert_eq!(
                document["coverage"]["evaluated"],
                bytes.len() / 512,
                "{name}"
            );
            // The same damaged main file without sidecars has identical physical facts.
            let clean = tempfile::tempdir().unwrap();
            let clean_path = clean.path().join("main.sqlite");
            std::fs::write(&clean_path, &bytes).unwrap();
            let plain = hostile::begin(&clean_path);
            plain.scan(|_| ScanControl::Continue).unwrap();
            assert_eq!(
                hostile::physical(document.clone()),
                hostile::physical(serde_json::to_value(&*plain.graph().unwrap()).unwrap()),
                "{name}"
            );
            // Previously returned revisions stay immutable after input invalidation.
            let retained = session.graph().unwrap();
            let before = serde_json::to_value(&*retained).unwrap();
            let mut changed = bytes.clone();
            changed[0] ^= 1;
            std::fs::write(&path, changed).unwrap();
            assert!(session.graph().is_err());
            assert_eq!(serde_json::to_value(&*retained).unwrap(), before);
            assert_eq!(hostile::get(&session, "/revisions/1").await.0, 409);
            let mut terminal = TerminalFlow::new(Arc::clone(&session));
            assert!(terminal.screen(240, 80).join("\n").contains("Invalidated"));
        }
    }
}

#[tokio::test]
async fn damaged_sibling_and_error_responses_cannot_disclose_unselected_values_or_blob_bytes() {
    use volmap_sqlite::inspection::{DeepBudget, DeepSelector, DeepState};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("main.sqlite");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("PRAGMA page_size=512; CREATE TABLE t(value,payload); INSERT INTO t VALUES('SELECTED_VALUE',x'5241575f48494445'),('OTHER_VALUE',x'5241575f48494445');").unwrap();
    drop(connection);
    let original = InspectionSession::open(&path).unwrap();
    let original_graph = original.graph().unwrap();
    let pointer =
        usize::try_from(original_graph.pages[1].detail.cells[1].pointer.file_offset).unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[pointer..pointer + 2].copy_from_slice(&7_u16.to_be_bytes());
    std::fs::write(&path, bytes).unwrap();
    let session = hostile::begin(&path);
    session.scan(|_| ScanControl::Continue).unwrap();
    let old = session.graph().unwrap();
    assert_eq!(
        old.pages[1].detail.cells[1].diagnostic.as_deref(),
        Some("invalid_cell_pointer")
    );
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
    let result = serde_json::to_value(session.deep_result(&job.id, &target).unwrap()).unwrap();
    assert_eq!(result["values"][0]["value"]["value"], "SELECTED_VALUE");
    assert_eq!(
        result["values"][1]["value"],
        json!({"type":"blob","byteLength":"8"})
    );
    for forbidden in ["OTHER_VALUE", "RAW_HIDE", "5241575f48494445"] {
        assert!(!result.to_string().contains(forbidden));
    }
    let mut other = target.clone();
    other.cell_index = 1;
    assert!(session.deep_result(&job.id, &other).is_err());
    assert_eq!(old.revision, 1);
    for suffix in ["", "/revisions/1", "/revisions/2", "/revisions/2/metadata"] {
        let (_, body) = hostile::get(&session, suffix).await;
        for forbidden in [
            "SELECTED_VALUE",
            "OTHER_VALUE",
            "RAW_HIDE",
            "5241575f48494445",
        ] {
            assert!(!body.to_string().contains(forbidden));
        }
    }
    let mut terminal = TerminalFlow::new(Arc::clone(&session));
    hostile::go(&mut terminal, "2:1");
    let frame = terminal.screen(240, 100).join("\n");
    for forbidden in ["SELECTED_VALUE", "OTHER_VALUE", "RAW_HIDE"] {
        assert!(!frame.contains(forbidden));
    }
    let response = atlas_router(Arc::clone(&session))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/snapshots/{}/deep-inspections",
                    session.status().snapshot_id
                ))
                .header("host", "localhost")
                .header("origin", "http://localhost")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"target":{"pageNumber":"RAW_HIDE OTHER_VALUE"}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(response.status().is_client_error());
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    assert!(!String::from_utf8_lossy(&body).contains("RAW_HIDE"));
    assert!(!String::from_utf8_lossy(&body).contains("OTHER_VALUE"));
}
