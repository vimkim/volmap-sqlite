# Ticket 15 validation

Validated on Linux on 2026-09-11 with Rust 1.97.1, Python 3.14.3 and Node 25.8.0.
Review baseline: `c2f139c41783f947afdf31fcfb2a141c38920c08`.
The implementation commit containing this report includes the tested corpus,
runner, production changes and rebuilt embedded browser asset.

| Check | Result |
| --- | --- |
| `cargo check --all-targets` | Passed |
| `cargo clippy --all-targets` | Passed without warnings |
| `cargo test` | 180 passed, 2 pre-existing large-profile tests ignored; semantic helper integration harness also passed |
| `(cd frontend && npm test)` | TypeScript build and embedded asset build passed; 28 tests passed |
| `cargo test --test hostile_inputs --test btree_pages --test terminal_flow` after review refactors | 33 passed |
| `python3 fuzz/run.py` after review correction | 325 candidates passed, eight targets, seed 15 |
| `python3 fuzz/run.py --profile extended` | 8,197 candidates passed, eight targets, seed 15 |
| `python3 fuzz/run.py --target records --replay tests/corpus/damage/valid-cells.sqlite` after correction | Passed; asserts two successful reconstructions plus cancelled runs |
| `python3 fuzz/run.py --profile extended --target records` after correction | 1,030 candidates passed, seed 15 |
| Corpus regeneration and staged whitespace check | Passed |

The complete extended campaign preceded the review correction to make normal and
cancelled deep selections deterministic. The corrected records target was then
re-run with its complete extended profile; the complete continuous profile was
also re-run. No inspector panic, timeout, memory-limit termination or invariant
failure remained. These are finite campaign observations, not an exhaustive proof
about all possible byte strings.

## Requirement evidence

| Ticket requirement | Evidence |
| --- | --- |
| Checked-in damage corpus | 30 main-file cases and seven sidecar files in `tests/corpus/damage/`; independently constructed `generate.py` and literal `manifest.json` expectations |
| Exact containment and retained evidence | Manifest checks physical coordinates, identities, local diagnostics, excluded claims, ordered traversal prefixes, sidecar commit prefix and per-scope coverage; shared session/adapter checks verify inventory arithmetic and valid coordinates |
| Opaque unsupported content | Explicit opaque input extent on geometry failure; `unsupported_format` diagnostic for nonstandard magic; `opaque_content` and `opaque_reserved` page regions; encrypted-looking, compressed, custom-VFS, short and reserved fixtures |
| Public-boundary fuzz targets | Eight targets and their production boundaries enumerated in `README.md`; valid structures seed deep parser entry points |
| Resource safety and cancellation | Safe Rust requirement, bounded session/deep budgets, checked protocol length, process ceilings/watchdog, cancellation/invalidation assertions; existing operational-budget and deep-inspection regressions included in full suite |
| Explicit-target disclosure | Separate selected/unselected text and BLOB markers with a damaged sibling; exact-selector result lookup, broad HTTP/revision/metadata and terminal checks; malformed JSON error sanitization |
| Snapshot/revision/privacy/authority invariants | Every navigable corpus case checks immutable retained revision plus HTTP/terminal invalidation; sidecar-free physical comparison; fuzz helper output injected through real enrichment subprocess without changing physical facts |
| Finite profiles and reproducers | Continuous/extended iteration ceilings; per-candidate process limits; bytes, SHA-256, invocation metadata and failure output retained before execution; exact-byte replay exercised |
| Both adapter contracts | Every corpus case executes HTTP status/revision projection and terminal flow, including every retained page and cell, alongside the inspection-session oracle |

## Standards

Final independent review: **0 findings**. The production refactors preserve
behavior and domain vocabulary. The record target remains explicitly bounded.
The rebuilt embedded browser asset matches the opaque-input display in `App.tsx`.

## Spec

One medium coverage finding was identified: the positive record seed always took
the cancellation path and the target selected only its first cell. It was fixed
by exercising each of the first eight cells (all cells in the small corpus cases)
with both normal and cancelled execution, using the current revision. The valid
control must now complete two successful decodes. Exact replay, continuous and
extended record campaigns pass. Independent re-review confirmed the finding is
resolved, with **0 remaining findings** and no demonstrated production defect or
scope creep.

Standards: 0 findings. Spec: 1 addressed, 0 remaining; no unresolved issue in either axis.
