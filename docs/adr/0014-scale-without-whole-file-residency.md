# Scale without whole-file residency

Version 1 is designed for multi-gigabyte databases without loading the entire file or all decoded payloads into memory, using positional access, bounded caches, and spillable indexes where necessary. Concrete time and memory thresholds will be set from reproducible benchmarks rather than guessed upfront, accepting more explicit resource management so large-file support remains compatible with complete unsampled structural inspection.
