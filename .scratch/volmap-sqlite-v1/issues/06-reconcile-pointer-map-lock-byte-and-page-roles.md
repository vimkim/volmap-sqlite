# 06: Reconcile pointer-map, lock-byte, and page-role evidence

**What to build:** Complete the page atlas's physical classification by inspecting applicable pointer-map and lock-byte pages, reconciling all accumulated role claims, and making unknown or conflicting pages explicit.

**Blocked by:** 04: Navigate B-tree topology and overflow claims; 05: Navigate the freelist

**Status:** resolved

- [x] Auto-vacuum header evidence determines whether pointer-map pages are applicable, and their physical locations are derived with checked arithmetic.
- [x] Supported pointer-map entries expose their entry kind, parent or owner claim where applicable, physical coordinates, and typed target identities.
- [x] Malformed entry kinds, invalid parents, contradictory relationships, and truncated pointer maps yield contained diagnostics without erasing readable entry evidence.
- [x] The lock-byte page is identified from database geometry when it falls within the main-file page range and is never parsed as an ordinary content page.
- [x] Every page retains all evidence-backed role claims from B-tree, overflow, freelist, pointer-map, lock-byte, and local structural evidence.
- [x] Exactly compatible claims may resolve to a page role; incompatible competing claims remain explicitly conflicting rather than being silently prioritized.
- [x] Pages with no validated supported role are shown as unknown or unreferenced evidence without speculative classification.
- [x] The page atlas provides a complete role and finding legend and visibly distinguishes supported roles, unknown pages, and conflicts across the entire main-file image.
- [x] Fixtures cover auto-vacuum and non-auto-vacuum databases, pointer-map boundaries, databases large enough to contain the lock-byte page, conflicting claims, and unreferenced pages.


## Answer

Implemented pointer-map inspection, geometry-derived lock-byte reservation, and evidence-backed page-role reconciliation in the published inspection graph and atlas. The atlas exposes map entries and navigation, physical coordinates, retained role claims, and a complete role/finding legend.

All five pointer-map entry kinds retain readable evidence through malformed kinds, invalid parents, contradictory ownership, and truncated trailing maps. Compact checked geometry describes map placement even when inventory stops early; inspected-map navigation lists only observed pages. Reserved map and lock-byte pages bypass ordinary content parsing.

Compatible generic and specific role claims resolve together. Stale headers on freed pages remain observations; incompatible supported roles remain conflicts. Losing physical owner support removes dependent links through the entire overflow chain, while independent evidence survives ownership ambiguity. Reconciliation checks cancellation throughout and processes dependencies in deterministic physical source order.

Validation:

- `cargo test --release --locked --all-targets --all-features`: 86 tests passed, including real SQLite auto-vacuum fixtures, truncated maps, boundaries, sparse gigabyte lock-byte fixtures, and dependent-chain regressions.
- `npm --prefix frontend test -- --testTimeout=20000`: 18 tests passed, including atlas and embedded production bundle checks; the command also runs TypeScript checks and rebuilds tracked assets.
- `cargo clippy --locked --all-targets --all-features -- -D warnings`, `cargo fmt -- --check`, and `git diff --check`: passed.
- Local test commands used `TMPDIR` under `target/test-tmp` because the system temporary filesystem was full.

Standards review against `053fb50`: addressed eager geometry enumeration, cancellation gaps, inspected-map navigation, and deterministic dependency processing; no remaining findings.

Spec review against `053fb50`: addressed dependent-link containment, transitive overflow support, and map navigation; no remaining findings.

Format reference: [SQLite database file format](https://www.sqlite.org/fileformat2.html).
