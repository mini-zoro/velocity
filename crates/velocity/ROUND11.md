# Round 11: typing, Vintage, user grammars

Changes stay in `velocity`, `vel-ui` and root Cargo.lock. `vel-vintage`, `vel-syntax`,
the large-file engines and their resources are consumed read-only. No commit, GUI or Metal run.

## Mandatory typing gate

The merge binary was frozen before editing. All comparisons use the specified round-nine
`/tmp/velocity-round10-baseline/velocity`, `--headless-intelligence --review-ui on --intel-words 0`,
500 timed edits/scenes per process and ten alternating AB/BA process pairs. No compilation,
tests or other lane-A benchmark runs during a series. Paired process bootstrap uses 20,000
resamples with a fixed seed. The runner reports both median paired ratios and ratios of
process medians, with paired 95% intervals; **both upper bounds** must be ≤1.05 to pass.

| State vs round 9 | Paired p50 change | Paired 95% CI | Ratio of medians / CI |
| --- | --- | --- | --- |
| Merge `3f4f4cb` before changes | +6.89% | +3.90…+9.32% | +3.82% / +1.18…+9.23% |
| Performance fix, before feature work | −13.11% | −14.10…−12.06% | −13.26% / −13.83…−12.06% |

The finished binary measured **-13.83%** paired p50 (95% CI **-15.28%…-13.26%**), with both upper bounds below +5%.

The gate passed before starting Vintage/grammar implementation. The final binary comparison
is recorded in `timings-round11.json` and RESULTS.md after all implementation checks.

The retained widget cache checks the complete original path (including ancestor attributes)
and immutable settings identity. Theme/palette/selector changes invalidate it. Animation
sampling still runs every frame, including texture sequences; settled styles keep their
sampled endpoints. This removes repeated resolver hashing/guards and paint-cache probes.
Line padding and caret indentation/guide options are computed once per scene, keeping the
same paint output. Existing structural goldens pass without regeneration.

Separate diagnostic processes measured editor scene p50 0.3685→0.3082 ms and chrome
0.0444→0.0250 ms. Resolver calls across those workloads fell from 583,257 to 74, with
26 paint conversions in each. These diagnostic processes are not an alternating A/B;
the process-paired series above establishes the gate. `VELOCITY_REVIEW_PROFILE=1` also
reports layout, language/annotations, text, brackets/carets, minimap, scrollbar,
completion and HTML phases. Empty phantom/annotation and popup checks remain enabled.

## Editor behavior

Vintage is **off by default**. Set `"vintage_enabled": true` in User preferences or toggle
the active view with Ctrl+Alt+V. `vintage_start_in_command_mode` defaults to false;
`vintage_ctrl_keys` and `vintage_use_clipboard` also default to false. Start mode applies
when enabling a view. Changing control/clipboard options restarts the vi session in its
current insert/command mode; text and selections remain, while engine journals/registers reset.
The integration is original, based on the [public Vintage documentation](https://www.sublimetext.com/docs/vintage.html)
and the existing engine's [coverage matrix](../vel-vintage/COVERAGE.md).

The shared native/headless route gives focused widgets priority, then calls Vintage before
the ordinary document keymap. `Handled`, `PassThrough` and `NeedsCommand` have distinct paths.
Printable insert-mode key events reach the normal keymap and native input method; committed
Unicode text enters Vintage once, after composition/replacement mapping. Command mode consumes
character keys before native interpretation and ignores document preedit/commit callbacks.
Our `Velocity Vintage.sublime-keymap` uses vi contexts; user keymaps can query
`setting.command_mode` and all engine `vi_*` predicates.

Command/replace/visual modes show block carets and caret-line/selection highlights. The status
bar shows mode, pending count/operator/motion and macro recording. `:`, `/` and `?` open the
ordinary buffer-backed input panel with its clipboard/IME/editing behavior. Enter submits
through the engine; Escape, focus changes and disabling Vintage cancel the engine prompt.
Search wraps and supports pending operators. Normal `n`/`N` reuse the engine search.

Vi edits publish their real ChangeSets to layout, highlighting, annotations and clones.
Multi-step macro/history operations use the existing snapshot reset when a single map is
unavailable. Clipboard requests go through the registry and native clipboard bridge. Reindent
runs through the existing asynchronous registry command; accepted completion calls
`complete_host_command` and remains one vi undo. New vi input cancels an unfinished reindent
and proceeds immediately; stale completions cannot change newer text.
`:w`, `:wq`, `:q`, `:e`, optional paths and `!` use the ordinary save/open/close host flow;
`:e` without a path reloads the current file on a worker. Unforced dirty closes/reloads retain
the normal save/discard checks, and failed/cancelled saves do not close a view.

Engine compatibility limits still apply: regex uses Rust syntax, there is no full Vim Ex
language or persistent macro/mark storage, and external host edits (e.g. completion/snippet
transactions) invalidate engine undo grouping. Native IME/caret/clipboard/file-dialog behavior
still requires the reviewer checklist; headless evidence does not establish visual correctness.

## User grammars

Loose `.sublime-syntax`, `.tmLanguage` and `.tmLanguage.json` definitions load in place from
`<data>/Packages/**`, after the shipped root, via `SyntaxSet::load_cached_roots`. Plain Text
remains an in-memory virtual definition. Archive grammars use the already-supported resource
overlay from `Installed Packages/*.sublime-package`; effective archive/loose definitions are
passed as virtual extras without extraction, with loose overrides and User-last precedence.
Scope preferences follow the same roots/overlay.

Files and containing package directories are observed for edits, additions, removals and
new nested directories. Reload rebuilds detection, picker/palette entries and open-view
language services without replacing buffer contents/history. Missing or removed choices fall
back to Plain Text. Corrupt definitions produce loader diagnostics while valid ones remain;
corrupt caches rebuild, and unavailable/read-only caches fall back to uncached root loading.
The JSON and XML examples under `fixtures/user-grammars` are original local fixtures.

See RUNBOOK.md for exact verification commands and MANUAL_TEST.md for the native handoff.
