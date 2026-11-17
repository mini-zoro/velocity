# Spec: encodings and line endings, side bar and file patterns, multiple windows, macOS shell

Status: P3, round 20 (after `vel-settings-coverage.md` in round 18 and sections 4-5 of
`vel-snippets-builds-layouts-details.md` in round 19). Lane A: sections A and B (encodings, line endings, file
reloads, text and status-column details). Lane B: sections C-G (side bar, file patterns and index, windows, macOS
shell and menus, drawing invisible characters). **Exception: section H (settings entry points, color scheme and
theme selection) is P2 and moves forward to round 19** — today a user cannot open the settings or switch the color
scheme from inside the app. H1 and H3 go to lane B (with layouts), H2 to lane A (with navigation).

Background: a reviewer audit (2026-11-17, main at 77c071c) covered areas that no earlier spec handles: side bar and
folder operations, multiple windows, encodings and line endings, macOS integration and Unicode editing. Each item
was found by reading the code; nothing was built or run. Line numbers are approximate and from 77c071c.
*code-confirmed* means the code clearly lacks the behaviour or does something else. *(verify)* means the expected
Sublime behaviour comes from the auditor's memory or from reading between changelog lines: check it in the docs or
the changelog first and cite the source in a code comment. If it cannot be confirmed, keep today's behaviour and say
so. Sources:
- https://www.sublimetext.com/docs/ pages: file_patterns, projects, settings, indexing, git_integration,
  command_line.
- https://docs.sublimetext.io pages: reference/settings, reference/commands, guide/usage/file-management/projects.
- The ST4 changelog on the download page (builds 4050-4215).

Clean-room: implement the described behaviour with our own code, resources and wording. Do not download, read or
copy Sublime's Default package, themes, keymaps or color schemes.

Already planned elsewhere, do not redo:
- `vel-file-session-details.md`: Reopen/Save with Encoding menus and display names, `follow_symlinks`, default
  exclude patterns, `file_include_patterns`.
- `vel-find-overlay-details.md` §4: the Where field and the status selection text.
- `vel-settings-coverage.md`: `binary_file_patterns` defaults, `show_line_column`, `show_full_path`, `preview_on_click`.
- `vel-snippets-builds-layouts-details.md`: layouts, groups, tab context menu.

## Lane A

### A. Encodings, line endings and reloads (`vel-fileio`, `vel-largefile/src/hex.rs`, `workspace.rs` load/reload, `workbench.rs` restore, `app.rs` command dispatch for these commands)

1. **Reloads keep the user's encoding** *(code-confirmed)*
   - Code: an external change reloads with auto-detection plus `fallback_encoding` (`workspace.rs` ~1860
     `load_background`, ~1872 `fileio::load`).
   - Problem: after File › Reopen with Encoding › Japanese (Shift JIS), the next external change turns the file into
     Windows-1252 mojibake.
   - Fix: when `tab.format` differs from what detection would give, reload with `load_with_encoding(tab.format.encoding)`.
   - Test: write Shift JIS bytes, `reopen_encoding`, rewrite the file, poll; the text and `format` still read Shift JIS.
2. **Session restore keeps the document encoding** *(code-confirmed)*
   - Code: restore re-detects (`workbench.rs` ~194/~203). `Tab::install` (`workspace.rs` ~215) then overwrites
     `format` and `saved_format`, which were just set from `document.encoding` (`workbench.rs` ~780).
   - Fix: decode clean documents with the stored encoding, and fall back to detection only when that fails.
     Changelog 4200: "Fixed file encoding not being restored for unsaved files".
   - Test: restore a session whose document has `encoding: "Shift_JIS"`; the text decodes correctly and the tab is
     not dirty.
3. **UTF-16 without a BOM is not binary** *(code-confirmed; Sublime's heuristic is (verify))*
   - Code: any NUL in the first 8 KiB means binary (`vel-largefile/src/hex.rs:21` `detect_content`,
     `vel-fileio` ~258 "binary file: NUL in first 8 KiB").
   - Expected: docs.sublimetext.io `fallback_encoding` says "ASCII, UTF-8 and UTF-16 encodings will be detected
     automatically".
   - Fix: when the prefix has an even length, NULs sit mostly at odd positions (LE) or even positions (BE), and it
     decodes without unpaired surrogates, open it as UTF-16 LE/BE with `bom: false`.
   - Test: an ASCII+CJK sample in UTF-16 LE and BE without a BOM opens as text and saves byte-identical; a real PNG
     is still binary.
4. **Saving characters the encoding cannot hold** *(code-confirmed; the fallback is (verify))*
   - Code: `write_encoded` (`vel-fileio` ~585) fails with "characters cannot be represented in windows-1252". A
     user who pastes an emoji into a Windows-1252 file cannot press Cmd+S.
   - Expected (from memory of Sublime): save as UTF-8, switch the tab's encoding, and say so in a dialog or status
     message. If that can't be confirmed, keep the error but offer "Save as UTF-8" in the alert.
   - Test: a Windows-1252 tab with "é😀" saves as valid UTF-8, and `format.encoding == "UTF-8"`.
5. **Sublime's command names** *(code-confirmed)*
   - Gap: `reopen {encoding}`, `save {encoding}` and `set_line_ending {type: windows|unix|cr}` (docs.sublimetext.io
     reference/commands) don't work, so user key bindings copied from Sublime silently fail.
   - Code: only `reopen_encoding` / `save_encoding` / `set_line_endings` exist (`app.rs` ~1168/~1174/~1299,
     `vel-commands/src/lib.rs:295`); `save` (`app.rs` ~1219) ignores `encoding`.
   - Fix: add the aliases and keep our names. Add `is_checked` for `set_line_ending` (current `format.line_ending`)
     for the lane B menu (E5).
   - Test: a dispatch table covering each alias.
6. **Status bar encoding and line-ending text** *(code-confirmed; captions and click menus are (verify))*
   - Code: the status bar prints the encoding_rs label (`panels.rs` ~3887 passes `tab.format.encoding`, e.g.
     "windows-1252", "UTF-16LE") and drops the BOM.
   - Fix: reverse-map it through the `ENCODINGS` display table ("Western (Windows 1252)", "UTF-8 with BOM",
     "UTF-16 LE with BOM"). Show line endings as Unix / Windows / Mac OS 9 rather than LF/CRLF/CR.
   - Clicking the encoding opens a menu with the Reopen with Encoding and Save with Encoding submenus. Clicking the
     line ending offers the three choices.
   - Test: a `vel_ui::status::position` table, plus the menu model produced for a click.
7. **A file with no line break gets `default_line_ending`** *(code-confirmed; (verify))*
   - Code: `DecodeReader::ending` (`vel-fileio` ~349) returns LF when it counts no line breaks. With
     `"default_line_ending": "windows"`, typing into a one-line file then saves LF.
   - Fix: use the setting when the counts are all zero.
   - Test: load "abc" with the setting set to windows; the format is CRLF.
8. **`trim_only_modified_white_space` defaults to true** *(code-confirmed)*
   - Code: `Preferences.sublime-settings:32`, `vel-config/src/settings.rs:254`.
   - Source: the changelog says "trim_trailing_white_space_on_save now trims only newly inserted trailing whitespace
     by default. Controlled via trim_only_modified_white_space".
   - Test: with trimming on, the default leaves untouched lines' trailing spaces alone.
9. **`reload_file_on_change`** *(code-confirmed)*
   - Code: clean tabs always reload silently (`workspace.rs` ~1883 `tab.install(loaded)`).
   - Source: the changelog says "Added reload_file_on_change setting to control if files are automatically reloaded
     or not".
   - Fix: when false, treat a clean tab like a dirty one: keep `tab.external` and ask before reloading.
   - Test: change a clean file on disk with the setting false; the text is unchanged and `external` is set.
10. **Reopen as Hexadecimal** *(verify, from memory: File › Reopen with Encoding › Hexadecimal)*
    - Gap: a text file can't be viewed as hex; only NUL-detected files reach the hex view (`binary.rs`).
    - Fix: add a `reopen {encoding: "Hexadecimal"}` entry that switches the tab to the existing `binary::Task`, and
      reopening with a text encoding returns to text.
    - Test: reopen a UTF-8 file as hex; `editor.binary` is set and the byte length matches.

### B. Text and Unicode details (`panels.rs` status text hook only, `vel-buffer`)

1. **The status column counts tabs as `tab_size` columns** *(code-confirmed; the expected rule is inferred)*
   - Code: `col = len_chars() + 1` (`panels.rs` ~3833).
   - Source: changelog 4152, "Fixed column number in the status bar not updating upon changing tab width", implies
     the column is tab-expanded.
   - Fix: expand tabs to the next tab stop; keep wide CJK characters at one column.
   - Test: "\tab" with the caret at the end, tab_size 4, gives Col 7; tab_size 8 gives Col 11.

## Lane B

### C. Side bar (`panels.rs` side bar, `context_menu.rs`, `app_context.rs`, `Side Bar.sublime-menu`)

1. **The side bar follows file-system changes** *(code-confirmed)*
   - Code: expanded directories refresh only after our own file operations (`refresh_sidebar` ~617 via
     `app_context.rs` ~337/~348) or when a git snapshot generation changes (`merge_git_tree` ~5040). The index
     snapshot (~2868) updates only Goto Anything candidates.
   - Problem: a file created from a terminal in a non-git folder, or a git-ignored build output, never appears.
   - Fix: use the project `Index` notify events (`vel-search/src/project.rs` `Index::start` ~489, already coalesced
     at 250 ms) to re-request only the affected expanded parents. Changelog: "Fixed a scenario where folders weren't
     being watched for changes".
   - Test: a temporary non-git folder; expand, create and delete files outside the app, and poll until `side_rows`
     matches.
2. **`refresh_folder_list`** *(code-confirmed absent)*
   - Expected: docs.sublimetext.io says it "Reloads all folders in the current project and updates the side bar".
   - Fix: add the command, a Project › Refresh Folders entry and a palette entry; it calls `refresh_sidebar` and
     restarts the index.
   - Test: dispatch it after an external change and the rows update.
3. **Alt+Right / Alt+Left expand and collapse recursively** *(code-confirmed)*
   - Source: changelog, "Folders in the sidebar can be recursively expanded via alt+arrow key".
   - Code: the side bar key handler (~2425-2448) ignores modifiers.
   - Fix: expand descendants lazily as their listings arrive, with no recursion into symlink cycles. Alt+Left
     collapses the whole subtree.
   - Test: a three-level tree; alt+right on the root expands all directories, and alt+left clears the set.
4. **Sort order** *(code-confirmed; the exact rule is (verify))*
   - Code: children sort by raw path bytes (`panels.rs` ~5035, `project.rs` ~201), so `README.md` and `Makefile`
     come before `a.txt`.
   - Fix: directories first, then case-insensitive by name, with byte order only to break ties. Check in the docs
     whether numbers sort naturally ("file2" before "file10").
   - Test: a sort table.
5. **Side bar and folder-list commands** *(code-confirmed absent)*
   - Add `close_folder_list` (Project › Remove All Folders from Project) and `toggle_show_open_files`, both in
     docs.sublimetext.io reference/commands.
   - Add a View › Side Bar submenu (Show/Hide Side Bar, Show/Hide Open Files). Persist the Open Files visibility
     per window in the session. Whether Open Files shows by default is (verify).
   - Test: the commands update `workspace.folders` / the visibility flag and survive a session round trip.
6. **Images open as images** *(code-confirmed)*
   - Source: changelog, "Added image_file_patterns for controlling which files are automatically opened as an image".
   - Code: a PNG has NULs, so it opens as a read-only hex dump (`hex.rs:21`, `binary.rs`). `vel-ui` already
     depends on `image` (png, jpeg, gif).
   - Fix: add an `image_file_patterns` setting (our own default list) and a read-only image sheet that fits the
     image to the view, with the size in the status bar.
   - Test: open a 2×3 PNG; the tab is an image sheet reporting 2×3, and a file not matching the patterns still
     opens as hex.
7. **Reveal through symlinked subfolders** *(code-confirmed path; the expected behaviour is (verify))*
   - Code: tabs are opened by canonical path (`open_requests.rs` ~265). `reveal_sidebar_path` (~627) canonicalizes
     too, and when the target is outside every root it adds the target's parent as a new top-level folder.
   - Fix: when a project folder reaches the file through a followed symlink, reveal the row under that link and
     don't add a folder. Changelog: "Resolved various issues tracking symlinks in the side-bar".
   - Test: project/link → /tmp/other; open link/x.rs, run reveal_in_side_bar; `workspace.folders` is unchanged and
     the row under link is selected.

### D. File patterns and the index (`vel-search/src/project.rs`, `vel-index/src/service.rs`, `intelligence.rs` ~243/~276, `workspace.rs` project build ~733-773 and reload ~1965-1995, `panels.rs` `add_folder` ~490)

1. **Match file patterns the way file_patterns.html describes** *(code-confirmed)*
   - Code: `globs()` (~27) uses globset with `literal_separator(false)`, and `Rules::allows` (~57) tests every
     pattern against both the relative path and the name.
   - Gaps against the page:
     - (a) A pattern with no `/` matches names only. Today `foo*` in `file_exclude_patterns` also matches the
       relative path `foo/bar/x.txt` and hides every file under a folder named `foo…`. Changelog 4126: "Fixed
       wildcards incorrectly matching subpaths".
     - (b) Only `*` and `?` are wildcards; `[`, `{`, `**` are literal.
     - (c) With a `/`, the pattern matches the full path as a path suffix: `mydir/one` matches `/parent/mydir/one`
       and `/mydir/one/sub`.
     - (d) A trailing `/` means a directory and everything inside it.
     - (e) A leading `/` is an absolute path.
     - (f) A leading `//` is relative to the project folder.
     - (g) `.git` is hard-coded in the walker (~137); leave it to `folder_exclude_patterns`.
   - Fix: write our own matcher, so no pattern can be malformed. Today `Rules::new(...)?` makes one bad pattern fail
     the whole project or settings load (`workspace.rs` ~767).
   - Test: a table built from every example on the page, plus a subpath regression.
2. **The index honours project filters** *(code-confirmed)*
   - Code: the `vel-index` walker (~640) ignores folder and file exclude/include patterns and uses
     `follow_links(false)`. `intelligence.rs` passes only the global `index_exclude_patterns`.
   - Gap: projects.html and file_patterns.html allow `index_exclude_patterns` / `index_include_patterns` per
     folder and in settings.
   - Fix: index exactly the project file set (Goto Definition must not jump into a folder the project excludes),
     then apply the index patterns. Once that holds, the default can be Sublime's `["*.log"]` (indexing.html);
     today `target`, `node_modules` and `.git` (`Preferences.sublime-settings:102`) stand in for the missing folder
     excludes.
   - Test: a project with `folder_exclude_patterns: ["vendor"]` and a per-folder `index_exclude_patterns`;
     symbols from both are absent and a symlinked folder is indexed.
3. **Add Folder with an open project updates the project** *(code-confirmed; the persistence detail is (verify))*
   - Code: `add_folder` (`panels.rs` ~490) only pushes to `workspace.folders`. `remove_sidebar_folder` does save
     `project_model` (`panels.rs` ~738).
   - Problem: reopening the project, or editing it on disk ("An actual project-file edit still replaces its
     configured folder list", `workspace.rs` ~1970), drops the added folder.
   - Fix: add it to `project_model.folders` (relative to the project file when it is inside it) and save.
   - Test: open a project, add a folder, reload the project; the folder is still there.

### E. Multiple windows (`app.rs` window code, `open_requests.rs`, `session.rs`, `vel-session`, `vel-platform`)

1. **`close_windows_when_empty` keeps windows that have folders** *(code-confirmed)*
   - Code: `app.rs` ~893-902 and `app_context.rs` ~281-290 close the window after its last tab even when the
     window has folders.
   - Expected: docs.sublimetext.io says "unless there's a folder open within the window". Changelog: "Fixed window
     unexpectedly closing when project has no added folders".
   - Fix: apply the same rule on every close path (close button, middle click, close_all).
   - Test: closing the last tab with a folder keeps the window; without a folder it requests a close.
2. **Recent files are global** *(code-confirmed)*
   - Code: each window copies the recent lists at creation (`app.rs` ~327) and publishes its own copy (~290-300).
     `Sessions::collect` (`session.rs` ~194) keeps whichever window published last.
   - Problem: files opened in window A drop out of Open Recent when window B checkpoints, and A's menu never shows
     B's files.
   - Fix: keep the recent files, folders and projects in the shared `Sessions` and rebuild every window's Open
     Recent menu when they change. Keep Reopen Closed File per window (`workspace.rs` `closed_files`).
   - Source: changelog 4143, "The recent file list is now global instead of per window"; 4152, "Reopen Closed File
     now uses the window's file history".
   - Test: two `App`s in one test; open in A, checkpoint B, and the collected session has A's file.
3. **Window cascading and frame restore** *(code-confirmed)*
   - Code: every window opens at `NSPoint(160,160)` (`macos.rs` ~1306) with a fixed size (`app.rs` ~353), so
     New Window lands exactly on top of the current one. `vel_session::Window` (~125) has no frame.
   - Source: changelog 4152, "Mac: Implemented window cascading".
   - Fix: cascade from the key window (`cascadeTopLeftFromPoint:`), and store and restore each window's frame,
     clamped to the visible screens.
   - Test: session serde round trip of the frame and the clamping math. Native placement goes to MANUAL_TEST.md.
4. **Window menu** *(code-confirmed absent; native tabbing is (verify))*
   - Gap: `Main.sublime-menu` has no Window menu, and `set_menu` (`macos.rs` ~252) never calls `setWindowsMenu:`.
     That leaves no Minimize (Cmd+M), Zoom, Bring All to Front or window list, and Cmd+` window cycling is
     unverified.
   - Fix: add a Window menu with these items.
   - (verify) The changelog mentions Mac "Window > Merge All Windows" and "Window/New Tab". Decide whether to stop
     disallowing native tabbing (`macos.rs` ~1326/~1528); if we keep it off, record why.
   - Test: the menu model contains the Window submenu with the native roles.
5. **View › Line Endings menu** *(code-confirmed absent)*
   - Add Windows / Unix / Mac OS 9 rows using A5's `set_line_ending` with check marks.
   - Test: the menu model's checked state follows `format.line_ending`.
6. **Moving tabs between windows** *(code-confirmed absent; (verify) the gestures)*
   - Gap: tabs can't be dragged out to a new window or dropped onto another window's tab bar. The changelog
     mentions "dragging a tab to a window". Each window owns its own `Workspace`.
   - Fix: add a move operation that carries the buffer, undo history, selections and dirty state through the shared
     `Sessions`.
   - Test: move a dirty tab from A to B in one test; B has the same text and undo, and A has no tab.
7. **An already open file in another window** *(verify)*
   - Code: `open_path` (`open_requests.rs` ~265) checks only the receiving window's tabs, so a Finder or `velocity`
     open routed to the active window (`app.rs` ~374-392) duplicates a file already open elsewhere.
   - Fix: confirm what Sublime does. If it focuses the existing view, search every window through `Sessions`.

### F. macOS shell and menus (`vel-platform/src/macos.rs`, `Main.sublime-menu`, `Velocity.sublime-commands`, keymap)

1. **Settings access** — moved to H1 (round 19).
2. **Standard app and Edit menu items** *(code-confirmed absent; (verify) for Sublime itself)*
   - Gap: the app menu has no Services (`setServicesMenu:`), Hide Others or Show All. `set_menu` removes every
     top-level menu after the app menu (~258), so check that AppKit still adds Emoji & Symbols and Start Dictation
     to our Edit menu.
   - Fix: make the editor view a services requestor (`validRequestorForSendType:returnType:`,
     `writeSelectionToPasteboard:types:`, `readSelectionFromPasteboard:`) so Services work on the selection.
   - Native checks go to MANUAL_TEST.md.
3. **System recent documents and windowless state** *(code-confirmed absent; the windowless menus are (verify))*
   - Gap: opened files never reach `NSDocumentController noteNewRecentDocumentURL:`, so the Dock menu and
     Finder's recent list don't show them.
   - Source: changelog 4143, "Files opened in Sublime Text are now added to the system recent file list (See the
     update_system_recent_files setting)"; also "Recent files are now cleared when update_system_recent_files is
     disabled".
   - Windowless state: the app stays running with no windows (`macos.rs` ~1216). Check that File › New Window,
     Open… and Open Recent still work then (changelog: "Mac: Recent files are now available without having a
     window open").
   - Test: a fake platform records the noted URLs.
4. **Title-bar document state** *(verify)*
   - Gap: no `setRepresentedURL:` (proxy icon, Cmd-click path menu) and no `setDocumentEdited:` (the dot in the
     close button for dirty tabs).
   - These are standard macOS behaviours; confirm Sublime shows them before adding.
   - Native checks go to MANUAL_TEST.md.

### G. Drawing invisible characters (`view.rs`, `vel-render`)

1. **Unicode white space, bidi controls and control characters as hex** *(code-confirmed absent)*
   - Source: changelog 4107, "Unicode white space characters, such as the zero width no-break space, are now drawn
     as hex values. Controlled via draw_unicode_white_space setting"; also "Added draw_unicode_bidi setting".
   - Code: nothing reads either setting, and C0/DEL characters draw as `.notdef` (`vel-render` font ~319 only
     warns).
   - Fix: draw a boxed `<0x200b>`-style placeholder that is selectable as one unit. Confirm the setting values in
     the docs.
   - Test: layout of "a\u{200B}b\u{202E}c\u{1}" gives placeholder cells, and caret motion steps over each one in a
     single move.

### H. Settings entry points, color scheme and theme selection (round 19)

1. **Settings access** (lane B; `vel-platform/src/macos.rs` app menu, `Main.sublime-menu`, palette, window/group
   code) *(code-confirmed absent)*
   - Gap: settings.html says "Preferences › Settings" opens the defaults on the left and the user file on the
     right. Velocity has no Preferences menu, no `edit_settings`, no Cmd+, and no way to open
     `Packages/User/Preferences.sublime-settings` or the user keymap from the app. The app menu holds only About,
     Hide and Quit (`macos.rs` ~1421). The palette has no settings entries either.
   - Fix: add `edit_settings {base_file, user_file?, default}` (docs.sublimetext.io reference/commands), opening a
     new window with two groups: our default resource read-only on the left, the User file on the right (created
     from the `default` argument if missing). `${platform}` in `base_file` expands to `OSX`.
   - Add app menu Settings… (Cmd+,) with a submenu: Settings, Key Bindings, Mouse Bindings, Settings – Syntax
     Specific (`Packages/User/<Syntax>.sublime-settings`), Settings – Distraction Free, Color Scheme…, Theme…,
     Customize Color Scheme, Customize Theme, Font (Larger / Smaller / Reset), Browse Packages (reveal the Packages
     directory in Finder). Add matching palette entries ("Preferences: Settings", …).
   - Saving the User file applies the settings live (the existing config reload); a JSON error is a non-modal
     status message naming the file and line.
   - Test: `edit_settings` against a temporary data directory gives the expected window, groups and tabs, the left
     tab read-only; the menu and palette resolve every command.
2. **Color scheme and theme selection** (lane A; quick panel in `panels.rs`, user preferences writer)
   *(code-confirmed absent; changelog: "Improved color scheme/theme selection UI")*
   - Gap: the only UI is "Theme: Light/Dark" (`set_theme_appearance`). There is no way to pick one of the bundled
     or installed color schemes or themes without editing JSON.
   - Fix: `select_color_scheme` / `select_theme` open a quick panel listing every non-hidden scheme (all suffixes
     the loader accepts) or theme, the current one preselected, with **live preview** while moving the selection;
     Enter writes `color_scheme` / `theme` to the User preferences, Escape restores the previous value without
     writing. When the current value is `auto`, offer to set the dark or light variant
     (`dark_color_scheme` / `light_color_scheme`, `dark_theme` / `light_theme`) as well as a fixed choice.
   - The preview must not rewrite the preferences file per keystroke and must not reparse unchanged schemes
     (cache compiled schemes by path).
   - Test: driving the panel through the real key path previews, commits and cancels correctly; the User file is
     written once on commit and never on cancel; `auto` variants.
3. **Customize commands** (lane B, after H1) *(changelog: "Added commands to simplify customizing the active theme or color
   scheme")*
   - `customize_color_scheme` / `customize_theme`: open (through H1's `edit_settings`) the active scheme or theme
     read-only on the left and `Packages/User/<same file name>` on the right, created from a minimal template
     written by us (`variables`, `globals`, `rules` for schemes; `variables`, `rules` for themes). User overrides
     already merge by file name (color_schemes.html).
   - Test: the command opens the expected files for a bundled scheme and for `auto`.

## Already correct (keep, with their tests)

- Grapheme-cluster caret motion and deletion (`vel-buffer/src/ops.rs` ~130-200).
- East Asian width in column selection and wrap (`editor.rs` `cells_with_tab_at` ~1295, `layout.rs` ~33-60).
- Non-ASCII case conversion through Unicode mappings.
- `sort_lines` with or without case on Unicode text, leaving out the empty last line when nothing is selected.
- `permute_lines`.
- BOM detection for UTF-8/UTF-16 and BOM preservation on save.
- Majority line-ending detection with normalization on save.
- `ensure_newline_at_eof_on_save`.
- Find in Folder applying the containing project folder's rules (`vel-search/src/files.rs` ~427).
- Git badges in Folders and Open Files with ignored entries dimmed.
- Side bar multi-select, create, rename, trash and Copy Path.
- `open_files_in_new_window` routing.
- Cycle detection when following symlinks.
- `binary_file_patterns` excluded from search but shown in the side bar.

## Rules

- Worktrees and ownership
  - Worktrees `/Users/zoro/dev/velocity-lane-a` / `/Users/zoro/dev/velocity-lane-b`, rebased on main first. Do
    not commit.
  - Lane A owns `vel-fileio`, `vel-largefile/src/hex.rs` detection, the reload paths in `workspace.rs`, the
    restore decode in `workbench.rs` (A2 only), `vel-commands`, the encoding and line-ending command dispatch in
    `app.rs`, the status text inputs (A6, B1) and its own keymap block.
  - Lane B owns the side bar in `panels.rs`, `context_menu.rs`, `app_context.rs`, `vel-search/src/project.rs`,
    `vel-index`, `intelligence.rs` index options, the window code in `app.rs`, `open_requests.rs`, `session.rs`,
    `vel-session`, `vel-platform`, `view.rs` / `vel-render` (G1), Preferences, menus, the palette and its own
    keymap block.
  - Lane B adds the menu rows for A5 (E5) and the status click menus for A6.
  - Section H (round 19): lane B owns H1 and H3 (`edit_settings`, the customize commands, the app/Preferences
    menu, palette rows); lane A owns H2 (the selection quick panels and the preferences writer) and keeps the
    command names `select_color_scheme` / `select_theme` for lane B's menu rows.
- Headless tests for every item:
  - Temporary directories for files, projects, sessions and symlinks.
  - Encoding round-trip tables.
  - The pattern table from file_patterns.html.
  - Two-`App` window tests through the shared `Sessions`.
  - Menu-model assertions.
  - Native-only checks (cascading, Window menu, Services, Dock recents, proxy icon, image rendering, hex
    placeholders) go to MANUAL_TEST.md, clearly marked.
- Performance
  - C1 adds work to the poll loop and D1/D2 change the project walkers: measure a cold walk and the side bar
    refresh on a 100k-file tree before and after.
  - G1 and B1 touch layout and the status bar: paired headless `features` / `typing1` / `typing1000` against
    main, at most 5% regression (retired instructions if the machine is busy).
- No regressions; fmt, clippy `-D warnings`, `cargo test --workspace --locked --release`, rustdoc `-D warnings`.
