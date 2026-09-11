# 14: Scale beyond memory-sized databases

**What to build:** Demonstrate that complete unsampled structural inspection remains usable for multi-gigabyte main files under constrained memory, with implementation details such as caching and spill storage hidden behind the inspection-session contract.

**Blocked by:** 13: Budget and cancel inspection work

**Status:** done

- [x] Main-file reads use positional access and never require whole-file residency or a mutable shared seek cursor.
- [x] Page decoding, relationship traversal, and adapter collection queries operate through bounded caches and streaming iteration.
- [x] Snapshot-wide indexes use bounded memory and spill to private temporary storage when benchmark evidence shows in-memory retention would violate the configured ceiling.
- [x] Decoded payloads are retained only for explicit deep-inspection targets and never accumulated across the full database.
- [x] Generated or sparse multi-gigabyte fixtures complete an unsampled structural inspection when budgets permit and preserve exact partial coverage when they do not.
- [x] Constrained-memory tests measure peak resident memory, prove it does not scale linearly with input size, and confirm that spill behavior does not change graph facts or entity identities.
- [x] Cancellation remains responsive during long sequential scans, index spilling, traversal, and selected payload reconstruction.
- [x] Reproducible benchmarks record representative sizes, page counts, topology, elapsed time, peak memory, temporary storage, and effective budgets.
- [x] Default resource settings and any claimed performance envelope are derived and documented from those benchmark results rather than guessed.
- [x] A practical automated regression profile protects bounded-memory and unsampled behavior, while heavier multi-gigabyte benchmarks remain reproducible on demand.

## Comments

2026-09-10: Baseline measurements at `9c73e11` are recorded in
`benchmarks/ticket14-baseline.json`. The existing debug probe completes 20,000
indexed rows; 100,000 rows stop during topology, and 500,000 rows stop during
page inventory under the default 256 MiB resident-memory ceiling. VmHWM includes
fixture generation, so final acceptance benchmarks must measure inspection in a
separate process after generating the fixture.

The current session retains `Arc<Vec<PageEntity>>`; topology creates additional
snapshot-wide vectors and maps. The browser requests one serialized graph and
renders all pages. Implementation must cover bounded storage and bounded adapter
queries together, preserving immutable revisions and complete structural facts.
The public test seams, additional process-level benchmark seam and review baseline
were proposed for confirmation before starting the TDD slices.

Confirmed: session, HTTP, browser and terminal seams plus process-level memory/spill benchmarks; review baseline `9c73e11`.

2026-09-10 implementation checkpoint: page inventory, reconciled classifications,
freelist page overrides, relationship claims, normalized relationships and traversal
records now share bounded caches and private spill storage. Claim storage preserves
private stop reasons without changing public JSON. Forced-spill valid and corrupt
fixtures agree with memory-backed inspections. Bounded summary/page APIs exist;
browser and terminal integration is still pending. Snapshot-wide diagnostic,
schema and pointer-map collections and bulk-operation cancellation remain under
review; this ticket is not complete.

Intermediate isolated release probes completed unsampled sparse 2 GiB/4 GiB
inventories and structural phases at 3.75 MiB peak RSS (87/170 seconds). Before
moving claims and traversal records into spill storage, generated overflow-heavy
2 GiB/4 GiB files completed at roughly 19/34 MiB RSS (193/511 seconds). These are
intermediate measurements, not the final performance envelope. The benchmark
runner now records source and binary SHA-256 hashes, and a fresh large overflow
run is in progress. Work tracker 112 retains the current process and log pointers.

2026-09-10 adapter checkpoint: revision metadata omits structural collections;
schema declarations and attributed pages have independent bounded queries. Claims,
relationships and traversals have stored page indexes for selected-page queries,
also used by explicit overflow payload inspection. Browser and terminal now browse
collection windows and navigate to physical pages outside the current window.
The browser can reduce a refused window; terminal collection paging uses `<`/`>`.
Focused validation passes: 27 browser tests, 14 terminal tests (including the
production PTY harness), 15 large-inspection tests, and the existing deep/revision
tests. Strict Clippy passes at this checkpoint.

Remaining before completion: audit single-record allocation and spill cancellation,
finish adapter edge cases, regenerate final benchmark evidence and resource-default
documentation, run the full suite and two-axis review, then commit on `main`.
The previously reported large-file measurements remain intermediate evidence.

2026-09-10 storage audit checkpoint: high-degree index/diagnostic/role-record
allocations now reserve headroom before cloning or allocating vectors; the process
regression verifies an explicit resident-memory stop before a large diagnostic is
built. Cached records now remain in place when storage fills: new records spill
individually, and changed cached records are evicted only after their replacement
is written. Raw page prefixes remain cached with the remaining raw-cache allowance
used for disk reads. There are no bulk cache-to-disk migration loops. A regression
cancels at the first spilled page with a private-file ceiling too small to copy the
cached prefix and verifies that the full observed prefix remains navigable.

Focused checks cover mixed memory/disk ordering, cancellation, spill ceilings,
process memory growth, deep inspection, and both terminal harnesses. Final benchmark
evidence is still pending. The adapter audit must also restore complete reverse
schema attribution for selected pages in bounded mode; displaying only the current
schema object's page window is insufficient for the earlier schema-navigation
contract.

2026-09-10 navigation checkpoint: reverse page-to-schema attribution now has a
spillable page/object index and bounded session/HTTP queries. Browser and terminal
navigation reach declarations outside the current object window. Terminal tests
pass (14), as do the large-inspection tests (17) and strict Clippy.

A new dense indexed, full-auto-vacuum benchmark exposed a page-query rejection:
valid dense 64 KiB pages exceeded the usual 8 MiB decoded-window estimate. A
single page can now reserve up to 32 MiB (the estimate includes 4x serialized
bytes), still subject to resident-memory admission; subsequent pages retain the
usual window ceiling. The regression checks complete cell access and serialized
response size below 8 MiB. Isolated release probes for 8/16 MiB dense indexed
fixtures complete and retrieve first/last pages in 16.8/32.9 seconds with
18,448,384/17,879,040 bytes peak RSS. Intermediate evidence is retained at
`/tmp/volmap-ticket14-dense-after.json`; final large-file benchmarks and release
documentation remain pending.

2026-09-10 review checkpoint: the full Rust suite and 28 frontend tests pass on
the pre-review candidate. Standards review found deep page reads preceding memory
admission; those reads now use admitted page retrieval, including overflow role
checks, and concurrent deep reservations include selected-page headroom. Focused
deep (16) and operational (12) tests and strict Clippy pass after this fix.
Unpublished topology also participates in retained-evidence export admission.

Spec review still requires full pointer-map record query support, allocation
admission before pointer-map decoding/mutation, and retention of the selected
schema declaration when the browser catalog window changes. Standards review also
suggested named operations for the private atomic storage-failure state.

The release probe now includes real terminal navigation. Its 2 GiB sparse scan
completed in 63,203 ms at 4,587,520 bytes peak RSS, but navigation took 105,908 ms
because collection queries repeatedly hash the full frozen input. This is a
remaining usability issue, not a final performance claim. The benchmark chain was
deliberately stopped during its 4 GiB run; prior evidence is retained in
`/tmp/volmap-ticket14-navigation-before.json`. Optimize verification/navigation
while preserving content-change detection, resolve review findings, and then
regenerate the final evidence before committing.

2026-09-10 review-fix checkpoint: selected schema declarations now survive catalog
window changes; the browser regression and all 28 frontend tests pass. Full 64 KiB
pointer-map records use the admitted single-record window, and map construction
and edits reserve memory first. The extended sparse 820 MiB regression verifies
all 13,107 entries through local/global queries and verifies an empty, incomplete
map with a pointer-map budget receipt under constrained headroom. Standards
follow-up reports no remaining findings after deep admission and private named
storage-error handling changes.

The private frozen-input fingerprint now uses pinned BLAKE3 with the same full-byte
verification and fixed 16 KiB cancellation boundaries. Revision/invalidation and
operational tests pass. An isolated 2 GiB sparse probe records 31,314 ms scan time,
7,736 ms terminal navigation, and 5,242,880 bytes peak RSS; intermediate comparison
is at `/tmp/volmap-ticket14-navigation-after.json` (before: 105,908 ms navigation).

Spec follow-up identifies one remaining adapter gap: very long traversal prefixes
are indivisible collection records and can exceed even the admitted single-record
response limit. Add independently bounded traversal headers and prefix-step queries
backed by storage reads that do not deserialize the entire prefix, then wire both
adapters and validate long-prefix navigation. Final benchmarks/documentation,
final verification and commit remain pending.

2026-09-10 traversal-storage checkpoint: private traversal records now contain a
small JSON header followed by fixed-width page numbers. SQLite incremental BLOB
reads fetch the header or requested steps without materializing the full prefix.
The records table now has rowids for this access; its initial file requirement is
28 KiB. Legacy full-traversal DTOs still decode to the same graph facts.

`traversal_header_batch` (optionally page-scoped) and `traversal_prefix_batch` are
available through the session and HTTP routes. A regression compares all paged
steps to complete traversal facts in memory and spilled sessions. Another lowers
the HTTP response ceiling to 4 KiB, verifies rejection of a whole traversal, and
then reconstructs its complete prefix from readable 17-step windows. The large
inspection suite passes (19 tests, one extended test ignored); existing deep (16)
and topology (27) tests pass, and strict Clippy passes.

Browser and terminal must now use these new header/step queries. They still use
whole traversal records at this checkpoint, so the reviewer finding is not yet
closed. Final benchmark evidence must be regenerated after adapter integration.

2026-09-10 adapter completion checkpoint: browser and terminal now request traversal
headers and independent prefix windows. Terminal page/cell entries open a traversal
focus; `</>` pages its ordered prefix and Enter follows a page. The browser opens
one explicitly selected prefix and pages it independently. Regressions exercise
steps 33–64 and follow pages beyond the initial window. All 15 terminal and 28
browser tests pass, as does strict Clippy. Follow-up Standards and Spec reviews
report no remaining substantive findings.

Final full-suite/extended-map verification and fresh sparse/overflow 2/4 GiB plus
dense-indexed 8/16 MiB benchmarks are running. Work tracker 112 records their live
handles and logs. Measured-results/default rationale, final acceptance audit and
commit remain pending; this ticket is still claimed.

## Final acceptance evidence

The implementation is reviewed against baseline `9c73e11`. Standards and Spec
follow-ups found no remaining substantive findings after admission, pointer-map,
selected-schema, and traversal-window fixes.

| Requirement | Evidence |
| --- | --- |
| Positional main-file access | `inspection.rs`, `btree.rs`, `topology.rs`, `freelist.rs`, `pointer_map.rs`, `schema.rs`, and `deep.rs` use `FileExt` positional reads. Frozen-input hashing uses its own file handle and a fixed 16 KiB buffer. |
| Bounded decoding, traversal, and adapter queries | `storage.rs`, `index.rs`, and `index_map.rs` bound cached evidence; session collection APIs admit windows. `index/traversals.rs` reads only a header/requested prefix steps through incremental BLOB access. Browser and terminal tests navigate beyond their first page, schema, and traversal windows. |
| Private spill indexes | One session-owned temporary SQLite store serves pages and snapshot indexes. Forced-spill tests check storage ceilings and accepted-prefix navigation; large-profile tests require spilled indexes and enforce the cache allowance. |
| Selected payload retention | Structural records omit application values. All 16 deep-inspection tests pass, including selected-only reconstruction, broad-graph non-disclosure, revision immutability, and worker/admission bounds. Selected-page and overflow-role loads reserve memory before decoding. |
| Complete or exact partial coverage | Large-inspection tests verify spill stops, cancellation at first spill and during reconciliation, schema stops, and readable retained prefixes. Multi-gigabyte measurements record every page and every structural phase; final results are linked below. |
| Measured memory and graph parity | `large_profile.rs` measures inspector subprocess RSS separately from fixture creation, checks complete 8/32 MiB inventories and sublinear growth, and checks pre-allocation admission for high-degree diagnostics. Memory/spill parity tests compare page facts, claims, relationships, traversal order/stop reasons, pointer maps, diagnostics, and schema attribution on valid and damaged inputs. |
| Responsive cancellation | Revision tests interrupt full-input verification; operational-budget tests cancel every structural work category at zero/one unit; large-inspection tests cancel during spill/index phases; deep tests cancel during payload reads and decoding and join workers. |
| Reproducible measurement | `benchmarks/run_large.py` generates separate fixtures, freezes the measured binary, and records geometry, budgets, complete coverage, elapsed/navigation time, VmHWM, private-file bytes, and source/binary hashes. |
| Evidence-based defaults and envelope | README large-file section records final measurements, headroom rationale, work-budget distinctions, temporary-space expansion, and full-input verification latency. |
| Practical regression profile | `cargo test --all-targets` passes 175 tests, including the process memory profile and 15 terminal tests. The separately invoked extended full-pointer-map test passes; its 13,107 entries remain queryable after inventory cancellation. Frontend typecheck/build and all 28 tests pass. Strict Clippy passes. |

Validation commands:

```sh
cargo test --all-targets
cargo test --release --test large_inspection full_large_pointer_map -- --ignored
cargo clippy --all-targets -- -D warnings
(cd frontend && npm test)
```

The ordinary Rust suite intentionally ignores the subprocess worker (the memory
regression invokes it explicitly) and the extended pointer-map case (run separately
above). All six final benchmark runs completed successfully. The README records
the measured results and default-setting rationale; this ticket is closed by the
implementation commit containing this audit.

Final benchmark audit: the sparse and overflow 2/4 GiB profiles and dense-indexed
8/16 MiB profiles all have complete inventory, topology, schema, and every recorded
work phase. First/last pages and real terminal navigation are available. All peaks
are below the 64 MiB ceiling; cache and private-file bytes stay within their limits.
Sparse peaks are 5/5 MiB, overflow peaks 6.25/5 MiB, and dense peaks 16.95/17.18 MiB.
The largest measured spill file is 580.45 MiB. Navigation and dense-space limits
are documented without extrapolating to arbitrary inputs.

Reports: `benchmarks/ticket14-sparse.json`, `benchmarks/ticket14-overflow.json`, and
`benchmarks/ticket14-dense-indexed.json`. All three record the same measured binary
hash and source SHA-256
`e3991f700f5a3b5b818ebefd803e9c7cb6452cf58c6cf5e5c0e92df14ba5b897`;
recomputing that hash from the current source files matches. The benchmark chain
exited successfully. Unrelated local tickets and the project spec are excluded
from the implementation commit.


## User acceptance — 2026-09-11

The user tried the Inspector and explicitly accepted v1: “I think it's nice. I accept.”
Previous tracker status: `done`. The existing implementation and validation record is retained above.
Closed as part of the accepted v1 effort; see [the acceptance record](../../../release/ACCEPTANCE.md).
