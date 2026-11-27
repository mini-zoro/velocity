# Lane A, round 14

Implemented Finder/Dock/window opens, resource-defined native context menus, and the five
round-12 native fixes. The system UI font correction (`vel-render/src/native.rs`) was prepared
as a patch because `vel-render` belonged to lane B this round; the reviewer applied it at merge.

## Opening documents

| Entry point | Behavior |
| --- | --- |
| Finder Open With | `application:openURLs:` / legacy `application:openFiles:` enqueue literal file paths; text/binary/huge files retain existing loaders and folders become sidebar roots. |
| Double-click | The same native event route after an explicit user file association. Bundle declarations remain Alternate handlers. |
| Dock file/folder drop | Native application delegate queues batches; batches waiting for startup are drained in arrival order before calling a destination that may forward and exit. |
| Window drop | The root content view accepts file URLs across editor, tabs, and sidebar; non-file text retains the existing behavior. |
| Running instance | The current key host receives opens; canonical duplicate files focus their tab without changing text, selection, or scroll. With no live windows, a fresh window is created. CLI requests continue to use the existing profile socket. |

LaunchServices detection is path-free interactive mode plus parent PID 1 or a valid legacy
`-psn_<high>_<low>` argument. That process runs the GUI in-process to retain the Apple-event
recipient. CLI files/stdin and measurement modes retain their previous path. An unexpected
secondary native process waits for its real paths, forwards them with `native_open: true`,
then exits after acknowledgement. If the primary exits first, it claims the socket and
restores/reserves every recovered window before permitting session writes. Older IPC clients
omit `native_open` and preserve CLI line/column semantics.

The last closed handler is retained as a Dock/IPC template. Its display link stops; a 100 ms
idle callback polls IPC without rendering. Closing a window releases its CLI `-w` waiters.
Missing/unreadable paths report errors without preventing later valid paths from opening.

## Context menus

Clean-room resources: [editor](resources/Packages/Default/Context.sublime-menu),
[tab](<resources/Packages/Default/Tab Context.sublime-menu>),
[sidebar](<resources/Packages/Default/Side Bar.sublime-menu>). Default/package/User layering,
loose-over-archive precedence, stable IDs and ignored-package filtering remain available.
`velocity_dynamic` inserts spelling, diff-hunk and configured Git actions.

All items receive their resource args plus `paths`, clicked `group`, group-local `index` and
window-local `event: {x,y}`. Internal sheet/version metadata protects targets during worker
delivery and tracking, including nested `chain` commands. Enabled/visible/checked state uses
the same effective args as dispatch. Native tracking begins after the mouse callback and
host borrow end; action dispatch occurs after tracking returns. Context menus never call
`set_menu`. The reviewer menu-bar installation cache and display-link coalescing remain.

| Surface | Commands |
| --- | --- |
| Editor | `cut`, `copy`, `paste`, `select_all`; existing spelling/diff/Git commands; `copy_file_path`, `reveal_in_side_bar`, `reveal_in_finder`. A click in any selection preserves every region; an outside click moves the caret. Diff commands target the pointed hunk while preserving selection. |
| Tab | `close_file`, `close_other_tabs`, `close_tabs_to_right`, `close_unmodified_tabs`, `new_file`, and the same path/reveal commands. Close targets follow the clicked group's visual order and existing dirty prompts. |
| Sidebar | `side_bar_open`, `side_bar_new_file`, `side_bar_new_folder`, `side_bar_rename`, `side_bar_delete`, `copy_file_path`, `copy_relative_path`, `reveal_in_finder`, `side_bar_find_in_folder`, `side_bar_remove_folder`, plus existing Git commands. |

Names use the existing input panel. Delete requires confirmation and calls
`NSFileManager::trashItemAtURL`; there is no unlink fallback. Rename retains open buffers,
undo and shared sheets, retargets descendants and re-arms watchers. Deleted clean buffers
become dirty/deleted and survive Hot Exit; saving clears shared and lazy deletion markers.
Root removal updates the project atomically and forces a fresh reload after an in-flight
resource worker. Errors surface in the status bar or existing dialogs. Native menus own
their accessibility; the duplicate custom context list is suppressed in painting and AX.

## Round-12 fixes and telemetry

| Bug | Result |
| --- | --- |
| `file:line[:col]` not revealed | One-shot centering after final pane geometry/style, including initial load, background completion and IPC. |
| Black sidebar cap | Paint the cap using the same theme surface as the sidebar, in light/dark themes and at UI scale. |
| Debug title | File name, dirty/deleted markers and optional ` — project`; cached native setter skips unchanged text. |
| System UI font falls back to Times | Applied by the reviewer in `vel-render/src/native.rs` `font_location`: system aliases and private `.SFNS` names use `CTFontCreateUIFontForLanguage`; other names keep `CTFontCreateWithName`. |
| Telemetry writes into build tree | Off by default. `--log-input`, `--output` and measurement modes opt in. |

Without explicit `--output`, **every measurement mode** creates
`interactive-<runid>-<pid>.jsonl` under nonempty `$VELOCITY_LOG_DIR`, otherwise
`~/Library/Logs/Velocity/`. JSON records retain the actual mode. Existing files are never
overwritten; missing runtime destinations produce an error. No compile-time source path is
a default sink. Config/package stamp traversal runs at most every 500 ms; explicit reloads
bypass the interval.

## Verification and measurements

Final gate logs and paired raw data are retained under `output/round14-20261116/` in this
worktree. The baseline binary was built from a clean `git archive 4b65f50` and frozen before
lane-A builds. Its submodule and fixture resources use the same unchanged checkout data.
The paired scripts freeze/hash both executables, alternate AB/BA and run sequentially with
no concurrent compilation. Results below cover CPU work; AppKit tracking, Finder/Dock,
fonts, GPU/scanout, VoiceOver and real trackpad budgets require native review.

All required checks passed: `cargo fmt --all -- --check`, strict workspace/all-target Clippy,
`cargo test --workspace --release` with default parallelism (**588 passed, 1 opt-in benchmark
ignored**), and strict workspace rustdoc. The final velocity recheck passed **185 tests** after
its last sidebar scale adjustment. The three plist contract tests pass with system Python.
No packaging script ran. [Verification record](output/round14-20261116/verification.json).

| CPU metric | Baseline | Lane A | Budget/result |
| --- | ---: | ---: | --- |
| Typing p50, 10 process pairs | 0.4199 ms | 0.4227 ms | ≤2 ms; paired change **+0.46%**, maximum 95% CI upper **+4.41%**: p50 A/B gate passes. |
| Typing p95 | 0.7170 ms | 0.5829 ms | ≤4 ms; relative tail interval remains too wide for a ≤5% claim. |
| Scroll scene p50, 20 process pairs | 0.2872 ms | 0.2907 ms | Paired change **+0.67%**, maximum 95% CI upper **+4.42%**: p50 A/B gate passes. |
| Scroll scene p95 | 0.3701 ms | 0.3943 ms | ≤2 ms; ratio of process medians **+6.56%**. A ≤5% p95 result is **not established**. |

The scroll p95 ratio-of-medians 95% interval is approximately **−1.18% to +11.40%**.
Its paired median change is +5.79%, with interval −0.19% to +10.13%. These results do not
establish either a stable regression or a ≤5% tail bound. The original ten pairs and the
fixed ten-pair extension are all retained, including outliers. Native trackpad/AppKit/scanout
review remains necessary. A default embedded-only stamp helper had zero filesystem stamps;
its timings are not used as performance evidence.

Raw evidence: [typing](output/round14-20261116/typing/summary.json),
[initial scroll](output/round14-20261116/scroll/summary.json),
[all 20 scroll pairs](output/round14-20261116/scroll-20-pairs/summary.json),
[combined metrics](output/round14-20261116/performance-summary.json).
The final binary SHA-256 is
`d023514e626abd0d44f6e6061811acb15b2ddabc6c4db39e06d171ac1b5fc45f`.

Exact paired commands used from the repository root:

```sh
/usr/bin/python3 -B crates/velocity/scripts/measure-typing.py \
  --before /tmp/velocity-r14-baseline/velocity-main-4b65f50 \
  --after crates/velocity/output/round14-20261116/velocity-lane-a-final \
  --runs 10 --output-dir crates/velocity/output/round14-20261116/typing
/usr/bin/python3 -B crates/velocity/scripts/compare-scroll.py \
  --old /tmp/velocity-r14-baseline/velocity-main-4b65f50 \
  --new crates/velocity/output/round14-20261116/velocity-lane-a-final \
  --pairs 10 --output-dir crates/velocity/output/round14-20261116/scroll
/usr/bin/python3 -B crates/velocity/output/round14-20261116/extend-scroll.py
```

The extension verifies the frozen executable hashes and adds exactly ten pairs. Existing
output directories are never overwritten; choose fresh directories for a reviewer rerun.
Alternatively, run `compare-scroll.py --pairs 20` into a new directory for a fresh 20-pair
series. No compilation or other local benchmark ran concurrently with the measured pairs.

Exact reviewer commands and the native checklist are in
MANUAL_TEST.md.
Platform-specific contracts/checks are in
[vel-platform/MANUAL_TEST.md](../vel-platform/MANUAL_TEST.md). Do not substitute headless
passing results for those native checks.

## Reviewer decision

Accepted. The reviewer applied the system-UI font patch (`vel-render/src/native.rs`) and fixed the
following findings of a correctness review before merging (tests in `context_tests.rs`,
`workspace.rs`, `theme_tests.rs`, `vel-config/tests/context_menus.rs`, `vel-fileio/tests/io.rs`):

- Rename (including a no-op rename) cleared `deleted` and reset the disk stamps / pending
  external change of every retargeted tab, so the only copy of a trashed file could close
  without a prompt. A rename now keeps the disk state; a deleted buffer stays deleted and dirty.
- Rename checked the destination and then renamed (a race that could replace a new file), and
  refused case-only renames on case-insensitive APFS. It now uses one exclusive
  `renameatx_np(RENAME_EXCL)` call (`vel_fileio::rename_no_replace`).
- `Workspace::close` moved the active tab when an earlier tab closed; tab-menu closes ignored
  `close_windows_when_empty`.
- Detach Project Folder rewrote relative project paths as absolute paths; renaming a relative
  project root left a ghost root after the next reload. Project files are now edited as
  written (`Folder::follow_symlinks` is optional; empty lists are not serialized).
- Click context arguments replaced concrete resource arguments (`{"group": 1}`); they now fill
  only absent keys and the `-1` / `[]` / `null` placeholders.
- Side bar paths below a symlinked folder did not match the canonical tab paths
  (`context_menu::canonical_entry`).

Performance: a paired headless run at review time showed scroll p95/p99 and typing
regressions, but the machine ran at load average 13-42 on 8 cores and the outliers were
bimodal (threads on efficiency cores). The code on the benchmark paths is unchanged (only
`View::center_caret` was added and new window commands are registered), and whole-process
retired instructions (`/usr/bin/time -l`) match main within 0.5% (features 240.2 vs 241.0 G,
1,000-caret typing 10.63 vs 10.66 G; lane A first). Forwarded to lane B: forwarder process
edge cases, which tab activates after closing the active one, trashing a project root.
