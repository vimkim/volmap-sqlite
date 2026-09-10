use rusqlite::Connection;
use tempfile::TempDir;
use volmap_sqlite::inspection::{InspectionSession, OperationalBudget, ScanControl};

#[test]
fn cell_budget_stops_before_a_page_without_discarding_the_validated_inventory() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("budget.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE t(value); INSERT INTO t VALUES(1),(2),(3);")
        .unwrap();
    drop(db);
    for ceiling in [0, 1, 3, 4] {
        let session = InspectionSession::begin(&path)
            .unwrap()
            .with_operational_budget(OperationalBudget {
                max_processed_cells: ceiling,
                ..OperationalBudget::default()
            });
        session.scan(|_| ScanControl::Continue).unwrap();
        let status = serde_json::to_value(session.status()).unwrap();
        let graph = session.graph().unwrap();
        let expected_pages = if ceiling == 0 {
            0
        } else if ceiling < 4 {
            1
        } else {
            2
        };
        assert_eq!(graph.pages.len(), expected_pages);
        assert_eq!(status["coverage"]["evaluated"], expected_pages);
        assert_eq!(status["coverage"]["remainder"], 2 - expected_pages);
        assert_eq!(
            status["coverage"]["reason"],
            if ceiling < 4 {
                "cell_budget"
            } else {
                "complete"
            }
        );
        assert!(graph.diagnostics.is_empty());
        assert!(status["diagnostic"].is_null());
        assert_eq!(status["operationalBudget"]["maxProcessedCells"], ceiling);
    }
}

#[test]
fn topology_budget_preserves_complete_inventory_and_reports_the_stopped_phase() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("topology.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE t(value); INSERT INTO t VALUES(1),(2),(3);")
        .unwrap();
    drop(db);
    for ceiling in [0, 1] {
        let session = InspectionSession::begin(&path)
            .unwrap()
            .with_operational_budget(OperationalBudget {
                max_phase_units: ceiling,
                ..OperationalBudget::default()
            });
        session.scan(|_| ScanControl::Continue).unwrap();
        let graph = session.graph().unwrap();
        assert_eq!(graph.pages.len(), 2);
        assert_eq!(
            graph.coverage.reason,
            volmap_sqlite::inspection::CoverageReason::Complete
        );
        assert_eq!(
            graph.topology_coverage.reason,
            volmap_sqlite::inspection::TopologyCoverageReason::Budget
        );
        assert_eq!(graph.topology_coverage.evaluated, ceiling);
        assert!(graph.diagnostics.is_empty());
    }
}

#[test]
fn resident_memory_ceiling_stops_before_allocating_page_evidence() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("memory.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE t(value);").unwrap();
    drop(db);
    let session = InspectionSession::begin(&path)
        .unwrap()
        .with_operational_budget(OperationalBudget {
            max_resident_bytes: 0,
            ..OperationalBudget::default()
        });
    session.scan(|_| ScanControl::Continue).unwrap();
    let graph = session.graph().unwrap();
    assert!(graph.pages.is_empty());
    let coverage = serde_json::to_value(&graph.coverage).unwrap();
    assert_eq!(coverage["reason"], "resident_memory_budget");
    assert_eq!(coverage["evaluated"], 0);
    assert_eq!(coverage["nextPage"], 1);
    assert_eq!(coverage["remainder"], 2);
    assert!(graph.diagnostics.is_empty());
}

#[test]
fn structural_cancellation_can_be_selected_by_phase_and_extent_without_racing() {
    use volmap_sqlite::inspection::{TopologyCoverageReason, TopologyPhase};
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("cancel.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE t(value); INSERT INTO t VALUES(1),(2),(3);")
        .unwrap();
    drop(db);
    for boundary in [0, 1] {
        let session =
            InspectionSession::begin(&path)
                .unwrap()
                .with_work_observer(move |progress| {
                    if progress.phase == TopologyPhase::BtreeClaimCollection
                        && progress.evaluated == boundary
                    {
                        ScanControl::Cancel
                    } else {
                        ScanControl::Continue
                    }
                });
        session.scan(|_| ScanControl::Continue).unwrap();
        let graph = session.graph().unwrap();
        assert_eq!(graph.pages.len(), 2);
        assert_eq!(
            graph.topology_coverage.reason,
            TopologyCoverageReason::Cancelled
        );
        assert_eq!(
            graph.topology_coverage.phase,
            TopologyPhase::BtreeClaimCollection
        );
        assert_eq!(graph.topology_coverage.evaluated, boundary);
        assert_eq!(graph.topology_coverage.remainder, Some(2 - boundary));
        assert!(graph.diagnostics.is_empty());
    }
}

#[tokio::test]
async fn http_reports_effective_request_limits_and_rejects_oversized_work() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use std::sync::Arc;
    use tower::ServiceExt;
    use volmap_sqlite::web::{WebLimits, atlas_router_for_listener};
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("http.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE t(value);").unwrap();
    drop(db);
    let session = Arc::new(InspectionSession::open(&path).unwrap());
    let id = session.status().snapshot_id;
    let app = atlas_router_for_listener(
        session,
        "127.0.0.1:3000".parse().unwrap(),
        WebLimits {
            request_bytes: 32,
            ..WebLimits::default()
        },
    );
    let status = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/snapshots/{id}"))
                .header("host", "127.0.0.1:3000")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&to_bytes(status.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(json["webLimits"]["requestBytes"], 32);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/snapshots/{id}/deep-inspections"))
                .header("host", "127.0.0.1:3000")
                .header("origin", "http://127.0.0.1:3000")
                .header("content-type", "application/json")
                .body(Body::from(" ".repeat(33)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 413);
    let receipt: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(receipt["coverage"]["scope"], "request_admission");
    assert_eq!(receipt["coverage"]["evaluated"], 0);
    assert_eq!(receipt["coverage"]["total"], 33);
    assert_eq!(receipt["coverage"]["remainder"], 33);
}

#[test]
fn terminal_shows_effective_limits_and_precise_partial_coverage() {
    use std::sync::Arc;
    use volmap_sqlite::terminal::TerminalFlow;
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("terminal.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE t(value);").unwrap();
    drop(db);
    let session = InspectionSession::begin(&path)
        .unwrap()
        .with_operational_budget(OperationalBudget {
            max_processed_cells: 0,
            ..OperationalBudget::default()
        });
    session.scan(|_| ScanControl::Continue).unwrap();
    let mut terminal = TerminalFlow::new(Arc::new(session));
    let text = terminal.screen(160, 60).join("\n");
    assert!(text.contains("Processed cells: 0"), "{text}");
    assert!(text.contains("Resident memory ceiling:"), "{text}");
    assert!(text.contains("Next page: 1; remaining pages: 2"), "{text}");
}

#[test]
fn schema_work_stops_have_their_own_coverage_without_erasing_topology() {
    use volmap_sqlite::inspection::{TopologyCoverageReason, TopologyPhase};
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("schema.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE a(x); CREATE TABLE b(y);")
        .unwrap();
    drop(db);
    for boundary in [0, 1] {
        let session = InspectionSession::begin(&path)
            .unwrap()
            .with_work_observer(move |p| {
                if p.phase == TopologyPhase::SchemaInspection && p.evaluated == boundary {
                    ScanControl::Cancel
                } else {
                    ScanControl::Continue
                }
            });
        session.scan(|_| ScanControl::Continue).unwrap();
        let graph = session.graph().unwrap();
        assert_eq!(
            graph.topology_coverage.reason,
            TopologyCoverageReason::Complete
        );
        let receipt = graph.work_coverage.last().unwrap();
        assert_eq!(receipt.phase, TopologyPhase::SchemaInspection);
        assert_eq!(receipt.reason, TopologyCoverageReason::Cancelled);
        assert_eq!(receipt.evaluated, boundary);
        assert!(graph.diagnostics.is_empty());
    }
}

#[test]
fn helper_copy_budget_reports_known_remainder_without_corruption_diagnostics() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("helper.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE t(value);").unwrap();
    drop(db);
    for ceiling in [0, 1] {
        let session = InspectionSession::begin(&path)
            .unwrap()
            .with_semantic_metadata_budget(volmap_sqlite::semantic::SemanticBudget {
                max_copy_bytes: ceiling,
                ..volmap_sqlite::semantic::SemanticBudget::default()
            });
        session.scan(|_| ScanControl::Continue).unwrap();
        let graph = session.graph().unwrap();
        let metadata = serde_json::to_value(&graph.semantic_metadata).unwrap();
        assert_eq!(metadata["coverage"]["reason"], "copy_byte_budget");
        let work = graph.work_coverage.last().unwrap();
        assert_eq!(
            work.reason,
            volmap_sqlite::inspection::TopologyCoverageReason::Budget
        );
        assert_ne!(work.remainder, Some(0));
        assert_eq!(metadata["coverage"]["evaluatedBytes"], "0");
        assert_eq!(metadata["coverage"]["remainderBytes"], "8192");
        assert!(graph.diagnostics.is_empty());
    }
}

#[test]
fn every_structural_work_category_can_cancel_at_zero_and_one_completed_unit() {
    use volmap_sqlite::inspection::{TopologyCoverageReason, TopologyPhase};
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("categories.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch(
        "PRAGMA page_size=512; PRAGMA auto_vacuum=INCREMENTAL;
        CREATE TABLE live(payload);
        WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<20)
        INSERT INTO live SELECT zeroblob(2000) FROM n;
        CREATE TABLE discarded(payload);
        WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<20)
        INSERT INTO discarded SELECT zeroblob(2000) FROM n;
        DROP TABLE discarded;",
    )
    .unwrap();
    drop(db);
    for phase in [
        TopologyPhase::BtreeClaimCollection,
        TopologyPhase::BtreeTraversal,
        TopologyPhase::FreelistInspection,
        TopologyPhase::OverflowInspection,
        TopologyPhase::PointerMapInspection,
        TopologyPhase::PointerMapValidation,
        TopologyPhase::SchemaInspection,
        TopologyPhase::SemanticHelper,
    ] {
        for boundary in [0, 1] {
            let session = InspectionSession::begin(&path)
                .unwrap()
                .with_semantic_metadata()
                .with_work_observer(move |p| {
                    if p.phase == phase && p.evaluated == boundary {
                        ScanControl::Cancel
                    } else {
                        ScanControl::Continue
                    }
                });
            session.scan(|_| ScanControl::Continue).unwrap();
            let graph = session.graph().unwrap();
            let receipt = graph.work_coverage.last().unwrap();
            assert_eq!(receipt.phase, phase, "{phase:?} at {boundary}: {receipt:?}");
            assert_eq!(
                receipt.reason,
                TopologyCoverageReason::Cancelled,
                "{phase:?} at {boundary}"
            );
            assert_eq!(receipt.evaluated, boundary, "{phase:?}");
            assert!(
                graph.diagnostics.is_empty(),
                "{phase:?}: {:?}",
                graph.diagnostics
            );
            assert_eq!(
                graph.coverage.reason,
                volmap_sqlite::inspection::CoverageReason::Complete
            );
            assert!(!graph.pages.is_empty());
        }
    }
}

#[test]
fn schema_decode_budget_cannot_become_complete_phase_coverage() {
    use volmap_sqlite::inspection::{
        SchemaBudget, SidecarBudget, TopologyCoverageReason, TraversalBudget,
    };
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("schema-budget.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE a(x); CREATE TABLE b(y);")
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
    .unwrap();
    session.scan(|_| ScanControl::Continue).unwrap();
    let graph = session.graph().unwrap();
    assert_eq!(
        graph.work_coverage.last().unwrap().reason,
        TopologyCoverageReason::Budget
    );
    assert_ne!(graph.work_coverage.last().unwrap().remainder, Some(0));
}

#[test]
fn terminal_shows_fast_limits_before_any_revision_exists() {
    use std::sync::Arc;
    use volmap_sqlite::terminal::{Key, TerminalFlow};
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("pending.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE t(x);").unwrap();
    drop(db);
    let session = InspectionSession::begin(&path)
        .unwrap()
        .with_operational_budget(OperationalBudget {
            max_processed_cells: 123,
            ..OperationalBudget::default()
        });
    let mut terminal = TerminalFlow::new(Arc::new(session));
    for help in [false, true] {
        if help {
            terminal.key(Key::Char('?'));
        }
        let text = terminal.screen(160, 60).join("\n");
        assert!(text.contains("Processed cells: 123"), "{text}");
        assert!(text.contains("Resident memory ceiling:"), "{text}");
        assert!(text.contains("B-tree depth:"), "{text}");
    }
}

#[test]
fn helper_with_incomplete_schema_is_explicitly_unavailable() {
    use volmap_sqlite::inspection::{SchemaBudget, SidecarBudget, TraversalBudget};
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("prerequisite.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE a(x);").unwrap();
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
    .with_semantic_metadata();
    session.scan(|_| ScanControl::Continue).unwrap();
    let graph = session.graph().unwrap();
    let receipt = serde_json::to_value(graph.work_coverage.last().unwrap()).unwrap();
    assert_eq!(receipt["reason"], "unavailable");
    assert_ne!(receipt["remainder"], 0);
}
