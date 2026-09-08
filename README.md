# Volmap SQLite Inspector

Read-only inspection of a frozen SQLite main-file image. The current implementation
publishes geometry and page inventory; B-tree structure, roles, and application values
are not inspected yet.

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
