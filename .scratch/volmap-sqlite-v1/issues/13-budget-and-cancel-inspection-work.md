# 13: Budget and cancel inspection work

**What to build:** Give operators explicit control over potentially unbounded inspection work and make every budget or cancellation stop visible as honest coverage rather than corruption, truncation, or false completeness.

**Blocked by:** 06: Reconcile pointer-map, lock-byte, and page-role evidence; 09: Deep-inspect one selected cell into a new revision; 10: Enrich semantic metadata safely

**Status:** resolved

- [x] Operator configuration exposes effective ceilings for resident memory, concurrency, traversal depth, linked-chain length, processed cells, reconstructed payload bytes, decoded value bytes, request size, and response size.
- [x] Fast scans, B-tree traversal, freelist and pointer-map traversal, overflow traversal, deep reconstruction, decoding, and helper execution check the applicable budgets at deterministic validation boundaries.
- [x] Fast and deep work can be cancelled without timing-dependent races, partial in-place graph mutation, or abandoned background tasks.
- [x] Reaching a budget or cancellation records the evaluated extent, exact stopping boundary, reason, trusted total when available, and known or unknown remainder.
- [x] Budget and cancellation outcomes never create corruption diagnostics solely because work stopped and never silently convert partial coverage into complete coverage.
- [x] Validated prefixes and independently complete evidence remain navigable after one facet stops, while dependent facts beyond the boundary remain absent or explicitly unknown.
- [x] Effective budgets, progress, cancellation controls, and resulting coverage are visible in both browser workspaces and the TUI.
- [x] Deep-inspection stops leave existing immutable revisions intact; a coherent partial enrichment may publish only when its partial coverage is explicit and disclosure invariants remain satisfied.
- [x] Deterministic tests stop every governed work category at multiple boundaries and verify cleanup, graph coherence, coverage semantics, diagnostics separation, and adapter presentation.

## Answer

Implemented against baseline `975e2c03cb5cb5c65533b62dfcf9664cb4063208`,
using the confirmed public session, HTTP, browser-component and terminal seams.

Shared operational budgets now govern resident-memory admission, page-cell work,
structural phases and freelist chains alongside existing traversal, schema, WAL,
helper and deep limits. Decoded-byte accounting bounds selected value construction.
The CLI and both adapters expose effective limits, progress and coverage receipts.
Linux RSS checks reserve conservative allocation headroom; this cooperative ceiling
is not an OS memory sandbox. README documents that limitation and reproducible
2,000-row and 20,000-row default-budget probes.

Phase receipts distinguish pending, complete, budget-stopped, cancelled and
unavailable work. Local schema/helper limits cannot become false completion.
Known admission totals survive early deep stops. HTTP refusals disclose coverage
without exposing partial serialized evidence or selected values. Existing revisions
remain immutable and validated structural prefixes remain navigable.

Worker cancellation is exercised at deterministic structural, payload and decode
boundaries. Deep waits join their workers; shutdown closes admission, cancels active
jobs and joins them. Regression tests cover an active worker and resource cleanup,
multiple budget boundaries, pre-publication terminal limits and both browser
workspaces.

## Standards

Independent review against the documented standards and code-smell baseline:
no unresolved findings. Schema/helper completion semantics and typed helper
coverage counters were corrected and rechecked.

## Spec

Independent review against ticket 13: no unresolved findings. Known deep admission
totals, active-worker shutdown coverage, pre-publication TUI limits, unavailable
helper receipts and HTTP budget coverage were verified in the final rechecks.

Review totals: Standards 0 unresolved; Spec 0 unresolved.

## Validation

- `cargo test --all-targets`: passed, including 152 Rust tests, semantic-helper
  containment, terminal process/PTY tests, sparse gigabyte fixtures and production
  HTTP security/disclosure tests.
- `npm --prefix frontend test`: all 25 tests passed; embedded assets rebuilt.
- `npm --prefix frontend run typecheck`: passed.
- `cargo clippy --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

## Comments

Completed implementation and both independent review axes on 2026-09-10.
The final full suite includes the reviewed fixes and HTTP coverage assertions.
