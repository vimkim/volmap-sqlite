# Ship one Rust binary with an embedded web viewer

Volmap SQLite Inspector uses a Rust 2024 core with unsafe code forbidden, a Crossterm terminal interface, an Axum and Tokio HTTP adapter, and a React, TypeScript, and Vite browser viewer embedded into one distributable `volmap-sqlite` binary. Dependencies and frontend tools are pinned, and the optional bundled-SQLite metadata helper runs as a private subcommand of the same executable, accepting frontend build complexity in exchange for one reproducible artifact and architectural continuity with the CUBRID sibling.
