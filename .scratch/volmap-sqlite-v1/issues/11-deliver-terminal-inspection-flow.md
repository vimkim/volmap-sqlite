# 11: Deliver the terminal inspection flow

**What to build:** Provide a dense keyboard-oriented TUI that projects the same inspection graph and lets terminal users traverse schema or allocation entry points to pages and selected cells without mirroring the browser's spatial layout.

**Blocked by:** 06: Reconcile pointer-map, lock-byte, and page-role evidence; 07: Disclose sidecars without applying them; 08: Follow schema attribution in schema flow; 09: Deep-inspect one selected cell into a new revision

**Status:** done

- [x] The production executable can launch a Crossterm-based terminal inspection flow for the same frozen-input and inspection-session contract as the browser.
- [x] Database-level entry points include schema objects, B-tree storage, freelist, pointer maps when applicable, and diagnostics.
- [x] A visible path records the focused traversal from database through schema or allocation context to page and optional cell.
- [x] Keyboard-only controls support movement, entering a target, returning to its parent context, opening diagnostics, requesting or cancelling deep inspection, changing revision, and viewing help.
- [x] The focused evidence pane shows the active entity's physical facts, byte map or coordinate summary, role claims, relationships, diagnostics, and coverage without adapter-specific reinterpretation.
- [x] Sidecar mode, snapshot validity, inspection revision, fast and deep coverage, and pending work remain visible in the dense flow.
- [x] Typed values appear only after explicit deep inspection of the selected cell, and raw application payload bytes never appear in terminal fallbacks or diagnostics.
- [x] Terminal resize, narrow display, empty collections, rootless schema objects, invalid selectors, partial coverage, and invalidated snapshots degrade into usable labelled states.
- [x] A terminal harness verifies user-visible navigation and state transitions rather than Crossterm internals, using the same prepared graph assertions as browser adapter contract tests.


## Implementation

`--terminal` launches the Crossterm adapter over the shared InspectionSession.
Navigation preserves database/schema/allocation/page/cell context, supports physical
selectors and evidence scrolling, and exposes explicit deep requests, cancellation,
revision selection, diagnostics and help. Input controls are escaped at rendering.
The terminal harness tests visible frames, adapter parity, and real PTY lifecycle.

Review baseline: `961992a`. Confirmed test seams: inspection-session boundary and
user-visible terminal harness. See README for keys and terminal behavior.

Validation: `cargo test --release --all-targets`, all 13 terminal harness tests
(including real PTY navigation, compact resize, and terminal restoration),
`cargo clippy --all-targets -- -D warnings`, frontend typecheck, and all 23 frontend
tests passed. Formatting and diff whitespace checks passed.

Standards and Spec reviews against `961992a` have no remaining findings after
fixing configurable deep budgets, compact status/navigation, long evidence access,
and missing header/freeblock/record-state facts.


## User acceptance — 2026-09-11

The user tried the Inspector and explicitly accepted v1: “I think it's nice. I accept.”
Previous tracker status: `ready-for-human`. The existing implementation and validation record is retained above.
Closed as part of the accepted v1 effort; see [the acceptance record](../../../release/ACCEPTANCE.md).
