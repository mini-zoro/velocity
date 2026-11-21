#!/bin/zsh
# Black-box behavior probe: same keystrokes in Sublime (safe mode) and Velocity, then compare saved files.
# usage: bb.sh <sublime|velocity> <case-file> <key tokens...>
# tokens: CODE[:mods]  | t:TEXT (unicode typing) | s:SECONDS
# Build the driver first: swiftc -O scripts/live-check-driver.swift -o /tmp/live/ax
AX=${AX:-/tmp/live/ax}
kind=$1; fx=$2; shift 2
out=/tmp/live/bbout/$kind; mkdir -p $out
f=$out/$(basename $fx); cp /tmp/live/bbfx/$(basename $fx) $f
if [[ $kind == sublime ]]; then
  /usr/bin/open -n -a "/tmp/sublime-blackbox/app/Sublime Text.app" --args --safe-mode "$f"; owner="Sublime Text"
  sleep 3; pid=$(pgrep -n sublime_text)
else
  d=$(mktemp -d /tmp/live/bbprof.XXXX)
  /usr/bin/open -n -a /tmp/live/VelocityTest.app --args --data-dir $d/data "$f"; owner=Velocity
  sleep 3; pid=$(pgrep -f "velocity --data-dir $d/data")
fi
$AX activate $pid >/dev/null; sleep 1.2
for tok in "$@"; do
  case $tok in
    s:*) sleep ${tok#s:};;
    t:*) $AX type hid "${tok#t:}";;
    *:*) $AX key hid ${tok%%:*} ${tok#*:};;
    *) $AX key hid $tok;;
  esac
  sleep 0.12
done
sleep 0.4; $AX key hid 1 cmd; sleep 0.8      # ⌘S
$AX key hid 12 cmd; sleep 1.5; kill $pid 2>/dev/null; sleep 0.5; kill -9 $pid 2>/dev/null
echo "== $kind $(basename $fx)"; cat -A "$f" 2>/dev/null || cat -e "$f"
