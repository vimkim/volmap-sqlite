# 09: Deep-inspect one selected cell into a new revision

**What to build:** Allow an operator to explicitly select one physical cell for bounded payload reconstruction and typed value decoding, publishing the result as a later immutable inspection revision without leaking any other application data.

**Blocked by:** 04: Navigate B-tree topology and overflow claims; 08: Follow schema attribution in schema flow

**Status:** done

- [x] A deep-inspection request contains a session-, snapshot-, revision-, and cell-scoped selector that must resolve to exactly one existing physical cell.
- [x] The request runs asynchronously and exposes pending, completed, cancelled, budget-stopped, invalid-target, stale-revision, and invalidated-snapshot outcomes.
- [x] Only the selected cell's validated local payload and overflow-chain prefix are reconstructed, with exact coverage and stopping evidence.
- [x] Supported SQLite serial types decode to typed application values using the database text encoding and validated record structure.
- [x] Decoded values retain structural provenance, serial type, declared-column enrichment when available, and overflow relationships without replacing cell identity.
- [x] Raw application payload bytes are never emitted as a success representation, diagnostic attachment, log field, URL component, or fallback for failed decoding.
- [x] A successful coherent enrichment publishes a new immutable revision while preserving all prior revisions and existing entity identities.
- [x] Failed, cancelled, or budget-stopped work leaves the prior published revision unchanged and reports precise coverage without implying complete decoding.
- [x] Broad graph, atlas, schema-flow, search, diagnostic, progress, and routine API responses remain free of seeded sensitive values; only the explicitly scoped selected-cell response may contain them.
- [x] Tests cover local payloads, multi-page overflow, NULL and numeric types, text encodings, BLOB representation without raw dumping, broken and cyclic overflow, repeated targets, concurrent requests, stale revisions, and invalidated snapshots.

## Answer

Implemented asynchronous selected-cell jobs through the public InspectionSession
API and browser. Requests capture the exact session, snapshot, revision, page, and
cell. Typed values are returned only through an exact-selector POST result lookup;
broad graphs retain structural provenance and coverage. BLOBs expose length only.
Column enrichment is explicitly unavailable pending the optional semantic helper.

Successful jobs publish a later immutable revision; the browser can revisit older
revisions. Budget stops, cancellation, stale work, malformed records, and input
invalidation withhold values and preserve prior publication. Configurable session
admission bounds retained jobs, workers, and per-job ceilings.

Validation: 119 distinct Rust tests passed across the full all-targets/all-features
run and the post-review rerun. The rerun excluded only the two unchanged sparse
gigabyte fixtures that passed in the full run. All 23 frontend tests passed,
including embedded assets, explicit disclosure, selection changes, and cancellation.
Clippy with warnings denied, rustfmt, and TypeScript checks passed.

Standards and Spec reviews against d0f27e7 have no unresolved blocking findings.
Review fixes corrected stopping coordinates, bounded job admission, and moved
revision preparation outside publication locks so cancellation can prevent
publication. Full-graph copying and global relationship indexing remain bounded
performance limitations for ticket 14.


## User acceptance — 2026-09-11

The user tried the Inspector and explicitly accepted v1: “I think it's nice. I accept.”
Previous tracker status: `resolved`. The existing implementation and validation record is retained above.
Closed as part of the accepted v1 effort; see [the acceptance record](../../../release/ACCEPTANCE.md).
