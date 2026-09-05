# Preserve partial evidence from damaged inputs

Malformed structure produces a best-effort partial inspection graph instead of failing the entire inspection. Traversals stop at explicit validation boundaries, retain their validated prefixes and byte-offset evidence, and report unknown pages or conflicting claims as diagnostics; only untrustworthy database geometry prevents broader inspection, trading implementation simplicity for useful and honest analysis of damaged files.
