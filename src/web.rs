//! Same-origin browser projection of inspection-session status and immutable revisions.
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::inspection::{InspectionError, InspectionSession, ScanControl};

const APP_JAVASCRIPT: &str = include_str!("../frontend/dist/assets/app.js");
const APP_STYLESHEET: &str = include_str!("../frontend/dist/assets/app.css");

/// Builds browser and session routes; no graph is exposed before publication.
pub fn atlas_router(session: Arc<InspectionSession>) -> Router {
    Router::new()
        .route("/", get(atlas))
        .route("/api/snapshots/{snapshot_id}", get(status))
        .route(
            "/api/snapshots/{snapshot_id}/revisions/{revision}",
            get(revision),
        )
        .route("/api/snapshots/{snapshot_id}/evidence", get(evidence))
        .route("/api/snapshots/{snapshot_id}/cancel", post(cancel))
        .route("/assets/app.js", get(javascript))
        .route("/assets/app.css", get(stylesheet))
        .layer(axum::middleware::map_response(
            |mut response: Response| async {
                response.headers_mut().insert(
                    header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static("no-store"),
                );
                response
            },
        ))
        .with_state(session)
}

async fn atlas(State(session): State<Arc<InspectionSession>>) -> Html<String> {
    // Only a generated UUID enters this bootstrap; all presentation belongs to React.
    let id = session.status().snapshot_id;
    Html(format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Page atlas · Volmap SQLite Inspector</title><link rel="stylesheet" href="/assets/app.css"></head>
<body><div id="root"></div><noscript>Enable JavaScript to view inspection status and the page atlas.</noscript>
<script>window.__VOLMAP_BOOTSTRAP__={{snapshotId:"{id}"}};</script>
<script type="module" src="/assets/app.js"></script></body></html>"#
    ))
}

async fn status(Path(id): Path<String>, State(session): State<Arc<InspectionSession>>) -> Response {
    let status = session.status();
    if id != status.snapshot_id {
        return StatusCode::NOT_FOUND.into_response();
    }
    Json(status).into_response()
}

async fn revision(
    Path((id, number)): Path<(String, u64)>,
    State(session): State<Arc<InspectionSession>>,
) -> Response {
    if id != session.status().snapshot_id {
        return StatusCode::NOT_FOUND.into_response();
    }
    match session.revision(number) {
        Ok(graph) => Json(&*graph).into_response(),
        Err(InspectionError::Invalidated) => {
            (StatusCode::CONFLICT, Json(session.status())).into_response()
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn evidence(
    Path(id): Path<String>,
    State(session): State<Arc<InspectionSession>>,
) -> Response {
    if id != session.status().snapshot_id {
        return StatusCode::NOT_FOUND.into_response();
    }
    match session.evidence() {
        Some(evidence) => Json(evidence).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn cancel(Path(id): Path<String>, State(session): State<Arc<InspectionSession>>) -> Response {
    if id != session.status().snapshot_id {
        return StatusCode::NOT_FOUND.into_response();
    }
    match session.stop(ScanControl::Cancel) {
        Ok(()) => Json(session.status()).into_response(),
        Err(_) => (StatusCode::CONFLICT, Json(session.status())).into_response(),
    }
}

async fn javascript() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        APP_JAVASCRIPT,
    )
}
async fn stylesheet() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        APP_STYLESHEET,
    )
}
