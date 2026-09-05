# Split fast and deep inspection

Inspection is divided into a complete, unsampled structural pass and bounded opt-in deep inspection of selected cells. The fast pass establishes page inventory, topology, allocation structures, schema attribution, and cell boundaries, while payload reconstruction, overflow traversal, and typed value decoding occur only for explicit targets; this keeps whole-file navigation honest and scalable without weakening the disclosure boundary.
