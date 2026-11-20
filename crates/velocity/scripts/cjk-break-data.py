#!/usr/bin/env python3
"""Print the CJK wrap table from Unicode 17.0.0 UCD files in the given directory."""
import sys
from pathlib import Path


def points(path):
    for line in path.read_text(encoding="utf-8").splitlines():
        record = line.split("#", 1)[0].strip()
        if record:
            code, value = map(str.strip, record.split(";"))
            lo, sep, hi = code.partition("..")
            for point in range(int(lo, 16), int(hi if sep else lo, 16) + 1):
                yield point, value


def main(ucd):
    width = dict(points(ucd / "EastAsianWidth.txt"))
    cjk = {n for n, script in points(ucd / "Scripts.txt")
           if script in {"Han", "Hiragana", "Katakana", "Hangul", "Bopomofo", "Katakana_Or_Hiragana"}}
    ranges = []
    for n, lb in points(ucd / "LineBreak.txt"):
        wide = width.get(n) in {"W", "F", "H"}
        if wide and lb == "OP":
            kind = "Open"
        elif (wide and lb in {"CL", "CP", "EX", "IS", "NS", "IN"}) or lb == "CJ":
            kind = "NoStart"
        elif lb == "IN" and width.get(n) == "A":
            kind = "Inseparable"
        elif n in cjk and lb in {"ID", "H2", "H3", "JL", "JV", "JT"}:
            kind = "Cjk"
        else:
            continue
        if ranges and ranges[-1][1] + 1 == n and ranges[-1][2] == kind:
            lo, _, _ = ranges.pop()
            ranges.append((lo, n, kind))
        else:
            ranges.append((n, n, kind))
    print("// Generated from https://www.unicode.org/Public/17.0.0/ucd/ by")
    print("// scripts/cjk-break-data.py. See ../../vel-render/fixtures/UNICODE-LICENSE.txt.")
    print("use super::BreakKind::*;\n")
    print("pub(super) const RANGES: &[(u32, u32, super::BreakKind)] = &[")
    for lo, hi, kind in ranges:
        print(f"    (0x{lo:04X}, 0x{hi:04X}, {kind}),")
    print("];")


if __name__ == "__main__":
    main(Path(sys.argv[1]))
