#!/usr/bin/env python3
"""Prepare disposable native-open/menu fixtures; never build or launch an app."""
import argparse
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--huge", action="store_true", help="also write an 80 MiB text file")
    args = parser.parse_args()
    root = args.directory.resolve()
    root.mkdir(parents=True, exist_ok=False)
    project = root / "Project with spaces"
    (project / "nested").mkdir(parents=True)
    (project / "one.rs").write_text("".join(
        f"// line {row}: Velocity reviewer fixture 你好 😀\n" for row in range(1, 601)
    ))
    (project / "two.txt").write_text("Select this phrase\nSpeling example\n")
    (project / "nested" / "three.txt").write_text("Nested document\n")
    (project / "literal:12:3").write_text("A native URL keeps colons literal.\n")
    (project / "any-file.velocity-blob").write_bytes(b"\x00Velocity\xff\x01\x02")
    (root / "fixture.sublime-project").write_text(
        '{"folders":[{"path":"Project with spaces"}]}\n'
    )
    if args.huge:
        block = (b"Velocity large-file reviewer fixture\n" * 2048)
        with (project / "huge.log").open("xb") as stream:
            for _ in range((80 * 1024 * 1024 + len(block) - 1) // len(block)):
                stream.write(block)
    print(root)


if __name__ == "__main__":
    main()
