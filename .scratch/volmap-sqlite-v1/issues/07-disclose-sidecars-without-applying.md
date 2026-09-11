# 07: Disclose sidecars without applying them

**What to build:** Automatically identify adjacent WAL, rollback-journal, and SHM evidence and explain its interpretation consequences everywhere an operator navigates, while preserving the physical main-file image exactly as scanned.

**Blocked by:** 02: Publish coherent frozen inspection revisions

**Status:** done

- [x] Adjacent WAL, rollback-journal, and shared-memory candidates are discovered as part of the accepted database snapshot without exposing their absolute paths.
- [x] Supported WAL header, frame, commit-boundary, checksum, and validation metadata is reported as sidecar evidence without overlaying any frame onto a main-file page.
- [x] Supported rollback-journal metadata and hot-journal evidence are reported only when their prerequisites can be established; uncertain lock or format state remains explicit rather than guessed.
- [x] Supported SHM or WAL-index metadata is labelled native-order, non-authoritative coordination and lookup evidence that is not required for recovery.
- [x] Malformed or truncated sidecars produce their own bounded diagnostics and do not invalidate independently valid main-file parsing unless the accepted input set changes during inspection.
- [x] Main-file page facts and identities are byte-for-byte and semantically identical with and without adjacent sidecars.
- [x] The page atlas and detail surfaces prominently identify the result as a physical main-file image and distinguish WAL-not-applied, rollback-not-applied, and SHM-non-authoritative consequences.
- [x] Sidecar names and selected metadata may be shown using path-private display values; absolute paths and raw sidecar payloads never enter routine responses or URLs.
- [x] Fixtures cover absent, valid, malformed, and changing sidecars; WAL plus SHM; rollback journals with different hot-journal evidence; and the invariant that no logical database image is constructed.

## Answer

Implemented separate sidecar evidence in the immutable inspection graph and retained observations. Accepted WAL, rollback-journal, and SHM candidates expose path-private display names, file extents, supported fields with sidecar-relative coordinates, validation results, diagnostics, and evaluated coverage. Unreadable or non-regular candidates do not hide independently valid main-file facts; changes to the accepted input set still invalidate the snapshot.

WAL inspection validates header format and checksum, frame salts and cumulative checksums, and retains validated frame/commit prefixes. A salt mismatch stops at an unresolved generation boundary: ordinary SQLite WAL reuse can leave old-generation frames in the tail. Frames never enter main-file topology or replace page bytes. `--max-wal-frames` (default 100,000; zero means header only) and `InspectionSession::begin_with_budgets` expose the evidence ceiling. Cancellation and budget stops retain exact byte coverage.

Rollback-journal support covers the first header and declared record extent. Empty/zeroed journals and the size prerequisite are distinguished. Reserved-lock ownership and super-journal prerequisites remain explicitly unknown/uninspected, so readable headers do not claim a hot journal or recovery eligibility. Journal records are neither validated nor replayed. SHM support covers both native-order header copies, their checksums, selected checkpoint fields, and header/file consistency; hash tables, live locks, and recovery are outside that supported scope.

The atlas prominently labels the physical main-file image, preserves sidecar consequences during page selection, and presents metadata, diagnostics, and paginated WAL frame evidence. The rebuilt production assets are included.

Validation:

- `cargo test --release --locked --all-targets --all-features`: 97 tests passed.
- `npm --prefix frontend test -- --testTimeout=20000`: 19 tests passed, including the embedded production asset; TypeScript checks and production build passed.
- `cargo clippy --locked --all-targets --all-features -- -D warnings`: passed.
- `cargo fmt -- --check` and `git diff --check`: passed.
- Test fixtures include SQLite-produced WAL+SHM, both WAL checksum byte orders, ordinary WAL reuse, malformed/truncated boundaries, journal header prerequisites, absent/non-regular/changing sidecars, cancellation, budget stops, and unchanged main-file facts. Commands used `TMPDIR` under `target/test-tmp` because the system temporary filesystem was full.

Review baseline: `c9b0102`.

- Standards: no documented-standard violations. One non-blocking suggestion to replace string-valued sidecar domain states with enums remains; it does not affect the observed behavior.
- Spec: corrected the WAL reuse/generation-boundary finding; final recheck found no remaining concrete spec findings.

References: [SQLite file formats](https://www.sqlite.org/fileformat2.html), [WAL-index format](https://www.sqlite.org/walformat.html), [hot-journal prerequisites](https://www.sqlite.org/lockingv3.html).


## User acceptance — 2026-09-11

The user tried the Inspector and explicitly accepted v1: “I think it's nice. I accept.”
Previous tracker status: `resolved`. The existing implementation and validation record is retained above.
Closed as part of the accepted v1 effort; see [the acceptance record](../../../release/ACCEPTANCE.md).
