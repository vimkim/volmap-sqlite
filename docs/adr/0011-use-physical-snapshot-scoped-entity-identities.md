# Use physical snapshot-scoped entity identities

Inspection entities are identified within one database snapshot using physical identities wherever possible: a page by its one-based page number and a cell by page number plus physical cell index. Schema names, rowids, keys, and decoded values remain interpreted facts, while selectors and web routes merely resolve to typed identities; this keeps links stable across semantic enrichment and avoids conflating mutable or corrupt logical values with physical storage identity.
