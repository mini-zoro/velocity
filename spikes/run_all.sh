#!/bin/sh
set -eu
cd "$(dirname "$0")"
export CARGO_HOME="$PWD/data/cargo-home"
export CARGO_TARGET_DIR="$PWD/target"
export CARGO_HTTP_MULTIPLEXING=false
export PYTHONDONTWRITEBYTECODE=1
mkdir -p data/raw
if [ ! -f data/crate_versions.json ]; then python3 scripts/resolve_versions.py; fi
python3 scripts/prepare_data.py > data/prepare.log
python3 scripts/machine.py
cargo build --release --locked --workspace
cp target/release/bench-syntax target/release/bench-syntax-fancy
cargo build --release --locked -p bench-syntax --no-default-features --features onig
cp target/release/bench-syntax target/release/bench-syntax-onig
cargo tree --locked --workspace > data/dependency-tree.txt
python3 scripts/run_benchmarks.py --force "$@"
python3 scripts/validate_results.py
python3 scripts/report.py
