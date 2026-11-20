#!/usr/bin/env python3
"""Prepare the clean-room comparison fixtures under the ignored shared corpus."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import zipfile


DOWNLOADS = {
    "jquery-3.7.1.js": (
        "https://code.jquery.com/jquery-3.7.1.js",
        "78a85aca2f0b110c29e0d2b137e09f0a1fb7a8e554b499f740d6744dc8962cfe",
        "MIT",
    ),
    "jquery-2.1.4.js": (
        "https://code.jquery.com/jquery-2.1.4.js",
        "b2215cce5830e2350b9d420271d9bd82340f664c3f60f0ea850f7e9c0392704e",
        "MIT",
    ),
    "sqlite3.c": (
        "https://www.sqlite.org/2014/sqlite-amalgamation-3080701.zip",
        "8cabe6cebe3a01dedc8028f43ee069504f963e4f2fa3e69ea23075e095a8c016",
        "public domain (sqlite-amalgamation-3080701/sqlite3.c)",
    ),
    "gcc.c": (
        "https://raw.githubusercontent.com/peeknext/keypress/main/test_files/gcc.c",
        "d4593beff9c0211c8c6cc7712f1c68e2b7e33e991890e7728c8ad4a2e0c9d309",
        "Peeknext typing fixture; upstream licensing notices retained",
    ),
}


def digest(path):
    with path.open("rb") as data:
        sha = hashlib.sha256()
        for chunk in iter(lambda: data.read(1 << 20), b""):
            sha.update(chunk)
        return sha.hexdigest()


def fixture(dest, staging, name, source, write, expected=None, note=None):
    path = dest / name
    candidate = staging / name
    if expected is None or not path.is_file() or digest(path) != expected:
        write(candidate)
        if expected is not None and digest(candidate) != expected:
            raise ValueError(f"{name}: source does not match pinned SHA256; not installed")
        os.replace(candidate, path)
    return {"name": name, "source": source, "note": note,
            "bytes": path.stat().st_size, "sha256": digest(path)}


def download(url, dest):
    archive = dest.with_suffix(".zip") if dest.name == "sqlite3.c" else dest
    subprocess.run(["/usr/bin/curl", "-fLsS", "--retry", "2", "--max-time",
                    "90", "-o", archive, url], check=True)
    if archive != dest:
        with zipfile.ZipFile(archive) as z:
            dest.write_bytes(z.read("sqlite-amalgamation-3080701/sqlite3.c"))


def generated_c(dest, lines):
    with dest.open("wb") as output:
        for start in range(0, lines, 10_000):
            output.write("".join(
                f"int data_{i:07d} = 0x{i:06x};\n"
                for i in range(start, min(start + 10_000, lines))
            ).encode("ascii"))


def logs(dest):
    line = b"2026-11-20T00:00:00Z INFO request complete duration_ms=1\n"
    block = line * (1_000_000 // len(line))
    with dest.open("wb") as output:
        blocks, last = divmod(250_000_000, len(block))
        for _ in range(blocks):
            output.write(block)
        output.write(block[:last])


def main():
    repo = Path(__file__).resolve().parent.parent
    dest = repo / "spikes/data/sublime-comparison"
    dest.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=dest) as temp:
        staging = Path(temp)
        manifest = [fixture(dest, staging, name, url,
                            lambda p: download(url, p), sha, note)
                    for name, (url, sha, note) in DOWNLOADS.items()]
        for n in (1, 5):
            manifest.append(fixture(
                dest, staging, f"generated-{n}m.c", "scripts/prepare-perf-corpus.py",
                lambda p: generated_c(p, n * 1_000_000),
                note="29 bytes/record; Peeknext size and file type, not their generator"))
        for size in (10_000, 100_000, 1_000_000, 50_000_000):
            manifest.append(fixture(
                dest, staging, f"line-{size}.txt", "scripts/prepare-perf-corpus.py",
                lambda p: p.write_bytes((b"abc 0123 " * (size // 9 + 1))[:size]),
                note="single ASCII line, no LF"))
        manifest.append(fixture(dest, staging, "log-250mb.txt",
                                "scripts/prepare-perf-corpus.py", logs,
                                note="synthetic plain log; not the MonoVM fixture"))
        output = staging / "manifest.json"
        output.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        os.replace(output, dest / output.name)
    print(dest / "manifest.json")


if __name__ == "__main__":
    main()
