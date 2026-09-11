# Reproducible Inspector artifact

The v1 effort ships one Linux x86-64 executable. The package version remains
`0.1.0`; `--version` also reports a content build identity, Rust compiler, and target.
The binary embeds the browser assets and SQLite used by private storage and the
isolated metadata helper. It requires ordinary Linux system libraries (glibc 2.34
or newer), with no application runtime installation. The helper is an internal
protocol, not a SQL execution interface.

## Pinned build

Install the exact tool versions in [toolchain.json](toolchain.json), including
`/usr/bin/gcc`, `/usr/bin/ld`, and `/usr/bin/ar`. Rustup uses the repository's
[rust-toolchain.toml](../rust-toolchain.toml); Node and npm versions are also enforced
by the frontend package metadata. Python 3 runs the release scripts. Python, npm,
Node, Rust, and the C toolchain are build/test prerequisites only.

```sh
python3 release/build.py --output target/v1-release
cat target/v1-release/build-info.json
(cd target/v1-release && sha256sum -c SHA256SUMS)
target/v1-release/volmap-sqlite --version
```

The builder checks tool versions, runs `npm ci` and regenerates the embedded
assets, refuses any difference from the checked-in assets, then builds with
`cargo build --release --locked`. Cargo.lock pins transitive Rust dependencies;
package-lock.json pins frontend dependencies and their integrity hashes. SQLite
is compiled from the locked bundled source. Build flags fix the system linker,
disable linker build IDs, strip debug information, and remap source/cache paths.
The manifest records tool versions and executable hashes, asset hashes, and the
final executable SHA-256. Changing dependency versions is an intentional source
change followed by asset regeneration and verification.

The content identity hashes production Rust source, embedded assets, dependency
manifests and locks, release build recipe/tool pins, compiler, target, and profile.
It excludes Git state, time, and local path names. It is a source/build identity;
`SHA256SUMS` identifies the actual executable bytes. This is reproducibility within
the declared Linux toolchain, not a claim that arbitrary operating systems,
compilers, environment overrides, or library implementations produce identical
binaries. Dependency caches may be shared, but the verifier never shares Cargo
build output between its two builds. Initial dependency installation needs registry
access; the running Inspector never needs network access beyond its local UI.

## Clean verification

Install Linux `strace`, Python 3 with its standard-library SQLite module, and a
Chromium headless-shell executable. Use headless-shell for the strict network-log
check: full desktop Chrome can issue its own account/update traffic. The browser harness uses pinned `playwright-core`; it does not
download a browser. The browser version is recorded in the verification report.
Run from a committed tree, or pass an immutable Git tree prepared for review:

```sh
python3 release/verify.py --tree HEAD --chromium /path/to/chrome-headless-shell
python3 release/verify.py --tree HEAD --chromium /path/to/chrome-headless-shell --extended
```

Each run exports the requested tree into two independent clean directories,
rebuilds frontend assets and the release executable twice, and requires identical
binary hashes and build manifests. On the first clean tree it runs formatting,
Clippy, all production Rust targets/features, frontend typechecking/tests, and
black-box smoke tests against a copied release executable. No working-directory
files or untracked fixtures can satisfy those clean builds. Successful output is
copied to `target/verified-v1/`, together with `verification.json`. The verifier
cleans temporary checkouts and does not commit or publish anything.

| Profile | Required scope |
| --- | --- |
| Continuous (default) | Real frozen fixture; fast immutable revision; page atlas and schema flow in Chromium; selected-cell deep values; TUI navigation/deep inspection; private metadata helper; all 30 checked-in damaged main files; malformed adjacent sidecars; invalidation; explicit budget stop; production HTTP/CSP/disclosure/path checks; syscall tracing for outbound connections; two clean builds |
| Extended | All continuous checks, plus a 2 GiB sparse image inspected under a 64 MiB resident ceiling and 1 MiB cache, complete inventory/topology/schema, and first/last-page navigation |

The browser/helper and terminal smoke processes use a runtime directory containing
only the copied binary and a PATH with no tools. The security harness verifies
loopback defaults, explicit remote warning, embedded assets, selector-scoped value
disclosure, request limits, and absence of outbound server connections. The real
browser checks successful CSP boot, same-origin requests, and exclusion of
unselected row values and BLOB contents. The separate damage/fuzz and large-file
benchmarks provide deeper topology coverage; see [fuzzing](../fuzz/README.md) and
[large-file measurements](../docs/usage.md#large-file-inspection-and-private-storage).
A sparse 2 GiB release smoke is not a promise that all dense 2 GiB inputs fit.

To rerun only packaged behavior after a local change:

```sh
python3 release/smoke.py target/v1-release/volmap-sqlite --chromium /path/to/chrome-headless-shell
```

## Use

Copy `volmap-sqlite` to the destination and verify its checksum. Run it against a
**frozen main-file image**, with stable adjacent sidecars when present. The browser
is a page atlas and schema flow; `--terminal` opens the focused terminal Inspector.
`--semantic-metadata` enables bounded enrichment through the same executable's
private helper. There is no development prototype selector or built-in demo data.

WAL and rollback-journal bytes are reported, never applied. The main-file image is
therefore not necessarily SQLite's logical database image. v1 does not repair or
recover databases. Consult the detailed [usage guide](../docs/usage.md) for operational
budgets, disclosure boundaries, and explicit remote-serving options.
