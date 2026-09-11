# Hostile-input verification

Ticket 15 uses the inspection session, HTTP router, terminal flow, public entity
and deep-selector decoders, and semantic helper protocol as test seams. The parser
internals remain private: geometry, B-tree headers/cells/records/freeblocks,
schema records, overflow, freelist, pointer maps, WAL, rollback-journal and SHM
interpretation are reached through `InspectionSession` using real bounded files.

## Continuous profile

Run on Linux with Rust and Python 3, from the repository root:

```sh
cargo test --test hostile_inputs
python3 fuzz/run.py
```

The runner builds the replay executable and production helper, then runs all
checked-in seeds plus 16 deterministic mutations per target. It uses seed 15 by
default. No database service, network, additional Python package, or nightly Rust
is required. This is a deterministic mutation campaign, not coverage-guided
libFuzzer; the explicit target/seed matrix below prevents random-byte geometry
rejection from being mistaken for coverage of deeper parsers.

Each candidate executes in a fresh subprocess with these hard limits:

| Resource | Ceiling |
| --- | --- |
| Input candidate | 32 KiB |
| Process address space | 1 GiB |
| CPU time | 10 seconds |
| Wall time | 15 seconds; watchdog kills the process group |
| Open descriptors | 128 |
| Individual output/spill file | 64 MiB |
| Core files | disabled |
| Inspector resident memory | 256 MiB |
| Structural cells / phase units | 4,096 / 16,384 |
| B-tree / overflow / freelist depth | 64 / 64 / 64 |
| Total traversal pages | 4,096 |
| WAL frames | 16 |
| Schema bytes | 16 KiB |
| Selected deep payload / decoded bytes | 16 KiB / 16 KiB |
| Selected deep overflow pages / values | 16 / 64 |
| Deep selections per candidate | First 8 cells, each with normal and cancelled execution |

Input-size, iteration and inspector limits are deterministic. The OS CPU/wall
limits are last-resort failure detectors, not successful coverage stops. A timeout,
crash, failed invariant or memory-limit termination fails the campaign. A normal
inspector budget/cancellation receipt must retain honest incomplete coverage.
The production Rust crate forbids unsafe code; bundled SQLite executes only in the
resource-limited metadata helper. Fuzz success is evidence for tested inputs,
not proof that every possible adversarial input has been exhausted.

## Extended profile and replay

```sh
python3 fuzz/run.py --profile extended
python3 fuzz/run.py --profile extended --seed 27157
python3 fuzz/run.py --target records --iterations 2000 --seed 42
python3 fuzz/run.py --target http --replay target/hostile-reproducers/NAME.bin
```

Extended runs all seeds plus 1,000 mutations per target. An explicit override is
bounded to 0–10,000 mutations per target. Every invocation is finite. Rotate seeds
for additional campaigns; retain the seed, toolchain and commit with results.

Before executing each candidate, the runner writes its exact `.bin` input and
`.json` target/seed/index/SHA-256/limits/command metadata under
`target/hostile-reproducers/`. Failure also retains `.log` output and prints a replay
command. Successful candidates are removed. Replay uses the retained bytes rather
than relying on a random generator version. Minimize an actionable failure and
promote it to `tests/corpus/damage/` with independently reasoned expectations.
The watchdog terminates child helpers with the candidate's process group.

## Target and invariant matrix

| Target | Inputs and production boundaries | Properties |
| --- | --- | --- |
| `database` | Arbitrary files and all corpus seeds → geometry, local page parsers and session publication | No panic; bounded extents; page/phase coverage arithmetic; source privacy; opaque unsupported input; HTTP and small terminal frames |
| `traversal` | Corpus mutations preserving the 100-byte database header → B-tree/overflow, freelist, pointer-map and role reconciliation | No repeated/out-of-range page in validated prefixes; bounded work and valid coverage receipts |
| `records` | Header-preserving cell/schema/record mutations → fast structural decoding and explicit deep jobs | Immutable previous revision; bounded reconstruction; deterministic normal and cancelled runs for each of the first eight cells (all cells in the small corpus fixtures); wrong-selector result refusal |
| `sidecars` | Short garbage, worked WAL examples (both checksum orders), journal/SHM structures and mutations → all three sidecar parsers | Main-file facts equal a sidecar-free inspection; every input remains byte-identical; no application of sidecars or raw-byte disclosure |
| `selectors` | Entity identity, deep selector/budget JSON, integer maxima, duplicates/nesting, terminal page[:cell] input | Bounded decoding, invalid target handling and collection boundaries; terminal input cannot reveal stored values |
| `helper` | Arbitrary replies through the public bounded decoder and injected enrichment subprocess; arbitrary stdin through the production helper | Exact protocol/schema validation, no helper text in metadata, physical facts unchanged, no source mutation; parent enforces output/time limits |
| `http` | Bounded body, URI, headers, method, malformed JSON and all selector shapes through the production router | Admission budgets and origin checks; errors never echo payload markers; bounded responses; no internal server failures |
| `lifecycle` | Deterministic cancellation at inventory boundaries and source changes after publication | No resumed work after cancellation; no fabricated complete inventory; immutable retained revision and HTTP invalidation |

The focused disclosure regression uses distinct selected/unselected text and a
BLOB marker with a damaged sibling cell. It proves selected-value availability,
wrong-selector refusal and absence of unselected values or raw BLOB bytes in
results, revision/status/metadata responses, terminal navigation and rejected HTTP
requests. The existing `deep_inspection`, `revisions`, `operational_budgets`,
`semantic_metadata` and `web_security` suites additionally exercise mid-phase and
mid-reconstruction cancellation, subprocess failures, immutable revisions and
real TCP request limits.

## Damage corpus contract

`tests/corpus/damage/manifest.json` records explicit checks and validation boundaries
for every file. `generate.py` constructs byte-level SQLite format examples and
literal expectations; it never calls the inspector to manufacture an oracle.
Binary files are checked in so replay requires no generator. The SHM example is
little-endian; checksum invalidity and non-authority assertions remain applicable
when a different native byte order is unsupported.

The corpus test runs **every case** through the session, HTTP status/revision
contracts, and terminal flow. It compares HTTP evidence with the complete session
projection and navigates every retained page and cell in the terminal, checking
coverage, regions and diagnostic codes. Collection matching uses stable identities
or subset predicates, not implementation ordering. Ordered validated-prefix arrays
are compared exactly. A complete page inventory does not imply that every local
record, traversal or sidecar is complete: the oracle asserts their distinct stops.

For untrustworthy geometry, no graph is invented: source identity, zero evaluated
pages, unknown total/remainder, an opaque input extent and a boundary diagnostic
remain available to both adapters. Nonstandard readable content with no SQLite signature carries an opaque
input extent. `fatal`/`fatal_geometry` in the session state/coverage describe the
inability to establish page geometry, not a corruption verdict; the diagnostic is
`unsupported_format`. Unsupported pages with valid geometry retain `opaque_content`
extents and `unsupported` local coverage, and reserved bytes retain
`opaque_reserved`. No proprietary decoding is attempted.

To regenerate identical fixtures and run the corpus:

```sh
python3 tests/corpus/damage/generate.py
cargo test --test hostile_inputs
```
