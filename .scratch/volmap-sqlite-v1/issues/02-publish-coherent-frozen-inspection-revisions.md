# 02: Publish coherent frozen inspection revisions

**What to build:** Turn the initial scan into an honest inspection session: operators can observe deterministic fast-inspection progress, navigate only a published immutable revision, and see the snapshot invalidated if any accepted input changes while work is in progress.

**Blocked by:** 01: Inspect a real database in an embedded page atlas

**Status:** done

- [x] The inspection session exposes distinct scanning, published, cancelled or stopped, and invalidated states without presenting a live partial graph as coherent.
- [x] Fast-inspection progress is deterministic and reports completed structural units against trusted totals when totals are available.
- [x] The first navigable graph is published atomically as an immutable inspection revision only when the fast inspection completes or stops at an explicitly recorded boundary.
- [x] The snapshot fingerprint covers the accepted main file and observed adjacent sidecar set before and after inspection work, including replacement, size or metadata change, addition, and removal.
- [x] A detected input change invalidates the database snapshot, preserves already observed facts as diagnostic evidence, and prevents further enrichment.
- [x] Published entities retain stable snapshot-scoped identities and a published revision cannot change after readers obtain it.
- [x] Inspection coverage records evaluated extent, stopping boundary, reason, and known or unknown remainder; it never labels a sampled or interrupted scan complete.
- [x] The browser clearly distinguishes scanning progress, a coherent published graph, explicit partial coverage, cancellation, fatal geometry, and invalidation.
- [x] Controlled synchronization tests mutate and replace inputs during fast inspection without timing-dependent sleeps and verify publication, invalidation, retained evidence, and refusal of later work.


## User acceptance — 2026-09-11

The user tried the Inspector and explicitly accepted v1: “I think it's nice. I accept.”
Previous tracker status: `ready-for-agent`. Implementation commits: `e332ee8, fe2d3be`. Regression evidence: `tests/revisions.rs` and the ticket 16 clean production verification.
Closed as part of the accepted v1 effort; see [the acceptance record](../../../release/ACCEPTANCE.md).
