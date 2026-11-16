#!/bin/bash
# All tests are local. A headless socket fixture avoids starting an interactive GUI.
set -euo pipefail
VELOCITY_REPO=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if [ "$#" != 1 ]; then
    printf '%s\n' 'usage: scripts/test-package-macos.sh /path/to/Velocity.app' >&2
    exit 2
fi
VELOCITY_APP=$(CDPATH= cd -- "$1" && pwd -P)
VELOCITY_PACKAGING=$VELOCITY_REPO/crates/velocity/packaging
VELOCITY_PYTHON=${VELOCITY_PYTHON:-/usr/bin/python3}
plutil -lint "$VELOCITY_APP/Contents/Info.plist"
"$VELOCITY_PYTHON" "$VELOCITY_PACKAGING/bundle.py" validate --app "$VELOCITY_APP"
codesign --verify --deep --strict "$VELOCITY_APP"
if [ "${VELOCITY_SIGNING_IDENTITY:--}" = - ]; then
    # Capture first: `codesign | grep -q` races under pipefail (grep exits early, codesign gets SIGPIPE).
    VELOCITY_SIGNATURE=$(codesign --display --verbose=4 "$VELOCITY_APP" 2>&1)
    case $VELOCITY_SIGNATURE in
        *Signature=adhoc*) ;;
        *)
            printf '%s\n' 'error: expected an ad-hoc signature' >&2
            exit 1
            ;;
    esac
fi
VELOCITY_TEST_DIR=$(mktemp -d)
trap 'rm -rf "$VELOCITY_TEST_DIR"' EXIT
trap 'exit 130' INT
trap 'exit 143' HUP TERM
rustc --edition=2024 "$VELOCITY_PACKAGING/forwarding-fixture.rs" -o "$VELOCITY_TEST_DIR/forwarding-fixture"
"$VELOCITY_PYTHON" "$VELOCITY_PACKAGING/test-forwarding.py" "$VELOCITY_APP" "$VELOCITY_TEST_DIR/forwarding-fixture"
