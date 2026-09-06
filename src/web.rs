//! Same-origin browser adapter for the inspection graph.

use std::fmt::Write as _;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, Response, StatusCode, header};
use axum::response::{Html, IntoResponse};
use axum::routing::get;

use crate::inspection::InspectionSession;

const APP_JAVASCRIPT: &str = include_str!("../frontend/dist/assets/app.js");
const APP_STYLESHEET: &str = include_str!("../frontend/dist/assets/app.css");

#[derive(Clone)]
struct AtlasState {
    session: Arc<InspectionSession>,
}

/// Builds the same-origin page-atlas and inspection-graph routes for one session.
pub fn atlas_router(session: Arc<InspectionSession>) -> Router {
    Router::new()
        .route("/", get(atlas))
        .route("/api/snapshots/{snapshot_id}", get(snapshot))
        .route("/assets/app.js", get(javascript))
        .route("/assets/app.css", get(stylesheet))
        .with_state(AtlasState { session })
}

async fn atlas(State(state): State<AtlasState>) -> impl IntoResponse {
    let graph = state.session.graph();
    let snapshot = &graph.snapshot;
    let geometry = &snapshot.geometry;
    let display_name = escape_html(&snapshot.source.display_name);
    let mut page_tiles = String::new();
    for page in &graph.pages {
        write!(
            page_tiles,
            "<button class=\"page{}\" data-page-number=\"{}\" type=\"button\"><span>Page</span><strong>{}</strong></button>",
            if page.number == 1 { " selected" } else { "" },
            page.number,
            page.number
        )
        .expect("writing to a string cannot fail");
    }

    let html = format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="color-scheme" content="dark">
  <title>Page atlas · Volmap SQLite Inspector</title>
  <link rel="stylesheet" href="/assets/app.css">
</head>
<body>
  <div id="root">
    <main class="workspace">
      <header class="topbar">
        <div><p class="eyebrow">Volmap SQLite Inspector</p><h1>Page atlas</h1></div>
        <div class="source-badge"><span class="status-dot"></span><span>{display_name}</span></div>
      </header>
      <section class="geometry" aria-label="Snapshot geometry">
        <div><span>Pages</span><strong>{page_count}</strong></div>
        <div><span>Page size</span><strong>{page_size} B</strong></div>
        <div><span>Usable size</span><strong>{usable_size} B</strong></div>
        <div><span>Reserved</span><strong>{reserved_bytes} B</strong></div>
        <div><span>Encoding</span><strong>{encoding:?}</strong></div>
      </section>
      <div class="content-grid">
        <section class="atlas-panel">
          <div class="section-heading"><div><p class="eyebrow">Physical projection</p><h2>Complete main-file mosaic</h2></div><p>{page_count} complete pages</p></div>
          <div class="mosaic">{page_tiles}</div>
        </section>
        <aside class="evidence-panel"><p class="eyebrow">Selection-linked evidence</p><h2>Page 1</h2><p class="evidence-note">Geometry and physical page identities are ready.</p></aside>
      </div>
    </main>
  </div>
  <script>window.__VOLMAP_BOOTSTRAP__={{snapshotId:"{snapshot_id}"}};</script>
  <script type="module" src="/assets/app.js"></script>
</body>
</html>"#,
        page_count = geometry.page_count,
        page_size = geometry.page_size,
        usable_size = geometry.usable_size,
        reserved_bytes = geometry.reserved_bytes,
        encoding = geometry.text_encoding,
        snapshot_id = snapshot.id,
    );

    (
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        Html(html),
    )
}

async fn snapshot(
    Path(snapshot_id): Path<String>,
    State(state): State<AtlasState>,
) -> Response<Body> {
    let graph = state.session.graph();
    if snapshot_id != graph.snapshot.id {
        return StatusCode::NOT_FOUND.into_response();
    }

    match serde_json::to_vec(graph) {
        Ok(body) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CACHE_CONTROL, "no-store")
            .body(Body::from(body))
            .expect("static response headers are valid"),
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

fn escape_html(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for character in input.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}
