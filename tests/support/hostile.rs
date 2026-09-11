//! Shared black-box properties for the damage corpus and bounded mutation campaign.
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc};
use tower::ServiceExt;
use volmap_sqlite::{
    inspection::{
        InspectionSession, OperationalBudget, SchemaBudget, SidecarBudget, TraversalBudget,
    },
    terminal::{Key, TerminalFlow},
    web::atlas_router,
};

pub const MAX_INPUT: usize = 32 * 1024;
pub const SECRET: &str = "RAW_HIDE";

pub fn begin(path: &Path) -> Arc<InspectionSession> {
    Arc::new(configured(path))
}

pub fn configured(path: &Path) -> InspectionSession {
    InspectionSession::begin_with_schema_budget(
        path,
        TraversalBudget::with_total_pages(64, 64, 4096),
        SidecarBudget { max_wal_frames: 16 },
        SchemaBudget {
            max_decoded_bytes: 16 * 1024,
        },
    )
    .unwrap()
    .with_operational_budget(OperationalBudget {
        max_processed_cells: 4096,
        max_phase_units: 16_384,
        max_resident_bytes: 256 * 1024 * 1024,
        max_freelist_trunks: 64,
    })
}

pub async fn get(session: &Arc<InspectionSession>, suffix: &str) -> (StatusCode, Value) {
    let response = atlas_router(Arc::clone(session))
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/snapshots/{}{suffix}",
                    session.status().snapshot_id
                ))
                .header("host", "localhost")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

pub fn private(value: &Value, directory: &Path) {
    let text = value.to_string();
    assert!(!text.contains(SECRET), "raw payload leaked");
    assert!(
        !text.contains(directory.to_str().unwrap()),
        "source path leaked"
    );
}

/// Object subsets ignore new fields; arrays retain exact ordered-prefix semantics.
pub fn matches(actual: &Value, expected: &Value) -> bool {
    match expected {
        Value::Object(fields) => fields
            .iter()
            .all(|(key, value)| actual.get(key).is_some_and(|a| matches(a, value))),
        Value::Array(items) => actual.as_array().is_some_and(|a| {
            a.len() == items.len() && a.iter().zip(items).all(|(a, b)| matches(a, b))
        }),
        _ => actual == expected,
    }
}

pub fn expectations(document: &Value, checks: &Value, name: &str) {
    for check in checks.as_array().unwrap() {
        let path = check["path"].as_str().unwrap();
        let actual = document
            .pointer(path)
            .unwrap_or_else(|| panic!("{name}: missing {path}"));
        if let Some(expected) = check.get("equals") {
            assert_eq!(actual, expected, "{name}: {path}");
        } else {
            let expected = &check["contains"];
            assert!(
                actual
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|a| matches(a, expected)),
                "{name}: {path} lacks {expected}; observed {actual}"
            );
        }
    }
}

pub fn graph_properties(graph: &Value) {
    let coverage = &graph["coverage"];
    let evaluated = coverage["evaluated"].as_u64().unwrap();
    let total = coverage["total"].as_u64().unwrap();
    assert!(evaluated <= total);
    assert_eq!(coverage["remainder"], total - evaluated);
    if coverage["reason"] == "complete" {
        assert_eq!(evaluated, total);
    }
    let size = graph["snapshot"]["geometry"]["pageSize"].as_u64().unwrap();
    for page in graph["pages"].as_array().unwrap() {
        let number = page["number"].as_u64().unwrap();
        assert!((1..=evaluated).contains(&number));
        for region in page["detail"]["regions"].as_array().unwrap() {
            let range = &region["range"];
            let offset = range["pageOffset"].as_u64().unwrap();
            let length = range["length"].as_u64().unwrap();
            assert!(offset.checked_add(length).is_some_and(|end| end <= size));
            assert_eq!(range["fileOffset"], (number - 1) * size + offset);
        }
    }
    for traversal in graph["traversals"].as_array().unwrap() {
        let mut seen = std::collections::BTreeSet::new();
        for page in traversal["validatedPrefix"].as_array().unwrap() {
            let n = page["pageNumber"].as_u64().unwrap();
            assert!((1..=evaluated).contains(&n));
            assert!(seen.insert(n), "cycle in validated prefix");
        }
    }
    for work in graph["workCoverage"].as_array().unwrap() {
        if let (Some(e), Some(t)) = (work["evaluated"].as_u64(), work["total"].as_u64()) {
            assert!(e <= t);
            assert_eq!(work["remainder"], t - e);
            if work["reason"] == "complete" {
                assert_eq!(e, t);
            }
        }
    }
    assert!(graph["deepInspections"].as_array().unwrap().is_empty());
}

pub fn go(terminal: &mut TerminalFlow, target: &str) {
    terminal.key(Key::Char('g'));
    for c in target.chars() {
        terminal.key(Key::Char(c));
    }
    terminal.key(Key::Enter);
}

/// Exercise adapter projections and compare their evidence with the session contract.
pub async fn adapters(session: &Arc<InspectionSession>, directory: &Path) -> Value {
    let status = serde_json::to_value(session.status()).unwrap();
    let (code, web) = get(session, "").await;
    assert_eq!(code, StatusCode::OK);
    assert!(matches(&web, &status));
    private(&web, directory);
    let mut terminal = TerminalFlow::new(Arc::clone(session));
    let frame = terminal.screen(240, 1000).join("\n");
    assert!(!frame.contains(SECRET));
    assert!(!frame.contains(directory.to_str().unwrap()));
    if let Ok(graph) = session.graph() {
        let mut document = serde_json::to_value(&*graph).unwrap();
        private(&document, directory);
        graph_properties(&document);
        let (code, web) = get(session, "/revisions/1").await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(web, document);
        // Navigate every retained page and cell, not only the entry frame.
        for page in &graph.pages {
            go(&mut terminal, &page.number.to_string());
            let frame = terminal.screen(240, 1000).join("\n");
            assert!(frame.contains(&format!("Page {} |", page.number)));
            assert!(frame.contains(&format!("coverage {:?}", page.detail.coverage)));
            for code in &page.detail.diagnostics {
                assert!(frame.contains(code.as_ref()));
            }
            for region in &page.detail.regions {
                assert!(frame.contains(region.kind.as_ref()));
            }
            for cell in &page.detail.cells {
                go(
                    &mut terminal,
                    &format!("{}:{}", page.number, cell.identity.index),
                );
                let frame = terminal.screen(240, 1000).join("\n");
                if let Some(code) = &cell.diagnostic {
                    assert!(frame.contains(code.as_ref()));
                }
                assert!(!frame.contains(SECRET));
            }
        }
        terminal.key(Key::Char('!'));
        let frame = terminal.screen(240, 1000).join("\n");
        for finding in &graph.diagnostics {
            assert!(frame.contains(finding.code.as_ref()));
        }
        document["status"] = status;
        document
    } else {
        assert_eq!(status["revision"], Value::Null);
        assert_eq!(status["coverage"]["total"], Value::Null);
        assert_ne!(get(session, "/revisions/1").await.0, StatusCode::OK);
        assert!(frame.contains(status["diagnostic"]["code"].as_str().unwrap()));
        json!({"status":status})
    }
}

pub fn physical(mut graph: Value) -> Value {
    graph.as_object_mut().unwrap().remove("sidecars");
    graph.as_object_mut().unwrap().remove("semanticMetadata");
    graph.as_object_mut().unwrap().remove("workCoverage");
    graph.as_object_mut().unwrap().remove("status");
    graph["snapshot"]["id"] = Value::Null;
    graph["snapshot"]["source"]["id"] = Value::Null;
    graph
}
