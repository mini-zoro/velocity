#!/bin/zsh
# Native reviewer matrix. Run on an awake, unlocked, visible display.
set -eu
cd -- "${0:A:h}/../../.."
cargo build --release -p velocity
VELOCITY_R9_GUI=$(mktemp -d /tmp/velocity-r9-gui.XXXXXX)
for run in 1 2 3 4 5; do
  if [ $((run % 2)) -eq 1 ]; then modes=(off on); else modes=(on off); fi
  for mode in "${modes[@]}"; do
    ./target/release/velocity --bench-intelligence --review-ui "$mode" --intel-words 0 --width 1200 --height 800 --data-dir "$VELOCITY_R9_GUI/profile-$mode-$run" --output "$VELOCITY_R9_GUI/typing-$mode-$run.jsonl" --timeout 300
  done
done
./target/release/velocity --bench-intelligence --review-ui on --intel-words 0 --width 2048 --height 1089 --data-dir "$VELOCITY_R9_GUI/4k-profile" --output "$VELOCITY_R9_GUI/typing-4k.jsonl" --timeout 300
./target/release/velocity --headless-review-history --data-dir "$VELOCITY_R9_GUI/history-profile" --output "$VELOCITY_R9_GUI/history-cpu.jsonl" --timeout 300
./target/release/velocity --bench-startup --restore-session --data-dir "$VELOCITY_R9_GUI/history-profile" --output "$VELOCITY_R9_GUI/restore.jsonl" --timeout 300
MTL_DEBUG_LAYER=1 ./target/release/velocity --bench-startup --restore-session --data-dir "$VELOCITY_R9_GUI/history-profile" --output "$VELOCITY_R9_GUI/metal-validation.jsonl" --timeout 300
python3 crates/velocity/scripts/prepare-review-fixture.py --output-dir "$VELOCITY_R9_GUI/repo" --profile "$VELOCITY_R9_GUI/manual-profile"
./target/release/velocity -n --project "$VELOCITY_R9_GUI/repo/review.sublime-project" "$VELOCITY_R9_GUI/repo/main.rs:1:1" --data-dir "$VELOCITY_R9_GUI/manual-profile" --log-input
