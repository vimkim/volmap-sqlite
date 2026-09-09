use std::sync::Arc;

use rusqlite::Connection;
use tempfile::TempDir;
use volmap_sqlite::inspection::{DeepBudget, DeepSelector, DeepState, InspectionSession};

fn fixture(sql: &str) -> (TempDir, std::path::PathBuf, Arc<InspectionSession>) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("selected.sqlite");
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("PRAGMA page_size=512;").unwrap();
    connection.execute_batch(sql).unwrap();
    drop(connection);
    let session = Arc::new(InspectionSession::open(&path).unwrap());
    (dir, path, session)
}

fn target(session: &InspectionSession, revision: u64, index: u16) -> DeepSelector {
    let status = session.status();
    DeepSelector {
        session_id: status.session_id,
        snapshot_id: status.snapshot_id,
        revision,
        page_number: 2,
        cell_index: index,
    }
}

#[test]
fn selected_values_publish_a_later_revision_without_entering_broad_graphs() {
    let (_dir, _path, session) = fixture("CREATE TABLE t(a,b,c,d,e);
        INSERT INTO t VALUES(NULL, -9223372036854775808, 1.25, 'SELECTED_SECRET', X'424C4F425F534543524554');
        INSERT INTO t VALUES(1,2,3,'OTHER_SECRET',NULL);");
    let selector = target(&session, 1, 0);
    let initial = session.revision(1).unwrap();
    let job = session.request_deep(selector.clone(), DeepBudget::default());
    let status = job.wait();
    assert_eq!(status.state, DeepState::Completed);
    assert_eq!(status.result_revision, Some(2));
    let result = session.deep_result(&job.id, &selector).unwrap();
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["values"][0]["value"]["type"], "null");
    assert_eq!(json["values"][1]["value"]["value"], "-9223372036854775808");
    assert_eq!(json["values"][2]["value"]["value"], "1.25");
    assert_eq!(json["values"][3]["value"]["value"], "SELECTED_SECRET");
    assert_eq!(
        json["values"][4]["value"],
        serde_json::json!({"type":"blob", "byteLength":"11"})
    );
    assert!(!json.to_string().contains("OTHER_SECRET"));
    for broad in [
        serde_json::to_string(&status).unwrap(),
        serde_json::to_string(&session.status()).unwrap(),
        serde_json::to_string(&*session.revision(1).unwrap()).unwrap(),
        serde_json::to_string(&*session.revision(2).unwrap()).unwrap(),
    ] {
        for secret in ["SELECTED_SECRET", "OTHER_SECRET", "BLOB_SECRET", "424c4f42"] {
            assert!(!broad.contains(secret));
        }
    }
    assert_eq!(initial.revision, 1);
    assert_eq!(initial.pages, session.revision(2).unwrap().pages);
    assert_eq!(session.status().revision, Some(2));
}

#[test]
fn reconstructs_only_the_selected_overflow_record_with_exact_field_provenance() {
    for encoding in ["UTF-8", "UTF-16le", "UTF-16be"] {
        let secret = format!("選択{}\0end", "text".repeat(1000));
        let (dir, path, _initial) = fixture(&format!(
            "PRAGMA encoding='{encoding}'; CREATE TABLE t(value);"
        ));
        let connection = Connection::open(&path).unwrap();
        connection
            .execute("INSERT INTO t VALUES(?)", [&secret])
            .unwrap();
        connection
            .execute("INSERT INTO t VALUES(?)", ["UNSELECTED_SENSITIVE"])
            .unwrap();
        drop(connection);
        let session = Arc::new(InspectionSession::open(&path).unwrap());
        let selector = target(&session, 1, 0);
        let job = session.request_deep(selector.clone(), DeepBudget::default());
        assert_eq!(job.wait().state, DeepState::Completed, "{encoding}");
        let result =
            serde_json::to_value(session.deep_result(&job.id, &selector).unwrap()).unwrap();
        assert_eq!(result["values"][0]["value"]["value"], secret);
        assert!(
            result["values"][0]["field"]["source"]
                .as_array()
                .unwrap()
                .len()
                > 1
        );
        assert_eq!(result["evidence"]["coverage"]["remainderBytes"], "0");
        assert!(!result.to_string().contains("UNSELECTED_SENSITIVE"));
        assert!(
            !serde_json::to_string(&*session.revision(2).unwrap())
                .unwrap()
                .contains("texttext")
        );
        drop(dir);
    }
}

#[test]
fn rejected_targets_cannot_disclose_values_or_publish_revisions() {
    let (_dir, _path, session) = fixture("CREATE TABLE t(value); INSERT INTO t VALUES('SECRET');");
    let selector = target(&session, 1, 0);
    let mut bad = selector.clone();
    bad.page_number = 0;
    assert_eq!(
        session
            .request_deep(bad, DeepBudget::default())
            .wait()
            .state,
        DeepState::InvalidTarget
    );
    let mut wrong_session = selector.clone();
    wrong_session.session_id = "other".into();
    assert_eq!(
        session
            .request_deep(wrong_session, DeepBudget::default())
            .wait()
            .state,
        DeepState::InvalidTarget
    );
    let mut wrong_snapshot = selector.clone();
    wrong_snapshot.snapshot_id = "other".into();
    assert_eq!(
        session
            .request_deep(wrong_snapshot, DeepBudget::default())
            .wait()
            .state,
        DeepState::InvalidTarget
    );
    let mut wrong_cell = selector.clone();
    wrong_cell.cell_index = 9;
    assert_eq!(
        session
            .request_deep(wrong_cell, DeepBudget::default())
            .wait()
            .state,
        DeepState::InvalidTarget
    );
    let mut stale = selector.clone();
    stale.revision = 9;
    assert_eq!(
        session
            .request_deep(stale, DeepBudget::default())
            .wait()
            .state,
        DeepState::StaleRevision
    );
    assert_eq!(session.status().revision, Some(1));
    let job = session.request_deep(selector.clone(), DeepBudget::default());
    assert_eq!(job.wait().state, DeepState::Completed);
    let mut other = selector;
    other.cell_index = 1;
    assert_eq!(
        session.deep_result(&job.id, &other),
        Err(DeepState::InvalidTarget)
    );
}

#[test]
fn budgets_and_cancellation_leave_the_base_revision_unchanged() {
    use volmap_sqlite::inspection::ScanControl;
    let sql = format!(
        "CREATE TABLE t(value); INSERT INTO t VALUES('{}');",
        "PRIVATE".repeat(1000)
    );
    let (_dir, _path, session) = fixture(&sql);
    for budget in [
        DeepBudget {
            max_payload_bytes: 10,
            ..DeepBudget::default()
        },
        DeepBudget {
            max_overflow_pages: 0,
            ..DeepBudget::default()
        },
        DeepBudget {
            max_values: 0,
            ..DeepBudget::default()
        },
    ] {
        let selector = target(&session, 1, 0);
        let job = session.request_deep(selector.clone(), budget);
        let receipt = job.wait();
        assert_eq!(receipt.state, DeepState::BudgetStopped);
        assert!(receipt.coverage.stopping_payload_offset.is_some());
        if budget.max_values == 0 {
            assert_eq!(receipt.coverage.stopping_page, Some(2));
        }
        assert_eq!(session.status().revision, Some(1));
        assert_eq!(
            session.deep_result(&job.id, &selector),
            Err(DeepState::BudgetStopped)
        );
    }
    let selector = target(&session, 1, 0);
    let job = session.request_deep_observed(selector, DeepBudget::default(), |status| {
        if status.coverage.reconstructed_bytes == "0" {
            ScanControl::Continue
        } else {
            ScanControl::Cancel
        }
    });
    assert_eq!(job.wait().state, DeepState::Cancelled);
    assert_ne!(job.status().coverage.reconstructed_bytes, "0");
    assert_eq!(
        job.status().coverage.stopping_payload_offset.as_ref(),
        Some(&job.status().coverage.reconstructed_bytes)
    );
    assert_eq!(session.status().revision, Some(1));
}

#[test]
fn concurrent_jobs_publish_once_and_repeated_targets_keep_old_revisions() {
    use std::sync::Barrier;
    use volmap_sqlite::inspection::ScanControl;
    let (_dir, _path, session) =
        fixture("CREATE TABLE t(value); INSERT INTO t VALUES('FIRST'),('SECOND');");
    let gate = Arc::new(Barrier::new(3));
    let mut jobs = vec![];
    for index in [0, 1] {
        let gate = Arc::clone(&gate);
        jobs.push(session.request_deep_observed(
            target(&session, 1, index),
            DeepBudget::default(),
            move |status| {
                if status.coverage.phase == "publication" {
                    gate.wait();
                }
                ScanControl::Continue
            },
        ));
    }
    gate.wait();
    let states: Vec<_> = jobs.iter().map(|job| job.wait().state).collect();
    assert_eq!(
        states
            .iter()
            .filter(|state| **state == DeepState::Completed)
            .count(),
        1
    );
    assert_eq!(
        states
            .iter()
            .filter(|state| **state == DeepState::StaleRevision)
            .count(),
        1
    );
    let old = session.revision(2).unwrap();
    let repeated = session.request_deep(target(&session, 2, 0), DeepBudget::default());
    assert_eq!(repeated.wait().result_revision, Some(3));
    assert_eq!(session.revision(1).unwrap().revision, 1);
    assert_eq!(old.revision, 2);
    assert_eq!(session.revision(3).unwrap().pages, old.pages);
}

#[test]
fn snapshot_invalidation_during_work_or_after_completion_withholds_values() {
    use volmap_sqlite::inspection::ScanControl;
    let (_dir, path, session) = fixture("CREATE TABLE t(value); INSERT INTO t VALUES('SECRET');");
    let selector = target(&session, 1, 0);
    let mutation = path.clone();
    let job =
        session.request_deep_observed(selector.clone(), DeepBudget::default(), move |status| {
            if status.coverage.phase == "publication" {
                let mut bytes = std::fs::read(&mutation).unwrap();
                bytes[60] ^= 1;
                std::fs::write(&mutation, bytes).unwrap();
            }
            ScanControl::Continue
        });
    assert_eq!(job.wait().state, DeepState::InvalidatedSnapshot);
    assert_eq!(
        session.deep_result(&job.id, &selector),
        Err(DeepState::InvalidatedSnapshot)
    );
    assert!(session.revision(2).is_err());
    assert!(
        !serde_json::to_string(&session.evidence())
            .unwrap()
            .contains("SECRET")
    );

    let (_dir, path, session) = fixture("CREATE TABLE t(value); INSERT INTO t VALUES('SECRET');");
    let selector = target(&session, 1, 0);
    let job = session.request_deep(selector.clone(), DeepBudget::default());
    assert_eq!(job.wait().state, DeepState::Completed);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[60] ^= 1;
    std::fs::write(&path, bytes).unwrap();
    assert_eq!(
        session.deep_result(&job.id, &selector),
        Err(DeepState::InvalidatedSnapshot)
    );
    assert_eq!(
        session.deep_status(&job.id).unwrap().state,
        DeepState::InvalidatedSnapshot
    );
}

#[test]
fn broken_and_cyclic_overflow_stop_at_the_validated_prefix() {
    use volmap_sqlite::inspection::TraversalKind;
    for cyclic in [false, true] {
        let (_dir, path, initial) = fixture(&format!(
            "CREATE TABLE t(value); INSERT INTO t VALUES('{}');",
            "PRIVATE".repeat(1000)
        ));
        let graph = initial.revision(1).unwrap();
        let traversal = graph
            .traversals
            .iter()
            .find(|t| t.kind == TraversalKind::Overflow)
            .unwrap();
        let first = traversal.validated_prefix[0].page_number;
        let next = if cyclic { first } else { 999_999 };
        let mut bytes = std::fs::read(&path).unwrap();
        let offset = (first as usize - 1) * 512;
        bytes[offset..offset + 4].copy_from_slice(&next.to_be_bytes());
        std::fs::write(&path, &bytes).unwrap();
        let session = Arc::new(InspectionSession::open(&path).unwrap());
        let selector = target(&session, 1, 0);
        let job = session.request_deep(selector.clone(), DeepBudget::default());
        let receipt = job.wait();
        assert_eq!(receipt.state, DeepState::Failed);
        assert_eq!(receipt.coverage.overflow_pages.len(), 1);
        assert_eq!(receipt.coverage.stopping_page, Some(next));
        assert!(
            receipt
                .coverage
                .remainder_bytes
                .as_ref()
                .unwrap()
                .parse::<u64>()
                .unwrap()
                > 0
        );
        assert_eq!(session.status().revision, Some(1));
        assert_eq!(
            session.deep_result(&job.id, &selector),
            Err(DeepState::Failed)
        );
        assert!(!serde_json::to_string(&receipt).unwrap().contains("PRIVATE"));
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn integer_widths_empty_values_and_malformed_records_remain_typed() {
    let (_dir, path, session) = fixture("CREATE TABLE t(a,b,c,d,e,f,g,h,i,j);
        INSERT INTO t VALUES(0,1,-128,-32768,-8388608,-2147483648,-140737488355328,9223372036854775807,'',X'');");
    let selector = target(&session, 1, 0);
    let job = session.request_deep(selector.clone(), DeepBudget::default());
    assert_eq!(job.wait().state, DeepState::Completed);
    let result = session.deep_result(&job.id, &selector).unwrap();
    let serials: Vec<_> = result
        .values
        .iter()
        .map(|v| v.field.serial_type.as_str())
        .collect();
    assert_eq!(
        serials,
        ["8", "9", "1", "2", "3", "4", "5", "6", "13", "12"]
    );
    let values = serde_json::to_value(&result).unwrap();
    for (index, expected) in [
        "0",
        "1",
        "-128",
        "-32768",
        "-8388608",
        "-2147483648",
        "-140737488355328",
        "9223372036854775807",
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(values["values"][index]["value"]["value"], *expected);
    }
    let graph = session.revision(1).unwrap();
    let local = graph.pages[1].detail.cells[0]
        .local_payload
        .as_ref()
        .unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[usize::try_from(local.file_offset).unwrap() + 1] = 10; // Reserved serial type.
    std::fs::write(&path, bytes).unwrap();
    let malformed = Arc::new(InspectionSession::open(&path).unwrap());
    let job = malformed.request_deep(target(&malformed, 1, 0), DeepBudget::default());
    assert!(matches!(
        job.wait().state,
        DeepState::Failed | DeepState::InvalidTarget
    ));
    assert_eq!(malformed.status().revision, Some(1));
}

#[test]
fn session_admission_bounds_workers_and_retained_jobs_without_losing_revisions() {
    use std::sync::mpsc;
    use volmap_sqlite::inspection::{DeepLimits, ScanControl};
    let (_dir, path, _) = fixture("CREATE TABLE t(value); INSERT INTO t VALUES('SECRET');");
    let session = Arc::new(
        InspectionSession::open(&path)
            .unwrap()
            .with_deep_limits(DeepLimits {
                max_jobs: 1,
                max_concurrent_jobs: 1,
                ..DeepLimits::default()
            }),
    );
    // Invalid selectors consume no admission slot.
    let mut invalid = target(&session, 1, 0);
    invalid.cell_index = 99;
    let rejected = session.request_deep(invalid, DeepBudget::default());
    assert_eq!(rejected.wait().state, DeepState::InvalidTarget);
    assert!(session.deep_status(&rejected.id).is_none());
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let first = session.request_deep_observed(
        target(&session, 1, 0),
        DeepBudget::default(),
        move |status| {
            if status.coverage.phase == "queued" {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            }
            ScanControl::Continue
        },
    );
    entered_rx.recv().unwrap();
    let rejected = session.request_deep(target(&session, 1, 0), DeepBudget::default());
    assert_eq!(rejected.wait().state, DeepState::BudgetStopped);
    assert!(session.deep_status(&rejected.id).is_none());
    release_tx.send(()).unwrap();
    assert_eq!(first.wait().state, DeepState::Completed);
    let rejected = session.request_deep(target(&session, 2, 0), DeepBudget::default());
    assert_eq!(rejected.wait().coverage.reason, "session_job_budget");
    assert_eq!(session.revision(1).unwrap().revision, 1);
    assert_eq!(session.revision(2).unwrap().revision, 2);
}

#[test]
fn cancellation_at_publication_preserves_the_previous_revision() {
    use volmap_sqlite::inspection::ScanControl;
    let (_dir, _path, session) = fixture("CREATE TABLE t(value); INSERT INTO t VALUES('SECRET');");
    let job =
        session.request_deep_observed(target(&session, 1, 0), DeepBudget::default(), |status| {
            if status.coverage.phase == "publication" {
                ScanControl::Cancel
            } else {
                ScanControl::Continue
            }
        });
    assert_eq!(job.wait().state, DeepState::Cancelled);
    assert_eq!(session.status().revision, Some(1));
}
