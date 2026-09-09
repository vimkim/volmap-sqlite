use rusqlite::Connection;
use tempfile::TempDir;
use volmap_sqlite::inspection::InspectionSession;

fn database() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("private.sqlite");
    Connection::open(&path)
        .unwrap()
        .execute_batch("PRAGMA page_size=512; CREATE TABLE t(x);")
        .unwrap();
    (dir, path)
}

#[test]
fn absent_and_malformed_sidecars_are_disclosed_without_changing_main_file_facts() {
    let (dir, path) = database();
    let before = InspectionSession::open(&path).unwrap().graph().unwrap();
    let absent = serde_json::to_value(&*before).unwrap();
    assert_eq!(absent["sidecars"].as_array().unwrap().len(), 3);
    assert_eq!(absent["sidecars"][0]["state"], "absent");
    for suffix in ["wal", "journal", "shm"] {
        std::fs::write(
            dir.path().join(format!("private.sqlite-{suffix}")),
            b"SECRET_PAYLOAD",
        )
        .unwrap();
    }
    let after = InspectionSession::open(&path).unwrap().graph().unwrap();
    assert_eq!(before.pages, after.pages);
    assert_eq!(before.relationships, after.relationships);
    assert_eq!(before.snapshot.geometry, after.snapshot.geometry);
    let json = serde_json::to_value(&*after).unwrap();
    for sidecar in json["sidecars"].as_array().unwrap() {
        assert_eq!(sidecar["state"], "malformed");
        assert_eq!(sidecar["length"], "14");
        assert!(!sidecar["diagnostics"].as_array().unwrap().is_empty());
    }
    let text = json.to_string();
    assert!(!text.contains("SECRET_PAYLOAD"));
    assert!(!text.contains(dir.path().to_str().unwrap()));
}

#[test]
fn sqlite_wal_frames_expose_validated_commits_and_coordinates_without_overlay() {
    let (dir, path) = database();
    let source = dir.path().join("source.sqlite");
    let connection = Connection::open(&source).unwrap();
    connection.execute_batch("PRAGMA page_size=512; CREATE TABLE t(x); PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; INSERT INTO t VALUES ('SECRET_FRAME_PAYLOAD'); INSERT INTO t VALUES ('SECOND_SECRET');").unwrap();
    std::fs::copy(&source, &path).unwrap();
    let before = InspectionSession::open(&path).unwrap().graph().unwrap();
    std::fs::copy(
        dir.path().join("source.sqlite-wal"),
        dir.path().join("private.sqlite-wal"),
    )
    .unwrap();
    std::fs::copy(
        dir.path().join("source.sqlite-shm"),
        dir.path().join("private.sqlite-shm"),
    )
    .unwrap();
    let after = InspectionSession::open(&path).unwrap().graph().unwrap();
    assert_eq!(before.pages, after.pages);
    let json = serde_json::to_value(&*after).unwrap();
    let wal = &json["sidecars"][0];
    assert_eq!(json["sidecars"][2]["state"], "supported_header");
    assert_eq!(wal["state"], "validated");
    assert_eq!(
        wal["wal"]["checksumByteOrder"],
        if cfg!(target_endian = "little") {
            "little"
        } else {
            "big"
        }
    );
    assert_eq!(wal["wal"]["validatedFrames"], "2");
    assert_eq!(wal["wal"]["lastCommitFrame"], "2");
    assert_eq!(wal["wal"]["frames"][0]["offset"], "32");
    assert_eq!(wal["wal"]["frames"][0]["pageNumber"], 2);
    assert_eq!(wal["wal"]["frames"][0]["databaseSize"], 2);
    assert_eq!(wal["wal"]["frames"][1]["offset"], "568");
    assert_eq!(wal["coverage"]["remainingBytes"], "0");
    assert!(!json.to_string().contains("SECRET"));
}

fn wal_fixture(big: bool) -> Vec<u8> {
    // Fixed worked format examples: one 512-byte zero page, page 2, commit size 2.
    let hex = if big {
        "377f0683002de218000002000000000000000001000000021604cad8bcdda493000000020000000200000001000000021efc78e3fd2bdea5"
    } else {
        "377f0682002de21800000200000000000000000100000002d5cb03138fa4dab80000000200000002000000010000000220637ace7cb13cdd"
    };
    let mut bytes = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect::<Vec<_>>();
    bytes.resize(568, 0);
    bytes
}

#[test]
fn wal_byte_orders_and_damaged_boundaries_preserve_only_the_validated_prefix() {
    for big in [false, true] {
        for damage in [
            "none",
            "header_checksum",
            "frame_checksum",
            "salt",
            "truncated",
            "tail",
        ] {
            let (dir, path) = database();
            let mut bytes = wal_fixture(big);
            match damage {
                "header_checksum" => bytes[24] ^= 1,
                "frame_checksum" => bytes[100] ^= 1,
                "salt" => bytes[40] ^= 1,
                "truncated" => bytes.truncate(50),
                "tail" => bytes.extend([0, 0, 0, 3]),
                _ => {}
            }
            std::fs::write(dir.path().join("private.sqlite-wal"), bytes).unwrap();
            let graph = InspectionSession::open(&path).unwrap().graph().unwrap();
            let json = serde_json::to_value(&*graph).unwrap();
            let evidence = &json["sidecars"][0];
            let wal = &evidence["wal"];
            assert_eq!(
                wal["validatedFrames"],
                if ["none", "tail"].contains(&damage) {
                    "1"
                } else {
                    "0"
                },
                "{damage}"
            );
            assert_eq!(
                wal["lastCommitFrame"],
                if ["none", "tail"].contains(&damage) {
                    serde_json::json!("1")
                } else {
                    serde_json::Value::Null
                }
            );
            if damage == "none" {
                assert_eq!(evidence["state"], "validated");
            } else if damage == "salt" {
                assert_eq!(evidence["state"], "unresolved_tail");
            } else {
                assert_eq!(evidence["state"], "malformed");
                assert!(!evidence["diagnostics"].as_array().unwrap().is_empty());
            }
            if damage == "truncated" {
                assert_eq!(wal["frames"][0]["pageNumber"], 2);
                assert_eq!(wal["frames"][0]["readableBytes"], 18);
                assert_eq!(wal["frames"][0]["storedChecksum"], serde_json::Value::Null);
            }
        }
    }
}

#[test]
fn rollback_header_evidence_does_not_guess_hotness_from_missing_lock_state() {
    for (mode, state, hot) in [
        ("valid", "supported_header", "unknown"),
        ("zero", "inactive", "not_hot"),
        ("empty", "inactive", "not_hot"),
        ("short", "malformed", "not_hot"),
        ("bad_sector", "malformed", "unknown"),
    ] {
        let (dir, path) = database();
        let mut bytes = vec![0; 1032];
        bytes[..8].copy_from_slice(&[0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7]);
        for (offset, value) in [(8, 1_u32), (12, 17), (16, 2), (20, 512), (24, 512)] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
        match mode {
            "zero" => bytes[..28].fill(0),
            "empty" => bytes.clear(),
            "short" => bytes.truncate(20),
            "bad_sector" => bytes[20..24].copy_from_slice(&0_u32.to_be_bytes()),
            _ => {}
        }
        std::fs::write(dir.path().join("private.sqlite-journal"), bytes).unwrap();
        let graph = InspectionSession::open(&path).unwrap().graph().unwrap();
        let json = serde_json::to_value(&*graph).unwrap();
        let journal = &json["sidecars"][1];
        assert_eq!(journal["state"], state, "{mode}");
        assert_eq!(journal["journal"]["hotStatus"], hot, "{mode}");
        assert_eq!(journal["journal"]["reservedLock"], "unknown");
        assert_eq!(journal["journal"]["superJournal"], "not_inspected");
        if mode == "valid" {
            assert_eq!(journal["journal"]["headerValid"], true);
            assert_eq!(journal["coverage"]["scope"], "journal_header");
        }
    }
}

#[test]
fn sqlite_shm_is_native_order_header_evidence_with_independent_validation() {
    let (dir, path) = database();
    let source = dir.path().join("source.sqlite");
    let connection = Connection::open(&source).unwrap();
    connection.execute_batch("PRAGMA page_size=512; CREATE TABLE t(x); PRAGMA journal_mode=WAL; INSERT INTO t VALUES (1);").unwrap();
    let original = std::fs::read(dir.path().join("source.sqlite-shm")).unwrap();
    for damage in ["none", "copy", "checksum", "truncated"] {
        let mut bytes = original.clone();
        match damage {
            "copy" => bytes[56] ^= 1,
            "checksum" => {
                bytes[40] ^= 1;
                bytes[88] ^= 1;
            }
            "truncated" => bytes.truncate(60),
            _ => {}
        }
        std::fs::write(dir.path().join("private.sqlite-shm"), bytes).unwrap();
        let graph = InspectionSession::open(&path).unwrap().graph().unwrap();
        let json = serde_json::to_value(&*graph).unwrap();
        let shm = &json["sidecars"][2];
        assert_eq!(shm["consequence"], "shm_non_authoritative");
        assert_eq!(
            shm["shm"]["byteOrder"],
            if cfg!(target_endian = "little") {
                "native_little"
            } else {
                "native_big"
            }
        );
        if damage == "none" {
            assert_eq!(shm["state"], "supported_header");
            assert_eq!(shm["shm"]["copiesMatch"], true);
            assert_eq!(shm["shm"]["headerChecksumsValid"], true);
            assert_eq!(shm["shm"]["maxFrame"], 1);
            assert_eq!(shm["coverage"]["scope"], "shm_header");
        } else {
            assert_eq!(shm["state"], "malformed");
            assert!(!shm["diagnostics"].as_array().unwrap().is_empty());
        }
    }
}

#[test]
fn stopped_inventory_discloses_presence_without_claiming_sidecar_validation() {
    let (dir, path) = database();
    std::fs::write(dir.path().join("private.sqlite-wal"), wal_fixture(false)).unwrap();
    let session = InspectionSession::begin(&path).unwrap();
    session
        .scan(|s| {
            if s.progress.completed == 1 {
                volmap_sqlite::inspection::ScanControl::Stop
            } else {
                volmap_sqlite::inspection::ScanControl::Continue
            }
        })
        .unwrap();
    let graph = session.graph().unwrap();
    let json = serde_json::to_value(&*graph).unwrap();
    assert_eq!(json["sidecars"][0]["state"], "uninspected");
    assert_eq!(json["sidecars"][0]["consequence"], "wal_not_applied");
    assert_eq!(json["sidecars"][0]["coverage"]["remainingBytes"], "568");
}

#[test]
fn invalidation_retains_discovered_sidecars_before_publication() {
    let (dir, path) = database();
    let wal = dir.path().join("private.sqlite-wal");
    std::fs::write(&wal, wal_fixture(false)).unwrap();
    let session = InspectionSession::begin(&path).unwrap();
    session.advance().unwrap();
    std::fs::remove_file(&wal).unwrap();
    assert_eq!(
        session.status().state,
        volmap_sqlite::inspection::SessionState::Invalidated
    );
    let json = serde_json::to_value(session.evidence().unwrap()).unwrap();
    assert_eq!(json["sidecars"][0]["state"], "uninspected");
    assert_eq!(json["sidecars"][0]["length"], "568");
}

#[test]
fn cancellation_at_a_wal_boundary_publishes_honest_sidecar_coverage() {
    use volmap_sqlite::inspection::{ScanControl, SessionState};
    let (dir, path) = database();
    std::fs::write(dir.path().join("private.sqlite-wal"), wal_fixture(false)).unwrap();
    let session = InspectionSession::begin(&path).unwrap();
    session
        .scan(|s| {
            if s.progress.building_sidecars {
                ScanControl::Cancel
            } else {
                ScanControl::Continue
            }
        })
        .unwrap();
    assert_eq!(session.status().state, SessionState::Cancelled);
    let json = serde_json::to_value(&*session.graph().unwrap()).unwrap();
    assert_eq!(json["sidecars"][0]["coverage"]["reason"], "cancelled");
    assert_eq!(json["sidecars"][0]["wal"]["validatedFrames"], "0");
    assert_eq!(json["sidecars"][0]["coverage"]["remainingBytes"], "536");
    assert!(
        json["sidecars"][0]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn non_regular_sidecar_candidates_are_contained_without_hiding_the_main_file() {
    let (dir, path) = database();
    std::fs::create_dir(dir.path().join("private.sqlite-wal")).unwrap();
    let graph = InspectionSession::open(&path).unwrap().graph().unwrap();
    let json = serde_json::to_value(&*graph).unwrap();
    assert_eq!(graph.pages.len(), 2);
    assert_eq!(json["sidecars"][0]["state"], "unreadable");
}

#[test]
fn wal_frame_budget_reports_the_unvisited_boundary_without_corruption() {
    use volmap_sqlite::inspection::{ScanControl, SidecarBudget, TraversalBudget};
    let (dir, path) = database();
    std::fs::write(dir.path().join("private.sqlite-wal"), wal_fixture(false)).unwrap();
    let session = InspectionSession::begin_with_budgets(
        &path,
        TraversalBudget::default(),
        SidecarBudget { max_wal_frames: 0 },
    )
    .unwrap();
    session.scan(|_| ScanControl::Continue).unwrap();
    let json = serde_json::to_value(&*session.graph().unwrap()).unwrap();
    let wal = &json["sidecars"][0];
    assert_eq!(wal["state"], "partial");
    assert_eq!(wal["coverage"]["reason"], "budget");
    assert_eq!(wal["coverage"]["evaluatedBytes"], "32");
    assert_eq!(wal["coverage"]["remainingBytes"], "536");
    assert!(wal["diagnostics"].as_array().unwrap().is_empty());
}

#[test]
fn reused_sqlite_wal_discloses_old_generation_tail_without_claiming_corruption() {
    let (dir, path) = database();
    let source = dir.path().join("source.sqlite");
    let connection = Connection::open(&source).unwrap();
    connection.execute_batch("PRAGMA page_size=512; CREATE TABLE t(x); PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; INSERT INTO t VALUES (1); INSERT INTO t VALUES (2); INSERT INTO t VALUES (3); PRAGMA wal_checkpoint(FULL); INSERT INTO t VALUES (4);").unwrap();
    std::fs::copy(&source, &path).unwrap();
    std::fs::copy(
        dir.path().join("source.sqlite-wal"),
        dir.path().join("private.sqlite-wal"),
    )
    .unwrap();
    let graph = InspectionSession::open(&path).unwrap().graph().unwrap();
    let json = serde_json::to_value(&*graph).unwrap();
    let evidence = &json["sidecars"][0];
    assert_eq!(evidence["state"], "unresolved_tail");
    assert_eq!(evidence["coverage"]["reason"], "salt_boundary");
    assert_eq!(evidence["wal"]["validatedFrames"], "1");
    assert_eq!(evidence["wal"]["lastCommitFrame"], "1");
    assert_eq!(evidence["wal"]["frames"][1]["state"], "unresolved");
    assert_eq!(evidence["coverage"]["remainingBytes"], "536");
}
