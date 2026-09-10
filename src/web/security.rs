//! HTTP admission and bounded serialization, independent of storage interpretation.
use std::io::{self, Write};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, HttpBody, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::ser::{CompactFormatter, Formatter};
use tokio::sync::Semaphore;

/// Hard ceilings for a web projection. Operators may lower them at startup.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebLimits {
    pub request_bytes: usize,
    pub response_bytes: usize,
    pub collection_items: usize,
    pub concurrent_requests: usize,
}

impl Default for WebLimits {
    fn default() -> Self {
        Self {
            request_bytes: BODY_BYTES,
            response_bytes: 8 * 1024 * 1024,
            collection_items: 100_000,
            concurrent_requests: 8,
        }
    }
}

impl WebLimits {
    pub(super) fn capped(self) -> Self {
        let ceiling = Self::default();
        Self {
            request_bytes: self.request_bytes.min(BODY_BYTES),
            response_bytes: self.response_bytes.clamp(1024, ceiling.response_bytes),
            collection_items: self.collection_items.clamp(1, ceiling.collection_items),
            concurrent_requests: self
                .concurrent_requests
                .clamp(1, ceiling.concurrent_requests),
        }
    }
}

pub(super) struct Admission {
    pub address: SocketAddr,
    pub request_bytes: usize,
    pub response_bytes: usize,
    pub requests: Arc<Semaphore>,
}

const BODY_BYTES: usize = 4096;
const URI_BYTES: usize = 512;
const HEADER_BYTES: usize = 8192;

pub(super) async fn protect(
    State(state): State<Arc<Admission>>,
    request: Request,
    next: Next,
) -> Response {
    let limit = state.response_bytes;
    let mut response = admit(state, request, next).await;
    if response
        .body()
        .size_hint()
        .upper()
        .is_some_and(|length| length > limit as u64)
    {
        response = budget_at(
            StatusCode::INSUFFICIENT_STORAGE,
            "response_byte_budget",
            "response_admission",
            0,
            response.body().size_hint().exact(),
        );
    }
    for (name, value) in [
        ("cache-control", "no-store"),
        ("pragma", "no-cache"),
        (
            "content-security-policy",
            "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; font-src 'none'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'",
        ),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("x-frame-options", "DENY"),
        ("cross-origin-resource-policy", "same-origin"),
        ("cross-origin-opener-policy", "same-origin"),
        (
            "permissions-policy",
            "camera=(), microphone=(), geolocation=(), payment=(), usb=()",
        ),
    ] {
        response
            .headers_mut()
            .insert(name, HeaderValue::from_static(value));
    }
    response
}

async fn admit(state: Arc<Admission>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    if request.uri().to_string().len() > URI_BYTES {
        return budget(StatusCode::URI_TOO_LONG, "request_uri_budget");
    }
    if headers
        .iter()
        .map(|(name, value)| name.as_str().len() + value.len())
        .sum::<usize>()
        > HEADER_BYTES
    {
        return budget(
            StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE,
            "request_header_budget",
        );
    }
    // There are no query-based APIs. Reject rather than ignore unrecognized selectors or limits.
    if request.uri().scheme().is_some() || request.uri().authority().is_some() {
        return error(StatusCode::BAD_REQUEST, "absolute_uri_rejected");
    }
    if request.uri().query().is_some() {
        return error(StatusCode::BAD_REQUEST, "query_not_supported");
    }
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
    else {
        return error(StatusCode::FORBIDDEN, "origin_rejected");
    };
    if headers.get_all(header::HOST).iter().count() != 1 || !valid_host(host, state.address) {
        return error(StatusCode::FORBIDDEN, "origin_rejected");
    }
    let origin = headers.get(header::ORIGIN);
    if headers.get_all(header::ORIGIN).iter().count() > 1
        || origin.is_some_and(|origin| !same_origin(origin, host))
        || (request.method() != "GET" && request.method() != "HEAD" && origin.is_none())
        || headers
            .get("sec-fetch-site")
            .is_some_and(|site| site != "same-origin" && site != "none")
    {
        return error(StatusCode::FORBIDDEN, "origin_rejected");
    }
    let Ok(permit) = Arc::clone(&state.requests).try_acquire_owned() else {
        return budget(StatusCode::TOO_MANY_REQUESTS, "concurrent_request_budget");
    };
    // The task owns the slot through handler completion, even if its client disconnects.
    tokio::spawn(async move {
        let _permit = permit;
        let (parts, body) = request.into_parts();
        let body_total = body.size_hint().exact();
        let bytes =
            match tokio::time::timeout(Duration::from_secs(5), to_bytes(body, state.request_bytes))
                .await
            {
                Ok(Ok(bytes)) => bytes,
                Ok(Err(_)) => {
                    return budget_at(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "request_body_budget",
                        "request_admission",
                        0,
                        body_total,
                    );
                }
                Err(_) => return budget(StatusCode::REQUEST_TIMEOUT, "request_body_timeout"),
            };
        if parts.method != "POST" && !bytes.is_empty() {
            return error(StatusCode::BAD_REQUEST, "unexpected_body");
        }
        let mut response = next
            .run(Request::from_parts(parts, Body::from(bytes)))
            .await;
        // Extractor errors must never echo rejected selectors or user-supplied JSON.
        if response.status().is_client_error()
            && response
                .headers()
                .get(header::CONTENT_TYPE)
                .is_none_or(|v| v != "application/json")
        {
            response = error(response.status(), "request_rejected");
        }
        response
    })
    .await
    .unwrap_or_else(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "request_failed"))
}

fn same_origin(origin: &HeaderValue, host: &str) -> bool {
    let Some(origin) = origin
        .to_str()
        .ok()
        .and_then(|value| value.strip_prefix("http://"))
    else {
        return false;
    };
    let Ok(origin) = origin.parse::<axum::http::uri::Authority>() else {
        return false;
    };
    let Ok(host) = host.parse::<axum::http::uri::Authority>() else {
        return false;
    };
    origin.host() == host.host()
        && origin.port_u16().unwrap_or(80) == host.port_u16().unwrap_or(80)
        && !origin.as_str().contains('@')
}

fn valid_host(host: &str, listen: SocketAddr) -> bool {
    let Ok(authority) = host.parse::<axum::http::uri::Authority>() else {
        return false;
    };
    if authority.as_str().contains('@') || authority.port_u16().unwrap_or(80) != listen.port() {
        return false;
    }
    let hostname = authority.host();
    if hostname == "localhost" {
        return listen.ip().is_loopback() || listen.ip().is_unspecified();
    }
    let Ok(ip) = hostname
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<std::net::IpAddr>()
    else {
        return false;
    };
    !ip.is_unspecified()
        && (ip == listen.ip()
            || (listen.ip().is_unspecified() && ip.is_ipv4() == listen.ip().is_ipv4()))
}

pub(super) fn error(status: StatusCode, reason: &'static str) -> Response {
    (
        status,
        axum::Json(serde_json::json!({"state": "rejected", "reason": reason})),
    )
        .into_response()
}

pub(super) fn budget(status: StatusCode, reason: &'static str) -> Response {
    budget_at(
        status,
        reason,
        if reason.starts_with("response_") {
            "response_admission"
        } else {
            "request_admission"
        },
        0,
        None,
    )
}

fn budget_at(
    status: StatusCode,
    reason: &'static str,
    scope: &'static str,
    evaluated: u64,
    total: Option<u64>,
) -> Response {
    (status, axum::Json(serde_json::json!({
        "state": "budget_stopped", "reason": reason,
        "coverage": { "scope": scope, "evaluated": evaluated, "total": total,
            "remainder": total.map(|n| n.saturating_sub(evaluated)), "stoppingBoundary": reason }
    }))).into_response()
}

struct Output {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(io::Error::other("response_byte_budget"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Collections {
    counts: Vec<usize>,
    limit: usize,
    exceeded: bool,
}
impl Formatter for Collections {
    fn begin_array<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.counts.push(0);
        CompactFormatter.begin_array(writer)
    }
    fn end_array<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.counts.pop();
        CompactFormatter.end_array(writer)
    }
    fn begin_array_value<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        if let Some(count) = self.counts.last_mut() {
            if *count >= self.limit {
                self.exceeded = true;
                return Err(io::Error::other("response_collection_budget"));
            }
            *count += 1;
        }
        CompactFormatter.begin_array_value(writer, first)
    }
}

/// Stops serialization before emitting any partial evidence or allocating an oversized body.
pub(super) fn json_status(
    status: StatusCode,
    value: &impl Serialize,
    limits: WebLimits,
) -> Response {
    let mut response = json(value, limits);
    if response.status() == StatusCode::OK {
        *response.status_mut() = status;
    }
    response
}

pub(super) fn json(value: &impl Serialize, limits: WebLimits) -> Response {
    let mut output = Output {
        bytes: Vec::new(),
        limit: limits.response_bytes,
        exceeded: false,
    };
    let formatter = Collections {
        counts: Vec::new(),
        limit: limits.collection_items,
        exceeded: false,
    };
    let mut serializer = serde_json::Serializer::with_formatter(&mut output, formatter);
    let result = value.serialize(&mut serializer);
    if result.is_err() {
        return budget_at(
            StatusCode::INSUFFICIENT_STORAGE,
            if output.exceeded {
                "response_byte_budget"
            } else {
                "response_collection_budget"
            },
            "response_serialization",
            output.bytes.len() as u64,
            None,
        );
    }
    ([(header::CONTENT_TYPE, "application/json")], output.bytes).into_response()
}
