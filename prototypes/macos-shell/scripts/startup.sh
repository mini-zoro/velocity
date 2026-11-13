#!/bin/bash
# Freshly link the prototype, then one cold-ish and ten warm launches.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/target"
exec python3 scripts/measure.py startup "$@"
