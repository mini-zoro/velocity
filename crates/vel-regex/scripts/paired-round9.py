#!/usr/bin/env python3
"""Compare an unmodified before checkout with current code in one process.

Use --prepare-only while another benchmark is running. The generated workspace
lives in /tmp; no project manifests or before sources are modified.
"""
import argparse
import hashlib
import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path

CRATE = Path(__file__).resolve().parents[1]
ROOT = CRATE.parents[1]


def digest(root):
    result = hashlib.sha256()
    for name in ("vel-regex", "vel-scope", "vel-syntax"):
        crate = root / "crates" / name
        for path in sorted([crate / "Cargo.toml", *crate.joinpath("src").rglob("*.rs")]):
            result.update(str(path.relative_to(root)).encode() + b"\0" + path.read_bytes())
    return result.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--before", type=Path, required=True)
    parser.add_argument("--workspace", type=Path)
    parser.add_argument("--output", type=Path, default=CRATE / "round9-paired.json")
    parser.add_argument("--prepare-only", action="store_true")
    args = parser.parse_args()
    scratch = args.workspace or Path(tempfile.mkdtemp(prefix="velocity-round9-paired-"))
    scratch.mkdir(parents=True, exist_ok=True)
    (scratch / "Cargo.toml").write_bytes((args.before / "Cargo.toml").read_bytes())
    (scratch / "Cargo.lock").write_bytes((args.before / "Cargo.lock").read_bytes())
    for name in ("vel-regex", "vel-scope", "vel-syntax"):
        target = scratch / "crates" / name
        shutil.copytree(args.before / "crates" / name, target, dirs_exist_ok=True)
        manifest = target / "Cargo.toml"
        text = manifest.read_text().replace(f'name = "{name}"', f'name = "{name}-before"')
        for dependency in ("vel-regex", "vel-scope"):
            text = text.replace(f'{dependency} = {{ path', f'{dependency} = {{ package = "{dependency}-before", path')
        manifest.write_text(text)
    harness = scratch / "crates" / "round9-paired"
    (harness / "src").mkdir(parents=True, exist_ok=True)
    (harness / "Cargo.toml").write_text(
        '[package]\nname = "round9-paired"\nversion = "0.0.0"\nedition = "2024"\n'
        '[dependencies]\nbefore = { package = "vel-syntax-before", path = "../vel-syntax" }\n'
        f'after = {{ package = "vel-syntax", path = {json.dumps(str(ROOT / "crates/vel-syntax"))} }}\n'
        'serde_json = "1"\n'
    )
    shutil.copyfile(CRATE / "scripts/paired-round9.rs", harness / "src/main.rs")
    if args.prepare_only:
        print(scratch)
        return
    subprocess.run(["cargo", "build", "--offline", "--release", "--manifest-path", str(scratch / "Cargo.toml"), "--bin", "round9-paired"], check=True)
    data = Path(os.environ.get("VEL_REGEX_FIXTURE_ROOT", ROOT))
    subprocess.run([str(scratch / "target/release/round9-paired"), str(ROOT / "third_party/sublime-packages"), str(data), str(args.output.resolve())], check=True)
    evidence = json.loads(args.output.read_text())
    evidence.update({"before_source_sha256": digest(args.before), "after_source_sha256": digest(ROOT)})
    temporary = args.output.with_suffix(".tmp")
    temporary.write_text(json.dumps(evidence, indent=2) + "\n")
    temporary.replace(args.output)


if __name__ == "__main__":
    main()
