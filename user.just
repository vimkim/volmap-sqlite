set positional-arguments

# Build the local executable and its embedded browser assets.
build:
    npm --prefix frontend ci --no-audit --no-fund
    npm --prefix frontend run build
    cargo build --locked --target-dir target

# Serve on all interfaces at port 3000; --listen overrides the address.
web database *args: build
    #!/usr/bin/env bash
    set -euo pipefail
    for argument in "$@"; do
        case "$argument" in
            --listen|--listen=*) exec ./target/debug/volmap-sqlite "$@" ;;
        esac
    done
    exec ./target/debug/volmap-sqlite --listen 0.0.0.0:3000 "$@"

# Open your frozen database in the terminal; q exits.
terminal database *args: build
    ./target/debug/volmap-sqlite --terminal "$@"

# Print the local executable's version and content identity.
version: build
    ./target/debug/volmap-sqlite --version

# Build the pinned release executable under target/v1-release.
release:
    python3 release/build.py
