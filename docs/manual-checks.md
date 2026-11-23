# Manual checks on a real Mac (需要本人在 Mac 前、屏幕亮着)

These checks need a person at an awake, unlocked Mac with the built-in or an attached display
showing the desktop (not a remote-control session: in a remote session macOS skips every
drawable and the benchmarks time out). Use the installed app
(`/Users/zoro/dev/velocity-stable/target/package-macos/Velocity.app`, or the latest package).

## 1. GUI benchmarks (≈ 10 min, opens windows — don't touch the keyboard or mouse)
```sh
cd /Users/zoro/dev/velocity
scripts/bench-gui.sh /Users/zoro/dev/velocity-stable/target/package-macos/Velocity.app/Contents/MacOS/velocity
```
Report the summary table (startup first real scanout, skipped drawables, typing p50/p95 for 1 and
1000 carets, scroll frame p95, footprint). Compare with `docs/performance.md`.

## 2. VoiceOver (≈ 5 min)
Turn on VoiceOver (⌘F5). In Velocity:
- [ ] Editor: moving the caret reads the line/characters; selecting text announces the selection.
- [ ] Tabs: VO-arrows move across tabs and read their titles and the modified state.
- [ ] Command palette (⇧⌘P): typing reads the first result; arrows read each row.
- [ ] Find (⌘F): the Find field, option buttons (Regex, Case, Whole word, …) and the match count are read.
- [ ] Settings window (⌘,): the search field reads as a search field, the categories as a list whose
      selected row is announced; checkboxes, radio buttons and popups read their label and value
      ("Show line numbers, checked" / "Tab shape, Angled, selected"); "?" descriptions are read as help.
- [ ] Switch UI language to 简体中文 (Preferences › Language): the same places are read in Chinese.
Note anything silent, mislabeled or read in the wrong language.

## 3. Chinese input method (≈ 4 min)
Use the macOS Pinyin (Simplified) input method.
- [ ] Place three carets at the start of three lines and type `nihao`. All three show underlined
      `ni hao`, but only the primary caret has a candidate window. Repeat with the primary in the
      middle of the document, after scrolling, on a wrapped line, and in a split view.
- [ ] Space commits `你好` at all three carets and one ⌘Z removes only that commit. Repeat with
      non-empty selections: the preedit visually replaces each selection, as does the commit.
- [ ] Esc and switching to another window discard all the marks and restore the original selections.
      Clicking during preedit discards it and puts the caret where clicked. The document does not
      become modified or retain any preedit in autosave or a restored session.
- [ ] Type Pinyin in Find, Replace (including a multiline field), the command palette and a Settings
      text field. In both light and dark appearances the underline uses the field's text colour and
      is clearly visible at 1× and Retina scale. Confirm a candidate still commits in each field.

Round 40 matches the Sublime 4215 multi-caret preview in headless tests. The live Pinyin candidate
window, native discard callbacks and field contrast still need the native recheck above.

## 4. Non-US keyboard layout (≈ 2 min)
Add a non-US layout (e.g. German or French) in System Settings › Keyboard and switch to it.
- [ ] Menu shortcuts display the characters of the active layout.
- [ ] ⌘/ (toggle comment), ⌘[ / ⌘] (indent), ⌘= / ⌘− (zoom) work as on the US layout.

## 5. Visual spot check
- [ ] Light and dark appearance (System Settings › Appearance) switch live.
- [ ] A Chinese document (e.g. a Markdown file with paragraphs) reads without extra letter spacing; wrapped
      lines don't start with "。" or "，".
- [ ] Close a window, quit, relaunch: closed windows do not come back; windows open at quit do.

## 6. Round 35 windows and resources

- [ ] **App tabs vs. editor tabs:** With two Velocity windows and multiple editor
      tabs, check window switching and editor tab cycling separately, including
      with macOS window tab preference set to Always. Velocity currently disables
      AppKit tab groups. Native app-tab cycling, merge and restore require separate
      implementation and validation against the documented macOS behavior.
- [ ] **Scrollbar system policy:** In a long document with no scrollbar mousemap
      override, switch between “Jump to the next page” and “Jump to the spot
      that's clicked” in System Settings. Click the track with and without
      Option; drag the thumb. Repeat with overlay scrollbars, checking that the
      hidden track doesn't consume text clicks and that a live setting change
      affects the open window.
- [ ] **GPU initialization:** Check startup, resize and mixed-DPI display moves
      on Apple Silicon and Intel, where available. Record the startup diagnostics
      and error if the Metal adapter cannot initialize. Do not mark fallback
      verified: a software window backend and equivalent Console diagnostics
      are not implemented.
- [ ] **Font chooser:** Open Preferences › Font › Choose. Confirm the installed
      families and initial selection, then choose a different family and check
      the actual glyphs, caret, and wrapping. Cancel a second visit and verify
      the family stays unchanged. Test a syntax-specific `font_face` override.
- [ ] **Physical keypad and menu shortcuts:** With a numeric keypad, bind keypad
      digits, operators, decimal and Enter to distinguishable actions and check
      that the main keyboard keys remain independent. Repeat with a decimal-comma
      keyboard layout if available. With a Velocity User keymap override, compare
      the native menu label and invoked action before and after changing input source.
- [ ] **Theme transitions:** Install a Velocity fixture theme with tab hover and
      selection transitions and a tooltip opacity rule. Move quickly between tabs,
      bring a second window to the front and switch light/dark appearance mid-transition.
      Check virtual state, animation on the next frame, clipping and final colors
      with `animation_enabled` both on and off.

## 7. Round 36 parity

Use a disposable profile and fixtures. These are checks of Velocity’s native behavior,
not instructions to inspect an installed Sublime Text copy.

| Check | Procedure and expected observation |
| --- | --- |
| Find Results modifiers | With three selected tabs in one group, open a match from the Find Results sheet using Return, double-click and Command-click. Repeat with side-by-side on/off. Record selected tabs, focus, source cursor and which preview is replaced. Record Shift/Option/Control separately rather than assuming undocumented bindings. |
| Indexing Status | Start indexing an offline fixture tree, open Help › Indexing Status, and toggle the checkbox by click and Space. Confirm the old scan stops, a new scan starts after re-enabling, User `index_files` follows the control, and toggle focus remains in the auxiliary window. Repeat after closing the originating window. |
| `open_file` flags | Use an authored key binding or the CLI named command in a two-group fixture. Verify `force_group` reuses a target-group view, `force_clone` creates another view of the same edits, and `clear_to_right` deselects but never closes tabs. Verify `transient` is not recent until promoted and record its tab-bar/preview placement. |
| Image tab menu | Open a fixture PNG as Image, then as Text, and right-click each tab, including an inactive tab in another group. Check applicable Open As items, targeting, and that common close/reveal actions still work. |
| Quick Switch Project | With two different disposable projects in recents, choose the second with Return, Command+Return and Command-click. Plain Return switches in place; Command should use a separate window or activate the target's already-open one. Verify the originating project’s unsaved document stays put. |
| Native recents and icons | With the test app registered as the opener, double-click a document and a project in Finder and drop a file on the Dock icon. Check routing, icons, the macOS recent list, and that disabling `update_system_recent_files` clears/stops native additions without erasing project history. |
| Build camera and microphone | Run a local capture fixture as a build child. Observe TCC prompt attribution and status for deny, allow and retry. Do not record personal audio/video or change permissions outside this fixture. |
| Git gutter and inline colors | On a disposable repository, add, change and delete lines. Override the scheme’s diff colors and check the gutter, selected hunk and inline/deleted rows in light and dark mode. |
| Merge handoff | Point `sublime_merge_path` at the test integration executable. Use a repository with a spaced path, a modified file, a nested folder and several commits on one line. Test Open Repository, File History, Folder History (descendants included), Line History (one-based requested line) and Blame; record the actual view for each action. |
| Update dialog theming | Use a local update fixture with current, available and failed responses. Open Check for Updates under a legacy-style theme and check labels, version, changelog link, button states and legibility without installing anything. |

Record results in `crates/velocity/MANUAL_TEST.md` (section "Live checks") or tell the agent.

## Finder New Text File (round 45)

Run outside the sandbox. Package to a disposable output directory with
`scripts/package-macos.sh /absolute/path/to/artifacts`, then use that `Velocity.app`.
Packaging registers the app and extension without changing the Finder extension preference.
Use a test user or record and restore the existing preference.

1. With the extension off, confirm Finder does not show the item. Run **Finder: Enable New
   Text File in Context Menu** in Velocity and check its status bar. The alternative on
   macOS 15 and later is System Settings > General > Login Items & Extensions > Finder
   (under Extensions); on macOS 14, use Privacy & Security > Extensions > Finder Extensions.
2. Right-click a folder window's empty area, then one folder, one file, a mixed selection,
   and the Desktop background. Confirm one Velocity item and the correct destination each
   time: the container, the selected folder, or the first selected item's parent. Include a
   folder with spaces, non-ASCII characters, and `&` or `+` in its name.
3. Invoke the action repeatedly, with a pre-existing numbered file and a gap. Confirm each
   file is empty, the first available name is used, and nothing existing is overwritten.
   With two Velocity windows, the selected window gets a tab and comes to the front.
4. Repeat with all windows closed and with Velocity fully quit. The containing app should
   create a window and open the file once. Repeat in a read-only test folder; expect a
   status bar error, not a dialog or a tab. With all windows closed, the new window should
   show the same error.
5. Try Simplified Chinese and English as macOS system languages. The menu should read
   **新建文本文件** or **New Text File**. Set Velocity's UI language to the opposite language
   and confirm the name still follows Velocity: `新建文本文档.txt` or
   `New Text Document.txt`, then ` (2)`, ` (3)`, etc.
6. With a second copy running, invoke the registered copy's extension and confirm only
   its own app opens the tab. Try opening `velocity://new-text-file?dir=%2Ftmp%2Ftest-folder`
   directly from Terminal or a browser, using an existing disposable folder. No file
   should be created. Confirm Finder Open With and Dock file drops still open regular files.
7. Run **Finder: Disable New Text File in Context Menu**. Check its status and confirm the
   item disappears. Restore the original Finder preference.

## Automated live run (2026-11-22, by the agent, user away from keyboard)

Driven from outside the app through the macOS accessibility API and synthesized HID key events
(`/tmp/live/ax.swift`; not shipped). Packaged build of main `6da6b18` plus the fixes listed below,
fresh `--data-dir` profiles, system language 简体中文, Pinyin (Simplified) input method.

| Area | Result |
| --- | --- |
| VoiceOver tree: editor | Text area exposes the full value, selected text, UTF-16 selection range and insertion line; caret moves and Shift-selection update them; typing replaces the selection. |
| VoiceOver tree: tabs | Tabs are radio buttons with role description "tab", the file name as label and the selected state; a modified tab reads "b.txt，已编辑"; each tab has a "关闭 b.txt" button. |
| VoiceOver tree: command palette | Search field "搜索命令", results list "结果", rows with caption and shortcut; arrow keys move the selected row. **Fixed:** rows also read the internal command name (`save`, `toggle_setting`), which the compact palette does not show. |
| VoiceOver tree: Find | Find field, five option checkboxes with their state, Find / Find Previous / Find All buttons, and the match count status ("2 个匹配项"). |
| VoiceOver tree: Settings (Graphical) | Rows read name, key, value and modified state; toggles are switches with values; popups read their value; category buttons mark the selected one. |
| Localization | **Fixed:** the window content group read "Velocity workspace" in Chinese; it now reads "Velocity 工作区". **Fixed:** AppKit's own menu items (Services, Minimize All, Zoom All, Fill, Center, Move & Resize) and system role descriptions stayed English because the bundle declared no localizations; `CFBundleLocalizations` and `en.lproj` / `zh-Hans.lproj` now declare English and Simplified Chinese. |
| Startup report | **Fixed:** every fresh launch of the packaged app reported "Package Theme - Velocity: 1 Python files not run" because the theme generator script was shipped inside the package. It now lives in `crates/velocity/scripts/theme/`. |
| Chinese input method | Marked text is underlined at the caret with the candidate window directly below it; Space commits "你好"; ⌘Z removes the committed word in one step; Esc cancels the marked text; three carets each receive the committed text; the Find field and the command palette accept Pinyin. Minor: with several carets the marked text is drawn only at the primary caret (the committed result is correct); in the Find field the marked-text underline is a hairline that is hard to see. |
| Non-US layout (German) | AppKit relocates menu equivalents for the layout (Toggle Line Comment shows ⌘ß). Neither ⌘ß nor ⌘⇧7 ("/" on German) toggled the comment, and ⌘⇧7 opened the Help search. Pending a black-box comparison with Sublime on the same layout before changing key matching. |
| Chinese document | Markdown wraps; no extra spacing between Han characters; continuation lines do not start with "。" or "，". |
| Session windows | Windows open at quit return with their tabs, including an unsaved buffer marked as edited. A normal quit removes the crash-report placeholder (leftover placeholders came only from killed processes). |
| Window focus and menus | **Fixed:** after switching windows through the Window menu (or ⌘\`, Mission Control), menu commands such as Close Window still went to the previously key window, because the menu is reinstalled only on that window's next frame and a window becoming key requested none. Becoming or resigning key now requests a frame. |
| Startup without a visible frame | **Fixed:** an interactive window waited indefinitely for a real scanout before polling forwarded files and loads. In a remote-control session every drawable is skipped, so files passed on the command line never opened. Interactive startup now continues after 1 s without a real scanout; benchmarks still wait. A window that macOS keeps behind other windows (activation refused) still starts when it first becomes visible. |

Second run (2026-11-22, 01:45–02:10, user away; every keystroke script aborts if the pointer
moves or another app becomes frontmost):

| Area | Result |
| --- | --- |
| Window focus and menus | Verified natively: after Window › a.rs, File › New File opened in the a.rs window; after Window › b.txt, Window › Close Window closed the b.txt window. |
| Launch while locked | Verified natively: launched while the display was asleep and the screen locked, the window presented after unlock with both files open. |
| GUI benchmarks | `scripts/bench-gui.sh` passed every workload 5/5; see "Native verification after round 39" in `performance.md`. Two Velocity budgets were missed (1,000-caret typing p95 10.4 ms, 5,000-line first-frame footprint 107.5 MB). |
| Black-box comparison with Sublime 4215 | Same keystrokes, saved files compared (`scripts/blackbox-behavior-probe.sh`). Join Lines, embedded-language comments, verbatim block markers and the German-layout menu shortcut now follow Sublime (unit tests reproduce the Sublime outputs). Regex `^` replace already matched; `(?=b)` lookahead is not supported by Velocity's regex engine (Sublime: `a-bc`). With several carets Sublime shows the composing Pinyin at every caret; Velocity only at the primary one. |
| Evidence for U01 / U04 | Sublime opened with three files shows no sidebar; opened with a folder it shows an empty editor without a tab, and Goto Anything previews a file without adding a tab. Velocity shows the sidebar and an untitled tab, and previews in an italic tab (the user chose "keep" for both before this evidence). |

Third run (2026-11-22, 12:35–12:55, at the user's request "跑复核"):

| Area | Result |
| --- | --- |
| Multi-caret Pinyin (round 40) | Verified natively: three carets each show underlined `ni hao` while composing; Space commits `你好` at all three; one ⌘Z removes it; Esc discards. |
| Find-field underline (round 40) | Verified natively: the marked-text underline under `ni hao` in the Find field is clearly visible in the light theme. |
| German layout ⌘ß (0fe1911) | **Failed natively, then fixed:** AppKit rewrites the menu item's own key equivalent to the relocated one (`ß`), so comparing it with the pressed key treated the chord as declined. The check now compares against the equivalent Velocity installed (`/`). Verified natively with `f4d5f88`: ⌘ß toggles `# x = 1` on and off on German; ⌘/ still toggles on US. |
| U01 / U04 side by side with Sublime 4215 | Matches: files only (tabs, no sidebar), folder only (FOLDERS, no tab, `… — folder` title), Goto preview without a tab, Escape back to the tabless window, Return opens one tab. Differences sent to round 43: empty-window title (`Velocity` vs `untitled`), no folder-tree highlight of the active/previewed file, "Show Open Files" on by default (Sublime: off), static "Show Side Bar ✓" caption (Sublime: dynamic "Hide Side Bar"), absolute instead of folder-relative Goto detail paths. |

Still manual: VoiceOver speech itself (the tree was checked, not the voice), light/dark switch
(covered by render scenes), native recheck of the German-layout fix, and the Round 35/36 tables.

### Finder New Text File — native verification (2026-11-22, `075979f`)

Packaged into velocity-stable, registered, enabled with `pluginkit -e use -i dev.velocity.Velocity.FinderNewFile`.

| Check | Result |
| --- | --- |
| Menu | One item "新建文本文件" on a folder background and on a file. |
| First build `7d1d60c` | **Failed natively:** the item did nothing. Finder copies the extension's menu into its own process, so `representedObject` never reaches the action. Fixed in `075979f`: the target folder is resolved when the item is chosen. |
| Cold start | With Velocity quit, the item created `新建文本文档.txt` (0 bytes) and launched Velocity showing it. |
| Numbering and D4 | With Velocity running, the next file was `新建文本文档 (2).txt`, opened as a new tab in the current window; right-clicking a file created `新建文本文档 (3).txt` in the same folder. |
| Sender check | `open "velocity://new-text-file?dir=…"` from Terminal created nothing. |
| After re-signing | A repackaged extension (new ad hoc signature) was not relaunched by Finder until `pluginkit -e ignore` then `-e use`; after a repackage, toggle the extension once. |

Not yet checked: Desktop background, read-only folder message, English system/UI language,
mixed selections.

### Inline tab rename (round 46, native review pending)

Use a disposable local folder with `新建文本文档.txt`, `.zshrc`, and an already occupied name. Run with English and Simplified Chinese UI, light and dark.

| Check | Native steps and expected result |
| --- | --- |
| Tab meanings | Double-click the preview tab: it becomes permanent and does not edit. After that click sequence ends, double-click the permanent tab: the field replaces its label, the stem is selected, and the close button and dirty dot are gone. A dotfile selects the whole name. Double-click empty tab space: a new file opens. Untitled/settings/Find Results/image tabs do not edit. |
| Context menu and editing | Secondary-click a file tab and choose Rename… / 重命名…. Try a long name, Shift+Arrow, ⌘A, ⌘C, ⌘V, and ⌘Z. Text scrolls within the tab; ⌘W must not close it and dragging in the field should select text, not move the tab. Escape cancels and returns to the editor. |
| Pinyin and VoiceOver | With Simplified Chinese Pinyin enabled, type `ni hao` over the selected stem. Check that the thin underline is legible and the candidate window is below the tab, including on Retina. Choose 你好 with Space, then press Return to rename to `你好.txt`. VoiceOver should focus the Rename / 重命名 text field with its current value. |
| Blur and propagation | Start with dirty text and an open clone plus another window showing the file. Rename via Return, click another tab, and switch to another app or Velocity window during separate attempts. Check that the path follows in every window and in Open Recent, dirty text is still unsaved, and editor Undo still works. Check the restored path after a normal quit with Hot Exit. |
| Errors and case changes | Submit the occupied name, a path with a slash, and a new name in a folder you can read but cannot modify. Check the status text and VoiceOver announcement, no dialog, full filename selection, and that the existing file is untouched. Try `case.txt` → `CASE.txt` and check the directory's actual spelling in Finder. |

### Inline tab rename — native verification (2026-11-23, `780a26f`)

Fresh profile, Pinyin input method. Double-clicking the permanent tab `新建文本文档.txt` turned its
label into a field with `新建文本文档` selected; typing `nihao` showed the composition, Space
committed `你好`, Return renamed the file on disk to `你好.txt`, and the tab and window title
followed. (The field's accessible label read "Rename" only because the test bundle reused older
resources without the new catalog entry; the shipped catalog has 「重命名」.)
Not yet checked natively: Escape, focus loss, collisions, read-only folders, VoiceOver speech.

## Settings window (2026-11-23, native review pending)

Use a fixture profile (`--data-dir`), fixture HOME/SHELL, both appearances and both UI languages.
Compare side by side with Typora's Preferences window.

- [ ] ⌘, opens a 720×480 "Settings" / "设置" window centred over the editor window; ⌘, again (from the
      editor or the Settings window) brings the same window forward with the search field focused.
      The app menu shows Settings… ⌘,; Preferences shows Settings… and Settings (JSON)….
- [ ] The window cannot be made smaller than 600×400, is absent from the Window menu, cannot enter
      full screen on its own, and joins an editor window's full-screen space when opened from it.
- [ ] Close button, ⌘W and Esc close it (Esc clears a search first). No save prompt, ever.
- [ ] Toggle Show line numbers and pick a color scheme: every open editor window changes at once.
      Edit `Preferences.sublime-settings` in the JSON window and save: the Settings window updates
      within a second. Comments and formatting in the file survive both directions.
- [ ] Font popup: lists installed families, type a few letters to jump, the preview box redraws in
      the chosen family and size. Indentation preview follows tab width and Insert spaces.
- [ ] Tab / Shift+Tab, arrows in the category list, Space on a checkbox, Enter on a popup, Left/Right
      on a radio group; focus rings are visible in light and dark.
- [ ] Pinyin input: type 换行 in the search field (candidate window under the field, results filter on
      commit); type in the Encoding field and the Hidden files list.
- [ ] Hover the "?" of a row: the description and the "Takes effect" note appear after a short delay.
- [ ] Buttons: Edit Settings in JSON…, Key Bindings…, Mouse Bindings…, Browse Packages, Customize
      Color Scheme… each do what the menu item does.
- [ ] Quit with the Settings window open, relaunch: it does not come back; editor windows restore as
      before. Open a file from Finder while Settings is in front: it opens in an editor window.
- [ ] Close every editor window while Settings stays open; ⌘N opens a new editor window; Dock click
      brings Settings forward. Close Settings last, then ⌘, from the menu bar opens it again.
- [ ] VoiceOver: see section 2.
- [ ] Windows (VM or CI artifact): Ctrl+, opens the window (no menu bar), Ctrl+K, Ctrl+, opens the
      JSON pair, Ctrl+Shift+. jumps to the previous modification, Ctrl+F6 skips the Settings window.

