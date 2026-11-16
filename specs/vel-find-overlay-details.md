# Spec: find panels, overlays, status bar and editor detail behaviour

Status: P4 / lane A, round 15. Locations: `crates/velocity/src/panels.rs`, `crates/vel-ui/`, `crates/vel-search/`
(replace format and Find in Files "Where" only), the `velocity` editor/view files named below, and small hooks in
`crates/velocity/src/app.rs` (keep app.rs edits minimal: lane B edits app.rs this round too).

Background: a reviewer detail audit (2026-11-16) compared the find/replace panels, Goto Anything, the command
palette, the status bar and the side bar with Sublime Text 4 behaviour. Every item below was checked against the
code at the cited location (line numbers are approximate after the reviewer commit "Editing details"). Already fixed
by the reviewer, do not redo: `panel` / `panel_has_focus` key binding contexts (`Editor::ui_panel`,
`ui_panel_focused`, mirrored in `Panels::resolve_key`); `alt+enter` / `ctrl+alt+enter` now require a focused find /
replace panel; Replace Next is on `super+alt+e`.

Sublime references: https://www.sublimetext.com/docs/ (navigation, file_patterns, command_palette),
docs.sublimetext.io guide/usage/search-and-replace. Clean-room: describe behaviour, never copy Sublime resources.

## 1. Find and replace panels (`panels.rs`)

1. **Enter in the Replace panel must find, not replace.** `accept` → `Some(Panel::Replace)` calls
   `replace_document`. Sublime: Enter = Find Next, shift+Enter = Find Prev; replacing only through Replace /
   Replace All (buttons, `super+alt+e`, `ctrl+alt+enter`).
2. **Incremental find (super+i):** Enter selects the next match *and closes the panel*; Escape (and `hide_panel`
   with `{"cancel": true}`) restores the selection and scroll position saved when the panel opened. Today Enter
   keeps it open and Escape leaves the caret on the match; `hide_panel` ignores `cancel`.
3. **Typing in Find/Incremental selects the first match at or after the caret (wrapping) and reveals it.**
   Today the normal Find panel never moves the selection while typing, Incremental selects the first *visible*
   match (possibly above the caret), and when the match is off screen the selection changes but the view does not
   scroll (`pending_navigation` path, `app.rs` does not reveal after `panels.poll`). Add a reveal request from
   `poll` that the app honours. Respect `find_selected_text` / `auto_find_in_selection` if cheap.
4. **Re-opening super+f selects the existing query** (and the replace field on super+alt+f) so typing replaces it.
5. **super+f toggle only closes when the panel has focus.** If the panel is visible but focus is in the editor,
   super+f focuses the panel and selects the query (the reviewer's toggle is in `show_panel`; add `panel_focused`).
6. **Find history:** up/down in the Find and Replace inputs walk `find_history` / replace history. Record history
   only when a search is committed (find_next/prev, find_all, replace, Find in Files), not from `capture_session`
   checkpoints (today half-typed prefixes are recorded).
7. **Replace format:** support Boost-perl style `\1`..`\9`, `$&`, `\n`, `\t`, `\\`, `$+{name}` and the case
   escapes `\u \l \U \L \E` (`vel-search/src/text.rs` replacement parser). Keep `$1`, `${1}`, `${name}`, `$$`.
8. **Feedback:** `find_next` with no match shows `Unable to find <query>` in the status bar; `Replaced N
   occurrences` must not be overwritten by the next poll; remove debug text such as `"{n} visible matches · {ms}
   ms"`. Panel and overlay messages are drawn in the status bar with an expiry (Sublime keeps them a few seconds).
   Regex errors are shown inline / in the status bar without the internal `(?m)(?i)` wrapper — never a modal alert
   (`replace_document`, `start_files` → `show_error`).
9. **Buttons per panel:** Find: Find, Find Prev, Find All. Replace: Find, Replace, Replace All. Find in Files:
   Find, Replace — no current-buffer Find All button, and `alt+enter` does nothing there.

## 2. Find in Files

1. **F4 / shift+F4** (`next_result` / `prev_result`) walk Find Results when the last result source is a search,
   otherwise build diagnostics (today they only call `builds.navigate` and are disabled without diagnostics).
   Reuse `jump_result`.
2. **Where field:** `<open folders>`, `<open files>`, `<current file>` (and `<project>` if the project has one) are
   symbolic locations; patterns without a slash match file *or directory* names; a trailing `/` means a directory;
   a leading `//` is relative to the project folders; `-pattern` excludes and must prune directories, so
   `-node_modules` and `-node_modules/` both exclude everything under it (`vel-search/src/files.rs`).
3. **Progress, count and cancel:** show "Searching N files…" / "M matches in F files" in the results panel header
   and status bar; closing the panel or Escape cancels the search (drop/cancel `files_task` in `close_panel`).

## 3. Overlays (Goto Anything, command palette, symbol lists)

1. **super+r symbol list is empty most of the time:** `poll` calls `query_quick` every frame for symbol targets and
   `query_quick` cancels the worker and clears the items first, so results are discarded; `set_items` also resets
   the selection. Re-query only when the query text or the symbol source changes, and keep the selected item
   across refreshes when it still exists.
2. **Enter before fuzzy ranking finishes** opens the best current candidate (today nothing).
3. **Mouse:** a click outside an overlay cancels it; a single click on an item accepts it.
4. **Command palette filters commands that are disabled in the current context** (use `command_enabled`;
   e.g. "Close Split" with one group must not be listed). Accept must also check enabled.
5. **Footer text per overlay** (no stale find messages, "Cmd+Enter split" only where it applies).
6. `ui_scale` ≠ 1: centre `quick_bounds.x` correctly and scale `review_side_path` hit testing.

## 4. Status bar and side bar

1. **Selection text:** nothing for a single empty caret; `N characters selected` for one region; `N selection
   regions` (and the summed characters) for several; correct singular/plural. Encoding and line endings are shown
   only when `show_encoding` / `show_line_endings` are true (default false, like Sublime). The indentation item
   ("Spaces: 4" / "Tab Size: 4") is shown and clickable, opening a quick panel to switch tabs/spaces, the width, and
   convert indentation.
2. **Side bar:** Escape with side bar focus only returns focus to the view (does not close panels); `focus_side_bar`
   (ctrl+0) does not close the find panel. File operations in the side bar context menu are lane A round 14 work —
   check what landed and fill gaps (New File, New Folder, Rename, Delete to Trash, Find in Folder, Reveal in Finder,
   Copy Path).

## 5. Editor details

1. **Visual-line Home/End with word wrap:** `move_to bol/eol` (⌘←/⌘→, Home/End) go to the start/end of the
   *visual* row on a wrapped line, then to the logical line on a second press for bol (smart: indentation, then
   column zero, which `vel-buffer` now implements for logical lines); `hardbol`/`hardeol` always use the logical
   line. Intercept in the editor with the layout (`Editor::intercept_text_command`).
2. **Auto-pairing during re-highlight:** `selector` contexts evaluate to false while scopes are not ready
   (`vel-config/src/keymap.rs`), so quotes stop pairing for a moment after each edit. Answer selector queries from
   the last published scopes for the line (shifted through the edit) while the new ones are computed; only fall back
   to "not ready" when no scopes exist yet.
3. **Find under / Quick find all must keep working with multiple carets** (vel-buffer now expands every empty caret
   on ⌘D; check the panels' `find_under` / `slurp_find_string` paths use the primary selection consistently).

## Rules

- Worktree: /Users/zoro/dev/velocity-lane-a (branch `lane-a`), rebased on main first. Do not commit.
- Headless tests for every item (panel/overlay state machines, replace format table, Where parsing, status text);
  add the native checks to MANUAL_TEST.md. No regressions in existing tests, including the find-panel close/toggle
  tests and the keymap tests added in "Editing details".
- fmt, clippy `-D warnings`, `cargo test --workspace --locked --release`, rustdoc `-D warnings`.
