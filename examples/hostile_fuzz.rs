//! Replay one bounded hostile-input candidate through production public boundaries.
#![allow(dead_code)] // Shared corpus helpers include checks used by the integration suite.
#[path = "../tests/support/hostile.rs"]
mod hostile;

use axum::{
    body::{Body, to_bytes},
    http::{Request, Uri},
};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
    sync::Arc,
};
use tower::ServiceExt;
use volmap_sqlite::{
    inspection::{DeepBudget, DeepSelector, DeepState, EntityIdentity, ScanControl},
    terminal::{Key, TerminalFlow},
    web::atlas_router,
};

const BASE: &[u8] = include_bytes!("../tests/corpus/damage/out-of-range-link.sqlite");
const CELLS: &[u8] = include_bytes!("../tests/corpus/damage/impossible-record-header.sqlite");

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args
        .get(1)
        .is_some_and(|arg| arg == "--private-semantic-helper")
    {
        // Fault-injection helper: only the private copy carries the reply bytes.
        let copy = std::fs::read("snapshot.sqlite").unwrap();
        let count = u32::from_be_bytes(copy[60..64].try_into().unwrap()) as usize;
        std::io::stdout()
            .write_all(&copy[512..512 + count])
            .unwrap();
        return;
    }
    assert_eq!(args.len(), 3, "usage: hostile_fuzz TARGET INPUT");
    let metadata = std::fs::metadata(&args[2]).unwrap();
    assert!(metadata.len() <= hostile::MAX_INPUT as u64);
    let bytes = std::fs::read(&args[2]).unwrap();
    run(&args[1], &bytes).await;
}

async fn run(target: &str, bytes: &[u8]) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("main.sqlite");
    match target {
        "database" | "traversal" | "records" => {
            std::fs::write(&path, bytes).unwrap();
            let session = hostile::begin(&path);
            let _ = session.scan(|_| ScanControl::Continue);
            check(&session, directory.path()).await;
            if target == "records" {
                deep(&session, bytes);
            }
            session.shutdown();
        }
        "sidecars" => {
            std::fs::write(&path, BASE).unwrap();
            let plain = hostile::begin(&path);
            plain.scan(|_| ScanControl::Continue).unwrap();
            let before = hostile::physical(serde_json::to_value(&*plain.graph().unwrap()).unwrap());
            for suffix in ["wal", "journal", "shm"] {
                std::fs::write(
                    directory.path().join(format!("main.sqlite-{suffix}")),
                    bytes,
                )
                .unwrap();
            }
            let session = hostile::begin(&path);
            session.scan(|_| ScanControl::Continue).unwrap();
            check(&session, directory.path()).await;
            assert_eq!(
                before,
                hostile::physical(serde_json::to_value(&*session.graph().unwrap()).unwrap())
            );
            assert_eq!(std::fs::read(&path).unwrap(), BASE);
            for suffix in ["wal", "journal", "shm"] {
                assert_eq!(
                    std::fs::read(directory.path().join(format!("main.sqlite-{suffix}"))).unwrap(),
                    bytes
                );
            }
            session.shutdown();
        }
        "selectors" | "http" | "helper" | "lifecycle" => {
            std::fs::write(&path, CELLS).unwrap();
            let session = hostile::begin(&path);
            if target == "lifecycle" {
                let boundary = u32::from(bytes.first().copied().unwrap_or(0) % 3);
                session
                    .scan(|status| {
                        if status.progress.completed >= boundary {
                            ScanControl::Cancel
                        } else {
                            ScanControl::Continue
                        }
                    })
                    .unwrap();
                let status = session.status();
                assert_ne!(
                    status.coverage.unwrap().reason,
                    volmap_sqlite::inspection::CoverageReason::Complete
                );
                assert!(session.advance().is_err());
                let previous = serde_json::to_value(session.status()).unwrap();
                assert!(session.scan(|_| ScanControl::Continue).is_err());
                assert_eq!(serde_json::to_value(session.status()).unwrap(), previous);
            } else {
                session.scan(|_| ScanControl::Continue).unwrap();
                match target {
                    "selectors" => selectors(&session, bytes),
                    "http" => http(&session, bytes).await,
                    "helper" => helper(&session, bytes),
                    _ => unreachable!(),
                }
            }
            if let Ok(graph) = session.graph() {
                let before = serde_json::to_value(&*graph).unwrap();
                let mut replacement = CELLS.to_vec();
                replacement[60] ^= 1;
                std::fs::write(&path, replacement).unwrap();
                assert!(session.graph().is_err());
                assert_eq!(serde_json::to_value(&*graph).unwrap(), before);
                assert_eq!(hostile::get(&session, "/revisions/1").await.0, 409);
            }
            session.shutdown();
        }
        _ => panic!("unknown fuzz target"),
    }
}

async fn check(session: &Arc<volmap_sqlite::inspection::InspectionSession>, directory: &Path) {
    let (_, status) = hostile::get(session, "").await;
    hostile::private(&status, directory);
    if let Ok(graph) = session.graph() {
        let graph = serde_json::to_value(&*graph).unwrap();
        hostile::graph_properties(&graph);
        hostile::private(&graph, directory);
        let (code, web) = hostile::get(session, "/revisions/1").await;
        if code == 200 {
            assert_eq!(web, graph);
        } else {
            assert_eq!(web["state"], "budget_stopped");
        }
    } else {
        assert_ne!(status["coverage"]["reason"], "complete");
    }
    let mut terminal = TerminalFlow::new(Arc::clone(session));
    for (width, height) in [(0, 0), (1, 1), (40, 16), (120, 40)] {
        let frame = terminal.screen(width, height);
        assert!(frame.len() <= usize::from(height));
        assert!(!frame.join("\n").contains(hostile::SECRET));
    }
}

fn selectors(session: &Arc<volmap_sqlite::inspection::InspectionSession>, bytes: &[u8]) {
    // Both serde entity forms and the terminal's page[:cell] decoder.
    let _ = serde_json::from_slice::<EntityIdentity>(bytes);
    let _ = serde_json::from_slice::<DeepBudget>(bytes);
    if let Ok(selector) = serde_json::from_slice::<DeepSelector>(bytes) {
        let job = session.request_deep(selector.clone(), budget());
        assert_ne!(job.wait().state, DeepState::Pending);
        let status = session.status();
        let live = DeepSelector {
            session_id: status.session_id,
            snapshot_id: status.snapshot_id,
            ..selector
        };
        let job = session.request_deep(live, budget());
        assert_ne!(job.wait().state, DeepState::Pending);
    }
    let mut terminal = TerminalFlow::new(Arc::clone(session));
    hostile::go(&mut terminal, &String::from_utf8_lossy(bytes));
    terminal.key(Key::Enter);
    for c in String::from_utf8_lossy(bytes).chars().take(64) {
        terminal.key(Key::Char(c));
    }
    let frame = terminal.screen(120, 40).join("\n");
    assert!(!frame.contains(hostile::SECRET));
    for (first, limit) in [(0, 0), (u32::MAX, u32::MAX), (1, 257)] {
        assert!(session.page_batch(1, first, limit).is_err());
    }
    if let Ok(batch) = session.page_batch(1, u32::MAX, 1) {
        assert!(batch.pages.is_empty());
        assert!(batch.next_page.is_none());
    }
}

fn budget() -> DeepBudget {
    DeepBudget {
        max_payload_bytes: 16 * 1024,
        max_overflow_pages: 16,
        max_values: 64,
        max_decoded_bytes: 16 * 1024,
    }
}

fn deep(session: &Arc<volmap_sqlite::inspection::InspectionSession>, bytes: &[u8]) {
    let Ok(graph) = session.graph() else {
        return;
    };
    let before = serde_json::to_value(&*graph).unwrap();
    let mut completed = 0;
    // All cells of the small corpus fixtures, with a fixed cap for dense mutations.
    for cell in graph.pages.iter().flat_map(|p| &p.detail.cells).take(8) {
        for cancel in [false, true] {
            let status = session.status();
            let selector = DeepSelector {
                session_id: status.session_id,
                snapshot_id: status.snapshot_id,
                revision: status.revision.unwrap(),
                page_number: cell.identity.page_number,
                cell_index: cell.identity.index,
            };
            let job = session.request_deep_observed(selector.clone(), budget(), move |_| {
                if cancel {
                    ScanControl::Cancel
                } else {
                    ScanControl::Continue
                }
            });
            let result = job.wait();
            assert_ne!(result.state, DeepState::Pending);
            if cancel {
                assert_ne!(result.state, DeepState::Completed);
            } else if result.state == DeepState::Completed {
                completed += 1;
            }
            assert_eq!(serde_json::to_value(&*graph).unwrap(), before);
            let mut other = selector.clone();
            other.cell_index = other.cell_index.wrapping_add(1);
            assert!(session.deep_result(&job.id, &other).is_err());
        }
    }
    if bytes == include_bytes!("../tests/corpus/damage/valid-cells.sqlite") {
        assert_eq!(
            completed, 2,
            "the positive record seed must exercise both successful decodes"
        );
    }
    // Broader revision endpoints contain structural evidence, never typed values.
    for revision in session.status().available_revisions {
        let value = serde_json::to_value(&*session.revision(revision).unwrap()).unwrap();
        assert!(value.get("values").is_none());
    }
}

fn helper(session: &Arc<volmap_sqlite::inspection::InspectionSession>, bytes: &[u8]) {
    let graph = session.graph().unwrap();
    let before = serde_json::to_value(&*graph).unwrap();
    if let Some(reply) = volmap_sqlite::semantic::decode_helper_reply(&graph.schema, bytes) {
        assert!(
            !serde_json::to_string(&reply)
                .unwrap()
                .contains(hostile::SECRET)
        );
    }
    assert_eq!(serde_json::to_value(&*graph).unwrap(), before);
    let injected = tempfile::tempdir().unwrap();
    let path = injected.path().join("main.sqlite");
    let count = bytes.len().min(hostile::MAX_INPUT - 512);
    let pages = (count + 512).div_ceil(512);
    let mut image = include_bytes!("../tests/corpus/damage/opaque-page.sqlite").to_vec();
    image.resize(pages * 512, 0);
    image[28..32].copy_from_slice(&u32::try_from(pages).unwrap().to_be_bytes());
    image[60..64].copy_from_slice(&u32::try_from(count).unwrap().to_be_bytes());
    image[512..512 + count].copy_from_slice(&bytes[..count]);
    std::fs::write(&path, &image).unwrap();
    let plain = hostile::begin(&path);
    plain.scan(|_| ScanControl::Continue).unwrap();
    let enriched = hostile::configured(&path).with_semantic_metadata();
    enriched.scan(|_| ScanControl::Continue).unwrap();
    assert_eq!(
        hostile::physical(serde_json::to_value(&*plain.graph().unwrap()).unwrap()),
        hostile::physical(serde_json::to_value(&*enriched.graph().unwrap()).unwrap())
    );
    assert_eq!(std::fs::read(&path).unwrap(), image);
    // Exercise the actual fixed-token stdin decoder and private executable entry point.
    let executable = std::env::var_os("VOLMAP_HELPER_BIN").expect("runner supplies helper binary");
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("snapshot.sqlite"),
        include_bytes!("../tests/corpus/damage/opaque-page.sqlite"),
    )
    .unwrap();
    let mut child = Command::new(executable)
        .arg("--private-semantic-helper")
        .current_dir(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(bytes);
    let output = child.wait_with_output().unwrap();
    assert!(output.stdout.len() <= 256 * 1024);
    assert!(output.stderr.is_empty());
    assert!(!String::from_utf8_lossy(&output.stdout).contains(hostile::SECRET));
}

async fn http(session: &Arc<volmap_sqlite::inspection::InspectionSession>, bytes: &[u8]) {
    let id = session.status().snapshot_id;
    let paths = [
        format!("/api/snapshots/{id}/deep-inspections"),
        format!("/api/snapshots/{id}/deep-inspections/RAW_HIDE/result"),
        format!("/api/snapshots/{id}/deep-inspections/RAW_HIDE/cancel"),
        format!("/api/snapshots/{id}/revisions/1/pages/0/4294967295"),
        format!("/api/snapshots/{id}/revisions/1/collections/traversals/18446744073709551615/256"),
        format!("/api/snapshots/{id}/revisions/1/traversals/18446744073709551615/pages/0/256"),
        format!(
            "/api/snapshots/{id}/revisions/1/pages/2/collections/cells/18446744073709551615/256"
        ),
    ];
    let arbitrary = String::from_utf8_lossy(bytes);
    for path in paths
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(arbitrary.as_ref()))
    {
        let Ok(uri) = path.parse::<Uri>() else {
            continue;
        };
        for method in ["GET", "POST", "HEAD", "PUT", "OPTIONS"] {
            for body in [bytes, b"".as_slice()] {
                let request = Request::builder()
                    .method(method)
                    .uri(uri.clone())
                    .header("host", "localhost")
                    .header("origin", "http://localhost")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_vec()))
                    .unwrap();
                let response = atlas_router(Arc::clone(session))
                    .oneshot(request)
                    .await
                    .unwrap();
                assert!(!response.status().is_server_error() || response.status() == 507);
                let body = to_bytes(response.into_body(), 8 * 1024 * 1024)
                    .await
                    .unwrap();
                assert!(!String::from_utf8_lossy(&body).contains(hostile::SECRET));
            }
        }
    }
    // Malformed origins, duplicate headers and oversized header values are admission inputs.
    if let Ok(value) = axum::http::HeaderValue::from_bytes(bytes) {
        for header in ["host", "origin", "sec-fetch-site", "x-extra"] {
            let request = Request::builder()
                .uri(format!("/api/snapshots/{id}"))
                .header("host", "localhost")
                .header(header, value.clone())
                .body(Body::empty())
                .unwrap();
            let response = atlas_router(Arc::clone(session))
                .oneshot(request)
                .await
                .unwrap();
            let body = to_bytes(response.into_body(), 8 * 1024 * 1024)
                .await
                .unwrap();
            assert!(!String::from_utf8_lossy(&body).contains(hostile::SECRET));
        }
    }
}
