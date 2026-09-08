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
- `GET /api/snapshots/{snapshot_id}/revisions/1`: the published graph; unavailable
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
no validated extent or dependent record facts. Child/overflow page numbers are
untraversed claims; record headers extending beyond local payload report
`needs_overflow`. Topology, overflow traversal, and global role reconciliation are
later work. Index keys are not decoded as application values.

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
