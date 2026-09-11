#!/usr/bin/env python3
"""Create small frozen demo inputs without replacing an existing example."""
from contextlib import closing
import fcntl
from pathlib import Path
import shutil
import sqlite3
import tempfile

ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "target/examples"


def catalog(destination):
    with closing(sqlite3.connect(destination / "database.sqlite")) as database, database:
        database.executescript("""
            PRAGMA page_size = 1024;
            PRAGMA auto_vacuum = INCREMENTAL;
            CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT NOT NULL);
            CREATE TABLE orders(id INTEGER PRIMARY KEY, customer_id INTEGER, total_cents INTEGER);
            CREATE INDEX orders_by_customer ON orders(customer_id);
            CREATE TABLE documents(id INTEGER PRIMARY KEY, title TEXT, body TEXT, attachment BLOB);
            CREATE TABLE tags(document_id INTEGER, tag TEXT, PRIMARY KEY(document_id, tag)) WITHOUT ROWID;
            CREATE VIEW customer_totals AS
                SELECT customer_id, sum(total_cents) AS total_cents FROM orders GROUP BY customer_id;
            INSERT INTO customers VALUES (1, 'Ada'), (2, 'Mina'), (3, 'Sam');
            CREATE TABLE scratch(payload BLOB);
            INSERT INTO scratch VALUES(zeroblob(65536));
            DROP TABLE scratch;
        """)
        database.executemany("INSERT INTO orders VALUES (?, ?, ?)",
                             [(number, number % 3 + 1, 1000 + number * 75) for number in range(1, 37)])
        database.execute("INSERT INTO documents VALUES (1, ?, ?, ?)",
                         ("Overflow demonstration", "This selected text spans overflow pages. " * 256, bytes(range(256))))
        database.executemany("INSERT INTO tags VALUES (1, ?)", [("storage",), ("example",)])


def wal(destination):
    # The only writer is this connection. Copy after commit, before its close
    # checkpoints the original. Never open the frozen destination with SQLite.
    with tempfile.TemporaryDirectory(dir=OUTPUT) as scratch:
        source = Path(scratch) / "live.sqlite"
        with closing(sqlite3.connect(source)) as database, database:
            database.executescript("""
                PRAGMA page_size = 1024;
                PRAGMA journal_mode = WAL;
                PRAGMA wal_autocheckpoint = 0;
                CREATE TABLE notes(message TEXT);
                INSERT INTO notes VALUES ('Stored in the main-file image');
                PRAGMA wal_checkpoint(TRUNCATE);
                INSERT INTO notes VALUES ('Present only in the WAL logical image');
            """)
            database.commit()
            shutil.copyfile(source, destination / "database.sqlite")
            shutil.copyfile(Path(str(source) + "-wal"), destination / "database.sqlite-wal")


def damaged(destination):
    path = destination / "database.sqlite"
    with closing(sqlite3.connect(path)) as database, database:
        database.executescript("""
            PRAGMA page_size = 512;
            CREATE TABLE notes(message TEXT);
            INSERT INTO notes VALUES ('Damaged cell'), ('Independent sibling cell');
        """)
    # Page 2's first cell pointer targets its own header; its sibling stays valid.
    with path.open("r+b") as file:
        file.seek(512 + 8)
        file.write(bytes([0, 1]))


def main():
    OUTPUT.mkdir(parents=True, exist_ok=True)
    # Serialize generators so a second invocation cannot publish a partial demo.
    with (OUTPUT / ".generate.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        for name, create in [("catalog", catalog), ("wal", wal), ("damaged", damaged)]:
            destination = OUTPUT / name
            if not destination.exists():
                with tempfile.TemporaryDirectory(dir=OUTPUT) as scratch:
                    staging = Path(scratch) / name
                    staging.mkdir()
                    create(staging)
                    staging.rename(destination)
            if not (destination / "database.sqlite").is_file():
                raise SystemExit(f"Incomplete existing demo: {destination}")
            print(f"{name}: {destination / 'database.sqlite'}")
        print("Existing examples are preserved. See examples/README.md for guided steps.")


if __name__ == "__main__":
    main()
