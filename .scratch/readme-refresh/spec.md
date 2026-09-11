# README refresh

Status: done

Approved: user confirmed developers exploring SQLite internals as the audience and real local catalog screenshots of Page atlas and Schema flow.

Preserve the existing technical README under docs/usage.md, repairing relocated links and inbound detailed-guide links. Write a friendly root README explaining the purpose, quick start, real screenshots, first navigation steps, essential input limitations, and links to detailed usage/examples/build documentation. Preserve existing domain terminology; no new domain definitions or ADRs are needed. No application behavior changes. Verify instructions, links, images and existing suites, review Standards and Spec against baseline 89bb11f1b77610119a86387f3cc1212879d835f1, and commit to the current branch.

## Verification

- Original technical README preserved exactly except relocated relative links; 21 relative links and anchors across README.md, docs/usage.md, and release/README.md resolve.
- Frontend build/typecheck and cargo build --locked passed; demo generator and real browser startup passed.
- cargo test --locked --all-targets --all-features: 181 passed, 2 ignored, 0 failed.
- npm --prefix frontend test: 29 tests passed across 2 files (includes build/typecheck).
- Chromium exercised Page atlas, orders Schema flow, and documents selected-cell deep inspection without browser errors. Terminal demo launched and exited with q.
- Real screenshots captured from the catalog example at 1600x1100, scale 1, using Playwright element screenshots of .content-grid after waiting for selected-page evidence (page 3 for atlas, page 4 for orders). Images were visually inspected.
- Standards and Spec reviews against the recorded baseline found no issues. No new domain terms or architectural decisions were introduced.
