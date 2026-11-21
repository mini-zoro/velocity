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
- [ ] Settings (Graphical) (Preferences › Settings (Graphical)…): toggles read as switches with their state
      ("自动换行，关" / "Word Wrap, off"); popups and text fields read their value.
- [ ] Switch UI language to 简体中文 (Preferences › Language): the same places are read in Chinese.
Note anything silent, mislabeled or read in the wrong language.

## 3. Chinese input method (≈ 3 min)
Use the macOS Pinyin (Simplified) input method.
- [ ] Typing pinyin in the editor shows marked (underlined) text at the caret; the candidate window
      appears right under the caret, also after scrolling, in a wrapped line, and with 2 groups (split view).
- [ ] Selecting a candidate inserts it; Esc cancels marked text; undo removes the committed text in one step.
- [ ] The same in the Find field, the command palette and a Settings text field.
- [ ] Multiple carets (⌘D twice) + pinyin: the text is inserted at every caret.

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
