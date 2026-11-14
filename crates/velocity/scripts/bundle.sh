#!/bin/sh
# Bundle-less development and this optional .app use the same executable.
set -eu
VELOCITY_REPO=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
VELOCITY_DEST=${1:-"$VELOCITY_REPO/crates/velocity/dist/Velocity.app"}
cd "$VELOCITY_REPO"
MACOSX_DEPLOYMENT_TARGET=14.0 cargo build --release -p velocity
mkdir -p "$(dirname -- "$VELOCITY_DEST")"
VELOCITY_STAGE=$(mktemp -d "$(dirname -- "$VELOCITY_DEST")/.velocity-bundle.XXXXXX")
trap 'rm -rf "$VELOCITY_STAGE"' EXIT HUP INT TERM
mkdir -p "$VELOCITY_STAGE/Contents/MacOS" "$VELOCITY_STAGE/Contents/Resources"
cp "${CARGO_TARGET_DIR:-target}/release/velocity" "$VELOCITY_STAGE/Contents/MacOS/velocity"
cp crates/velocity/Info.plist "$VELOCITY_STAGE/Contents/Info.plist"
cp crates/velocity/fixtures/ime-and-unicode.rs "$VELOCITY_STAGE/Contents/Resources/ime-and-unicode.rs"
# Public, permissively licensed syntax definitions and metadata, with their notices.
cp -R third_party/sublime-packages "$VELOCITY_STAGE/Contents/Resources/Syntaxes"
if [ -e "$VELOCITY_DEST" ]; then
    mv "$VELOCITY_DEST" "$VELOCITY_DEST.backup.$(date +%Y%m%d%H%M%S).$$"
fi
mv "$VELOCITY_STAGE" "$VELOCITY_DEST"
trap - EXIT HUP INT TERM
printf '%s\n' "$VELOCITY_DEST"
