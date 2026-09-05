# Volmap SQLite Inspector

Volmap SQLite Inspector is a read-only explorer of SQLite physical storage. It makes database-file structures, relationships, and anomalies navigable without acting as a general SQL browser, repair tool, or recovery tool.

## Language

**Inspector**:
The product that presents physical SQLite storage evidence and relationships without modifying or repairing the inspected database.
_Avoid_: Database browser, recovery tool, repair tool

**Inspection graph**:
The normalized, snapshot-scoped set of SQLite storage entities, structural evidence, relationships, and diagnostics projected by every user interface.
_Avoid_: Presentation tree, parsed file

**Inspection adapter**:
A user-interface projection of the inspection graph. An adapter presents shared inspection facts rather than defining its own interpretation of database bytes.
_Avoid_: Independent inspector, parser frontend

**Diagnostic**:
An evidence-backed finding about suspicious, invalid, unsupported, or incomplete physical structure. A diagnostic preserves what was observed and never implies that the inspector repaired the database.
_Avoid_: Repair, recovered data, generic error

**Database snapshot**:
The frozen set of SQLite database bytes accepted as one stable inspection input. It scopes every entity and relationship in the inspection graph.
_Avoid_: Live database, open connection

**Invalidated snapshot**:
A database snapshot whose input changed during inspection. Its already observed facts remain diagnostic evidence, but the inspector does not present the result as one coherent snapshot.
_Avoid_: Refreshed snapshot, partial update

**Page role**:
An evidence-backed classification of a database page as a B-tree, freelist, overflow, pointer-map, lock-byte, unknown, or conflicting page. A role may be derived from validated relationships rather than guessed from the page's leading bytes.
_Avoid_: Page type byte, ownership

**Schema attribution**:
The validated relationship from a SQLite schema object through its root B-tree to the physical pages and cells that store it.
_Avoid_: SQL query result, filename ownership

**Validation boundary**:
A bounded structure or reference whose prerequisite checks must succeed before dependent interpretation or traversal continues. Failure contains damage to that boundary while independently valid evidence remains usable.
_Avoid_: Global parse failure, trust score

**Validated prefix**:
The ordered members of a linked structure reached through consecutively valid boundaries before the first missing, invalid, cyclic, conflicting, or budget-stopped link.
_Avoid_: Recovered chain, complete chain

**Opaque evidence**:
Readable bytes whose meaning is outside the supported standard SQLite format, including extension-owned reserved regions and encrypted or custom-VFS content. Opaque evidence is preserved as a structural fact without speculative decoding.
_Avoid_: Corruption, decoded extension data

**Main-file image**:
The physical page state present in the SQLite main database file. It may differ from the latest committed database state when a WAL or rollback journal exists.
_Avoid_: Current database, logical database

**Logical database image**:
A transactionally meaningful page state obtained by applying the appropriate SQLite WAL or rollback semantics to a main-file image. Version 1 does not claim to construct this image.
_Avoid_: Main file, raw snapshot

**Sidecar evidence**:
The observed presence and supported structural metadata of a WAL, rollback-journal, or shared-memory file adjacent to the main database. Sidecar evidence warns about interpretation limits without silently changing the main-file image.
_Avoid_: Applied transaction, recovered state

**Explicit-target disclosure**:
The rule that structural facts, schema names, byte extents, and serial types may appear throughout the inspection graph, while typed application values appear only for a cell explicitly selected by the operator and raw application payload bytes are never disclosed.
_Avoid_: Row browser, hex dump

**Physical projection**:
A navigation of database pages, cells, and overflow relationships by their physical location in the main-file image.
_Avoid_: Schema hierarchy

**Semantic projection**:
A navigation from schema objects through their attributed B-trees to the pages and cells that store them.
_Avoid_: SQL result set

**Fast inspection**:
The complete, unsampled structural pass that establishes database geometry, page bounds, page-role claims, schema attribution, allocation structures, B-tree topology, and cell boundaries without disclosing application values.
_Avoid_: Sample scan, row decoding

**Deep inspection**:
The bounded, opt-in reconstruction of a selected cell's payload and overflow chain, including typed application values under explicit-target disclosure.
_Avoid_: Full database scan, automatic value decoding

**Operational budget**:
An explicit resource ceiling on inspection work, such as memory, concurrency, traversal depth, chain length, cell count, or decoded bytes. Reaching it is not evidence of corruption and never silently converts an incomplete inspection into a complete one.
_Avoid_: SQLite format limit, sampling rate

**Inspection coverage**:
The evidence-backed degree to which a requested inspection completed, including its evaluated extent, stopping boundary, reason, and any known or unknown remainder.
_Avoid_: Progress estimate, diagnostic severity

**Entity selector**:
A user-interface address that resolves to one typed entity in a database snapshot, such as a page or cell. A selector is not itself an entity identity or an on-disk reference.
_Avoid_: Entity reference, SQL predicate

**Web inspection session**:
One server process presenting a database snapshot and its inspection graph through same-origin browser views. It binds to loopback by default but may bind to a non-loopback address through explicit operator opt-in; its URLs are session-scoped rather than persistent hosted database links.
_Avoid_: Web deployment, database server

**Physical evidence**:
A fact decoded directly from bounded positions in the inspected SQLite files, including its byte coordinates and validation rule. Physical evidence is authoritative for storage layout and attribution.
_Avoid_: SQL query result, inferred schema behavior

**Semantic metadata**:
Failure-tolerant descriptive information about schema objects and declared columns that enriches physical evidence without replacing it. Its absence never invalidates the inspection graph.
_Avoid_: Physical evidence, application query result

**Rootless schema object**:
A schema object, such as a view, trigger, or virtual table, that has no directly attributable storage B-tree. Its declaration remains visible without inventing page ownership or executing extension code.
_Avoid_: Missing B-tree, unsupported page

**Entity identity**:
A typed, database-snapshot-scoped identity for one inspection entity. Pages use their one-based page number and cells use their containing page plus physical cell index; decoded keys, rowids, and schema names remain interpreted facts rather than replacement identities.
_Avoid_: Entity selector, row value, display name

**Inspection revision**:
An immutable version of one inspection graph within the same database snapshot. A completed deep inspection creates a later revision with additional evidence while preserving existing entity identities.
_Avoid_: Database version, mutable result

**Published inspection**:
A navigable inspection graph whose fast inspection either completed or stopped at a recorded boundary. Scan progress may be observed before publication, but it is not presented as a coherent graph without explicit partial coverage.
_Avoid_: Live partial tree, progress event

**Application value**:
A typed value reconstructed from a selected SQLite record payload. It is distinct from structural metadata and may cross the web connection only under explicit-target disclosure.
_Avoid_: Raw payload, structural fact

**Source identity**:
An opaque, snapshot-scoped identifier used by adapters in place of an absolute database path. A separate display name may identify the source to the operator without disclosing its filesystem location.
_Avoid_: Absolute path, database URL
