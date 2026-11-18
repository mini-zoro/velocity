#!/usr/bin/env python3
"""Build all release targets and reject login fixtures in the shipping executable.

Does not assemble a package or start Velocity. Feature unification is deliberately
exercised by --all-targets. Run from any directory.
"""
from pathlib import Path
import os
import subprocess

repo = Path(__file__).resolve().parents[3]
subprocess.run(["cargo", "build", "--release", "-p", "velocity", "--all-targets", "--locked"], cwd=repo, check=True)
target = Path(os.environ.get("CARGO_TARGET_DIR", repo / "target"))
if not target.is_absolute():
    target = repo / target
binary = target / "release" / "velocity"
source = binary.read_bytes()
for marker in [b"VELOCITY_R18_LOGIN_FIXTURE", b"test login HOME", b"fixture SHELL", b'#!/bin/sh\nexec /bin/sh -c "$3"\n']:
    assert marker not in source, f"test login fixture leaked into {binary}: {marker!r}"
print(f"PASS: all-targets release binary has no login fixture: {binary}")
