# velocity

Native macOS editor, lane A round 7. MIT OR Apache-2.0; macOS 14+.
See [RUNBOOK.md](RUNBOOK.md) for exact checks and hardware measurements, and
[MANUAL_TEST.md](MANUAL_TEST.md) for native input/editing acceptance.

```sh
cargo build --release -p velocity
./target/release/velocity
./target/release/velocity first.txt second.txt:8:3
printf 'hello\n你好\n' | ./target/release/velocity -
```

Velocity now opens and saves real files with UTF-8/BOM, UTF-16LE/BE, fallback encodings,
majority line-ending detection and permission-preserving atomic saves. Files above 64 MiB
load on workers with a progress tab. External changes reload clean views or prompt for dirty
ones; notify has a bounded metadata fallback for restricted sessions.

Editing uses vel-buffer's shared transactions, multi-selection model, grouped undo/redo and
soft history. Keymaps, menus and IME commits enter the Sublime-named command layer. IME marked
text is a transient primary preview; committing inserts once per caret, while an explicit
native replacement range targets that region. Tabs keep their document, history, settings
and viewport. Native Open/Save dialogs and Save/Cancel/Discard alerts complete file workflows.

Settings and keymaps use comments/trailing-comma JSON, loose/zip resources and User overrides.
Our resources/Packages/Default files are original. See the runbook for precedence, data paths
and shortcuts. Highlighting uses versioned workers over the shared buffer. Snippet field navigation and
wrap_lines remain deferred. Wrapping, folds and native IME geometry share a grapheme-based
monospace layout; complex scripts and keyboard layouts still need human visual QA.

`--bench-panels`, `--bench-startup`, `--bench-scroll`, `--bench-typing --carets 1|1000`, `--bench-memory` and
`--bench-open file` exit automatically. Incremental JSONL separates actual scanout, callback
receipt, skipped drawables and present-return. Startup retries the prepared frame until a
positive presentedTime and checks asleep/locked/occluded conditions instead of relying on the
long watchdog. `--headless-smoke --carets 1|1000` tests the keymap/command/buffer/CPU scene path
without AppKit/Metal initialization. Benchmarks retain the bundled 5,000-line workload;
interactive launch starts with an empty document.

[RESULTS.md](RESULTS.md) contains CPU evidence and the hardware review template. GUI startup,
input-to-present, GPU timing, footprint and native behavior are still pending reviewer runs.

Round 5 adds Goto Anything, the command palette, find/replace and incremental find, streaming
Find in Files, an ignore-aware project side bar, scrolling/reorderable tabs and a status bar.
Inputs use vel-buffer editing, undo and native IME. Goto previews use one transient tab;
Escape restores the source selection, Enter pins it, and Cmd/Ctrl+Enter creates a simple split.
`#term` searches words fuzzily. Cmd+R lists current-file symbols from syntax metadata, with transformed names and previews.
The status bar displays the detected syntax and opens its picker when clicked.

Cmd+P / Cmd+Shift+P / Cmd+R / Ctrl+G navigate; Cmd+F / Cmd+Alt+F / Cmd+Shift+F open search;
Cmd+G / Cmd+Shift+G find next/previous; Cmd+E / Cmd+Shift+E seed find/replacement;
Cmd+K, Cmd+B toggles the side bar. See the runbook for flags and remaining bindings.
`.sublime-project` folders/settings and the `open_dir`/`add_folder` commands add project roots.
The palette also includes menu-derived commands with binding hints and session MRU ordering.
Find Results is a virtualized bottom view with headers, numbered context, highlights and jump
locations. Replace in Files prepares its plan on a worker and requires native confirmation.

`cargo run --release -p vel-search --example search_bench` measures 100k-path ranking and
100 MB rope search without AppKit/Metal. `--bench-panels` is the automatic native counterpart:
100 scripted actions over 100k palette candidates, then JSONL completion and process exit.
Like all native modes it retains display preflight and a watchdog. No GUI result is claimed
from headless measurements. See [vel-ui](../vel-ui/README.md) and [vel-search](../vel-search/README.md)
for widget classes, ranking goldens, regex differences, cancellation and replacement guarantees.


Round 6 integrates 203 syntaxes (including our original Plain Text fallback), syntax settings,
Set Syntax palette entries, original auto dark/light schemes and token font decorations.
Cmd+/ toggles line comments; Cmd+Alt+/ toggles block comments. Enter uses language indentation
rules, and Reindent runs on a worker with stale-result protection. Matching brackets and tags
ignore string/comment scopes. Selector bindings wait for current-version scopes.

Word wrap supports a viewport width or configured column width, continuation indentation,
CJK/emoji graphemes and visual Up/Down/Page movement. Indentation/bracket/block-scope folding
uses tracked buffer regions; fold markers, Fold by Level, Unfold All and Fold Tag Attributes
are available. The minimap paints styled token rectangles, tracks the viewport and supports
click/drag scrolling. Settings control whitespace, guides, rulers, line padding/numbers,
caret animations, margins and scrolling. See the round-6 section of RUNBOOK.md for exact
CPU and reviewer GUI commands, evidence boundaries and lane-B API requests.

`--headless-view-features` automatically measures the actual 10 MB Rust first screen, a
3840×2160 logical CPU viewport with highlighting/minimap, typing, 100k-line rewrap and real
fold/unfold commands. It does not initialize AppKit, fonts or Metal. Native Open now waits
for positive scanout of an exact highlighted viewport; Scroll reports isolated minimap GPU
pass timings when timestamp queries are available.

`python3 crates/velocity/scripts/measure-views.py --runs 5` records five sequential CPU runs
with one copied release binary, raw JSONL, environment/source hashes and pooled quantiles.


Round 7 adds persistent pane layouts (Single, Columns 2/3/4, Rows 2/3, Grid 4), draggable
splitters, independent tab bars, neighboring-group commands and clone views with shared buffer
undo and view-local selections/scroll. Cmd-click/Shift-click select tabs for side-by-side display
within a group; the sidebar and Goto Cmd-Enter support additive selection. Selected sheets can
move between groups or close together. Cmd-Shift-N opens another native window in this process.
Full screen and distraction-free mode use the View menu; the latter loads its own settings layer.

Sessions save on a debounced worker and flush on quit. Dirty/untitled text, normal command
undo/redo, folds, selections, syntax overrides and window/project UI return on launch; hidden
clean tabs load lazily. Missing, read-only and corrupt data directories have regression coverage.
The undo adapter's exact fidelity limits and requested vel-buffer serialization API are in
[vel-session](../vel-session/README.md); private soft history/edit-owner IDs are not serialized.

Project Open/Save As/Close/Edit/Recent/Quick Switch and `.sublime-workspace` files are wired.
`velocity -n/-a/-w`, `--project` and stdin use a local socket and a single native application;
`-w` completes when the requested sheets close. Build/Cmd-B, Build With/Cmd-Shift-B, Cancel,
F4/Shift-F4 and output double-clicks run/navigate `.sublime-build` and project-inline definitions.
The output panel streams both pipes, supports syntax/wrap and displays exit status/time; error
locations receive simple text badges. Try the original `fixtures/workbench.sublime-project`.

`--headless-workbench` measures a 50-file session (10 hidden 11 MB files), lazy first-window CPU
restore, and 300 typing/scroll samples with four highlighted Rust viewports. It initializes no
native fonts, window or GPU. `scripts/measure-workbench.py --runs 5` retains one binary, source
fingerprints, raw JSONL, individual runs and pooled quantiles. `--groups 4` also applies to native
typing/scroll modes; `--bench-startup --restore-session --data-dir PATH` measures actual session
startup scanout outside the sandbox. Exact commands and pending native checks are in RUNBOOK.md.
