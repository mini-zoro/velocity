# Spec: settings coverage

Status: P4, round 17 (after the round-15 detail specs and `vel-editing-commands-details.md` in round 16). Two lanes, split below.

Background: a reviewer audit (2026-11-16) grepped the code for the documented Sublime Text 4
preferences. The behaviours mostly exist, but about thirty common settings are not read at all, so
users cannot turn them off or tune them. Each item below names the setting, the behaviour to
implement and where it lives. **Verify every default and value set against the current Sublime
documentation** (https://www.sublimetext.com/docs/ settings, indentation, multiple_selection_with_the_keyboard,
the ST4 changelog) before implementing; note the source in a code comment when the docs are
unclear. Clean-room: describe behaviour in our own words in `Preferences.sublime-settings`, never
copy Sublime's comments.

Every setting is read through `editor.settings` (view layers: default, platform, user, project,
syntax, distraction free, view), so a syntax-specific or project override works, and changes
apply live without reopening the file. Add each key with our own one-line comment to
`crates/velocity/resources/Packages/Default/Preferences.sublime-settings` (also `word_separators`,
which is read but not listed), and add a table to `docs/settings.md` (key, default, supported /
accepted-no-op with the reason).

## Lane A: editing, input and find (`editing.rs`, `editor.rs`, `vel-buffer`, `vel-commands`, `panels.rs`, keymap)

1. `indent_to_bracket` (false): when true, a newline inside an open `(`/`[` aligns with the column
   after the bracket. (Already done by the reviewer, do not redo: `auto_indent`, `smart_indent`,
   `tab_completion` (+ `exact` argument of `insert_best_completion`), `shift_tab_unindent` with
   Sublime's shift+tab bindings, `use_tab_stops`, `copy_with_empty_selection`, shift+enter.)
2. `find_selected_text` (true): opening Find/Replace/Incremental seeds the query from a non-empty
   single-line selection (or the word under the caret, per Sublime); false keeps the last query.
   `auto_find_in_selection` (false): opening Find with a multi-line selection turns "In selection"
   on. (If lane A round 15 already did these, only add tests.)
3. `close_find_after_find_all` / `close_find_after_replace_all`: close the panel after Find All /
   Replace All when true (check defaults in the docs).
4. `move_to_limit_on_up_down` (false): Up on the first line moves to the start of the buffer, Down
   on the last line to the end, when true.
5. `auto_close_tags` (true): typing `</` in markup scopes (`text.html`, `text.xml`) inserts the
   name of the innermost open tag and `>`.
6. `auto_complete_trailing_symbols` (false) / `auto_complete_trailing_spaces` (true): when
   committing a completion, keep or drop trailing punctuation / a space from the completion text as
   the docs describe.
7. `save_on_focus_lost` (false): when the window or a tab loses focus, save dirty file-backed tabs
   (not untitled, not read-only), like Save without a dialog.
8. `scroll_speed` (1.0): animation speed for keyboard-driven scrolling (page up/down,
   `show_at_center`); 0 disables the animation. Wheel/trackpad scrolling is unaffected.

## Lane B: display, tabs, side bar and window chrome (`view.rs`, `vel-render`, `vel-ui`, `app.rs` drawing, `workbench.rs`)

1. Caret: `block_caret` (false) draws a block caret over the next character;
   `caret_extra_top` / `caret_extra_bottom` (0) extend the caret height in pixels;
   `inverse_caret_state` (false) swaps the visible/hidden blink phases (so screenshots show the
   caret off when true). `caret_style` already exists — combine.
2. Gutter: `highlight_line_number` (verify the name and default in the docs) highlights the line
   numbers of caret lines using the color scheme's `gutter_foreground_highlight`.
3. Minimap: `draw_minimap_border` (false) draws a border around the minimap viewport;
   `minimap_scroll_to_clicked_text` (false) scrolls the clicked text to the click position instead
   of centring it.
4. Tabs: `highlight_modified_tabs` (false) colors the tab label of dirty tabs (theme rule
   `tab_label` with `dirty` attribute); `enable_tab_scrolling` (true) — when false, tabs shrink to
   fit instead of scrolling; `hide_new_tab_button` (false) and `hide_tab_scrolling_buttons` (false)
   if the theme draws them.
5. Window title: `show_full_path` (true) shows the full file path in the title bar, otherwise only
   the file name; `show_rel_path` (verify) for project-relative paths.
6. Side bar: `preview_on_click` (true; ST4 also accepts `"only_left"`) — a single click opens a
   transient preview tab that is replaced by the next preview and becomes permanent on edit or
   double click; false selects without opening.
7. Builds: `show_panel_on_build` (true): open the build output panel when a build starts; false
   leaves it closed (errors still go to the status bar / inline annotations).
8. Status bar: `show_line_column` (verify the name; ST4 uses values `"enabled"`, `"compact"`,
   `"disabled"`) controls the "Line N, Column M" item.
9. `tree_animation_enabled` (true): side bar folder expand/collapse animation; false is instant.
   Respect macOS Reduce Motion as today.

## Rules

- Worktrees: lane A `/Users/zoro/dev/velocity-lane-a`, lane B `/Users/zoro/dev/velocity-lane-b`,
  rebased on main first. Do not commit.
- A headless test per setting showing both values (and a syntax-specific override for at least
  one editing and one display setting). Native-only checks (caret drawing, minimap border, title
  bar, preview tabs) go to MANUAL_TEST.md.
- No regressions: current defaults keep today's behaviour unless the docs say otherwise.
- fmt, clippy `-D warnings`, `cargo test --workspace --locked --release`, rustdoc `-D warnings`.
