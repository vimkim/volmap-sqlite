# 16: Ship the reproducible v1 artifact

**What to build:** Close the v1 effort with one pinned, reproducible executable that contains the authoritative parser, inspection session, embedded page atlas and schema flow, focused TUI, hardened web server, and private metadata helper, and whose shipped behavior is verified end to end.

**Blocked by:** 10: Enrich semantic metadata safely; 11: Deliver the terminal inspection flow; 12: Harden local and remote web sessions; 14: Scale beyond memory-sized databases; 15: Contain hostile inputs and fuzz public boundaries

**Status:** done

- [x] Rust dependencies, frontend dependencies, build tools, and generated embedded assets are pinned sufficiently for a clean reproducible build.
- [x] The production artifact is one executable and does not require a separately installed JavaScript runtime, web server, SQLite command-line tool, or application dependency at runtime.
- [x] The executable exposes the browser and terminal inspection flows and keeps the bundled-SQLite metadata helper private rather than offering arbitrary SQL execution.
- [x] A clean-build smoke suite opens a real frozen fixture, publishes a fast revision, navigates physical and semantic projections, deep-inspects one cell, and exercises both browser and terminal adapters.
- [x] Packaging tests verify loopback-default serving, explicit remote warning, embedded assets, path privacy, no external connections, sidecar non-application, and selected-value disclosure boundaries in the built artifact.
- [x] Release verification includes valid, damaged, sidecar-bearing, invalidated, budget-stopped, and multi-gigabyte inspection scenarios at scopes appropriate to continuous and extended testing.
- [x] User-facing usage material consistently calls the product an Inspector, distinguishes the main-file image from a logical database image, requires frozen inputs, and states that v1 neither repairs nor recovers databases.
- [x] The shipped browser follows the page-atlas and schema-flow decision, the shipped TUI follows the terminal inspection flow, and no development-only prototype switcher or fixture data remains.
- [x] The complete production verification suite passes from a clean checkout and the resulting artifact reports its version and pinned build identity without exposing local build paths.


## Comments

Implemented on 2026-09-11 against baseline `b493cbb`. The pinned builder checks
asset freshness and emits a standalone executable, content identity, tool/asset
manifest, and checksum. Two independent clean builds produced identical binaries
and manifests. Clean verification passed 181 Rust tests (two existing ignored
entries), the separate semantic-helper harness, all 29 frontend tests, Clippy,
formatting, and the extended copied-artifact smoke including a 2 GiB input.

The packaged browser caught a windowed-revision refresh defect that discarded
selected deep values; a red/green regression and real-browser check verify its
fix and historical-value withholding. Standards review was clear; one spec-review
documentation correction was made and confirmed resolved.

The user additionally requested `just` modules and runnable examples. `user.just`
opens real files; `example.just` generates preserved frozen catalog, WAL, and
damaged inputs. Actual HTTP and PTY launches, path-with-spaces forwarding, demo
semantics, and byte preservation passed. Supplemental reviews were clear.

See [release verification](../../../release/VALIDATION.md),
[build and verification commands](../../../release/README.md), and
[guided examples](../../../examples/README.md). Dependency tickets 10–12 had stale
`ready-for-human` labels but completed checklists and no pending implementation;
their metadata, terminal, and web-security contracts passed the clean suite.


## User acceptance — 2026-09-11

The user tried the Inspector and explicitly accepted v1: “I think it's nice. I accept.”
Previous tracker status: `done`. The existing implementation and validation record is retained above.
Closed as part of the accepted v1 effort; see [the acceptance record](../../../release/ACCEPTANCE.md).
