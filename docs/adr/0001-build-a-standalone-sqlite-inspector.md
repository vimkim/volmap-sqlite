# Build a standalone SQLite inspector

Build the SQLite inspector as a standalone sibling of the CUBRID Volmap project. Reuse Volmap's product concepts—read-only inspection, a shared inspection graph, evidence-backed diagnostics, and presentation adapters—but do not create a shared multi-engine codebase before the two storage domains reveal stable common abstractions; this avoids coupling SQLite's native model to CUBRID-specific assumptions while preserving the option to extract proven common code later.
