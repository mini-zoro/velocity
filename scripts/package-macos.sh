#!/bin/bash
# Local artifacts only. Notarization is opt-in and requires explicit credentials.
set -euo pipefail
VELOCITY_REPO=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if [ "$(uname -s)" != Darwin ]; then
    printf '%s\n' 'package-macos.sh requires macOS' >&2
    exit 1
fi
if [ "$#" -gt 1 ]; then
    printf '%s\n' 'usage: scripts/package-macos.sh [OUTPUT_DIRECTORY]' >&2
    exit 2
fi
VELOCITY_OUTPUT=${1:-"$VELOCITY_REPO/target/package-macos"}
mkdir -p "$VELOCITY_OUTPUT"
VELOCITY_OUTPUT=$(CDPATH= cd -- "$VELOCITY_OUTPUT" && pwd -P)
VELOCITY_STAGE=$(mktemp -d "$VELOCITY_OUTPUT/.package.XXXXXX")
VELOCITY_LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
VELOCITY_FINDER_APPEX=Contents/PlugIns/VelocityFinderNewFile.appex
VELOCITY_FINDER_ID=dev.velocity.Velocity.FinderNewFile
# Remember whether the user had enabled the Finder extension: a repackaged extension has a
# new ad hoc signature, and Finder keeps running the old one until it is toggled.
if /usr/bin/pluginkit -m -i "$VELOCITY_FINDER_ID" 2>/dev/null | grep -q '^+'; then
    VELOCITY_FINDER_WAS_ENABLED=1
else
    VELOCITY_FINDER_WAS_ENABLED=0
fi
unregister() {
    # Staging and backup copies share the bundle identifier; keep them out of
    # LaunchServices so Finder's Open With never picks one of them.
    if [ -x "$VELOCITY_LSREGISTER" ] && [ -e "$1" ]; then
        "$VELOCITY_LSREGISTER" -u "$1" >/dev/null 2>&1 || true
    fi
}
cleanup() {
    unregister "$VELOCITY_STAGE/Velocity.app"
    rm -rf "$VELOCITY_STAGE"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' HUP TERM
cd "$VELOCITY_REPO"
VELOCITY_PACKAGING=$VELOCITY_REPO/crates/velocity/packaging
VELOCITY_PYTHON=${VELOCITY_PYTHON:-/usr/bin/python3}
"$VELOCITY_PYTHON" -c 'import json, plistlib' # Check prerequisites before building.
VELOCITY_IDENTITY=${VELOCITY_SIGNING_IDENTITY:--}
VELOCITY_NOTARIZE=${VELOCITY_NOTARIZE:-0}
if [ "$VELOCITY_NOTARIZE" != 0 ] && [ "$VELOCITY_NOTARIZE" != 1 ]; then
    printf '%s\n' 'VELOCITY_NOTARIZE must be 0 or 1' >&2
    exit 2
fi
if [ "$VELOCITY_NOTARIZE" = 1 ]; then
    if [ "$VELOCITY_IDENTITY" = - ]; then
        printf '%s\n' 'Notarization requires VELOCITY_SIGNING_IDENTITY (Developer ID Application)' >&2
        exit 2
    fi
    if [ -z "${VELOCITY_NOTARY_PROFILE:-}" ] && { [ -z "${APPLE_ID:-}" ] || [ -z "${APPLE_TEAM_ID:-}" ] || [ -z "${APPLE_APP_PASSWORD:-}" ]; }; then
        printf '%s\n' 'Provide VELOCITY_NOTARY_PROFILE or APPLE_ID/APPLE_TEAM_ID/APPLE_APP_PASSWORD' >&2
        exit 2
    fi
fi
notarize() {
    if [ -n "${VELOCITY_NOTARY_PROFILE:-}" ]; then
        xcrun notarytool submit "$1" --wait --keychain-profile "$VELOCITY_NOTARY_PROFILE"
    else
        xcrun notarytool submit "$1" --wait --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD"
    fi
}

cargo metadata --locked --no-deps --format-version 1 > "$VELOCITY_STAGE/cargo-metadata.json"
VELOCITY_TARGET_DIR=$("$VELOCITY_PYTHON" -c 'import json,sys; print(json.load(open(sys.argv[1]))["target_directory"])' "$VELOCITY_STAGE/cargo-metadata.json")
VELOCITY_VERSION=$("$VELOCITY_PYTHON" -c 'import json,sys; print(next(p["version"] for p in json.load(open(sys.argv[1]))["packages"] if p["name"] == "velocity"))' "$VELOCITY_STAGE/cargo-metadata.json")
VELOCITY_HOST=$(rustc -vV | sed -n 's/^host: //p')
VELOCITY_TARGETS=$(rustup target list --installed)
if ! printf '%s\n' "$VELOCITY_TARGETS" | grep -qx aarch64-apple-darwin; then
    printf '%s\n' 'Install aarch64-apple-darwin with rustup before packaging.' >&2
    exit 2
fi
build_target() {
    if [ "$1" = "$VELOCITY_HOST" ]; then
        MACOSX_DEPLOYMENT_TARGET=14.0 cargo build --locked --release -p velocity
        VELOCITY_BUILT=$VELOCITY_TARGET_DIR/release/velocity
    else
        MACOSX_DEPLOYMENT_TARGET=14.0 cargo build --locked --release -p velocity --target "$1"
        VELOCITY_BUILT=$VELOCITY_TARGET_DIR/$1/release/velocity
    fi
}
build_target aarch64-apple-darwin
VELOCITY_ARM_BINARY=$VELOCITY_BUILT
VELOCITY_ARCHITECTURES=arm64
VELOCITY_ARCHIVE_ARCH=arm64
if printf '%s\n' "$VELOCITY_TARGETS" | grep -qx x86_64-apple-darwin; then
    build_target x86_64-apple-darwin
    lipo -create "$VELOCITY_ARM_BINARY" "$VELOCITY_BUILT" -output "$VELOCITY_STAGE/velocity"
    VELOCITY_ARCHITECTURES='arm64 x86_64'
    VELOCITY_ARCHIVE_ARCH=universal
else
    cp "$VELOCITY_ARM_BINARY" "$VELOCITY_STAGE/velocity"
fi
VELOCITY_EPOCH=${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct)}
VELOCITY_REVISION=$(git rev-parse HEAD)
VELOCITY_APP=$VELOCITY_STAGE/Velocity.app
"$VELOCITY_PYTHON" "$VELOCITY_PACKAGING/bundle.py" assemble --repo "$VELOCITY_REPO" --app "$VELOCITY_APP" \
    --binary "$VELOCITY_STAGE/velocity" --metadata "$VELOCITY_STAGE/cargo-metadata.json" \
    --architectures "$VELOCITY_ARCHITECTURES" --revision "$VELOCITY_REVISION" --epoch "$VELOCITY_EPOCH"
swift -module-cache-path "$VELOCITY_TARGET_DIR/packaging-tools/swift-cache" "$VELOCITY_PACKAGING/generate-icon.swift" "$VELOCITY_STAGE/Velocity.iconset"
"$VELOCITY_PYTHON" "$VELOCITY_PACKAGING/bundle.py" icon --app "$VELOCITY_APP" --iconset "$VELOCITY_STAGE/Velocity.iconset"
"$VELOCITY_PYTHON" "$VELOCITY_PACKAGING/bundle.py" manifest --app "$VELOCITY_APP" --epoch "$VELOCITY_EPOCH"
VELOCITY_SIGN_ARGS=(--force --sign "$VELOCITY_IDENTITY")
if [ "$VELOCITY_IDENTITY" = - ]; then
    VELOCITY_SIGN_ARGS+=(--timestamp=none)
else
    VELOCITY_SIGN_ARGS+=(--options runtime --timestamp)
fi
# Sign inside out. Deep signing would replace the extension's sandbox entitlements.
codesign "${VELOCITY_SIGN_ARGS[@]}" --entitlements "$VELOCITY_PACKAGING/finder-new-file/entitlements.plist" \
    "$VELOCITY_APP/$VELOCITY_FINDER_APPEX"
codesign "${VELOCITY_SIGN_ARGS[@]}" "$VELOCITY_APP"
if [ "$VELOCITY_NOTARIZE" = 1 ]; then
    ditto -c -k --keepParent "$VELOCITY_APP" "$VELOCITY_STAGE/notarization.zip"
    notarize "$VELOCITY_STAGE/notarization.zip"
    xcrun stapler staple "$VELOCITY_APP"
fi
"$VELOCITY_REPO/scripts/test-package-macos.sh" "$VELOCITY_APP"

VELOCITY_BASENAME=Velocity-$VELOCITY_VERSION-$VELOCITY_ARCHIVE_ARCH
mkdir -p "$VELOCITY_STAGE/dmg-root"
ditto "$VELOCITY_APP" "$VELOCITY_STAGE/dmg-root/Velocity.app"
ln -s /Applications "$VELOCITY_STAGE/dmg-root/Applications"
# Prefer `hdiutil create`: it copies files without adding attributes, so the app inside the image
# still passes `codesign --verify --deep --strict` (required for notarization). It needs to attach
# a disk device, which restricted sandboxes forbid; there we fall back to DiscRecording
# (`makehybrid`), which works without a device but stamps com.apple.FinderInfo on every file.
# `hdiutil create` can fail transiently on hosted runners ("Resource busy" while another image
# detaches, or diskimages-helper still starting), so retry before giving up on it.
VELOCITY_DMG_METHOD=create
VELOCITY_DMG_ERRORS="$VELOCITY_STAGE/hdiutil-create.log"
velocity_dmg_create() {
    hdiutil create -quiet -volname Velocity -srcfolder "$VELOCITY_STAGE/dmg-root" -fs HFS+ -layout SPUD \
        -format UDZO -imagekey zlib-level=9 -o "$VELOCITY_STAGE/$VELOCITY_BASENAME.dmg" 2>>"$VELOCITY_DMG_ERRORS"
}
VELOCITY_DMG_CREATED=0
for velocity_attempt in 1 2 3 4; do
    if velocity_dmg_create; then
        VELOCITY_DMG_CREATED=1
        break
    fi
    rm -f "$VELOCITY_STAGE/$VELOCITY_BASENAME.dmg"
    sleep $((velocity_attempt * 5))
done
if [ "$VELOCITY_DMG_CREATED" = 0 ]; then
    VELOCITY_DMG_METHOD=makehybrid
    printf '%s\n' 'warning: hdiutil create failed 4 times; using makehybrid (app inside the DMG will fail strict codesign verification)' >&2
    sed 's/^/  hdiutil create: /' "$VELOCITY_DMG_ERRORS" >&2 || true
    rm -f "$VELOCITY_STAGE/$VELOCITY_BASENAME.dmg"
    hdiutil makehybrid -quiet -hfs -hfs-volume-name Velocity -o "$VELOCITY_STAGE/payload.dmg" "$VELOCITY_STAGE/dmg-root"
    hdiutil convert -quiet "$VELOCITY_STAGE/payload.dmg" -format UDZO -imagekey zlib-level=9 -o "$VELOCITY_STAGE/$VELOCITY_BASENAME.dmg"
fi
if [ "$VELOCITY_IDENTITY" = - ]; then
    codesign --force --sign - --timestamp=none "$VELOCITY_STAGE/$VELOCITY_BASENAME.dmg"
else
    codesign --force --sign "$VELOCITY_IDENTITY" --timestamp "$VELOCITY_STAGE/$VELOCITY_BASENAME.dmg"
fi
if [ "$VELOCITY_NOTARIZE" = 1 ]; then
    notarize "$VELOCITY_STAGE/$VELOCITY_BASENAME.dmg"
    xcrun stapler staple "$VELOCITY_STAGE/$VELOCITY_BASENAME.dmg"
fi
codesign --verify --strict "$VELOCITY_STAGE/$VELOCITY_BASENAME.dmg"
hdiutil verify -quiet "$VELOCITY_STAGE/$VELOCITY_BASENAME.dmg"
# Acceptance check on the shipped bytes: the app inside the mounted image must pass strict, deep
# signature verification. Skipped (with a warning) only where attaching images is impossible.
VELOCITY_MOUNT=$(mktemp -d "$VELOCITY_STAGE/mount.XXXXXX")
if hdiutil attach -quiet -nobrowse -readonly -mountpoint "$VELOCITY_MOUNT" "$VELOCITY_STAGE/$VELOCITY_BASENAME.dmg" 2>/dev/null; then
    VELOCITY_DMG_APP_STRICT=passed
    codesign --verify --deep --strict "$VELOCITY_MOUNT/Velocity.app" || VELOCITY_DMG_APP_STRICT=failed
    hdiutil detach -quiet "$VELOCITY_MOUNT" || hdiutil detach -quiet -force "$VELOCITY_MOUNT"
    if [ "$VELOCITY_DMG_APP_STRICT" != passed ]; then
        printf '%s\n' 'error: Velocity.app inside the DMG fails strict codesign verification' >&2
        exit 1
    fi
else
    VELOCITY_DMG_APP_STRICT=skipped
    printf '%s\n' 'warning: cannot attach the DMG here; strict in-image codesign check skipped' >&2
fi
hdiutil convert -quiet "$VELOCITY_STAGE/$VELOCITY_BASENAME.dmg" -format UDTO -o "$VELOCITY_STAGE/read-back.cdr"
"$VELOCITY_PYTHON" "$VELOCITY_PACKAGING/check-hfs.py" "$VELOCITY_STAGE/read-back.cdr" "$VELOCITY_APP"
ditto -c -k --keepParent "$VELOCITY_APP" "$VELOCITY_STAGE/$VELOCITY_BASENAME.app.zip"
(
    cd "$VELOCITY_STAGE"
    shasum -a 256 "$VELOCITY_BASENAME.app.zip" "$VELOCITY_BASENAME.dmg" > SHA256SUMS
)
"$VELOCITY_PYTHON" "$VELOCITY_PACKAGING/bundle.py" validate --app "$VELOCITY_APP" > "$VELOCITY_STAGE/packaging-report.json"
VELOCITY_DMG_METHOD=$VELOCITY_DMG_METHOD VELOCITY_DMG_APP_STRICT=$VELOCITY_DMG_APP_STRICT \
    "$VELOCITY_PYTHON" - "$VELOCITY_STAGE/packaging-report.json" "$VELOCITY_OUTPUT" "$VELOCITY_STAGE/$VELOCITY_BASENAME.dmg" "$VELOCITY_STAGE/$VELOCITY_BASENAME.app.zip" <<'PY'
import json
import os
from pathlib import Path
import sys
report_path, output, dmg, archive = map(Path, sys.argv[1:])
report = json.loads(report_path.read_text())
report.update(app=str(output / 'Velocity.app'), dmg=str(output / dmg.name), dmg_bytes=dmg.stat().st_size,
              app_zip=str(output / archive.name), app_zip_bytes=archive.stat().st_size,
              codesign='verified', cli_forwarding='passed (headless running IPC fixture)',
              dmg_layout='passed (HFS+ read-back: all app bytes, modes, Applications symlink)',
              dmg_method=os.environ.get('VELOCITY_DMG_METHOD', ''),
              dmg_app_strict_codesign=os.environ.get('VELOCITY_DMG_APP_STRICT', ''))
report_path.write_text(json.dumps(report, indent=2, sort_keys=True) + '\n')
PY
for VELOCITY_ARTIFACT in Velocity.app "$VELOCITY_BASENAME.dmg" "$VELOCITY_BASENAME.app.zip" SHA256SUMS packaging-report.json; do
    if [ -e "$VELOCITY_OUTPUT/$VELOCITY_ARTIFACT" ] || [ -L "$VELOCITY_OUTPUT/$VELOCITY_ARTIFACT" ]; then
        VELOCITY_BACKUP="$VELOCITY_OUTPUT/$VELOCITY_ARTIFACT.backup.$(date +%Y%m%d%H%M%S).$$"
        mv "$VELOCITY_OUTPUT/$VELOCITY_ARTIFACT" "$VELOCITY_BACKUP"
        if [ "$VELOCITY_ARTIFACT" = Velocity.app ]; then unregister "$VELOCITY_BACKUP"; fi
    fi
    if [ "$VELOCITY_ARTIFACT" = Velocity.app ]; then unregister "$VELOCITY_STAGE/$VELOCITY_ARTIFACT"; fi
    mv "$VELOCITY_STAGE/$VELOCITY_ARTIFACT" "$VELOCITY_OUTPUT/$VELOCITY_ARTIFACT"
done
if [ -x "$VELOCITY_LSREGISTER" ]; then
    "$VELOCITY_LSREGISTER" -f "$VELOCITY_OUTPUT/Velocity.app" >/dev/null 2>&1 || true
fi
# Registration does not enable the extension; that is an explicit user choice.
if ! /usr/bin/pluginkit -a "$VELOCITY_OUTPUT/Velocity.app/$VELOCITY_FINDER_APPEX"; then
    printf '%s\n' 'Warning: could not register the Finder extension.' >&2
fi
# Re-apply an existing user choice only (never enable it on our own).
if [ "$VELOCITY_FINDER_WAS_ENABLED" = 1 ]; then
    /usr/bin/pluginkit -e ignore -i "$VELOCITY_FINDER_ID" >/dev/null 2>&1 || true
    /usr/bin/pluginkit -e use -i "$VELOCITY_FINDER_ID" >/dev/null 2>&1 || true
fi
cat "$VELOCITY_OUTPUT/packaging-report.json"
