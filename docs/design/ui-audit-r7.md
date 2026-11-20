# UI audit, round 7 (2026-11-20)

Starting point: main `cddbb70`. Since round 5 it has gained lane B round 24 (palette command
icons in 14 families, the menu review, quick-panel footers with ⌘⌥⇧⌃ glyphs, the Find in Files
"…" (Where) button), lane C rounds 25–26 (measured CJK/emoji advances in the editor, CJK line
breaking in word wrap, `file_tab_style`, `show_tab_close_buttons_on_left`,
`overlay_scroll_bars`), lane B round 25 (phase 2 of the Chinese UI: find panels, side bar,
status bar, input panels, dialogs, titles, build headers, accessibility) and the review fix that
gives the output view measured CJK advances. Scope: the last visual review of the whole UI
before the menu, settings and Chinese work closes. Same method and clean-room rules as
`ui-audit-r1.md`: offscreen renders through the real `App::scene` → Metal path with CoreText
fonts, public Sublime Text documentation and the macOS HIG for conventions, all values and
artwork our own.

## Method

- Every scene in `render_scenes.rs` (71: rounds 1–6, 25 and 26) was rendered at 1x in both
  languages and both appearances: 284 PNGs. The panels, the palette, Goto, the side bar, the
  status bar and the tab strip were also rendered at 2x (25 scenes, 100 PNGs). The render
  scenes themselves are unchanged this round.
- English and Chinese, dark and light were compared side by side as stacked crops (2x crops
  zoomed further for corners, baselines and stroke weights).
- Contrast was measured, not estimated: `VELOCITY_RENDER_DEBUG=1` lists every text run with its
  colour, and the colour was compared (WCAG relative luminance) with the pixels under the run.
- After the fixes, all 284 1x renders were diffed pixel by pixel against `cddbb70`, so the list
  of changed states below is complete. In English the only differences are the focused tab's
  accent ends (the top 3 rows of the strip, 6 at UI scale 2), the rounded tab, the Find in Files
  panels and the Chinese `zh_hans*` scenes, plus `phantom_selections`, which differs between
  two runs of the same binary (see Held back).

`before/`: `cddbb70` (this branch before the fixes, same fixture). `after/`: this branch.

## Findings and fixes

Each entry: scenario · problem — fix — test.

### Fixed

1. **The rounded tab style had no accent line** · `r26-tabs-rounded-left` — the theme rule for
   `file_tab_style: "rounded"` draws the focused tab's accent with `tab-indicator-rounded.png`,
   but that texture was never added to the embedded theme resources (`theme_ui::BUNDLED`). The
   lookup failed silently, so a focused rounded tab looked exactly like an unfocused one. Fix:
   the texture is bundled at 1x, 2x and 3x. Test:
   `theme_ui::tests::every_texture_named_by_a_bundled_theme_is_bundled_at_every_density` (reads
   every `Theme - Velocity/…` texture the bundled themes and file icons name and requires all
   three densities; on `cddbb70` it fails for this texture, and it is the only one).
2. **The selected rounded tab was notched where it meets the sheet** · `r26-tabs-rounded-left`
   — `radius: 6` rounded all four corners of the selected tab's box. Its bottom corners showed
   the strip's background, and the strip's bottom hairline turned up into them, between the tab
   and the editor it belongs to: the selected tab is part of the sheet below it, and rounding
   its bottom corners cut the two apart. Fix: a selected tab with a radius paints its box one radius taller and clipped
   at the strip, so only the top corners are round. Hit rects, labels and unselected tabs (their
   hover wash stays a floating rounded shape) are unchanged. Test:
   `app::round26_settings_tests::selected_rounded_tabs_join_the_sheet_and_keep_their_accent_line`
   (UI scale 1 and 2, dark and light, square and rounded: no rounded fill of the tab ends inside
   the strip, and the accent's ink spans the whole tab (square) or stops 5 DIP inside each side
   (rounded)). Lane C's `rounded_and_square_tab_styles_change_the_drawn_tab_not_the_hit_rect`
   now expects the taller box.
3. **The accent line faded at both ends** · every focused tab (`tabs`, `r26-tabs-square` and 140
   other renders) — the square indicator is nine-sliced at `[0, 2, 0, 0]`, a single slice
   stretched to the texture's edges, so it was filtered against the atlas beyond them: at 1x the
   line ramped from about 30 % to full over its first and last 6 DIP (2–3 DIP at 2x). The
   rounded texture was worse: its 4-texel middle sat between transparent texels and faded over
   about 10 DIP. Fix: one-DIP end slices for the square bar (`[1, 2, 1, 0]`), and a rounded
   texture whose ink (columns 5–10) reaches one texel into each corner slice. Stretched slices
   now only sample ink, and the ends are crisp; the rounded bar stops 5 DIP inside the corners.
   `generate.py` holds both changes; running the whole generator in a scratch copy reproduces
   the committed theme, textures and registry byte for byte. Test: the test of finding 2 also
   requires every stretched slice of the bar to have neighbours (or, at the texture's edge,
   nothing) with the same alpha as its edge texels; it fails with the old textures (checked).
4. **Find in Files buttons ran past the fields** · `r5_find_in_files`, `find_in_files`,
   `find_in_files_short`, `r25_find_in_files_ui2`, `zh_hans_mixed` — Find in Files reserves
   26 DIP for its Where (…) button beside the close button. The fields were shortened by it,
   but the action row still aligned its buttons to the old right edge (panel − 40 DIP), so
   Find/Replace (查找/替换) ended 26 DIP to the right of the fields, under the edge buttons'
   column. Fix: the action area ends where the inputs end (12 DIP plus the edge reserve). The
   Find and Replace panels are unchanged (their reserve gives the old 40). The one-row layout now
   needs 26 DIP more, so in Find in Files the buttons take their own row a little earlier: in
   702–728 DIP wide windows with the side bar at UI scale 1, and in `r25_find_in_files_ui2`
   (UI scale 2 in a 1200-pixel window), which is what the layout already did below that width.
   Test: `panels::details::tests::find_panel_buttons_end_with_the_inputs` (find, replace and
   find in files at UI scale 1 and 2, 640–1600 DIP: the last button ends with the inputs, the
   edge buttons stay to its right, the toggles start with the inputs); the 10,000-layout
   `find_panel_layout_sweep_never_overlaps_and_keeps_controls_inside` still passes.
5. **Bold Chinese was drawn regular** · `r5_sidebar` and every Chinese scene with a side bar —
   the side bar headings are bold 11 pt. OPEN FILES and FOLDERS were bold, 打开的文件 and 文件夹
   were regular, so the Chinese headings were lighter than the English ones (round 25 keeps "the
   heading font, size and weight"). Cause: asked for bold, CoreText answers for the system UI
   font's Chinese fallback with `.AppleSimplifiedChineseFont-Bold`, a font with no file of its
   own (only a `wght` 700 variation). `native::font_variant` gave up ("variant font has no file
   URL") and the regular face was used. Fix (vel-render): when the bold copy has no file, its
   variation is applied to the original face, which gives `.PingFangUITextSC-Bold` from the same
   file. Bold CJK in editor scopes benefits as well; advances are unchanged. Test:
   `font::tests::bold_ui_runs_keep_their_weight_in_the_cjk_fallback` (bold 打开 resolves without
   the warning, has more than 1.2× the regular ink, and the same advance; measured 38,614 against
   25,767). Lane B's strict Chinese layout scenes (`zh_hans*`) still pass.

### Checked, no change needed

6. **Palette icons** · `r5_palette_all` (2x) — the 14 masks share one stroke (1.1 DIP, snapped
   per density), are tinted muted on rows and with the foreground on the selected row, and sit at
   y + 8 in the 32-DIP rows, i.e. centred. Optical sizes differ (ink 17–25 px wide at 2x: file and
   bookmarks narrow, view, project and package wide), within what one family reads as; the
   neutral mask is three dots. Goto Symbol's letter badges are centred and keep their colours.
7. **CJK and Latin on one line** — palette captions (文件：以指定编码保存：UTF-8), tabs
   (更新日志.md), the status bar (46 行，9 列 · UTF-8 · Unix), and the find labels against their
   fields (查找 / task) share one baseline at 1x and 2x.
8. **Contrast** — hints and footers are the muted colour on the overlay: 6.1:1 light, 5.5:1
   dark. Placeholders (65 % of the foreground) are 4.2:1 in light, like macOS placeholders. Details on
   the selected Goto row are 3.95:1 light and 4.1:1 dark (see Held back).
9. **Status bar** — left items are joined with " · ", right items are 19 DIP apart with no
   separator, like Sublime's. In Chinese, 空格：4 and 46 行，9 列 look wider than "Spaces: 4"
   because the full-width "：" and "，" carry their own space; that is the punctuation's design and how
   macOS sets Chinese UI text.
10. **Quick-panel footers** — both languages fit down to the 420 × 300 window
    (`overlay_small`: "↩ 打开 · ⌘↩ / ⌃↩ 在拆分视图中打开 · ⎋ 取消"); the glyphs come from the system
    font in both appearances.
11. **Side bar headings without uppercase in Chinese** — 11 pt bold Chinese is optically taller
    than 11 pt bold capitals. Matching the capitals' height would take about 9 pt, too small for
    Chinese strokes at 1x, so the size was kept; with the weight fixed (finding 5) the two
    headings read as the same element.
12. **Input panel, output panel, Goto Symbol annotations (第 7 行), the narrow palette** — as
    rounds 5 and 25 left them.

### Still English in Chinese mode (for lane B; not changed here)

- Hover definitions popup: the headings "Definitions" and "References" and the link title "Open
  location" (`panels.rs`, `paint_hover`).
- Completion popup: the kind name "Word" of buffer-word completions (vel-complete, the `Kind`
  built for word completions).
- Scope popup (`show_scope_name`): "Copy scope" and "1 scope" / "N scopes, outermost first"
  (`html_ui.rs`).
- Image fallback: "Not shown as an image: unsupported or damaged" (`app.rs`,
  `IMAGE_FALLBACK_NOTE`), shown on the sheet and in the status bar.
- A regular-expression error keeps the engine's English detail after the translated prefix
  (正则表达式无效：regex parse error: …). By the round-25 terminology rules that is library output
  and stays verbatim; listed so the decision is explicit.

## Held back

- **Where the "…" button sits.** Lane B put it beside the close button on the Find row and
  tests that their glyphs share a centre line, so it reads as a panel control rather than as
  part of the Where field it edits (its tool tip says 位置… / Where…). Moving it to the Where row
  would put it next to what it changes and give the Find and Replace fields the full width
  again; that is a product decision, not a fix.
- **Details on the selected row** (3.95:1 light, 4.1:1 dark): the muted colour on the selection
  colour. Raising it needs a separate colour for selected details in both themes; it is a theme
  decision and changes English as well.
- **Nine-slice filtering at texture edges in general.** The hairlines (`edge-top`,
  `edge-bottom`, `edge-right`) are stretched to their edges the same way, but their colour is
  within one level of the background, so nothing is visible. A half-texel inset in
  `nine_slice::paint` would cover every texture; not needed for this release.
- **`phantom_selections` is not deterministic**: two runs of the same binary differ in 2-pixel
  bands every 16 pixels of its selection. It is not a regression from this round; it makes that
  scene unusable for pixel diffs until its owner settles it.
- From round 5: Goto Anything shows absolute paths for a second project folder; scattered
  English-alias hits stay listed (lane B's matcher).

## State of the UI

At parity with Sublime Text's public screenshots and documentation: the tab strip (square and
rounded styles, close buttons on either side, dirty markers, scrolling and fitting), the side bar
(Open Files, folders, headings in both languages), the status bar, the command palette (icons,
macOS key glyphs, footers, highlighted matches in both languages), Goto Anything and Goto Symbol,
the Find, Replace, Find in Files and input panels, the output panel, completions, popups and
annotations, and the Chinese chrome as a whole. Chinese text now has the same weights, baselines
and truncation as English.

Still visibly behind, and why:

- Flat chrome: the two themes are opaque colours with no translucency (vibrancy) behind the
  side bar or the overlays, and no motion beyond hover fades. Deliberate for 1.0 (one code
  path, predictable contrast in both appearances).
- File-type icons: five masks (code, config, image, text, generic) against Sublime's per-type
  set. Enough to scan a tree; more icons are artwork work, not layout.
- The Find in Files "…" placement and the selected-row details contrast (Held back).
- Four English strings in Chinese mode (listed above, lane B).
- Native menus and alerts are AppKit and match the system by construction; they cannot be
  rendered offscreen and were audited from the menu model in round 5.

## Render set

`after/` (28): fixed states — `r7-tabs_rounded_left-{en-dark,en-light,zh-dark,zh-light}@2x`,
`r7-tabs_square-{en-dark,zh-light}@2x` (tab strip crops), `r7-find_in_files-{en,zh}-{dark,light}@1x`,
`r7-find_in_files-zh-dark@2x` (the panel), `r7-find_in_files_ui2-{en-dark,zh-light}@1x`,
`r7-sidebar-{zh-dark,zh-light,en-dark}@2x` (side bar crops), `r7-sidebar-zh-dark@1x`; overview
— `r7-palette_all-{zh-dark,zh-light,en-light}@1x`, `r7-palette_all-zh-light@2x` (icons and
rows), `r7-palette_mixed-zh-light@1x`, `r7-goto_anything-zh-light@1x`, `r7-find-zh-dark@1x`,
`r7-status_selection-zh-light@1x`, `r7-input_panel-zh-dark@1x`, `r7-build_output-zh-light@1x`,
`r7-narrow-zh-dark@1x`.

`before/` (9): `r7-tabs_rounded_left-{en-dark,zh-light}@2x` (no accent, notched corners),
`r7-tabs_square-en-dark@2x` (faded ends), `r7-find_in_files-{en-dark,zh-light}@1x` and
`r7-find_in_files-zh-dark@2x` (buttons past the fields), `r7-find_in_files_ui2-en-dark@1x`
(one-row layout with the overhang), `r7-sidebar-zh-{dark,light}@2x` (regular-weight headings).

Scene names map to `render_scenes.rs` as `tabs_rounded_left` = `r26-tabs-rounded-left`,
`tabs_square` = `r26-tabs-square`, `find_in_files_ui2` = `r25_find_in_files_ui2`,
`status_selection` and `build_output` = `r25_*`, the rest = `r5_*`.

Command used: `env -i HOME=… SHELL=/bin/sh PATH=/usr/bin:/bin TMPDIR=/tmp/ velocity
--render-scenes DIR --scale 1 --language en|zh-Hans` (all scenes, both appearances), and
`--scale 2 --only …` for the 2x set.

## Performance

App-level scene cost (`--bench-scene 200 --scale 1 --appearance dark --language zh-Hans`,
retired instructions per `App::scene`, two runs each):

| scene | before | after | |
| --- | --- | --- | --- |
| `r5_sidebar` | 447,725–447,803 | 447,741–448,821 | +0.0–0.2 % |
| `r26-tabs-square` | 596,721–598,631 | 587,766–602,223 | −1.5–+0.9 % |
| `r5_find_in_files` | 1,537,112–1,551,773 | 1,538,593–1,540,342 | −0.9–+0.2 % |

The bold Chinese face is resolved once and cached like any other variant; the square bar adds
two 1-DIP slices per focused tab.

## Goldens and fixtures

`crates/velocity/fixtures/theme-scenes.json` and the render fixture are unchanged. The theme
changes are one rule in `Velocity.sublime-theme` (the square bar's slices) and the three
`tab-indicator-rounded` textures, both from `generate.py`.
