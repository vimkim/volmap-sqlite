# 03: Inspect B-tree pages and structural cells

**What to build:** Let an operator select a table or index B-tree page in the atlas and inspect its physical layout and cells—including page 1's special geometry—without decoding or exposing broad application values.

**Blocked by:** 02: Publish coherent frozen inspection revisions

**Status:** done

- [x] Direct parsing recognizes supported table and index interior and leaf B-tree page headers only after their prerequisite bounds and format checks succeed.
- [x] Page 1 correctly separates the database header from its B-tree header and reports both in their proper byte-coordinate spaces.
- [x] Page detail reports B-tree header fields, cell-pointer array, unallocated region, freeblocks and fragments when supported, cell-content area, usable space, and opaque reserved region.
- [x] Every structurally available cell has an identity composed of containing page number and physical cell index, independent of rowid, key, schema name, or decoded value.
- [x] Cell detail reports validated boundaries, structural size, serial types, and structurally available rowids or keys without treating interpreted values as identities.
- [x] Malformed headers, pointers, varints, extents, freeblocks, and overlapping cells stop dependent interpretation at explicit validation boundaries while retaining independently valid page evidence.
- [x] The page atlas marks supported B-tree roles and opens a selection-linked structural byte map and cell inventory for the active page.
- [x] Broad graph and page-detail responses contain no decoded application values or raw application payload bytes.
- [x] Fixtures cover all four B-tree page kinds, multiple supported page sizes and encodings, page 1, reserved regions, malformed cell pointers, overlapping cells, and truncated records through the inspection-session seam.



## User acceptance — 2026-09-11

The user tried the Inspector and explicitly accepted v1: “I think it's nice. I accept.”
Previous tracker status: `ready-for-agent`. Implementation commits: `44fb032, 9943df5`. Regression evidence: `tests/btree_pages.rs` and the ticket 16 clean production verification.
Closed as part of the accepted v1 effort; see [the acceptance record](../../../release/ACCEPTANCE.md).
