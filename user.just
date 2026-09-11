set positional-arguments

# Build the local executable and its embedded browser assets.
build:
    npm --prefix frontend ci --no-audit --no-fund
    npm --prefix frontend run build
    cargo build --locked --target-dir target

# Open your frozen database in the browser; optional arguments go to the Inspector.
web database *args: build
    ./target/debug/volmap-sqlite "$@"

# Open your frozen database in the terminal; q exits.
terminal database *args: build
    ./target/debug/volmap-sqlite --terminal "$@"

# Print the local executable's version and content identity.
version: build
    ./target/debug/volmap-sqlite --version

# Build the pinned release executable under target/v1-release.
release:
    python3 release/build.py
