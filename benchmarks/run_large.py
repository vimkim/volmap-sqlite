"""Generate fixtures separately, then measure the optimized inspection process."""
import argparse
import datetime
import hashlib
import json
import pathlib
import sqlite3
import shutil
import subprocess
import tempfile


def fixture(path, size_mib, topology):
    database = sqlite3.connect(path)
    database.execute("PRAGMA page_size=65536")
    if topology == "dense-indexed":
        database.execute("PRAGMA auto_vacuum=FULL")
    database.execute("CREATE TABLE entries(value)")
    if topology == "overflow":
        for _ in range(max(1, size_mib // 8)):
            database.execute("INSERT INTO entries VALUES(zeroblob(?))", (8 * 1024 * 1024,))
        database.commit()
    if topology == "dense-indexed":
        database.execute("CREATE INDEX entries_value ON entries(value)")
        inserted = 0
        while database.execute("PRAGMA page_count").fetchone()[0] * 65536 < size_mib * 1024 * 1024:
            database.executemany(
                "INSERT INTO entries VALUES(?)",
                ((value,) for value in range(inserted, inserted + 8192)),
            )
            database.commit()
            inserted += 8192
    database.close()
    if topology == "sparse":
        with path.open("r+b") as image:
            image.truncate(size_mib * 1024 * 1024)
            image.seek(28)
            image.write((size_mib * 16).to_bytes(4, "big"))


def source_fingerprint(root):
    source_files = sorted({
        *root.glob("Cargo.*"), *root.glob("src/**/*.rs"),
        *root.glob("examples/*.rs"), *root.glob("benchmarks/*.rs"),
        pathlib.Path(__file__).resolve(), *root.glob("frontend/dist/**/*"),
    })
    source_hash = hashlib.sha256()
    source_files = [source for source in source_files if source.is_file()]
    for source in source_files:
        source_hash.update(str(source.relative_to(root)).encode() + b"\0")
        source_hash.update(source.read_bytes() + b"\0")
    return source_hash.hexdigest(), [str(source.relative_to(root)) for source in source_files]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sizes-mib", type=int, nargs="+", default=[2048, 4096])
    parser.add_argument("--topology", choices=["sparse", "overflow", "dense-indexed"], default="sparse")
    parser.add_argument("--resident-mib", type=int, default=64)
    parser.add_argument("--cache-kib", type=int, default=1024)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    options = parser.parse_args()
    if min(options.sizes_mib) < 1 or options.resident_mib < 1 or options.cache_kib < 0:
        parser.error("sizes and resident memory must be positive; cache must be nonnegative")
    root = pathlib.Path(__file__).resolve().parents[1]
    source_hash, source_files = source_fingerprint(root)
    subprocess.run(["cargo", "build", "--release", "--example", "large_probe"], cwd=root, check=True)
    if source_fingerprint(root)[0] != source_hash:
        raise RuntimeError("Source changed during compilation; rerun after edits settle.")
    report = {
        "sourceSha256": source_hash,
        "sourceFiles": source_files,
        "sourceCommit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip(),
        "workingTreeModified": bool(subprocess.check_output(["git", "diff", "--name-only"], cwd=root)),
        "recordedAt": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "profile": "release", "topology": options.topology,
        "method": "Fixture creation and inspection run in separate processes; RSS is inspector VmHWM.",
        "runs": [],
    }
    with tempfile.TemporaryDirectory(prefix="volmap-benchmark-") as directory:
        binary = pathlib.Path(directory) / "large_probe"
        shutil.copy2(root / "target/release/examples/large_probe", binary)
        report["binarySha256"] = hashlib.sha256(binary.read_bytes()).hexdigest()
        for size in options.sizes_mib:
            path = pathlib.Path(directory) / "generated.sqlite"
            fixture(path, size, options.topology)
            result = subprocess.run([
                str(binary), str(path),
                str(options.resident_mib * 1024 * 1024), str(options.cache_kib * 1024),
            ], capture_output=True, text=True)
            if result.returncode:
                raise RuntimeError(f"Inspector exited {result.returncode}: {result.stderr.strip()}")
            measurement = json.loads(result.stdout)
            report["runs"].append(measurement)
            print(json.dumps(measurement), flush=True)
            options.output.parent.mkdir(parents=True, exist_ok=True)
            options.output.write_text(json.dumps(report, indent=2) + "\n")
            path.unlink()
    return 0 if all(
        run["summary"]["coverage"]["reason"] == "complete"
        and run["summary"]["topologyCoverage"]["reason"] == "complete"
        and run["firstPage"] is not None and run["lastPage"] is not None
        and run["terminalNavigation"]["available"]
        for run in report["runs"]
    ) else 1


if __name__ == "__main__":
    raise SystemExit(main())
