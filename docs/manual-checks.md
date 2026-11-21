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

Record results in `crates/velocity/MANUAL_TEST.md` (section "Live checks") or tell the agent.
