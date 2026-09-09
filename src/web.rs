//! Same-origin browser projection of inspection-session status and immutable revisions.
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::inspection::{
    DeepBudget, DeepSelector, DeepState, InspectionError, InspectionSession, ScanControl,
};

mod listener;
mod security;
pub use listener::bounded_listener;
pub use security::WebLimits;

#[derive(Clone)]
struct WebState {
    session: Arc<InspectionSession>,
    limits: WebLimits,
}

const APP_JAVASCRIPT: &str = include_str!("../frontend/dist/assets/app.js");
const APP_STYLESHEET: &str = include_str!("../frontend/dist/assets/app.css");

/// Builds routes for `127.0.0.1:80`; no graph is exposed before publication.
/// Use [`atlas_router_for_listener`] with the actual address for other listeners.
pub fn atlas_router(session: Arc<InspectionSession>) -> Router {
    atlas_router_for_listener(
        session,
        std::net::SocketAddr::from(([127, 0, 0, 1], 80)),
        WebLimits::default(),
    )
}

/// Binds HTTP origin checks to the actual listener and enforces operator-lowered ceilings.
pub fn atlas_router_for_listener(
    session: Arc<InspectionSession>,
    address: std::net::SocketAddr,
    limits: WebLimits,
) -> Router {
    let limits = limits.capped();
    let admission = Arc::new(security::Admission {
        address,
        response_bytes: limits.response_bytes,
        requests: Arc::new(tokio::sync::Semaphore::new(limits.concurrent_requests)),
    });
    Router::new()
        .route("/", get(entry))
        .route("/sessions/{session_id}", get(atlas))
        .route("/sessions/{session_id}/bootstrap.js", get(bootstrap))
        .route("/api/snapshots/{snapshot_id}", get(status))
        .route(
            "/api/snapshots/{snapshot_id}/revisions/{revision}",
            get(revision),
        )
        .route("/api/snapshots/{snapshot_id}/evidence", get(evidence))
        .route("/api/snapshots/{snapshot_id}/cancel", post(cancel))
        .route(
            "/api/snapshots/{snapshot_id}/deep-inspections",
            post(start_deep),
        )
        .route(
            "/api/snapshots/{snapshot_id}/deep-inspections/{job_id}",
            get(deep_status),
        )
        .route(
            "/api/snapshots/{snapshot_id}/deep-inspections/{job_id}/cancel",
            post(cancel_deep),
        )
        .route(
            "/api/snapshots/{snapshot_id}/deep-inspections/{job_id}/result",
            post(deep_result),
        )
        .route("/assets/app.js", get(javascript))
        .route("/assets/app.css", get(stylesheet))
        .layer(axum::middleware::from_fn_with_state(
            admission,
            security::protect,
        ))
        .with_state(WebState { session, limits })
}

async fn entry(State(state): State<WebState>) -> Redirect {
    Redirect::temporary(&format!("/sessions/{}", state.session.status().session_id))
}

async fn atlas(Path(id): Path<String>, State(state): State<WebState>) -> Response {
    let status = state.session.status();
    if id != status.session_id {
        return StatusCode::NOT_FOUND.into_response();
    }
    let snapshot = status.snapshot_id;
    Html(format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="inspection-snapshot" content="{snapshot}">
<title>Page atlas · Volmap SQLite Inspector</title><link rel="stylesheet" href="/assets/app.css"></head>
<body><div id="root"></div><noscript>Enable JavaScript to view inspection status and the page atlas.</noscript>
<script src="/sessions/{id}/bootstrap.js"></script>
<script type="module" src="/assets/app.js"></script></body></html>"#
    )).into_response()
}

async fn bootstrap(Path(id): Path<String>, State(state): State<WebState>) -> Response {
    let status = state.session.status();
    if id != status.session_id {
        return StatusCode::NOT_FOUND.into_response();
    }
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        format!(
            "window.__VOLMAP_BOOTSTRAP__={{snapshotId:\"{}\"}};",
            status.snapshot_id
        ),
    )
        .into_response()
}

async fn status(Path(id): Path<String>, State(state): State<WebState>) -> Response {
    let WebState { session, limits } = state;
    let status = session.status();
    if id != status.snapshot_id {
        return StatusCode::NOT_FOUND.into_response();
    }
    security::json(&status, limits)
}

async fn revision(
    Path((id, number)): Path<(String, u64)>,
    State(state): State<WebState>,
) -> Response {
    let WebState { session, limits } = state;
    if id != session.status().snapshot_id {
        return StatusCode::NOT_FOUND.into_response();
    }
    let worker_session = Arc::clone(&session);
    match tokio::task::spawn_blocking(move || worker_session.revision(number)).await {
        Ok(Ok(graph)) => security::json(&*graph, limits),
        Ok(Err(InspectionError::Invalidated)) => {
            security::json_status(StatusCode::CONFLICT, &session.status(), limits)
        }
        Ok(Err(_)) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn evidence(Path(id): Path<String>, State(state): State<WebState>) -> Response {
    let WebState { session, limits } = state;
    if id != session.status().snapshot_id {
        return StatusCode::NOT_FOUND.into_response();
    }
    match session.evidence() {
        Some(evidence) => security::json(&evidence, limits),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn cancel(Path(id): Path<String>, State(state): State<WebState>) -> Response {
    let WebState { session, limits } = state;
    if id != session.status().snapshot_id {
        return StatusCode::NOT_FOUND.into_response();
    }
    let worker_session = Arc::clone(&session);
    match tokio::task::spawn_blocking(move || worker_session.stop(ScanControl::Cancel)).await {
        Ok(Ok(())) => security::json(&session.status(), limits),
        Ok(Err(_)) => security::json_status(StatusCode::CONFLICT, &session.status(), limits),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
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

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeepRequest {
    target: DeepSelector,
    #[serde(default)]
    budget: DeepBudget,
}

async fn start_deep(
    Path(id): Path<String>,
    State(state): State<WebState>,
    Json(request): Json<DeepRequest>,
) -> Response {
    let WebState { session, limits } = state;
    if id != session.status().snapshot_id {
        return StatusCode::NOT_FOUND.into_response();
    }
    if !valid_target(&request.target, &session) {
        return security::error(StatusCode::NOT_FOUND, "selector_rejected");
    }
    match tokio::task::spawn_blocking(move || {
        let status = session
            .request_deep(request.target, request.budget)
            .status();
        match status.state {
            DeepState::InvalidTarget => security::error(StatusCode::NOT_FOUND, "selector_rejected"),
            DeepState::StaleRevision => security::error(StatusCode::CONFLICT, "stale_revision"),
            _ => security::json_status(StatusCode::ACCEPTED, &status, limits),
        }
    })
    .await
    {
        Ok(response) => response,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn deep_status(
    Path((id, job)): Path<(String, String)>,
    State(state): State<WebState>,
) -> Response {
    let WebState { session, limits } = state;
    if id != session.status().snapshot_id {
        return StatusCode::NOT_FOUND.into_response();
    }
    session.deep_status(&job).map_or_else(
        || StatusCode::NOT_FOUND.into_response(),
        |status| security::json(&status, limits),
    )
}

async fn cancel_deep(
    Path((id, job)): Path<(String, String)>,
    State(state): State<WebState>,
) -> Response {
    let WebState { session, limits } = state;
    if id != session.status().snapshot_id {
        return StatusCode::NOT_FOUND.into_response();
    }
    session.cancel_deep(&job).map_or_else(
        || StatusCode::NOT_FOUND.into_response(),
        |status| security::json(&status, limits),
    )
}

async fn deep_result(
    Path((id, job)): Path<(String, String)>,
    State(state): State<WebState>,
    Json(target): Json<DeepSelector>,
) -> Response {
    let WebState { session, limits } = state;
    if id != session.status().snapshot_id {
        return StatusCode::NOT_FOUND.into_response();
    }
    if !valid_target(&target, &session) {
        return security::error(StatusCode::NOT_FOUND, "selector_rejected");
    }
    match tokio::task::spawn_blocking(move || session.deep_result(&job, &target)).await {
        Ok(Ok(result)) => security::json(&result, limits),
        Ok(Err(state)) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"state": state})),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"state": DeepState::Failed})),
        )
            .into_response(),
    }
}

fn valid_target(target: &DeepSelector, session: &InspectionSession) -> bool {
    let status = session.status();
    target.session_id == status.session_id
        && target.snapshot_id == status.snapshot_id
        && target.revision > 0
        && target.page_number > 0
}
