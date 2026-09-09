# Volmap SQLite Inspector

Read-only inspection of a frozen SQLite main-file image. The current implementation
publishes geometry, page inventory, and local table/index B-tree structural evidence.
Select a page for its byte map, header, cell-pointer inventory, cell extents, record
serial types, rowids, and supported freeblock/fragment layout. Application values
and raw payload bytes are never included in these responses.

Build the embedded browser assets, then run the standalone binary:

```sh
npm --prefix frontend ci
npm --prefix frontend run build
cargo run --locked -- /path/to/frozen.sqlite
```

Per-path topology ceilings are configurable with `--max-btree-pages` and
`--max-overflow-pages`; the shared aggregate prefix-allocation ceiling is
`--max-total-traversal-pages`. The B-tree and aggregate ceilings must be at least
one; an overflow ceiling of zero records the first unfollowed link. Their effective
values and topology completion, budget, or cancellation reason are disclosed in the
published graph and browser geometry panel. The aggregate `u64` ceiling is encoded as
a decimal string in JSON so browsers disclose it without precision loss.

Open the printed loopback URL. The browser shows scan progress until revision 1 is
published. Cancellation publishes the evaluated prefix with explicit partial coverage;
cancellation before geometry has no navigable revision. Invalid geometry produces a
fatal status. Changed input permanently invalidates the session: navigation and later
work are refused, while observed facts remain available as diagnostic evidence.

The session fingerprints the main file and adjacent `-wal`, `-journal`, and `-shm`
set. It compares device/inode, size, modification/change timestamps and file mode.
Streaming SHA-256 fingerprints also detect content differences when filesystem
timestamps have insufficient resolution. Hashes stay private and use a fixed 16 KiB
read buffer. Acceptance, publication and revision retrieval verify input contents;
lightweight status queries compare metadata. Content verification reads at most the
captured file lengths, runs outside the session mutex, and can be interrupted by a stop.
An interrupted integrity check cannot publish a revision. Content checks currently add I/O.
These checks detect changes; they do not acquire an atomic live-database snapshot.
Supply a stopped database or stable copy. Sidecar bytes are never applied.

The browser API separates session status from immutable revisions:

- `GET /api/snapshots/{snapshot_id}`: progress, state, coverage and diagnostics.
- `GET /api/snapshots/{snapshot_id}/revisions/{revision}`: a retained immutable graph; unavailable
  during scanning and rejected with HTTP 409 after invalidation.
- `GET /api/snapshots/{snapshot_id}/evidence`: retained invalidated observations,
  without a revision identity.
- `POST /api/snapshots/{snapshot_id}/cancel`: stop at the current validated boundary.

Page inventory coverage reports evaluated pages, trusted total when known, the next
uninspected page, remaining pages, and the completion or stop reason. A complete page
inventory does not imply that the full structural inspection has been implemented.

Each page carries separate local B-tree coverage (`complete`, `partial`, or
`unsupported`). Header recognition is a local role claim, not global role attribution.
Malformed boundaries carry diagnostics and stop dependent interpretation while
independent cells and page evidence remain available. Conflicting allocations have
no validated extent or dependent record facts. Record headers extending beyond local
payload report `needs_overflow`. Index keys are not decoded as application values.

The published graph separately records source-backed B-tree child and overflow
relationship claims, validated relationships, root- or cell-scoped traversal prefixes,
structured diagnostics, effective traversal ceilings, and topology coverage. Pointer evidence includes page-relative and main-file byte
coordinates plus a named SQLite validation rule. Missing targets retain their intended
page number without creating a page entity. Type conflicts, duplicate parents or
overflow, cycles, overlapping claim sources, broken overflow links, and incompatible
overflow ownership stop only the affected traversal. The browser follows validated
links in either direction and jumps from relationship diagnostics to their evidence
page and byte. Overflow traversal reads only the four-byte linkage fields and never
adds overflow payload bytes to the broad graph. Global page-role reconciliation remains
later work.

Freelist inspection starts at the header's first-trunk pointer and declared page
count. The graph records bounded trunk headers, declared leaf pointers, distinct
trunk/leaf roles, and a validated trunk prefix. Allocation coverage is independent
of page-inventory coverage: invalid counts, repeated claims, cycles, missing targets,
and operational stops retain evidence and report an unknown remainder. Unused leaf
contents and unused trunk slots never become active B-tree cells or relationships.
Independently supported storage links that also target a freelist page retain both
claims as conflicting evidence. The atlas provides a freelist entry point, trunk
navigation, allocation colors, byte regions, and source/target relationship links.

Trunk capacity uses usable bytes, excluding the reserved region. The reader accepts
all format-valid declared slots, including the final six slots that SQLite writers
avoid for compatibility with versions before 3.6.0. Those slots are ignored whenever
they lie outside the declared leaf array. See [SQLite's freelist format](https://sqlite.org/fileformat2.html#the_freelist).

The parser follows the [SQLite file format](https://www.sqlite.org/fileformat.html),
including its local-payload formulas. It reads one bounded page at a time, retains
structural facts rather than page buffers, and treats reserved bytes as opaque.
Ranges use zero-based `[start, end)` coordinates in both the containing page and
main file. Page 1 separates its 100-byte database header from the B-tree header.
Cell identity is snapshot-scoped `(page number, zero-based pointer-array index)`;
rowids and serial types use decimal strings to avoid JavaScript integer rounding.
Page detail is embedded in the immutable graph, so selecting a page does not read
or reinterpret database bytes through a separate UI path.

Verification:

```sh
cargo test --locked --all-targets --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo fmt -- --check
npm --prefix frontend run typecheck
npm --prefix frontend test
```

Session tests use public progress callbacks and channels to synchronize input changes
with inspection, without sleeps. Frontend tests also execute the rebuilt embedded asset.

Schema flow starts from directly decoded five-field `sqlite_schema` records on
page 1's validated table B-tree. It preserves each declaration's physical cell
identity, type, name, related table, signed root-page claim, and declaration text.
The reader follows reconciled overflow links for long schema records and supports
UTF-8 and both UTF-16 encodings. It never opens an SQLite connection, evaluates SQL,
loads an extension, or invokes a virtual-table module. See the
[SQLite schema storage format](https://www.sqlite.org/fileformat2.html#storage_of_the_sql_database_schema).

Only validated storage roots and their bounded B-tree traversal prefixes receive
schema attribution. Invalid, conflicting, non-root, and out-of-range claims retain
semantic diagnostics without changing physical page facts. Duplicate names remain
separate objects; competing claims to one storage root are withheld. WITHOUT ROWID
tables and physical shadow tables remain independent storage entries. Views,
triggers, and virtual tables with zero or NULL roots terminate as declaration-only
objects. Damaged records retain their source cell and unavailable metadata.

Choose a schema object to open the dedicated Schema flow workspace, then follow
its root, descendant pages, and cells. Page atlas and Schema flow share page/cell
selectors and selection; the physical evidence panel lists every validated schema
attribution for its page. Declarations render as text. Cell selection here exposes
structural evidence until an explicit deep-inspection request.

`--max-schema-bytes` caps aggregate schema-record payload bytes charged for decoding
(default 16 MiB; zero disables decoding). The graph exposes that ceiling, charged
bytes, partial/unavailable states, and the first unprocessed cell when known.
Schema attribution also respects the configured B-tree and overflow traversal
limits. A malformed or incomplete schema does not invalidate independently valid
physical evidence. This is a direct storage projection, not full SQL semantic
validation or the optional semantic-enrichment helper.

Select a physical cell and choose **Deep-inspect selected cell** to reconstruct its
validated payload and decode stored NULL, integer, real, text, or BLOB values.
Text uses the database encoding; BLOBs expose only type and byte length. Column
metadata is currently unavailable, and rowid aliases are not substituted.
The request captures the session, snapshot, base revision, page, and cell index.
It runs asynchronously with configurable payload-byte, overflow-page, and value-count
limits. Cancellation, malformed records, stopped overflow prefixes, stale revisions,
and changed inputs withhold values and preserve the previous revision.

A successful job publishes the next immutable revision. The revision picker retains
older graphs. Broad graphs and job receipts contain structural provenance and coverage,
never decoded application values. Values are held privately by the job and returned
only through its exact-selector result request; changing the selected cell hides them.

- `POST /api/snapshots/{snapshot_id}/deep-inspections`: start with `{target, budget}`.
- `GET /api/snapshots/{snapshot_id}/deep-inspections/{job_id}`: structural progress/outcome.
- `POST /api/snapshots/{snapshot_id}/deep-inspections/{job_id}/cancel`: cancel one job.
- `POST /api/snapshots/{snapshot_id}/deep-inspections/{job_id}/result`: supply the
  original selector as the body to retrieve the completed scoped result.

Default deep limits are 16 MiB of payload, 32,768 overflow pages, and 4,096 values.
Reconstruction follows the initial graph's validated prefix and cannot extend its
coverage. Frozen-input fingerprint checks still cover the captured input files.

Session admission defaults to 64 retained jobs and four concurrent workers, with
the default per-job limits also acting as ceilings. `InspectionSession::with_deep_limits`
configures these before sharing a session; status exposes the effective limits.
Requests exceeding them receive a terminal budget-stopped receipt and are not retained.
Invalid targets consume no admission slot. Accepted jobs and old revisions remain
available for the session lifetime. Revision preparation currently copies the graph
outside publication locks; cancellation prevents publication after preparation.
