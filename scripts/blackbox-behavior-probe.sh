#!/bin/zsh
# Black-box behavior probe: same keystrokes in Sublime (safe mode) and Velocity, then compare saved files.
# usage: bb.sh <sublime|velocity> <case-file> <key tokens...>
# tokens: CODE[:mods]  | t:TEXT (unicode typing) | s:SECONDS
# Build the driver first: swiftc -O scripts/live-check-driver.swift -o /tmp/live/ax
# Fixtures are read from /tmp/live/bbfx; results land in /tmp/live/bbout/<app>/.
# The script aborts (exit 3) if the pointer moves or another app becomes frontmost.
AX=${AX:-/tmp/live/ax}
kind=$1; fx=$2; shift 2
out=/tmp/live/bbout/$kind; mkdir -p $out
f=$out/$(basename $fx); cp /tmp/live/bbfx/$(basename $fx) $f
if [[ $kind == sublime ]]; then
  rm -rf "$HOME/Library/Application Support/Sublime Text (Safe Mode)" "$HOME/Library/Caches/Sublime Text (Safe Mode)"
  /usr/bin/open -n -a "/tmp/sublime-blackbox/app/Sublime Text.app" --args --safe-mode "$f"; owner="Sublime Text"
  sleep 3; pid=$(pgrep -n sublime_text)
  $AX press $pid OK >/dev/null 2>&1; sleep 0.5   # dismiss the Safe Mode notice
else
  d=$(mktemp -d /tmp/live/bbprof.XXXX)
  /usr/bin/open -n -a ${VA:-/Users/zoro/dev/velocity/target/package-macos/Velocity.app} --args --data-dir $d/data "$f"; owner=Velocity
  sleep 3; pid=$(pgrep -n -f "velocity --data-dir $d/data")
fi
$AX activate $pid >/dev/null; sleep 1.2
start_mouse=$($AX mouse)
guard() {  # abort if the user is back: pointer moved or another app is frontmost
  if [[ $($AX mouse) != $start_mouse || $($AX frontpid) != $pid ]]; then
    echo "ABORT $kind $(basename $fx): user activity or focus change" >&2; exit 3
  fi
}
for tok in "$@"; do
  guard
  case $tok in
    s:*) sleep ${tok#s:};;
    t:*) $AX type hid "${tok#t:}";;
    *:*) $AX key hid ${tok%%:*} ${tok#*:};;
    *) $AX key hid $tok;;
  esac
  sleep 0.12
done
sleep 0.4; guard; $AX key hid 1 cmd; sleep 0.8      # ⌘S
guard; $AX key hid 12 cmd; sleep 1.5; kill $pid 2>/dev/null; sleep 0.5; kill -9 $pid 2>/dev/null
echo "== $kind $(basename $fx)"; cat -A "$f" 2>/dev/null || cat -e "$f"
