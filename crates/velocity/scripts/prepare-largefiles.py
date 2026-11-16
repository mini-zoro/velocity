#!/usr/bin/env python3
"""Prepare exact decimal-size fixtures outside measurement; the log contains real ASCII bytes."""
import argparse
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    args = parser.parse_args()
    args.directory.mkdir(parents=True, exist_ok=True)
    log = args.directory / 'log-4gb.log'
    if not log.exists() or log.stat().st_size != 4_000_000_000:
        line = b'2026-11-15 INFO velocity '.ljust(99, b'x') + b'\n'
        assert len(line) == 100
        block = line * 10_000
        with log.open('wb') as out:
            for _ in range(4000):
                out.write(block)
    js = args.directory / 'line-50mb.js'
    if not js.exists() or js.stat().st_size != 50_000_000:
        with js.open('wb') as out:
            out.write(b'const payload="')
            out.write(b'a' * (50_000_000 - 18))
            out.write(b'";\n')
    binary = args.directory / 'binary-4gb.bin'
    if not binary.exists() or binary.stat().st_size != 4_000_000_003:
        with binary.open('wb') as out:
            out.truncate(4_000_000_003)
            out.seek(-3, 2)
            out.write(b'END')
    for path in (log, js, binary):
        print(path, path.stat().st_size)


if __name__ == '__main__':
    main()
