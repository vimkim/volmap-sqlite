# 08: Follow schema attribution in schema flow

**What to build:** Give operators a semantic projection that starts from directly parsed schema declarations and follows validated root B-trees to descendant pages and cells without allowing semantic metadata to override physical evidence.

**Blocked by:** 04: Navigate B-tree topology and overflow claims

**Status:** resolved

- [x] The authoritative parser extracts supported schema records needed for object type, name, related table name, root page, and declaration from the physical schema B-tree.
- [x] Tables and indexes with valid root pages are attributed only through validated root and descendant B-tree relationships.
- [x] Views, triggers, and virtual tables without a directly attributable storage B-tree remain visible as rootless schema objects with a declaration-only state.
- [x] No extension, application-defined function, trigger, view body, or virtual-table module executes while schema evidence is inspected.
- [x] Physically present shadow tables remain independently visible even when their virtual-table module is unavailable.
- [x] Malformed schema records or invalid root claims yield unavailable or partial semantic metadata and contained diagnostics without changing authoritative page evidence.
- [x] The dedicated schema-flow workspace explicitly renders object to root B-tree to descendant pages and cells, including a clear rootless termination state.
- [x] Schema flow and page atlas share entity selectors and selection state: schema selection can land on a valid root page, while page selection shows all validated schema attribution.
- [x] Fixtures cover tables, indexes, WITHOUT ROWID storage, views, triggers, virtual tables, shadow tables, duplicate names, invalid roots, and damaged schema records.

## Answer

Implemented direct schema-record decoding in `src/inspection/schema.rs`, published
through `InspectionSession` before the final frozen-input verification. Schema
objects retain physical source-cell identities and evidence ranges; all five
schema fields support UTF-8/UTF-16 and validated overflow reconstruction. No
SQLite execution dependency is introduced into the inspector.

Attribution uses reconciled root claims and bounded descendant prefixes. Invalid,
non-root, conflicting, or damaged claims retain semantic diagnostics without
changing physical page evidence. Declaration-only objects and real FTS shadow
storage remain independently visible. `--max-schema-bytes` exposes a 16 MiB
aggregate payload-decode ceiling; preprocessing and attribution are cancellable.

`SchemaFlow.tsx` renders declaration → root → descendant pages → physical cells.
It shares selectors and selection with the atlas, displays all page attribution,
escapes declaration text, and distinguishes rootless termination from unavailable
attribution. The embedded production assets are rebuilt with this workspace.

Validation evidence includes real SQLite fixtures for table/index and WITHOUT ROWID
storage, all three text encodings, schema-tree descendants and long declarations,
views/triggers/virtual tables, unavailable-module FTS shadow tables, duplicate
names/root claims, invalid roots, damaged records/overflow, traversal/decode
budgets, cancellation and operator stop. Physical-graph equality with schema
decoding disabled and seeded application-value withholding are asserted.

## Standards

Reviewed against `517e9c7`. The initial cancellation finding and the two naming/type
suggestions were addressed. Follow-up review: no remaining documented-standard
violations or blocking findings.

## Spec

Reviewed against ticket 08 and its parent spec. Both reviews found no actionable
Spec gaps or regressions; every ticket fixture category is represented.

Review totals: Standards 0 remaining; Spec 0.

## Validation

- `cargo test --locked --all-targets --all-features`: ran the full suite; the two
  gigabyte-scale fixtures passed. Its progress-callback assertion exposed the new
  schema phase and was updated to distinguish schema from page-inventory progress.
- `cargo test --locked --test revisions`: all 9 passed after that assertion update.
- `cargo test --locked --test schema --test sidecars --test topology --test web_atlas`:
  all remaining suites passed (11 schema, 11 sidecar, 27 topology, 4 web).
  Together with the completed full-run suites: 108 Rust tests passed.
- `npm --prefix frontend test`: 21 passed, including the rebuilt embedded viewer.
  Its new navigation assertions wait for the production bundle's visible render.
- `cargo clippy --locked --all-targets --all-features -- -D warnings`: passed.
- `cargo fmt -- --check`: passed.
- `npm --prefix frontend run typecheck`: passed; also included in the final build.
