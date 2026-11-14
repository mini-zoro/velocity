# velocity

Native macOS plain-text editor, lane A round 5. MIT OR Apache-2.0; macOS 14+.
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
and shortcuts. The plain-text `Highlighter` and scope provider are the syntax integration seams.
Snippet field navigation and wrap_lines remain deferred. Layout is an unwrapped
monospace grid; complex scripts, keyboard layouts and candidate geometry need human visual QA.

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
`#term` searches words fuzzily. Symbol navigation has a provider hook awaiting next-round syntax
integration; the status bar correctly remains Plain Text in this round.

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
