# 04: Navigate B-tree topology and overflow claims

**What to build:** Let an operator follow physical relationship claims from B-tree roots through interior children and from payload-bearing cells through overflow pages, while malformed or conflicting links remain visible as bounded diagnostic evidence.

**Blocked by:** 03: Inspect B-tree pages and structural cells

**Status:** done

- [x] Interior-page child pointers and right-most pointers become source-backed relationship claims with byte coordinates and named validation rules.
- [x] Valid B-tree parent-child claims resolve to typed page identities and are navigable in both directions without replacing physical identity with decoded key content.
- [x] Structurally present first-overflow pointers and overflow next-page pointers become source-backed claims without reconstructing or disclosing broad payloads.
- [x] Traversals retain the validated prefix before the first missing, out-of-range, invalid, cyclic, conflicting, or budget-stopped link.
- [x] Missing targets remain visible as unresolved intended page identities rather than disappearing or creating fabricated page entities.
- [x] Cycles, duplicate parents where disallowed, type mismatches, overlapping extents, and incompatible claims produce stable diagnostics with severity, evidence, affected relationships, and containment impact.
- [x] The atlas and page-detail view allow operators to follow validated child and overflow relationships and jump from a relevant diagnostic to its physical locus.
- [x] Damage in one subtree or overflow chain does not hide independently validated parent facts, sibling subtrees, other cells, or unrelated pages.
- [x] Valid, broken, cyclic, conflicting, and truncated relationship fixtures verify graph behavior through the inspection-session seam rather than private traversal order.



## User acceptance — 2026-09-11

The user tried the Inspector and explicitly accepted v1: “I think it's nice. I accept.”
Previous tracker status: `ready-for-agent`. Implementation commits: `e1d3c5d`. Regression evidence: `tests/topology.rs` and the ticket 16 clean production verification.
Closed as part of the accepted v1 effort; see [the acceptance record](../../../release/ACCEPTANCE.md).
