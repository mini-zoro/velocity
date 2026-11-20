#!/bin/bash
# Explicit opt-in: these benchmarks create visible native windows.
set -euo pipefail
cd -- "$(dirname -- "$0")/.."

if [[ ${1:-} == --help || $# -gt 1 ]]; then
    printf 'Usage: %s [path/to/release/velocity] (opens windows; five runs per workload)\n' "$0"
    exit 0
fi
binary=$(/usr/bin/python3 -c 'import os,sys; print(os.path.abspath(sys.argv[1]))' "${1:-target/release/velocity}")
if [[ ! -x "$binary" ]]; then
    printf 'Not an executable: %s\n' "$binary" >&2
    exit 1
fi
corpus="$PWD/spikes/data/sublime-comparison"
for file in sqlite3.c gcc.c generated-{1,5}m.c line-{10000,100000,1000000,50000000}.txt log-250mb.txt; do
    if [[ ! -s "$corpus/$file" ]]; then
        printf 'Missing %s; run python3 scripts/prepare-perf-corpus.py first.\n' "$corpus/$file" >&2
        exit 1
    fi
done
scale=${VELOCITY_BENCH_SCALE:-2}
case "$scale" in
    1|2) ;;
    *) printf 'VELOCITY_BENCH_SCALE must be 1 or 2.\n' >&2; exit 1 ;;
esac

root=$(mktemp -d "${TMPDIR:-/tmp}/velocity-bench-gui.XXXXXX")
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export HOME="$root/home" SHELL=/bin/zsh
export VELOCITY_LOG_DIR="$root/logs" TMPDIR="$root/tmp"
mkdir -p "$HOME" "$VELOCITY_LOG_DIR" "$TMPDIR"
printf 'Native benchmarks will open windows. Keep the display awake and unobscured.\nRaw logs: %s\n' "$root" >&2

run() {
    local label="$1" stem="$root/$i-$1"
    shift
    printf '%s (%s/5)\n' "$label" "$i" >&2
    /usr/bin/uptime > "$stem.uptime"
    if ! "$binary" --data-dir "$HOME/profile" --output "$stem.jsonl" --timeout 300 "$@" \
        </dev/null > "$stem.stdout" 2> "$stem.stderr"; then
        printf 'Failed: %s (see %s.stderr); continuing.\n' "$label" "$stem" >&2
    fi
}

for i in 1 2 3 4 5; do
    run startup --bench-startup --lines 5000
    run typing-sqlite-1 --bench-typing "$corpus/sqlite3.c" --carets 1
    run typing-sqlite-1000 --bench-typing "$corpus/sqlite3.c" --carets 1000
    run typing-gcc-1 --bench-typing "$corpus/gcc.c" --carets 1
    run scroll-4k --bench-scroll "$corpus/sqlite3.c" \
        --width "$((3840 / scale))" --height "$((2160 / scale))"
    run memory-5k --bench-memory --lines 5000 "$corpus/sqlite3.c"
    for file in generated-{1,5}m.c line-{10000,100000,1000000,50000000}.txt log-250mb.txt; do
        run "large-$file" --bench-large-files "$corpus/$file"
    done
done

/usr/bin/python3 -B scripts/bench-gui-summary.py "$root"
