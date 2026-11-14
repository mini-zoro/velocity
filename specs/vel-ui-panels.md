# Spec: quick panels, Goto Anything, command palette, find/replace, find in files, side bar

Status: P1/P2 / lane A (platform, UI). Locations: new `crates/vel-ui/` (widgets) and
`crates/vel-search/` (search engines), integration in `crates/velocity/`.

Goal: the navigation and search features that make Sublime feel like Sublime, with its documented
behavior and key bindings: https://www.sublimetext.com/docs/ (Command Palette, Goto Anything,
Projects, Side Bar sections) and the `find`/`replace`/`find_in_files` command names from the API
reference. Clean-room rules as always — our own drawing and default files.

## `vel-ui` widgets (own drawing via `vel-render` scenes; the `.sublime-theme` engine comes in P3)

- A small retained widget layer: layout (rows/columns/stack; `taffy` is allowed), focus, keyboard
  routing (overlay > panel > view), hit testing, hover, scroll containers, text inputs that use
  `vel-buffer` underneath (so editing keys, IME and undo work inside inputs), and an animation
  hook. Every widget exposes the `class` + `attributes` (hover, pressed, selected, dirty, …) that
  `.sublime-theme` rules will target later — keep a mapping table in the README.
- **Quick panel** (overlay): input + virtualized list with fuzzy-highlighted matches, kind
  badges, annotations/details, keyboard navigation, live preview callback (transient sheet).
- **Input panel** and **output panel** (bottom, e.g. build output later).
- **Tab bar** improvements (scrolling tabs, close buttons, dirty dot, drag to reorder) and a
  **status bar** (line/col, selection count, encoding, line endings, indentation, syntax).
- **Side bar**: folder tree (lazy directory loading, `.gitignore`-aware via `ignore` crate +
  `folder_exclude_patterns`/`file_exclude_patterns`/`binary_file_patterns` settings), "Open Files"
  section, click to preview (semi-transient), double-click to open, keyboard navigation,
  toggle `toggle_side_bar`. Projects: `.sublime-project` folders/settings, `open_dir`, adding
  folders; `.sublime-workspace` session can come later.

## `vel-search`

- **Fuzzy matcher** for Goto Anything: start from `nucleo-matcher` (or own) and tune ranking
  toward ST's documented/observed behavior: prefer matches on file-name over path, consecutive
  and word-start matches, shorter paths; recency boost. Index of all project files built in
  the background (`ignore` parallel walker), updated by file watching.
  **Budget: ≤ 10 ms per keystroke at 100k files** (P0 measured 6–73 ms with naive rescoring — use
  incremental narrowing from the previous query, top-K partial sort, background scoring with
  cancellation of stale queries).
- **Goto Anything** syntax: `file`, `file:line`, `file:line:col`, `file@symbol` (symbols from the
  current file via `vel-syntax` symbol metadata once available — leave a hook), `#term` (fuzzy word
  search in file), `:line`, `@symbol`; Ctrl/Cmd+Enter opens side by side (needs multi-select
  sheets — a simple split is fine).
- **Command palette** from `.sublime-commands` files (+ menu-derived commands), with captions,
  key binding hints, and recently used ordering.
- **Find / Replace panel** (`show_panel {panel: find|replace|incremental_find}`): regex, case,
  whole word, wrap, in-selection, highlight matches, preserve case on replace; `find_next`,
  `find_prev`, `find_all`, `replace_next`, `replace_all`, `find_under`, `find_under_prev`,
  `slurp_find_string` (Cmd+E), `slurp_replace_string`; regex search on the rope via
  `vel-text`'s `regex-cursor` integration (no full-buffer copies). Regex syntax compatible with
  Sublime's (Boost-like Perl syntax) as far as the `regex` crate allows; document differences.
  Budget: highlight all matches in a 100 MB buffer for a literal in ≤ 200 ms (async beyond the
  viewport), incremental find per keystroke ≤ 5 ms for the viewport.
- **Find in Files** (`find_in_files` panel: Where field with folders, `<open files>`,
  `<project>`, include/exclude patterns like ST): parallel search with `ignore` + `grep-searcher`,
  streaming results into a **Find Results** view (Sublime's format: file headers, line numbers,
  context lines, match highlighting), double-click / Enter to jump; replace in files with confirmation.

## Integration and tests

- Key bindings in our own default keymap for all of the above (Cmd+P, Cmd+Shift+P, Cmd+R, Ctrl+G,
  Cmd+F, Cmd+Alt+F, Cmd+Shift+F, Cmd+G/Shift+G, Cmd+E, Cmd+K Cmd+B, …).
- Headless tests: fuzzy ranking golden tests (write a small table of query → expected top-3 on a
  fixed file list), Goto Anything parser, find/replace semantics incl. preserve case and regex
  groups, find-in-files result formatting, side-bar exclusion rules, widget focus routing.
- GUI measurements by the reviewer (sandbox has no window server); update RUNBOOK.md and add a
  human checklist to `crates/velocity/IME_TEST.md` (rename it `MANUAL_TEST.md` and keep the IME
  section).

## Rules

- Create/modify only: `crates/vel-ui/`, `crates/vel-search/` (new), `crates/velocity/` (except
  `crates/velocity/resources/Packages/Color Scheme - Velocity/`, owned by lane B),
  `crates/vel-platform/`, `crates/vel-render/`, `crates/vel-config/`, `crates/vel-commands/`,
  `crates/vel-fileio/` (+ root `Cargo.lock`). Do not modify `vel-text`, `vel-buffer`, or lane B's
  crates (`vel-scope`, `vel-regex`, `vel-syntax`, `vel-color-scheme`, `vel-highlight`).
