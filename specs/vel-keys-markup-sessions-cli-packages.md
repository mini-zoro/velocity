# Spec: macOS keys and editing details, markup tools, saving and sessions, command line, packages, Goto Anything, accessibility

Status: P2/P3, round 21 (after round 20 lands, including lane B's sections C-G of
`vel-encodings-sidebar-windows-details.md`). Lane A: sections A-C (keyboard, editing commands, markup and syntax
runtime). Lane B: sections D-I (saving and sessions, command line, packages, Goto Anything and the index,
accessibility, window commands and menus). Each item carries its own priority. Do P2 items first. Lane B is
larger, so its P3 items may slip to round 22; record which ones did.

**Lane split update (2026-11-19):** lane A finished sections A-C (merged as `3cb35a0`). To run in parallel with lane C's round 20,
sections D (saving and sessions), F (packages and resources) and H (accessibility) move to **lane A, round 22**;
sections E (command line), G (Goto Anything and the index) and I (window commands and menus) stay with **lane B,
round 21**, starting after lane C's round 20 merges. Each lane adds the key bindings, menu rows, palette entries,
settings rows and MANUAL_TEST entries for its own items, grouped in one block per file.

Background: a reviewer audit (2026-11-19, main at 63c4db2) looked at areas no earlier spec covers. Nothing was
built or run: every item comes from reading the code, public documentation and the changelogs. Line numbers are
approximate and from 63c4db2.

Markers:
- *code-confirmed*: the code clearly lacks the behaviour or does something else.
- *(verify)*: the expected Sublime behaviour comes from the auditor's memory or an ambiguous page. Check the docs or
  changelog first and cite the source in a code comment. If it cannot be confirmed, keep today's behaviour and say
  so.

Sources:
- https://www.sublimetext.com/docs/ pages: settings, syntax, command_line, safe_mode, indexing, packages.
- https://docs.sublimetext.io pages: reference/commands, reference/comments,
  reference/keyboard_shortcuts_osx (community list of default macOS shortcuts), reference/key_bindings.
- ST4 changelog (https://www.sublimetext.com/download, builds 4107-4213). Dev builds:
  https://www.sublimetext.com/dev. ST3 changelog: https://www.sublimetext.com/3.
- Apple's public documentation: NSAccessibility, NSApplicationDelegate, and the Cocoa Text System key bindings
  (StandardKeyBinding selectors).

Clean-room: implement the described behaviour with our own code, resources, data tables and wording. Do not
download, read or copy Sublime's Default package, themes, keymaps or menus. The bundled language packages in
`third_party/sublime-packages` may be *loaded* at runtime (F1). Do not port their Python plugins
(`html_completions.py`, `css_completions.py`, `diff.py`); C1 derives its data from the public HTML and CSS
specifications.

Already planned elsewhere, do not redo:
- `vel-encodings-sidebar-windows-details.md` sections C-G (in progress): side bar refresh, file patterns, the index
  honouring project filters (D2), global recent files, window cascading, an already-open file in another window
  (E7), system recent documents (F3).
- `vel-file-session-details.md`:
  - Non-existent paths and per-path errors.
  - Directories open in a new window.
  - `velocity -` writes back.
  - Deleted files, Save All, reopen closed files.
- `vel-release-packages.md` and `vel-wiring-a11y.md`: the package core, install/remove UI, crash-report prompt and
  the baseline AX text area.
- `vel-editing-commands-details.md`: transpose, wrap_lines, select_lines, bracket/scope/tag expansion, bookmarks,
  marks, macros and fold basics.
- `vel-render-scheme-syntax-details.md` E: syntax engine load-time isolation of broken `.sublime-syntax` files.
- `vel-snippets-builds-layouts-details.md`: completion triggers, build systems, layouts, jump history and
  `side_by_side`.
- `vel-find-overlay-details.md` §3: overlay fixes.
- `vel-settings-coverage.md`: `auto_close_tags`, `index_exclude_gitignore`, `index_files`.

## Lane A

### A. Keyboard: macOS text-system keys and missing default shortcuts (`vel-platform/src/input.rs` only, `editor.rs` `selector_command`, `Velocity.sublime-keymap`)

1. **Standard macOS text keys (Ctrl+A, Ctrl+E, Ctrl+L, Ctrl+O, Ctrl+Y…)** *(code-confirmed; selector names
   (verify), P2)*
   - Code: keys with no keymap binding go to `interpretKeyEvents` (`vel-platform/src/macos.rs` ~995-1000) and then
     `doCommandBySelector:` (~1130). `Command::from_selector` (`input.rs` ~247-290) knows only ~30 selectors.
     Everything else is dropped.
   - Problem: Ctrl+A / Ctrl+E (AppKit sends `moveToBeginningOfParagraph:` / `moveToEndOfParagraph:`) do nothing.
     So do Ctrl+L, Ctrl+O, Ctrl+Y, Ctrl+Backspace, and any selector a user binds in
     `~/Library/KeyBindings/DefaultKeyBinding.dict`.
   - Expected: the docs shortcut page lists Ctrl+A "Move to beginning of line", Ctrl+E "Move to end of line",
     Ctrl+L "Center current line" and Ctrl+V "Scroll down one page".
   - Fix: map the public `NSStandardKeyBindingResponding` selectors in `input.rs` / `selector_command`:
     - Line and paragraph moves → `move_to hardbol/hardeol` (and their AndModifySelection forms).
     - `centerSelectionInVisibleArea:` → `show_at_center`.
     - `insertNewlineIgnoringFieldEditor:` → insert "\n". AppKit follows it with `moveBackward:`, which gives
       Ctrl+O "open line".
     - `yank:`, `transpose:`, `deleteToEndOfParagraph:`, `deleteWordBackward/Forward:`,
       `deleteBackwardByDecomposingPreviousCharacter:`.
     - `pageUp/DownAndModifySelection:`.
     - `setMark:` / `selectToMark:` / `swapWithMark:` / `deleteToMark:` → the mark commands.
     - `uppercaseWord:` / `lowercaseWord:` / `capitalizeWord:`, `selectWord:` / `selectLine:` /
       `selectParagraph:`.
   - Test: a selector → (command, args) table, and a check that every mapped command is registered. Ctrl+A/E with
     the system key bindings goes to MANUAL_TEST.md (via lane B, see Rules).
2. **Default shortcuts that are missing** *(code-confirmed against the docs shortcut page; rows marked (verify) are
   from memory; P2)*
   - Code: `super+shift+[` / `super+shift+]` exist only as the second key of a `super+j` chord (`Velocity.sublime-keymap`
     ~1689-1704). `super+9` selects index 8 (~137). `ctrl+tab` runs `next_view` (~41). There are no `super+f12`,
     `super+shift+f12`, `ctrl+shift+p` or `ctrl+shift+w` bindings.
   - Add these bindings. Lane B provides the commands marked [B] (I1).

     | Keys | Command |
     |---|---|
     | ⇧⌘[ / ⇧⌘] | `prev_view` / `next_view` |
     | ⌘9 | `select_by_index {"index": -1}`, the last tab [B] |
     | Ctrl+Tab / Ctrl+Shift+Tab | `next_view_in_stack` / `prev_view_in_stack` [B] |
     | ⌘F12 / ⇧⌘F12 | `goto_definition` / `goto_reference` with `{"side_by_side": true}` |
     | Ctrl+Shift+P | `show_scope_name` (C3) |
     | Ctrl+Shift+W | wrap selection in tag (C2), context `selector` `text.html, text.xml` |
     | ⌥⌘. *(verify)* | `close_tag` |
     | ⌥⌘↑ *(verify)* | `switch_file` [B] |
     | ⌥⌘← / ⌥⌘→ *(verify)* | `prev_view` / `next_view` |
     | Ctrl+←/→, Ctrl+Shift+←/→ *(verify the exact chords)* | sub-word motion (B3) |

   - Also: quote auto-pairing treats non-ASCII letters as word characters. The `preceding_text` context
     `[\"a-zA-Z0-9_]$` (~1151, ~1268) pairs `'` after `é`. Use a Unicode word class.
   - Test: the keymap conflict tests, plus a resolution table for each new chord.

### B. Editing commands (`editing.rs`, `editor.rs`, `vel-buffer`, `vel-commands`, `vel-config/src/settings.rs` `EditorSettings` only)

1. **Dedent while typing a closing token** *(code-confirmed absent; the exact trigger is (verify), P2)*
   - Code: `decreaseIndentPattern` is applied only when Enter is pressed (`editing.rs` `newline_with_blank`
     ~707-820, ~771).
   - Problem: typing `}` with `auto_match_enabled` off, `</div>` (including the `</` auto-close at ~349-406),
     Python `else:`, Ruby `end` or shell `fi` on an auto-indented line keeps the deeper indentation. Users then
     dedent by hand.
   - Fix: after an `insert` on a line that is whitespace up to the caret, re-check the line against
     `decreaseIndentPattern` when `auto_indent` and `smart_indent` are on. If it now matches, set the line's
     indentation to the indentation of the matching open line (or one level less). Make it one undo step with the
     typed character, and never touch a line the user indented by hand on the same line.
   - Test: a table over C, HTML, Python, Ruby and Bash, plus a check that a manually indented line is unchanged.
     Paired `typing1` / `typing1000` numbers, since this is the typing path.
2. **Toggle comment details** *(alignment (verify); the rest code-confirmed; P2)*
   - Code: line comments are inserted at each line's own indentation (`editing.rs` `toggle_comment` ~1340-1360).
     `TM_COMMENT_DISABLE_INDENT*` and `TM_COMMENT_CASE_INSENSITIVE` are never read (~1205-1215), and there is no
     `variant` argument.
   - Expected:
     - Multi-line toggles put the markers in one column, at the minimum indentation of the non-blank selected
       lines *(verify)*.
     - `TM_COMMENT_DISABLE_INDENT` = yes puts the marker at column 0 (docs reference/comments). The bundled C++,
       Go, Rust, C#, D and Makefile metadata set it.
     - `TM_COMMENT_CASE_INSENSITIVE` uncomments `REM` / `rem` alike (changelog 4166).
     - `toggle_comment {"variant": n}` picks the n-th marker set (4142).
   - Test: an indented block in Rust gives aligned `// `. A metadata fixture with DISABLE_INDENT gives column 0.
     Batch `rem` is uncommented. `variant` selects `_2` markers.
3. **Sub-word motion** *(setting code-confirmed absent; boundary rules (verify), P3)*
   - Code: `sub_boundary` (`vel-buffer/src/ops.rs` ~113) hard-codes `_` and lowercase→uppercase. `sub_word_separators`
     (changelog 4142) is not read.
   - Fix: add the setting (default `"_"`; check the documented default) to `EditorSettings` / `buffer_settings`.
     Add boundaries at digit↔letter and acronym→word transitions (`HTTPServer` → `HTTP|Server`), as
     `identifier_case` already does (~1540). Bind the chords in A2.
   - Test: a motion table over `fooBarBaz`, `HTTPServer2Go`, `a-b` with `"-_"`, and Unicode letters.
4. **Line commands next to folds** *(code-confirmed for hidden edits; the expected rules (verify), P3)*
   - Code: `fold_navigation_command` (`editing.rs` ~1718-1790) reveals folds only for insert, paste and delete
     commands.
   - Problem: `swap_line_up` / `swap_line_down` (`ops.rs` ~1066-1126), `duplicate_line`, `join_lines`,
     `indent` / `unindent` and `toggle_comment` change hidden lines silently. Folds stay attached to moved text.
   - Expected: changelog 4107 says "Improved behavior of Swap Lines". From memory, a line moves past a whole fold,
     and a folded block moves as one unit and stays folded.
   - Fix: at minimum, unfold any fold whose hidden text an edit touches. Then implement the fold-as-unit rules once
     verified.
   - Test: fold rows 2-5, put the caret on row 6, swap up; the text and fold state match the table.
5. **One undo step per built-in macro run** *((verify), P3)*
   - Code: `run_macro_file` runs each command as its own transaction (`vel-commands/src/lib.rs` ~476-488). With
     "Delete Left Right" (Backspace inside `()`, ~599), a DeleteBack followed by a DeleteForward does not group
     (`vel-buffer/src/buffer.rs` `typing_run` ~981-1003), so undo needs two steps.
   - Fix: wrap built-in macro playback in one undo group, or group that pair explicitly. Check user-macro playback
     against the docs first.
   - Test: `(|)` + Backspace + undo gives `(|)` in one step.

### C. Markup and syntax runtime (`completion.rs`, `vel-complete`, `editing.rs`, `html_ui.rs`, `language.rs`, `vel-syntax`, `vel-highlight`)

1. **HTML and CSS completions** *(code-confirmed absent; the exact UX is (verify), P2)*
   - Code: in Sublime these come from the HTML/CSS packages' Python plugins (`third_party/sublime-packages/HTML/
     html_completions.py`, `CSS/css_completions.py`). We cannot run those, and `completion.rs` / `vel-complete` have
     no markup source.
   - Problem: typing in HTML or CSS offers only buffer words.
   - Sources: the changelog API notes "Plugins may now disable the default HTML and CSS completions". Changelog 3033:
     "Improved HTML completions when typing outside of tags". 3019: "CSS: Completions will not be offered when
     typing in selectors".
   - Fix: write a native completion source with our own data tables, generated by a script from the WHATWG HTML
     element and attribute indexes and the W3C CSS property index (or MDN browser-compat-data, CC0). Record the
     provenance.
     - HTML: tag names after `<` and at text level, where Tab expands `div` to `<div>$0</div>`; void elements expand
       without a closing tag. Attribute names inside a start tag, inserted as `name="$1"`. Enumerated attribute
       values.
     - CSS: property names inside declaration blocks, inserted as `prop: $0;`. Keyword values after `:`. Nothing in
       selectors.
     - Respect `auto_complete_selector` and the kinds/annotations of `vel-complete`, and stay within the existing
       ≤ 5 ms per keystroke budget.
     - Honour `disable_default_completions`, which the bundled `HTML.sublime-settings` / `CSS.sublime-settings`
       define. Those files apply once lane B's F1 lands; until then the default is false.
   - Test: completion tables per scope (`text.html` text, inside a tag, attribute value, `source.css` block, selector,
     embedded `<style>`), and Tab expansion of a void and a normal element.
2. **Tag commands** *(code-confirmed absent, P3)*
   - `close_tag` (docs reference/commands; ST3 3033 "Fixed Close Tag not respecting self closing tags"): insert the
     closing tag of the innermost open element at each caret. Reuse `closing_tag_values` (`editing.rs` ~912) and
     skip void and self-closed elements.
   - Wrap selection in tag (Ctrl+Shift+W, docs shortcut page "Insert XML long tag"): an `insert_snippet` binding
     whose contents we write ourselves. The opening tag name is field 1, mirrored into the closing tag through a
     transform that drops attributes. `vel-snippet` already supports transforms.
   - Test: nested HTML with void elements; wrapping several selections; the XML case.
3. **`show_scope_name`** *(code-confirmed absent, P3)*
   - Expected: docs reference/commands, and ST3 3114 "show_scope_name command shows the scope in a popup".
   - Fix: a popup through `html_ui.rs` `show_popup` showing the scope stack at the primary caret, one scope per
     line, with a `subl:` link that copies the full scope string. Use the published scopes (no extra parse); when
     scopes are not ready, show "Scopes not ready yet".
   - Test: the popup HTML for a Rust string inside a function; the copy link puts the expected text on the clipboard.
4. **Robust syntax and metadata loading** *(code-confirmed, P2)*
   - Code:
     - `Assets::with_user_packages` (`language.rs` ~395-418) fails as a whole on one non-UTF-8 grammar (`r.text()?`)
       or one bad `.tmPreferences` (`MetadataSet::load_roots` stops at the first plist error,
       `vel-syntax/src/metadata.rs` ~155-170).
     - The error reaches `Configuration::load`. At startup `main.rs` retries once and then exits.
   - Problem: a Finder-zipped package with `__MACOSX/._Comments.tmPreferences` (AppleDouble) prevents the app from
     starting.
   - Fix: load per file. Report a broken file through the existing resource warnings, skip it and keep the rest.
   - Also: `set_file_type` and the `syntax` setting accept `scope:source.python` (changelog 4113). Today
     `detect_syntax` (`vel-highlight/src/lib.rs` ~161-176) compares the scope without the prefix.
   - Lane B filters the junk files at install time (F5).
   - Test: User packages with an AppleDouble `.tmPreferences` and a Latin-1 `.sublime-syntax` plus a valid
     `tab_size` setting; assets load, the setting applies and the report names both files.
     `set_file_type {"syntax": "scope:source.rust"}` works.

## Lane B

### D. Saving and sessions (`vel-fileio` save path, `vel-session`, `session.rs`, `session_restore.rs`, `vel-crash`, `vel-platform/src/macos.rs` termination, `app.rs` quit/checkpoint hooks)

1. **An in-place save must never destroy the file on disk** *(code-confirmed, P2)*
   - Code: `save_with_identity` (`vel-fileio/src/lib.rs` ~937-955) calls `set_len(0)` before writing. This is the
     default path: `atomic_save` is false, and the atomic path falls back to it whenever a file has any user xattr,
     another owner, hard links or a read-only mode (~826-850).
   - Problem: ENOSPC, EDQUOT, EIO or a dropped network volume in the middle of the write leaves a truncated file. The
     watcher then offers to "reload" the truncated text.
   - Fix: first make a same-directory clone of the original (`fclonefileat`, O(1) on APFS, else a copy). Failing
     there means the original was never touched. Then overwrite from offset 0, truncate to the final length and
     sync. On success delete the clone; on failure write it back into the same inode. If even that fails, keep the
     clone and name it in the error.
   - If the directory is not writable, check the free space (`fstatvfs`) before truncating.
   - Test: a `cfg(test)` writer fault after N bytes. The original bytes, inode and xattrs are unchanged and no
     clone remains. Also the read-only-directory branch.
2. **Saves that succeeded are not reported as failures; atomic fallbacks** *(code-confirmed; F_FULLFSYNC on
   network volumes is (verify), P2)*
   - Code:
     - After the data is written, `File::open(parent)` + `sync_all` (~978-981) fails with `?` when the directory is
       write/execute-only (0733, 0711). The tab stays dirty, and the next save shows a false "changed on disk".
     - `sync_all` (F_FULLFSYNC) has no fallback when a volume returns ENOTSUP.
     - `atomic_save: true` with a non-writable directory fails instead of saving in place.
     - The atomic rename drops ACLs, group, BSD flags and creation date (only xattrs are checked).
   - Fix:
     - `fcntl_fullfsync`, falling back to `fsync`.
     - A directory sync only after a create or rename, and its failure is a warning.
     - EACCES/EPERM/EROFS on the temp file falls back to in place.
     - Clone the temp file from the original (inherits metadata), or add gid/flags/ACL to `rename_would_lose`.
     - Source: ST3 "atomic_save is adaptively disabled when it's not possible to preserve file permissions".
   - Test: a 0o333 directory saves Ok; a 0o555 directory with atomic on saves in place (same inode); an injected
     ENOTSUP falls back; the group is kept after saving with atomic on.
3. **Session writes cannot be starved** *(code-confirmed, P2)*
   - Code:
     - `session_due` is pushed 300 ms forward on every input (`app.rs` ~2744) and re-armed by any `changed`
       (~3464), including each build output poll.
     - The `vel-session` writer restarts its 400 ms debounce on every `Changed` / `Retire` (`lib.rs` ~408-427).
   - Problem: during a streaming build, or while typing fast or holding an arrow key, nothing is written, so a crash
     loses all of it.
   - Fix:
     - Add a `max_wait` (~2 s) to the writer.
     - Throttle rather than postpone in `app.rs`.
     - Re-arm only for session-relevant changes.
     - Skip publishing an identical snapshot.
   - Test: a writer with 200 ms debounce and 600 ms max wait, poked every 50 ms for 3 s, writes within ~700 ms and
     then keeps writing. An App test with fake build output.
4. **Keep a previous session generation; never delete the newest session** *(code-confirmed, P2)*
   - Code:
     - `vel-session/src/lib.rs` ~313-345: an unreadable or invalid session is renamed aside and the app starts
       blank.
     - `session_restore.rs` `backup()` ~235-307: on a runtime lazy-restore failure it writes the *old* restore-time
       bytes, then removes the current session file (~283).
     - Crashed `Sessions/.tmp*` files are never cleaned up.
   - Source: changelog 4126 "Avoid session data corruption if a crash happens while saving the session".
   - Fix:
     - Keep `Session.sublime-session.1` (hard-link the old file before the rename).
     - On load, try `.1`, then salvage documents with valid `text` from the JSON as "Recovered …" tabs.
     - Use `F_BARRIERFSYNC` before the rename.
     - `backup()` deletes the source only while a restore is in progress.
     - Clean up stale temp files at startup.
   - Test: truncated v2 loads v1; both corrupt salvage the text; a forced lazy-restore timeout leaves the session
     file in place.
5. **Logout, restart and shutdown; quitting without loading lazy tabs** *(code-confirmed; the loginwindow reaction
   is (verify), P2)*
   - Code:
     - `applicationShouldTerminate:` (`vel-platform/src/macos.rs` ~1343-1367) always returns `TerminateCancel`
       after `quit()`, so loginwindow sees a refusal and log out, restart and shutdown are aborted.
     - `prepare_quit` (`app.rs` ~378-381) calls `prepare_sheet` on every tab, which reads and decodes every
       never-activated restored tab (each with a 2 s budget). That stalls quitting on slow or disconnected volumes.
   - Source: changelog 4075, "Session is now saved when application is notified of system shutdown".
   - Fix: reply `TerminateLater` and `replyToApplicationShouldTerminate:` after teardown (or `TerminateNow` with
     teardown in `applicationWillTerminate:`). Flush on `NSWorkspaceWillPowerOffNotification`. Unmark only loaded
     tabs.
   - Test: a pure decision function for the reply. 20 lazy tabs with `restore_io::block_next()` quit in < 100 ms
     and stay lazy. Logout with a dirty tab goes to MANUAL_TEST.md.
6. **Crash reporter gaps** *(code-confirmed; system `.ips` behaviour (verify), P3)*
   - Code (`vel-crash/src/signals.rs`):
     - No `SA_ONSTACK` (~64), so a stack overflow cannot be reported.
     - `_exit(128+sig)` (~113) hides the crash from the system.
     - An empty `.signal` reservation file per launch (~43-47) is never removed, and reports are unbounded.
     - The panic hook (`lib.rs` ~171-182) also reports panics that `catch_unwind` recovers
       (`session_restore.rs`, `workbench.rs`), so the next launch prompts about non-crashes.
   - Fix: `SA_ONSTACK`; after writing, restore the default action and re-raise; delete our reservation on clean exit;
     keep ≤ 20 reports; a `vel_crash::recoverable(|| …)` scope that only logs.
   - Test: a child `raise(SIGSEGV)` is `WIFSIGNALED` with a non-empty report; a thread stack overflow writes a report;
     a clean exit leaves no empty file; a panic in `recoverable` writes no `.crash`.

### E. Command line and single instance (`cli.rs`, `instance.rs`, `open_requests.rs`, `main.rs`, `app.rs` request routing, `workbench.rs` setting reads, `packaging/bundle.py`)

1. **The GUI process started by the CLI must not belong to the terminal** *(code-confirmed, P2)*
   - Code: on a cold start `main.rs` ~196-220 spawns the GUI with the CLI's process group, controlling terminal,
     stderr, cwd and environment. There is no `setsid` / `process_group`.
   - Problem: with `EDITOR="velocity -w"`, Ctrl+C or closing the terminal tab kills the editor and its unsaved
     buffers. GUI logs print into the shell. The cwd pins the volume. `GIT_*` variables from `git commit` leak into
     the long-lived instance.
   - Fix: `process_group(0)` + `setsid`, `current_dir("/")`, stderr to `~/Library/Logs/Velocity/`, and drop
     `GIT_DIR`/`GIT_INDEX_FILE`/`GIT_WORK_TREE`.
   - Test: build the command through a `gui_command()` and check its cwd and envs. Spawn `/bin/sh` with the same
     builder and compare pgid/sid with the test process.
2. **Window routing** *(code-confirmed; `"finder_only"` and Sublime's focus-existing behaviour (verify), P2)*
   - Code:
     - The cold-start child gets no files (`main.rs` ~196-220), so `restored_window()` (`open_requests.rs` ~15-43)
       never sees them.
     - Forwarded requests then hit `request_needs_window` (~216-223), which never reuses the untouched startup
       window. `velocity .` or `velocity -n f` on a cold start gives an extra empty window, and a folder or
       project already open in a restored window opens a duplicate.
     - `open_files_in_new_window` is read as a bool (`app.rs` ~3161, `open_requests.rs` ~208-219,
       `workbench.rs` ~1590), so ST4's string values are silently false.
     - A `.sublime-project` opened from Finder/Dock opens as JSON text (`open_path` ~283-327).
   - Sources: changelog 4126 "subl -n will reuse an existing empty window if the application isn't running"; 4121
     "Fixed case where folders were being added to existing windows instead of opening in a new window"; 4121
     "open_files_in_new_window is now respected when reading from stdin"; docs.sublimetext.io settings
     (Finder/Dock only).
   - Fix: a pure `route(request, windows)`. Focus a window whose folders/project match exactly. Otherwise reuse an
     untouched startup window. Otherwise open a new one. Accept `"never" | "finder_only" | "always"`, with `true` →
     finder_only. A project file opens as a project.
   - Test: a routing table, plus two-App `Sessions` tests.
3. **Single-instance robustness** *(code-confirmed, P2)*
   - Code:
     - CLI requests are acked only after the main thread handles them (`instance.rs` ~98-100). While a modal panel
       is up, requests are not drained (`macos.rs` ~657/761). After the 30 s ack timeout (~225-247), `main.rs`
       ~239-242 treats any error as "not running" and starts a *second* full GUI on the same session.
     - The socket path uses `$TMPDIR` and `DefaultHasher` (~197-211): it is not stable across Rust versions,
       depends on the shell's TMPDIR (nix-shell, ssh), has no 104-byte `sun_path` check and no lock.
     - The CLI binds and drops before the child binds (`main.rs` ~197), and concurrent invocations can unlink each
       other's socket (~276-285).
   - Fix:
     - Ack on enqueue.
     - Split errors into `NotRunning` (may run locally) and `Submitted` (never run locally). EOF while waiting
       (`-w`) exits non-zero with a message.
     - Use `confstr(_CS_DARWIN_USER_TEMP_DIR)` or `/tmp/velocity-<uid>/` with mode 0700, a stable hash (FNV/SHA)
       of the canonical data dir and a length check.
     - The primary holds an `flock` for its lifetime.
     - Update `packaging/forwarding-fixture.rs`.
   - Test: a fake primary that never replies gives `Submitted`; a golden socket path that ignores TMPDIR; a long data
     dir gives ≤ 103 bytes; two threads start at once and only one becomes primary.
4. **stdin** *(code-confirmed; buffer details from the changelog, P3)*
   - Code: stdin is read only for an explicit `-` (`main.rs` ~169), with `read_to_string` (~171), which fails on
     Latin-1. The text goes in through `run("insert")` (`app.rs` ~616-628), so it is undoable and not
     syntax-detected.
   - Sources: changelog 4113 "subl can now be used to edit stdin, eg: echo test | subl | cat" and "Syntax and
     indentation detection is now done when editing stdin"; 4121 "Undo stack is now cleared when reading from
     stdin"; 4200 "Tab translation is now disabled when reading from stdin".
   - Fix:
     - Read stdin implicitly when there are no file arguments and fd 0 is a FIFO or regular file (not a tty, not
       `/dev/null`, not a LaunchServices launch).
     - Decode with `vel-fileio` and the fallback encoding, carry bytes in the protocol and write back in the same
       encoding.
     - Load stdin as the buffer's initial content.
   - Test: an implicit-stdin decision table; a Latin-1 round trip; undo after the load is a no-op; a
     `#!/usr/bin/env python3` first line detects Python.
5. **Command-line options and help** *(code-confirmed absent; `-s` and focus return (verify), P3)*
   - Code: `cli.rs` ~162-269 rejects `-b`, `-s`, `--command`, `-v/--version` and `--launch-or-new-window` as
     unknown options (exit 1). `HELP` (~334) starts with benchmark flags and has no Usage section.
     `velocity notes:2024` is split into file `notes`, line 2024, even when `notes:2024` exists (~309-331).
   - Sources:
     - command_line.html ("-b or --background: Don't activate the application").
     - Changelog 4126: "Mac: Fixed subl -b not working", "Mac: Fixed terminal not being focused after waiting on
       file".
     - 4200: "Commands passed via the command line are now delayed until files and plugins have loaded".
     - 4180: "Files containing colons can now be opened from the command line".
   - Fix:
     - `Request` gains `background`, `stay`, `command` and `return_pid` (serde defaults). With `background`, the
       primary does not focus. When a `-w` wait ends, the terminal app (`return_pid`) is reactivated unless `stay`
       is set.
     - `--command` runs after the request's files open.
     - A user-facing help text, with the benchmark flags moved to `--help-bench`.
     - A literal path that exists wins over `:line`.
   - Test: a parse table; old request JSON still deserializes; `--help` lists every documented option and no
     `--bench`; a `notes:2024` file opens literally.
6. **Safe mode and crash-loop protection on restore** *(code-confirmed absent, P3)*
   - Source: safe_mode.html and the ST4 release notes: "--safe-mode", holding Option at startup on macOS, a separate
     data directory erased at start, no session.
   - Code: `session_restore.rs` ~146-160 catches only panics, so a restored document that crashes natively crashes
     every launch. The only way out is deleting the session, which loses unsaved work.
   - Fix:
     - `--safe-mode` and Option-at-launch use `…/Velocity (Safe Mode)/` (a separate socket follows from the
       data-dir hash).
     - Write a `restore-in-progress` marker (pid + time) before restoring, and remove it a few seconds after the
       first frame.
     - If the next launch finds the marker with a dead pid and a new crash report, rename the session to
       `Session.skipped-<ts>` (never delete it) and ask whether to reopen the windows.
   - Test: parsing; a stale marker plus a crash report means the session is neither restored nor overwritten.

### F. Packages and resources (`vel-packages`, `vel-config/src/resources.rs` and `keymap.rs`, `workspace.rs` `Configuration::load` and syntax-setting lookup, `release.rs`, `settings_entry.rs`)

1. **Load the bundled language packages completely** *(code-confirmed, P2)*
   - Code:
     - From `third_party/sublime-packages` (copied whole to `Resources/Syntaxes` by `packaging/bundle.py` ~101)
       only `.sublime-syntax/.tmLanguage/.tmPreferences` are loaded (`resources.rs` `load_syntaxes` ~152-182,
       `workspace.rs` ~887).
     - `bundle_resources()` (`release.rs` ~51) reads only `Resources/Packages`.
     - Three build systems are embedded with `include_str!` (`workspace.rs` ~1015-1035).
   - Missing: 873 snippets, 61 `.sublime-completions`, 39 syntax settings files and 17 build systems. Visible
     effects:
     - With the common user setting `translate_tabs_to_spaces: true`, a new Makefile or Go file gets spaces,
       because `Makefile`/`Go.sublime-settings` (`false`) are ignored. `make` then fails.
     - JavaScript/TypeScript `word_separators` without `$` are ignored, so `$foo` is two words.
     - `C++.sublime-settings` `"extensions": ["h"]` is ignored, so `.h` opens as C.
   - Sources: settings.html lists `Packages/<syntax>/<syntax>.sublime-settings` in the settings order, and
     `docs/PLAN.md` ~372 plans to use the packages' snippets and build files.
   - Fix:
     - Load these packages as shipped packages: lowest tier, honouring `ignored_packages`, overridden per F4. Skip
       `*.py`. Their keymaps, menus and `.sublime-commands` come in only together with F3's unknown-command filter.
     - Honour the syntax-specific `extensions` key in detection. The bundled C++ settings comment says it
       "overrides file_extensions". Resolve it in `configure_language` before calling `detect_syntax`, so lane A's
       file is untouched.
     - Key syntax-specific settings by the syntax *file* base name. Today the lookup uses the display name
       (`workspace.rs` ~983 vs ~2717; `settings_entry.rs` ~225), so `DOT.sublime-settings` never applies to
       "Graphviz (DOT)". Keep reading display-name files as a fallback *(verify the naming in the docs)*.
     - Stamp `.sublime-build` / `.sublime-macro` origins for live reload (`workspace.rs` ~1264-1354).
   - Test:
     - A new Makefile buffer indents with a tab under a user `translate_tabs_to_spaces: true`.
     - `x.h` opens as C++.
     - A Python snippet trigger expands.
     - The DOT settings apply.
     - Ignoring "Python" removes its snippets and settings.
     - The bundle self-test lists the loaded counts.
2. **Symlinked `Packages/User` and symlinked packages** *(code-confirmed, P2)*
   - Code:
     - `resources.rs` ~121-134 uses `entry.file_type()` (does not follow links), so a symlinked `User` folder or
       package is skipped.
     - `vel-packages` `list()` (~150) skips it too, and `set_enabled` rejects it (~347-349, ~416-433).
     - Preferences writers do follow the link (`appearance.rs` ~328), so values written from the UI are never read
       back.
   - Problem: the Dropbox setup that Package Control's sync docs describe (`ln -s ~/Dropbox/…/User`) silently loses
     all user settings and keymaps.
   - Fix: follow links with a canonical visited set (cycles); `remove` deletes only the link.
   - Test: a `User` link to an external directory with `tab_size: 7` and a keymap; both apply; Disable writes through
     the link; `Packages/Loop -> ..` terminates.
3. **Key bindings from packages** *(code-confirmed; Sublime's handling of a missing command (verify), P2)*
   - Code:
     - `keymap.rs` ~404-415 resolves to the last matching binding without checking that its command exists, and
       `app.rs` ~873/~1004 swallows the key. A package (or a bundled language package after F1) that binds
       `super+d` to a Python-only command disables Find Under Expand.
     - One unsupported binding (an unknown modifier/operator, or a `regex` lookaround) drops the whole file
       (`keymap.rs` ~140-157, ~324-341; `workspace.rs` ~1146).
     - Every `*.sublime-keymap` loads, while docs reference/key_bindings says only `Default.sublime-keymap` /
       `Default (OSX).sublime-keymap` are read.
     - A keymap that failed to parse is not stamped, so fixing it needs a restart (~1142-1148).
   - Fix:
     - Skip bindings to unregistered commands in non-User packages. User `noop`-style unbinding keeps working.
     - Report per binding and compile context regexes with `vel-regex`.
     - Whitelist file names, exempting our own Default package.
     - Stamp every keymap origin.
     - Show one non-modal notice per package that contains Python plugins ("N plugin files not run; K bindings / M
       menu items need the plugin API"), deduplicated by name and version.
   - Test: a Python-only binding leaves `super+d` on `find_under_expand`; one bad binding of two keeps the other;
     `Example.sublime-keymap` is ignored; a fixed keymap applies without restart; the notice appears once.
4. **Shipped vs installed precedence and per-package platform order** *(code-confirmed, P3)*
   - Code: shipped and data resources merge per file (`resources.rs` ~32-81), so an `Installed Packages/
     Python.sublime-package` leaves renamed grammars of the shipped Python package behind. The sort key
     `(rank, contains("(OSX)"), path)` (~200-213) puts every package's `(OSX)` files after every other package's
     plain files.
   - Source: docs.sublimetext.io packages guide: an installed package replaces the shipped one of the same name; load
     order Default, shipped, installed, others, User.
   - Fix: record a tier per entry, drop a whole shipped package when an installed one shadows it, and sort by
     (tier, package, platform, path).
   - Test: shipped `Old.sublime-syntax` plus installed `New.sublime-syntax` leaves only New; the Beta package's plain
     keymap beats the Alpha package's OSX keymap.
5. **Package manager robustness** *(code-confirmed, P3)*
   - Code:
     - One unreadable package makes `list()` fail (`vel-packages/src/lib.rs` ~144-162). So do a loose package with
       more than 10k entries (`.git`, `node_modules`) and a zip with a symlink (`archive.rs` ~116-155).
     - The package UI and Preferences: Settings use the strict `resources.load(data)?` (`release.rs` ~547/551,
       `settings_entry.rs` ~50), so one broken archive blocks List, Remove, Disable and the settings window.
     - Enable/Disable lists only data-dir packages (`release.rs` ~540-616), so Vintage (ignored by default) cannot
       be enabled from the UI.
     - Install keeps `__MACOSX/`, `._*`, `.DS_Store`, and a GitHub zipball's single top folder (`Foo-main/…`), which
       breaks `Packages/Foo/…` references.
   - Fix:
     - Per-package results with an `error` field. Count loose entries per resource only, skipping dot folders.
     - `load_with_warnings` everywhere.
     - Enable/Disable lists shipped packages, Vintage and the current `ignored_packages`.
     - Normalize archives at install: drop the junk files, strip a single top folder, suggest a name without
       `-main`/`-master`, and warn on zero resources.
   - Test: a `not a zip` archive next to a good one lists 2 and is removable; Vintage can be enabled on a fresh
     profile; a zipball fixture installs with its files at the root.

### G. Goto Anything and the index (`vel-search/src/fuzzy.rs` and `goto.rs`, `vel-index`, `intelligence.rs`, the overlay code in `panels.rs`)

Rebase on round 20 first: its section D changes `intelligence.rs` index options and `vel-search/src/project.rs`.

1. **Goto Definition and hover use open files; the current file comes first** *(code-confirmed, P2)*
   - Code:
     - An index exists only with folders (`intelligence.rs` ~270).
     - Unsaved overlays go only to an index whose root contains the file (~386).
     - Queries search only project indexes (~107-165), and results sort by path (`vel-index/src/data.rs` ~402).
   - Problem: a single file opened from Finder, a file outside the project, or `index_files: false` shows "No
     indexed locations" for a function defined three lines up.
   - Sources: api_reference.html `SymbolSource.ANY` ("both the index and open files"); changelog 4186 "Fixed
     regression in Goto Definition where symbols in the current file weren't prioritized".
   - Fix: add an open-files source (from each tab's `analysis.index_symbols`, read-only), merge and deduplicate, and
     order the current file, then other open files, then the project.
   - Test: no folder, caret on a call, jumps to the definition; with `a.rs` and `b.rs` both defining `foo`, the
     current `b.rs` is first.
2. **Multi-term queries and ranking** *(code-confirmed; the expected order for some cases (verify), P2)*
   - Code: the whole lowercased query, spaces included, is one subsequence needle (`fuzzy.rs` ~126-145; same in
     `vel-index/src/names.rs` ~117-160), so "fuzzy search" or "vel index service" returns nothing.
   - Code: the alignment is greedy leftmost (~63-85), so "searchfuz" highlights scattered letters. A stem that
     equals the query gets only the +150 prefix bonus (~166-171), and the per-byte length penalty (~172) puts
     `src/maintenance.rs` above `crates/velocity/src/main.rs` for "main".
   - Sources: ST3 3170 "Fuzzy matching can match terms out of order by space separating them"; 4213 "Fine tuned fuzzy
     matching logic to improve sorting in some cases".
   - Fix: split on white space and match each term independently, in any order, summing the scores. Use the best
     alignment (DP per tier, bounded) and take the highlights from the same alignment. Add a bonus for an exact
     stem, and cap or log-scale the length penalty. Apply the same to Goto Symbol in Project and the palette.
   - Test: a golden ranking table: main, ac, searchfuz, lib (keep `lib.rs` first), "search fuzzy" / "fuzzy search",
     plus highlight positions.
3. **Goto Symbol list** *(code-confirmed; ordering (verify), P3)*
   - Code:
     - An empty query still subtracts length, so symbols sort by name length, not source order (`fuzzy.rs`
       ~147-172).
     - Rows show a literal "symbol/word" kind (`panels.rs` ~2403-2406).
     - Goto Symbol in Project rows have no match highlights and show absolute paths (~1226-1244).
   - Source: changelog 4107 "Icons are now shown next to symbols, indicating the symbol kind".
   - Fix: keep source order for an empty query. Show kind and line from `analysis.index_symbols`. Add highlights and
     project-relative paths.
   - Test: an empty query keeps source order; no row contains "symbol/word".
4. **Index idle cost and cache** *(code-confirmed; magnitudes inferred, P2)*
   - Code:
     - The coordinator loops on `park_timeout(1 ms)` and is never woken (`vel-index/src/service.rs` ~811).
     - With watching on, it stats every indexed file every 500 ms even when FSEvents works (~607-628).
     - It runs one 8-worker pool per root (`intelligence.rs` ~299) and rereads `.gitignore` files per event
       (~316-340).
     - The append-only log is never compacted (`store.rs` ~109-137), and replay holds the write lock (~378-382).
     - The cache key hashes the worker count and debounce (~144-155), and the cache lives in Application Support
       (`intelligence.rs` ~281-289).
   - Sources: indexing.html ("ensure the processes don't interfere with other usage of the machine"); ST4 release
     notes "The cache and index are now located in … ~/Library/Caches".
   - Fix:
     - Block on a channel with the next deadline.
     - Poll with stat only when the watcher failed.
     - One shared pool, and cached gitignore matchers.
     - Compact when dead bytes pass a threshold (write a temp file, then rename).
     - Hash only content-affecting options.
     - Replay into a local index, then swap.
     - Move the cache to `~/Library/Caches/Velocity`.
   - Test: 2,000 files idle for 1 s gives < 20 loop turns and zero `Stamp::read` (test counters); a log after 200
     reindexes is ≤ 2× the live size; changing `index_workers` keeps cache hits. Report idle wakeups/s before and
     after on a 100k-file tree.
5. **Goto Anything on large projects; `file@symbol` for unopened files** *(code-confirmed; streaming results
   (verify), P3)*
   - Code:
     - Opening the overlay deep-copies every candidate (three allocations each) when recency exists
       (`panels.rs` ~2364-2376, ~3452).
     - Symbol queries are requeried and the worker cancelled on every 8 ms poll (~3419-3425, ~2133-2142).
     - `a.rs@beta` for an unopened `a.rs` looks up symbols before its analysis exists and never retries
       (~2516-2549).
     - Recency is recorded only for Goto accepts (~2785).
   - Fix:
     - A shared candidate `Arc` plus a separate recency map.
     - Requery only on input or source changes.
     - Wait for analysis and key the preview on its version.
     - Record recency from every open and keep it per window in the session.
     - Partial results while walking, in coordination with `project.rs`.
   - Test: `Arc::ptr_eq` after opening; ≤ 1 query per 10 idle polls; `a.rs@beta` lands on the right line.

### H. Accessibility (`vel-platform/src/accessibility.rs`, `macos_a11y.rs`, `macos.rs` AX hooks, `app_a11y.rs`, `panels_a11y.rs`, focus drawing in `panels.rs`)

1. **Text model for large documents, and its cost** *(code-confirmed; VoiceOver output (verify), P2)*
   - Code:
     - The 64K-unit AX window recentres on the caret at every query (`accessibility.rs` ~79-84), so
       `selectedTextRange` stays near 32768 while moving.
     - The insertion line is a document line (~68; `app_a11y.rs` ~70), but `AXRangeForLine` uses window lines
       (~129-146). `MANUAL_TEST.md` ~3085 documents this mismatch.
     - Every getter and every frame rebuilds the whole tree, including the 64K string copy (`macos_a11y.rs`
       ~13-19, ~270-275; `macos.rs` ~704/737/793).
     - `ax_queried` never resets (~964), so any AX client (writing tools, launchers) taxes every frame.
   - Fix: a page-aligned window that moves only when the caret leaves its middle half, with ValueChanged and
     SelectedTextChanged when it moves. Window-relative line numbers. Cache the tree per input or frame revision.
     Reset `ax_queried` after N seconds without queries.
   - Test: 200k lines with the caret at line 100k; moving right keeps the window start and adds 1 to the location;
     `range_for_line(insertion_line)` contains it; 20 getters in one frame build one tree.
2. **Announcements** *(code-confirmed absent, P3)*
   - Code: no `NSAccessibilityAnnouncementRequestedNotification` anywhere. Focus stays in the "Filter choices"
     field (`panels_a11y.rs` ~144) while the selected row changes, and status-bar text is deliberately not hashed
     (`macos_a11y.rs` ~86-88).
   - Fix: announce the selected row ("caption, details, N of M") in quick panels and the completion popup, and
     status messages such as "No indexed locations". Don't repeat an unchanged selection.
   - Test: moving the selection from row 1 to row 2 produces one announcement containing row 2's caption.
3. **Keyboard focus and controls** *(code-confirmed, P3)*
   - Code:
     - `accessibility_focus` is never drawn (`app.rs` ~3108-3117, `app_a11y.rs` ~174-212).
     - `super+f6` walks every node, including two stops per tab and every side-bar row (`accessibility.rs`
       ~227-245).
     - Find toggles are plain buttons with no state (`panels_a11y.rs` ~99-114).
     - Not exposed: the output and Find Results panels, the status-bar indentation button, the Indexing Status
       window and non-active groups' editors.
     - Labels: the overlay field is always "Filter choices", dirty tabs end in " •", and rows include "symbol/word".
   - Fix: draw a visible focus ring and cycle by region (tabs, editor, side bar, panel, status bar; arrows inside a
     region). Toggles become `AXCheckBox` with a value. Expose those surfaces as read-only text areas, rows and
     popup buttons, and fix the labels (", edited").
   - Test: with 5 tabs and 50 rows, ≤ 6 presses return to the editor; regex toggled on is an AXCheckBox with value 1;
     the output panel text is in the tree.

### I. Window commands and menus (`app.rs` command dispatch, `workbench.rs`, `Main.sublime-menu`, `Velocity.sublime-commands`, `Preferences.sublime-settings`, `docs/settings.md`)

1. **Missing documented window commands** *(code-confirmed absent; P2 for `revert` and the MRU stack, P3 for the
   rest)*
   - `revert` (docs reference/commands; File › Revert File): reload from disk with the current encoding. Ask first
     when dirty (changelog 4142 "Reopening a file now asks for confirmation when there are unsaved changes").
   - `next_view_in_stack` / `prev_view_in_stack` ("Switches to the most recently active view"):
     - A per-group MRU stack.
     - Holding Ctrl while repeating Tab walks deeper, and releasing commits *(verify the gesture)*.
     - The order is stored in the session (ST3 3059 "Ctrl+Tab ordering is stored in the session").
     - Lane A rebinds Ctrl+Tab.
   - `switch_file {extensions}`: open the sibling with the next extension in the list, handling compound
     extensions (ST4 release notes).
   - `select_by_index {"index": -1}` selects the last tab (⌘9 in the docs shortcut list).
   - Aliases: `prompt_add_folder`, `prompt_select_project`, `open_recent_project`, `clear_recent_files`,
     `clear_recent_projects`, `hide_overlay`, `exit`.
   - Test: a dispatch table, an MRU sequence across a session round trip, and `revert` on a dirty tab with
     confirm/cancel.
2. **Menus and preferences for this round's commands** *(code-confirmed absent, P3)*
   - View › Syntax: every non-hidden syntax, grouped by package, with the current one checked.
   - "Open all with current extension as…" *(verify the caption)*: adds the extension to `extensions` in
     `Packages/User/<Syntax>.sublime-settings`, using F1's detection support for that key.
   - Also add:
     - File › Revert File.
     - Edit › Tag (Close Tag, Wrap Selection With Tag) for lane A's C2.
     - Tools › Developer › Show Scope Name for lane A's C3.
     - Goto › Switch File › Switch Header/Implementation (`switch_file`).
     - Palette rows for all of the above.
   - Add lane A's `sub_word_separators` with our own comment, plus `docs/settings.md` rows.
   - Test: the menu model contains each row, and every command resolves.

## Already correct (keep, with their tests)

- **Editing** (`vel-buffer`, `editing.rs`):
  - `swap_line_up/down` with the last empty line, keeping tabs and caret columns.
  - `duplicate_line` with selections and carets.
  - Multi-caret paste distribution, and whole-line paste above the caret.
  - `paste_and_indent` rebasing.
  - `find_under_expand` whole-word rules.
  - Acronym-aware `convert_ident_case`.
  - Tab-stop backspace.
  - The bracket/tag newline indentation via `increaseIndentPattern` + `decreaseIndentPattern` on Enter.
- **Syntax detection** (`vel-highlight` `detect_syntax`): whole file name, then compound extension, then
  `first_line_match` (BOM stripped). It includes `hidden_file_extensions`, keeps an explicit syntax, re-detects after
  Save As, and hides hidden syntaxes from the palette.
- **Packages** (`vel-packages`):
  - Archives reject traversal, absolute paths, symlinks and CRC errors.
  - Installs are atomic and backed up, and only use HTTPS.
  - No default network access.
  - Python is never executed and is labelled "partially supported: no plugin API".
  - `ignored_packages` is read only from Default/User and applies live.
  - Unknown commands are hidden in the palette and disabled in menus.
- **Command line** (`instance.rs`, launcher):
  - Relative paths are resolved by the client, and `--` works.
  - `-w` waits for all sheets, and stdin implies `-w`.
  - A stale socket is replaced only if it is owned by our euid; the socket is 0600.
  - Finder events are deferred during modals.
  - The launcher resolves symlinks.
- **Saving and sessions** (`vel-fileio`, `vel-session`):
  - Saving through a symlink writes the target.
  - Hard links survive in-place writes.
  - Case-only renames, `RENAME_EXCL`, and FIFO/device guards.
  - Encoding errors surface before truncation.
  - The BLAKE3 save-conflict check.
  - Sessions are written as temp + fsync + rename + directory fsync with mode 0600.
  - Quit is refused when hot exit cannot persist dirty tabs.
- **Crash reporter**: installed before session load, with an allocation-free signal handler.
- **Goto Anything and the index**:
  - The `file:line:col`, `@sym`, `#term` grammar.
  - Scoring runs on a worker with generation drop and incremental narrowing.
  - FSEvents-incremental file lists.
  - Index workers run at utility QoS.
- **Accessibility**:
  - The AX text area attributes and surrogate handling.
  - Tab, outline and list roles.
  - Reduce Motion and Increase Contrast apply live.

## Not in this round (recorded for later)

- `paste_from_history` (ST3 3033; needs a quick panel in `panels.rs`).
- Diff package commands (`diff_files`, Show Unsaved Changes).
- `new_snippet`.
- `encode_html_entities`.
- `syntax_detection_size_limit` (4113) and `set_unsaved_view_name_for_syntax` (4200).
- First-line syntax detection for untitled buffers *(verify)*.
- Asynchronous saving (4107 release notes).
- Per-document session blobs, large-unsaved-file prompts (4142) and `hot_exit_projects`.
- Import from an existing Sublime Text data directory.
- AX `AttributedStringForRange` / `RangeForPosition` / `StyleRangeForIndex`.
- Reduce Transparency.

## Rules

- **Worktrees**: `/Users/zoro/dev/velocity-lane-a` / `/Users/zoro/dev/velocity-lane-b`, rebased on main *after
  round 20 lands*. Do not commit. The lanes touch disjoint files:
  - **Lane A owns:**
    - `crates/velocity/src/{editing.rs, editor.rs, language.rs, completion.rs, html_ui.rs}`
    - `crates/{vel-buffer, vel-commands, vel-syntax, vel-highlight, vel-complete, vel-snippet}`
    - `crates/vel-platform/src/input.rs` (only this platform file)
    - `crates/vel-config/src/settings.rs` (only `EditorSettings`)
    - `Velocity.sublime-keymap`
  - **Lane B owns:**
    - `crates/velocity/src/{app.rs, main.rs, cli.rs, instance.rs, open_requests.rs, workspace.rs, workbench.rs,
      session.rs, session_restore.rs, restore_io.rs, shared_document.rs, release.rs, settings_entry.rs,
      intelligence.rs, panels.rs, panels_a11y.rs, app_a11y.rs}`
    - `crates/{vel-fileio, vel-session, vel-crash, vel-packages, vel-index}`
    - `crates/vel-search/src/{fuzzy.rs, goto.rs}` (`project.rs` only for G5, after round 20)
    - `crates/vel-config/src/{resources.rs, keymap.rs}`
    - The rest of `crates/vel-platform`
    - `crates/velocity/packaging/`
    - `Main.sublime-menu`, the context menus, `Velocity.sublime-commands`, `Preferences.sublime-settings`,
      `docs/settings.md` and `MANUAL_TEST.md`
- **Cross-lane rows**, with the names fixed by this spec:
  - Lane A binds lane B's `next_view_in_stack`, `prev_view_in_stack`, `select_by_index {"index": -1}` and
    `switch_file`.
  - Lane B adds menu, palette and preference rows for lane A's `show_scope_name`, `close_tag`, the wrap-in-tag
    snippet binding and `sub_word_separators`.
  - Lane A lists its native-only checks in its report, and the reviewer appends them to `MANUAL_TEST.md`.
- **Tests**: a headless test for every item.
  - Selector and keymap tables.
  - Indentation, comment and motion tables over several syntaxes.
  - Completion tables per scope.
  - Fault-injection seams for writes (D1/D2) and session loads (D4).
  - Temporary data directories, packages, symlinks and archives.
  - Two-`App` routing tests.
  - Golden ranking tables.
  - AX model tests.
  - Native-only checks (Ctrl+A/E through AppKit, logout/restart, terminal detach, focus return after `-w`,
    Option-at-launch safe mode, VoiceOver) go to `MANUAL_TEST.md`, clearly marked.
- **Performance**:
  - Lane A: B1 and C1 run on the typing path. Report paired headless `typing1` / `typing1000` against main (≤ 5%;
    retired instructions if the machine is busy) and the completion time per keystroke on a large HTML/CSS file.
  - Lane B, D1/D2: save latency for 1 MB and 100 MB files before and after.
  - Lane B, G2: Goto Anything p50/p95 over 100k candidates before and after (no regression beyond 10%).
  - Lane B, G4: idle wakeups/s and stat calls/s on a 100k-file tree.
  - Lane B, H1: frame time with an AX client attached.
- **No regressions**: fmt, clippy `-D warnings`, `cargo test --workspace --locked --release`, rustdoc
  `-D warnings`.
