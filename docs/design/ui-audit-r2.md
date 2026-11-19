# UI audit, round 2 (2026-11-19)

Scope: the screens round 1 covered thinly — the gutter, the side bar, popups (completion,
definition hover, phantoms, annotations, minihtml popups, tool tips), the status bar at narrow
widths, the settings window, Goto Anything previews, quick panels with many results and in small
windows, the minimap and scroll bars, and a dark/light parity sweep of both colour schemes.
Same method and clean-room rules as `ui-audit-r1.md`: offscreen renders through the real
`App::scene` → `vel-render` Metal path with CoreText fonts, compared against general Sublime
Text 4 conventions (public screenshots and docs only) and the macOS HIG; no Sublime package,
theme, icon or asset was read, all values and artwork are our own.

## Renderer additions

`velocity --render-scenes` (see `ui-audit-r1.md` for usage) gained scenarios, now 36 × dark/light
× 1x/2x = 144 images:

- `editor` now shows real gutter state: a collapsed fold (`impl fmt::Display` body), a bookmark,
  unsaved edits against the file on disk (added line, modified line, deleted lines → all three
  diff marks), and the pointer resting in the gutter so fold buttons are faded in.
- `minimap_scrollbar` (pointer on the overlay scroll bar), `minimap_hover` (pointer on the
  minimap: viewport box), `sidebar_focused` (keyboard focus in the side bar), `status_narrow`
  (520 × 340 with Git, indentation and syntax items, hovered syntax item), `status_bar` (hovered
  syntax item), `quick_panel_many` (command palette with an empty query), `overlay_small`
  (Goto Anything in a 420 × 300 window).
- Status-bar hover needs the previous frame's measured layout, so that scenario builds one more
  frame before moving the pointer. Tool tips are rendered as after their rest delay (the timer is
  back-dated by 1 s), see "Tool-tip timing" below.
- `VELOCITY_RENDER_DEBUG=1` also lists every painted icon surface (rect, clip, tint, source).

`before/` images: main `5497418` plus only this round's `render_scenes.rs` (exported with
`git archive`, built in its own target directory). `after/`: this branch with the review fixes.

## Findings, ranked by user impact

Each entry: scenario · region — problem — why it matters — fix (status).

### P0 — invisible or wrong

1. **Fold buttons were never visible** · `editor` gutter — the fold-button theme surfaces were
   pushed into the same scene layer as the gutter and line backgrounds; a layer draws surfaces
   before solid fills, so every fold button was painted over (in the real app too, not just
   offscreen). Folding code by mouse was undiscoverable. Fix: gutter marks go to the next layer;
   fold buttons are 16 × 16 icons centred in a 12-DIP fold column right of the numbers, faded in
   on gutter hover as before; a collapsed fold keeps its button while the others are faded, so
   hidden lines stay discoverable (fixed).
2. **Bold UI text rendered regular, and UI text used the display optical size** · everywhere —
   the system UI font is one variable file (`SFNS.ttf`); CoreText's bold face is that file at
   `wght` 700, and at 11–17 pt CoreText uses `opsz` 17 (the "text" cut). The rasterizer loaded
   the file's default instance (`wght` 400, `opsz` 28), so `font.bold` (side-bar headings, Git
   badges, kind letters), `<b>`/`<h3>` in popups and definition links came out regular, and all
   UI text used the tighter, thinner display cut at small sizes. Fix: `vel-render` reads
   `CTFontCopyVariation` for every face it loads and applies those coordinates when shaping,
   measuring and rasterizing; static faces (Menlo, SF Mono, fallbacks without variations) are
   unaffected (fixed). This is the most visible change in every image: labels are slightly wider
   and sturdier, matching native macOS text. A variable *editor* font is measured at the same
   instance (cell width, ascent), so its glyphs are not squeezed into the default instance's grid
   (review L1). The optical size is CoreText's for the face's creation size (17 for text sizes);
   popup headings at ~24 pt still use the 17 cut where CoreText would pick a larger one — a
   fidelity difference only, left as is (review L7).

### P1 — overlapping, clipped or misleading

3. **Status bar items overlapped at narrow widths** · `status_bar`, `status_narrow` — Git,
   indentation and syntax sat at fixed offsets from the right edge (600/310/180 DIPs), so
   "Spaces: 4" ran into "Rust", and Git text into the position text below ~900 px. Fix: items are
   laid out right to left with measured widths, in priority order (syntax, indentation, Git,
   indexing); the first item that would leave fewer than 140 DIPs for the position/message text
   ends the row, so lower-priority items drop first and never take a higher-priority item's place
   (review L3). Labels are measured with the style they are painted with (`vcs_status` for Git)
   and padded by the theme's `status_button` `content_margin` (review L4). The position text is
   ellipsized to the room left. Each item is a `status_button` (hover wash, 4-DIP radius, 2-DIP
   gaps) so it looks clickable; the Git item keeps its branch icon. Clicks use the drawn columns
   over the bar's full height (window edge stays clickable); before the first paint nothing in
   the status bar is clickable, and the accessibility node of the syntax item uses the painted
   rectangle too (review L5) (fixed).
4. **Goto Anything details wrapped into a half-hidden line** · `overlay_small` — a long path
   wrapped inside the row's single detail line and showed only the top half of its second line.
   Fix: quick-panel details are laid out with `white-space: nowrap` and clipped at the row edge
   (fixed).
5. **Squeezed inputs: text sank out of the box** · `replace_300_ui2` — at UI scale 2 in a 300-px
   window the inputs are shrunk to 28 DIPs; text, selection and caret kept their unshrunk
   offsets and the descenders were cut off. Fix: vertical offsets scale with the box, so cap
   height to descender stays inside (fixed; the ascender of a 26-pt glyph still touches the top
   of a 28-DIP box, which is the box's limit).
6. **Completion popup** · `completion` — rows ran edge to edge, kind letters were not centred in
   their badges, and the footer (kind name, details) was cramped against the last row. Fix:
   4-DIP inset, 18 × 18 badges with the letter centred, a hairline above the footer, footer 28
   DIPs (52 when any item in the list has details, so the popup keeps one height while the
   selection moves and does not jump above the caret). Clicks map through the rows as painted
   (first item and row count are recorded by paint); the rows that fit are computed with the
   padding and the footer, so the popup is only clipped when not even one row fits; clicks on the
   padding or footer are swallowed instead of reaching the text below (review M1) (fixed).
7. **No sign of more quick-panel results** · `quick_panel_many` — the palette shows 14 of ~100
   rows with nothing indicating the rest. Fix: a slim 4-DIP scroll indicator at the list's right
   edge (`puck_control` under `quick_panel`, theme-overridable) sized and placed by the scroll
   position (fixed).
8. **Side bar** · `sidebar`, `sidebar_focused`, `settings_window`, `empty_window`
   - every file had the same page icon → original file-type icons: code (`< >`), text (lines),
     config (`{ }`), image (hill and sun), mapped by extension/name in
     `Velocity.sublime-file-icons` (generated, 1x/2x/3x) (fixed);
   - the selected row looked the same with and without keyboard focus → a stronger
     `selected_focus` wash while the side bar is focused (dark `#395869`, light `#abbec7`; label
     contrast ≥ 6.3:1) (fixed);
   - with no folders the FOLDERS heading had nothing under it → "No folders open" and
     "Project › Add Folder…" hint in the muted label style, directly under the heading with or
     without the Open Files group (review L2) (fixed);
   - open-file labels lost their qualifier ("Preferences.sublime-settings (Default)" → elided)
     → the label keeps a trailing " (…)" and elides the name instead; with no room for even an
     ellipsis the label is left out rather than drawn over a close button or Git badge (review
     M3; also for every `text_fit` label) (fixed);
   - views without a path (default settings, untitled) had no icon in tabs and side bar → typed
     by their name (without the qualifier), generic page icon otherwise (fixed).
9. **Bookmark mark** · `editor` gutter — a "◆" text glyph from whatever fallback font had it,
   not centred on the line. Fix: a 6-DIP round accent dot centred on the line at the gutter's
   left edge; the gutter is 4 DIPs wider (12 left for bookmarks, numbers, 4 of space, 12-DIP
   fold column). Rows of 16 KiB or more (their own paint path) place their numbers with the same
   helper as every other row (review M2). Without line numbers the gutter is 12 + 16 DIPs when
   fold buttons are on (16 when they are off), so the fold column no longer covers the bookmark
   dots or gets clipped at the left edge (review L6) (fixed).
10. **Light scheme selection and both schemes' indent guides too faint** · `editor`,
    `find_options_on` — light selection was 1.19:1 against paper; indent guides 1.15–1.2:1.
    Fix: light `selection` `#c9dbe7` (1.30:1, text on it 8.6:1); a `guide` variable (dark
    `#253949`, 1.34:1; light `#dddbcf`, 1.27:1). Values and contrasts are recorded in
    `Color Scheme - Velocity/DESIGN.md` (fixed).
11. **Minimap viewport box faint in light** · `minimap_hover` — the accent at 20 % gave 1.17:1 on
    light paper versus 1.70:1 in dark. Fix: the light theme uses 18 % (30 % on hover or with
    `always_show_minimap_viewport`) (fixed).
12. **Inline code in popups read as prose** · `phantoms_popup_build_errors`, `hover_definition` —
    Fix: the default popup CSS gives `code`/`tt` a faint chip (foreground at 8 %, 3-DIP radius,
    3-DIP side padding); a scheme's `popup_css` still overrides it (fixed).
13. **Tab captions lost "(Default)"/"(User)"** · `settings_window` — same `fit_label` as the side
    bar (fixed).

### P2 — checked, consistent

- Gutter at 1x and 2x: numbers right-aligned on the text baseline, fold buttons and bookmark dots
  on pixel boundaries, diff marks (added bar, modified bar, deleted wedge) aligned to their rows.
- Popups and tool tips share one border colour, 6-DIP radius, shadow and padding in both themes;
  links use the accent and are underlined; headings are bold (now actually bold, see 2).
- Scroll bar thumb appears on hover over the overlay bar left of the minimap, readable in both
  appearances; it does not cover the gutter.
- Settings window: two groups, read-only default settings with JSON highlighting, both tabs keep
  their qualifier; status shows "Tab Size: 4 · JSON".
- Find highlights, bracket match, active match and selection were re-checked in both schemes;
  only the light selection and the guides changed (10).

## Review fixes

The independent review of this round found no High issues; the Medium and Low findings were
fixed on top of the rebased branch (`b0e985e`, on main `5497418`):

| finding | fix | test |
| --- | --- | --- |
| M1 completion clicks miss the last row of a clipped popup; padding/footer clicks reach the text; height jumps with the selection | paint records first item and row count, `hit` uses them; rows fit with padding + footer; footer height from "any item has details"; `PopupHit::Chrome` swallows padding/footer clicks | `completion::tests::popup_clicks_follow_painted_rows_and_its_height_is_stable` (view heights 60–260, popup below and above the caret, every painted row, padding, footer, outside; one height while arrowing) |
| M2 long lines draw their number 4 DIPs right, into the fold column | `View::line_number_x` shared by both row paths (`GUTTER_FOLD`) | `view::tests::long_line_numbers_align_with_the_other_rows` |
| M3 `fit_label` draws "…" with no room (over close buttons and Git badges) | empty label when the width is ≤ 0 or not even "…" fits; callers skip empty labels; `text_fit` too | `app_chrome::tests::narrow_tab_labels_never_draw_over_close_buttons` (12–20 fitted tabs), `panels::tests::narrow_side_bar_labels_never_draw_over_git_badges` (side bar 36–150 DIPs) |
| L1 variable editor font measured at the default instance | `Fonts::from_face` measures at the face's coordinates; Menlo unchanged bit for bit | `vel-render` `primary_grid_matches_the_shaped_instance` |
| L2 empty-folder hint misplaced without Open Files | hint row from `folder_row_base` | `panels::tests::empty_folder_hint_follows_the_folders_heading` |
| L3 lower-priority status item takes a higher one's place | the first misfit ends the row | `status_items_never_overlap_at_narrow_widths` (long syntax name, widths 220–900) |
| L4 status items measured with another style, padding hard-coded | measured with the painted style, padding from `status_button` `content_margin` | same test: every label ends inside its item |
| L5 test-only pre-paint status hit areas | removed; tests paint first and click `status_item` centres; a11y syntax node uses the painted rect | `status_bar_syntax_picker_…`, `round18_accepting_a_status_bar_syntax_row_…`, `syntax_detection_settings_status_picker_…`, `indentation_status_click_…` |
| L6 fold button overlaps the bookmark dot without line numbers | no-number gutter 28 DIPs with fold buttons (16 without) | `view::tests::gutter_without_numbers_keeps_fold_buttons_clear_of_bookmarks` |
| L7 `opsz` 17 for ~24-pt popup headings | noted, not changed (fidelity only) | — |

## Not fixed (with reason)

- Side-bar indentation guides and drag-over states: drag-and-drop targets are side-bar logic
  (lane C); Sublime Text 4 does not draw side-bar guides by default either.
- The inline build error annotation is clipped at the minimap edge (`phantoms_popup_build_errors`):
  annotations are right-aligned minihtml; eliding them needs an annotation layout change.
- Goto Anything titles are clipped without an ellipsis (the matched-character runs are laid out
  separately); the detail line is now single-line.
- Lane A (scope popup, markup completion kinds) and lane C (image view, hex-label boxes) are not
  merged yet; they get styled after they land.
- Encoding and line endings stay in the left position text (lane B's model and its menus).

## Tool-tip timing

Tool tips (find toggles, buttons, review UI) appear after the pointer rests 350 ms; definition
hover after 250 ms (requested only inside the text area). Both are unchanged. The renderer
back-dates the rest timer so scenes show the tool tip as a user would after the delay; a real
mouse check is still listed below.

## Goldens and fixtures (regenerated deliberately)

- `crates/velocity/fixtures/theme-scenes.json` (`VEL_UPDATE_GOLDENS=1`): no text content changed.
  Gutter glyph runs, line backgrounds and text x positions move 4 DIPs right (wider gutter);
  bookmark glyph runs become round rects; status-bar items move to their measured right-to-left
  positions and gain their button surfaces.
- `crates/vel-theme/tests/resource-colors.json`: regenerated for the new rules (side-bar
  `selected_focus`, `table_row` radius, light minimap opacities); the rule indices after the
  inserted rules shift, which is most of the diff.
- `crates/vel-color-scheme/tests/resource-colors.json`: regenerated (only `guide` in both
  schemes and the light `selection`/`inactive_selection` change); the test gained the same
  `VEL_UPDATE_GOLDENS` switch as the other goldens.
- `round16_tests.rs`, `round16_review_tests.rs`: bookmark checks count the round gutter dots
  instead of "◆" text runs.

## Behaviour touched

- Status-bar Git, indentation and syntax click targets follow the drawn items (full bar height)
  instead of fixed regions; an item that does not fit is neither drawn nor clickable.
- Completion popup clicks map to the rows as painted; clicks on its padding or footer no longer
  reach the text below (they used to move the caret and close the popup).
- Before the first frame no status-bar item is clickable (there was a fixed-region fallback).
- Without line numbers but with fold buttons the gutter is 28 DIPs (was 16).
- Fold-button click column is the 16 DIPs at the gutter's right edge (was 12 of a narrower gutter).
- With `fade_fold_buttons` (default) a collapsed fold's button is drawn without gutter hover.

## Tests added

- `vel-render` `variable_system_font_bold_and_text_cut_follow_coretext`: the system font's bold
  face carries variation coordinates, is wider and inks more than regular; regular carries the
  text optical size.
- `view::tests::fold_buttons_draw_above_the_gutter_and_collapsed_ones_stay_visible`: fold
  buttons are in a later layer than the gutter background; one button while faded (the collapsed
  fold), both on gutter hover.
- `panels::tests::status_items_never_overlap_at_narrow_widths`: 7 widths × UI scale 1 and 2 —
  items right to left with gaps, inside the bar, full-height targets, ≥ 140 DIPs left for the
  position text, whose clip ends before the leftmost item.

## Performance

After the review fixes, against main `5497418` (exported with `git archive`, built in its own
target directory with this round's `render_scenes.rs` for the scene benchmark), isolated
`HOME`/`SHELL`, temporary `--data-dir`, `/usr/bin/time -l` instructions retired, base/new
alternating:

| mode | 5497418 | this branch | change |
| --- | --- | --- | --- |
| `--headless-smoke` (median of 5) | 308.1M | 308.3M | +0.07% |
| `--headless-smoke --carets 1000` (median of 5) | 4,210M | 4,189M | −0.5% |
| `--headless-view-features spikes/data/rust-10mb.rs` (median of 3) | 215.24G | 214.78G | −0.2% |

The 1000-caret mode is noisy on this machine while other agents build (±10 % between runs of the
same binary); the coordinator's own run on the rebased commit gave −0.7 %.

App-level scene cost (`--render-scenes DIR --bench-scene 400 --only … --appearance dark --scale
2`), steady-state instructions per `App::scene`, mean of 2 alternating runs:

| scenario | 5497418 | this branch | change |
| --- | --- | --- | --- |
| editor | 1,787K | 1,807K | +1.1% |
| tabs (10 tabs, side bar) | 2,691K | 2,707K | +0.6% |
| groups_grid | 832K | 838K | +0.8% |
| sidebar (expanded tree, Git badges) | 460K | 466K | +1.4% |
| find (with tool tip) | 1,610K | 1,627K | +1.1% |
| replace | 1,561K | 1,581K | +1.3% |
| command_palette | 1,650K | 1,661K | +0.7% |
| goto_anything | 663K | 675K | +1.9% |

Repeating the base run gave goto_anything +1.5 % and editor +0.5 %, so differences below about
0.5 % are build-to-build variance. The review fixes themselves (the rebased `b0e985e` against
this branch, same method) measure +0.3 % to +1.2 %; per frame they add one cached style lookup
(the status-button padding) and nothing else on these scenes. Before the rebase the round
measured +0.3 % to +0.9 % against `f5f0f8a`.

A first measurement of the round showed sidebar +17 %, goto +10 %, groups +8 %: with ~100
file-type mappings, `FileIcons::resolve` scanned every key for a '/' (path mappings) on each
visible row every frame. Path keys are now indexed once when the icon theme is parsed or merged
(`vel-theme/src/textures.rs`). Variable-font coordinates cost nothing per frame (they are
resolved once per face).

## Still needs a native (real window) check

- Fold-button fade with a real mouse; diff marks after save/revert.
- SF text cut at 1x on external displays (hinting), and bold weights in CJK fallbacks.
- Status-bar item hover/press with a real mouse; Git popup from the status item.
- Tool-tip timing with a real mouse.

## Images

`before/` and `after/` (same names; 1x images are native 1x renders, 2x crops scaled to 45–80 %):
`r2-01-gutter-dark@2x` (fold buttons, bookmark dot, diff marks), `r2-02-gutter-light@1x`,
`r2-03-sidebar-dark@2x` (icons, bold headings, badges, hover), `r2-04-sidebar-focused-light@2x`,
`r2-05-settings-window-dark@1x`, `r2-06-empty-window-light@1x` (empty state),
`r2-07-completion-dark@2x`, `r2-08-completion-light@2x`, `r2-09-hover-definition-dark@2x`,
`r2-10-popups-light@2x`, `r2-11-popups-dark@2x` (phantoms, annotation, build error, popup),
`r2-12-status-narrow-dark@2x`, `r2-13-status-bar-light@2x` (hovered syntax item),
`r2-14-quick-panel-many-dark@2x` (scroll indicator), `r2-15-overlay-small-light@2x`,
`r2-16-minimap-hover-light@2x`, `r2-17-minimap-scrollbar-dark@2x`,
`r2-18-replace-300-ui2-dark@2x`, `r2-19-command-palette-light@1x`, `r2-20-tabs-dark@2x`.
Round-1 images are in git history at `f5f0f8a`.
