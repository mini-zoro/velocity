#!/bin/bash
# Native AppKit + CAMetalLayer startup floor (clear-only), for comparison with vel-shell.
# Prints per-phase ms since the first Swift statement; `presented(scanout)` uses
# MTLDrawable.addPresentedHandler, i.e. actual display presentation. Run outside any sandbox.
set -euo pipefail
cd "$(dirname "$0")"
swiftc -O floor.swift -o floor
echo "loadavg: $(sysctl -n vm.loadavg)"
for i in $(seq 1 ${1:-6}); do ./floor; done
