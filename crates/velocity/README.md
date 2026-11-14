# velocity

Native macOS plain-text editor, lane A/P1 round 4. MIT OR Apache-2.0; macOS 14+.
See [RUNBOOK.md](RUNBOOK.md) for exact checks and hardware measurements, and
[IME_TEST.md](IME_TEST.md) for native input/editing acceptance.

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
Palette UI, snippet field navigation and wrap_lines remain deferred. Layout is an unwrapped
monospace grid; complex scripts, keyboard layouts and candidate geometry need human visual QA.

`--bench-startup`, `--bench-scroll`, `--bench-typing --carets 1|1000`, `--bench-memory` and
`--bench-open file` exit automatically. Incremental JSONL separates actual scanout, callback
receipt, skipped drawables and present-return. Startup retries the prepared frame until a
positive presentedTime and checks asleep/locked/occluded conditions instead of relying on the
long watchdog. `--headless-smoke --carets 1|1000` tests the keymap/command/buffer/CPU scene path
without AppKit/Metal initialization. Benchmarks retain the bundled 5,000-line workload;
interactive launch starts with an empty document.

[RESULTS.md](RESULTS.md) contains CPU evidence and the hardware review template. GUI startup,
input-to-present, GPU timing, footprint and native behavior are still pending reviewer runs.
