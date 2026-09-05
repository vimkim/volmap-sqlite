# Make resource budgets explicit

Every potentially unbounded scan, traversal, reconstruction, and decode runs under explicit configurable resource budgets and is cancellable. Reaching a budget records partial inspection coverage with the exact stopping boundary and reason rather than silently sampling, truncating, or claiming completeness, accepting a richer result model in exchange for predictable behavior on huge or hostile files.
