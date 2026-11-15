# Spec: HTML export/print, command-coverage audit, remaining performance targets

Status: P4 / lane B. Locations: new `crates/vel-export/`, `crates/vel-buffer/` (ops), `crates/vel-commands/`
(command adapters only — coordinate: lane A owns the rest of lane-A crates), `crates/vel-regex/`, `crates/vel-syntax/`.

## 1. HTML export (ST4: "Edit/Copy as HTML", "File/Print — prints via a browser")

- `vel-export`: render a buffer range (or selections) with its highlight spans and color scheme to standalone HTML
  (inline styles or a `<style>` block, background/foreground, font family and size, bold/italic/underline, optional
  line numbers, tab expansion, HTML escaping, Unicode intact); clipboard HTML + plain-text flavours for "Copy as HTML";
  a print path that writes a temporary HTML file and returns its path for the app to open in the default browser.
- Budget: export of a 10k-line highlighted file ≤ 50 ms; golden-output tests.

## 2. Command-coverage audit

- Build a table of Sublime command names that appear in the public API reference and documentation (commands invoked
  via key bindings, menus and `run_command` examples) vs what `vel-commands` implements. Write it to
  `crates/vel-commands/COVERAGE.md` (implemented / partially / missing / out of scope with reason — e.g. Python-only).
- Implement the missing **text-level** operations in `vel_buffer::ops` with tests (examples to check: `wrap_lines`,
  `permute_lines` {reverse, unique, shuffle}, `permute_selection`, `sort_selection`, `title_case`/`swap_case` coverage,
  `convert_ident_case`-style Convert Case variants listed in the ST4 changelog (snake_case, camelCase, PascalCase,
  kebab-case, UPPER_SNAKE), `select_to_mark`/`swap_with_mark`/`set_mark`/`clear_bookmarks`/`toggle_bookmark`/
  `next_bookmark`/`prev_bookmark`, `expand_selection_to_paragraph`, `split_selection_into_lines` edge cases,
  `trim_trailing_white_space`, `chain`), plus thin adapters in `vel-commands` so they're callable by name. Behaviour
  not documented publicly: mark `ST-BEHAVIOR: unverified` and list in `crates/vel-buffer/BEHAVIOR.md`.

## 3. Remaining performance targets (paired A/B only)

- C++ background parse+style ≥ 2.5 MB/s (currently 2.40); warm syntax-cache load p95 ≤ 20 ms (currently 21.2 ms with
  204 definitions). General rules only, zero differential divergences.

## Rules

- Your worktree: /Users/zoro/dev/velocity-lane-b (branch `lane-b`). Do not commit. Modify `vel-export` (new), `vel-buffer`
  (additive), the command adapter file(s) in `vel-commands` only, `vel-regex`, `vel-syntax`, `vel-scope` (+ root `Cargo.lock`).
  Lane A wires menus/keymaps next round.
- `cargo test --workspace --release`, fmt, clippy -D warnings, rustdoc -D warnings; vel-buffer perf bench (no regression > 5%).
