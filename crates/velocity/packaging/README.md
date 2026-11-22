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
  Info.plist                           dev.velocity.Velocity; macOS 14.0; velocity URL scheme
  MacOS/velocity                       release Mach-O (arm64 or universal)
  PlugIns/VelocityFinderNewFile.appex/Contents/
    Info.plist                         dev.velocity.Velocity.FinderNewFile; com.apple.FinderSync
    MacOS/VelocityFinderNewFile         Swift extension; matches app architectures and target
    Resources/{en,zh-Hans}.lproj/Localizable.strings
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
role and Alternate handler rank. `public.data` (Editor, Alternate) offers Open With for other
files, including binaries; `public.folder` (Viewer, Alternate) offers folder opening. These
declarations do not seize default document ownership. It enables
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

### Finder New Text File (round 45)

`bundle.py` builds the Finder Sync extension with the macOS Command Line Tools' `swiftc`;
Xcode is not needed for the local build. The package script signs the extension first, with
only `com.apple.security.app-sandbox`, then signs the enclosing app without `--deep`. Developer
ID signing uses the hardened runtime and timestamps for both. The layout validator checks the
extension's plist, localizations, architecture slices, deployment target and signed
entitlements. Deep signature verification still checks the nested signature.

After publishing to the output directory, the script registers the app with `lsregister -f`
and the embedded extension with `pluginkit -a`. It does not change the extension preference.
From the installed copy of Velocity, run **Finder: Enable New Text File in Context Menu**
or **Finder: Disable New Text File in Context Menu**. Alternatively, on macOS 15 and later,
use [System Settings > General > Login Items & Extensions > Finder](https://support.apple.com/en-us/guide/mac-help/mtusr003/mac)
(under Extensions) and toggle Velocity. On macOS 14, look for Finder Extensions in
System Settings > Privacy & Security > Extensions.

Finder chooses the action's language; Velocity chooses the file's name using its own UI
language. The extension never writes the file. It opens the request in its containing app
with running-app substitution disabled. Velocity accepts only a single absolute `dir`
argument from the Apple-event audit token of that bundle's exact extension executable.
Unauthenticated links, including direct opens from a browser or Terminal, do not create files.
See [native checks](../../../docs/manual-checks.md#finder-new-text-file-round-45).

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
the native packaging path as well. Rust checks use the default test parallelism. The earlier
IPC fixture flake was resolved with readiness/acknowledgement synchronization and a deadlock
watchdog; serialization is no longer required. Neither workflow is triggered by this worktree's changes.

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

## DMG creation (reviewer fix, round 12)

`hdiutil makehybrid -hfs` (used when disk devices cannot be attached, e.g. inside a sandbox) stamps
`com.apple.FinderInfo` on every file, so the app inside the image fails `codesign --verify --deep --strict`
— notarization would reject it. The script now uses `hdiutil create -srcfolder … -fs HFS+ -layout SPUD`
whenever possible (locally outside a sandbox and on GitHub Actions), falls back to `makehybrid` with a
warning, and — when the image can be attached — mounts it and requires strict deep signature
verification of the shipped app. `packaging-report.json` records `dmg_method` and
`dmg_app_strict_codesign`.


## Round 14 native launch and telemetry contract

System UI aliases and discovered private system names are routed through CoreText's UI-font
API in `vel-render/src/native.rs` instead of looking those names up again (which may substitute
Times).

Path-free interactive launches remain in the process selected by LaunchServices when its
parent is launchd (PID 1) or it supplies the legacy `-psn_<high>_<low>` argument. These are
independent signals; no bundle/source-path assumption is used. Keeping that process alive
allows its native delegate to receive Finder/Dock open events. A secondary native process
that finds an existing primary stays alive until it relays the actual event paths, rather
than treating an empty startup probe as the document request. Its temporary session stays
in memory, so it cannot overwrite the primary process's session. If the primary exits before
the event arrives, the receiver claims the instance socket and opens the paths locally.
Interactive CLI invocations
with file or stdin arguments keep the existing GUI re-exec and per-profile IPC forwarding.
An explicit `VELOCITY_GUI_PROCESS=1` still selects an in-process GUI for reviewer fixtures.
Benchmark/headless modes ignore the LaunchServices heuristic.

Interactive telemetry is off unless `--log-input` or `--output` is given. Measurement modes
also enable it. Without `--output`, every mode writes a new
`interactive-<runid>-<pid>.jsonl` under `$VELOCITY_LOG_DIR`, falling back to
`~/Library/Logs/Velocity/`. The JSON `mode` field distinguishes measurement types.
An empty `VELOCITY_LOG_DIR` uses the home fallback. If neither runtime directory is available,
measurement fails with an instruction to set one or use `--output`. Existing results are
never overwritten. The build machine's source directory is never a default output path.

Headless contract checks (no packaging or GUI):

```sh
/usr/bin/python3 -B -m unittest discover -s crates/velocity/packaging -p test_bundle.py
cargo test -p velocity --release cli::
cargo test -p velocity --release report::
cargo test -p velocity --release open_requests::
cargo test -p velocity --release workspace::tests::
```

Native acceptance checklist (outside the sandbox; results remain pending human review):

- [ ] From a stopped app, Finder Open With → Velocity opens a text file, a binary and a
  large text file through their normal loaders; the startup open event is retained.
- [ ] After explicitly choosing Velocity in Get Info → Open with → Change All…, double-click
  a disposable text file. Repeat while Velocity is running: its key window receives the tab.
- [ ] Drop files and a folder onto the Dock icon, then onto the editor, tab strip and side bar.
  Files open as tabs and folders appear in the side bar. Duplicate symlink/relative paths
  focus the same tab/root and retain unsaved edits. Text drops keep their existing behavior.
- [ ] Open a missing or unreadable path and a valid file in the same request: the error names
  the failed path and the valid file still opens. Click the Dock icon after closing all windows:
  a new window appears.
- [ ] Start through Finder and through the CLI without measurement flags: no interactive JSONL
  is created. Repeat with `--log-input`, first with a temporary `VELOCITY_LOG_DIR`, then without
  it; confirm the exact runtime destination/name above. An explicit `--output` survives CLI
  GUI re-exec and cannot overwrite a previous result.
- [ ] Open `file:line:col` at startup and over IPC. The requested location is centered after
  viewport layout. Switch tabs and edit/save: titles show filename, dirty marker and project
  name without diagnostic text. Reload user preferences and a menu resource: changes appear
  within the 500 ms stamp polling interval plus worker time; explicit reload starts immediately.

A focused resource-stamp benchmark helper is available for paired release samples:

```sh
cargo test -p velocity --release workspace::tests::benchmark_resource_stamp_polling -- --ignored --nocapture
```

It prints five paired baseline/throttled samples in ns per poll, with resource-stamp counts.
It does not initialize AppKit/Metal and does not substitute for the full typing/scroll gate.
An embedded-only configuration can report zero filesystem stamps; that result cannot measure
stamp-scan savings. Use a populated native profile and the manual profiler checklist for
the per-frame AppKit result.
