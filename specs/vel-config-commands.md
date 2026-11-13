# Spec: `vel-config` + `vel-commands` + `vel-fileio` + editor integration in `velocity`

Status: P1 / lane A (platform, UI). Locations: `crates/vel-config/`, `crates/vel-commands/`,
`crates/vel-fileio/`, changes in `crates/velocity/` (and `vel-platform`/`vel-render` as needed).

Goal of this round: **Velocity becomes a usable plain-text editor**: open/save real files with
correct encodings and line endings, edit with `vel-buffer` (multi-cursor, undo/redo), all input
routed through Sublime-compatible key bindings and command names, several files in tabs.
No syntax highlighting yet (lane B is building `vel-syntax` in parallel; leave a `Highlighter`
trait seam with a plain-text implementation).

Clean-room (CONTRIBUTING.md): use Sublime's public docs —
https://www.sublimetext.com/docs/settings.html , key_bindings.html, menus.html, packages.html,
command_line.html, api_reference.html (command names/arguments) — and black-box behavior.
**Never reproduce Sublime's bundled `Default` package files** (keymaps, menus, settings). Write
our own default files from scratch, covering only commands we implement; individual key choices
may match Sublime's well-known shortcuts (functional facts), but the file content, order and
comments must be our own.

## `vel-config`

- **Lenient JSON** used by all Sublime resource files: `//` and `/* */` comments, trailing commas,
  UTF-8 BOM. Parse into a `serde_json::Value`-like tree with source spans; a writer that can
  update one key in a settings file **preserving comments and layout** (ST4 does this).
- **Settings**: layered lookup Default → Default (OSX) → User → syntax-specific
  (`<Syntax>.sublime-settings`) → project → per-view overrides, with change observers. Typed
  accessors for the settings we use (`tab_size`, `translate_tabs_to_spaces`, `word_separators`,
  `font_face`, `font_size`, `line_numbers`, `gutter`, `draw_white_space`, `rulers`,
  `ensure_newline_at_eof_on_save`, `trim_trailing_white_space_on_save`, `default_line_ending`,
  `default_encoding`, `fallback_encoding`, `scroll_past_end`, `caret_style`, `highlight_line`, …).
- **Resources**: a package resource loader over `Packages/<pkg>/` directories and
  `Installed Packages/*.sublime-package` zip files with Sublime's override order (loose files
  override zipped packages; `User` last). Our bundled defaults live in
  `crates/velocity/resources/Packages/Default/` (our own files).
- **Key bindings** (`.sublime-keymap`): key syntax (`super+shift+p`, `ctrl+alt+up`, named keys,
  `keypad_*`, sequences `"super+k", "super+b"`, modifier-tap sequences like `["shift", "shift"]`),
  `command` + `args`, `context` entries with `key`/`operator`/`operand`/`match_all`, operators
  `equal`, `not_equal`, `regex_match`, `not_regex_match`, `regex_contains`,
  `not_regex_contains`. Built-in context keys implemented now: `setting.*`, `selection_empty`,
  `num_selections`, `preceding_text`, `following_text`, `text`, `has_prev_field`/`has_next_field`
  (false for now), `overlay_visible`, `panel_visible`, `auto_complete_visible` (false for now),
  `is_recording_macro`; `selector` evaluates against a scope provider trait (plain text returns
  `text.plain`). Resolution: later files and later entries win; the first binding (from the end)
  whose context matches is used; sequences keep pending state with a timeout-free prefix like ST.
- `.sublime-mousemap`, `.sublime-menu`, `.sublime-commands` parsers (menus are wired to the native
  menu bar via vel-platform; command palette UI comes later).

## `vel-commands`

- Registry of commands by **Sublime command name**, with JSON args, three kinds
  (`TextCommand` operating on a view, `WindowCommand`, `ApplicationCommand`), `is_enabled`,
  `run`, and `chain` (`{"commands": [[name, args], …]}`).
- Text commands over `vel_buffer::ops` (names and argument shapes from the API docs / default
  keymaps semantics): `insert {characters}`, `paste`, `copy`, `cut`, `left_delete`, `right_delete`,
  `delete_word {forward, sub_words}`, `move {by, forward, extend, word_begin?, punct_begin?,
  empty_line?}`, `move_to {to, extend}`, `select_all`, `undo`, `redo`, `redo_or_repeat`,
  `soft_undo`, `soft_redo`, `find_under_expand`, `find_under_expand_skip`, `find_all_under`,
  `single_selection`, `split_selection_into_lines`, `select_lines {forward}`, `expand_selection
  {to}`, `indent`, `unindent`, `insert {characters: "\n"}` with auto-indent (copy previous line's
  leading whitespace), `insert_best_completion` (inserts a tab / spaces for now), `join_lines`,
  `swap_line_up`, `swap_line_down`, `duplicate_line`, `upper_case`, `lower_case`, `title_case`,
  `swap_case`, `transpose`, `sort_lines {case_sensitive}`, `reverse_lines` (via `permute_lines`
  semantics as documented), `scroll_lines {amount}`, `show_at_center`, `wrap_lines` (skip).
- Window commands: `new_file`, `prompt_open_file`, `open_file {file}`, `save`, `prompt_save_as`,
  `save_all`, `close`, `close_file`, `next_view`, `prev_view`, `select_by_index {index}`,
  `new_window`, `close_window`, `increase_font_size`/`decrease_font_size`/`reset_font_size`.
- Bracket/quote auto-pairing (`auto_match_enabled`) is implemented via key bindings with
  contexts, the way Sublime does it (`insert_snippet` with `$0` is the exception — implement a
  minimal `insert_snippet {contents}` supporting `$0`, `$1`.. and `${1:default}` placeholders,
  fields navigation can come later).
- Macro recording (`toggle_record_macro`, `run_macro`) — implement if time permits.

## `vel-fileio`

- Load: detect BOM (UTF-8, UTF-16 LE/BE), otherwise try UTF-8, otherwise `fallback_encoding`
  (default Western/Windows-1252) using `encoding_rs`; report the encoding. Line endings: detect
  LF/CRLF/CR (majority), normalize to LF in the buffer, remember per view (`set_line_endings`).
  Large files (> 64 MiB) load on a background thread with progress (use `vel_text::Text::from_reader`).
  Binary detection (NUL in the first 8 KiB → open as Hexadecimal is out of scope: show a message).
- Save: re-encode + restore line endings, keep BOM state, `ensure_newline_at_eof_on_save`,
  `trim_trailing_white_space_on_save` (incl. `"not_on_caret"` option and
  `trim_only_modified_white_space` using the buffer's change tracking), **atomic** write (temp
  file + rename, preserve permissions), `atomic_save` setting.
- File change watching (`notify`) → reload if not dirty, prompt if dirty (prompt UI can be a
  simple native alert for now).

## `velocity` integration

- Replace the demo editing with `vel_buffer::Buffer`/`ViewState`; input path: key event → keymap
  lookup (unless IME composition is active) → command; unbound printable input and IME commits →
  `insert`; `doCommandBySelector:` falls back to our keymap/default commands.
- Multiple carets and selections rendered (all carets blink-free like ST's default
  `caret_style`), selection rects per region, current-line highlight for every caret.
- Tabs: a simple tab bar (our own drawing; the `.sublime-theme` engine comes in P3), one window,
  `next_view`/`prev_view`, dirty indicator, close with save prompt (native alert).
- Command line: `velocity [files…]`, `velocity file:line:col` (ST's `subl` syntax), `-` reads stdin.
- Keep all P1 performance budgets from `specs/vel-platform-render.md` (startup, typing latency);
  add a `--bench-typing` variant that types through the full keymap→command→vel-buffer path with
  1 and 1,000 carets.

## Tests and budgets

- Unit/property tests for lenient JSON (round-trip edit preserves comments), settings layering,
  keymap parsing & resolution (incl. context operators and sequences), every text command on
  multi-cursor input, encoding/line-ending round trips (UTF-8/BOM, UTF-16LE/BE, CP1252, CRLF, CR,
  mixed), atomic save.
- Budgets: keymap lookup ≤ 5 µs per key event with 1,000 bindings; typing through the full path
  p50 ≤ 2 ms (1 caret) and ≤ 4 ms (1,000 carets) input→present; opening a 100 MB UTF-8 file:
  first frame ≤ 300 ms with progress, fully loaded ≤ 1 s.
- Same sandbox limits as before (no GUI/Metal in the sandbox): headless tests + RUNBOOK; the reviewer
  runs GUI measurements and returns the data.
- Don't modify `crates/vel-text`, `crates/vel-buffer` (stable — if you need an API change, describe
  it in your final message instead) or lane B's syntax crates.
