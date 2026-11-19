# UI audit, round 5 (2026-11-20)

Starting point: main `e6afe5f` (lane B round 23 merged: menus with key equivalents, settings, and
phase 1 of the Simplified Chinese UI — `ui_language`, `zh-Hans.sublime-localization` with 538
entries, translated menus, app menu and command palette, palette search in both languages;
design round 4). Scope: the visual and layout pass on the Chinese UI, with the review findings
in `specs/vel-ui-menus-settings-i18n.md` (section D5, "Layout") as the list, and the scenes the
design record needs. Same method and clean-room rules as `ui-audit-r1.md`: offscreen renders
through the real `App::scene` → Metal path with CoreText fonts (PingFang SC reaches CJK text
through the system fallback), public Sublime Text documentation and the macOS HIG for
conventions, all values and artwork our own.

## Renderer additions

- `velocity --render-scenes DIR … --language en|zh-Hans` writes `ui_language` into the
  fixture's User preferences, so every scene starts in that language the way a user's setting
  makes it (no in-memory override, the normal start-up path).
- A second project folder, `文档`, holds documents with Chinese names (`说明.md`, `更新日志.md`,
  `配置.toml`, `设计/界面草图说明（第二版）.md`). Only the round-5 scenes open it (through
  `r5.sublime-project`), so the side bar and Goto results of earlier scenes are unchanged.
- Ten scenes, each meaningful in both languages (the same files, only `ui_language` differs):
  `r5_palette` (command palette, "save"), `r5_palette_mixed` ("rust": Chinese captions, English
  syntax and build-system names, English-alias hits), `r5_palette_all` (empty query, shortcut
  hints), `r5_find`, `r5_find_in_files`, `r5_goto_anything` ("说明"), `r5_goto_symbol` ("@"),
  `r5_sidebar` (Chinese names in the tree, the tabs and Open Files, an unsaved Chinese
  document, the status bar), `r5_input_panel` (Side Bar › Rename… on `说明.md`, run through the
  side bar's own `sidebar_command`, so caption and initial text are the real ones) and
  `r5_narrow` (440 × 360, eight tabs of which four have Chinese names, the palette on
  "neighbor").
- Under `--language zh-Hans` the round-5 scenes also get lane B's Chinese layout report
  (`<scene>-<appearance>@<n>x-layout.json`: every text run containing CJK, measured with its
  painted style against its clip). For these scenes it reports and does not fail: a scrolled
  tab is clipped on purpose. It reported nothing. Lane B's own `zh_hans*` scenes keep their
  strict check, and they still pass.
- Native menus (the menu bar, context menus) and alerts (unsaved changes, move to Trash) are
  AppKit's `NSMenu`/`NSAlert`. Velocity does not draw them, so they cannot be rendered offscreen.
  The menus were audited from the menu model the renderer dumps (`zh_hans-menu.json`, 567 rows
  under 12 top-level menus). The only dialog Velocity draws itself is the input panel.

`before/`: main `e6afe5f` plus only this round's `render_scenes.rs`, exported with `git archive`
and built in its own target directory. `after/`: this branch.

## Findings and fixes

Each entry: scenario · problem — fix — test.

### Fixed

1. **English-alias hits painted as full matches** · `r5-palette_mixed` — in Chinese a row that
   the query finds through its English caption is highlighted as a whole (lane B's rule; its
   byte positions do not apply to the translation). That is right for "save" → 保存, 全部保存,
   文件：另存为… (`r5-palette`). But the matcher also finds rows whose English caption only
   holds the query's letters scattered through other words. "rust" finds
   偏好设置：自定主题 (P**r**eferences: C**ust**omize Theme), 上一处修改 (P**r**evio**us**
   Modifica**t**ion), 查找：上一个匹配词 and seven more. Ten of the thirteen rows shown were drawn
   entirely in the match colour, which made them look stronger than the real hits ("Rust" in
   代码片段：Rust 函数, Set Syntax: Rust). In English the same rows show their scattered letters
   and read as weak. Fix: an alias hit highlights the whole caption only when every run of
   matched letters starts a word of the English caption (at its start, after a non-letter, or
   at a lower → upper hump). "save" in "File: Save All", "enc" in "Save with Encoding" and
   "utf8" in "UTF-8" qualify. Other alias hits stay listed at the same rank, with no highlight.
   Direct hits on the Chinese caption keep their exact positions. Tests:
   `panels::tests::alias_hits_must_start_words` (six word-aligned matches and four scattered
   ones, plus empty, out-of-range and inside-a-character positions) and
   `panels::tests::chinese_palette_highlights_only_word_aligned_alias_hits` (a zh-Hans
   workspace through `show_overlay`: "save" → 保存 highlighted whole; "rust" →
   偏好设置：自定主题 with no highlight; "主题" → exactly its two characters). Lane B's
   `live_language_switch_and_bilingual_palette` still passes.
2. **Cutting a caption hid what tells rows apart** · `r5-narrow` — palette captions read
   "category: verb: argument" (in Chinese joined with "："), and sibling rows differ only in the
   last segment: 上/下/左/右, an encoding, a layout. A narrow palette cut the end, so four rows
   read "显示：编辑组：将所选选项卡移至相邻编辑组：…" (and "…Neighboring Group: Do…" in
   English). Fix: a command-palette caption that does not fit keeps its last segment,
   separator included, and shortens the part before it: "显示：编辑组：将所选选项卡移至相邻编辑…：上"
   and "View: Groups: Focus Neighboring Gro…: Down". This is how `fit_label` already keeps
   " (Default)". The match colour follows the characters into both parts and never lands on
   the ellipsis. When the segment would leave less than three ems for the rest, the end is cut
   as before. Captions that fit, and Goto and file rows, are painted exactly as before.
   Implemented as `ThemeUi::text_fit_highlighted_keeping` (separators "：" and ": "; the last
   one is looked up only for captions that do not fit), with the highlight mapping shared with
   `text_fit_highlighted` (`highlight_ranges`). Tests:
   `vel_ui::theme::tests::fitted_captions_keep_their_last_segment` (Chinese and English:
   fits → unchanged; at 70 % → stem cut, segment kept, both matches mapped to painted bytes,
   inside the width; segment + 2 em → end cut; no segment → end cut) and
   `panels::tests::narrow_palette_rows_keep_their_last_segment` (four Chinese and four English
   siblings at 300, 360 and 440 DIP, with and without a shortcut hint: every row ends in its
   own segment inside its clip, and all four stay distinct).
3. **Input panel caption ran under its field** · `r5-input_panel` — the caption sat in the
   find panels' fixed 80-DIP label column and was never measured. "New name for this entry"
   (about 130 DIP) ran under the field and the selected file name, and was unreadable in either
   language. Side Bar › New File… and New Folder…, Save Macro, Goto byte offset and the package
   prompts have the same problem, and so would any translated caption. The panel was also 68
   DIP tall for one 28-DIP field, leaving an empty 32-DIP band below it. Fix: the caption is
   measured with the style it is painted with, and its column is the caption plus 8 DIP on each
   side. The column is at least 80 DIP, so short captions and Vintage's ":" line up with the
   find panels, and at most half the panel; a longer caption ends in an ellipsis. The
   measurement happens in paint, and a changed measurement lays the panel out again before it
   is painted, so even the first frame is right. The panel is now one row: 8 + 28 + 8 = 44 DIP.
   The Find, Replace and Where labels are fitted to their column too, which changes nothing in
   English and protects their translations. Test:
   `panels::tests::input_panel_captions_end_before_their_field` (UI scale 1 and 2, 900 and 420
   DIP, English, Chinese, ":" and two over-long captions: the panel is 44 DIP, the field keeps
   at least half the panel at y + 8, the caption ends at least 8 DIP before the field, ":" gets
   an 80-DIP column, a caption that fits is drawn whole and one that does not ends in "…").

### Checked, no change needed

4. **CJK vertical metrics** · `r5-palette`, `r5-sidebar`, `r5-narrow` at 2x — CJK runs centre
   like Latin ones. In the 32-DIP palette rows the ink centre of 保存 is within 1 DIP of the row
   centre (measured at 2x); the tabs, the side bar and the status bar were checked by eye.
   Nothing is clipped above or below the glyphs. Lane B's ink-bounds check on the mixed caption
   (文件：以指定编码保存：UTF-8, UI scale 2) still passes at 1x and 2x.
5. **Mixed CJK/Latin spacing and punctuation** — all 538 catalog entries were scanned. There
   is no space between two CJK characters ("保存 文件"), no CJK character touching Latin
   without a space ("关于 Velocity" and "折叠至第 1 层" are as macOS writes them), no ASCII colon
   or comma next to CJK, and no "..."; composed captions use the full-width "：". The ellipsis
   (U+2026) comes from the system font as baseline dots, as in macOS's own Chinese menus.
6. **Ellipsis placement** — fitting is by grapheme (lane B), so cuts never split a cluster:
   "界面草图说明（第二版…" in the side bar, "图说明（第…" on a tab scrolled half out of the
   strip.
7. **Shortcut hints and long captions** · `r5-palette_all`, `r5-narrow` — the caption column
   ends 24 DIP before the hint at every width, so there are no collisions.
8. **Highlight for "save"** · `r5-palette` — the whole Chinese caption, as specified.
9. **Native menus** (model audit) — every row is translated except the intended names
   (encodings, syntaxes, build systems, the language name "English", recent paths) and the
   dynamic macro rows (see below). The longest caption is 13 characters
   (从项目中移除所有文件夹). AppKit sizes menus to their content, so menus cannot truncate.

### Still English in Chinese mode (lane B, phase 2: listed, not changed here)

- Palette and Goto footers: "Enter run · Esc cancel", "Enter open · Cmd/Ctrl+Enter split · Esc
  cancel" ("Cmd/Ctrl" should be ⌘ on macOS in either language).
- Dynamic palette rows: "Set Syntax: Rust" and "Tools: Build System: Rust" stay English as a
  whole. The verb could be translated and the name kept (设置语法：Rust).
- Catalog consistency: "File: Save" → 保存 and "File: Save All" → 全部保存 drop the 文件：
  category that every other File row has (文件：另存为…, 文件：关闭所有文件).
- Find, Replace and Find in Files: the labels Find/Replace/Where, the buttons
  Find/Find Prev/Find All/Replace, the "30 matches" status and the tool tips.
- Status bar: "Ln 41, Col 1", "4 characters selected", "Spaces: 4", "Tab Size: 4", "Unix".
- Side bar headings OPEN FILES and FOLDERS; Goto Symbol's "Line 7" annotations.
- Input panel captions ("New name for this entry", …) and Vintage prompts.
- Menus: Tools › Macros rows read "Macro: Default/Add Line…".

## Held back

- **CJK in the editor** (`r5-sidebar`): a CJK character takes two cells of the monospace grid,
  which is wider than PingFang's 1-em advance, so Chinese prose in a document reads
  letter-spaced ("一 个 小 型"). That is the editor grid's column model, not chrome. It needs its
  own decision: centre the glyph in its cells, or use proportional CJK runs that keep grid
  columns for the caret.
- **Find in Files "…" button** (lane B, `r5-find_in_files`): a text ellipsis drawn at 70 % of
  the button height, about 2–3 DIP below the close button's centre line, with no hover state
  and no tool tip. It belongs in the find panel's next design pass.
- **Scattered alias hits stay listed.** They are now unhighlighted; whether they should be
  ranked lower or dropped is a matcher question for lane B.
- Goto Anything shows absolute paths on the details line for files in a second project
  folder; English does the same.

## Render set

`after/` (55): 40 renders at 1x — the ten round-5 scenes × `zh` and `en` × dark and light,
named `r5-<scene>-<zh|en>-<dark|light>@1x.png` — and 15 crops at 2x: `zh` dark and light of
`palette` (input and first rows), `palette_mixed` (the whole list), `narrow` (whole window),
`input_panel` (the panel and status bar) and `sidebar` (side bar, tabs, document); `en` dark
and light of `narrow` and `input_panel`, and `en` dark of `palette`.

`before/` (13): `r5-palette_mixed-zh-*@1x` (whole-row highlights for "rust"),
`r5-narrow-{zh,en}-*@1x` (cut-off segments), `r5-input_panel-{zh,en}-*@1x` (caption under the
field), and 2x crops of `narrow` zh dark, `input_panel` zh light and `palette_mixed` zh dark.

Command used: `env -i HOME=… SHELL=/bin/sh PATH=/usr/bin:/bin TMPDIR=/tmp/ velocity
--render-scenes DIR --scale 1,2 --only r5_palette,… --language zh-Hans` (and `--language en`).

## Performance

App-level scene cost (`--render-scenes DIR --bench-scene 200 --scale 1 --appearance dark
--language zh-Hans`, retired instructions per `App::scene`, before → after, two runs):

| scene | before | after | |
| --- | --- | --- | --- |
| `r5_palette` | 1,848,224 | 1,852,317–1,862,793 | +0.2–0.8 % |
| `r5_palette_all` | 1,720,273 | 1,713,278–1,714,356 | −0.4 % |
| `r5_narrow` | 968,961 | 969,375–969,954 | +0.0–0.1 % |
| `r5_input_panel` | 1,478,953 | 1,518,624–1,519,066 | +2.7 % |

The input-panel difference is the editor: the panel is 24 DIP shorter, so one more line of code
is painted (line 63 against 62). The caption costs one cached measurement per frame, and only
while the panel is open. The alias check runs once per published result list, not per frame.

## Goldens and fixtures

`crates/velocity/fixtures/theme-scenes.json` is unchanged. The render fixture gained the
`文档` folder and `r5.sublime-project`, which only the round-5 scenes use.
