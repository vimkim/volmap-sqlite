use std::path::Path;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use rusqlite::Connection;
use tempfile::TempDir;
use tower::ServiceExt;
use volmap_sqlite::inspection::{InspectionSession, ScanControl};
use volmap_sqlite::web::atlas_router;

fn write_fixture(path: &Path) {
    let connection = Connection::open(path).expect("create SQLite fixture");
    connection
        .execute_batch(
            "PRAGMA page_size = 4096;
             CREATE TABLE inventory (sku TEXT PRIMARY KEY, quantity INTEGER NOT NULL);
             INSERT INTO inventory VALUES ('fixture-value', 7);",
        )
        .expect("populate SQLite fixture");
}

async fn response_body(response: axum::response::Response) -> String {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read response body");
    String::from_utf8(bytes.to_vec()).expect("UTF-8 response")
}

#[tokio::test]
async fn serves_the_real_graph_and_embedded_atlas_without_disclosing_its_path() {
    let directory = TempDir::new().expect("temporary directory");
    let database_path = directory.path().join("customer-data.sqlite");
    write_fixture(&database_path);
    let session = Arc::new(InspectionSession::open(&database_path).expect("valid database"));
    let snapshot_id = session.status().snapshot_id;
    let app = atlas_router(session);

    let api_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/snapshots/{snapshot_id}/revisions/1"))
                .body(Body::empty())
                .expect("API request"),
        )
        .await
        .expect("API response");
    assert_eq!(api_response.status(), StatusCode::OK);
    assert_eq!(api_response.headers()[header::CACHE_CONTROL], "no-store");
    let api_body = response_body(api_response).await;
    assert!(api_body.contains("\"pageCount\":3"));
    assert!(api_body.contains("\"number\":3"));
    let graph: serde_json::Value = serde_json::from_str(&api_body).unwrap();
    let table = &graph["pages"][1]["detail"];
    assert_eq!(table["kind"], "table_leaf");
    assert_eq!(table["cells"][0]["identity"]["pageNumber"], 2);
    assert_eq!(
        table["cells"][0]["record"]["serialTypes"],
        serde_json::json!(["39", "1"])
    );
    assert_eq!(graph["pages"][2]["detail"]["kind"], "index_leaf");
    assert!(!api_body.contains("fixture-value"));
    assert!(!api_body.contains(directory.path().to_string_lossy().as_ref()));

    let atlas_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/")
                .body(Body::empty())
                .expect("atlas request"),
        )
        .await
        .expect("atlas response");
    assert_eq!(atlas_response.status(), StatusCode::OK);
    let atlas_body = response_body(atlas_response).await;
    assert!(atlas_body.contains("Page atlas"));
    assert!(atlas_body.contains("id=\"root\""));
    assert!(!atlas_body.contains("data-page-number"));
    assert!(atlas_body.contains(&snapshot_id));
    assert!(!atlas_body.contains("fixture-value"));
    assert!(!atlas_body.contains(directory.path().to_string_lossy().as_ref()));

    let asset_response = app
        .oneshot(
            Request::builder()
                .uri("/assets/app.js")
                .body(Body::empty())
                .expect("asset request"),
        )
        .await
        .expect("asset response");
    assert_eq!(asset_response.status(), StatusCode::OK);
    assert_eq!(
        asset_response.headers()[header::CONTENT_TYPE],
        "text/javascript; charset=utf-8"
    );
    assert!(!response_body(asset_response).await.is_empty());
}

#[tokio::test]
async fn rejects_a_snapshot_identifier_from_another_session() {
    let directory = TempDir::new().expect("temporary directory");
    let database_path = directory.path().join("main.sqlite");
    write_fixture(&database_path);
    let session = Arc::new(InspectionSession::open(&database_path).expect("valid database"));
    let app = atlas_router(session);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/snapshots/not-this-snapshot")
                .body(Body::empty())
                .expect("API request"),
        )
        .await
        .expect("API response");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

async fn get(app: axum::Router, uri: &str) -> axum::response::Response {
    app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn status_progress_is_separate_from_revision_and_invalidated_evidence() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("main.sqlite");
    write_fixture(&path);
    let session = Arc::new(InspectionSession::begin(&path).unwrap());
    let base = format!("/api/snapshots/{}", session.status().snapshot_id);
    let app = atlas_router(Arc::clone(&session));
    let status = response_body(get(app.clone(), &base).await).await;
    assert!(status.contains("\"state\":\"scanning\""));
    assert!(!status.contains("\"pages\":"));
    assert_eq!(
        get(app.clone(), &format!("{base}/revisions/1"))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    session.scan(|_| ScanControl::Continue).unwrap();
    let published = get(app.clone(), &format!("{base}/revisions/1")).await;
    assert_eq!(published.status(), StatusCode::OK);
    let published_body = response_body(published).await;
    assert!(published_body.contains("\"revision\":1"));
    assert_eq!(
        get(app.clone(), &format!("{base}/revisions/2"))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    std::fs::write(dir.path().join("main.sqlite-wal"), [0; 32]).unwrap();
    let rejected = get(app.clone(), &format!("{base}/revisions/1")).await;
    assert_eq!(rejected.status(), StatusCode::CONFLICT);
    assert_eq!(rejected.headers()[header::CACHE_CONTROL], "no-store");
    let body = response_body(rejected).await;
    assert!(body.contains("input_changed"));
    assert!(!body.contains("\"pages\":"));
    let evidence = response_body(get(app, &format!("{base}/evidence")).await).await;
    assert!(evidence.contains("\"number\":3"));
    assert!(!evidence.contains("\"revision\":"));
    for body in [&status, &published_body, &body, &evidence] {
        assert!(!body.contains(dir.path().to_str().unwrap()));
        assert!(!body.contains("fixture-value"));
    }
}

#[tokio::test]
async fn cancellation_publishes_an_explicit_partial_revision() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("main.sqlite");
    write_fixture(&path);
    let session = Arc::new(InspectionSession::begin(&path).unwrap());
    session.advance().unwrap();
    session.advance().unwrap();
    let base = format!("/api/snapshots/{}", session.status().snapshot_id);
    let app = atlas_router(Arc::clone(&session));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{base}/cancel"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response_body(response)
            .await
            .contains("\"state\":\"cancelled\"")
    );
    let graph = response_body(get(app, &format!("{base}/revisions/1")).await).await;
    assert!(graph.contains("\"reason\":\"cancelled\""));
    assert!(graph.contains("\"nextPage\":2"));
    assert!(!graph.contains("\"number\":2"));
}
