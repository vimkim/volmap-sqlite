use rusqlite::Connection;
use tempfile::TempDir;
use volmap_sqlite::inspection::{
    CoverageReason, InspectionError, InspectionSession, ScanControl, SessionState,
};

#[test]
fn progress_is_deterministic_and_only_a_finished_revision_is_navigable() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("main.sqlite");
    Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY);")
        .unwrap();
    let session = InspectionSession::begin(&path).unwrap();
    assert_eq!(session.status().state, SessionState::Scanning);
    assert!(session.revision(1).is_err());
    let mut progress = Vec::new();
    session
        .scan(|status| {
            assert!(session.revision(1).is_err());
            progress.push((status.progress.completed, status.progress.total));
            ScanControl::Continue
        })
        .unwrap();
    assert_eq!(
        progress,
        vec![
            (0, None),
            (0, Some(2)),
            (1, Some(2)),
            (2, Some(2)),
            (2, Some(2))
        ]
    );
    assert_eq!(session.status().state, SessionState::Published);
    let revision = session.revision(1).unwrap();
    assert_eq!(
        revision.pages.iter().map(|p| p.number).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(revision.coverage.evaluated, 2);
    assert_eq!(revision.coverage.remainder, Some(0));
}

fn fixture(path: &std::path::Path) {
    Connection::open(path)
        .unwrap()
        .execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY);")
        .unwrap();
}

#[test]
fn cancelled_and_stopped_prefixes_record_exact_coverage() {
    for (control, state, reason) in [
        (
            ScanControl::Cancel,
            SessionState::Cancelled,
            CoverageReason::Cancelled,
        ),
        (
            ScanControl::Stop,
            SessionState::Stopped,
            CoverageReason::OperatorStop,
        ),
    ] {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("main.sqlite");
        fixture(&path);
        let session = InspectionSession::begin(&path).unwrap();
        session
            .scan(|s| {
                if s.progress.completed == 1 {
                    control
                } else {
                    ScanControl::Continue
                }
            })
            .unwrap();
        assert_eq!(session.status().state, state);
        let revision = session.revision(1).unwrap();
        assert_eq!(revision.coverage.reason, reason);
        assert_eq!(revision.coverage.evaluated, 1);
        assert_eq!(revision.coverage.next_page, Some(2));
        assert_eq!(revision.coverage.remainder, Some(1));
        assert_eq!(revision.pages.len(), 1);
        assert!(session.status().diagnostic.is_none());
        assert!(matches!(
            session.scan(|_| ScanControl::Continue),
            Err(InspectionError::NotScanning)
        ));
    }
}

#[test]
fn cancellation_before_geometry_retains_unknown_remainder_without_a_revision() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("main.sqlite");
    fixture(&path);
    let session = InspectionSession::begin(&path).unwrap();
    session.scan(|_| ScanControl::Cancel).unwrap();
    let status = session.status();
    assert_eq!(status.state, SessionState::Cancelled);
    assert_eq!(status.coverage.unwrap().remainder, None);
    assert!(session.revision(1).is_err());
}

#[test]
fn mutation_and_replacement_at_a_progress_boundary_invalidate_and_retain_evidence() {
    for replace in [false, true] {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("main.sqlite");
        fixture(&path);
        let replacement = dir.path().join("replacement.sqlite");
        fixture(&replacement);
        let session = InspectionSession::begin(&path).unwrap();
        let result = session.scan(|s| {
            if s.progress.completed == 1 {
                if replace {
                    std::fs::rename(&replacement, &path).unwrap();
                } else {
                    use std::os::unix::fs::FileExt;
                    std::fs::OpenOptions::new()
                        .write(true)
                        .open(&path)
                        .unwrap()
                        .write_at(&[42], 4096)
                        .unwrap();
                }
            }
            ScanControl::Continue
        });
        assert!(matches!(result, Err(InspectionError::Invalidated)));
        assert_eq!(session.status().state, SessionState::Invalidated);
        assert_eq!(session.evidence().unwrap().pages.len(), 1);
        assert_eq!(
            session.evidence().unwrap().coverage.reason,
            CoverageReason::InputChanged
        );
        assert!(matches!(
            session.revision(1),
            Err(InspectionError::Invalidated)
        ));
        assert!(matches!(
            session.advance(),
            Err(InspectionError::Invalidated)
        ));
        assert!(matches!(
            session.scan(|_| ScanControl::Continue),
            Err(InspectionError::Invalidated)
        ));
    }
}

#[test]
fn fingerprint_detects_every_sidecar_addition_removal_mutation_and_replacement() {
    for suffix in ["-wal", "-journal", "-shm"] {
        for change in ["add", "remove", "mutate", "replace", "resize", "metadata"] {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("main.sqlite");
            fixture(&path);
            let sidecar = dir.path().join(format!("main.sqlite{suffix}"));
            if change != "add" {
                std::fs::write(&sidecar, [1, 2, 3]).unwrap();
            }
            let session = InspectionSession::begin(&path).unwrap();
            let result = session.scan(|s| {
                // After the last unit but before publication is still inspection work.
                if s.progress.completed == 2 {
                    match change {
                        "add" => std::fs::write(&sidecar, [1, 2, 3]).unwrap(),
                        "remove" => std::fs::remove_file(&sidecar).unwrap(),
                        "mutate" => std::fs::write(&sidecar, [3, 2, 1]).unwrap(),
                        "replace" => {
                            let other = dir.path().join("other");
                            std::fs::write(&other, [1, 2, 3]).unwrap();
                            std::fs::rename(&other, &sidecar).unwrap();
                        }
                        "resize" => std::fs::OpenOptions::new()
                            .write(true)
                            .open(&sidecar)
                            .unwrap()
                            .set_len(1)
                            .unwrap(),
                        "metadata" => {
                            use std::os::unix::fs::PermissionsExt;
                            std::fs::set_permissions(
                                &sidecar,
                                std::fs::Permissions::from_mode(0o400),
                            )
                            .unwrap();
                        }
                        _ => unreachable!(),
                    }
                }
                ScanControl::Continue
            });
            assert!(
                matches!(result, Err(InspectionError::Invalidated)),
                "{suffix}: {change}"
            );
            assert_eq!(session.evidence().unwrap().pages.len(), 2);
            assert_eq!(
                session.status().diagnostic.unwrap().affected_inputs,
                vec![&suffix[1..]]
            );
        }
    }
}

#[test]
fn published_readers_keep_an_immutable_revision_after_invalidation() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("main.sqlite");
    fixture(&path);
    let session = InspectionSession::open(&path).unwrap();
    let revision = session.revision(1).unwrap();
    let before = serde_json::to_string(&*revision).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(session.status().state, SessionState::Invalidated);
    assert!(session.revision(1).is_err());
    assert_eq!(serde_json::to_string(&*revision).unwrap(), before);
    assert_eq!(
        session.evidence().unwrap().snapshot.id,
        revision.snapshot.id
    );
    assert_eq!(session.evidence().unwrap().pages, revision.pages);
}

#[test]
fn fatal_geometry_has_unknown_coverage_and_no_navigable_revision() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("bad.sqlite");
    std::fs::write(&path, b"bad").unwrap();
    let session = InspectionSession::begin(&path).unwrap();
    assert!(matches!(
        session.scan(|_| ScanControl::Continue),
        Err(InspectionError::IncompleteHeader)
    ));
    let status = session.status();
    assert_eq!(status.state, SessionState::Fatal);
    assert_eq!(status.coverage.unwrap().remainder, None);
    assert_eq!(status.diagnostic.unwrap().code, "fatal_geometry");
    assert!(session.revision(1).is_err());
}

#[test]
fn a_worker_can_pause_at_progress_while_readers_query_and_the_operator_replaces_input() {
    use std::sync::{Arc, mpsc};
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("main.sqlite");
    let replacement = dir.path().join("replacement.sqlite");
    fixture(&path);
    fixture(&replacement);
    let session = Arc::new(InspectionSession::begin(&path).unwrap());
    let worker_session = Arc::clone(&session);
    let (reached, wait_for_progress) = mpsc::sync_channel(0);
    let (resume, wait_for_resume) = mpsc::sync_channel(0);
    let worker = std::thread::spawn(move || {
        worker_session.scan(|s| {
            if s.progress.completed == 1 {
                reached.send(()).unwrap();
                wait_for_resume.recv().unwrap();
            }
            ScanControl::Continue
        })
    });
    wait_for_progress.recv().unwrap();
    assert_eq!(session.status().progress.completed, 1);
    assert!(session.revision(1).is_err());
    std::fs::rename(replacement, path).unwrap();
    resume.send(()).unwrap();
    assert!(matches!(
        worker.join().unwrap(),
        Err(InspectionError::Invalidated)
    ));
    assert_eq!(session.evidence().unwrap().pages.len(), 1);
}

#[test]
fn cancellation_can_interrupt_verification_without_locking_status_or_publishing_unchecked_facts() {
    use std::sync::{Arc, mpsc};
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("main.sqlite");
    fixture(&path);
    let session = Arc::new(InspectionSession::begin(&path).unwrap());
    let worker_session = Arc::clone(&session);
    let (reached, wait_for_verification) = mpsc::sync_channel(0);
    let (resume, wait_for_resume) = mpsc::sync_channel(0);
    let worker = std::thread::spawn(move || {
        worker_session.scan(|status| {
            if status.progress.verifying {
                reached.send(()).unwrap();
                wait_for_resume.recv().unwrap();
            }
            ScanControl::Continue
        })
    });
    wait_for_verification.recv().unwrap();
    assert!(session.status().progress.verifying);
    session.stop(ScanControl::Cancel).unwrap();
    assert_eq!(session.status().state, SessionState::Cancelled);
    assert!(session.revision(1).is_err());
    resume.send(()).unwrap();
    worker.join().unwrap().unwrap();
    assert!(!session.status().progress.verifying);
    assert!(session.status().diagnostic.is_none());
    assert_eq!(
        session.status().coverage.unwrap().reason,
        CoverageReason::Cancelled
    );
    assert!(session.revision(1).is_err());
}
