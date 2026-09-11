# Ticket 16 release verification

Date: 2026-09-11. Review baseline: `b493cbb` on `main`.

## Regression evidence

The executable's `--version` contract was first exercised without a database and
failed because the option was unavailable. The added version/build identity made
that focused test pass.

The copied release executable exposed a browser defect absent from the earlier
component tests: completing a deep inspection advanced the windowed revision,
unmounted the selected-cell inspector, and discarded its values. A focused App
regression reproduced the missing result. The fix keeps the inspector mounted
while the next window loads and withholds values during that transition. The
regression then passed, including immediate value removal when selecting a
historical revision; the real packaged browser also displayed the selected value.

## Review

### Standards

No findings against the documented repository standards, domain vocabulary, ADRs,
or the code-review skill's heuristic baseline.

### Specification

One documentation finding was corrected: optional semantic enrichment does not
supply deep-value column names. The README again states that column metadata is
unavailable. The reviewer confirmed no remaining findings.

## Scope and limits

The continuous copied-artifact smoke passed with a real frozen fixture, Chromium
headless-shell, a PTY, the metadata helper, all 30 damaged main files, malformed
adjacent sidecars, invalidation, a zero-cell budget stop, HTTP security checks,
strict browser network logging, and server syscall tracing. Broad responses
exclude row values; an explicit selected-cell operation discloses its text while
withholding the other row and raw BLOB contents. The fixture hash is unchanged
apart from the deliberate invalidation edit, which the harness restores.

The headless-shell build is required by the strict browser network-log check.
An exploratory run with full desktop Chrome exposed browser-owned account/update
traffic; the test was not weakened to allow that traffic. The release documentation
now specifies headless-shell, and the harness explicitly requests headless mode.

The extended scenario is a 2 GiB sparse main file under a 64 MiB resident ceiling
and 1 MiB cache, with complete inventory/topology/schema and first/last-page reads.
It supplements the existing dense, overflow, fuzz, and memory-growth suites; it is
not a performance guarantee for arbitrary dense multi-gigabyte inputs.

The verifier's source tree predates the validation records, final ticket notes,
and subsequently requested `just` helpers and demo generator/guide. Those additions
are not build-identity inputs and are verified separately below. Final artifact identity
and clean-suite results are recorded below.

## Clean-suite results

On immutable tree `124d469ae2d3540327a02e87e9ab7bc94eea6b26`:

- `cargo fmt -- --check`: passed.
- `cargo clippy --locked --all-targets --all-features -- -D warnings`: passed.
- `cargo test --locked --all-targets --all-features`: 181 passed, zero failed,
  two existing ignored entries, plus the separate semantic-metadata executable's
  success/bounded-failure/disclosure checks.
- `npm test`: TypeScript build and all 29 frontend tests passed.

The ignored entries are the constrained-memory subprocess worker (invoked by its
parent regression) and the opt-in 820 MiB full pointer-map profile. The release
extended smoke independently checks the 2 GiB sparse scenario described above.

The copied release artifact also passed the complete **extended** smoke profile,
including the 2 GiB input with 32,768 pages, 64 MiB resident ceiling, 1 MiB cache,
complete inventory/topology/schema, and successful first/last-page navigation.

## Reproducibility result

Two independent clean builds passed and produced identical build manifests and
identical executable bytes:

- Executable SHA-256: `ee311a34fa99422e48c03b78ab63a5a5d3208941c7b60927aa0d04afe2faa02c`.
- Content build identity: `75c909d6727ee60287510d2d3ca735b7cf939ccf66010350a69c6ff38fb987fa`.
- Version: `volmap-sqlite 0.1.0`.
- Compiler: `rustc 1.97.1 (8bab26f4f 2026-07-14)`.
- Target: `x86_64-unknown-linux-gnu`.
- Browser: `Google Chrome for Testing 151.0.7922.34` (headless-shell).

See [the verification receipt](verification.json) and [the build manifest](verified-build.json)
for the machine-readable evidence. The final binary is emitted under
`target/verified-v1/volmap-sqlite`; it is intentionally not committed. Its dynamic
requirements are the Linux loader, glibc, libm, and libgcc_s; no SQLite or JavaScript
runtime library is linked dynamically. Re-running the verifier on the final commit
rebuilds the same production inputs; audit documents are not build-identity inputs.

## Requested justfile examples

The subsequent user request added root `justfile`, `user.just`, `example.just`,
and a development-only generator/guide. These do not change the verified
production source, embedded assets, dependency pins, or release build recipe.

Focused checks passed with just 1.57.0:

- The recipe list exposes both modules; `just user build` rebuilds successfully.
- `just user web` accepts a real path containing spaces and an ephemeral listener
  argument; its HTTP session publishes a revision.
- `just example web wal` generates/reuses examples and publishes a browser session.
- `just example terminal` enters the real PTY, displays schema evidence, and exits
  cleanly with `q`.
- Catalog evidence includes all advertised schema objects, overflow, freelist,
  and pointer maps. The damaged input retains an independently readable sibling.
- The WAL is validated by the release artifact and its main-file table has one
  cell; SQLite on a separate copied logical image sees two rows.
- Repeated generation and Inspector navigation leave all demo input hashes intact.

Separate supplemental standards and specification reviews both reported zero
findings. Full production verification was not repeated after these development
helper additions; the production/build inputs are byte-for-byte unchanged.
