# Round 17 Lane A final verification fixes (native review pending)

The latest regressions use `App::run_named`, real key dispatch and the package/resource
loader. Evidence for this pass is in `/tmp/velocity-lane-a-r17-final-review/`; earlier
measurements below are historical. Syntax cache schema `velocity-syntax-roots-v1-13`
invalidates cached diagnostics that lack an originating resource path. Native checks
remain unobserved in this sandbox.

The final gate passes: fmt, strict release/locked workspace clippy over all targets,
`cargo test --workspace --locked --release` at default parallelism (**807 passed, 0 failed,
4 ignored**, 114 test executables), and rustdoc with `-D warnings`. The standalone syntax
runner reports **126,600 passed / 0 failed / 0 skipped**, including **35/35 reindent
variants** (203 syntaxes, 242 files), unchanged from the incoming verified candidate. The
first gate attempt hit the existing TLS fixture's empty port-file publication race in
`vel-packages/tests/https.rs:74`; the unchanged full rerun passes. Both logs are retained.
No golden content, insert-session code or Lane B drawing/platform files changed in this pass.

Fresh five-run medians against frozen main `99d1bb4` (headless CPU only):

| Metric | Main | Final candidate | Change |
| --- | ---: | ---: | ---: |
| syntax parse MB/s | 5.043145 | 5.000526 | -0.85% throughput |
| full highlight MB/s | 3.753227 | 3.772233 | +0.51% throughput |
| highlight edit p95 ms | 0.002459 | 0.002459 | unchanged |
| completeness query ns (nine pairs) | 0.954861 | 0.954861 | unchanged |
| cached syntax load ms, including content hashes | 11.383291 | 11.288042 | -0.84% time |
| YAML load ms | 871.061333 | 866.217666 | -0.56% time |
| 100-line syntax window ms | 0.010500 | 0.010375 | -1.19% time |
| convergence ns | 125 | 167 | +42 ns (+33.6%), one clock tick |

Throughput remains within 3%, and cached loading does not slow down. The convergence
sample spans three versus four clock ticks; the measured Mach timebase is 125/3 ns per
tick. Two alternating complete syntax-benchmark pairs use **207,762,213,832 →
204,935,798,129 retired instructions (-1.36%)**, within 3%, counted with Darwin
`proc_pid_rusage(RUSAGE_INFO_V4)` after exit and before reaping. These are whole-process
totals, including YAML/cache loading, warmup, five 10 MB parse passes, windows and convergence;
they do not isolate the convergence case. Raw wall-clock samples and the one-tick increase
remain in the evidence. No throughput measurement was discarded or rerun.

Twenty-one alternating typing pairs all pass the 5% gate, milliseconds:

| Bench | p50 main → candidate | p95 main → candidate | p99 main → candidate |
| --- | --- | --- | --- |
| typing1 | 0.200020 → 0.200230 (+0.10%) | 0.347796 → 0.345809 (-0.57%) | 7.117584 → 7.104674 (-0.18%) |
| typing1000 | 5.244021 → 5.251167 (+0.14%) | 14.574602 → 14.527039 (-0.33%) | 27.041032 → 26.992232 (-0.18%) |

Logs, samples, load conditions, timer resolution, binary identities and the focused patch
are in `/tmp/velocity-lane-a-r17-final-review/`. The candidate SHA-256 is
`3be783265c86a85682db88c9775bcf938ef4822dae5b5b251c03395261a1a7f5`; the typing baseline is
the frozen `/tmp/velocity-lane-a-r17/velocity-main-99d1bb4` binary. Benchmark-generated tracked
timing files were restored after measurement. All changes remain uncommitted; no GUI,
packaging script or `cargo clean` was used.

- [ ] **Cold/stale tab conversion:** select Bash and immediately run Expand Tabs. Repeat
  after highlighting finishes, insert `#`, and immediately run it again. Test Unexpand Tabs
  too. In a `<<-EOF` heredoc, content and tab-prefixed terminator bytes remain unchanged;
  code indentation converts in one undo step. Include a Ruby embedded heredoc whose target
  falls back to Plain Text. Repeat with Find focused.
- [ ] **Literal reindent:** enable spaces and reindent a function containing a case branch
  with `cat <<-EOF`. Mixed tab/space content and `\tEOF` remain byte-identical. Try Python
  triple strings and C++ raw strings too; ordinary following code still reindents. A bare
  multiline JavaScript template must not shift the following function.
- [ ] **Reachable, non-modal diagnostics:** launch from Finder with two missing references
  in a User syntax, malformed mousemap/scheme entries and a corrupt installed package.
  Status summaries name the first offending file and `Velocity.log`; no warning opens an
  alert. Open `<data-dir>/Velocity.log` with File > Open and check every filename and reason.
  The default macOS path is `~/Library/Application Support/Velocity/Velocity.log` (unless
  `--data-dir` or `VELOCITY_DATA_DIR` overrides it). Restart and reload resources: details
  append to the same log, and healthy neighboring resources still work. A bundled-only
  profile produces no warning status or alerts.
- [ ] **Warning provenance:** install a healthy User syntax named like a bundled syntax
  with a missing optional target. Bundled diagnostics must not become User status warnings.
  Add a bad reference to the User syntax: only its own warning is counted. Repeat using
  loose packages and Installed Packages, then restart from the cache.
- [ ] **Empty Vintage repeat:** after `dd`, try `i<Esc>.`, `A<Esc>.` and `R<Esc>.`.
  Dot repeats the empty insertion; it must not delete another line. Undo restores `dd`
  without a phantom step. The fix is at the command boundary; insert-session code is untouched.
- [ ] **Shell lexical cases:** reindent Bash and Zsh branches containing `${#a[@]}`,
  `${x:-y}` and nested parameter substitutions. Subsequent `;;` and patterns stay at the
  correct level. A one-line `case $x in a) echo a;; esac` inside a function must not deindent
  the next statement or the function's closing brace.

# Round 17 Lane A earlier review fixes (historical headless evidence)

All eight reviewer findings have dedicated headless regressions. The implementation remains
uncommitted. The original round-17 evidence below is retained; validation and measurements
for this earlier review pass are in `/tmp/velocity-lane-a-r17-review/`.

Headless validation for the review fixes: fmt, strict release/locked workspace clippy over all
targets, `cargo test --workspace --locked --release` at default parallelism (**800 passed,
0 failed, 4 ignored**) and rustdoc with `-D warnings` pass. The syntax suite remains
**126,600/126,600**, including **35/35 reindent variants** (203 syntaxes, 242 files).
The three legacy scalar forms load and highlight through both fresh and cached real package
loading, beside a corrupt neighboring archive. Warnings are retained in `SyntaxSet::warnings()`;
only the presentation channel changes. The cache schema for that pass was `velocity-syntax-roots-v1-12`
so older linked references cannot survive this fallback repair.

Fresh five-run medians against frozen main `99d1bb4` (headless CPU only):

| Metric | Main | Review candidate | Change |
| --- | ---: | ---: | ---: |
| syntax parse MB/s | 4.732084 | 4.754804 | +0.48% throughput |
| full highlight MB/s | 3.533281 | 3.513311 | -0.57% throughput |
| completeness query ns (nine pairs) | 1.041667 | 0.984028 | -5.53% time |
| cached syntax load ms, including content hashes | 11.769500 | 11.712000 | -0.49% time |
| YAML load ms | 921.537791 | 923.119083 | +0.17% time |
| 100-line syntax window ms | 0.011042 | 0.010666 | -3.41% time |
| convergence ms | 0.000125 | 0.000125 | unchanged |
| highlight edit p95 ms | 0.002542 | 0.002625 | +3.27% (+83 ns) |

The machine's one-minute load was 6–12 on eight cores during these wall-clock measurements;
the near-threshold edit sample is supplemented with retired-instruction measurements below.
Throughput stays within 3%; cached loading does not slow down. Raw logs, binary identities,
and every sample are in `before/`, `after/`, `bench-conditions.json` and `bench-summary.json`
under the review evidence directory.

Two alternating complete highlight-benchmark pairs, counted with Darwin
`proc_pid_rusage(RUSAGE_INFO_V4).ri_instructions` after exit and before reaping, give
**199,998,475,736.5 → 200,637,924,495 instructions (+0.32%)**, within 3%. The process includes
five full 10 MB passes, edit samples and syntax loading; these are process totals, not isolated
edit counters. `highlight-instructions.json` records all samples and binary hashes. The 83 ns
wall-clock edit-p95 increase above remains visible rather than being omitted from the evidence.
The unchanged structural golden also passes **12 additional runs in four parallel processes**;
`golden-repeats.json` and individual logs retain those results.

Twenty-one alternating typing pairs all pass the 5% gate, milliseconds:

| Bench | p50 main → candidate | p95 main → candidate | p99 main → candidate |
| --- | --- | --- | --- |
| typing1 | 0.215126 → 0.215521 (+0.18%) | 0.369756 → 0.373337 (+0.97%) | 7.012600 → 7.035121 (+0.32%) |
| typing1000 | 5.420146 → 5.427042 (+0.13%) | 15.010235 → 14.985519 (-0.16%) | 27.744109 → 27.756192 (+0.04%) |

The reviewer can reproduce the gate with the commands retained in the original round-17
section below. For these measurements use `/tmp/velocity-lane-a-r17-review/paired` as the output
for `paired-headless.py` and compare against `/tmp/velocity-lane-a-r17/velocity-main-99d1bb4`.
The candidate hash is `5d88d22062e2020fa2ad6b1f7e691b265be7e0a4aad6b8fd3f584b6eb338a506`.
Direct native `--bench-open` remains unchecked in this sandbox; its message-queue prerequisite
is covered by fresh `App`/`Workspace` loading, cache reuse and resource reload tests. No GUI,
packaging script or `cargo clean` was used. The previously unverified implicit Shift+multi-click
and Cmd+plus defaults remain unchanged; explicit mousemap mappings are covered headlessly.

- [ ] **Launch diagnostics:** launch a fresh profile twice; no syntax alerts or warning status.
  Install a User syntax with two missing references: one status naming its file and
  `Velocity.log`, with both details in that log. Repeat with bad scheme globals, a User
  override of a bundled scheme filename and two malformed mousemap entries; healthy values
  and bindings work, with one non-modal summary per resource kind. Reload and restart from cache.
- [ ] **Shell reindent:** inside a function, reindent `case "${1}" in`, `${x:-y}` and `${#a[@]}`.
  Clauses remain at column 4, bodies at 8. Put a function inside a case branch: its brace stays
  at 4 and `esac` at 0. Try brace-style Zsh cases, including a single-line case followed by another.
- [ ] **Compatible user grammars:** load quoted capture indices, an empty `scope:` and numeric
  extensions `[man, 1, 2, 3]`, next to a broken package. Select each syntax, check highlighting
  and reopen a `.1` file. Only discarded malformed data should produce warnings.
- [ ] **Fold ink:** use a foreground/background-only scheme or legacy tmTheme, fold a selection
  and move the caret away. The ellipsis remains visible. Missing borders remain optional.
- [ ] **Vintage repeat:** `dd`, then `rb` on `b`, `~` on a digit or `guu` on lowercase text,
  then `.`. Dot repeats the attempted no-op; Undo restores the deletion without a phantom step.
- [ ] **Missing embeds:** open a Ruby `<<~'HTML'` heredoc without its embedded HTML syntax.
  Inspect `text.plain` inside it; highlighting resumes after the terminator. A missing context
  within an installed syntax stays empty and its escape still works.
- [ ] **Indentation focus/content:** leave Find focused while invoking all three indentation
  commands. They act on the document and retain the query/focus. With cold, current or stale
  scopes, convert a shell `<<-EOF` heredoc in either direction: content and terminator tabs remain byte-identical.
  Also try a multiline string, Undo/Redo and read-only documents.
- [ ] **Theme fixture:** compare native light/dark phantom, popup and annotation scenes. The
  headless golden now explicitly snapshots the cold dark frame and waits for the matching
  diff publication before the light frame; its frozen JSON content is unchanged.

# Round 17 Lane A: mousemaps, schemes, syntax and indentation (native review pending)

Run a reviewer-built release binary on an unlocked macOS desktop with a disposable profile.
No GUI, AppKit input, Metal, packaging script or `cargo clean` is used in this lane. Keep
these boxes unchecked until observed. Lane B owns native selection corners, bracket glyph
styles/glow, drag delay, scrollbar paging, fonts, minimap and window-state drawing.

- [ ] **A1 mousemaps:** install an original User mousemap with `button1`, numeric buttons,
  `button2`, `button16`, `scroll_up`/`scroll_down`, press/release commands and contexts. Add
  one malformed entry and an unrelated corrupt installed archive. Healthy entries still
  work. Try `drag_select` with `by: words/lines`, `extend`, `additive`, `subtractive`, drag
  past the viewport and release. Secondary/Control-click overrides take precedence when
  configured; an empty mousemap keeps the existing native/context/column-selection behavior.
- [ ] **A1 focus / A2 verification limit:** click/scroll Find and the command palette with
  document-editing mouse bindings installed: document text stays unchanged. Click back into
  the document: its binding runs. Explicit Shift+double/triple-click mappings using
  `extend: true` retain the anchor and extend by word/line in either direction. The implicit
  Shift+multi-click default remains unchanged because the allowed docs do not establish its
  anchor rule; compare it with the reviewer's current editor before proposing that change.
- [ ] **D1/D2/D6 schemes:** select a minimal legacy tmTheme and an explicitly assigned
  `.hidden-tmTheme`. The latter stays out of the picker. Gutter/line numbers, caret line,
  whitespace, guides, rulers, find matches and diffs remain legible. Missing borders are transparent; folded ellipses retain foreground ink. Add bad colors/variables/rules alongside valid ones and change
  numeric-string radii/widths and corner enums; healthy properties survive with diagnostics.
- [ ] **D3 palette:** compare our light/dark schemes and theme with main, including dimmed
  inactive sheets. Colors must look unchanged. User expressions follow the corrected base
  percentage: black `blend(white 25%)` is RGB 191; the inverse is RGB 64. Compare alpha blends.
- [ ] **E1–E6 syntax:** install original parent/child grammars with anonymous contexts,
  embed wrappers and prototypes. Add one malformed grammar and an unrelated corrupt archive.
  Healthy grammars still load on the first launch and on cache reuse. Highlight deeply nested
  JSON/JavaScript, including closing delimiters after the resource limit. Missing references
  warn at load time and cannot abort the highlighting worker. Check diagnostics for unknown
  variables, malformed regexes and wrong value types, then fix/reload the individual file.
- [ ] **E7 reindent:** reindent C/C++ strings/comments containing bracket/comment characters,
  CSS comment-led lines, JS switch cases/templates, JSX attributes with quoted/interpolated
  `>`, shell cases, PHP transitions and Python dedents. Include embedded languages and User
  indentation preference overrides. Undo once; async completion must not overwrite later edits.
- [ ] **F1/F2 commands:** use View > Indentation (Lane B's rows) for `expand_tabs`,
  `unexpand_tabs`, `detect_indentation`. Try mixed leading whitespace, interior tabs, multiple
  directed selections, clones, read-only files and a large buffer. Conversion is one undo
  step; repeat conversion is a no-op. `set_translate_tabs: true` updates only this view's
  typing style. `set_setting {setting, value}` accepts arbitrary JSON on the view layer.
- [ ] **F3 keys:** Ctrl+Cmd+Shift+F toggles distraction free and its existing alias still
  works. Ctrl+Option+Up/Down scroll by a line; Ctrl+Shift+Up/Down still add selection rows.
  Repeat with Find focused. Cmd+= keeps zooming; Cmd+plus is unchanged because its alias
  was not confirmed in the allowed documentation. Test physical keys/layouts after Lane B's
  window/zoom changes are integrated.

- [ ] **Validation-found Vintage regression:** enable Vintage, delete and put back a
  combining grapheme plus Han text, apply `~` to the uncased Han character, then Undo/Redo.
  Undo targets the put rather than a phantom case-change transaction; Unicode carets stay valid.

Headless evidence and frozen main binaries: `/tmp/velocity-lane-a-r17/`.
The compiled-color goldens were captured from main 99d1bb4 before the expression change;
all explicit scheme globals, rule/gradient colors and compiled theme color properties,
including evaluated sheet modifiers in both appearances, are compared exactly.
The syntax corpus now reports the pinned tree's 16 genuine optional missing references;
its test permits only those targets, while rejecting other authoring/linking warnings. Bundled
syntax diagnostics stay in the console. User syntax, scheme and mousemap warnings each produce
one non-modal status summary, including when the syntax cache is reused.

Round-17 validation: fmt, strict locked release clippy (workspace/all targets), the full
locked release workspace tests at default parallelism (**790 passed, 0 failed, 4 intentionally
ignored**) and rustdoc with warnings denied all pass. Syntax tests improve from **126,595 passed /
5 failed / 0 skipped** to **126,600 passed / 0 failed / 0 skipped** (203 syntaxes, 242 files;
35/35 reindent variants). The checked-in corpus baseline now locks in those five fixes.

The workspace property test also found an existing Vintage no-op history bug. The new seed
and `kk2xP~u` reproduce against the frozen main sources (`vintage-main-repro.log`); `Vintage::apply`
now counts only real buffer transactions. Its deterministic Unicode regression and the saved
property-test seed pass. The scope of that small compatibility fix is limited to Vintage's
history adapter; the buffer's accepted no-op filtering remains in effect.

All 112 explicit compiled scheme colors and 882 compiled theme color properties match their
frozen pre-change values. Our minihtml fixture percentages and the theme's legacy array-tint
adapter were migrated too; their existing display-list goldens retain their original colors.

Five-run benchmark medians, main → candidate (10,000,000 bytes, 297,030 lines):

| Benchmark / metric | Before | After | Change |
| --- | ---: | ---: | ---: |
| syntax-bench parse MB/s | 4.965831 | 5.062297 | +1.94% throughput |
| highlight_bench full MB/s | 3.555764 | 3.751701 | +5.51% throughput |
| highlight_bench edit p95 ms | 0.002625 | 0.002459 | -6.32% time |
| completeness_bench optimized query ns (nine pairs) | 0.984028 | 0.954861 | -2.96% time |
| syntax-bench cached load ms, including content hashes | 11.065500 | 11.299375 | +2.11% (+0.234 ms) |
| syntax-bench YAML load ms | 865.052625 | 854.753042 | -1.19% time |
| syntax-bench 100-line window ms | 0.010375 | 0.010541 | +1.60% time |
| syntax-bench convergence ms | 0.000125 | 0.000125 | unchanged |

Syntax/highlight regressions remain within 3%; cached loading has no noticeable increase.
The bench binaries were run without concurrent compilation in this lane. These are headless
CPU measurements and do not establish native drawing/scanout performance.

Twenty-one alternating typing pairs against the frozen main binary, milliseconds:

| Bench | p50 main → candidate | p95 main → candidate | p99 main → candidate |
| --- | --- | --- | --- |
| typing1 | 0.200042 → 0.200417 (+0.19%) | 0.345681 → 0.344500 (-0.34%) | 7.057019 → 7.062445 (+0.08%) |
| typing1000 | 5.263500 → 5.266334 (+0.05%) | 14.604685 → 14.572817 (-0.22%) | 27.088861 → 27.054803 (-0.13%) |

All six typing metrics pass the 5% gate. `paired/report.json` preserves every sample and binary
identity: main `12bae0fbc43a0f23f1944d73976b3448cdd24ec5a73f8796bbaf5014ed9ed3f5`,
candidate `53fa9e79b2101506cdf1b2a4169cfab9fb4da8224c9eddfdaf48e4f71627344a`.
The complete syntax/highlight JSON and command logs are in `before/` and `after/`, with a combined
`bench-summary.json`, under `/tmp/velocity-lane-a-r17/`.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --release --locked -- -D warnings
cargo test --workspace --locked --release
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo run --release --locked -p vel-syntax --bin syntax-tests
cargo run --release --locked -p vel-highlight --example highlight_bench
cargo run --release --locked -p vel-highlight --example completeness_bench
cargo run --release --locked -p vel-syntax --bin syntax-bench
python3 tools/scroll-gate/paired-headless.py \
  --base /tmp/velocity-lane-a-r17/velocity-main-99d1bb4 --candidate target/release/velocity \
  --bench typing1 --bench typing1000 --pairs 21 --output /tmp/velocity-r17-review-paired
```

# Round 16 Lane A: editing, bookmarks/marks and macros (native review pending)

Use a reviewer-built binary with an isolated profile. The sandbox runs headless tests only;
leave every native box unchecked until review. Lane B owns the menu/palette resource rows and
bookmark gutter drawing. `Editor::bookmarks()` exposes the complete mapped regions without an
allocation; `Panels::macro_file_items()` exposes package macro invocations with our own captions.
`run_macro_file` without arguments opens that catalog as a chooser.

- [ ] **1.1 Undo:** at BOF, Backspace; select uppercase text and Upper Case; unindent a line
  without indentation. None creates a dirty marker or an undo step. Undo a real edit, run each
  no-op, then Redo: the edit returns. Repeat with Unicode, several carets, a clone, snippet
  mirrors, language highlighting and an inline annotation. A later real edit updates all of them.
- [ ] **1.2 Column regions:** Ctrl+Shift+Up/Down adds the previous/next visual row at both
  endpoints of a forward or reversed selection. Repeat through short lines, tabs, CJK, wraps
  and a collapsed fold. Follow with Up/Down: the goal column agrees, the active new region is
  revealed, and no endpoint lies inside hidden text.
- [ ] **2.1 Brackets/scopes/tags:** Ctrl+M from before/after either delimiter and inside the
  pair; Ctrl+Shift+M repeatedly selects contents then delimiters then the enclosing pair.
  Include empty/adjacent pairs, unmatched punctuation, Rust/Python/JavaScript strings and
  comments containing their own brackets. An unmatched string bracket cannot pair with code.
  Cmd+Shift+Space expands a scope. Cmd+Shift+A expands nested HTML/XML tags, including quoted
  `>` attributes and fake tags in comments. Run indentation expansion through the menu/command;
  it has no default key binding. Repeat with reversed and multiple selections.
- [ ] **2.2 Comments:** Cmd+/ on an empty HTML/CSS selection comments its whole line;
  pressing it twice immediately restores the line even before highlighting catches up.
  From inside a single/multiline block comment, Cmd+/ and Cmd+Option+/ remove its delimiters.
  Include an empty line, an indented line, multiple carets on one line, and Rust line/block
  comments. Undo restores the complete previous state.
- [ ] **2.3–2.5 Transpose/wrap/permutation:** Ctrl+T advances after swapping adjacent Unicode
  graphemes, and circulates differently sized selected strings. Cmd+Option+Q retains indentation
  and the active language's `//`, `#`, `--` comment prefixes on every output line. Plain Text
  and Markdown keep list/heading markers only on their original first line. Test positive
  `wrap_width`, zero with the first ruler, and explicit command width. Unique/shuffle/reverse
  with only carets permutes the entire buffer, including an unterminated last line.
- [ ] **2.6–2.7 Join/indent:** Cmd+Shift+J retains forward/reversed selected text and endpoint
  positions while joining several lines, including a whitespace-only intervening line. Keep
  Cmd+J as its existing chord prefix. Spacing before closing brackets and indentation of empty
  lines retain the earlier behavior, pending exact documented rules (verification note below).
- [ ] **3 Bookmarks:** Cmd+F2 stores full directed regions or empty carets. F2/Shift+F2 wraps
  between them, Option+F2 selects all, Cmd+Shift+F2 clears them; `select_bookmarks` remains an
  alias. Type before/between bookmarks, Undo/Redo, save/quit/relaunch, clone, and restore after
  an external disk change invalidates history. Regions and direction survive. After Lane B's
  drawing is integrated, verify one gutter icon on every covered bookmarked line, clipped to
  the visible gutter and following edits/folds.
- [ ] **3 Marks:** Cmd+K then Cmd+Space sets the mark; Cmd+A selects to it, Cmd+W deletes to
  it, Cmd+X exchanges it, Cmd+Y yanks the last killed text, Cmd+G clears it (second keys also
  have Cmd held). Test multiple carets, Unicode, edits before the mark and a session restart.
  Kill in one tab and yank in another; the application kill ring distributes matching counts.
  `clear_bookmarks {"name":"mark"}` clears only the mark and its primary-region metadata.
- [ ] **4 Macros:** Ctrl+Q starts/stops one application macro; switch tabs/windows while
  recording and keep the recording indicator/context. Record typing, an Add Line Before macro,
  Find Next/Prev/Under/All and a replacement; tab/window commands must stay out of the recording.
  Ctrl+Shift+Q applies each recorded command once across all carets. The find result must be
  selected before the next recorded edit. Change the visible query before playback: captured
  find options still reproduce the recording. Retest `redo_or_repeat`.
- [ ] **4 Files/UI:** after Lane B's menu rows are integrated, use Tools Record/Playback/Save
  Macro and Macros entries. Save writes JSON in the profile's `Packages/User`; an occupied
  name gets a free numeric suffix, and all existing files/backups remain intact. Save three
  recordings using the default name. Cancel the naming panel; choose an invalid name; test
  an unwritable profile. Play a macro from another loose/installed package, override/reload it,
  disable that package, and try malformed/recursive files. Errors restore recording state and
  preserve editor usability. The catalog and no-argument chooser list effective package files.

Verification limits: the public command reference confirms the first-ruler fallback and
transpose semantics. The official changelog confirms find recording (4107) and maintained
Join Lines selections (4213). Build 4166 only says that Join Lines and empty-line indentation
improved; it does not specify the exact closing-bracket spacing or blank-line rule. The public
macro guide does not specify one undo step per playback. Those three rules and the existing
70-column hard-wrap fallback remain unchanged; source comments record the references.

Headless validation and retained evidence (no native AppKit/Metal/scanout result is implied):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --release --locked -- -D warnings
cargo test --workspace --locked --release
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo test -p velocity --release --locked command_hook_timings_large_buffer_many_carets -- --ignored --nocapture
python3 tools/scroll-gate/paired-headless.py \
  --base /tmp/velocity-lane-a-r16/velocity-main-32506af --candidate target/release/velocity \
  --bench typing1 --bench typing1000 --pairs 21 --output /tmp/velocity-lane-a-r16/paired-final
```

The baseline was frozen before edits. Current `main` at `1433d8b` has identical editor/build
sources to `32506af`; its only additional changes are packaging forwarding fixtures, which
are not compiled into the compared editor. Raw timing logs, hashes and summaries are retained
under `/tmp/velocity-lane-a-r16/`. Packaging and `cargo clean` are not run for this round.

Initial submission validation: fmt, strict release clippy, the full locked release workspace tests
(default parallelism: 749 passed, 3 intentionally ignored), and rustdoc with warnings denied
all passed. The ignored command-hook timing test was then run explicitly and passed.
The existing `empty_window_sidebar_omits_input_adapter_before_and_after_preview` regression
also passed; the hidden input adapter hook was retained.

Paired CPU typing results: 21 alternating pairs, median of each run's reported percentile,
main → candidate milliseconds (the combined editing/CPU scene path; no native/GPU claim).

| Bench | p50 main → candidate | p95 main → candidate | p99 main → candidate |
| --- | --- | --- | --- |
| typing1 | 0.200896 → 0.201500 (+0.30%) | 0.349953 → 0.345797 (−1.19%) | 7.099487 → 7.157677 (+0.82%) |
| typing1000 | 5.263104 → 5.267021 (+0.07%) | 14.557619 → 14.606450 (+0.34%) | 27.084054 → 27.147966 (+0.24%) |

All six metrics pass the 5% gate. Earlier 5-/7-pair runs crossed the threshold on variable
samples; their raw data is retained in `paired-initial` and `paired-current`. The final run
above followed all builds and tests, with no concurrent compilation in this lane.
`paired-final/report.json` retains binary SHA-256 values and all paired samples.

Command hooks, milliseconds per command averaged over 20 invocations with 10,000 carets:

| Buffer | Insert empty text (no-op) | Type with 10,000 bookmarks | Type without bookmarks | Type while recording |
| --- | --- | --- | --- | --- |
| 1 MiB | 0.5096 | 1.6481 | 1.1092 | 1.0711 |
| 32 MiB | 0.3009 | 1.2454 | 0.9913 | 1.0213 |

The same probe includes one-caret cases in `hook-timings.log`. No-op comparisons touch only
the edited slices. Bookmark mapping uses the current change set and preserves region order;
macro recording appends one command independently of selection count and history length.
The review fixes below restore a stable sort after bookmark mapping; replacement-boundary
bias can reverse their order. The sort is linear for already sorted insert/delete mappings.
Bracket commands use the existing interval tree to collect only enclosing/touching pairs,
and scope expansion walks only the active contiguous occurrence, with the root in constant time.

Round-16 review corrections (all native boxes remain pending):

- [ ] Record Find Next with Cmd+G and Enter, Find Prev with Shift+Enter, then an edit. Replay
  with Ctrl+Shift+Q while a different query is focused. Compare text/carets with the manual
  sequence. Each Find records exactly once. Repeat with Replace Next followed by an edit.
- [ ] Save a user macro and invoke its Tools entry; replay package macros containing Find
  and a subsequent edit. Find finishes before the edit. A tab/window action that closes or
  changes the starting view stops playback before later edits, with status-bar feedback.
  Repeat macro-backed editing keys with the Find field focused: they edit the input, and
  the document stays untouched. Document-started playback still applies its steps to the document.
- [ ] Bookmark at columns 2 and 4 of `a\nb\nc\nd\n`; replace bytes 2..5 with `X`, then
  navigate both ways. Toggle at another column on a bookmarked line, and over a partially
  overlapping region: remove the matching bookmark. Repeat after edits move the bookmark.
- [ ] Set a mark at the caret and delete to it. No character, dirty state, undo step or kill
  entry changes. Repeat with one empty and one nonempty mark/caret pair, then Yank.
- [ ] Paste and Indent text identical to the selection, alone and alongside a different
  replacement. Every affected selection collapses as after a normal paste; only real byte
  changes dirty the document or publish layout/language changes.

The new regressions use `App::run_named` and the shared native/headless key dispatcher for
macro recording, naming-panel completion and playback. The optional native window handle
isolates clipboard/dialog/frame operations; the Registry, action queue, panel interception,
workspace/tab switching and ordered macro executor are the production implementations.
Bookmark toggling uses the review's line/overlap fallback: the public command reference says
only active regions, and the build-4166 changelog does not define the improved toggle rule.
Sources are cited in `ops::toggle_bookmark`. Legacy round-seven restore probes cover an
unsaved cursor-zero journal and no-op steps before/after cursor/save indexes.

Review validation: fmt, strict locked release clippy, the full locked release workspace
tests (default parallelism: 762 passed, 0 failed, 3 intentionally ignored), and
rustdoc with warnings denied all pass. The hook timing test passes when run explicitly;
the sidebar regression remains passing. The two existing macro-backed keymap regression
tests now drive the application dispatcher, preserving their original assertions for line
editing and empty-pair deletion. No GUI, packaging script, `cargo clean` or commit was used.

Review paired typing results, 21 alternating pairs against the **same frozen baseline**
(SHA-256 `4b105c8e6fc69e4105122828ba0261bab4b5dde99b06df96b645de94efab4383`), main → candidate milliseconds:

| Bench | p50 main → candidate | p95 main → candidate | p99 main → candidate |
| --- | --- | --- | --- |
| typing1 | 0.201146 → 0.201416 (+0.13%) | 0.346060 → 0.346509 (+0.13%) | 7.120734 → 7.078790 (-0.59%) |
| typing1000 | 5.260395 → 5.267375 (+0.13%) | 14.595638 → 14.585129 (-0.07%) | 27.107928 → 27.103455 (-0.02%) |

All six metrics pass the 5% gate. Source/binary hashes, raw samples and gate logs are in
`/tmp/velocity-lane-a-r16-review/`; `paired/report.json` contains the binary identity.
Reproduce with the earlier paired command, using the review evidence directory for `--base`
and `--output` (`/tmp/velocity-lane-a-r16-review/paired`).

Review hook timings, milliseconds per command averaged over 20 invocations, 10,000 carets:

| Buffer | No-op insert | Type with 10,000 bookmarks | Plain typing | Recording |
| --- | --- | --- | --- | --- |
| 1 MiB | 0.3093 | 1.3123 | 1.0383 | 1.0770 |
| 32 MiB | 0.2933 | 1.3217 | 1.1055 | 1.1046 |

The restored stable region sort shows no buffer-size-dependent growth in this probe.
The new bookmark-toggle overlap lookup uses sorted interval endpoints rather than scanning
all bookmarks once per caret. Full one-caret/many-caret data is in `hook-timings.log`.

Second-review native checklist (pending; headless regressions use the production key dispatcher):

- [ ] With the document caret between `()` and Find text `abc`, press Backspace: input `ab`,
  document unchanged. Repeat Cmd+Backspace and Ctrl+Shift+K with Find, the command palette,
  and the Save Macro name field focused. Undo edits only the input. With document focus,
  the same keys retain their usual behavior, including empty-pair deletion.
- [ ] Add an unrelated corrupt `.sublime-package` and an unreadable loose file to a disposable
  profile. Cmd+Enter, Ctrl+Shift+K, Cmd+Backspace, empty-pair Backspace and a User macro still
  work. The corrupt package produces a warning; valid User macros remain in the chooser.
  A corrupt Default archive falls back to our built-in macros. Test a loose override of an
  installed macro, then remove/reload it: the archive supplies the next playback.
- [ ] With 100 MB of unrelated loose data, invoke the first macro after launch and after a
  settings/resource reload. Check responsiveness. The command path loads only its requested
  file; the chooser uses the workspace's background filename index.
- [ ] Record typing across tabs, stop, replay, Undo and Redo or Repeat. Edit and undo an input
  field while the document's recorded macro stays intact. No new recorder is allocated for
  each host dispatch (headless allocation probe: 10,000 handoffs allocate zero times).
- [ ] Run `[close, select_all, left_delete, save]` from a clean tab next to a dirty file.
  The origin closes, playback stops with status feedback, and the neighbor stays unchanged
  and unsaved. Repeat through a nested `chain` and a hand-written macro file; also change the
  active view with New File. Test normal dirty-close prompting and close-last-window behavior.

The second-review code keeps the sidebar hook and its
`empty_window_sidebar_omits_input_adapter_before_and_after_preview` regression.
The earlier review's focused-input assertion above was corrected: a key-bound macro file
uses the input's own registry unless document playback is already running.

Second-review validation: fmt, strict locked release clippy (all targets), the full locked
release workspace tests at default parallelism (770 passed, 0 failed, 4 intentionally ignored),
and rustdoc with warnings denied pass. Both the 100 MB macro timing and the existing many-caret
hook timing pass when invoked explicitly. The zero-allocation test is
`vel-commands/tests/registry_dispatch.rs::registry_handoffs_allocate_no_placeholder_state`;
its companion real-key regression covers shared recording, local repeat and input undo.

Same 100 MB fixture before/after this review's fixes (100 loose files of 1,000,000 bytes;
five fresh application instances, real Cmd+Enter dispatch, startup/reload work outside the timer):

| Command | Before median | After median |
| --- | --- | --- |
| First macro key | 15.863 ms | 0.090 ms |
| Warm macro key | 0.034 ms | 0.031 ms |
| First macro after resource reload | 15.346 ms | 0.091 ms |

The first sample after the change took 0.455 ms; all five first-key samples were below 0.5 ms.
The same fixture directory was reused, without changing its contents. No unrelated package
contents are retained by the macro cache; only parsed commands for invoked paths survive.

Second-review paired typing, 21 alternating pairs against the same frozen baseline hash above:

| Bench | p50 main → candidate | p95 main → candidate | p99 main → candidate |
| --- | --- | --- | --- |
| typing1 | 0.217666 → 0.219500 (+0.84%) | 0.367498 → 0.368387 (+0.24%) | 7.065017 → 7.030640 (-0.49%) |
| typing1000 | 5.458208 → 5.465333 (+0.13%) | 15.024710 → 15.220507 (+1.30%) | 27.924256 → 28.129881 (+0.74%) |

All six metrics pass the 5% gate. Candidate SHA-256:
`12bae0fbc43a0f23f1944d73976b3448cdd24ec5a73f8796bbaf5014ed9ed3f5`.
Logs, the reused fixture, before/after samples and `paired/report.json` are retained in
`/tmp/velocity-lane-a-r16-review2/`. Reproduce the timing with
`cargo test -p velocity --release --locked macro_first_key_with_large_loose_package -- --ignored --nocapture`.
The paired command uses the same `--base` as the first review, with
`--candidate /tmp/velocity-lane-a-r16-review2/velocity-candidate` and
`--output /tmp/velocity-lane-a-r16-review2/paired`. No native checks, packaging, clean or commit
was performed.

Earlier checklists follow.

# Round 15: opening, saving, projects and sessions (native review pending)

Use a reviewer-built binary/bundle with a disposable profile and fixture tree. Do not rebuild or
replace the currently running `target/package-macos` bundle. No GUI, Finder, dialogs or menu
tracking were run in the sandbox; leave these boxes unchecked until native review.

- [ ] **1.1–1.3 opening:** CLI-open a missing filename and one under a missing parent. Both
  produce named new tabs; Save creates only the former and reports the latter's path. Mix
  unreadable/special paths with valid files in CLI/Finder batches: later files open, errors
  list all failed paths, the CLI returns its ack. Open a directory normally (new window),
  then with `-a` / `--add` (current window). Repeat with several windows and no live window.
- [ ] **1.4 stdin:** run `printf 'original\n' | velocity --data-dir PROFILE - > /tmp/vel-r15-out`,
  edit to include Unicode, close the tab and compare stdout. Repeat after Save As and by
  closing the whole window / quitting; the caller waits and receives the final edited text.
- [ ] **1.5 symlinks:** project root contains a linked directory with a link back to an ancestor.
  With follow_symlinks omitted, expand once and use Goto Anything / Find in Files; linked
  content appears, cycles do not expand forever. Set explicit false and repeat. Save the
  project: the absent key stays absent and false stays false.
- [ ] **2.1 Save All:** dirty named/untitled tabs, clean tabs and a read-only active hex tab.
  Save All remains enabled; only dirty documents save. Cancel an untitled dialog and fail
  another save, then confirm later dirty files still save and skipped buffers remain dirty.
- [ ] **2.2–2.4 deleted files and names:** delete a loaded file externally; its title shows
  deleted and closing prompts. Save recreates it. Restore a session with missing files;
  other tabs restore. With close_deleted_files true, clean missing tabs close but dirty
  contents remain. Close prompt buttons are Save / Don't Save / Cancel; its filename has
  no dirty dot. Untitled names and Save suggestions use the first line, capped at 50 scalars.
- [ ] **2.5–2.6 empty/reopen/recent:** close the last tab: empty tab bar, no new Untitled tab.
  Type, Cmd+N and double-click empty tab-bar space to create on demand. Close a saved file
  with a directed selection and scrolled viewport; Cmd+Shift+T / File → Reopen Closed File
  restores both. File → Open Recent lists files/folders/projects; Clear Items persists across
  relaunch. A recent folder opens a new window. Test independent reopen stacks in two windows.
- [ ] **2.7–2.8 policy/errors:** set hot_exit to boolean false, make dirty text and close/quit:
  normal Save/Don't Save/Cancel behavior. Unreadable small and background-loaded files must
  report the actual path once, without repeated errors on tab access.
- [ ] **3.1–3.2 projects:** switch a dirty untitled window into a project using the menu and
  running-instance --project. With Hot Exit, quit/relaunch and recover the departed untitled
  window as well as the project. With Hot Exit disabled, verify prompts (menu) or a separate
  window (CLI). Change an opened project's file externally: watching still reloads it.
- [ ] **3.3–3.5 folder rules:** Save Project As into a sibling directory, then reopen it;
  original relative roots still resolve and unknown top-level/folder keys remain. Include
  patterns apply before excludes. Default filters hide VCS folders, .DS_Store, compiled
  objects and workspace files, while target remains visible. A .gitignore-only file appears
  in Goto Anything and Find in Files; the side bar retains its existing ignore behavior.
- [ ] **4.1–4.2 settings/encoding:** load detected indentation, manually choose width/style,
  reload resources and externally reload the file: manual choices remain. Exercise File →
  Reopen with Encoding and Save with Encoding (Windows 1251, UTF-16 LE/BE with BOM, UTF-8
  with/without BOM), default_encoding and fallback_encoding display names. Invalid fallback
  reports once and uses Windows-1252; invalid data after a BOM falls back. Reopen cancellation
  or decode failure must preserve unsaved text. Retest syntax-over-project settings precedence.
- [ ] **Review (a) and (c):** interleave tabs from two groups, close an earlier tab (focus stays),
  close active middle/rightmost tabs (choose right then left in the same group). Trash a
  project root from the side bar: only successfully trashed roots detach, including from the
  saved project; other roots/unknown keys/relative paths survive. Test failed trash/save too.
- [ ] **Review (d), LaunchServices:** while a primary runs, force a secondary `open -na` with
  no document event: no ordinary secondary window/session is left behind and it exits after
  the quiet interval. Repeat with the primary blocked in a native save dialog and several
  Finder/Dock batches during relay: UI does not wait on the socket, requests retain order,
  no spurious timeout failure/duplicate local opens, and the receiver exits after the last ack
  plus a quiet interval. Quit the primary during relay: takeover restores all dirty windows,
  installs the crash handler and opens all retained paths. These timing/routing cases need
  native evidence; deterministic headless tests cover the relay state/protocol only.
- [ ] **Review 1 / 10 / 11, empty preview:** close the last tab, single-click a side-bar
  file and preview several Goto Anything results, including replacement of the only
  transient tab. Escape leaves an empty tab bar and Open Files has no ghost Untitled.
  Typing creates one visible tab. Save As / Save and Close are disabled while empty.
  Quit with dirty content in another window and relaunch: all retained content survives.
  Preview cancellation does not add to Reopen Closed File. Close two real files, delete
  the latest on disk, then Cmd+Shift+T: skip it and restore the older file's view state.
  Compare the deleted-file reopen behavior with Sublime on the reviewer's native machine.
- [ ] **Review 2 / 3 / 4, filesystem changes:** expand a folder containing dangling/self/
  ancestor symlink loops and unreadable children; healthy siblings remain visible. Run
  Replace in Files twice with empty user excludes: our recovery backups remain hidden
  from side bar, Goto Anything and Find in Files and keep their original contents.
  Create/delete/rename files and subfolders while the index is running: native watcher
  events refresh Goto Anything (the headless index thread test disables native watching
  to exercise unavailable-watcher polling; FSEvents delivers no events in the sandbox).
  Delete/recreate a clean tab's file with identical then different content (also cloned
  tabs): deleted markers clear, clean tabs reload, real local edits retain a conflict.
- [ ] **Review 5 / 6 / 8 / 9, session details:** repeatedly switch a clean saved-file window
  between projects / Close Project, quit and relaunch: no extra departed windows return.
  Repeat with dirty/untitled contents: those windows recover. Make a dirty session file
  unreadable or invalid on disk, restore and activate it: retained unsaved text is shown
  and survives another quit/relaunch. Hidden restored saved tabs do not show `[new]`;
  activate a lazily restored file with two-space indentation after appearance setup:
  detection runs, and later appearance/resource reloads keep manual indentation choices.
- [ ] **Review 12 / 13 / 14, paths and routing:** Save Project As across a symlinked parent
  (including `/tmp` and `/private/tmp`) and reopen: each relative root resolves to the
  same directory. Folder-name includes exclude unmatched ancestors as documented; an
  included descendant does not bypass its excluded parent. Launch with `--project X`
  or X's directory when a recovered window already contains it: reuse that window once;
  `--new-window` still creates another. Send a late Finder event just before relay quiet
  expiry: wait for the last request/ack plus the quiet interval. Force a relay failure:
  its error names all paths in the failed batch.
- [ ] **4.3 contexts (Lane A):** review new editor key-binding contexts with Lane A's changes;
  Lane B did not edit editor/view context providers.

# Round 15: find panels, overlays, status and editing details

Native checks are **pending human review**. Run an ordinary release build on an unlocked macOS
desktop with a disposable profile and project; no GUI, Metal or packaging script was run in the
sandbox. Keep the round-14 Finder/Dock/window-open and native context-menu checklist below.
Record OS, input source, physical display scale, `ui_scale`, and observed results.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --release --locked -- -D warnings
cargo test --workspace --locked --release
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo build --release --locked -p velocity
./target/release/velocity --data-dir /tmp/velocity-r15-review /path/to/disposable/project
```

- [ ] **1.1 / 1.9 Replace and buttons:** Enter / Shift+Enter in both Replace fields find
  forward/backward without editing. Replace uses Cmd+Alt+E or its button; Replace All uses its
  button / Ctrl+Alt+Enter. Find shows Find / Find Prev / Find All, Replace shows Find / Replace /
  Replace All, and Find in Files shows Find / Replace. Alt+Enter in Files never selects buffer
  matches. Confirm the reviewer's focused-panel key binding contexts still apply.
- [ ] **1.2 / 1.3 Incremental and typing:** start partway down a long document, Cmd+I, type a
  query with an offscreen next match. The first match at/after the original caret is selected and
  revealed, with wrapping. Enter advances and closes. Reopen, type, then Escape: original
  selections (including the primary region) and horizontal/vertical scroll return. Repeat with
  `hide_panel {"cancel":true}`, and Escape before the background search completes. Ordinary Find
  also selects/reveals while typing. After three Find Next presses, Left/Right, Cmd+A and Cmd+C
  in the field must keep the current document match. With In Selection enabled, typing and
  option changes must retain every original selection and reopening must retain the full scope.
  Seed Find with a forward/reversed selection: preview starts at its beginning. Test
  `find_selected_text` and `auto_find_in_selection`.
  Incremental Enter intentionally follows the [public search guide](https://docs.sublimetext.io/guide/usage/search-and-replace.html#incremental-search):
  advance to the next match and dismiss the panel; compare on the reviewer's current Sublime build.
- [ ] **1.4–1.6 Focus and history:** reopening Find selects its query; reopening Replace also
  selects its replacement. Click the editor with Find visible, Cmd+F focuses/selects, then Cmd+F
  closes. Check every find panel's close button and Escape from editor/panel focus. Up/Down in
  Find and Replace walk committed history and return to the draft. Pause during a half-typed
  query, quit/restart and verify checkpoints did not add prefixes to history.
- [ ] **1.7 / 1.8 Formats and feedback:** regex `(cat)([0-9]+)` with `\2:\u\1`, `$&`,
  `$+{name}` for a named capture, `\n`, `\t`, `\\`, `\U…\E` / `\L…\E` and the existing
  `$1`, `${1}`, `${name}`, `$$` behave as expected; Undo restores one replacement transaction.
  A missing query shows `Unable to find …`; replacement counts survive worker completion and
  expire after about five seconds. Invalid regex appears inline/in the status bar using the
  user's expression, with no internal flag wrapper or modal alert. Let the message expire, then
  retry Enter, Find Next/Prev, Find All and Replace without changing the invalid query: the error
  must reappear. Check long feedback stops before all three Find/Replace buttons at every scale.
  Also try `$1px` / `$1_old` (numeric capture followed by literal suffix). Repeat in Files.
- [ ] **2.1–2.3 Files:** try `<open folders>`, `<open files>`, `<current file>`, `<project>`,
  `*.rs`, `src/`, `//src/`, `-node_modules` and `-node_modules/`, including nested directories,
  paths containing spaces and unsaved active/open files. See `Searching N files…` followed by
  actual match/file counts in the header and status. Escape / hide the Results panel cancels
  promptly. While searching, open Find, a Vintage prompt, or build output: the search keeps
  running in the background, including when the other panel closes. Starting another search
  replaces the prior search. F4 / Shift+F4 skip context/header rows and jump search matches after
  the panel closes. A build switches those keys to diagnostics without dropping a running search;
  run a new search to switch back. In a root `/.../src/app`, `src/*.rs` must match only its own
  `src/` subtree, `-src/*.rs` must retain root-level files, and `-me/*` must not match a parent
  directory. A hidden root such as `~/.dotfiles` with `-.*` must still search visible descendants.
  Repeat relative patterns under an explicit nested root; `//` remains anchored at the project.
- [ ] **3.1–3.5 Overlays:** Cmd+R lists symbols and keeps the selected symbol through repeated
  frames and a source refresh. Filter quickly and press Enter before ranking finishes: the best
  current file/command candidate opens/runs. Changing a query returns selection to the best row;
  only refreshes of the same query preserve a manually selected row. Rank Cmd+P files, Escape,
  then Cmd+R in plain text or while indexing and immediately Enter: no stale file opens and no
  stale symbol offset is used. Switch a file query directly to `#word`, `@`, and `:line`, too.
  Single-click a result to accept; click outside to cancel and restore the preview. Close Split is absent with one group, available with two, and
  cannot run after becoming disabled; a rejected acceptance leaves the palette open. Idle palette
  frames reuse the enablement captured on open, and acceptance rechecks the chosen command.
  Footers describe each overlay and contain no find messages;
  split hints appear only on Goto. Repeat with Pinyin/Japanese composition in overlay inputs.
- [ ] **3.6 / 4.1 Scale and status:** at `ui_scale` 0.75, 1, 1.5 and 2 the overlay is centered;
  right-click sidebar rows and verify the path under the pointer. A single empty caret has no
  selection count; one/multiple regions show correct Unicode character counts and plurals.
  Encoding/EOL appear only with `show_encoding` / `show_line_endings`. Click Spaces / Tab Size:
  change tabs/spaces and width, convert mixed indentation, then Undo once.
- [ ] **4.2 Side bar / round-14 regressions:** Ctrl+0 leaves Find visible; Escape returns to the
  view without hiding Find. Retest Create File/Folder, Rename (including case-only/no-op), Trash,
  Search This Folder, Reveal in Finder and Copy Path. Rename retains deleted/external-change
  state; relative roots survive reload; symlinked sidebar paths target their canonical open tab.
  Keep the round-14 tab-close/active-tab/context-argument and Finder-open checks green.
- [ ] **5.1–5.3 Editor:** on wrapped Unicode lines, Home/End / Cmd+Left/Right reach visual
  boundaries; End on a non-final row remains just before its wrap point, on the same drawn row.
  Follow it with Home, Up and Down and verify the originating visual row; compare exact End
  placement with Sublime on the native desktop. Repeated BOL reaches indentation then column
  zero. Hard BOL/EOL remain logical. Repeat with Shift, grapheme clusters and multiple carets.
  Home on a huge single-line file must remain responsive. Find All to create 10,000 carets in a
  large viewport-highlighted buffer; arrows and quote keys must remain responsive with edit
  history retained. Type quotes rapidly after edits in code, strings and comments while
  highlighting catches up: published line scopes continue to control pairing.
  Cmd+D, Quick Find All, Cmd+E and Find Under retain caret-word and primary-selection behavior.

Earlier checklists follow.

# Round 11: Vintage and user grammar review

Run outside the sandbox on an awake, unlocked display:

```sh
cd /Users/zoro/dev/velocity-lane-a
cargo build --release -p velocity
zsh crates/velocity/scripts/review-round11-gui.zsh
```

The script freezes the binary, records its hash, creates an isolated profile with both original
TextMate fixtures and enables Vintage in command mode. It prints the profile/grammar/log paths.
`--prepare-only` creates those fixtures without starting a native application.

- [ ] See COMMAND in the status bar and a readable block caret. Try `2d3w`, `u`, `gg`, `G`,
  `f`/`;`, `ciw`, `v`, `V`, Ctrl+V, `>`, registers, `q`/`@` and `.`. Pending prefixes/counts
  and visual line/block selections should update on screen. Confirm multiple carets still work.
- [ ] Press `i`, activate Pinyin/Japanese, compose and choose candidates. Preedit and candidate
  geometry should match normal editing; Escape after committing returns to command mode.
  In command mode, `h/j/k/l` should act immediately with the input method still selected.
- [ ] Use `/one`, `?one`, `n` and `N`; Escape cancels a prompt without editing. Try `d/three`.
  Use the input panel's arrows, clipboard and IME. Open Find or another tab during a prompt.
- [ ] In `editor.rs`, reindent the second line with `==`; `u` restores indentation in one step.
  Verify clipboard `yw`/`p` using another application. Check `:w`, `:wq`, `:q`, `:q!`,
  `:e`, `:e!` and `:w <temporary path>` with dirty/save-cancel/failure paths.
- [ ] Ctrl+Alt+V disables/re-enables Vintage for the active view. Edit User preferences while
  open to test all four settings. Turning it off closes a vi prompt and restores normal typing.
  Reopen using a fresh profile to confirm Vintage is off by default.
- [ ] `notes.vnote` detects Velocity Notes; `xml-notes.vxnote` detects Velocity XML Notes.
  Check TODO/string/comment colors, syntax picker and palette entries. Edit the printed JSON
  grammar's name/rules, add another nested grammar, then remove it: open views and choices
  should reload while retaining unsaved edits. Try a malformed definition and read-only cache.
- [ ] Test split/clone views, Retina scaling, status clipping and Metal validation. Record the
  display, input source and input JSONL path with observed results. Native rows remain unverified
  until a human runs them; the headless tests initialize no window server or GPU.

Earlier checklists follow.

# Native AppKit NSTextInputClient manual checklist (P1)

Round-nine tester status: **Chinese IME confirmed working by the human tester**. Hot Exit is
currently under human test. The individual legacy rows below retain their original evidence
cells; Japanese/reconversion/display-specific cases are not marked passed by that confirmation.

Adapted from the repository’s MIT OR Apache-2.0 P0 checklist. Hardware results are pending.

Run outside the sandbox:

```sh
cd /Users/zoro/dev/velocity
cargo build --release -p velocity
./target/release/velocity --log-input crates/velocity/fixtures/ime-and-unicode.rs
```

Record machine, macOS version, active input source, keyboard layout, display/scale/refresh and the
JSONL filename. Test each relevant row with Simplified Chinese Pinyin, then the Japanese input
source where indicated. Position the caret by clicking or arrows; select with Shift+arrows or drag.
Leave the observed-result column empty until tested.

| Case and steps | Expected result | Observed result / pass/fail / log reference |
| --- | --- | --- |
| Pinyin: click after `→`, type `nihao` | Inline marked text with underline and preedit caret; document contains transient preedit (Undo/cancel restores the original); candidate window adjacent to caret | |
| Choose `你好` with Space or candidate number | Exactly one `你好` inserted, marked text cleared; confirmation Enter/Space does not insert an extra newline/space | |
| Continue typing English after commit | New characters insert once at the new caret; no duplicated IME commit | |
| Shift toggles English while composition is active | Native IME's selected commit/cancel behavior is reflected once; no stale preedit; subsequent English input works | |
| Backspace inside `nihao` preedit | Marked text updates through IME; previously committed document text is not deleted | |
| Esc during preedit | Native composition cancels, underline/candidate UI clears; document content remains as before composition | |
| Move within a preedit using native IME keys | Preedit cursor and selected conversion range update in UTF-16 code units, never splitting a surrogate | |
| Scroll a few pixels with an active visible composition | Marked text stays inline and candidate anchor follows the visible caret, with no 2× Retina offset | |
| Scroll composition offscreen and back | Candidate anchor stays inside the client area while offscreen; real caret position is restored on return | |
| Resize with an active composition | Underline, text, cursor and candidate anchor follow current geometry; no stretched/stale atlas text | |
| Move window between 1× and 2× displays | Glyphs rerasterize; caret/candidate placement keeps logical-point alignment | |
| Japanese: enter Kana, Space to convert, move conversion selection, then confirm | Kana/marked range is visible; candidate conversion commits once; caret ends after committed text | |
| Ctrl+Cmd+Space, choose an emoji in the native picker | Emoji inserts once and advances appropriately; record a missing commit separately from a missing image | |
| Option+E then E using an English layout | `é` inserted once, dead-key preedit cleared, no extra apostrophe or `e` | |
| Pinyin full-width punctuation: `，。！？（）` | Correct full-width glyphs and two-cell spacing; no duplicate ASCII punctuation | |
| Select text, then start Pinyin and confirm | Selection remains well-defined; commit replaces selected text exactly once; subsequent deletion is safe | |
| Switch focus away and back during preedit | No stale marked text; IME can be enabled and used again after focus returns | |
| Compare fixture's `你好`, Kana and punctuation | Correct fallback glyphs; no tofu, inappropriate Latin glyphs, clipping or baseline jumps | |
| Compare `😀 🦀 🚀 👩‍💻 ❤️ 🇨🇳` | Simple color emoji visible; inspect ZWJ, variation selectors and flags for correct single-cluster display | |
| Compare NFC `é` and decomposed `é`; Backspace each | Legible placement; one Backspace deletes one extended grapheme | |
| Shift+arrows and mouse drag across Chinese/emoji and newlines | Selection rectangles follow cell widths and line boundaries; Backspace/typing replaces selection | |
| PageUp/PageDown, horizontal and fractional trackpad scrolling | Smooth point-based movement, gutter fixed; caret is revealed by subsequent arrow/edit input | |
| Type quickly with `--log-input`, then close | Every delivered keyboard and IME event has a timestamp; changes have receipt→submit/present-return records | |

Native logging records key events, standard command selectors, Insert/Marked/Unmark,
explicit replacement ranges, selected/marked document UTF-16 ranges and latency to
queue submission/present-return. A commit without prior marked text must work. The
buffer contains inline transient preedit; cancellation restores its origin snapshot,
and the whole composition is one Undo step. `unmarkText` keeps the current text once.

Additional native range/geometry cases (all pending):

| Case and steps | Expected result | Observed result / pass/fail / log reference |
| --- | --- | --- |
| Japanese: select committed 日本語, invoke input source's reconversion, change and confirm | Explicit UTF-16 replacement range replaces exactly that text; one Undo restores it | |
| Place caret after a non-BMP emoji, invoke reconversion of adjacent CJK | Offsets count emoji as two UTF-16 units; no adjacent scalar is damaged | |
| English: hold E and choose an accent from the press-and-hold menu | AppKit replacementRange changes the original E once; selected/marked ranges remain valid | |
| Emoji picker with no composition and a nonempty selection | Exactly one emoji replaces the selection, with correct color and candidate position | |
| Start composition over a selected word, cancel, Undo/Redo, then paste | Original selection/text restore on cancel; subsequent history/paste is correct | |
| Reconversion requests attributedSubstring at document edges / emoji | Actual range is clipped to valid scalar boundaries; no crash or half-surrogate text | |
| Drag from the text to a window on a display left/above the primary display | Candidate rect remains in screen points with correct sign; no Retina multiplication | |
| Hide/occlude/unocclude during composition and move between 60/120 Hz displays | No idle redraw loop; candidate anchor and display rate refresh correctly | |
| About/Hide/Quit, Undo/Redo/Cut/Copy/Paste/Select All from menus and shortcuts | Native actions reach editor commands; clipboard text round-trips Unicode | |
| Toggle system light/dark, resize continuously, enter/leave fullscreen | Fresh geometry and colors, no stretched text; native fullscreen works with tabbing disabled | |

Check environment.fonts and final_render_state.fonts for resolved faces, warnings and
CoreText raster counts. Some OS font formats require the retained native face's bitmap
fallback. Check CJK, combining marks, emoji colors/alpha and ZWJ/flags visually.
The layout is a unidirectional grid, not paragraph bidi or automatic wrapping.

Candidate queries implement both firstRectForCharacterRange:actualRange: and
characterIndexForPoint:. View→window→screen conversion uses logical points, including
flipped-view conversion. Record requested/actual UTF-16 ranges and resulting screen
placement when any reconversion or picker case fails. No native input or visual pass
is claimed from headless tests alone.

## Round 4 editing smoke test

Use disposable copies of files. Record results here; these native cases were not run in the sandbox.

- [ ] Open two files with Cmd+O and via `velocity first.txt second.txt:2:3`; check the one-based Unicode position and tab labels.
- [ ] Type, select, cut/copy/paste, Enter with indentation, Tab/Shift+Tab; save with Cmd+S, Save As with Cmd+Shift+S, and reopen to compare bytes.
- [ ] Add carets with Cmd+click, Cmd+D and Ctrl+Alt+Up/Down; type/delete/paste at every caret. Split a multiline selection with Cmd+Shift+L; all carets and selections must render.
- [ ] Cmd+Z/Cmd+Shift+Z restores the full multi-caret transaction; grouped typing undoes by the buffer's rules. Cmd+U/Cmd+Shift+U restores soft selection history. Escape leaves one selection.
- [ ] Switch with Ctrl+Tab/Ctrl+Shift+Tab and Cmd+1…9; each tab retains its text, history and scroll. Closing a dirty tab/window offers Save/Cancel/Discard; cancelling leaves edits intact.
- [ ] Open/save UTF-8 with/without BOM, UTF-16LE/BE with BOM, Windows-1252, LF/CRLF/CR and mixed endings. Confirm displayed encoding and majority line endings, BOM retention and permissions. Unrepresentable saves must report an error and preserve the original file.
- [ ] Set `trim_trailing_white_space_on_save` to true then `"not_on_caret"`; test all caret lines, `trim_only_modified_white_space`, inserted lines and final-newline cleanup. Undo cleanup as one transaction.
- [ ] Change an open file externally: clean views reload, dirty views ask Keep Edits/Reload. Repeat with an atomic rename and after a previous save.
- [ ] Edit User preferences/keymap with comments/trailing commas; change tab size/font/gutter and override a shortcut. Verify the new binding wins and project/view settings keep their precedence.
- [ ] Enter `(`, `[`, `{`, quotes, and matching closing characters; confirm pairing/skip contexts. Repeat on the actual keyboard layout, including shifted shortcuts, keypad keys and an optional modifier-tap user binding.
- [ ] Repeat the IME rows above with multiple carets: marked text is a transient primary preview; commit inserts once per caret and one Undo removes the committed conversion. Explicit native replacement ranges replace only their specified region.
- [ ] Open the 100 MB fixture: a loading tab appears promptly, UI stays responsive, progress advances, and edits cannot overwrite the file before loading succeeds.
- [ ] While testing startup outside the sandbox, verify asleep/locked/occluded runs produce explicit errors in about two seconds, then unlock/uncover and rerun successfully. Inspect skipped_drawables, first_callback_main_ms and first_real_scanout separately.


## Round 5: widgets, navigation, search and side bar

Run the reviewer fixture command from RUNBOOK.md outside the sandbox. These are human acceptance
checks; use the automatic `--bench-panels`/search example/matrix for measurements. Leave results
blank until observed. Keep the entire IME section above, and repeat its core cases in both the
Goto input and Find/Replace/Where inputs (including Undo, horizontal scrolling and Retina moves).

| Case and steps | Expected result | Observed result / pass/fail / log reference |
| --- | --- | --- |
| Cmd+P; type `ime`, Up/Down/PageDown; Esc | Fuzzy glyph highlights, kind/path details and a bounded list; transient preview disappears; original tab, selection and viewport return | |
| Cmd+P `ime-and-unicode.rs:8:3`; Enter | Target uses one-based scalar coordinates and becomes a pinned tab | |
| Cmd+P `:8`; Esc, then repeat and Enter | Preview moves within current file; Esc restores selection; Enter retains it | |
| Cmd+P `#input`; select a word; Enter | Fuzzy word rows preview the correct UTF-8 position | |
| Cmd+R / `file@symbol` | Current-file syntax symbols appear with transformed names; selection previews, Escape restores, Enter commits | |
| Cmd/Ctrl+Enter on a Goto result | Two visible panes; each scrolls independently; clicking switches active target; Close Split palette action restores one pane | |
| Cmd+Shift+P `save`; inspect hint; run safe commands repeatedly | Captions, binding hints and per-invocation recent ordering; arrows and Enter work; Escape preserves document | |
| Pinyin/Japanese/emoji in Goto and find inputs | Composition stays inside the focused input; candidate rectangle follows wide graphemes; confirmation never accepts a panel prematurely; Undo changes only that input | |
| Cmd+F; toggle regex/case/word/wrap/selection; Cmd+G / Shift+G | Correct highlights and forward/backward wrap; invalid Perl lookaround/backreference shows an error without altering text | |
| Cmd+I and type incrementally; Escape | Viewport updates during typing, complete matches arrive asynchronously, no stale results after an edit/tab switch | |
| Select a region, Cmd+Alt+F; preserve case; Replace All; Undo | Only selected matches change; cat/CAT/Cat map to replacement casing; one undo restores all; repeated shrinking replacements remain in the tracked selection | |
| Regex `(cat)([0-9]+)` -> `${2}:$1`; Replace All | Captures expand correctly; zero-width regex progresses without hanging | |
| Cmd+E / Cmd+Shift+E; Alt+Enter | Find/replacement seeded from selection/word; Find All creates a multi-selection | |
| Cmd+Shift+F; Where `<project>, *.rs, -ignored*`; Enter | Streaming Find Results has file headers, numbered context, highlights and summary; Up/Down/Enter or double-click jumps | |
| Edit without saving; Files Where `<open files>` | Search sees the unsaved buffer text; IO/decoding failures are reported | |
| Fill Replace, run Files search, invoke Replace in Files; Cancel, then confirm on disposable files | Cancel writes nothing; confirmation reports counts; originals kept beside each file; changed/dirty targets rejected; backup bytes restore originals | |
| Expand fixture folders, create/rename/delete a file externally; adjust ignore rules | Lazy tree and Goto index update; .gitignore and file/folder/binary exclusions agree with their documented scopes | |
| Cmd+K, Cmd+B; Ctrl+0; navigate Open Files and folders | Visibility toggles, keyboard focus stays in tree, single-click previews and double-click/Enter pins | |
| Project: Add Folder…; choose a second directory | New root appears and indexes; canceling native picker leaves roots intact | |
| Open many files; wheel over tabs; close, dirty an input, drag tabs | Scrolling, close hit targets, dirty dots, reorder and active document identity remain correct | |
| Resize panels/window; hover/press controls; scroll output/results/list | Rows remain clipped/virtualized; caret/selection and active surface route correctly; status stays readable | |
| Click document while Find remains visible, then resume panel input | Document edits and panel edits target their own buffers; overlay always captures above both | |

Native geometry, font quality, appearance, Metal validation, refresh-rate behavior and GUI timing
remain pending reviewer evidence. Attach the run directories, completed table and observed hardware
configuration; do not replace missing scanout values with CPU timings.


## Round 6: highlighting, language commands and view features

All native observations and timing cells below remain pending reviewer runs. Use the round-6
RUNBOOK commands on an awake, unlocked display; attach actual physical dimensions and JSONL.

| Action | Expected | Observed / run directory |
| --- | --- | --- |
| Open Rust, Python, shell/shebang, compound-extension and hidden-extension files | Detected syntax appears in the status bar; first real highlighted screen is correlated to positive presentedTime | |
| Click status syntax; use Set Syntax in Cmd+Shift+P; choose another language | Filtered picker works with keyboard/IME; syntax-specific settings apply without text/history loss | |
| Set color_scheme auto; change macOS appearance, then set an explicit scheme | Original dark/light palette switches live; explicit selection stays fixed | |
| Override a scheme variable/global in Packages/User; edit an already highlighted file | Token colors and background/caret/selection/gutter/find/guides/rulers/minimap update without stale palette spans | |
| View bold/italic, solid/stippled/wavy underline; enable no_bold/no_italic | Real face variants and decorations appear; CJK/emoji fallback and baseline/grid remain correct | |
| Type quotes/brackets in code, strings and comments; repeat while scopes update | Pairing uses current scopes; closing-quote skip remains usable; pending selector contexts do not pair on unknown scopes | |
| Cmd+/ on multiple carets, overlapping selected lines, and reversed selections; Cmd+Alt+/ | Line/block delimiters follow syntax metadata; toggling and undo restore original text | |
| Enter around Rust braces, Python blocks and temporary continuation rules; Reindent | Correct indentation/tab stops; asynchronous reindent never overwrites a later edit | |
| Cmd+R, filter and move through symbols; Escape, Enter; repeat after editing | Metadata names/transforms and positions update; preview/cancel/commit preserve source selection | |
| Move carets around nested brackets and HTML/XML tags; toggle individual matching settings | String/comment brackets are ignored; tag matching can stay enabled with bracket matching disabled | |
| Toggle wrap; resize; set wrap_width/style and continuation indentation | Graphemes remain intact; Up/Down/Page, mouse hit tests, selections and reveal follow visual rows | |
| Compose Pinyin/Kana/emoji across a wrap boundary; commit, cancel, undo | Candidate rectangle covers the first visual fragment; committed text and layout use the same snapshot | |
| Toggle whitespace selection/all/none/leading/trailing, guides, rulers, relative numbers and gutter | Only requested decorations appear; scheme globals and clipping agree | |
| Set smooth/phase/blink/solid caret, extra width and line padding; move between editor and panels/windows | Animation resets on input; inactive selections and focus render correctly; no idle continuous redraw for solid caret | |
| Fold via gutter, selection, indentation/brackets/block scopes, level and tag attributes | Ellipsis markers and suffix text remain visible; hidden lines are skipped by navigation and highlighting requests | |
| Edit/undo before and inside folds; unfold, unfold all; toggle fold-button fading | Named fold regions track edits; viewport and candidate geometry stay valid | |
| Show/hide minimap, click, drag beyond its bounds and release; resize or edit/newline | Token colors match the document; viewport scrolls without changing selections; indicator and border obey settings | |
| Run automatic open/scroll/typing with default and 4K physical sizes, then Metal validation | Positive highlighted scanout, full CPU timing, isolated minimap GPU timing and clean validation; no external input in automatic runs | |


## Round 7 — workbench/session/build acceptance (native review pending)

Run the isolated-profile commands at the top of RUNBOOK.md. Fill the last column with evidence,
actual scale/size/refresh and any failure; these rows have not been executed in the sandbox.

| Action | Expected behavior | Reviewer observation |
| --- | --- | --- |
| Launch with a non-existent data directory | Empty editor opens, User tree may be created, no missing-Packages exception | |
| Launch with corrupt session / read-only profile / a file occupying the data path | Empty usable editor; corrupt original preserved; IO failures reported without data loss | |
| All seven layout presets, normalized custom cells, splitters and New/Close Pane | Cells cover content, divider hits respect spanning cells, tabs stay with their groups | |
| Cmd-click / Shift-click / Tab Selection commands | Selected sheets appear horizontally inside one group with connector styling | |
| Sidebar Cmd/Shift multi-select; Goto Cmd+Enter | Additive sheets stay visible and previews pin on acceptance | |
| Move/close multiple selected tabs | Order/selection preserved; the last dirty shared views ask once when policy requires | |
| Clone File, separate carets/scroll/folds, type/undo/redo, close current owner | One document/history, independent view state, no stale text or detached-view panic | |
| IME in lower/right group, scroll inactive selected sheet, return focus | Candidate anchor, hit testing, selection and view-local scroll use correct offsets | |
| Cmd+Shift+N; close one window; CLI -n/-a/-w and stdin | One application, independent windows; wait ends when requested sheets close | |
| Full screen; distraction-free from normal and already-full-screen; customize its settings | Chrome/settings restore correctly and sidebar visibility is remembered per mode | |
| Quit/relaunch dirty/untitled text, redo, folds, syntax, sidebar and search state | Retained state restored, hidden clean tabs lazy; round-nine native history removes the former private-history limitation | |
| Hot Exit always / only_on_quit / disabled; remember_open_files false | Close/quit/crash policies follow RUNBOOK; disabled never silently discards dirty project docs | |
| Edit a dirty document on disk while quit, then restore | Buffer text retained as dirty, restored undo history dropped | |
| Open/Save As/Close/Edit/Quick Switch Project; nested fresh destination | Folder rules, inline builds and unknown keys preserved; workspace tied to project | |
| Leave a project idle after editing | Debounced session writes complete without an autosave/settings-reload loop | |
| Cmd+B with Workbench Example; F4/Shift+F4; output double-click | Both streams appear, exit/time shown, correct diagnostic location and inline badge | |
| Build With variants, Cancel Process Tree, quiet, syntax and wrapping | Variant overrides apply, tree terminates, UI stays responsive, output follows settings | |
| Native automatic startup/typing/scroll with four groups | Positive scanout/complete JSONL; budgets from RESULTS/RUNBOOK assessed with no missing timestamps | |


## Round 8 — intelligence and Hot Exit acceptance (native review pending)

Run the exact commands in RUNBOOK.md. Do not run GUI/Metal in the sandbox.

- [ ] Cmd+Shift+R: type `render`, inspect kind badges and file:line, arrow through previews,
  Esc restores original selection/scroll and Enter pins the target. Repeat with unsaved names.
- [ ] F12/Cmd+Alt+Down on `unique_target` jumps to another already-open file. `shared_name`
  opens multiple definition rows. Shift+F12 lists references when syntax metadata publishes
  indexed references. Missing metadata yields no reference locations. Ctrl+- and Ctrl+Shift+-
  traverse cross-file jumps and remain bounded after 130 jumps.
- [ ] Hover over a symbol, click a location, then disable `show_definitions` and confirm it
  stays hidden. Open Help → Indexing Status, watch percentage/logs, close only that window,
  and verify editor focus, folders and session state are intact.
- [ ] Ctrl+Space/Auto Complete: arrows, cycling, Enter and mouse commit. At the bottom of a
  view the popup flips upward. Check annotations/kinds/details and a second window/group.
  Type rapidly, switch tabs and move the caret while queries are pending; stale results never
  replace text. Set `auto_complete_commit_on_tab`, `auto_complete_cycle`, delay, selector,
  triggers, `auto_complete_with_fields` and size limits and verify each behavior.
- [ ] Type `fn`, Tab, rename the first field, Tab to args, Shift+Tab back, then Tab to `$0`.
  Expand `${1:name} = $1; ${2:value} $0` from an inline/User snippet: field outlines and mirrors
  update live with Chinese/emoji and multiple carets. Esc or moving outside the field exits.
  Verify undo/redo treats a primary edit and all its mirrors as one transaction. Clone the file
  into another group: passive painting must preserve fields; editing the other clone must
  cancel stale fields in the source view.
- [ ] Add/change `.sublime-snippet` and `.sublime-completions` resources in Packages/User;
  snippet names appear as `Snippet: …` and tab triggers/resource suggestions update. Runtime
  folders and unsaved text must remain in the window.
- [ ] Hot Exit: edit unsaved named and untitled files with expanded/mirrored snippets, retain
  an undo/redo branch, create multiple windows/groups and open the index status window. Quit
  and relaunch the same profile. Text, selections, folders, layouts and normal undo/redo return;
  popup/field sessions and the separate status window are transient. Repeat with `hot_exit`
  always/only_on_quit/disabled, Cmd+W and a forced process exit after autosave. Keep the existing
  read-only-session failure guard: quit must not silently discard dirty text when saving fails.
- [ ] Start with fresh, read-only and corrupt profiles. Cache creation/fallback and subsequent
  session writes must not reload settings or drop folders added through Open Folder. Verify
  indexing exclusions, disabling/re-enabling indexing and closing an unsaved indexed buffer.
- [ ] Retest Chinese IME commits/candidate placement with a popup and inside a snippet field.
  IME preedit must suppress completion and never publish preview text to the index/session.
- [ ] Record five native intelligence runs, physical size/scale/refresh and memory. Use the
  separate `--intel-words 0` run for the real ten-file memory target and retain the default
  100k-word stress footprint separately. Compare
  p50/p95 typing to 2/4 ms and definition jumps to 20 ms; retain scanout and submit separately.


## Round 9 — full Hot Exit and review UI (human verification pending)

Use the exact disposable-profile commands at the top of RUNBOOK.md. Chinese IME confirmation
remains accepted; repeat one composition only to check the new Hot Exit boundary. Record native
results/log references here; headless tests do not check these boxes.

- [ ] **First priority — full history:** type two words, move/select text, soft undo/redo, text undo once, quit and relaunch. Continue Cmd+U/Cmd+Shift+U and Cmd+Z/Cmd+Shift+Z: text, selection direction/primary, groups, redo cursor and saved-state dirty indicator match the uninterrupted session.
- [ ] Quit halfway through typing a word, relaunch and finish it without moving the caret; one Undo removes the same typing group. Repeat after an actual save: undo/redo must return to the clean saved point.
- [ ] Clone a file into two groups; alternate text edits and selection-only moves, close one historical editing view, quit/relaunch, then undo/redo from the remaining view. Each edit retains its owner; clones retain independent selections/scroll and share text undo.
- [ ] Keep one clone hidden, quit/relaunch, activate it, save another session and relaunch again. No extra selection events appear in soft undo; clean hidden documents remain lazy.
- [ ] Change a dirty file externally while quit. Relaunch retains unsaved contents as dirty and drops text/soft history. For a changed clean file, load current disk contents and drop history. Missing/read-only/corrupt profiles still recover as documented.
- [ ] Commit one Chinese IME composition, soft-select elsewhere and quit/relaunch. The composition remains a single undoable edit; no transient preedit returns.
- [ ] In the fixture sidebar, inspect `? M ! A+ M+ D+ U!`; missing and staged-deleted paths are visible. Folder badge priority ends at Unmerged; ignored files/folders are dimmed. Hover badge areas for status tooltips, including the Open Files section.
- [ ] Check branch name and `? / M / S / !` counts against the fixture. Turn `show_git_status_in_status_bar` off; turn `show_git_status` off/on. Set an invalid `git_binary`: behavior continues through gix.
- [ ] With `sublime_merge_path` empty, clicking branch status does nothing. Configure a real smerge-compatible executable; branch/context actions open repository, file/folder history, line history and blame for the intended path. Index bytes remain unchanged by Velocity status/diff reads.
- [ ] Add/modify/delete lines, including a deletion at EOF without a final newline. Inspect color-scheme `line_diff_*` markers; Ctrl+. / Ctrl+, navigate with wrap. Test `mini_diff` true/false/"auto" both inside and outside the repository.
- [ ] Switch `git_diff_target` between index/head; stage/save/change the file externally. Markers and revert sources update to the selected reference. A non-repository save resets its saved-file reference.
- [ ] Cmd+K Cmd+/ and right-click Show/Hide Diff Hunk display original lines **beneath** current content, with changed scalar shading. Cmd+K Cmd+; shows only this hunk. Scroll, resize, wrap, move Up/Down across the read-only original rows and click below them; caret and IME candidate geometry stay aligned. An edit hides old expansions.
- [ ] Cmd+K Cmd+Z, Revert Modification and context Revert Diff Hunk restore exactly one hunk; Undo restores the edit. Repeat after grouped undo/redo; no stale hunk reverts another version.
- [ ] F6 toggles squiggles; with selector `comment, string`, comments/strings are checked and code identifiers are not. Change `misspelling` in the scheme. Scroll through long documents and two clone viewports: checks follow only visible exact scopes without an idle redraw loop.
- [ ] Ctrl+F6 / Ctrl+Shift+F6 select/reveal misspellings beyond the viewport and wrap. Move the caret or edit while a navigation request is pending; the old request must not jump later.
- [ ] Right-click a misspelling; accept a suggestion, Undo, Add to Dictionary and Ignore Word. Suggestions are version checked, Escape dismisses the menu, and words survive relaunch. Existing User preference comments/keys survive.
- [ ] View → Dictionary lists English and `Packages/User/review.dic`; select the custom dictionary. Test a syntax-specific `spell_check` setting, invalid dictionaries and a package-traversal path; no incorrect underlines or crash.
- [ ] Run automatic native on/off typing, 4K typing, full-history restore and Metal validation commands. Retain complete JSONL and assess ≤5% typing regression, 2/4 ms absolute typing and ≤300 ms native first-window targets separately from CPU results.

## Round 10: theme and minihtml acceptance

No GUI or Metal execution was performed in the sandbox. Leave these boxes unchecked until a
human runs them. Preserve the round-nine IME confirmation above; repeat the panel-input cases
because the UI now shares proportional font metrics with hit testing and candidate geometry.

Run `zsh crates/velocity/scripts/review-round10-gui.zsh /absolute/path/to/round9/velocity` on an
awake, unlocked display. It builds once, runs the new automatic native demo with Metal validation
at ordinary/4K logical sizes, then alternates round-nine/current four-group typing and scrolling.
It finally opens a disposable manual profile. Record its printed evidence directory, machine,
OS, scale/refresh, all completion/error records and any Metal diagnostics.

- [ ] Open the command palette and run **Theme & minihtml: Review Demo**. Inspect all three
  phantom placements, the right-hand annotation, the scrollable hover and the HTML sheet tab.
  Wrapped paragraphs, list markers and links must keep distinct baselines; emoji/CJK and
  bold/italic/code faces must align with their measured boxes.
- [ ] In the HTML sheet click Light/Dark, then switch OS appearance with `theme: "auto"`.
  Explicit `theme` stays selected; `dark_theme`/`light_theme` choose the auto pair. Check native
  title-bar appearance, then set `themed_title_bar: false` and verify system appearance returns.
- [ ] Visit Goto, palette, sidebar expanded/selected rows, transient/dirty tabs, find/replace
  buttons, context menu and status bar in both appearances. Hover/pressed/selected states,
  match colors, kind badges, file icons, close/fold glyphs and minimap tint must be legible.
  Drag a scroll puck and switch `overlay_scroll_bars` between enabled/disabled/system.
- [ ] Add a User theme fragment that changes a status/sidebar color, font size and content margin;
  verify live reload. Try `ui_scale` 0.75, 1, 1.5 and 2; in a search field click CJK/emoji text,
  extend selection and compose Pinyin. Caret, selection, hit testing and IME anchor must agree.
  Move between displays to check @2x/@3x resource selection and glyph/texture sharpness.
- [ ] Use the palette's **Theme & minihtml: Popup flags …** entries. Default popups close on
  selection changes; flag 16 keeps them; 4 closes on movement/click/scroll; 8 permits movement
  toward/inside the popup and closes on movement away; 32 closes on character input; 48 keeps
  selections while closing on typing. Escape closes the popup. Completion cooperation is
  exercised by the automatic demo's flag-18 popup beside the completion menu.
- [ ] Scroll a popup larger than its maximum height. Click `subl:` actions and the public HTTPS
  link, and hover titles. Link/tooltip hit regions must follow scroll/resize and stay clipped.
- [ ] Edit and undo text before an inline/below/block phantom. Text, carets, hit testing and
  phantom links must retain the same tracked anchor and reserved space. Annotations must not
  add editable rows or change vertical navigation.
- [ ] Open `fixtures/theme-ui.sublime-project` and `workbench-error.txt`, select **Theme annotations**
  in Build With and build. The error appears as a right-aligned HTML annotation with a colored
  left border; literal `<markup> &` is displayed as text. Toggle `show_errors_inline` off/on.
- [ ] In the HTML sheet click commands/browser links, scroll, resize, close/reopen the app and
  confirm its name/content return read-only. Text commands must not change its buffer.
- [ ] Repeat with fresh, unwritable and corrupt profiles, then a malformed/missing User theme
  and missing image. Confirm fallback/diagnostics and continued editing. Record actual native
  timings against the round-ten budgets; CPU structural goldens are not a pixel-quality result.
# Round 12: huge files, long lines and hex

Run `zsh crates/velocity/scripts/review-round12-gui.zsh target/release/velocity
/tmp/velocity-r12-fixtures` outside the sandbox for three automatic native runs. Each performs
100 scripted scroll/offset/edit actions and exits. Inspect its `large_file_native` record,
all load telemetry, Metal validation and presentation logs. No native result is claimed here.

For interactive checks, open the generated fixtures with `target/release/velocity -n --log-input
--data-dir /tmp/velocity-r12-manual /tmp/velocity-r12-fixtures/log-4gb.log
/tmp/velocity-r12-fixtures/line-50mb.js /tmp/velocity-r12-fixtures/binary-4gb.bin`.

1. On the 4 GB log, verify a visible prefix while the tab still shows a percentage, and the
   status bar says read-only. Scroll/select/copy within that prefix. Typing must do nothing;
   Save and Save As must be disabled and must not create a partial destination. Close during
   loading, open another file, and verify no late content replaces it. Reopen and let loading
   finish; the prefix restriction should clear and the file should be editable. Repeat with
   a large UTF-16 BOM file and a Windows-1252 file containing an invalid UTF-8 byte late in it;
   fallback may show an empty provisional view, but must retain progress and original encoding.
2. In the 50 MB JS line, wait for “Preparing line geometry” to clear, then scroll far right,
   move to the middle/end, select across the viewport and type/delete. Verify no full-line
   flash or stalled scrolling. Check Shift+arrows, Option+word arrows, Home/End, a second
   caret and pointer hit tests. Start Pinyin/emoji composition in the middle; candidate/marked
   rectangles and committed text must follow the caret. Cancel composition and undo/redo.
   Newline inserts LF without scanning an oversized line for automatic indentation.
3. Verify the large-document status hint and each **Large File: Enable … for This View**
   palette command. Explicit re-enabling can perform expensive work; automatic policy should
   remain active in another view. Viewport syntax can start approximate; a normal line after
   an oversized comment/string must receive repaired scopes without the scene thread parsing it.
4. In the binary, verify 16 hex bytes plus ASCII per row. Ctrl+G `0xee6b2800` goes to the
   `END` tail. Select those three bytes: Copy gives `45 4e 44`, **Hex: Copy Text** gives `END`.
   Try a partial final row, decimal offsets, malformed input and offsets beyond EOF. Editing,
   IME commits and Save must be blocked. Select All then Copy must report the clipboard size
   limit without trying a 4 GB allocation. Scroll with the scrollbar and trackpad. Modify a
   disposable source externally; a fresh snapshot should appear without mapping faults.
5. Close/restart with huge or binary tabs open, including a loading tab. Restore must use a
   disk reference and worker loading, with the same save restrictions, rather than treating
   the old prefix as a complete document. Verify ordinary small-file typing/scroll/IME still
   behave as in previous rounds.


## Round 13: release wiring and VoiceOver (human review required)

Run on an unlocked **macOS 14+** desktop, outside the sandbox. The script prepares a disposable
profile, a Unicode document, a windowed document and a local declarative package with a
Python marker. The isolated `vel-crash` tests exercise actual panic/signal hooks. The script
does not install the CLI or any package, or change the home directory. Choose a fresh fixture
directory; the script refuses to replace an earlier profile.

```sh
cargo build --release --locked -p velocity
scripts/package-macos.sh /tmp/velocity-r13-bundle
```

The reviewer retains the existing minimum-macOS-14 decision and the DMG signing fix. Inspect
`packaging-report.json`: prefer `hdiutil create` and require strict deep codesign of the mounted
app for distribution; a `makehybrid` fallback is not evidence of a distributable signature.

Release actions:

For the next-launch prompt, first create a uniquely named synthetic report in the normal
local crash directory (this does not crash the app or alter previous reports), then launch:

```sh
python3 - <<'PY'
from pathlib import Path
import uuid
folder = Path.home() / 'Library/Logs/Velocity'
folder.mkdir(parents=True, mode=0o700, exist_ok=True)
report = folder / ('reviewer-fixture-' + uuid.uuid4().hex + '.crash')
with report.open('x') as stream:
    stream.write('Synthetic Velocity reviewer report; no document contents.\n')
report.chmod(0o600)
print(report)
PY
```

Launch the prepared bundle with a fresh fixture profile:

```sh
zsh crates/velocity/scripts/review-round13-gui.zsh \
  /tmp/velocity-r13-bundle/Velocity.app /tmp/velocity-r13-review
```

After quitting, relaunch that same profile without rerunning fixture setup:

```sh
VELOCITY_GUI_PROCESS=1 /tmp/velocity-r13-bundle/Velocity.app/Contents/MacOS/velocity \
  --data-dir /tmp/velocity-r13-review/profile
```

- [ ] Launch: the native crash alert offers **Open Report Folder** and **Dismiss**. Open reveals
  `~/Library/Logs/Velocity`; dismiss opens nothing. The shown report becomes
  `.seen` and remains readable. Launch the executable again with the same profile,
  without creating another report: the alert does not recur. Help → Open Crash Reports Folder
  reveals that same local directory. No report upload occurs. The synthetic report can be
  removed manually after review.
- [ ] Velocity → Install Command Line Tool… shows exact source/destination paths and two bin
  choices. Select `~/.local/bin` (use Alternative when system bin is preferred).
  Cancel writes nothing; confirm creates the symlink; repeat reports an existing installation.
  Run `~/.local/bin/velocity --help`. For actual file forwarding,
  use `--data-dir /tmp/velocity-r13-review/profile`. Do not select
  `/usr/local/bin` for this disposable review. Unrelated existing commands must yield an error.
  If a new local symlink was installed for this review, remove that symlink after checking its target.
- [ ] Cmd+Shift+P → Package: Install from File… → `Reviewer Fixture.sublime-package` in the
  fixture root. Keep typing while the worker runs. Set Syntax: Reviewer Fixture, Snippet:
  Reviewer Fixture and the `reviewer` build system become available without reopening documents.
  Package: List reports resources and **partially supported: no plugin API**; select its row
  to inspect contribution paths. The Python marker never executes.
- [ ] Package: Disable/Enable removes/restores its resource choices live. Unsaved text, undo,
  selections, folders and open tabs survive. The preferences retain `KeepDisabled` and unrelated
  settings. Package: Remove removes its contributions and reports recoverable backup paths.
  Replacing an installed archive also reports the previous archive’s backup.
- [ ] Package: Install from URL… accepts a reviewer-controlled HTTPS archive URL, then a package
  name, including URLs with query parameters. Invalid TLS, HTTP URLs and invalid ZIPs show errors
  while the editor stays usable. No public service is configured or needed for local checks.
- [ ] Package: Check for Upgrades is absent with `package_index_url: null`. With an explicit
  reviewer-controlled HTTPS version-1 index (schema in `vel-packages/README.md`), it appears.
  Nothing fetches until invoked. Available/current versions appear before an installation
  confirmation. Cancel retains the archive; confirmation checks digest/metadata and retains
  disabled state. Restore the setting to null after review.

VoiceOver: enable with **Cmd+F5** (or the system accessibility shortcut). **VO** below means
Control+Option. Use Accessibility Inspector alongside VoiceOver to inspect the public AX values.

- [ ] Interact with the workspace/text area (VO+Shift+Down). The editor has role `AXTextArea`,
  a document label and the visible/windowed text value. Read `Hello 你好 😀 é` in order. Arrow,
  Shift+Arrow and mouse selection changes announce their current caret/selection rather than
  stale text. Typing, native IME preedit/commit and Undo update the value and selection once.
- [ ] Inspect `AXSelectedText`, `AXSelectedTextRange(s)` and `AXInsertionPointLineNumber`.
  UTF-16 offsets count 😀 as two units; combining marks retain valid scalar boundaries. Reverse
  a multiline selection: insertion line follows the actual primary caret. Add multiple carets:
  AX exposes the primary only; typing still edits all editor carets. Setting an AX range updates
  the primary editor selection, and no half surrogate is exposed or replaced.
- [ ] Query `AXLineForIndex`, `AXRangeForLine`, `AXStringForRange` and `AXBoundsForRange` in
  Inspector. Include newline, end-of-document and empty ranges. Bounds align with text at 1×/2×,
  after horizontal/vertical scrolling, wrapping, folds, window resize, UI scaling and splits.
  Screen bounds use points; a Retina display must not double offsets or sizes.
- [ ] Open `project/windowed.txt`, move near the end and query its value. Each value has at most
  **65,536 UTF-16 units**; Help reports its document offset and first line. Selected/parameterized
  ranges are relative to that value; insertion line remains document-relative. Move the caret
  beyond the old window and confirm the next value/range queries track it. Whole-value AX
  replacement is unavailable for a partial window. No full-document flattening or UI pause.
- [ ] Traverse with Cmd+F6 / Cmd+Shift+F6 and VO navigation. Tabs expose document names, selected
  state and separate Close buttons; Return/Space or VO+Space activates focused controls. Visit
  both tabs, close/cancel a dirty tab, then switch groups. The primary editor follows the selected
  group. Closing the quick panel restores the editor’s focus.
- [ ] The side bar is an outline with labelled open-file/project rows, disclosure depth and
  expansion state. Select/open files and expand/collapse directories with AX actions. Hidden
  rows do not become duplicate elements. Goto/palette rows expose captions, badges/details and
  selected state; filtering and Enter/VO+Space choose the announced row.
- [ ] Find/Replace and Find in Files fields have distinct Find/Replace/Where labels. Regex,
  case, whole-word, wrap, selection and preserve-case controls have named actions; Find Next,
  Find All and Replace All buttons work. Standard Tab/Shift+Tab retains input routing; the
  explicit Cmd+F6 traversal includes non-text controls.
- [ ] Status text announces line/column, selections, encoding and current message. The syntax
  popup opens its picker. Completion rows, context-menu choices and visible minihtml popup
  text/links have semantic names/actions. Activate a popup link and confirm its normal action.
- [ ] System Settings → Accessibility → Display → Reduce motion, toggled **while running**:
  theme tint/opacity transitions and texture sequences settle immediately; no theme animation
  resumes on resource reload. Toggle Increase contrast: chrome control boundaries strengthen.
  Toggle back and confirm ordinary animations/appearance return. Custom syntax color contrast
  remains the scheme author’s choice.

Record macOS version, display scale, VoiceOver version/verbosity, exact failing action and AX
attribute values. GUI behavior, actual speech and screen-coordinate alignment are **pending
human review**; headless tests do not establish these outcomes. Binary hex documents currently
have a read-only description rather than a navigable AX hex-text value.

## Round 14: native opens, context menus and round-12 regressions

Run on an unlocked macOS 14+ desktop, outside the sandbox, using a bundle prepared by the
reviewer. This round does **not** run the packaging script. The headless plist test only
serializes document-type declarations; it does not assemble, sign or launch an application.
Replace the app path below with the reviewer's updated bundle. Use a new fixture directory.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --release
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps
/usr/bin/python3 -B -m unittest discover -s crates/velocity/packaging -p 'test_bundle.py'
cargo build --release --locked -p velocity
python3 crates/velocity/scripts/review-round14-fixtures.py /tmp/velocity-r14-native --huge

APP=/absolute/path/to/Velocity.app
FIXTURE=/tmp/velocity-r14-native
PROFILE="$FIXTURE/profile"
DOCS="$FIXTURE/Project with spaces"
# Cold LaunchServices launch: the filename travels in the open Apple event, not argv.
/usr/bin/open -na "$APP" "$DOCS/one.rs" --args --data-dir "$PROFILE"
# With the first instance still running, route to its key window.
/usr/bin/open -a "$APP" "$DOCS/two.txt"
# CLI locations still use the existing per-profile socket.
"$APP/Contents/SharedSupport/bin/velocity" --data-dir "$PROFILE" "$DOCS/one.rs:120:7"
```

- [ ] **Cold Finder Open With:** quit Velocity, then Finder → Open With → this bundle for
  `one.rs`. Repeat for `any-file.velocity-blob`, a folder, and `literal:12:3`. Text opens in
  a tab, binary data in read-only hex, folders in the sidebar, and the native colon filename
  remains literal. Text/data are Alternate Editor handlers; folders are Alternate Viewer
  handlers. Check the receiving process remains alive rather than handing off an empty GUI.
- [ ] **Double-click:** on a disposable fixture extension, Get Info → Open With → Velocity →
  Change All…, quit, and double-click. Verify cold and running launches. Restore the previous
  association afterward. Velocity must not take document ownership merely by being installed.
- [ ] **Dock drop:** drag several files and a folder onto Velocity's Dock icon, first cold and
  then running. While startup/loading is busy, issue another open. Requests retain order and
  are applied after window initialization. Drop a duplicate, a symlink alias and an edited
  open file: focus its existing tab while preserving its text, selections and scroll.
- [ ] **Window drop:** drop files/folders over the editor, tab row and sidebar, including
  Unicode/space-containing paths. Existing text dragging/input behavior remains usable. Drop
  `huge.log` and the binary: retain background prefix/huge-file and hex behavior. Use an
  inaccessible or subsequently removed file and confirm a path-specific error, with later
  valid entries still opened.
- [ ] **Running instance/windows:** open two windows, make the second key and use Finder Open
  With and `open -a` again. Only that key window receives the files. Close all windows without
  quitting, then click the Dock icon: create a fresh window. Repeat with a Finder open and a
  CLI request while no window exists. The retained template polls IPC without rendering.
  `velocity --data-dir "$PROFILE" -w "$DOCS/two.txt"` must return when its sheet/window closes.
  Force a secondary LaunchServices process with
  `open -na "$APP" "$DOCS/literal:12:3" "$DOCS/two.txt" --args --data-dir "$PROFILE"`:
  its queued batches must reach the primary before the temporary receiver quits. Repeat while
  the primary quits; takeover must retain all recovered windows and dirty documents.

Menu resources are original Velocity files in `Packages/Default`: `Context.sublime-menu`,
`Tab Context.sublime-menu` and `Side Bar.sublime-menu`. Enabled packages and User extend their
stable IDs with the same resource precedence as the menu bar. `velocity_dynamic` inserts the
existing spelling, diff and Git choices. Commands receive resource args plus literal `paths`,
clicked `group`, group-local `index`, and the window-local `event: {x,y}` point. Internal sheet
identity keeps targets stable while workers/IPC progress during tracking.

- [ ] **Native tracking:** right-click, two-finger click and Control-click on each surface.
  Verify native menu appearance, keyboard navigation, Escape dismissal, disabled rows and
  checks. Keep a menu open while indexing/loading and IPC complete; select an action, then
  type/scroll immediately. No borrow panic, hung tracking loop or menu-bar replacement occurs.
  During the spelling-worker wait, Escape, typing and scrolling cancel the deferred popup.
  A User menu `chain` and a window command such as `toggle_setting` inherit the clicked tab's
  context and check its state even when another tab was previously active.
- [ ] **Editor:** right-click inside either of two selections and Copy: preserve all regions.
  Right-click outside all selections: move the caret first. Repeat in an inactive split.
  Cut/Copy/Paste/Select Document, cached/worker spelling replacements, dictionary actions,
  diff show/revert and configured Git actions work on the pointed document/hunk/word. Copy
  Full File Path, Locate in Project Tree and Show in File Browser use the pointed document.
- [ ] **Tabs:** right-click an inactive tab and use Close This Tab, Keep Only This Tab, Close
  Following Tabs and Close Saved Tabs. Targets follow that group's visual order. Dirty
  prompts, Save/Discard/Cancel and shared views retain their usual behavior. Create Untitled
  File opens in the clicked group; path/reveal actions use the clicked tab.
- [ ] **Sidebar:** Open Selected Entry expands directories/opens files. Create File/Folder
  and Change Entry Name use the existing input panel; Escape cancels, an existing destination
  reports an error, and separators/traversal names are rejected. Rename an open dirty file and
  a parent directory: retarget all sheets/descendants, keep undo and watch the new path.
  Move Entry to Trash first asks confirmation; Cancel changes nothing. Confirm moves to Finder
  Trash, including symlink entries, with no permanent-delete fallback. Open buffers remain
  readable, marked `[deleted]` and dirty, and can be saved. A previously clean deleted buffer
  must prompt on close and survive Hot Exit; saving either shared view clears the markers.
  Clipboard/reveal/permission errors surface.
- [ ] Copy Full Path and Copy Project-Relative Path have the expected text. Search This Folder
  opens Find in Files with that directory in Where. Detach Project Folder removes the root
  without deleting its files; configured Git choices retain the selected path. Repeat with
  User menu overrides, disabled packages and a hot resource reload.
- [ ] **Accessibility:** VoiceOver reads native menu items and their disabled/checked state;
  no second custom context list appears in the workspace AX tree. Dismissal leaves the round-13
  editor/tab/sidebar/input AX elements and focus behavior intact.

Round-12 regressions and change checks:

- [ ] CLI `one.rs:120:7` centers line 120 at startup and over IPC, even after restoring an old
  scroll position. Repeat in a resized/split/wrapped view and while a large file is loading.
- [ ] In light and dark themes at multiple UI scales, the strip above the sidebar matches its
  themed background and contains no black clear-color rectangle.
- [ ] Window title is the file name plus dirty marker and optional ` — project`; switching,
  typing, saving and external changes update it. No debug/native AppKit title or encoding/EOL
  suffix. Unchanged frames do not call the native title setter.
- [ ] UI labels/headings use system sans-serif regular/bold/italic faces at their actual sizes.
  No CoreText message about requesting private `.SFNS-*` names or substituting Times appears.
  The renderer side is `vel-render/src/native.rs` (`font_location` uses the CoreText UI font for
  system names).
- [ ] A normal launch creates no interactive telemetry file. Quit, then opt in with the direct
  command below. Confirm the exact default name `interactive-<runid>-<pid>.jsonl`, valid JSONL,
  and the JSON mode/PID; no compile-time source directory receives output. Unset
  `VELOCITY_LOG_DIR` and repeat to check `~/Library/Logs/Velocity/`. Explicit `--output` is an
  opt-in destination, creates a new file and refuses to overwrite one.

```sh
VELOCITY_GUI_PROCESS=1 VELOCITY_LOG_DIR="$FIXTURE/logs" \
  "$APP/Contents/MacOS/velocity" --data-dir "$PROFILE" --log-input "$DOCS/one.rs"
```

- [ ] **Reviewer regressions:** Cmd+F/Replace/Incremental/Find in Files toggle; Escape from
  editor focus and each find panel's close button hide the panel/match paint without losing
  Find Next. Scroll and mouse moves stay coalesced on the display link; keys/clicks remain
  immediate. The resource-stamp walk runs at most every 500 ms; an explicit package reload
  takes effect immediately.
- [ ] **Real-event profile:** while scrolling a 10 MB file using the trackpad, sample the app
  (for example `sample PID 10 -file /tmp/velocity-r14-native/native-sample.txt`). Inspect
  native menu/title work: no per-frame `setMainMenu`/menu reconstruction or unchanged title
  mutation. Record receipt-to-submit p50/p95, display refresh rate and frame gaps using lane
  B's native scroll gate after integration. Headless CPU numbers do not establish native
  AppKit, font rasterization, GPU, scanout, VoiceOver or Finder/Dock outcomes.

Paired CPU reviewer commands (use the retained clean `4b65f50` binary as BEFORE):

```sh
python3 crates/velocity/scripts/measure-typing.py \
  --before /tmp/velocity-r14-baseline/velocity-main-4b65f50 \
  --after target/release/velocity --runs 10 --output-dir /tmp/velocity-r14-typing-review
python3 crates/velocity/scripts/compare-scroll.py \
  --old /tmp/velocity-r14-baseline/velocity-main-4b65f50 \
  --new target/release/velocity --pairs 10 --output-dir /tmp/velocity-r14-scroll-review
```

Run pairs sequentially with no concurrent compilation. Retain raw JSONL, binary hashes and
summary confidence intervals. Typing and scrolling each have a **≤5% regression** budget;
report CPU budgets separately from pending native measurements.
