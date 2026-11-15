# Spec: typing regression first; Vintage mode in the editor; user TextMate grammars

Status: P3 / lane A. Locations: lane-A crates (`velocity`, `vel-ui`, `vel-render`, `vel-platform`,
`vel-config`, `vel-commands`, …). Consumes (read-only) `vel-vintage` and `vel-syntax`.

## 0. Mandatory first: typing back within the 5% gate

Round 10 (theme engine + minihtml UI) was merged under an explicit exception: the reviewer's
independent alternating A/B (8 pairs, `--headless-intelligence --review-ui on --intel-words 0`, round-9
binary `/tmp/velocity-round10-baseline/velocity` vs main `7c84f61`) measured **typing p50 +5.49%
(95% CI +1.9..+9.5%)**. Profile the per-keystroke path (theme resolution per widget per frame, minihtml
popup layout/paint, phantom/annotation checks, scene building) and get the paired p50 ratio's 95% CI
**upper bound ≤ +5%** against that same round-9 binary, without removing features. Report before/after
with ≥ 10 alternating pairs. Do not proceed to the rest until this holds (if it truly cannot, explain the
irreducible cost with profile evidence).

## 1. Vintage (vi) mode

References: https://www.sublimetext.com/docs/vintage.html and `crates/vel-vintage/README.md` +
`COVERAGE.md`. Vintage is **off by default** (like Sublime: it's in `ignored_packages` until enabled);
enable via a setting (`vintage_enabled` or Sublime-compatible removal from `ignored_packages` — pick one,
document it), plus `vintage_start_in_command_mode`, `vintage_ctrl_keys`, `vintage_use_clipboard`.

- Route key events to `Vintage::handle_key` when enabled, before the keymap; handle `Handled`,
  `PassThrough`, `NeedsCommand(Command)` (run through the command registry), and host completions
  (`complete_host_command`, e.g. after reindent). Keymap contexts for vi state (`setting.command_mode`
  and the engine's `context` keys) in **our own** keymap file `Velocity Vintage.sublime-keymap`.
- Visuals: block caret in command mode, line/visual highlighting, the mode and pending command/count in
  the status bar, the `:` command line in the input panel, `/` and `?` search through the find UI or an
  input panel.
- Multiple cursors with vi commands (the engine supports it); IME must keep working in insert mode and be
  bypassed in command mode.
- Tests: headless key sequences through the full app path (keymap → Vintage → buffer → scene) for a
  representative set of commands; IME insert-mode test; settings toggles at runtime.

## 2. User TextMate grammars

`vel-syntax` now loads `.tmLanguage` / `.tmLanguage.json`. Make the editor pick up user grammars from
`<data>/Packages/**` (and `Installed Packages/*.sublime-package` if already supported) via
`load_cached_roots` with the user packages root after the bundled one; live reload on change; they appear
in the syntax picker and syntax detection. Test with a small fixture grammar.

## Rules

- Your worktree: /Users/zoro/dev/velocity-lane-a (branch `lane-a`). Do not commit. Modify only lane-A crates
  (+ root `Cargo.lock`). Lane B is working on large-file engines (`vel-largefile`, `vel-text`, `vel-highlight`,
  …) — don't touch them.
- Run `cargo test --workspace --release` before finishing. Fresh/read-only/corrupt profile robustness.
- Update `MANUAL_TEST.md` with a short Vintage checklist and the reviewer GUI script.
