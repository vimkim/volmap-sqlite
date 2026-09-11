# Try the Inspector

These are development conveniences, tested with `just 1.57.0`. Install the pinned
Rust/Node/npm toolchain described in [release/README.md](../release/README.md), plus
Python 3 with SQLite support. Demo generation uses Python's SQLite module; the
Inspector itself needs no SQLite command-line tool or JavaScript runtime.

From the repository, run:

```sh
just                         # list commands, including both modules
just example web             # generate demos, build, print the browser URL
just example terminal        # the same catalog in the terminal
```

Open the printed **Page atlas** URL in your browser. Stop the web server with
Ctrl-C before starting another command on its default port. In the terminal,
`q` exits. Both launch commands rebuild the local executable and embedded assets.
They keep serving until you stop them; they do not automatically open a browser.

| Demo | Command | What to inspect |
| --- | --- | --- |
| Catalog | `just example web` | Schema flow links customers, orders and its index, documents, a WITHOUT ROWID tags table, and a declaration-only view. Page atlas also has pointer-map and freelist pages. |
| WAL | `just example web wal` | Sidecar reporting shows a valid WAL. The notes table's main-file image contains one cell; SQLite's logical image would contain two. WAL bytes are never applied by the Inspector. |
| Damaged | `just example web damaged` | Page 2 has one cell pointer aimed inside its header. Inspect the diagnostic and the independent sibling cell that remains readable. |

`just example terminal wal` and `just example terminal damaged` open the other
inputs in the terminal. `just example guide` prints this guide.

For a selected-value demo, use **Schema flow → documents → Root B-tree page**,
select its cell, then choose **Deep-inspect selected cell**. Its long text follows
validated overflow pages; its BLOB shows only type and byte length. Broad page and
schema navigation does not disclose those application values. Browser demos enable
optional semantic table metadata; deep-value column names remain unavailable.

To inspect your own frozen file, quote paths containing spaces. Extra arguments
are passed directly to the Inspector:

```sh
just user web '/path/to/frozen database.sqlite'
just user web '/path/to/frozen database.sqlite' --listen 127.0.0.1:0
just user terminal '/path/to/frozen database.sqlite'
just user version
just user release            # pinned artifact under target/v1-release/
```

The files live under ignored `target/examples/{catalog,wal,damaged}/database.sqlite`.
The generator publishes complete directories and reuses existing examples, so
starting another demo does not rewrite files being inspected. If you want fresh
examples, first stop sessions using them, remove their generated directory under
`target/examples/`, then run `just example generate`. Avoid opening the WAL example
with SQLite itself: that can checkpoint or change its sidecars. Copy the entire
example directory first if you want to compare logical SQLite behavior.

These fixtures and helper commands are not embedded in the shipped executable.
All inputs are frozen main-file images with optional adjacent sidecars. The
Inspector does not repair or recover the damaged example or any other database.
