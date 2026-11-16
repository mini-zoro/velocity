#!/bin/zsh
# Run on an awake, unlocked Mac outside the sandbox. Fixture setup is not timed.
set -eu
cd -- "${0:A:h}/../../.."
VELOCITY_R12_BINARY=${1:-./target/release/velocity}
VELOCITY_R12_DATA=${2:-/tmp/velocity-round12-fixtures}
VELOCITY_R12_OUT=$(mktemp -d /tmp/velocity-r12-gui.XXXXXX)
python3 crates/velocity/scripts/prepare-largefiles.py "$VELOCITY_R12_DATA"
cp "$VELOCITY_R12_BINARY" "$VELOCITY_R12_OUT/velocity"
for VELOCITY_R12_FILE in log-4gb.log line-50mb.js binary-4gb.bin; do
  MTL_DEBUG_LAYER=1 "$VELOCITY_R12_OUT/velocity" --bench-large-files \
    "$VELOCITY_R12_DATA/$VELOCITY_R12_FILE" --data-dir "$VELOCITY_R12_OUT/profile" \
    --output "$VELOCITY_R12_OUT/$VELOCITY_R12_FILE.jsonl" --timeout 180
done
print "Native results: $VELOCITY_R12_OUT"
