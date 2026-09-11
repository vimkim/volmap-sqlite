# 15: Contain hostile inputs and fuzz public boundaries

**What to build:** Harden the completed inspector against malformed or adversarial databases, sidecars, helper output, selectors, and requests so failures stay bounded, evidence-backed, and local to the correct validation boundary.

**Blocked by:** 06: Reconcile pointer-map, lock-byte, and page-role evidence; 07: Disclose sidecars without applying them; 09: Deep-inspect one selected cell into a new revision; 10: Enrich semantic metadata safely; 13: Budget and cancel inspection work

**Status:** done

- [x] A checked-in damage corpus covers truncated and contradictory geometry, malformed varints, impossible extents, overlapping cells, out-of-range links, cycles, duplicate claims, broken allocation structures, malformed sidecars, and unsupported record forms.
- [x] Corpus expectations assert the exact validation boundary, retained physical evidence, validated prefix, diagnostic occurrence, containment impact, and inspection coverage.
- [x] Readable reserved, encrypted-looking, compressed, or custom-VFS content outside supported standard SQLite structure is preserved as opaque evidence rather than speculatively decoded or automatically called corruption.
- [x] Fuzz targets cover every public parser boundary, traversal input, entity-selector decoder, helper protocol decoder, and bounded HTTP request shape.
- [x] Arbitrary inputs cannot cause a panic, memory unsafety, integer wraparound, uncontrolled allocation, unbounded recursion, runaway work after cancellation, or fabricated complete coverage.
- [x] Explicit-target disclosure properties verify that neither malformed inputs nor error formatting can surface raw application payload bytes or non-selected typed values.
- [x] Snapshot invalidation, immutable revision, path privacy, sidecar non-application, and physical-parser authority remain invariant under corpus and fuzz scenarios.
- [x] Fuzzing runs under finite deterministic resource ceilings, retains actionable reproducers, and has a practical continuous profile plus a documented extended profile.
- [x] The full damage corpus passes through the inspection-session seam and both adapter contract layers without relying on private implementation ordering.


## Implementation and validation

Implemented explicit opaque input/page extents, unsupported-format diagnostics and
a helper reply decoder that enforces its byte ceiling before parsing. Added a
30-case checked-in damage corpus, shared session/HTTP/terminal contract checks,
disclosure regressions and eight deterministic mutation targets with process
limits and retained exact-byte reproducers.

Validation: 180 ordinary Rust tests passed (two pre-existing large-profile tests
ignored), semantic helper integration passed, 28 frontend tests passed, and Clippy
is clean. Final continuous fuzzing passed 325 candidates. Extended fuzzing passed
8,197 candidates; after addressing the independent review's deep-selection
coverage finding, the corrected records target passed all 1,030 extended cases.
The final focused regression suites passed 33 tests.

Standards review: 0 findings. Spec review: 1 coverage finding addressed, 0 remaining.
The complete requirement audit and validation details are in `fuzz/VALIDATION.md`;
profile commands, budgets, boundary mapping and reproducer instructions are in
`fuzz/README.md`. Finite fuzz observations do not claim exhaustive input coverage.


## User acceptance — 2026-09-11

The user tried the Inspector and explicitly accepted v1: “I think it's nice. I accept.”
Previous tracker status: `done`. The existing implementation and validation record is retained above.
Closed as part of the accepted v1 effort; see [the acceptance record](../../../release/ACCEPTANCE.md).
