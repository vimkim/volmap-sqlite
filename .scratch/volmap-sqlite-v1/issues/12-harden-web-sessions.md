# 12: Harden local and remote web sessions

**What to build:** Make browser inspection safe by default and explicit when remotely exposed, with session-scoped navigation, bounded same-origin APIs, path privacy, and defenses against accidental caching or value disclosure.

**Blocked by:** 07: Disclose sidecars without applying them; 09: Deep-inspect one selected cell into a new revision

**Status:** done

- [x] The server binds only to loopback by default, and a non-loopback address is accepted only through an explicit operator option.
- [x] Every non-loopback startup emits a prominent warning that the service has no built-in authentication or TLS and requires operator-controlled network protection.
- [x] Browser assets and APIs remain same-origin and require no external runtime, CDN, analytics, telemetry, font, or network dependency.
- [x] Inspection responses use strict security headers and non-cacheable semantics appropriate for path-private physical evidence and selected values.
- [x] Source, snapshot, revision, and entity selectors are syntactically bounded and scoped to the active web inspection session; cross-session, cross-snapshot, forged, and stale selectors are rejected without information leakage.
- [x] Request bodies, query components, collection limits, concurrent work, and response sizes have enforced bounds with explicit non-corruption outcomes.
- [x] Absolute source and sidecar paths never appear in responses, rendered pages, startup URLs, diagnostic messages, error bodies, or access logs.
- [x] Deep-inspection values can cross only the operator's chosen connection in the selected-cell response and are never cached, embedded into URLs, or copied into broad endpoints.
- [x] Network-level tests cover default and explicit listeners, warning behavior, headers, caching, origin policy, malformed and foreign selectors, response bounds, path leakage, value leakage, and absence of outbound connections.



## Implementation

Implemented against baseline `30b76b3481e9ec04ccf00e0a70da5109db279d3c`.
The existing HTTP/CLI seams from this spec were used for behavioral tests.

- Session-specific entry/bootstrap routes expire with the process. Fresh snapshot
  identities scope the API; exact cell selectors and current-revision admission
  retain the shared inspection-session disclosure rules.
- Listener-bound Host and Origin validation rejects foreign origins, DNS rebinding,
  unsupported queries, and echoed extractor errors. Loopback remains the default;
  explicit remote binding prints the authentication/TLS/network-protection warning.
- Strict response headers and external bootstrap delivery keep the embedded viewer
  same-origin and non-cacheable. No access logger or outbound client is installed.
- Request admission bounds URI, header, body, body time, and concurrent handlers.
  A transport wrapper bounds accepted connections and their lifetimes. Streaming
  JSON serialization stops on byte or collection ceilings before returning any
  partial evidence or values. HTTP budget outcomes do not alter inspection coverage.
- The UI labels response-limit refusal without rendering a partial mosaic.
  README documents effective ceilings, numeric-host/proxy behavior, lifecycle,
  historical-revision semantics, and oversized-result handling.

## Validation

The production-process network harness covers loopback/default and wildcard
listeners, warning text, headers, origin/Host rejection, malformed/foreign/stale
selectors, unsupported queries, input/output/collection/concurrency bounds, stalled
body recovery, accepted-connection limits, path/log privacy, and selected-value
isolation. A selected result exceeding its response limit emits no value fragments
and preserves the completed immutable revision. Startup failures do not echo paths.
Mandatory `strace` coverage records no outbound `connect` calls. Chromium was also
run using `VOLMAP_TEST_CHROMIUM`: the real embedded atlas boots under CSP and its
network log contains only requests on the chosen origin.

The full `cargo test --all-targets --locked` suite passed with the Chromium
setting enabled, including sparse gigabyte fixtures, terminal PTY lifecycle,
semantic-helper containment, and the production HTTP harness. Frontend build,
typecheck and all 24 frontend tests passed. Formatting and
`cargo clippy --all-targets -- -D warnings` passed.

## Standards

Independent Standards review: zero documented-standard or actionable smell findings.

## Spec

Independent Spec review: zero missing requirements, scope-creep, or implementation
findings. Historical revision reads follow ADR-0012; stale new deep requests are rejected.

Review totals: Standards 0; Spec 0.


## User acceptance — 2026-09-11

The user tried the Inspector and explicitly accepted v1: “I think it's nice. I accept.”
Previous tracker status: `ready-for-human`. The existing implementation and validation record is retained above.
Closed as part of the accepted v1 effort; see [the acceptance record](../../../release/ACCEPTANCE.md).
