# Spec: snippets, completions, build systems, layouts and navigation details

Status: P4. Round 18 together with `vel-settings-coverage.md` (lane A: sections 1-2, lane B: section 3); round 19
(lane B: section 4, lane A: section 5), together with section H of `vel-encodings-sidebar-windows-details.md`
(settings entry points and color scheme/theme selection). Round 17 is `vel-render-scheme-syntax-details.md`.

Background: a reviewer detail audit (2026-11-16) compared these areas with Sublime Text 4. Each item was confirmed
against the code (line numbers approximate) and, where marked, with a throwaway probe program. Sources:
https://www.sublimetext.com/docs/completions.html, https://www.sublimetext.com/docs/build_systems.html,
https://www.sublimetext.com/docs/themes.html, https://docs.sublimetext.io/guide/extensibility/snippets.html.
Clean-room: implement the described behaviour with our own code, resources and wording; do not download, read or
copy Sublime's Default package.

## 1. Snippets (lane A, round 18; `vel-snippet`, `crates/velocity/src/completion.rs`)

1. **Environment variables:** `TM_LINE_INDEX` is the zero-based *column* of the caret (today the line number,
   `vel-snippet/src/lib.rs` ~274); add `TM_FULLNAME` (the user's full name from the system) and `TM_SCOPE` (scope
   name at the caret). Check the rest of the documented list (TM_CURRENT_LINE, TM_CURRENT_WORD, TM_DIRECTORY,
   TM_FILENAME, TM_FILEPATH, TM_LINE_NUMBER, TM_SELECTED_TEXT, TM_SOFT_TABS, TM_TAB_SIZE) with one test each.
2. **Tab triggers ending in punctuation** (`t.`, `if?`, `eac-`, `:`): match the trigger as a suffix of the text
   before the caret instead of requiring a word (`completion.rs` ~853 returns false on an empty word). Prefer the
   longest matching trigger; with several snippets for the same trigger, offer a choice (quick panel) like Sublime.
3. **Snippet entries in the command palette** are filtered by the snippet scope at the caret.

## 2. Completions (lane A, round 18; `vel-complete`, `completion.rs`, Preferences)

1. **Tab without a visible popup:** with several candidates open the popup (mini list) instead of committing the
   first; commit directly only when exactly one candidate matches (`completion.rs` ~544, ~882).
2. **`auto_complete_triggers`:** support entries with only a `selector` and the `rhs_empty` flag; default triggers:
   `<` in HTML/XML (`text.html, text.xml`) and member access after `punctuation.accessor` with an empty right-hand
   side, so `.` / `::` open the popup in code (today `[]` and entries without `characters` are dropped,
   `completion.rs` ~513, `vel-complete/src/lib.rs` ~124).
3. **Defaults that change behaviour** (verify each against the docs):
   `auto_complete_include_snippets_when_typing` true; `auto_complete_selector` excludes only block/heredoc strings
   and comments, not all strings; `auto_complete_use_history` false; `auto_complete_preserve_order` is
   `"none" | "some" | "strict"` (string, today read as bool); trailing symbols from the index are appended only with
   `auto_complete_trailing_symbols` (default false), and `auto_complete_trailing_spaces` is supported.
4. **Popup rendering:** per-kind colored `kind_container` / `kind_label` (theme), matched characters drawn with the
   theme's match color, the kind name in the details footer; the namespace kind letter is `a` (today `n`,
   `vel-complete/src/lib.rs` ~608, ~724).

## 3. Build systems (lane B, round 18; `vel-build`, `crates/velocity/src/build.rs`, app build hooks, menus/keymap)

1. **Process environment:** run `shell_cmd` through the user's login shell (`/usr/bin/env bash -l -c` on macOS) so
   the user's PATH applies (today `/bin/bash -c`, `vel-build/src/process.rs` ~66). `path` / `env` values expand
   *environment* variables (`/opt/bin:$PATH` must keep PATH; today build variables expand it to `/opt/bin:`,
   `vel-build/src/lib.rs` ~146). Without `working_dir` the process runs in the current file's directory (today the
   app's working directory, ~142). `word_wrap` defaults to true in the output panel (~179).
2. **`${var/regex/format/options}`** in build variables uses the snippet transformation semantics: only the first
   match is replaced unless `g` is given (today always global, `variables.rs` ~152, ~166). Reuse `vel-snippet`'s
   transform.
3. **Selectors:** match `selector` with `vel_scope::Selector` scoring against the scope at the caret (including
   `|`, `-` exclusions and embedded scopes); today a comma split with prefix compare against the syntax root
   (`vel-build/src/lib.rs` ~72, `build.rs` ~36). With several matching build systems, `super+b` asks once (quick
   panel) and remembers the choice per window until changed; Build With (`super+shift+b`) lists only the matching
   systems and their variants and remembers the selection (`build.rs` ~46, ~194).
4. **Flow and output:** "Save All on Build" (Tools menu toggle, on by default) saves dirty file tabs before a build;
   starting a build while one runs cancels the running one and starts the new one (today refused, `build.rs` ~26);
   output: no `[name]/[dir]` header; the finish line is `[Finished in 300ms]`, or `... with exit code N` on failure,
   `[Cancelled]` when cancelled, and no finish line with `quiet`; the status bar reports the error count;
   `show_panel_on_build` (in `vel-settings-coverage.md`) controls opening the panel.
5. **Bindings and menus:** `f7` builds, `ctrl+c` cancels a running build (only when a build runs), Tools › Build
   System radio list with Automatic and "New Build System…" (opens a new untitled `.sublime-build` template written
   by us), Tools › Build Results › Show / Next / Previous. `interactive` builds (stdin) may stay unsupported: report
   it in the panel instead of hanging.

## 4. Layouts, groups and tabs (lane B, round 19; `workbench.rs`, `vel-ui/src/workbench.rs`, app tab hooks, menus)

1. **Bindings:** `super+alt+1..5` (single, 2/3/4 columns, grid), `super+alt+shift+2/3` (2/3 rows); `focus_group` /
   `move_to_group` on `ctrl+1..9` / `ctrl+shift+1..9`; `super+k` chords for `new_pane`, `new_pane {move: false}`,
   `close_pane`, `focus_neighboring_group`, `move_to_neighboring_group`; View › Groups submenu including Max Columns.
2. **`new_pane`** moves the selected tab into the new group by default (`move: false` clones nothing and leaves it)
   and honours a max-columns setting before switching to rows (today it only splits the cell,
   `workbench.rs` ~453, `vel-ui/src/workbench.rs` ~410). `close_pane` merges its tabs into the *previous* group
   (today the next one, ~416).
3. **Tab context menu** (today right click on a tab opens the review menu, `app.rs` ~2015): Close, Close Others,
   Close Tabs to the Right, Close Unmodified, Close Selected, Close Deleted, New File, Split View, Copy File Path,
   Reveal in Side Bar, Reveal in Finder; commands `close_by_index`, `close_others_by_index`,
   `close_to_right_by_index`, `close_unmodified`, `close_deleted_files`, each with the unsaved-changes prompt. Check
   what lane A round 14 landed in `specs/vel-open-context-menus.md` §2 first and fill the gaps.
4. **Split View / tab multiselect:** File › Split View clones the file next to it and shows both side by side via
   tab multiselect (super/shift click on tabs); clones share the buffer with separate selection and scroll (today the
   clone is appended at the end and selected alone, `workbench.rs` ~363). Add the `group_has_multiselect` context.

## 5. Navigation (lane A, round 19; `panels.rs`, editor/workspace jump history, menus/keymap)

1. **Jump history** records selection changes (consecutive changes from the same command merge) and view
   activations, not only Goto Definition / symbol accepts (`panels.rs` ~888, ~1705, ~2146). After Goto Line, a find,
   a click or a tab switch, `ctrl+-` returns; `ctrl+shift+-` goes forward.
2. **Hover definitions** (`show_definitions`): only in source code (not comments or plain strings; in HTML only
   class/id attribute values), excluding the definition under the pointer itself, with a References section listing
   other locations (`panels.rs` ~855, ~4464).
3. **Goto Definition / Reference:** support `side_by_side` and `clear_to_right` arguments (`panels.rs` ~918); add
   `super+alt+down` (goto definition), `super+alt+shift+down` (goto reference) and a Goto menu (Goto Anything, Goto
   Symbol, Goto Symbol in Project, Goto Definition, Goto Reference, Goto Line, Jump Back, Jump Forward,
   Switch File › Next/Previous).

## Rules

- Worktrees `/Users/zoro/dev/velocity-lane-a` / `/Users/zoro/dev/velocity-lane-b`, rebased on main first. Do not
  commit.
- Headless tests per item (snippet variable table, trigger matching, popup decision, trigger selectors, build
  environment/variables/selector scoring/output format with small shell scripts in a temp dir, layout state
  machines, tab close commands with dirty tabs, jump history sequences). Native checks go to MANUAL_TEST.md.
- No regressions; fmt, clippy `-D warnings`, `cargo test --workspace --locked --release`, rustdoc `-D warnings`.
