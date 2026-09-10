use rusqlite::Connection;
use tempfile::TempDir;
use volmap_sqlite::inspection::{InspectionSession, ScanControl, StorageBudget};

#[test]
fn traversal_headers_and_steps_preserve_complete_facts_across_storage_backends() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("traversal-windows.sqlite");
    Connection::open(&path).unwrap().execute_batch("PRAGMA page_size=512; CREATE TABLE entries(value); INSERT INTO entries VALUES(zeroblob(200000));").unwrap();
    for cache_bytes in [0, 16 * 1024 * 1024] {
        let session = InspectionSession::begin(&path)
            .unwrap()
            .with_storage_budget(StorageBudget {
                cache_bytes,
                max_spill_bytes: 64 * 1024 * 1024,
            });
        session.scan(|_| ScanControl::Continue).unwrap();
        let graph = session.graph().unwrap();
        let headers = session.traversal_header_batch(1, None, 0, 256).unwrap();
        assert_eq!(headers.total, graph.traversals.len());
        for header in headers.items {
            let expected = &graph.traversals[header.traversal_offset];
            assert_eq!(header.origin, expected.origin);
            assert_eq!(header.stop, expected.stop);
            assert_eq!(header.prefix_count, expected.validated_prefix.len());
            let mut pages = Vec::new();
            let mut offset = 0;
            loop {
                let batch = session
                    .traversal_prefix_batch(1, header.traversal_offset, offset, 17)
                    .unwrap();
                assert_eq!(batch.total, header.prefix_count);
                pages.extend(batch.items);
                match batch.next_offset {
                    Some(next) => offset = next,
                    None => break,
                }
            }
            assert_eq!(pages, expected.validated_prefix);
        }
        assert!(session.traversal_prefix_batch(1, usize::MAX, 0, 1).is_err());
        assert!(session.traversal_header_batch(1, Some(0), 0, 1).is_err());
    }
}

#[test]
fn spilling_inventory_preserves_published_physical_facts() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("indexed.sqlite");
    let database = Connection::open(&path).unwrap();
    database.execute_batch("PRAGMA page_size=512; CREATE TABLE entries(id INTEGER PRIMARY KEY, label TEXT); CREATE INDEX labels ON entries(label); WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<200) INSERT INTO entries SELECT i, printf('entry-%08d',i) FROM n;").unwrap();
    drop(database);
    let memory = InspectionSession::open(&path).unwrap();
    let spilled = InspectionSession::begin(&path)
        .unwrap()
        .with_storage_budget(StorageBudget {
            cache_bytes: 1024,
            max_spill_bytes: 16 * 1024 * 1024,
        });
    spilled.scan(|_| ScanControl::Continue).unwrap();
    let expected = memory.graph().unwrap();
    let actual = spilled.graph().unwrap();
    assert_eq!(actual.pages, expected.pages);
    assert_eq!(actual.relationship_claims, expected.relationship_claims);
    assert_eq!(actual.relationships, expected.relationships);
    assert_eq!(actual.traversals, expected.traversals);
    assert_eq!(actual.diagnostics, expected.diagnostics);
    assert_eq!(actual.coverage, expected.coverage);
    assert_eq!(actual.schema, expected.schema);
    let status = serde_json::to_value(spilled.status()).unwrap();
    assert_eq!(status["storage"]["spilled"], true);
    assert!(status["storage"]["spilledIndexes"].as_u64().unwrap() > 0);
    assert!(status["storage"]["spillBytes"].as_u64().unwrap() > 0);
    assert!(status["storage"]["cacheBytes"].as_u64().unwrap() <= 1024);
}

#[test]
fn cancellation_at_the_first_spill_retains_the_cached_prefix() {
    use volmap_sqlite::inspection::{CoverageReason, SessionState};
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("spill-boundary.sqlite");
    let database = Connection::open(&path).unwrap();
    database.execute_batch("PRAGMA page_size=512; CREATE TABLE entries(value); WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<1000) INSERT INTO entries SELECT printf('%0100d',i) FROM n;").unwrap();
    drop(database);
    let session = InspectionSession::begin(&path)
        .unwrap()
        .with_storage_budget(StorageBudget {
            cache_bytes: 1024 * 1024,
            // Enough for the new page, not for rewriting the existing cached prefix.
            max_spill_bytes: 64 * 1024,
        });
    let mut boundary = None;
    session
        .scan(|progress| {
            if progress.storage.spilled {
                boundary = Some(progress.progress.completed);
                ScanControl::Cancel
            } else {
                ScanControl::Continue
            }
        })
        .unwrap();
    let count = boundary.expect("the first uncached page was stored before cancellation");
    assert!(count > 2);
    let summary = session.revision_summary(1).unwrap();
    assert_eq!(session.status().state, SessionState::Cancelled);
    assert_eq!(summary.coverage.reason, CoverageReason::Cancelled);
    assert_eq!(summary.page_count, count);
    assert_eq!(summary.coverage.next_page, Some(count + 1));
    let graph = session.graph().unwrap();
    assert_eq!(graph.pages.len(), count as usize);
    assert_eq!(graph.pages.first().unwrap().number, 1);
    assert_eq!(graph.pages.last().unwrap().number, count);
    assert!(!graph.pages.last().unwrap().detail.cells.is_empty());
    assert!(session.status().storage.cache_bytes <= 1024 * 1024);
    assert!(session.status().storage.spill_bytes <= 64 * 1024);
}

#[test]
fn spill_ceiling_stops_before_the_unstored_page_without_corruption() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("bounded.sqlite");
    let database = Connection::open(&path).unwrap();
    database
        .execute_batch("CREATE TABLE entries(value);")
        .unwrap();
    drop(database);
    let session = InspectionSession::begin(&path)
        .unwrap()
        .with_storage_budget(StorageBudget {
            cache_bytes: 0,
            max_spill_bytes: 0,
        });
    session.scan(|_| ScanControl::Continue).unwrap();
    let graph = session.graph().unwrap();
    let summary = session.revision_summary(1).unwrap();
    assert_eq!(summary.snapshot, graph.snapshot);
    assert_eq!(summary.coverage, graph.coverage);
    assert_eq!(summary.topology_coverage, graph.topology_coverage);
    assert_eq!(summary.page_count as usize, graph.pages.len());
    assert_eq!(summary.relationship_count, graph.relationships.len());
    let coverage = serde_json::to_value(&graph.coverage).unwrap();
    assert_eq!(coverage["reason"], "storage_budget");
    assert_eq!(coverage["evaluated"], 0);
    assert_eq!(coverage["nextPage"], 1);
    assert_eq!(coverage["remainder"], 2);
    assert!(graph.pages.is_empty());
    assert!(graph.diagnostics.is_empty());
}

#[test]
fn spilling_keeps_freed_page_regions_and_allocation_evidence() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("freelist.sqlite");
    let database = Connection::open(&path).unwrap();
    database.execute_batch("PRAGMA page_size=512; CREATE TABLE entries(value); INSERT INTO entries VALUES(zeroblob(16000)); DELETE FROM entries;").unwrap();
    drop(database);
    let memory = InspectionSession::open(&path).unwrap();
    let spilled = InspectionSession::begin(&path)
        .unwrap()
        .with_storage_budget(StorageBudget {
            cache_bytes: 0,
            max_spill_bytes: 1024 * 1024,
        });
    spilled.scan(|_| ScanControl::Continue).unwrap();
    let expected = memory.graph().unwrap();
    let actual = spilled.graph().unwrap();
    assert_eq!(actual.pages, expected.pages);
    assert_eq!(actual.freelist, expected.freelist);
}

#[test]
fn revision_pages_are_navigable_in_bounded_batches() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("batches.sqlite");
    let database = Connection::open(&path).unwrap();
    database.execute_batch("PRAGMA page_size=512; CREATE TABLE entries(value); INSERT INTO entries VALUES(zeroblob(16000));").unwrap();
    drop(database);
    let session = InspectionSession::begin(&path)
        .unwrap()
        .with_storage_budget(StorageBudget {
            cache_bytes: 1024,
            max_spill_bytes: 1024 * 1024,
        });
    session.scan(|_| ScanControl::Continue).unwrap();
    let graph = session.graph().unwrap();
    let mut first = 1;
    let mut collected = Vec::new();
    loop {
        let batch = session.page_batch(1, first, 3).unwrap();
        assert_eq!(batch.total, graph.coverage.evaluated);
        assert!(batch.pages.len() <= 3);
        collected.extend(batch.pages);
        match batch.next_page {
            Some(next) => {
                assert!(next > first);
                first = next;
            }
            None => break,
        }
    }
    assert_eq!(collected, graph.pages);
    assert!(session.page_batch(1, 0, 3).is_err());
    assert!(session.page_batch(1, 1, 0).is_err());
    assert!(session.page_batch(1, 1, 257).is_err());
    assert!(session.page_batch(2, 1, 3).is_err());
}

#[tokio::test]
async fn traversal_steps_remain_available_when_the_complete_record_exceeds_the_wire_budget() {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use tower::ServiceExt;
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("long-traversal.sqlite");
    Connection::open(&path).unwrap().execute_batch("PRAGMA page_size=512; CREATE TABLE entries(value); INSERT INTO entries VALUES(zeroblob(200000));").unwrap();
    let session = Arc::new(
        InspectionSession::begin(&path)
            .unwrap()
            .with_storage_budget(StorageBudget {
                cache_bytes: 0,
                max_spill_bytes: 64 * 1024 * 1024,
            }),
    );
    session.scan(|_| ScanControl::Continue).unwrap();
    let graph = session.graph().unwrap();
    let (position, traversal) = graph
        .traversals
        .iter()
        .enumerate()
        .max_by_key(|(_, t)| t.validated_prefix.len())
        .unwrap();
    assert!(traversal.validated_prefix.len() > 300);
    let id = session.status().snapshot_id;
    let app = volmap_sqlite::web::atlas_router_for_listener(
        session,
        "127.0.0.1:80".parse().unwrap(),
        volmap_sqlite::web::WebLimits {
            response_bytes: 4096,
            ..Default::default()
        },
    );
    let base = format!("/api/snapshots/{id}/revisions/1");
    let request = |suffix: String| {
        Request::builder()
            .uri(format!("{base}/{suffix}"))
            .header("host", "localhost")
            .body(Body::empty())
            .unwrap()
    };
    let response = app
        .clone()
        .oneshot(request(format!("collections/traversals/{position}/1")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INSUFFICIENT_STORAGE);
    let response = app
        .clone()
        .oneshot(request(format!(
            "collections/traversal_headers/{position}/1"
        )))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let header: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        header["items"][0]["prefixCount"],
        traversal.validated_prefix.len()
    );
    let mut pages = Vec::new();
    let mut offset = 0;
    loop {
        let response = app
            .clone()
            .oneshot(request(format!("traversals/{position}/pages/{offset}/17")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        let batch: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        pages.extend(
            batch["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|page| page["pageNumber"].as_u64().unwrap()),
        );
        if let Some(next) = batch["nextOffset"].as_u64() {
            offset = next;
        } else {
            break;
        }
    }
    assert_eq!(
        pages,
        traversal
            .validated_prefix
            .iter()
            .map(|page| u64::from(page.page_number))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn http_page_batches_are_scoped_bounded_and_value_private() {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use tower::ServiceExt;
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("web.sqlite");
    let database = Connection::open(&path).unwrap();
    database
        .execute_batch(
            "CREATE TABLE entries(value); INSERT INTO entries VALUES('private-batch-value');",
        )
        .unwrap();
    drop(database);
    let session = Arc::new(InspectionSession::open(&path).unwrap());
    let id = session.status().snapshot_id;
    let app = volmap_sqlite::web::atlas_router(session);
    for (suffix, expected) in [
        ("summary", StatusCode::OK),
        ("metadata", StatusCode::OK),
        ("pages/1/1", StatusCode::OK),
        ("pages/0/1", StatusCode::BAD_REQUEST),
        ("pages/1/257", StatusCode::BAD_REQUEST),
        ("collections/claims/0/1", StatusCode::OK),
        ("collections/relationships/0/1", StatusCode::OK),
        ("collections/traversals/0/1", StatusCode::OK),
        ("collections/traversal_headers/0/1", StatusCode::OK),
        ("traversals/0/pages/0/1", StatusCode::OK),
        ("traversals/99999/pages/0/1", StatusCode::NOT_FOUND),
        ("collections/diagnostics/0/1", StatusCode::OK),
        ("collections/freelist_trunks/0/1", StatusCode::OK),
        ("collections/pointer_maps/0/1", StatusCode::OK),
        ("collections/schema/0/1", StatusCode::OK),
        ("schema/0/pages/0/1", StatusCode::OK),
        ("schema/0/pages/0/0", StatusCode::BAD_REQUEST),
        ("schema/99/pages/0/1", StatusCode::NOT_FOUND),
        ("pages/1/collections/claims/0/1", StatusCode::OK),
        ("pages/1/collections/relationships/0/1", StatusCode::OK),
        ("pages/1/collections/traversals/0/1", StatusCode::OK),
        ("pages/1/collections/traversal_headers/0/1", StatusCode::OK),
        ("pages/2/collections/schema/0/1", StatusCode::OK),
        ("pages/0/collections/claims/0/1", StatusCode::NOT_FOUND),
        ("pages/1/collections/claims/0/257", StatusCode::BAD_REQUEST),
        ("pages/1/collections/unknown/0/1", StatusCode::NOT_FOUND),
        ("pages/1/allocation/pointer_map", StatusCode::OK),
        ("pages/1/allocation/freelist_trunk", StatusCode::OK),
        ("pages/0/allocation/pointer_map", StatusCode::NOT_FOUND),
        ("collections/claims/0/0", StatusCode::BAD_REQUEST),
        ("collections/claims/0/257", StatusCode::BAD_REQUEST),
        ("collections/unknown/0/1", StatusCode::NOT_FOUND),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/snapshots/{id}/revisions/1/{suffix}"))
                    .header("host", "localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(!text.contains("private-batch-value"));
        assert!(!text.contains(directory.path().to_str().unwrap()));
        if suffix == "metadata" {
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["summary"]["revision"], 1);
            assert!(body["schema"].get("objects").is_none());
            assert!(body.get("pages").is_none());
        }
        if suffix.starts_with("collections/") && expected == StatusCode::OK {
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["revision"], 1);
            assert_eq!(body["offset"], 0);
            assert!(body["items"].as_array().unwrap().len() <= 1);
        }
        if suffix == "pages/1/1" {
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["total"], 2);
            assert_eq!(body["nextPage"], 2);
            assert_eq!(body["pages"].as_array().unwrap().len(), 1);
        }
    }
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/snapshots/foreign/revisions/1/pages/1/1")
                .header("host", "localhost")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[test]
fn filling_private_storage_preserves_a_navigable_budget_receipt() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("full-spill.sqlite");
    let database = Connection::open(&path).unwrap();
    database.execute_batch("PRAGMA page_size=512; CREATE TABLE entries(value); WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<200) INSERT INTO entries SELECT printf('%080d',i) FROM n;").unwrap();
    let total: u32 = database
        .query_row("PRAGMA page_count", [], |row| row.get(0))
        .unwrap();
    drop(database);
    for ceiling in [12_288, 32_768, 65_536, 131_072] {
        let session = InspectionSession::begin(&path)
            .unwrap()
            .with_storage_budget(StorageBudget {
                cache_bytes: 0,
                max_spill_bytes: ceiling,
            });
        session.scan(|_| ScanControl::Continue).unwrap();
        let graph = session.graph().unwrap();
        assert_eq!(graph.coverage.total, Some(total));
        assert_eq!(graph.coverage.evaluated as usize, graph.pages.len());
        assert_eq!(
            graph.coverage.remainder,
            Some(total - graph.coverage.evaluated)
        );
        assert!(graph.diagnostics.is_empty());
        assert!(session.status().storage.spill_bytes <= ceiling);
    }
}

#[test]
#[ignore = "extended sparse 820 MiB content-verification profile; run with --release"]
fn full_large_pointer_map_remains_queryable_after_inventory_cancellation() {
    use std::os::unix::fs::FileExt;
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("full-map.sqlite");
    let database = Connection::open(&path).unwrap();
    database
        .execute_batch(
            "PRAGMA page_size=65536; PRAGMA auto_vacuum=FULL; CREATE TABLE entries(value);",
        )
        .unwrap();
    drop(database);
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_len(13110 * 65536).unwrap();
    file.write_all_at(&13110_u32.to_be_bytes(), 28).unwrap();
    let entries = [2, 0, 0, 0, 0].repeat(13107);
    file.write_all_at(&entries, 65536).unwrap();
    drop(file);
    let session = InspectionSession::begin(&path)
        .unwrap()
        .with_storage_budget(StorageBudget {
            cache_bytes: 0,
            max_spill_bytes: 64 * 1024 * 1024,
        });
    session
        .scan(|status| {
            if status.progress.completed == 2 {
                ScanControl::Cancel
            } else {
                ScanControl::Continue
            }
        })
        .unwrap();
    let map = session.page_pointer_map(1, 2).unwrap().unwrap();
    assert_eq!(map.entries.len(), 13107);
    assert!(serde_json::to_vec(&map).unwrap().len() > 2 * 1024 * 1024);
    assert!(serde_json::to_vec(&map).unwrap().len() < 8 * 1024 * 1024);
    assert_eq!(session.pointer_map_batch(1, 0, 1).unwrap().items, vec![map]);
    drop(session);
    let resident: u64 = std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let tight = InspectionSession::begin(&path)
        .unwrap()
        .with_storage_budget(StorageBudget {
            cache_bytes: 0,
            max_spill_bytes: 64 * 1024 * 1024,
        })
        .with_operational_budget(volmap_sqlite::inspection::OperationalBudget {
            max_resident_bytes: resident * 1024 + 12 * 1024 * 1024,
            ..Default::default()
        });
    tight
        .scan(|status| {
            if status.progress.completed == 2 {
                ScanControl::Cancel
            } else {
                ScanControl::Continue
            }
        })
        .unwrap();
    assert_eq!(
        tight.revision_summary(1).unwrap().page_count,
        2,
        "{:?}",
        tight.status()
    );
    let stopped = tight.page_pointer_map(1, 2).unwrap().unwrap();
    assert!(!stopped.complete);
    assert!(
        stopped.entries.is_empty(),
        "admission must precede entry allocation"
    );
    let receipts = serde_json::to_value(tight.status()).unwrap();
    assert!(
        receipts["workCoverage"]
            .as_array()
            .unwrap()
            .iter()
            .any(|receipt| receipt["phase"] == "pointer_map_inspection"
                && receipt["reason"] == "budget")
    );
}

#[test]
fn dense_large_pages_remain_queryable_when_their_decoded_estimate_exceeds_a_batch() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("dense.sqlite");
    let database = Connection::open(&path).unwrap();
    database.execute_batch("PRAGMA page_size=65536; CREATE TABLE entries(value); WITH RECURSIVE numbers(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM numbers WHERE n<12000) INSERT INTO entries SELECT 1 FROM numbers;").unwrap();
    drop(database);
    let session = InspectionSession::begin(&path)
        .unwrap()
        .with_storage_budget(StorageBudget {
            cache_bytes: 0,
            max_spill_bytes: 128 * 1024 * 1024,
        });
    session.scan(|_| ScanControl::Continue).unwrap();
    let summary = session.revision_summary(1).unwrap();
    assert!(session.status().storage.max_page_bytes > 8 * 1024 * 1024);
    let mut cells = 0;
    for number in 1..=summary.page_count {
        let batch = session.page_batch(1, number, 1).unwrap();
        assert!(serde_json::to_vec(&batch).unwrap().len() < 8 * 1024 * 1024);
        if number > 1 {
            cells += batch.pages[0].detail.cells.len();
        }
    }
    assert!(cells >= 12000);
}

#[test]
fn large_pages_with_few_cells_do_not_reserve_a_dense_page_worst_case() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("large-pages.sqlite");
    let database = Connection::open(&path).unwrap();
    database
        .execute_batch("PRAGMA page_size=65536; CREATE TABLE entries(value);")
        .unwrap();
    drop(database);
    let session = InspectionSession::begin(&path)
        .unwrap()
        .with_operational_budget(volmap_sqlite::inspection::OperationalBudget {
            max_resident_bytes: 64 * 1024 * 1024,
            ..Default::default()
        });
    session.scan(|_| ScanControl::Continue).unwrap();
    assert_eq!(
        session.revision_summary(1).unwrap().coverage.reason,
        volmap_sqlite::inspection::CoverageReason::Complete
    );
}

#[test]
fn spilling_preserves_corrupt_overflow_claims_and_stop_reasons() {
    use std::os::unix::fs::FileExt;
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("cycle.sqlite");
    let database = Connection::open(&path).unwrap();
    database.execute_batch("PRAGMA page_size=512; CREATE TABLE entries(value); INSERT INTO entries VALUES(zeroblob(16000));").unwrap();
    drop(database);
    let inspected = InspectionSession::open(&path).unwrap().graph().unwrap();
    let first = inspected
        .relationship_claims
        .iter()
        .find(|claim| claim.kind == volmap_sqlite::inspection::RelationshipKind::Overflow)
        .unwrap()
        .target
        .as_ref()
        .unwrap()
        .page_number;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .write_all_at(&first.to_be_bytes(), u64::from(first - 1) * 512)
        .unwrap();
    let expected = InspectionSession::open(&path).unwrap().graph().unwrap();
    let spilled = InspectionSession::begin(&path)
        .unwrap()
        .with_storage_budget(StorageBudget {
            cache_bytes: 0,
            max_spill_bytes: 1024 * 1024,
        });
    spilled.scan(|_| ScanControl::Continue).unwrap();
    let actual = spilled.graph().unwrap();
    assert_eq!(actual.relationship_claims, expected.relationship_claims);
    assert_eq!(actual.relationships, expected.relationships);
    assert_eq!(actual.traversals, expected.traversals);
    assert_eq!(actual.diagnostics, expected.diagnostics);
    assert_eq!(actual.pages, expected.pages);
}

#[test]
fn cancellation_in_spilled_index_phases_preserves_exact_receipts() {
    use volmap_sqlite::inspection::{TopologyCoverageReason, TopologyPhase};
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("cancel-indexes.sqlite");
    let database = Connection::open(&path).unwrap();
    database.execute_batch("PRAGMA page_size=512; CREATE TABLE entries(value); INSERT INTO entries VALUES(zeroblob(16000)),(zeroblob(16000));").unwrap();
    drop(database);
    for phase in [
        TopologyPhase::BtreeCycleReconciliation,
        TopologyPhase::OverflowReconciliation,
        TopologyPhase::RoleReconciliation,
    ] {
        let session = InspectionSession::begin(&path)
            .unwrap()
            .with_storage_budget(StorageBudget {
                cache_bytes: 0,
                max_spill_bytes: 4 * 1024 * 1024,
            })
            .with_work_observer(move |progress| {
                if progress.phase == phase && progress.evaluated == 1 {
                    ScanControl::Cancel
                } else {
                    ScanControl::Continue
                }
            });
        session.scan(|_| ScanControl::Continue).unwrap();
        let graph = session.graph().unwrap();
        assert_eq!(
            graph.coverage.reason,
            volmap_sqlite::inspection::CoverageReason::Complete
        );
        assert_eq!(
            graph.topology_coverage.reason,
            TopologyCoverageReason::Cancelled
        );
        assert_eq!(graph.topology_coverage.phase, phase);
        assert_eq!(graph.topology_coverage.evaluated, 1);
        assert!(graph.diagnostics.is_empty());
        assert!(session.status().storage.spilled_indexes > 0);
        assert_eq!(
            session.page_batch(1, 1, 1).unwrap().pages[0],
            graph.pages[0]
        );
    }
}

#[test]
fn spilling_preserves_valid_and_damaged_pointer_map_evidence() {
    use std::os::unix::fs::FileExt;
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("auto-vacuum.sqlite");
    let database = Connection::open(&path).unwrap();
    database.execute_batch("PRAGMA page_size=512; PRAGMA auto_vacuum=FULL; CREATE TABLE entries(value); INSERT INTO entries VALUES(zeroblob(16000));").unwrap();
    drop(database);
    for damaged in [false, true] {
        if damaged {
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .write_all_at(&[255], 512)
                .unwrap();
        }
        let expected = InspectionSession::open(&path).unwrap().graph().unwrap();
        let spilled = InspectionSession::begin(&path)
            .unwrap()
            .with_storage_budget(StorageBudget {
                cache_bytes: 0,
                max_spill_bytes: 4 * 1024 * 1024,
            });
        spilled.scan(|_| ScanControl::Continue).unwrap();
        let actual = spilled.graph().unwrap();
        assert!(!actual.pointer_map.pages.is_empty());
        assert_eq!(actual.pointer_map, expected.pointer_map);
        assert_eq!(actual.pages, expected.pages);
        assert_eq!(actual.relationship_claims, expected.relationship_claims);
        assert_eq!(actual.relationships, expected.relationships);
        assert_eq!(actual.traversals, expected.traversals);
        assert_eq!(actual.diagnostics, expected.diagnostics);
    }
}

#[test]
fn a_schema_spill_stop_keeps_completed_topology_facts() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("schema-ceiling.sqlite");
    let database = Connection::open(&path).unwrap();
    database.execute_batch("PRAGMA page_size=512;").unwrap();
    for number in 0..40 {
        database
            .execute_batch(&format!(
                "CREATE TABLE entries_{number}(value); INSERT INTO entries_{number} VALUES(1);"
            ))
            .unwrap();
    }
    drop(database);
    let complete = InspectionSession::begin(&path)
        .unwrap()
        .with_storage_budget(StorageBudget {
            cache_bytes: 0,
            max_spill_bytes: 16 * 1024 * 1024,
        });
    complete.scan(|_| ScanControl::Continue).unwrap();
    let required = complete.status().storage.spill_bytes;
    let expected = complete.graph().unwrap();
    let limited = InspectionSession::begin(&path)
        .unwrap()
        .with_storage_budget(StorageBudget {
            cache_bytes: 0,
            max_spill_bytes: required - 4096,
        });
    limited.scan(|_| ScanControl::Continue).unwrap();
    let actual = limited.graph().unwrap();
    assert_eq!(
        actual.topology_coverage.reason,
        volmap_sqlite::inspection::TopologyCoverageReason::Complete
    );
    assert_eq!(
        actual.schema.state,
        volmap_sqlite::inspection::SchemaState::Partial
    );
    assert_eq!(actual.pages, expected.pages);
    assert_eq!(actual.relationship_claims, expected.relationship_claims);
    assert_eq!(actual.relationships, expected.relationships);
    assert_eq!(actual.traversals, expected.traversals);
    assert!(actual.diagnostics.is_empty());
}

#[test]
fn claim_batches_preserve_order_and_identity_across_spilled_collections() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("claim-batches.sqlite");
    let database = Connection::open(&path).unwrap();
    database.execute_batch("PRAGMA page_size=512; CREATE TABLE entries(value); INSERT INTO entries VALUES(zeroblob(16000));").unwrap();
    drop(database);
    let session = InspectionSession::begin(&path)
        .unwrap()
        .with_storage_budget(StorageBudget {
            cache_bytes: 0,
            max_spill_bytes: 4 * 1024 * 1024,
        });
    session.scan(|_| ScanControl::Continue).unwrap();
    let graph = session.graph().unwrap();
    let mut offset = 0;
    let mut claims = Vec::new();
    loop {
        let batch = session.claim_batch(1, offset, 3).unwrap();
        assert_eq!(batch.revision, 1);
        assert_eq!(batch.offset, offset);
        assert_eq!(batch.total, graph.relationship_claims.len());
        assert!(batch.items.len() <= 3);
        claims.extend(batch.items);
        match batch.next_offset {
            Some(next) => {
                assert!(next > offset);
                offset = next;
            }
            None => break,
        }
    }
    assert_eq!(claims, graph.relationship_claims);
    assert!(session.claim_batch(1, 0, 0).is_err());
    assert!(session.claim_batch(1, 0, 257).is_err());
    assert!(session.claim_batch(2, 0, 1).is_err());
    assert!(
        session
            .claim_batch(1, claims.len(), 1)
            .unwrap()
            .items
            .is_empty()
    );
}

#[test]
fn revision_metadata_describes_collections_without_materializing_them() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("metadata.sqlite");
    let database = Connection::open(&path).unwrap();
    database.execute_batch("PRAGMA page_size=512; PRAGMA auto_vacuum=FULL; CREATE TABLE entries(value); INSERT INTO entries VALUES(zeroblob(16000));").unwrap();
    drop(database);
    let session = InspectionSession::begin(&path)
        .unwrap()
        .with_storage_budget(StorageBudget {
            cache_bytes: 0,
            max_spill_bytes: 4 * 1024 * 1024,
        });
    session.scan(|_| ScanControl::Continue).unwrap();
    let graph = session.graph().unwrap();
    let metadata = session.revision_metadata(1).unwrap();
    assert_eq!(metadata.summary.snapshot, graph.snapshot);
    assert_eq!(
        metadata.summary.claim_count,
        graph.relationship_claims.len()
    );
    assert_eq!(
        metadata.summary.schema_object_count,
        graph.schema.objects.len()
    );
    assert_eq!(
        metadata.summary.pointer_map_page_count,
        graph.pointer_map.pages.len()
    );
    assert_eq!(metadata.schema.state, graph.schema.state);
    assert_eq!(metadata.freelist.coverage, graph.freelist.coverage);
    let wire = serde_json::to_value(metadata).unwrap();
    assert!(wire.get("pages").is_none());
    assert!(wire.get("relationshipClaims").is_none());
    assert!(wire["schema"].get("objects").is_none());
    assert!(wire["freelist"].get("trunks").is_none());
    assert!(wire["pointerMap"].get("pages").is_none());
    assert!(wire["pointerMap"].get("locations").is_none());
    assert!(session.revision_metadata(2).is_err());
}

#[test]
fn schema_windows_preserve_declarations_and_complete_page_attribution() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("schema-windows.sqlite");
    let database = Connection::open(&path).unwrap();
    database.execute_batch("PRAGMA page_size=512; CREATE TABLE entries(value); CREATE INDEX by_value ON entries(value); CREATE VIEW declared AS SELECT value FROM entries; WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<150) INSERT INTO entries SELECT printf('%080d',i) FROM n;").unwrap();
    drop(database);
    let session = InspectionSession::begin(&path)
        .unwrap()
        .with_storage_budget(StorageBudget {
            cache_bytes: 0,
            max_spill_bytes: 16 * 1024 * 1024,
        });
    session.scan(|_| ScanControl::Continue).unwrap();
    let graph = session.graph().unwrap();
    assert_eq!(graph.schema.objects.len(), 3);
    for (position, expected) in graph.schema.objects.iter().enumerate() {
        let batch = session.schema_batch(1, position, 1).unwrap();
        assert_eq!(batch.total, 3);
        assert_eq!(batch.items.len(), 1);
        assert_eq!(batch.items[0].identity, expected.identity);
        let actual = serde_json::to_value(&batch.items[0]).unwrap();
        let mut declaration = serde_json::to_value(expected).unwrap();
        declaration.as_object_mut().unwrap().remove("pages");
        assert_eq!(actual, declaration);
        let mut offset = 0;
        let mut pages = Vec::new();
        loop {
            let batch = session.schema_page_batch(1, position, offset, 2).unwrap();
            assert_eq!(batch.total, expected.pages.len());
            assert!(batch.items.len() <= 2);
            pages.extend(batch.items);
            match batch.next_offset {
                Some(next) => offset = next,
                None => break,
            }
        }
        assert_eq!(pages, expected.pages);
    }
    assert!(session.schema_batch(1, 0, 0).is_err());
    assert!(session.schema_page_batch(1, 0, 0, 257).is_err());
    assert!(session.schema_page_batch(1, 3, 0, 1).is_err());
    assert!(session.schema_page_batch(2, 0, 0, 1).is_err());
    for page in &graph.pages {
        let expected: Vec<_> = graph
            .schema
            .objects
            .iter()
            .enumerate()
            .filter(|(_, object)| {
                object
                    .pages
                    .iter()
                    .any(|member| member.page_number == page.number)
            })
            .collect();
        let batch = session.page_schema_batch(1, page.number, 0, 1).unwrap();
        assert_eq!(batch.total, expected.len());
        for (actual, (position, object)) in batch.items.iter().zip(expected) {
            assert_eq!(actual.object_offset, position);
            assert_eq!(actual.object.identity, object.identity);
        }
    }
    assert!(session.page_schema_batch(1, 0, 0, 1).is_err());
    assert!(session.page_schema_batch(1, 1, 0, 0).is_err());
}

#[test]
fn selected_page_windows_include_incoming_claims_without_duplicates() {
    use volmap_sqlite::inspection::EntityIdentity;
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("page-claims.sqlite");
    let database = Connection::open(&path).unwrap();
    database.execute_batch("PRAGMA page_size=512; CREATE TABLE entries(value); INSERT INTO entries VALUES(zeroblob(16000));").unwrap();
    drop(database);
    for cache_bytes in [0, 64 * 1024, 8 * 1024 * 1024] {
        let session = InspectionSession::begin(&path)
            .unwrap()
            .with_storage_budget(StorageBudget {
                cache_bytes,
                max_spill_bytes: 16 * 1024 * 1024,
            });
        session.scan(|_| ScanControl::Continue).unwrap();
        let graph = session.graph().unwrap();
        for page in &graph.pages {
            let source_page = |source: &EntityIdentity| match source {
                EntityIdentity::Page { page_number } | EntityIdentity::Cell { page_number, .. } => {
                    *page_number
                }
            };
            let expected: Vec<_> = graph
                .relationship_claims
                .iter()
                .filter(|claim| {
                    source_page(&claim.source) == page.number
                        || claim
                            .target
                            .as_ref()
                            .is_some_and(|target| target.page_number == page.number)
                })
                .cloned()
                .collect();
            let mut offset = 0;
            let mut claims = Vec::new();
            loop {
                let batch = session.page_claim_batch(1, page.number, offset, 1).unwrap();
                assert_eq!(batch.total, expected.len());
                claims.extend(batch.items);
                match batch.next_offset {
                    Some(next) => offset = next,
                    None => break,
                }
            }
            assert_eq!(claims, expected);
            let links = session
                .page_relationship_batch(1, page.number, 0, 256)
                .unwrap();
            assert_eq!(
                links.items,
                graph
                    .relationships
                    .iter()
                    .filter(|link| source_page(&link.source) == page.number
                        || link.target.page_number == page.number)
                    .cloned()
                    .collect::<Vec<_>>()
            );
            let traversals = session
                .page_traversal_batch(1, page.number, 0, 256)
                .unwrap();
            assert_eq!(
                traversals.items,
                graph
                    .traversals
                    .iter()
                    .filter(|traversal| source_page(&traversal.origin) == page.number)
                    .cloned()
                    .collect::<Vec<_>>()
            );
        }
        assert!(session.page_claim_batch(1, 0, 0, 1).is_err());
        assert!(session.page_claim_batch(1, u32::MAX, 0, 1).is_err());
        assert!(session.page_claim_batch(1, 1, 0, 257).is_err());
    }
}
