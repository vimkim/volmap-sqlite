use rusqlite::Connection;
use std::sync::Arc;
use volmap_sqlite::inspection::InspectionSession;
use volmap_sqlite::terminal::{Key, TerminalFlow};

#[test]
fn navigates_schema_to_page_and_cell_using_shared_physical_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.sqlite");
    Connection::open(&path).unwrap().execute_batch("CREATE TABLE inventory(sku TEXT, quantity INTEGER); INSERT INTO inventory VALUES('PRIVATE_ROW',7);").unwrap();
    let session = Arc::new(InspectionSession::open(&path).unwrap());
    let mut terminal = TerminalFlow::new(Arc::clone(&session));
    let screen = terminal.screen(120, 40).join("\n");
    assert!(screen.contains("Main-file image"));
    assert!(screen.contains("Schema objects"));
    assert!(screen.contains("Freelist"));
    assert!(!screen.contains("PRIVATE_ROW"));
    terminal.key(Key::Enter);
    assert!(terminal.screen(120, 40).join("\n").contains("inventory"));
    terminal.key(Key::Enter);
    let screen = terminal.screen(120, 40).join("\n");
    assert!(screen.contains("CREATE TABLE inventory"));
    terminal.key(Key::Enter);
    let screen = terminal.screen(120, 40).join("\n");
    assert!(screen.contains("Page 2"));
    assert!(screen.contains("Cell 0"));
    assert!(screen.contains("TableLeaf"));
    terminal.key(Key::Enter);
    let screen = terminal.screen(120, 40).join("\n");
    assert!(screen.contains("Database / Schema objects / inventory / Page 2 / Cell 0"));
    let cell = &session.graph().unwrap().pages[1].detail.cells[0];
    assert!(screen.contains(&format!("Offset: {}", cell.offset)));
    assert!(!screen.contains("PRIVATE_ROW"));
    terminal.key(Key::Back);
    assert!(
        terminal
            .screen(120, 40)
            .join("\n")
            .contains("Database / Schema objects / inventory / Page 2")
    );
}

#[test]
fn explicit_deep_inspection_publishes_values_only_for_selected_cell_and_revision() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deep.sqlite");
    Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TABLE t(a); INSERT INTO t VALUES('SELECTED_SECRET'),('OTHER_SECRET');",
        )
        .unwrap();
    let session = Arc::new(InspectionSession::open(&path).unwrap());
    let mut terminal = TerminalFlow::new(Arc::clone(&session));
    terminal.key(Key::Char('g'));
    for c in "2:0".chars() {
        terminal.key(Key::Char(c));
    }
    terminal.key(Key::Enter);
    assert!(
        !terminal
            .screen(140, 45)
            .join("\n")
            .contains("SELECTED_SECRET")
    );
    terminal.key(Key::Char('d'));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let screen = loop {
        let screen = terminal.screen(140, 45).join("\n");
        if screen.contains("SELECTED_SECRET") {
            break screen;
        }
        assert!(std::time::Instant::now() < deadline, "{screen}");
        std::thread::yield_now();
    };
    assert!(screen.contains("Revision 2"));
    assert!(screen.contains("Deep: Completed"));
    assert!(!screen.contains("OTHER_SECRET"));
    assert!(
        !serde_json::to_string(&*session.revision(2).unwrap())
            .unwrap()
            .contains("SELECTED_SECRET")
    );
    terminal.key(Key::Char('['));
    let screen = terminal.screen(140, 45).join("\n");
    assert!(screen.contains("Revision 1"));
    assert!(!screen.contains("SELECTED_SECRET"));
    terminal.key(Key::Char(']'));
    assert!(
        !terminal
            .screen(140, 45)
            .join("\n")
            .contains("SELECTED_SECRET")
    );
    terminal.key(Key::Back);
    assert!(
        !terminal
            .screen(140, 45)
            .join("\n")
            .contains("SELECTED_SECRET")
    );
}

#[test]
fn production_terminal_harness_navigates_and_restores_the_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pty.sqlite");
    Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE t(a); INSERT INTO t VALUES('PRIVATE_ROW');")
        .unwrap();
    let output = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/support/terminal_pty.py"
        ))
        .arg(env!("CARGO_BIN_EXE_volmap-sqlite"))
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fixture(
    sql: &str,
) -> (
    tempfile::TempDir,
    std::path::PathBuf,
    Arc<InspectionSession>,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("states.sqlite");
    Connection::open(&path).unwrap().execute_batch(sql).unwrap();
    let session = Arc::new(InspectionSession::open(&path).unwrap());
    (dir, path, session)
}

fn goto(terminal: &mut TerminalFlow, selector: &str) {
    terminal.key(Key::Char('g'));
    for c in selector.chars() {
        terminal.key(Key::Char(c));
    }
    terminal.key(Key::Enter);
}

#[test]
fn rootless_empty_help_invalid_selectors_and_narrow_sizes_remain_usable() {
    let (_dir, _path, session) = fixture("CREATE VIEW rootless AS SELECT 1;");
    let mut terminal = TerminalFlow::new(session);
    terminal.key(Key::Enter);
    terminal.key(Key::Enter);
    let screen = terminal.screen(120, 40).join("\n");
    assert!(screen.contains("Rootless"));
    assert!(screen.contains("No targets"));
    terminal.key(Key::Char('?'));
    assert!(
        terminal
            .screen(120, 40)
            .join("\n")
            .contains("Keyboard help")
    );
    terminal.key(Key::Back);
    goto(&mut terminal, "999:65535");
    assert!(
        terminal
            .screen(120, 40)
            .join("\n")
            .contains("Invalid selector")
    );
    for (width, height) in [(30, 10), (1, 1), (0, 0), (40, 12), (120, 40)] {
        let screen = terminal.screen(width, height);
        assert!(screen.len() <= usize::from(height));
        assert!(screen.iter().all(|line| line.len() <= usize::from(width)));
    }
    terminal.key(Key::Char('!'));
    assert!(
        terminal
            .screen(120, 40)
            .join("\n")
            .contains("No diagnostics")
    );
}

#[test]
fn cancellation_partial_coverage_and_invalidation_withhold_values() {
    use volmap_sqlite::inspection::{ScanControl, SessionState};
    let (_dir, path, _) = fixture("CREATE TABLE t(a); INSERT INTO t VALUES('PRIVATE_ROW');");
    let session = Arc::new(InspectionSession::begin(&path).unwrap());
    session.advance().unwrap();
    let mut terminal = TerminalFlow::new(Arc::clone(&session));
    terminal.key(Key::Char('c'));
    assert_eq!(session.status().state, SessionState::Cancelled);
    let screen = terminal.screen(120, 40).join("\n");
    assert!(screen.contains("Cancelled"));
    assert!(screen.contains("Fast coverage"));
    let session = Arc::new(InspectionSession::begin(&path).unwrap());
    session.scan(|_| ScanControl::Continue).unwrap();
    let mut terminal = TerminalFlow::new(Arc::clone(&session));
    goto(&mut terminal, "2:0");
    terminal.key(Key::Char('d'));
    terminal.key(Key::Char('c'));
    assert!(!terminal.screen(120, 40).join("\n").contains("PRIVATE_ROW"));
    Connection::open(&path)
        .unwrap()
        .execute_batch("INSERT INTO t VALUES('CHANGED');")
        .unwrap();
    let screen = terminal.screen(120, 40).join("\n");
    assert!(screen.contains("Invalidated"));
    assert!(!screen.contains("PRIVATE_ROW"));
    assert!(!screen.contains("CHANGED"));
    terminal.key(Key::Char('d'));
    assert!(!terminal.screen(120, 40).join("\n").contains("PRIVATE_ROW"));
}

#[tokio::test]
async fn adapters_show_the_same_snapshot_coordinates_diagnostics_and_sidecars() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let (_dir, path, _) = fixture("CREATE TABLE t(a); INSERT INTO t VALUES('PRIVATE_ROW');");
    std::fs::write(path.with_extension("sqlite-wal"), b"invalid WAL").unwrap();
    let session = Arc::new(InspectionSession::open(&path).unwrap());
    let snapshot = session.status().snapshot_id;
    let response = volmap_sqlite::web::atlas_router(Arc::clone(&session))
        .oneshot(
            Request::builder()
                .header("host", "localhost")
                .header("origin", "http://localhost")
                .uri(format!("/api/snapshots/{snapshot}/revisions/1"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let web: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let mut terminal = TerminalFlow::new(session);
    let root = terminal.screen(160, 60).join("\n");
    assert!(root.contains(web["snapshot"]["id"].as_str().unwrap()));
    assert!(root.contains("Revision 1"));
    assert!(root.contains("Main-file image"));
    assert!(root.contains("wal malformed"), "{root}");
    goto(&mut terminal, "2:0");
    let screen = terminal.screen(160, 60).join("\n");
    assert!(screen.contains(&format!(
        "Offset: {}",
        web["pages"][1]["detail"]["cells"][0]["offset"]
    )));
    assert!(!screen.contains("PRIVATE_ROW"));
    terminal.key(Key::Char('!'));
    let screen = terminal.screen(160, 60).join("\n");
    for diagnostic in web["sidecars"][0]["diagnostics"].as_array().unwrap() {
        assert!(screen.contains(diagnostic["code"].as_str().unwrap()));
    }
}

#[test]
fn unicode_names_remain_readable_but_terminal_controls_are_escaped() {
    let (_dir, _path, session) = fixture("CREATE TABLE \"표\u{1b}[2J\"(a);");
    let mut terminal = TerminalFlow::new(session);
    terminal.key(Key::Enter);
    let screen = terminal.screen(120, 40).join("\n");
    assert!(screen.contains('표'));
    assert!(!screen.contains('\u{1b}'));
    assert!(screen.contains("\\u{1b}[2J"));
}

#[test]
fn allocation_entry_points_preserve_pointer_map_and_freelist_evidence() {
    let (_dir,_path,session) = fixture("PRAGMA page_size=512; PRAGMA auto_vacuum=INCREMENTAL; VACUUM;
        CREATE TABLE kept(a); CREATE TABLE discarded(a);
        WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<30) INSERT INTO discarded SELECT zeroblob(800) FROM n;
        DROP TABLE discarded;");
    let graph = session.graph().unwrap();
    assert!(!graph.freelist.trunks.is_empty());
    assert!(graph.pointer_map.applicable);
    let mut terminal = TerminalFlow::new(session);
    terminal.key(Key::Down);
    terminal.key(Key::Down);
    terminal.key(Key::Enter);
    let screen = terminal.screen(160, 60).join("\n");
    assert!(screen.contains("Freelist coverage: Complete"));
    terminal.key(Key::Enter);
    assert!(terminal.screen(160, 60).join("\n").contains("Freelist"));
    terminal.key(Key::Back);
    terminal.key(Key::Back);
    terminal.key(Key::Down);
    terminal.key(Key::Enter);
    assert!(
        terminal
            .screen(160, 60)
            .join("\n")
            .contains("Pointer maps applicable: true")
    );
    terminal.key(Key::Enter);
    let screen = terminal.screen(160, 100).join("\n");
    let first = &graph.pointer_map.pages[0].entries[0];
    assert!(
        screen.contains(&format!("Pointer-map target {:?}", first.target)),
        "{screen}"
    );
    assert!(screen.contains(&format!("file offset {}", first.evidence.range.file_offset)));
}

#[test]
fn redirected_terminal_mode_fails_without_dumping_application_data() {
    let (_dir, path, _) = fixture("CREATE TABLE t(a); INSERT INTO t VALUES('PRIVATE_ROW');");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_volmap-sqlite"))
        .arg(&path)
        .arg("--terminal")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("interactive"));
    assert!(!error.contains("PRIVATE_ROW"));
    assert!(!error.contains('\u{1b}'));
}

#[test]
fn compact_frames_navigate_and_long_evidence_can_be_scrolled() {
    let name = "long_schema_name_".repeat(8);
    let (_dir, _path, session) = fixture(&format!(
        "CREATE TABLE \"{name}\"(a TEXT /* DECLARATION_TAIL */); INSERT INTO \"{name}\" VALUES('PRIVATE_ROW');"
    ));
    let mut terminal = TerminalFlow::new(session);
    assert!(
        terminal
            .screen(39, 40)
            .join("\n")
            .contains("Schema objects")
    );
    terminal.key(Key::Enter);
    terminal.key(Key::Enter);
    let mut visible = String::new();
    for _ in 0..10 {
        visible.push_str(&terminal.screen(39, 40).join("\n"));
        terminal.key(Key::Char('l'));
    }
    assert!(visible.contains("DECLARATION_TAIL"));
    terminal.key(Key::Enter);
    terminal.key(Key::Enter);
    assert!(
        terminal
            .screen(80, 24)
            .join("\n")
            .contains("Page 2 / Cell 0")
    );
}

#[test]
fn terminal_deep_budgets_are_explicit_and_fail_without_values() {
    use volmap_sqlite::inspection::DeepBudget;
    let (_dir, _path, session) = fixture("CREATE TABLE t(a); INSERT INTO t VALUES('PRIVATE_ROW');");
    let mut terminal = TerminalFlow::new(session).with_deep_budget(DeepBudget {
        max_payload_bytes: 0,
        ..DeepBudget::default()
    });
    goto(&mut terminal, "2:0");
    terminal.key(Key::Char('d'));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let screen = terminal.screen(120, 40).join("\n");
        assert!(!screen.contains("PRIVATE_ROW"));
        if screen.contains("BudgetStopped") {
            let compact = terminal.screen(30, 10).join("\n");
            assert!(compact.contains("Deep: BudgetStopped 0B 0V"), "{compact}");
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{screen}");
        std::thread::yield_now();
    }
}

#[test]
fn page_header_freeblocks_and_record_state_are_visible_shared_evidence() {
    let (_dir, _path, session) = fixture(
        "CREATE TABLE t(a); INSERT INTO t VALUES('first-long-value'),('middle-long-value'),('last-long-value'); DELETE FROM t WHERE rowid=2;",
    );
    let graph = session.graph().unwrap();
    let page = &graph.pages[1];
    assert!(!page.detail.freeblocks.is_empty());
    let mut terminal = TerminalFlow::new(session);
    goto(&mut terminal, "2:0");
    let screen = terminal.screen(160, 100).join("\n");
    assert!(screen.contains("Cells declared: 2"), "{screen}");
    assert!(screen.contains(&format!(
        "Content start: {}",
        page.detail.header.as_ref().unwrap().content_start
    )));
    assert!(screen.contains(&format!(
        "Freeblock at {} -> {}",
        page.detail.freeblocks[0].offset, page.detail.freeblocks[0].next
    )));
    assert!(screen.contains("Record state: Complete"));
}

#[test]
fn explicitly_requested_text_tails_are_accessible_in_compact_evidence() {
    let text = format!("{}VISIBLE_VALUE_TAIL", "x".repeat(180));
    let (_dir, _path, session) = fixture(&format!(
        "CREATE TABLE t(a); INSERT INTO t VALUES('{text}');"
    ));
    let mut terminal = TerminalFlow::new(session);
    goto(&mut terminal, "2:0");
    terminal.key(Key::Char('d'));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let screen = terminal.screen(120, 40).join("\n");
        if screen.contains("Deep: Completed") {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    let mut visible = String::new();
    for _ in 0..16 {
        visible.push_str(&terminal.screen(39, 40).join("\n"));
        terminal.key(Key::Char('l'));
    }
    assert!(visible.contains("VISIBLE_VALUE_TAIL"));
}
