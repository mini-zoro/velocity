# Round 13 — release wiring, parallel IPC and accessibility

Lane A, based on main `85e6cc1`; no commit. `vel-crash` and `vel-packages` are consumed without
modification. `vel-commands/src/text_ops.rs` is neither created nor edited. The macOS 14 minimum
and round-12 DMG signing fix remain the release baseline.

## IPC failure and fix

The fixture treated scheduler speed as readiness: after spawning its client, it repeatedly
polled `Instance::drain()` and failed after **three seconds**. Parallel workspace tests can
occupy workers with syntax compilation and other CPU work. The listener was already bound;
the failing deadline did not establish a protocol failure. It also started the client's
30-second acknowledgement timeout before constructing an expensive workspace on the server side.

The test now prepares the workspace before starting the client, receives the actual request
through the listener's channel, and uses explicit client acceptance/completion channels. It
checks acceptance before closing the requested sheet, then waits for the close acknowledgement.
The 120-second receive timeout is a deadlock watchdog; there is no sleep/poll readiness gate or
latency assertion. A private acknowledgement observer shares the production forwarding path.
Each fixture retains its unique temp profile; the derived socket name and packaged-launcher
protocol remain compatible. There is no process-global test-thread override.

The only CI edit replaces the `RUST_TEST_THREADS: '1'` line with an empty YAML mapping (`{}`).
This leaves the existing `env:` container valid while meeting the one-line edit constraint.
The resulting test environment mapping was parsed and checked to be empty.

## User-visible release actions

Interactive primary launches install the local panic/signal hooks before workspace/native
initialization. Previous nonempty reports produce a native Open Report Folder / Dismiss alert
on the next interactive launch. Acknowledgement retains `.seen` files. Help has Open Crash
Reports Folder. Only static lifecycle text enters the crash history; runtime messages, document
contents and prior-hook stderr are not redirected into it. Help/automatic CPU modes do not
create reports in the user's log directory.

Velocity → Install Command Line Tool… (also in the palette) prepares two concrete bundle plans
on the worker, shows their resolved source/destination, and executes the selected plan after
confirmation. An unwritable system directory puts the user-local choice first. The UI explains
PATH and reports conflicts; the engine never overwrites an unrelated tool or escalates.

The palette exposes Package: Install from File…, Install from URL…, Remove, Disable/Enable and
List. URL installation asks for the HTTPS URL and package name, including signed/query URLs.
List shows effective disabled state/resource counts and contribution paths on selection, including
**partially supported: no plugin API** for Python-bearing packages. Python is never executed.
Mutations surface engine backup paths/errors. Upgrade checking exists only with an explicitly
configured `package_index_url`; its default is null. An explicit check lists installed/available
versions before a separate install confirmation; engine digest/metadata checks and disabled state
are preserved. There is no registry/timer/implicit network request.

Archive operations, downloads and app preferences writes share a serial worker queue across
windows. Live configuration discovery, syntax/metadata compilation, schemes/catalogs, PNG decoding
and both selected theme appearance variants are prepared on a worker. Publication moves compiled
paint data into fresh thread-local metrics/caches; it performs no ZIP I/O, downloads or PNG decode.
Existing editors receive the new catalogs/keymaps/menus/settings without replacing their buffers,
undo history, view selections, runtime folders or project state. Watcher-driven reload tests now
wait for publication rather than assuming the first poll finishes all work.

`Contents/Resources` overlays embedded fallbacks below installed/user data. The effective ignored
set filters the shared resource lookup for syntaxes, metadata, schemes, themes/images, keymaps,
menus, snippets, completions and builds. Syntax loading consumes those filtered virtual resources;
physical-root scans cannot resurrect disabled grammars. User preferences remain readable. Vintage
continues to use the app's existing `vintage_enabled` setting.

## Accessibility model and bridge

The implementation uses the public [NSAccessibility protocol](https://developer.apple.com/documentation/appkit/nsaccessibilityprotocol)
and [NSWorkspace display accessibility preferences](https://developer.apple.com/documentation/appkit/nsworkspace/accessibilitydisplayshouldreducemotion).
No native objects are constructed by the model tests.

The native workspace contains a semantic editor text area, tab group/tab buttons/close buttons,
project outline, quick-panel lists/rows, panel text fields and buttons, syntax popup, status text,
completion choices, context menus and visible minihtml popup text/links. Their actions route through
the same controller/hit geometry as ordinary input. Cmd+F6 / Cmd+Shift+F6 traverses active controls;
Return/Space activates an explicitly focused non-text control. Modal quick panels expose their
own controls and restore the editor on close.

Text values/ranges use UTF-16 and complete scalar boundaries. Parameterized line, line-range,
string and bounds queries share the editor/input layout, wrapping/folds and logical-point geometry;
the bridge converts to screen coordinates once. Multi-caret editing exposes the primary selection.
The insertion line follows the primary caret even for a reversed selection. Text/focus/list/layout
notifications are posted after publication, not during an app borrow. Accessibility trees are queried
on demand; normal CPU scene construction does not build them.

Each exposed value has at most **65,536 UTF-16 units**. Larger documents use a window around the
primary selection, with its document offset/first line in Help. AX ranges are relative to that value;
the insertion line remains document-relative. Moving the caret changes the window. Whole-value AX
replacement is disabled for a partial window. Binary hex documents currently expose a read-only
description rather than navigable AX hex text.

The native observer applies live Reduce Motion preferences to theme transitions/texture sequences,
including the first frame after resource publication. Increase Contrast adds stronger chrome
boundaries; it does not rewrite custom syntax palettes. Actual VoiceOver speech, native alerts,
window focus and screen-coordinate alignment require human review. No GUI/Metal run is claimed.

## Verification and performance evidence

`validation-round13.json` records the final strict checks, twenty consecutive default-parallel
workspace runs, per-run log hashes and source fingerprints. `timings-round13.json` and
`round13-normal.jsonl` record the paired normal-file CPU gate against the frozen main executable.
The allowed p50 regression is **+5%**, for both the paired ratio and ratio-of-process-medians
95% confidence upper bounds. p95 is retained as diagnostic evidence. Final values and the retained initial uncertainty failure appear below.

Headless coverage includes UTF-16/surrogate/combining/newline/empty/overflow queries, bounded huge
values and range translation, primary selections, actual layout bounds, role/name/focus ordering,
panel/list/modal semantics, visible clipped popup links, system motion/contrast behavior, every
resource family on package mutation, preservation of buffers/undo, serial preferences edits,
explicit index availability and plugin support markers. Core crash/package subprocess/TLS tests
continue to run as part of every workspace pass.

## Exact reviewer commands

Run the headless checks from this worktree (fresh output directories are required):

```sh
python3 crates/velocity/scripts/validate-round13.py \
  --output-dir /tmp/velocity-r13-review-checks
```

That executes the following checks with `RUST_TEST_THREADS` removed from the child environment;
it retains each of the twenty full-workspace test logs and checks the IPC fixture's result:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
# Twenty consecutive times, with default test parallelism:
env -u RUST_TEST_THREADS cargo test --workspace --locked --release
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked
cargo build --release --locked -p velocity
```

The frozen baseline was built from the clean `85e6cc1` worktree before edits and copied to
`/tmp/velocity-round13/velocity-before`; its SHA-256 is
`715d9b3828baa5bf68aef2f0c7a10e3adee2281ee1afa221ac138848167828da`.
Run the normal-file gate serially, after compilation/tests finish, with no other benchmark active:

```sh
python3 crates/velocity/scripts/measure-normal.py \
  --before /tmp/velocity-round13/velocity-before \
  --after target/release/velocity --pairs 10 \
  --output-dir /tmp/velocity-r13-review-ab
```

Native review, outside the sandbox on an unlocked macOS 14+ desktop:

```sh
scripts/package-macos.sh /tmp/velocity-r13-bundle
zsh crates/velocity/scripts/review-round13-gui.zsh \
  /tmp/velocity-r13-bundle/Velocity.app /tmp/velocity-r13-review
```

Follow [the round-13 VoiceOver/release checklist](MANUAL_TEST.md#round-13-release-wiring-and-voiceover-human-review-required),
including the explicit synthetic crash-report setup before checking next-launch prompting.

## Final measured results

Strict fmt, clippy (`--workspace --all-targets -- -D warnings`), rustdoc (`-D warnings`) and
release build passed. Twenty consecutive unchanged-source workspace runs passed **495 checks
each** at default parallelism. A final app-only inventory/message follow-up passed another
**496 checks**, plus the strict checks and release build. Its only Rust change is `release.rs`;
the IPC source hash is identical to the twenty-pass gate. The added case covers menu-only
packages and command palette contributions missing from the core seven-family inventory.

| CPU metric vs main 85e6cc1 | Before median p50 ms | After median p50 ms | Median paired change | Paired 95% CI | Ratio-of-medians 95% CI | Budget |
| --- | ---: | ---: | ---: | --- | --- | --- |
| Typing | 0.435708 | 0.424396 | -3.73% | -8.49% … -1.82% | -7.26% … -1.82% | +5% upper bound: pass |
| Scroll | 0.287240 | 0.287739 | +0.68% | -1.48% … +2.51% | -1.98% … +2.15% | +5% upper bound: pass |

The before/after columns are medians across processes; the paired percentage is the median
of per-pair ratios, so it need not equal the ratio of those two columns. Typing uses **10 pairs**;
scrolling uses **30 pairs**. Both series alternate AB/BA, with one process at a time and no
concurrent compilation/tests. All 80 process records and their frame samples are retained.

The original ten-pair scroll result was +2.42% with a maximum p50 confidence upper bound
of +6.70%, so it failed the uncertainty gate. Twenty more predeclared scrolling pairs used
the same frozen executables and the same bootstrap method. Every original sample remains
in the pooled result: +0.68%, with a maximum upper bound of +2.51%. No candidate code changed
during these measurements. The initial failed gate is retained in `timings-round13.json`.

Diagnostic paired p95 changes: typing -41.72% (-63.76% … -5.18%); scrolling +1.21%
(-1.18% … +6.09%). The tracked gate is p50; no native GPU, input-to-scanout or VoiceOver speech
measurement is claimed.

To reproduce the retained extension after the ten-pair command above:

```sh
python3 crates/velocity/scripts/extend-normal-scroll.py \
  --previous /tmp/velocity-r13-review-ab --pairs 20 \
  --output-dir /tmp/velocity-r13-review-ab-extended
```

The extension verifies the frozen binary and original runner hashes, preserves the original
typing result, and pools every original scrolling pair with the additional observations.
