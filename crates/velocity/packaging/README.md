# macOS release artifacts — Lane B round 12

Run from any directory on macOS:

```sh
scripts/package-macos.sh
# Or choose an output directory, including a path with spaces:
scripts/package-macos.sh /absolute/path/to/artifacts
```

The default destination is `target/package-macos/`. Packaging uses the pinned Rust toolchain,
Cargo.lock, the checked-out resource tree, system Python 3, Swift/AppKit, codesign, ditto and
hdiutil. `VELOCITY_PYTHON` can select another Python 3 with the standard library. The default
system interpreter avoids Homebrew library mismatches. Build tools and dependencies must
already be installed; no toolchain or dependency upgrade is performed by the script.

Outputs:

- `Velocity.app`
- `Velocity-<Cargo version>-arm64.dmg` (or `-universal.dmg`)
- `Velocity-<Cargo version>-arm64.app.zip` (or `-universal.app.zip`)
- `SHA256SUMS` for the zip and DMG
- `packaging-report.json` with paths, byte sizes and validation results

The script builds arm64, plus x86_64 whenever both Rust targets are installed, and combines
them with lipo. It never silently substitutes an Intel-only artifact. Native builds reuse the
host target directory; cross builds use Cargo's architecture directories. `CARGO_TARGET_DIR`
is obtained from Cargo metadata and can be absolute or relative. Missing arm64 target fails
early with an install instruction. Existing destination artifacts are moved to timestamped
backups before new artifacts are moved from a temporary directory on the same filesystem.
Failed builds leave the previous artifacts intact. No artifact is committed, pushed or
published by these scripts.

The build recipe pins dependencies, takes the app version from Cargo metadata, normalizes
resource timestamps to `SOURCE_DATE_EPOCH` (default: source commit time), and records the source
revision/version/architectures. Resource contents have a SHA-256 manifest. This is a repeatable
recipe, not a promise of byte-identical DMGs across OS/toolchain versions: HFS volume creation
dates, native encoders, signing tickets and the DMG container can differ.

Bundle layout:

```text
Velocity.app/Contents/
  Info.plist                           dev.velocity.Velocity; Cargo version; macOS 14.0
  MacOS/velocity                       release Mach-O (arm64 or universal)
  SharedSupport/bin/velocity           symlink-aware launcher, preserves cwd/stdin/args
  Resources/
    Velocity.icns                      original artwork; 10 standard/Retina representations
    Syntaxes/                          public permissive grammars, metadata, LICENSE
    Packages/Default/                  original Velocity settings/keymaps/menu/snippets
    Packages/Theme - Velocity/         original themes, icons and design notes
    Packages/Color Scheme - Velocity/  original light/dark schemes and design notes
    Dictionaries/en_US/                .aff/.dic, LICENSE and provenance
    LICENSES/                          Velocity and dependency notices, including MPL text
    build-info.json
    bundle-manifest.json
    ime-and-unicode.rs
  _CodeSignature/CodeResources
```

Info.plist declares common text/source extensions and public text/source UTIs with Editor
role and Alternate handler rank; it does not seize default document ownership. It enables
Retina and system appearance changes. The icon is an original cyan folded V with two motion
bars on an indigo rounded square, drawn entirely by `generate-icon.swift`. No external artwork,
font or Sublime asset is used. `bundle.py` writes its PNG representations into an ICNS container
directly and verifies each representation's dimensions. Regeneration does not depend on
iconutil conversion behavior. Git metadata and upstream syntax tests are excluded from bundles;
the upstream root LICENSE and resource notices remain beside the bundled resources.

The layout check also verifies every Mach-O architecture and its macOS 14.0 deployment target.
The launcher execs the real app executable, which already implements per-profile Unix-socket
single-instance forwarding, stdin, file locations, new/add window requests and close-driven
wait. It does not implement a second protocol or spawn `open --args`. `test-package-macos.sh`
validates the plist, complete resource manifest, icon, executable modes and strict signature,
then runs a headless IPC peer against the real packaged launcher/executable. It covers Unicode
and space-containing paths, symlink invocation, relative paths, `--`, stdin, `-n`, `-a`, and
`-w` staying blocked until a close acknowledgement. The peer mirrors `src/instance.rs` framing
and socket naming; this is not a native GUI end-to-end test. Existing workspace tests also
exercise the actual instance server and close-driven waiters.

DMGs use a read-only HFS+ filesystem built by DiscRecording (`hdiutil makehybrid -hfs`), then
UDZO compression. The root contains only the app and `Applications -> /Applications`.
This construction needs no disk-device attachment. After signing, `hdiutil verify` checks the
image checksum, conversion reads it back to a raw image, and `check-hfs.py` verifies every app
file's bytes and Unix modes against the signed bundle and reads the actual Applications
symlink target. It rejects unsupported/extents-overflow images rather than skipping data.
This permits complete content verification even on hosts where disk-device creation/mounts
return ENXIO. The app signature is validated before imaging; every signed byte is checked
again in the image. The standalone bundle test can be rerun on an installed/mounted app.

Signing is ad-hoc by default, for both app and DMG. It is suitable for local testing; it is not
a Developer ID distribution signature. For an installed Developer ID Application identity:

```sh
export VELOCITY_SIGNING_IDENTITY='Developer ID Application: YOUR NAME (TEAMID)'
scripts/package-macos.sh
```

Developer ID signing enables hardened runtime and Apple's secure timestamp. Signing itself
can contact Apple's timestamp service. Notarization is disabled unless `VELOCITY_NOTARIZE=1`.
To explicitly submit both the app zip and final DMG and staple their tickets:

```sh
export VELOCITY_NOTARIZE=1
export VELOCITY_NOTARY_PROFILE='your-existing-keychain-profile'
# Alternatively provide APPLE_ID, APPLE_TEAM_ID and APPLE_APP_PASSWORD in the environment.
scripts/package-macos.sh
```

Keep identities/credentials in the user's Keychain/environment, never in tracked files. No
signing identity or notarization credentials were present for this lane; only local ad-hoc
signing was exercised. The release-artifacts workflow triggers on `v*` tags and manual dispatch,
installs both architectures, runs the same checks, and retains workflow artifacts only. It
has read-only repository permissions and never invokes a release/publish action. CI runs
the native packaging path as well. Its Rust test harness runs serially because the existing
native IPC fixture has short wall-clock deadlines that can expire under parallel test-worker
contention; every test still runs. Neither workflow is triggered by this worktree's changes.

## Lane A integration required

No `crates/velocity/src`, other existing crate, or velocity manifest was modified by this lane.
The two new crates are already workspace members through `crates/*`; Lane A adds them to the
app's dependencies before wiring these APIs:

1. **Crash startup:** before native initialization, resolve the home directory, construct
   `vel_crash::CrashReporter::new(Config::for_home(home, env!("CARGO_PKG_VERSION")))`, collect
   `pending_reports()`, and install the panic/signal hooks once. Retain the reporter for
   `record(Level, static_event)` calls. On the next interactive launch show an Open Report
   Folder / Dismiss prompt for pending reports; only open `reporter.directory()` on acceptance,
   then `acknowledge` the reports that were shown. Keep runtime/document log text out of the
   static diagnostic history and do not redirect prior-hook stderr into it.
2. **Install Command Line Tool:** register the `install_cli` command/menu action. Derive the
   installed `.app` from `current_exe()`, offer `cli::install_directories(home)`, create a
   `CliInstallPlan`, display its exact source/destination, and pass the user's choice to
   `cli::install_cli`. Prefer the user-local choice if the system directory is unwritable.
   Do not ask for sudo or silently overwrite an existing command.
3. **Package actions:** create `PackageManager` with the same data directory used by the
   workspace and instance server. Wire list/install-local/install-HTTPS/remove/enable/disable
   commands to a worker and surface returned backups/errors. Supply inherited ignored defaults
   to `set_enabled_inheriting` when needed, and filter ignored package resources before loading
   syntaxes/themes/keymaps/snippets/completions/build systems. Reload settings/resource caches
   and notify existing views after a successful mutation. Show Python plugins' partial-support
   label. Serialize preferences edits with the app's existing settings writer.
4. **Explicit upgrade source:** add a user setting for an optional index URL (unset by default),
   apply it using `set_index_url`, and call `check_upgrades` only after an explicit user action.
   Show available versions before `install_release`; disabled packages remain disabled.
   There is no index URL to ship in this round.
5. **Bundled lookup:** existing syntax lookup already finds `Contents/Resources/Syntaxes`,
   and many defaults/themes/dictionary resources are embedded. For full on-disk bundle-resource
   support, load `Contents/Resources` as the lowest `vel_config::resources::Resources` layer,
   followed by installed/user data. Preserve original precedence and attach the effective
   ignored-package filter. The shipped dictionary and licenses are under `Resources/Dictionaries`.
6. **Minimum macOS 14 (reviewer decision):** `vel_platform::macos::run` requires macOS 14
   (view-bound `CADisplayLink`), so the bundle declares `LSMinimumSystemVersion` 14.0 and builds
   with `MACOSX_DEPLOYMENT_TARGET=14.0`. A bundle that installs on macOS 13 but refuses to start
   would be worse than not installing. Lowering the minimum later requires a frame-clock fallback
   in `vel-platform` first; the original spec's "13" was not checked against the APIs in use.

See the new crates' READMEs/rustdoc for exact API contracts and index schema. CLI forwarding
is already wired in the existing executable; the new menu/install, crash and package UI
entry points remain intentionally in Lane A's scope.
