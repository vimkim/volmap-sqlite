# 01: Inspect a real database in an embedded page atlas

**What to build:** Make the first production tracer bullet work end to end: an operator gives the standalone inspector a valid frozen SQLite main file, and the same executable directly parses its geometry into a snapshot-scoped inspection graph and serves a minimal embedded page atlas over loopback. This establishes the inspection-session seam, production toolchain, and path-private browser delivery without promoting prototype code.

**Blocked by:** None (can start immediately)

**Status:** done

- [x] One production executable is scaffolded around a Rust 2024 core with unsafe code forbidden, an Axum/Tokio server, and embedded React/TypeScript/Vite browser assets.
- [x] A valid SQLite main file is opened read-only and its magic, page size, usable size, reserved bytes, page count, and text encoding are decoded through bounds-checked direct parsing.
- [x] The inspection graph contains a database snapshot and one page entity for every complete page, identified by its one-based page number without loading the entire file into memory.
- [x] The source has an opaque snapshot-scoped identity and sanitized display name; absolute source paths do not appear in browser responses, HTML, logs, or URLs.
- [x] The embedded server binds to loopback by default and serves both its API and browser assets from the same origin.
- [x] The browser opens on a production page-atlas shell that displays snapshot geometry and the complete page mosaic from the real inspection graph.
- [x] Invalid or insufficient database geometry yields a bounded fatal result rather than guessed pages, panics, or unbounded allocation.
- [x] An end-to-end test invokes the inspection-session boundary with a generated valid fixture and verifies geometry, page identities, path privacy, embedded asset delivery, and atlas rendering.
- [x] Production code is written from the accepted architecture and information design; no throwaway prototype implementation is copied or promoted.



## User acceptance — 2026-09-11

The user tried the Inspector and explicitly accepted v1: “I think it's nice. I accept.”
Previous tracker status: `ready-for-agent`. Implementation commits: `a75d9b1, 888bf81`. Regression evidence: `tests/web_atlas.rs` and the ticket 16 clean production verification.
Closed as part of the accepted v1 effort; see [the acceptance record](../../../release/ACCEPTANCE.md).
