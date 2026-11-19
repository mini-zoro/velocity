# UI audit, round 3 (2026-11-19)

Starting point: main `569cc9c` (design round 2 merged, lane C round 20 and lane A round 21
merged). Scope: the verifier's leftovers that also exist on main (fitting, completion placement
and scale, tab hit testing, narrow side bars), the elements merged in lane C round 20 and lane A
round 21 that had no design pass (image view, invisible-character labels, side bar headings and
guides, the `show_scope_name` popup), and two round-2 leftovers (build-error annotations at the
minimap edge, Goto Anything titles). Same method and clean-room rules as `ui-audit-r1.md`:
offscreen renders through the real `App::scene` → Metal path with CoreText fonts; only public
Sublime Text documentation and screenshots for conventions; all values and artwork are our own.

## Renderer additions

`velocity --render-scenes` gained ten scenarios (46 × dark/light × 1x/2x = 184 images):
`completion_short` (1000 × 280 window, caret on line 42 — the verifier's repro),
`completion_ui2` (UI scale 2), `image_view` (a 360 × 220 PNG with transparency),
`image_fallback` (a damaged `.png`), `invisibles` (zero-width space, no-break space, a
right-to-left override and its terminator, a bell character; caret on one line, another
selected), `scope_popup` (`show_scope_name` inside a format string, 9 scopes),
`narrow_tabs` (640 wide, 15 fitted tabs with one- and two-letter names, tab scrolling off),
`narrow_tabs_ui2` (the same at UI scale 2), `sidebar_narrow` (a 96-DIP side bar with Git badges
on deep rows), `sidebar_folders_only` (Open Files hidden). The image and invisible-character
fixtures live outside the project folder, so the side bar and Goto results of the other scenes
are unchanged. Scenes now also wait for image decodes.

`before/`: main `569cc9c` plus only this round's `render_scenes.rs` (exported with `git archive`,
built in its own target directory). `after/`: this branch.

## Findings and fixes

Each entry: scenario · problem — fix — test.

### Verifier leftovers (also on main)

1. **Short labels drawn unmeasured** · `narrow_tabs`, `sidebar_narrow` — `ThemeUi::fitted`
   skipped labels of ≤ 2 bytes, so "ab" in an 18-tab strip ran up to 7.7 DIP (23.8 at UI
   scale 2) into the dirty dot or close button, and "db" into a side-bar badge. Fix: the only
   shortcut left is the 1.4-em-per-character bound. Test:
   `app_chrome::tests::short_tab_labels_are_fitted_too` (18/22/26 tabs named "ab", UI 1 and 2:
   every label ends before the close button or dirty dot).
2. **Completion popup under the tab bar in short views** · `completion_short` — rows were
   counted against the whole view (tab bar included) and the popup was clamped to y = 0, so in a
   1000 × 280 window the selected row hid under the tabs and a click on the tab bar committed
   it. Fix: the popup is placed inside the text area (below the tab bar, above the view's bottom
   edge — panels and the status bar are outside the view): below the caret when the list fits
   there, otherwise above, otherwise on the side with more room; rows give way first (down to
   one), then the details line, then the footer; only a view without room for one row is
   covered. Tests: `completion::tests::popup_stays_in_the_text_area_of_short_views` (UI 1/1.5/2,
   view heights 40–400, every caret row, 1/3/8/40 items, with and without details: inside the
   area, whole rows, the caret line uncovered whenever a side has room) and
   `app_chrome::tests::completion_popup_stays_out_of_the_tab_bar_in_short_windows` (the repro:
   popup below the tab bar; a click on the tab bar does not commit).
3. **Completion popup at UI scale 2** · `completion_ui2` — text was drawn at 2× in 24-DIP rows and
   the footer touched its separator. Fix: padding, rows, badges, footer, separator, details and
   width (460 DIP) all scale with `ui_scale`; clicks use the painted, scaled geometry. The quick
   panel already scaled its rows (checked). Test: `completion::tests::popup_metrics_scale_with_the_ui`.
4. **Tab clicks that select a neighbour or close a tab** · `narrow_tabs` — two causes:
   (a) the 20-DIP close button was kept on tabs as narrow as 28 DIP and on partly visible tabs,
   where it covered the visible tab's centre, so a click "on the tab" closed it (39 of 4,270
   centre clicks across 400–1400 wide windows, 1–40 tabs, scrolling on/off, UI 1/2);
   (b) the first click of a double click reveals a partly visible tab, which slides the strip, so
   the second click landed on a neighbour. Fix: a close button is drawn (and clickable) only
   where it lies entirely in the visible tab *and* in its outer half; the close button, icon and
   label offsets scale with `ui_scale` (the strip already did); the second click of a double
   click within 4 points of the first goes to the tab the first click selected. Tests:
   `app_chrome::tests::every_painted_tab_and_close_button_centre_hits_itself` (7 widths × 10 tab
   counts × scrolling on/off × UI 1/2: every painted tab centre selects that tab, every close
   button closes exactly its tab; > 4,000 clicks) and
   `double_click_stays_on_the_tab_that_the_first_click_revealed`.
5. **96-DIP side bar: badges over icons** · `sidebar_narrow` — Fix: priority is badge, then
   label (shortens to nothing), then icon, then disclosure triangle: an icon or triangle that
   would end within 2 DIPs of the row's badge is left out (rows without a badge keep theirs; the
   side bar edge clips them). Test:
   `app_chrome::tests::narrow_side_bar_keeps_badges_and_drops_overlapping_icons` (side bar 96–220
   DIPs, nested folders with badges: no label or icon reaches a badge).

### Lane C round 20 elements

6. **Image view** · `image_view` — the picture sat on the window's clear colour (the sheet
   scene's own background is dropped when sheets are composed, so the scheme's paper never
   showed), with a monospace "W × H" at the bottom left, and the tab and side bar said
   "logo.png [hex]". Fix: the view paints the scheme's paper; the picture is centred with a 16-DIP
   inset and scaled down (never up) to fit; a checkerboard (two tones mixed 4 % / 10 % from paper
   to ink, 8-DIP squares, at most ~1,600) shows transparency only inside the picture; a hairline
   frame (18 % mix) outlines it; a 24-DIP footer in the line-highlight colour, in the UI font and
   muted colour, reads "360 × 220 pixels · 4.3 KB · 100%" (decimal units like Finder; the zoom
   shows when it is scaled down). An image file's tab is named by the file (no " [hex]") from
   its path matching `image_file_patterns`, before and after decoding; only a failed decode
   adds " [hex]" (review 7). Colours come from the document's colour scheme, else the active
   scheme through the theme palette, else the built-in palette (review). The status
   bar shows neither syntax nor indentation for byte or picture documents. Test:
   `view::tests::image_view_paints_paper_checkerboard_frame_and_footer` (light and dark, fitting
   and scaled down).
7. **Image fallback to hex** · `image_fallback` — a damaged image silently became a hex view, and
   in the light appearance the hex view was light text on the dark window colour (same composed
   background bug). Fix: the hex view paints its paper; when an image file could not be decoded,
   the status bar says "Not shown as an image: unsupported or damaged" when the tab opens, and a
   quiet note in the same words sits below the last row of bytes (never over them).
   `ImagePreview` records the failure (`failed`), nothing else in its logic changed.
8. **Invisible-character labels** · `invisibles` — `<0x200b>` was drawn at the full font size in
   a 1-px box of the same 60 %-muted colour, the brackets touching the box, faint on paper
   (2.9:1 dark, 2.3:1 light). In a selection the band stopped short: selection bands
   ignored the extra cells the labels occupy (also for inline phantoms). Fix: each label is a
   rounded chip (3-DIP radius, 2 DIPs inside the line) filled with the label colour at 10 %,
   with the code point at 80 % of the font size, centred, in that colour at full strength
   (on the chip 4.9:1 dark, 4.1:1 light; 5.7:1 / 4.6:1 against paper); in selections and find
   matches it takes the text colour. Selection bands are measured in display columns (labels and inline phantoms
   included; a selection starting at a labelled character starts before its label). The
   minimap draws the text itself, not the labels (unchanged). Tests:
   `view::tests::invisible_labels_are_chips_and_selections_cover_them` and lane C's
   `r20_invisible_cells_and_caret` (label text unchanged).
9. **Side bar indent guides** · `sidebar`, `sidebar_folders_only` — new: a hairline per ancestor
   level under that ancestor's disclosure triangle, from our own `sidebar_indent_guide` theme
   element (`fg` at 10 % in Velocity's themes; themes without the element draw none, as Sublime
   Text does by default). Covered by the narrow side bar test above.
10. **Headings with Open Files hidden** · `sidebar_folders_only` — FOLDERS stays as the first
    heading (it anchors the empty-state hint and keeps the section named); no change needed.
11. **Drag-and-drop hover** — there is no side-bar drop target in lane C's logic: files dropped
    on a window are opened through `Event::OpenPaths`, and AppKit's `draggingEntered` is not
    forwarded to the app, so there is no hover state to style. Listed for a later round (needs
    a platform event first). The native Window menu needs no paint.

### Lane A round 21

12. **`show_scope_name` popup** · `scope_popup` — every scope was its own inline code chip, so a
    9-scope stack read as a ragged column of boxes with no emphasis. Fix: one monospace block
    (foreground at 6 %, 4-DIP radius, 6 × 8 padding), outer scopes muted (62 %), the innermost
    scope — the one under the caret — bold at full contrast, then "Copy scope · 9 scopes,
    outermost first". Copy still copies the whole stack as one line. Long scopes wrap only at
    the popup's 480-DIP limit (scope names have no spaces; the longest real ones fit). The
    content keeps its `<br>` structure, so lane A's `round21_scope_popup_and_copy_link` is
    unchanged.

### Round 2 leftovers

13. **Build errors clipped at the minimap** · `phantoms_popup_build_errors` — the annotation
    wrapped inside 55 % of the view and was then cut by the minimap. Fix: one line
    (`white-space: nowrap`), shortened with an ellipsis to the room between the code and the
    minimap (at most 60 % of the text area); hidden when not even "…" fits. The full message is
    in the build output panel. Test: `html_ui::tests::annotation_messages_fit_the_room_left_of_the_minimap`.
14. **Goto Anything titles cut without an ellipsis** · `overlay_small` — captions with matched
    characters were clipped mid-glyph. Fix: the caption is fitted first; matched runs are drawn
    up to the cut and an ellipsis follows. Test:
    `panels::tests::long_matched_captions_end_in_an_ellipsis`.

## Review fixes (on `f0b3b37`)

| finding | fix | test |
| --- | --- | --- |
| 1 band offset when it starts at a labelled invisible; prefixes narrower than a label underflowed | bands are measured from one per-row prefix of display columns (`width + display_gap` per unit, exactly as the glyph loop advances); a band runs from its first unit's gap to its last unit's end — no subtraction | `view::tests::selection_bands_cover_labels_and_phantoms_for_every_boundary_pair` (all 300+ boundary pairs of a line with a ZWSP, NBSP, bidi controls and an inline phantom, wrap off and on, against a reference built from unit widths; every selected label inside its band) |
| 2 wrapped rows of a labelled line showed the band only on the last row | the same prefix is per visual row (the segment's own units), never `Layout::column` of a segment end | same test (wrap on, `wrap_width` 14) |
| 3 `Layout::column` twice per selection per row | the prefix is built at most once per row, lazily; each band is two binary searches; only regions that reach the row are visited (`partition_point`) | `--bench-scene` `selections_3000` (12 KB line, one NBSP, 3,000 selections) and `phantom_selections` (300 rows × 4 selections, one inline phantom), below |
| 4 labels drew into the gutter when scrolled | label clip = chip ∩ text area | `view::tests::invisible_labels_never_draw_into_the_gutter` (120 `scroll_x` positions) |
| 5 text area shorter than one row: row text and badges spilled past the popup | every row, badge, label and hint clip is intersected with the popup | `completion::tests::popup_content_stays_inside_a_view_shorter_than_one_row` (areas 8–55 points, UI 1 and 2) |
| 6 no ellipsis when exactly 3 bytes were cut | "fits" is `Cow::Borrowed` (an owned result is always a cut) | `panels::tests::long_matched_captions_end_in_an_ellipsis` (every width 400–1100) |
| 7 "[hex]" on undecoded image tabs, lost on restore/move, flicker | derived from the path matching `image_file_patterns` (cached per settings revision); "[hex]" only after a failed decode | `app_chrome::tests::image_tabs_say_hex_only_after_a_failed_decode` (first/broken/second opened in turn, then shown in turn) |
| 8 the failure note stayed after switching to a decoded image | cleared when the active document changes; re-shown when switching back to the damaged one | same test |
| "1 scopes" | "1 scope" | `html_ui::tests::scope_popup_counts_in_words` |
| image/hex views used the built-in palette | document scheme, else active scheme via the theme palette (mixes for muted ink, line and selection), else built-in | `app_chrome::tests::image_and_hex_views_use_the_active_scheme` (a user scheme's paper under a picture and a damaged image) |
| dirty dot overflowed 4–9-DIP tabs | right-aligned when it fits before the label end, else centred in the visible tab; left out when wider than the tab | `app_chrome::tests::dirty_dots_stay_inside_very_narrow_tabs` (120–220 tabs) |

## Left for round 4 (not merged yet)

- Lane B's Goto Symbol rows (kind, line, relative path, highlights).
- Lane C's multi-line find field.
- Lane A's region focus ring (super+F6), painted only while region mode is on.
- Lane C's multi-line find fields (grow to four rows, then scroll): capped height and the
  scroll cue need styling.
- Also noted: Goto Anything detail paths are clipped (minihtml has no ellipsis) — lane B's new
  rows replace them; a partly visible tab at the strip edge may show neither label nor close
  button when the visible part is too small for both; very narrow fitted tabs (under about
  52 DIPs) no longer have close buttons (middle click and ⌘W still close them).

## Goldens and fixtures (regenerated deliberately)

- `crates/vel-theme/tests/resource-colors.json` (`VEL_UPDATE_GOLDENS=1`): the new
  `sidebar_indent_guide` rule shifts the compiled rule indices after it (no colour of an
  existing element changed).
- `crates/velocity/fixtures/theme-scenes.json`: unchanged.

## Behaviour touched

- Tab close buttons exist only in a tab's visible outer half; narrow fitted tabs and slivers at
  the strip edges select on click instead of closing. A double click finishes on the tab its
  first click selected.
- The completion popup never covers the tab bar; in short views it shows fewer rows (at least
  one), then drops the details line, then the footer.
- A decoded image's tab label has no " [hex]"; byte and picture documents show no syntax or
  indentation item in the status bar.
- Selections over invisible-character labels and inline phantoms cover them.
- Build-error annotations are one line, shortened before the minimap.

## Performance

Against main `569cc9c` (exported with `git archive`, built in its own target directory with
this round's `render_scenes.rs`), isolated `HOME`/`SHELL`, temporary `--data-dir`,
`/usr/bin/time -l` instructions retired, base/new alternating:

| mode | 569cc9c | this branch | change |
| --- | --- | --- | --- |
| `--headless-smoke` (median of 5) | 308.3M | 308.8M | +0.15% |
| `--headless-smoke --carets 1000` (median of 5) | 4,063M | 4,059M | −0.1% |
| `--headless-view-features spikes/data/rust-10mb.rs` (median of 5) | 215.44G | 215.26G | −0.1% |

`--headless-view-features` is bimodal on both binaries (one run in five near 203–206G: its
background workers do less work when they finish early), so only medians of five are compared.

App-level scene cost (`--render-scenes DIR --bench-scene 400 --only … --appearance dark --scale
2`), steady-state instructions per `App::scene`, mean of 2 alternating runs:

| scenario | 569cc9c | this branch | change |
| --- | --- | --- | --- |
| editor | 1,802K | 1,807K | +0.3% |
| tabs (10 tabs, side bar) | 2,705K | 2,715K | +0.4% |
| groups_grid | 836K | 841K | +0.6% |
| sidebar (expanded tree, Git badges) | 465K | 473K | +1.6% |
| find (with tool tip) | 1,619K | 1,628K | +0.5% |
| replace | 1,573K | 1,586K | +0.8% |
| command_palette | 1,662K | 1,669K | +0.4% |
| goto_anything | 674K | 680K | +0.9% |

A first build measured editor +2.1 % and command palette +2.3 %, proportional to the visible
lines of code: the larger invisible-label code inside the per-glyph loop changed how that loop
was compiled. The label painting is now a separate `#[cold]`, never-inlined method, which brought
those back. The side bar's +1.6 % is the indent guides (one hairline per ancestor level of each
nested row) and the badge-first ordering.

### After the review fixes

`--bench-scene` against `569cc9c` (mean of 2 alternating runs): editor +0.4 %, tabs +0.9 %,
groups_grid +0.4 %, sidebar +1.0 %, find +0.7 %, replace +0.9 %, command palette +1.4 %,
goto_anything +0.1 %. The band computation is two small never-inlined helpers outside the row
loop, so rows without selections pay nothing.

Selection-heavy scenes (`--bench-scene 20`, three alternating runs each):

| scene | 569cc9c | this branch | change |
| --- | --- | --- | --- |
| `selections_3000` (12 KB line, one NBSP, 3,000 selections) | 6.60G instr/frame | 5.85G | −11 % |
| `phantom_selections` (300 rows × 4 selections, one inline phantom) | 48.6M | 47.6M | −2 % |

Wall time per frame on the 3,000-selection scene: 0.55–0.95 s on main, 0.49–0.79 s here (the
machine had a load average above 100 during these runs). `--headless-smoke`: +0.15 %. The
1,000-caret run was not measurable this time (the same binary varied by ±30 % under that load;
round 3 measured −0.1 % on a quiet machine, and the band path does not run for empty carets).

## Still needs a native (real window) check

- Tab clicks and double clicks with a real mouse at narrow widths; the native tab drag.
- Image view at 1x on an external display (frame and checker crispness), large images (2048-px
  thumbnails).
- Invisible-character chips with Increase Contrast.

## Images

`before/` and `after/` (same names; 1x images are native 1x renders, 2x crops scaled to 60–100 %):
`r3-01-narrow-tabs-light@2x`, `r3-02-narrow-tabs-ui2-dark@2x`, `r3-03-completion-short-dark@2x`,
`r3-04-completion-ui2-light@2x`, `r3-05-sidebar-narrow-dark@2x`, `r3-06-sidebar-guides-light@2x`,
`r3-07-sidebar-folders-only-dark@2x`, `r3-08-image-view-dark@2x`, `r3-09-image-view-light@2x`,
`r3-10-image-fallback-light@2x`, `r3-11-invisibles-dark@2x`, `r3-12-invisibles-light@2x`,
`r3-13-scope-popup-dark@2x`, `r3-14-scope-popup-light@2x`, `r3-15-build-error-dark@2x`,
`r3-16-goto-ellipsis-light@2x`, `r3-17-replace-300-ui2-dark@2x`, `r3-18-tabs-ui1.5-dark@2x`,
`r3-19-image-view-light@1x`, `r3-20-invisibles-dark@1x`. Round-1 and round-2 images are in git
history at `f5f0f8a` and `569cc9c`.
