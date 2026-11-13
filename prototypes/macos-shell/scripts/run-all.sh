#!/bin/bash
# Full unattended suite; keep its windows visible on the display under test.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/target"
exec python3 scripts/measure.py suite "$@"
