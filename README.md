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

Opt into optional schema descriptions with `--semantic-metadata`. Schema flow then
labels ordinary tables with their declared column count (including generated
columns), `STRICT` status, and `WITHOUT ROWID` status. These descriptions live in
`semanticMetadata`, separate from directly parsed declarations, attribution,
diagnostics, coverage, identities, and selected values. Disabled or failed
enrichment is shown as unavailable; it does not change inspection coverage.

The same executable runs a private helper before starting any web runtime in the
child. The parent copies only accepted main-file bytes into a fresh private
directory and launches the helper with a fixed version token on stdin. Neither
source paths nor SQL are protocol inputs. SQLite opens the copy read-only and
immutable, so it cannot apply the original WAL/journal. The reply contains only
name hashes, bounded counts, booleans, and a fixed query-audit mask. The parent
matches hashes to unique directly parsed table objects and attaches descriptions
by their existing physical cell identities. Names and declaration text are never
returned by SQLite over this protocol.

The helper disables trusted schema, views, and triggers before reading schema.
It registers no functions, extensions, or modules. A deny-by-default authorizer
permits only the fixed schema read and table metadata pragma; statement tracing
rejects any other executed SQL. Because SQLite's `table_list` may initialize views
and virtual tables, the preceding schema read refuses schemas containing views
or any declaration containing `VIRTUAL` (case-insensitive, including comments and
quoted names). Rootless declarations remain available through direct parsing.
A partial or unavailable direct schema refuses the helper before it starts, so
the configured record budget never relies on an incomplete record count.
This conservative refusal also applies to unsupported, ambiguous, and damaged
schemas; v1 does not promise semantic descriptions for every valid database.

Configure lower operational budgets with `--max-semantic-bytes`,
`--max-semantic-records`, `--max-semantic-ms`, and
`--max-semantic-output-bytes`, or `InspectionSession::with_semantic_metadata_budget`.
Zero withholds optional metadata. Larger requests are clamped to the fixed
security ceilings; they cannot relax SQLite containment.

The fixed security ceilings are 64 MiB of copied input, 1,024 schema records,
256 columns per table, 64 KiB SQL/record strings, expression depth 32, compound
SELECT count 8, 10,000 VM program instructions, and 100,000 executed VM steps.
SQLite's heap ceiling is 16 MiB; the child also has a 256 MiB address-space limit,
two CPU seconds, and disabled core/file output. The parent enforces two wall-clock
seconds across copying and subprocess work and caps protocol output at 256 KiB.
Limits are installed before untrusted schema processing. Failure, refusal,
resource exhaustion, malformed replies, crashes, and timeouts all withhold the
entire description result. Frozen-input checks still run before publication.

Launch the terminal inspection flow with:

```sh
cargo run -- database.sqlite --terminal
```

The same production executable supplies the Crossterm terminal adapter, optional
metadata helper, and browser adapter. Terminal mode requires interactive stdin and
stdout; it does not fall back to printing application data when redirected. The
existing scan and semantic budgets also apply in terminal mode. Inspection runs
in a worker while keyboard input and scan progress remain available.

The terminal presents a focused list and evidence pane. Start with schema objects,
B-tree storage, freelist, pointer maps, or diagnostics, then enter a page and cell.
The breadcrumb preserves the route taken; returning restores the previous list
selection. The evidence pane uses shared graph coordinates, classifications,
role claims, relationships, pointer-map entries, diagnostics, and coverage.
It does not read or reinterpret database bytes. Snapshot identity is shown at the
database entry point; main-file mode, sidecars, revision, coverage, and deep-work
status stay in the frame header.

| Key | Action |
| --- | --- |
| Up/Down or k/j | Move in the focused list |
| Enter or Right | Enter the highlighted target |
| Esc, Backspace, or Left | Return to the previous context; cancel the selector prompt |
| PgUp/PgDn | Scroll focused evidence by the visible pane height |
| h/l; 0 | Scroll evidence horizontally; reset both scroll positions |
| g, then `page` or `page:cell`, Enter | Resolve a physical selector in the current revision |
| d | Explicitly deep-inspect the selected cell |
| c | Cancel pending deep/fast work and hide values |
| s | Stop the fast scan with explicit partial coverage |
| `[` / `]` | Select the previous/next available revision |
| ! | Open diagnostics |
| ? | Open help |
| q or Ctrl-C | Quit and restore normal terminal input |

Set terminal deep request budgets with `--max-deep-bytes`,
`--max-deep-overflow-pages`, and `--max-deep-values`, or configure the harness with
`TerminalFlow::with_deep_budget`. Defaults are 16 MiB, 32,768 overflow pages, and
4,096 values. The shared session enforces its admission ceilings; requests that
exceed them produce a budget-stopped outcome without values.

Deep inspection uses the shared asynchronous job and exact-selector result API.
Successful work selects its new revision only while the original cell remains
selected. Leaving the cell, changing revision, or cancelling hides values; a late
completion never restores a disclosure after navigation. BLOBs show typed lengths,
not bytes. Invalidated snapshots withhold navigation and values and label retained
observations as diagnostic evidence. Old published revisions remain immutable.

Frames adapt to terminal resize and keep bounded display widths. Below 40 columns
or 16 rows, a labelled compact layout retains the list and evidence pane while
space permits. Vertical scrolling advances by the visible pane height, so even a
one-line evidence pane can reach every row. Horizontal scrolling exposes long
schema declarations and typed-value tails. Long breadcrumbs retain the active
path suffix; the complete path is also available as scrollable evidence. Unicode names remain readable;
control characters and directional overrides are escaped before terminal output.
The terminal guard restores raw mode, cursor visibility, line wrapping, and the
alternate screen on normal exit and propagated terminal errors.

`cargo test --test terminal_flow` exercises visible frames and keyboard actions,
compares the shared fixture through both adapters, and launches the production
binary through a real PTY. That lifecycle harness requires Python 3 for tests;
the distributed executable has no Python dependency.

Web sessions default to `127.0.0.1:3000`. An explicit `--listen 0.0.0.0:3000`
(or an explicit IPv6 address) permits remote access and always prints a warning:
there is no built-in authentication or TLS, and operator-controlled network
protection is required. For wildcard listeners, the printed URL uses loopback;
remote operators replace that IP with the server's numeric address. Numeric IPs
and loopback `localhost` are accepted at the bound port; arbitrary DNS Host names
are rejected to prevent DNS rebinding. SSH forwarding must preserve the port.
A trusted reverse proxy must normalize Host and Origin to the chosen backend
origin; forwarded headers do not independently grant access.

The printed `/sessions/{session_id}` entry expires with the process. `/` redirects
to the active entry. Every API snapshot UUID is freshly generated for that session;
foreign snapshots, entry links, and job IDs do not resolve. Source IDs are opaque
output identities, not filesystem selectors. Queries are not supported and are
rejected, including requested collection limits. Deep selectors are JSON bodies
with bounded numeric fields and must match the active session and snapshot.
Historical revisions remain readable; starting new deep work requires the current
revision. Stored results require the exact original cell selector.

All assets are embedded and all browser requests stay on the chosen origin.
HTTP responses carry `no-store`, `no-cache` compatibility semantics, a restrictive
Content Security Policy, MIME-sniffing and framing protection, no-referrer policy,
and same-origin resource/opener policies. Bootstrap JavaScript is a session-scoped
external asset, so inline scripts are not permitted. POST requests require a
matching `Origin`; foreign origins and cross-site/same-site browser requests are
rejected. There is no CORS opt-in, access logging, telemetry, or outbound client.
Paths and rejected request contents are not echoed in HTTP errors.

Web admission ceilings are 512 URI bytes, 8 KiB of header names/values, 4 KiB of
body data, five seconds to receive a body, and eight simultaneous admitted
requests. Sixty-four accepted TCP connections bound transport residency, including
clients that stall before sending headers or while reading responses. Connections
expire after 30 seconds; normal browser polling reconnects. These are transport
limits, not inspection-work cancellation. Disconnecting a request does not release
its work slot before its handler finishes. Deep work also retains the separate
session job and worker ceilings described above.

JSON encoding stops at 8 MiB or 100,000 items in any array, without emitting a
truncated graph or partial selected values. Assets share the response byte ceiling.
Operators may lower the defaults with `--max-web-response-bytes` (minimum 1024),
`--max-web-collection-items` (minimum 1), and `--max-web-requests` (minimum 1).
These are admission ceilings, not database-corruption findings or performance
promises. HTTP 507 reports a `budget_stopped` response with a byte/collection reason;
413, 414, 431, 408, and 429 distinguish body, URI, header, body-timeout, and
concurrency limits. Oversized graphs are refused rather than sampled; pagination
and larger-scale projections belong to subsequent work. Lowering limits can also
withhold status, assets, and selected results; a completed deep job still leaves
its immutable revision intact even if its result exceeds the web response limit.

`cargo test --test web_security` starts production processes and exercises actual
HTTP listeners, origins, headers, selectors, disclosure, resource bounds, and
startup/log privacy. It requires Python 3 and Linux `strace`; syscall tracing asserts
that the server makes no outbound connections. Set `VOLMAP_TEST_CHROMIUM` to a
Chromium headless-shell executable to additionally boot the embedded UI under CSP
and check its network log for requests outside the selected origin.

## Operational budgets and cancellation

The same session controls fast work, deep work, the browser workspaces and the
terminal. Startup exposes these additional ceilings:

| Option | Default | Validation boundary |
| --- | ---: | --- |
| `--max-resident-bytes` | 268435456 | Before page, topology, schema-payload and revision allocations; structural checkpoints and deep admission |
| `--max-processed-cells` | 1000000 | Before accepting a page's structural cells |
| `--max-phase-units` | 1000000 | Work units in each named structural phase |
| `--max-freelist-trunks` | 32768 | Before following the next freelist trunk |
| `--max-decoded-bytes` | 16777216 | Before constructing the next selected typed value |
| `--max-deep-jobs` | 4 | Concurrent deep-job admission |
| `--max-retained-jobs` | 64 | Retained deep receipts and revisions |
| `--max-web-request-bytes` | 4096 | Before parsing an HTTP body |

Existing B-tree depth, overflow-chain, aggregate traversal, schema-decoding, WAL-frame,
helper, payload, value-count, HTTP-concurrency and response-size options still apply.
`--max-deep-bytes`, `--max-deep-overflow-pages`, `--max-deep-values`, and
`--max-decoded-bytes` set session-wide ceilings in both adapters. A browser may request
less for a selected cell. HTTP bodies cannot exceed the fixed 4096-byte security
ceiling; response and concurrency settings retain their existing hard caps.

A page is an inventory validation boundary. If accepting all of its cells would
exceed the cell ceiling, that page remains unevaluated; earlier pages remain
navigable. The receipt identifies the next page and remaining page count. It does
not invent a total cell count for unread pages. `workCoverage` separately records
named phases, evaluated units, trusted totals where available, next boundaries,
remainders, and stop reasons. A pending phase is never described as complete.
Traversal-specific receipts retain their exact unfollowed link. Schema/helper work
can stop while complete physical topology remains available. Helper coverage
separately describes copy/output bytes and resource-stop reasons.

Decoded-byte accounting counts UTF-8 bytes in returned value strings, including
numeric strings and BLOB length descriptions. NULL costs zero bytes. UTF-16 text
is sized before allocating its UTF-8 representation. A stopped deep job returns
structural coverage and withholds all values; it publishes no new revision.
Existing immutable revisions remain intact.

Resident-memory admission uses Linux `/proc/self/status` and conservative allocation
headroom. It includes process/runtime residency and retained revisions; concurrent
deep admissions reserve headroom together. It fails closed if residency cannot be
measured. This is a cooperative ceiling at validation boundaries, not an OS memory
sandbox: runtime/allocator overhead can change between observations. Operators
requiring a hard process/container cap should also apply their normal OS limit.
The inspector may stop early to preserve allocation headroom rather than exhaust
available memory. This ticket does not add the spill storage planned in ticket 14.

The browser's **Effective inspection budgets** and **Inspection work coverage**
sections are shared by page atlas and schema flow. Terminal database/help evidence
shows the same limits and receipts; PgUp/PgDn and horizontal scrolling expose long
entries. Browser cancellation and terminal `c` request cancellation; terminal `s`
records an operator stop. `InspectionSession::with_work_observer` can choose a
structural phase and evaluated extent deterministically. Observers run outside the
session lock. `DeepJob::wait` joins its worker, and adapter shutdown closes admission,
cancels jobs and joins them before returning.

Reproduce the small default-budget probe with:

```sh
cargo run --example budget_probe -- 2000
cargo run --example budget_probe -- 20000
```

On the development Linux host, the debug build inspected 2,000 indexed rows
(26 pages / 4,012 structural cells) in 102 ms with 7,040 KiB peak RSS, and 20,000 rows
(235 pages / 40,110 cells) in 937 ms with 23,524 KiB peak RSS. Both completed inventory,
topology and schema inspection under the defaults. These observations establish a
reproducible small-workload baseline, not a large-file performance guarantee. Timings
and residency depend on the host and allocator.
