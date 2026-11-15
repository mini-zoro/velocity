#!/bin/zsh
# Run outside the sandbox on an awake, unlocked, visible display. Baseline is a round-9 binary.
set -eu
cd -- "${0:A:h}/../../.."
if [[ $# -ne 1 ]]; then
  print -u2 'Usage: zsh crates/velocity/scripts/review-round10-gui.zsh /absolute/path/to/round9/velocity'
  exit 2
fi
VELOCITY_R10_BEFORE=${1:A}
cargo build --release -p velocity
VELOCITY_R10_GUI=$(mktemp -d /tmp/velocity-r10-gui.XXXXXX)
cp "$VELOCITY_R10_BEFORE" "$VELOCITY_R10_GUI/velocity-before"
cp ./target/release/velocity "$VELOCITY_R10_GUI/velocity-after"
python3 - "$VELOCITY_R10_GUI" <<'PY'
import datetime, hashlib, json, pathlib, platform, subprocess, sys
root = pathlib.Path(sys.argv[1])
manifest = {'utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
            'platform': platform.platform(),
            'head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
            'binaries': {name: hashlib.sha256((root/name).read_bytes()).hexdigest()
                         for name in ['velocity-before', 'velocity-after']}}
(root/'environment.json').write_text(json.dumps(manifest, indent=2)+'\n')
PY
"$VELOCITY_R10_GUI/velocity-after" --headless-theme-ui --data-dir "$VELOCITY_R10_GUI/cpu-profile" --output "$VELOCITY_R10_GUI/cpu.jsonl" --timeout 300
MTL_DEBUG_LAYER=1 "$VELOCITY_R10_GUI/velocity-after" --bench-theme-ui --width 1200 --height 800 --data-dir "$VELOCITY_R10_GUI/theme-profile" --output "$VELOCITY_R10_GUI/theme-metal.jsonl" --timeout 300
"$VELOCITY_R10_GUI/velocity-after" --bench-theme-ui --width 2048 --height 1089 --data-dir "$VELOCITY_R10_GUI/theme-4k-profile" --output "$VELOCITY_R10_GUI/theme-4k.jsonl" --timeout 300
for workload in typing scroll; do
  for run in 1 2 3 4 5; do
    if [[ $((run % 2)) -eq 1 ]]; then modes=(before after); else modes=(after before); fi
    for mode in "${modes[@]}"; do
      binary="$VELOCITY_R10_GUI/velocity-$mode"
      "$binary" "--bench-$workload" --groups 4 --width 1200 --height 800 --data-dir "$VELOCITY_R10_GUI/$workload-$mode-$run-profile" --output "$VELOCITY_R10_GUI/$workload-$mode-$run.jsonl" --timeout 300
    done
  done
done
print "Native evidence: $VELOCITY_R10_GUI"
print 'Manual: launch below, then Command Palette → Theme & minihtml: Review Demo. Check MANUAL_TEST.md round 10.'
"$VELOCITY_R10_GUI/velocity-after" -n --data-dir "$VELOCITY_R10_GUI/manual-profile" --log-input
