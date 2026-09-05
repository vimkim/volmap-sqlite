# Publish immutable inspection revisions

The product reports deterministic fast-scan progress and supports cancellation, but publishes a navigable graph only after the scan completes or stops at an explicit recorded boundary. Selected deep inspections run asynchronously and publish new immutable revisions within the same database snapshot while preserving all existing entity identities, trading in-place UI mutation for coherent URLs, reproducible results, and honest partial coverage.
