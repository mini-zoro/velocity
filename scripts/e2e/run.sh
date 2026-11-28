#!/bin/bash
# GUI end-to-end harness runner: builds the driver, runs one probe or scenario with
# a watchdog, and appends the results to $GITHUB_STEP_SUMMARY.
#
#   scripts/e2e/run.sh build
#   scripts/e2e/run.sh probe
#   scripts/e2e/run.sh scenarios       # probe first, then every scenario the runner supports
#   scripts/e2e/run.sh scenario ime
#   scripts/e2e/run.sh scenario tabdrag  # E2E_TABDRAG_LAUNCHERS=before=…,after=… compares two builds
#   scripts/e2e/run.sh summary         # nonzero unless every recorded scenario passed
#
# Environment:
#   E2E_BIN         driver build directory (default $RUNNER_TEMP/e2e/bin)
#   E2E_WORK        per-run work root (default $RUNNER_TEMP/e2e/work)
#   E2E_VELOCITY    Velocity CLI to launch (default target/release/velocity)
#   E2E_ARTIFACTS   where screenshots and AX dumps land on failure
#   E2E_RESULTS     directory holding per-scenario logs (default $E2E_WORK/results)
set -euo pipefail
VELOCITY_E2E=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
VELOCITY_ROOT=$(CDPATH= cd -- "$VELOCITY_E2E/../.." && pwd)
E2E_BIN=${E2E_BIN:-"${RUNNER_TEMP:-/tmp}/e2e/bin"}
E2E_WORK=${E2E_WORK:-"${RUNNER_TEMP:-/tmp}/e2e/work"}
E2E_ARTIFACTS=${E2E_ARTIFACTS:-"${RUNNER_TEMP:-/tmp}/e2e/artifacts"}
E2E_RESULTS=${E2E_RESULTS:-"$E2E_WORK/results"}
export E2E_VELOCITY=${E2E_VELOCITY:-"$VELOCITY_ROOT/target/release/velocity"}
export E2E_ARTIFACTS E2E_WORK E2E_RESULTS
export E2E_ROOT=${E2E_ROOT:-"$VELOCITY_ROOT"}
SUMMARY=${GITHUB_STEP_SUMMARY:-/dev/null}

usage() {
    printf '%s\n' "usage: $0 build|probe|scenarios|scenario <name>|summary" >&2
    exit 2
}

build() {
    mkdir -p "$E2E_BIN"
    # Crash offsets are only comparable with the same compiler.
    swiftc --version 2>&1 | head -1
    swiftc -O -wmo -o "$E2E_BIN/e2e" \
        "$VELOCITY_E2E/main.swift" "$VELOCITY_E2E/ax-support.swift" "$VELOCITY_E2E/scenarios.swift" \
        "$VELOCITY_E2E/tabdrag.swift" "$VELOCITY_E2E/startup.swift" "$VELOCITY_E2E/memory.swift"
    echo "driver: $E2E_BIN/e2e"
}

# run_with_timeout SECONDS command...
# A foreground poll keeps no orphan watchdog behind: GitHub waits for every
# descendant, so a stray `sleep` would hold the step open for the full limit.
run_with_timeout() {
    local limit=$1
    shift
    "$@" &
    local child=$!
    local started
    started=$(date +%s)
    while kill -0 "$child" 2>/dev/null; do
        if [ $(( $(date +%s) - started )) -ge "$limit" ]; then
            echo "TIMEOUT after ${limit}s: $*"
            kill -TERM "$child" 2>/dev/null || true
            sleep 5
            kill -KILL "$child" 2>/dev/null || true
            break
        fi
        sleep 1
    done
    local status=0
    wait "$child" 2>/dev/null || status=$?
    return "$status"
}

cleanup_apps() {
    pkill -x velocity 2>/dev/null || true
    pkill -x TextEdit 2>/dev/null || true
    sleep 0.5
}

append_summary() {
    local title=$1 log=$2
    {
        printf '\n### %s\n\n```\n' "$title"
        tail -c 60000 "$log"
        printf '```\n'
    } >>"$SUMMARY"
}

probe() {
    mkdir -p "$E2E_RESULTS" "$E2E_ARTIFACTS"
    build
    local log="$E2E_RESULTS/probe.log"
    local status=0
    run_with_timeout 600 "$E2E_BIN/e2e" probe 2>&1 | tee "$log" || status=$?
    append_summary "Step 1 probe: what a macOS runner can do" "$log"
    echo "probe exit=$status"
}

# crash_report NAME MARKER LOG: after a driver killed by a signal, waits for the
# system crash report written since MARKER, copies it into the artifacts and
# appends the crashed thread's frames to LOG.
crash_report() {
    local name=$1 marker=$2 log=$3 report="" _
    for _ in $(seq 1 30); do
        report=$(find "$HOME/Library/Logs/DiagnosticReports" -maxdepth 1 -name 'e2e*' -newer "$marker" 2>/dev/null | head -1)
        [ -n "$report" ] && break
        sleep 0.5
    done
    if [ -z "$report" ]; then
        echo "no system crash report for the $name driver" | tee -a "$log"
        return 0
    fi
    cp "$report" "$E2E_ARTIFACTS/crash-$name-$(basename "$report")"
    {
        echo "crash report: $(basename "$report")"
        python3 -I - "$report" <<'PY' || true
import json, sys
text = open(sys.argv[1]).read()
body = json.loads(text[text.index("\n") + 1:])
print("exception:", json.dumps(body.get("exception")), json.dumps(body.get("termination", {}).get("indicator")))
images = body.get("usedImages", [])
for thread in body.get("threads", []):
    if thread.get("triggered"):
        for frame in thread.get("frames", [])[:30]:
            image = images[frame.get("imageIndex", 0)].get("name", "?") if images else "?"
            print("  ", image, frame.get("symbol", "?"), "+", frame.get("symbolLocation", ""))
PY
    } | tee -a "$log"
}

scenario() {
    local name=$1
    mkdir -p "$E2E_RESULTS" "$E2E_ARTIFACTS"
    build
    cleanup_apps
    local work="$E2E_WORK/$name"
    local log="$E2E_RESULTS/$name.log"
    rm -rf "$work"
    mkdir -p "$work" "$E2E_ARTIFACTS"
    local status=0
    local marker="$work/.started"
    touch "$marker"
    # The tabdrag scenario measures two builds over several rounds; the startup
    # scenario records dozens of cold launches and the memory gate runs several full
    # editing sessions (both twice per build in an A/B run).
    local limit=480
    [ "$name" = tabdrag ] && limit=1200
    [ "$name" = startup ] && limit=1800
    [ "$name" = memory ] && limit=1800
    E2E_SUMMARY="$E2E_RESULTS/$name.summary" \
        run_with_timeout "$limit" "$E2E_BIN/e2e" scenario "$name" "$work/data" 2>&1 | tee "$log" || status=$?
    if [ "$status" -ge 128 ]; then
        crash_report "$name" "$marker" "$log"
    fi
    cleanup_apps
    append_summary "Scenario $name" "$log"
    echo "$name exit=$status"
    # Keep going after a failing scenario so the summary lists every result.
    return 0
}

# The probe decides which scenarios can run: no HID keyboard, no IME; no HID
# pointer, no drag or click scenarios.
scenarios() {
    local probe_log="$E2E_RESULTS/probe.log"
    if [ ! -e "$probe_log" ]; then
        echo "no probe log at $probe_log; running the probe first" >&2
        probe
    fi
    local keyboard=0 pointer=0
    # Selecting Pinyin is the gate, not the TextEdit baseline: the ime scenario is
    # where Velocity's own composition is judged.
    if grep -q '^PASS \[probe\] Pinyin HID: typing in a window opened after the switch commits' "$probe_log"; then keyboard=1; fi
    if grep -q '^PASS \[probe\] HID click places the caret' "$probe_log"; then pointer=1; fi
    echo "probe: keyboard=$keyboard pointer=$pointer"
    if [ "$keyboard" = 1 ] && [ "$pointer" = 1 ]; then
        # Scenarios that need the input method and the mouse.
        scenario ime
    fi
    if [ "$pointer" = 1 ]; then
        scenario tabs
        scenario tabdrag
        scenario activate
        scenario background
    fi
    scenario footprint
    # Screen recording only; no keyboard or mouse.
    scenario startup
    # Needs no HID: every action is a CLI request. Fails the run above its limits.
    scenario memory
    {
        printf '\n### Scenario selection\n\nkeyboard(IME)=%s pointer(mouse)=%s (from the probe)\n' "$keyboard" "$pointer"
    } >>"$SUMMARY"
}

summary() {
    local failed=0
    {
        printf '\n### Scenario results\n\n| scenario | verdict | checks |\n| --- | --- | --- |\n'
        local file name line verdict
        for file in "$E2E_RESULTS"/*.log; do
            [ -e "$file" ] || continue
            name=$(basename "$file" .log)
            if [ "$name" = probe ]; then
                # The capability table is informational; its CHECK lines are shown in
                # full above, so only a count goes in the table.
                printf '| probe | INFO | %s checks failed of %s |\n' \
                    "$(grep -c '^FAIL' "$file" || true)" "$(grep -c '^\(PASS\|FAIL\)' "$file" || true)"
                continue
            fi
            line=$(grep '^SUMMARY' "$file" | tail -1 || true)
            if [ -z "$line" ]; then
                printf '| %s | FAIL | no SUMMARY line (crash or timeout) |\n' "$name"
                failed=1
                continue
            fi
            verdict=$(printf '%s' "$line" | awk '{print $3}')
            case "$verdict" in
                PASS) printf '| %s | PASS | %s |\n' "$name" "$(printf '%s' "$line" | sed -n 's/^SUMMARY \[[^]]*\] PASS //p')" ;;
                FAIL)
                    printf '| %s | FAIL | %s |\n' "$name" "$(printf '%s' "$line" | sed -n 's/^SUMMARY \[[^]]*\] FAIL //p')"
                    failed=1
                    ;;
                *) printf '| %s | INFO | %s |\n' "$name" "$line" ;;
            esac
        done
    } >>"$SUMMARY"
    return "$failed"
}

command -v swiftc >/dev/null || { echo "swiftc is required" >&2; exit 2; }
if [ "$#" -eq 0 ]; then usage; fi
case "$1" in
    build) build ;;
    probe) probe ;;
    scenarios) scenarios ;;
    scenario) [ "$#" -ge 2 ] || usage; scenario "$2" ;;
    summary) summary ;;
    *) usage ;;
esac
