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

Record results in `crates/velocity/MANUAL_TEST.md` (section "Live checks") or tell the agent.
