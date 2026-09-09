# 10: Enrich semantic metadata safely

**What to build:** Optionally improve schema descriptions with a tightly constrained bundled-SQLite helper, while ensuring hostile declarations, sidecars, or helper failures can never change authoritative physical evidence or execute database-controlled behavior.

**Blocked by:** 07: Disclose sidecars without applying them; 08: Follow schema attribution in schema flow

**Status:** ready-for-human

- [x] The optional helper runs only as a private subcommand of the same executable and is not exposed as a general SQL interface.
- [x] The helper receives a private sidecar-free copy and executes only a fixed allowlisted set of read-only schema-metadata queries.
- [x] Application tables are never queried, and extensions, virtual-table modules, triggers, and application-defined functions are not loaded or executed.
- [x] Trusted-schema behavior is disabled and SQLite memory, SQL, recursion, operation, time, and output limits are set before untrusted schema processing.
- [x] Helper input and output use a bounded private protocol that cannot carry raw application payloads or absolute source paths.
- [x] Timeout, crash, malformed output, unsupported schema, resource exhaustion, and deliberate refusal are represented only as unavailable semantic metadata.
- [x] The inspection graph's physical facts, entity identities, relationships, diagnostics, selected stored values, and coverage are identical whether enrichment succeeds or fails.
- [x] Permitted enrichment is visibly descriptive and cannot erase directly parsed schema declarations, rootless states, or physical diagnostics.
- [x] Tests use hostile schema declarations and instrumented fixtures to prove that only allowlisted metadata queries execute and that all failure modes preserve the authoritative graph.


## Implementation

Optional `--semantic-metadata` enables same-executable enrichment before initial
publication. Descriptions are separate from physical evidence and include ordinary
table column counts, STRICT, and WITHOUT ROWID. Schemas with views or VIRTUAL
in declarations are deliberately refused before SQLite's table metadata pragma.

`tests/semantic_metadata.rs` uses the confirmed inspection-session and private
executable protocol seams. The test executable dispatches the real helper and
injects subprocess failures only in test code; the production binary is also
exercised directly. Tests compare complete physical graphs and selected values,
check traced fixed queries, hostile declarations, resource ceilings, bounded
protocol failures, source immutability, and main-file-only WAL behavior.

Review baseline: `5468846`. See README for the protocol and resource ceilings.

## Validation and review

- `cargo test --all-targets`: passed, including the existing sparse gigabyte-image tests.
- `cargo test --test semantic_metadata`: passed after the final budget fixes.
- `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all -- --check`: passed.
- `npm --prefix frontend test`: both test files and all 23 tests passed, including the rebuilt embedded asset.
- `npm --prefix frontend run typecheck`: passed.
- Production executable smoke check: optional enriched HTTP graph, embedded description label, no source-path or unselected-value disclosure, unchanged source bytes.
- Standards review against `5468846`: no remaining findings after adding configurable operational budgets and shared named audit constants.
- Spec review against `5468846`: no remaining findings after refusing incomplete direct schemas and zero-record budgets before helper launch. Regression tests first reproduced both failures.
