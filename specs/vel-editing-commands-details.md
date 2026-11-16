# Spec: editing commands, folding, bookmarks, macros, visuals, Vintage and spelling details

Status: P3, round 16 (before `vel-settings-coverage.md` and `vel-snippets-builds-layouts-details.md`, which move to
rounds 17-18: these are everyday Sublime features). Lane A: sections 1-4. Lane B: sections 5-9.

Background: a reviewer detail audit (2026-11-16) compared these areas with Sublime Text 4 and confirmed most items with
a throwaway probe program against `vel-buffer`. Line numbers are approximate. Items marked *(verify)* rest on the
auditor's memory of Sublime: check the current documentation or the changelog first and note the source in a code
comment; if it cannot be confirmed, keep today's behaviour and say so. Sources: https://www.sublimetext.com/docs/
(column_selection, vintage, spell_checking), https://docs.sublimetext.io/reference/commands.html,
https://docs.sublimetext.io/guide/extensibility/macros.html, the ST4 changelog (https://www.sublimetext.com/download).
Note: ST4 build 4107 moved Join Lines to super+shift+j and made super+j a chord prefix — keep both as they are.
Clean-room: implement the described behaviour with our own code, resources and wording; do not download, read or copy
Sublime's Default package.

## Lane A

### 1. Undo and multi-caret (`vel-buffer`, `crates/velocity/src/editing.rs`)

1. **Edits that change nothing make no undo step:** backspace at the start of the buffer, `upper_case` on upper-case
   text, unindent of unindented lines, etc. must not mark the view dirty or clear the redo stack. Today
   `editing.rs` `left_delete` (~304) commits an empty edit and `ops::delete` / `commit_owned` always call
   `Buffer::transact` (the `[H1]` comment at `buffer.rs` ~448 makes empty transactions undoable on purpose). Skip the
   transaction when the edit list is empty or the replaced text equals the new text; keep the `change_id` /
   `publish_direct_change` consumers (layout, annotations, language) consistent — they must not miss a real change.
2. **`select_lines` (ctrl+shift+up/down) extends a non-empty selection as a column block:** map both ends of each
   region to the next/previous line at the same visual columns (column_selection docs). Today `ops.rs` `select_lines`
   (~529) adds an empty caret at `r.b`, uses `MonospaceLayout` (ignores folds and word wrap, so carets land inside
   folds) and stores an `xpos` that disagrees with up/down. Use the view layout.

### 2. Brackets, selection expansion, comments (`vel-commands`, `editing.rs`, `language.rs`)

1. **Bracket and scope commands** (today `move_to` ~579 and `expand_selection` ~622 in `vel-commands/src/lib.rs`
   reject these arguments): `move_to {"to": "brackets"}` (ctrl+m) jumps to the matching bracket (from either side,
   or to the enclosing pair's bracket); `expand_selection {"to": "brackets"}` (ctrl+shift+m) selects the contents,
   then including the brackets; `{"to": "scope"}` (super+shift+space); `{"to": "tag"}` / smart expansion
   (super+shift+a) for markup; `{"to": "indentation"}` stays available without a default binding. Reuse the bracket
   pairs and tag matching in `language.rs`; brackets inside strings/comments match only within the same scope.
2. **Toggle comment with block-only languages (HTML, CSS):** super+/ on an empty selection comments the whole line
   (`<!-- line -->`, `/* line */`); with the caret inside an existing block comment super+/ and super+alt+/ remove
   that comment instead of nesting a new one (`editing.rs` `toggle_comment` ~580-606 inserts an empty pair at the
   caret today). Detect the `comment.block` scope at the caret. Update `feature_tests.rs` (~125) accordingly.
3. **`transpose` (ctrl+t):** with several selections, rotate their contents; with an empty caret swap the characters
   around it and move the caret forward one character (today `ops.rs` `transpose` ~1189 reverses each selection's
   text and moves the caret back).
4. **`wrap_lines` (super+alt+q):** keep the line's comment prefix (`//`, `#`, `--`, …) and indentation on every
   wrapped line; width from `wrap_width` when > 0, else the first ruler, else a default *(verify)*
   (`ops.rs` `wrap_lines` ~1383, `text_ops.rs` `wrap` ignores `wrap_width` and drops the prefix today, which
   uncomments code). Add the binding and the Edit › Wrap menu entry.
5. **`permute_lines` (unique, shuffle, reverse) without a selection work on the whole buffer**, like `sort_lines`
   (`ops.rs` ~1265).
6. **Join Lines details** (builds 4166/4213): keep the selection after joining; no space is inserted before a closing
   bracket (`foo(\n)` → `foo()`). *(verify the exact rules in the changelog/docs.)*
7. *(verify)* `indent` (super+]) leaves empty lines empty (`ops.rs` ~812 adds indentation today). Only change it if
   confirmed.

### 3. Bookmarks and marks (`vel-buffer` ops, `vel-session`, view gutter hook, keymap)

1. **Bookmarks are regions:** `toggle_bookmark` stores the selections; `next_bookmark` / `prev_bookmark` select the
   region again (today a caret at the line start, `ops.rs` ~1606-1675); `select_all_bookmarks` (rename from
   `select_bookmarks`, keep the old name as an alias); `clear_bookmarks` accepts `{"name": "mark"}`.
2. **Bindings, menu, gutter:** super+f2 toggle, f2 / shift+f2 next/prev, super+shift+f2 clear, alt+f2 select all;
   Goto › Bookmarks menu; a gutter icon per bookmarked line (expose the regions to the view; lane B draws the icon if
   it touches `view.rs` drawing — coordinate through a small accessor, no drawing code in lane A).
3. **Bookmarks track edits and persist in sessions/hot exit** (`vel-session/src/history.rs` `ViewState::capture`).
4. **Marks:** `set_mark` (super+k super+space), `select_to_mark` (super+k super+a), `delete_to_mark` (super+k
   super+w), `swap_with_mark` (super+k super+x), `yank` (super+k super+y, a kill ring fed by `delete_to_mark`),
   clear mark (super+k super+g). The mark is a named region set (`"mark"`).

### 4. Macros (`vel-commands` registry, `editor.rs`, `panels.rs`, keymap, Tools menu)

1. **One macro buffer per application,** not per editor (today `Registry` lives in `Editor`, `editor.rs` ~85, so
   switching tabs loses the macro). Recording captures text commands and, since build 4107, find commands
   (find_next/prev, find_under, etc. handled in `panels.rs` ~1268 before they reach the registry); window commands
   are not recorded. Built-in macro commands such as Add Line Before must record like any other command.
2. **Bindings and menu:** ctrl+q toggles recording, ctrl+shift+q plays back; Tools › Record Macro / Playback Macro /
   Save Macro… (writes a `.sublime-macro` file into `Packages/User`) / Macros submenu listing macro files from
   packages; `run_macro_file` runs any `.sublime-macro` resource, not only the built-in ones.
3. Playback with several carets runs each command once for all selections; one playback is one undo step
   *(verify)*. The `is_recording` context and `redo_or_repeat` keep working.

## Lane B

### 5. Folding (`editing.rs` `fold_command`, `view.rs` gutter, `vel-render` layout, `language.rs`)

1. **Selections entering a fold unfold it** (find, Goto Line, goto definition, undo, mouse into a fold marker);
   character-wise left/right step over a fold as one unit; typing never edits hidden text. Today only the fold
   commands unfold and `move by characters` walks into hidden bytes while the caret stays drawn at "…".
2. **Nested folds:** keep inner folds when an outer fold is added (gutter click ~`view.rs` 777 appends without
   handling them; `fold_command` drops them as non-disjoint) and project only the outermost fold
   (`layout.rs` `configure` ~135 assumes disjoint folds, so an inner fold's first line reappears). Unfolding the outer
   fold shows the inner one still folded. This is the layout hot path: measure scroll/typing with folds.
3. **`fold_by_level`:** unfold all, then fold at that level; level 0 = `unfold_all`. Bindings super+k super+1..9,
   super+k super+0 and super+k super+j (unfold all), super+k super+t (`fold_tag_attributes`).
4. **Indentation folds do not swallow trailing blank lines** (build 4143: fold regions no longer include the trailing
   newline; `language.rs` ~1205-1240 extends to the next non-blank line). Read `fold_style` (build 4151) *(verify the
   values)*.

### 6. Indent guides, white space and occurrence highlighting (`view.rs`, `vel-render`, Preferences)

1. **Indent guide columns:** draw at columns 0, tab, 2·tab… of the indentation (today `view.rs` ~1303 loops
   `tab_size..=columns`: no guide at column 0 and the last one touches the text), continue through blank lines using
   the surrounding indentation.
2. **`draw_active` highlights only the guide of the block containing the caret,** from the block's first to last
   line (today the whole column across the file). `solid` draws solid guides, otherwise stippled.
3. **Defaults** *(verify each)*: `draw_white_space` `["selection"]` (ST4 array form, also accept the string forms),
   `highlight_line` false, `indent_guide_options` `["draw_normal"]` (today `"none"`, true, and draw_active, in
   `Preferences.sublime-settings` lines ~9, 38, 70). Changing a default changes what users see: confirm first.
4. **`match_selection`** (default true *(verify)*): when the selection is a whole word, outline the other whole-word
   occurrences in the visible area (cheap: visible lines only, cancel on edit).

### 7. Vintage (`crates/vel-vintage`, `crates/velocity/src/vintage.rs`)

1. **Insert mode is ordinary editing:** Enter, Backspace, arrows, shift+arrows and typed text go through the normal
   keymap and commands (auto-indent, pair deletion, completion popup navigation and auto-complete), as Sublime's
   Vintage documentation describes (today `vel-vintage/src/lib.rs` ~463-535 handles them itself). Keep `.` repeat
   and counts working: record the inserted text from buffer changes rather than from intercepted keys.
2. **Selections made outside Vintage** (super+d, mouse drag, find) behave as a visual selection for operators (`c`,
   `d`, `y`, …).
3. **Enabling:** Vintage is on when "Vintage" is not in `ignored_packages` (Sublime ships it ignored); keep
   `vintage_enabled` as an alias. `vintage_ctrl_keys` default *(verify; the docs suggest true on macOS)*.
4. Missing keys from the Vintage docs: ctrl+e / ctrl+y (scroll one line), alt+w (sub-word motion).

### 8. Spelling (`crates/velocity/src/annotations.rs`, `review_ui.rs`, `vel-spell`)

1. **`spelling_selector`** default matches Sublime's narrower selector *(verify)*: today `"comment, string, text"`
   (~`annotations.rs` 62) also marks HTML tag names/attributes and embedded JS/CSS. Exclude source, keywords,
   `meta.tag`, link/URL scopes.
2. `next_misspelling` / `prev_misspelling` (ctrl+f6 / ctrl+shift+f6 *(verify)*) must not switch the view's
   `spell_check` on permanently (`review_ui.rs` ~51).
3. Dictionaries from `~/Library/Spelling` (build 4107) are listed with the packaged ones.

### 9. Menus and command palette (`crates/velocity/resources/Packages/Default/Main.sublime-menu`, palette resources)

Fill the Edit menu (Line, Comment, Text, Mark, Code Folding, Convert Case, Wrap, Permute Lines, Sort Lines), a
Selection menu (Split into Lines, Add Previous/Next Line, Single Selection, Select All, Expand Selection to
Line/Word/Paragraph/Scope/Brackets/Indentation/Tag), Find menu entries that exist as commands, Goto › Bookmarks, Tools ›
macros, and matching command palette entries (Convert Case, Permute, Wrap, Bookmarks, Fold Level…). Use our own
captions. Lane A's new commands are listed in sections 2-4; add their menu rows after lane A lands or reference the
command names already chosen in this spec.

## Rules

- Worktrees `/Users/zoro/dev/velocity-lane-a` / `/Users/zoro/dev/velocity-lane-b`, rebased on main first. Do not
  commit. Both lanes add key bindings: lane A only for sections 1-4, lane B for sections 5-9, each in its own block of
  `Velocity.sublime-keymap` to keep the merge simple.
- Headless tests per item (probe tables for transpose/permute/wrap/select_lines, bracket and scope expansion over
  several syntaxes, comment toggling in HTML/CSS/Rust, bookmark and mark sequences with edits and session
  round-trips, macro record/playback across tabs and with find, fold state machines with nested folds and caret
  movement, indent guide geometry, Vintage insert-mode key routing, spelling selector scopes). Native checks go to
  MANUAL_TEST.md.
- Performance: folding (section 5) and occurrence highlighting (6.4) touch the layout and scene hot paths; report
  paired headless scroll/typing numbers against main (≤5% regression), retired instructions if the machine is busy.
- No regressions; fmt, clippy `-D warnings`, `cargo test --workspace --locked --release`, rustdoc `-D warnings`.
