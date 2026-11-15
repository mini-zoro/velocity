# Spec: Vintage (vi) engine, `.tmLanguage` support, highlighting throughput to 2.5 MB/s

Status: P3/P2 / lane B (engines). Locations: new `crates/vel-vintage/`, `crates/vel-syntax/`,
`crates/vel-regex/`.

## 1. `vel-vintage` — vi emulation engine (pure, headless)

References: https://www.sublimetext.com/docs/vintage.html (what Vintage supports, `vintage_start_in_command_mode`,
`vintage_ctrl_keys`, `vintage_use_clipboard`), plus standard vi semantics (public knowledge).

- A state machine over `vel-buffer` (`Buffer` + `ViewState`) with modes: insert, normal (command),
  visual (char/line/block → multiple selections, as Sublime does), replace, and command-line `:`
  for the small set ST's Vintage supports (`:w`, `:wq`, `:q`, `:e`, line numbers).
- Counts, operators (`d c y > < = g~ gu gU`), motions (`h j k l w W b B e E ge 0 ^ $ gg G f F t T ; ,
  % { } ( ) H M L`, search `/ ? n N * #`), text objects (`iw aw iW aW is as ip ap i( a( i[ a[ i{ a{ i<
  a< it at i" a" i' a' i\` a\``), registers (`"a`–`"z`, `"0`, `"`, `"+` mapped to the clipboard when
  `vintage_use_clipboard`), `.` repeat, macros `q`/`@`, marks `m`/`'`/`` ` ``, undo/redo through the
  buffer's history (one vi command = one undo group), `J`, `r`, `~`, `x X s S C D Y p P o O a A i I`.
- **Multiple cursors work with vi commands** (Sublime's Vintage applies commands to every selection).
- API: `Vintage::handle_key(&mut self, key, &mut Buffer, view, &dyn VerticalLayout) -> Outcome`
  where `Outcome` says handled / pass-through / needs-command(name, args) so lane A can bind it to
  the keymap via contexts (`setting.command_mode`, `vi_*` context keys as in ST's Vintage keymap
  semantics — write our own keymap file later in lane A).
- Tests: a table of `(initial text with caret markers, keys, expected text with caret markers)`
  covering every command above, incl. counts, multiple cursors, Unicode graphemes; property tests
  that random command sequences never panic and keep selections valid.

## 2. `.tmLanguage` support in `vel-syntax`

Currently explicitly unsupported. Convert TextMate grammars (plist XML; also accept the JSON
`.tmLanguage.json` form) to the internal representation: `patterns`, `repository`, `include`
(`#name`, `$self`, `$base`, `source.x`, `source.x#name`), `match` + `captures`/`name`,
`begin`/`end` + `beginCaptures`/`endCaptures`/`contentName`/`name`, `while` (as supported), back-
references from `begin` in `end`, `applyEndPatternLast`, `injections` if feasible (document if not).
Tests: convert and parse a few grammars from permissively licensed public repositories fetched at
test time (skip offline), plus hand-written fixtures; verify against expected scopes.

## 3. Highlighting throughput (P2 target still open)

Rust background parse+style is 2.00 MB/s vs the 2.5 MB/s target; slow contexts `Markdown::__main`
(0.32×) and `Rust::type-path` (0.39×) remain. Profile and fix with general rules (no per-syntax
hacks); keep the differential test at zero divergences and the corpus baseline monotonic.
Report per-language numbers in `crates/vel-regex/PERF.md`.

## Rules

- Your worktree: /Users/zoro/dev/velocity-lane-b (branch `lane-b`). Modify only `crates/vel-vintage/`,
  `crates/vel-syntax/`, `crates/vel-regex/`, `crates/vel-scope/` (+ root `Cargo.lock`). Do not commit.
- fmt, clippy -D warnings, release tests, rustdoc -D warnings for touched crates; differential test.
