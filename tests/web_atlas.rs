use std::path::Path;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use rusqlite::Connection;
use tempfile::TempDir;
use tower::ServiceExt;
use volmap_sqlite::inspection::InspectionSession;
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
    let snapshot_id = session.graph().snapshot.id.clone();
    let app = atlas_router(session);

    let api_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/snapshots/{snapshot_id}"))
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
    assert!(atlas_body.contains("customer-data.sqlite"));
    assert!(atlas_body.contains("data-page-number=\"1\""));
    assert!(atlas_body.contains("data-page-number=\"3\""));
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
