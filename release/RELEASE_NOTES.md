# Volmap SQLite Inspector — accepted v1 effort

Package version: **0.1.0**. Platform: Linux x86-64 with glibc 2.34 or newer.
Accepted on 2026-09-11; source baseline `ee5e94c`.

One executable provides physical page inspection, a browser page atlas and schema
flow, a focused terminal interface, and bounded selected-cell deep inspection.
It embeds its browser assets and bundled SQLite for private internal work. No
Node.js, SQLite command-line tool, or separate application server is required.

## Run

After extracting the archive:

```sh
sha256sum -c SHA256SUMS
./volmap-sqlite --version
./volmap-sqlite /path/to/frozen.sqlite
./volmap-sqlite /path/to/frozen.sqlite --terminal
```

For a browser on another machine:

```sh
./volmap-sqlite /path/to/frozen.sqlite --listen 0.0.0.0:3000
```

Replace `127.0.0.1` in the printed URL with the server's IP or hostname, retaining
port 3000 and the complete `/sessions/...` path. The executable defaults to loopback;
the repository's `just user web` and `just example web` conveniences explicitly
select all interfaces. Remote serving emits the existing warning: the Inspector
has no built-in authentication or TLS and relies on operator-controlled network
protection. Ctrl-C stops serving; `q` exits the terminal.

Add `--semantic-metadata` for optional bounded descriptive table enrichment.
The helper is private and does not provide a SQL console.

## Interpretation

Use a frozen main file and stable adjacent sidecars. The main-file image can differ
from SQLite's logical database image. WAL and rollback-journal bytes are reported,
never applied. v1 neither repairs nor recovers databases.

Broad navigation shows structural evidence. Explicit deep inspection may disclose
the selected cell's stored values; BLOBs show type and byte length, not raw bytes.
Column names remain unavailable for those deep values. Budgets and malformed
boundaries can stop inspection with explicit partial coverage. Input changes
invalidate the session. Large-file navigation rechecks frozen contents, so bounded
memory does not imply instant navigation.

## Verification

The bundled manifest and verification receipt identify the executable and toolchain.
Two independent clean builds produced identical executable bytes. Validation passed
181 Rust tests, separate semantic-helper checks, 29 frontend tests, formatting,
Clippy, real browser and terminal smoke, network/disclosure checks, and a sparse
2 GiB input under a 64 MiB resident ceiling and 1 MiB cache. Two existing test entries
remain ignored by the normal suite (a subprocess worker used by its parent test
and an opt-in 820 MiB pointer-map profile).

Executable SHA-256:
`ee311a34fa99422e48c03b78ab63a5a5d3208941c7b60927aa0d04afe2faa02c`.
