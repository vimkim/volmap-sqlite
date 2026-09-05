# Require explicit-target disclosure

Structural facts, schema names, byte extents, and SQLite serial types may be shown throughout the product, but typed application values are decoded and displayed only for a cell explicitly selected by the operator, and raw application payload bytes are never emitted as a fallback. This preserves the usefulness of structural inspection while limiting accidental disclosure through summaries, diagnostics, exports, URLs, or broad scans.
