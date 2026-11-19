# UI audit, round 1 (2026-11-18)

Scope: every chrome surface of Velocity in both appearances, rendered offscreen through the real
`App::scene` → `vel-render` Metal path, compared against general Sublime Text 4 conventions
(public screenshots and docs only) and the macOS HIG. Clean-room: no Sublime package, theme,
icon or asset was read; all values and artwork here are our own.

## How the screens were rendered

`velocity --render-scenes <out-dir> [--scale 1|2|3[,…]] [--appearance dark|light|both]
[--only a,b]` (`crates/velocity/src/render_scenes.rs`). It builds real `App`/`Workspace` states
for a fixed scenario list around a synthetic project in the temporary directory
(`…/velocity-render-scenes/project` with `src/main.rs`, `src/render/…`, `README.md`, …), drives
them through the same command, panel and pointer paths as the native host (`run_named`,
`show_overlay`, `toggle_find_option`, mouse moves for hover and tool tips), waits until language,
index, completion and annotation workers have published, and renders with `Renderer::offscreen`
(a wgpu Metal adapter without a surface, `Bgra8Unorm` `RENDER_ATTACHMENT | COPY_SRC` texture,
`copy_texture_to_buffer`, PNG via the `png` crate). Fonts come from CoreText exactly as in the
app. No `NSApplication`, window, menu or Dock icon is created and focus is never taken. Run it
isolated, e.g.

```
env -i HOME=/tmp/h SHELL=/bin/sh PATH=/usr/bin:/bin TMPDIR=/tmp/ \
  target/release/velocity --render-scenes /tmp/scenes --scale 1,2
```

`VELOCITY_RENDER_DEBUG=1` additionally lists every painted text run (geometry checks).

Scenarios (each dark and light, 1x and 2x — 108 images): `editor` (Rust, gutter, fold,
bookmark, minimap, guides, selection + carets), `tabs` (overflow, dirty, preview, hovered tab),
`tabs_split_view`, `groups_columns`, `groups_grid`, `sidebar` (open files, expanded folders,
selected/hovered rows, synthetic in-memory Git badges, long name), `find` (hover + tool tip),
`find_options_on` (regex, case, whole word, in selection on, multi-line selection),
`incremental_find`, `find_regex_error`, `replace`, `find_in_files`, `command_palette`,
`goto_anything`, `goto_symbol`, `scheme_picker`, `theme_picker`, `completion`,
`hover_definition`, `phantoms_popup_build_errors` (phantoms, annotation, inline build error,
minihtml popup), `status_bar` (encoding, line endings, Git), `build_output`, `settings_window`
(two groups, default + user), `distraction_free`, `empty_window`, `narrow_640x480`,
`large_1920x1200`.

Approximations: Git status is injected in memory (`GitUi::insert_snapshot`) instead of creating a
repository; the build output panel is filled with a synthetic transcript instead of running a
build (no login-shell capture). The status-bar encoding / line-ending menus are native `NSMenu`s
and the console does not exist in Velocity (no Python), so neither is rendered.

The `before/` images were rendered from `d80d019` plus only the renderer tool (offscreen
`vel-render` path, `render_scenes.rs`, its CLI hook, `GitUi::insert_snapshot`); `after/` from
this branch. Crops: see the list at the end.

## Findings, ranked by user impact

Each entry: scenario · region — problem — why it matters — fix (status).

### P0 — broken, unreadable or not understandable

1. **Find panels' controls not understandable** (raised by the user) · `find`, `replace`,
   `find_in_files`, `incremental_find` — six toggles stretched into equal sixths of the panel
   with duplicated icon + caption (`.* .*`, `Aa Aa`, `Word`, `Selection`, `Preserve`), no names
   or shortcuts anywhere, on/off barely distinguishable (and a later base rule overrode the "on"
   tint), hover identical to normal, buttons with left-aligned labels squeezed against the status
   bar and the message line running under them; every option shown in every panel (05–09).
   — Redesign, paint/layout/hit-test only (find logic untouched):
   - icon-only 28×24 toggles directly under the input they modify (left-aligned with it);
     **on = solid accent with an inverse glyph**, off = faint fill, hover = stronger wash;
   - **tool tips** with a plain name, the current state and the real shortcut from the keymap,
     e.g. "Regular expression: off (⌥⌘R)", "Match case: on (⌥⌘C)", "Whole word (⌥⌘W)",
     "Wrap around", "In selection", "Preserve case", "Find (⌘G)" — hit-tested on the drawn
     rects (reusing the review tool-tip path, now sized to its text and flipped above the
     pointer at the window bottom);
   - only options that apply to the panel, decided by the panel alone (round 2): Find and
     Incremental Find show regex, case, whole word, wrap and in selection; Replace adds preserve
     case; Find in Files shows regex, case, whole word, preserve case (wrap and in-selection do
     not apply there). Regex, case and whole word have shortcuts (⌥⌘R/⌥⌘C/⌥⌘W); wrap, in
     selection and preserve case have none (the public macOS shortcut list has none either), so
     they are always shown where they apply;
   - buttons right-aligned with the input, centred labels, the Enter action ("Find") tinted as
     primary;
   - a flow layout (round 2): one row when it fits; otherwise the buttons move to their own row
     (and toggles/buttons wrap further in very narrow panels), the panel grows by those rows, and
     the status message (match counts, errors) keeps a slot of at least 96 DIPs (or the full
     width) — never overlapped, never zero-width; short windows shrink the whole panel instead of
     drawing controls outside it;
   - plain close glyph with a hover wash; compact panel heights (74/108/142 DIPs at normal
     widths, instead of 96/130/164).
   **Fixed** (`panels.rs` `layout`/`sync_widgets`/`paint`/`review_hover`; `panels/details.rs`
   `find_toggle_options`, `find_panel_height`, `find_controls_at`, `panel_message_clip`,
   `panel_tooltip`; `review_ui.rs` tool tip; `generate.py`).
2. **Overlays have no background** · `command_palette`, `goto_*`, pickers — the quick panel's root
   element (`quick_panel`) has no surface, so rows, kind badges and the footer are drawn straight
   over the code; the palette is unreadable (03, 04). — Paint the documented `overlay_control`
   container (variant per overlay) with a floating surface, hairline border and shadow; size the
   overlay to whole rows so the painted panel, the hit area and the footer agree. **Fixed**
   (`Panels::paint`, `Panels::layout`).
3. **Popups have no edge** · `phantoms_popup_build_errors`, `hover_definition`, `completion` — the
   minihtml body paints the scheme background, identical to the editor, and no border or shadow
   exists; popup text appears to float over code, the definition popup's default `h3` margins
   made it taller than the view, popup text touched its edge (13–15). — Floating style (border +
   shadow) for `popup_control`, `tool_tip_control`, `overlay_control`; the border is repainted
   above the html content; default popup inset; compact definition-popup markup; tool-tip labels
   no longer inherit the container shadow (which ghosted the text). **Fixed** (`generate.py`,
   `html_ui.rs`, `Panels::paint_hover`, `vel-ui` `ThemeUi::border`).
4. **Flat, structureless chrome** · every scenario — dark: side bar, tab strip, status bar,
   panels and editor are all exactly `#14232e`; light: cool `#f5f7f8` chrome next to the warm
   `#f7f5ed` editor. The active tab is a saturated blue block (`#3c6077`) that does not connect to
   its sheet, inactive tabs float as loose labels, nothing separates side bar, tabs, status bar or
   panels from the text (01, 02, 10). — Chrome one step darker than the editor (light: 6%,
   dark: 25%), floating panels lighter; one-DIP hairlines (nine-sliced edge textures) between
   regions; the selected tab takes the live editor background (`tint_index` 0), the focused
   group's tab gets a two-DIP accent bar; inactive labels muted, short dividers. **Fixed**
   (`generate.py`).
5. **Tab-bar buttons distorted and clipped** · `tabs`, `groups_*`, `narrow_640x480` — the new-tab
   `+` and scroll arrows are 16-DIP textures stretched to 16×32 (tall glyphs); the `+` touches and
   is cut by the window edge; the arrows crowd the last tab (11). — Elements sized by their
   content margins (20×20), centred in the bar with 4-DIP gaps, glyphs at intrinsic size, hover
   state; the drawn element is the click target. **Fixed** (`app_chrome.rs`, `ThemeUi::icon_surface`).
6. **Unthemed black strip** · `distraction_free` — with the status bar hidden the workbench still
   reserved 24 DIPs for it, leaving the renderer's clear colour at the bottom (17). — Reserve the
   status bar only when shown (side bar too). **Fixed** (`Panels::layout`).

### P1 — wrong, misleading or inconsistent

7. **UI font fallback through Times New Roman** · all scenes (CoreText log: `".SFNS-Regular" …
   will get TimesNewRomanPSMT`) — missing-glyph fallback for UI text started from
   `CTFontCreateWithName(".SFNS-Regular")`, i.e. Times' cascade, so CJK file names and symbols in
   chrome fell back to serif faces. — Create the UI font with `CTFontCreateUIFontForLanguage` for
   private system names. **Fixed** (`vel-platform` `fonts.rs` `system_face`; the log line is gone).
8. **Build output panel always empty** · `build_output` — the output editor is read-only and
   `apply_edits` ignores read-only editors, so mirrored output never reached the view. — Mirror
   with `read_only` lifted for that edit. **Fixed** (`Panels::sync_output`; behaviour touch).
9. **Definition hover through panels** · `find` — moving the pointer over the find panel opened a
   "Definitions" popup for the code hidden under the panel. — Hover only inside the text area.
   **Fixed** (`app_mouse.rs`, `Panels::leave_hover`).
10. **File icon reads as a missing-glyph box; icons blurry** · tabs, side bar, Goto — hollow
    rectangle file icon; all 16-DIP icons scaled to 14 or 12 DIPs (soft at 1x and 2x); unclear
    branch icon. — Page-with-fold file icon and Git branch icon, one-DIP strokes snapped per
    density, icons drawn at 16 DIPs on device pixels. **Fixed** (`generate.py`, call sites).
11. **Hard-clipped long names** · `tabs`, `sidebar`, `goto_anything`, `settings_window` — labels
    cut mid-glyph (`an_unusually_long_moa`). — `ThemeUi::text_fit` ends labels with `…` before the
    close button / badge. **Fixed**.
12. **Git badges unstyled and misaligned** · `sidebar` — the badge element was resolved without its
    status attribute, so theme colours never applied; letters at a fixed x ran into the edge and
    overlapped long names (12). — Status attribute via `Tree::style_child`, right-aligned, label
    ellipsized before it. **Fixed** (`Panels::paint_badge`).
13. **Active tab scrolled out of view** · `tabs`, `groups_grid` — after opening files into a full
    strip the focused tab (and other groups' selected tabs) stayed off-screen. — Scroll a group's
    strip once when its focused sheet changes. **Fixed** (`App::scene`; behaviour touch).
14. **Hard-coded heavy dividers** · `groups_*`, `settings_window` — 2-DIP `[0.28,0.30,0.35]`
    dividers ignore the theme (heavy in light mode); Split View sheets had no divider (16).
    — Theme `grid_layout_control` border (1 DIP) for both. **Fixed** (`App::scene`).
15. **Invalid regex** · `find_regex_error` — error shown twice (panel and status bar) as a
    whitespace-mangled multi-line string; no error state on the input (09). — `invalid`
    attribute → red input border and red one-line message; the status bar keeps the position
    while a find panel shows the message (format click areas follow). **Fixed**.
16. **Tab close buttons** · `tabs` — 12×12 target (below HIG), no hover state, full weight on
    every tab; dirty tabs marked twice (`•` in the label and next to the close glyph). — 20×20
    target with hover wash, shown on selected/hovered tabs, a dirty dot replaces it until hovered,
    no label bullet while close buttons are shown. **Fixed**.
17. **Buttons' states** · panels — normal and hover were both `raised`. — Hover/pressed washes,
    opaque `selected` row colour. **Fixed** (`generate.py`).

### P2 — polish

18. Inputs had no border and a faint focus wash; light wells barely separated from the panel.
    — Hairline border, accent border when focused, accent selection/caret. **Fixed**.
19. Light secondary text `#627984` was 3.94:1 on `raised` and 4.26:1 on chrome (below AA). — Light
    `muted` `#51626b`: 5.08:1 on chrome, 6.13:1 on floating panels; dark `#94a6ae`: 6.99:1 / 5.51:1;
    selected rows 8.57:1 (dark) / 7.61:1 (light). **Fixed**.
20. Quick panel: matched characters overprinted on the unmatched caption (double anti-aliasing),
    the Goto file icon drawn on top of a kind badge, single-line rows top-aligned, key hints read
    `Cmd+K, Cmd+/` instead of `⌘K ⌘/`, kind letters off-centre, a stray kind letter leaking below
    the panel. **Fixed** (display only; `Keymap::hint` is unchanged).
21. Kind badges used one neutral fill for every kind. — Per-kind tints. **Fixed**.
22. Minimap viewport opacity 0.08 was barely visible. — 0.12, 0.2 on hover. **Fixed**.
23. Status bar: Git text without an icon; all items at full foreground. — Branch icon, muted text.
    **Fixed**.
24. `narrow_640x480`, `replace_scaled_narrow`, `find_in_files_short`: find buttons collided
    with the toggles (round 1 still overlapped at ≤582 px with a side bar, more at larger UI
    scales), and the row was drawn outside short panels (18–20). — Flow layout and panel
    shrinking (see 1), covered by a sweep test. **Fixed** (round 2).
25. Bookmark glyph touched the gutter's left edge. — 3-DIP inset. **Fixed** (`view.rs`).

### Round 2 — independent review findings

| # | Finding | Fix | Proof |
| --- | --- | --- | --- |
| H1 | "In selection" vanished after typing (visibility followed the live selection) and could then not be turned on; wrap / in-selection / preserve case have no shortcuts | Toggle set depends only on the panel (`find_toggle_options`); no bindings invented (none documented) | `in_selection_toggle_stays_clickable_after_typing` |
| M1 | Toggles and buttons overlapped in narrow panels / large UI scale; covered toggle unclickable | Flow layout: buttons to their own row, further wrapping when needed | `find_panel_layout_sweep_never_overlaps_and_keeps_controls_inside` (4 panels × side bar on/off × UI 1/1.5/2 × widths 272–1600 step 16 × heights 120–800 = 14,112 layouts since round 3; no overlap, inside panel, panel inside window above the status bar, every control hit where drawn) |
| M2 | Messages/regex errors invisible when the message slot shrank to 0 | Message slot ≥ 96 DIPs or full width (own row if needed) | same sweep asserts the width |
| M3 | Clicking the palette footer ran a hidden row | Clicks below the list select nothing; list height is whole rows ending at the footer | `overlay_footer_selects_nothing_and_overlay_fits_short_windows` |
| M4 | Build output stalled 0.2–0.8 s per frame on large output (trim → full re-insert) | Trimming deletes only the dropped prefix (`trim_output_prefix`); appends are incremental and capped at 128 KB per frame; `busy` keeps frames coming until caught up; per-frame line count of the raw output removed | `trimmed_build_output_is_mirrored_exactly_and_incrementally` (12 MB through 8 MB retention, mirror byte-exact); ignored timing test: slowest frame 9.8 ms for 14 MB under load ≈ 40 |
| M5 | Short windows drew the action row outside the panel (over the status bar) | Panel height = natural height clamped to the window (editor keeps its tab row); inputs/controls shrink vertically to fit | sweep heights 120–800; `19-find-in-files-1000x200` |
| L1 | Tool tip stale after ⌥⌘C, stuck after Esc | Re-derived each frame when control/options/panel change; cleared when no control is under the pointer | `find_tooltip_follows_options_and_panel_close` |
| L2 | `show_tab_close_buttons: false` kept an invisible 20×20 close target and a 30 px label gap | Close target and gap only when the button is shown | `hidden_close_buttons_have_no_target_and_keep_the_dirty_marker` |
| L3 | Tabs < 28 px lost the dirty marker | Tabs without a close button draw the bullet outside the truncated label | `narrow_fitted_tabs_keep_the_dirty_marker` |
| L4 | Style-cache id collisions (overlay/review menu, kind labels/tool tip, rows/footer, tab ids ≥ 256 sheets) | `panels::StyleId` namespaces (disjoint ranges above 2^56) for all chrome painted outside the retained tree, including the pre-existing tab ids | code review |
| L5 | `App::scene` heavier | Label fitting cached per (text, width, font) with an unmeasured fast path; per-status Git badge style; find layout and tool-tip derivation cached; FNV texture lookups; palette rows without details html | App-level scene benchmark below |
| L6 | Palette extended below windows ≲150 px | Overlay moves up, then shrinks, to stay inside the window | same overlay test (heights 100–300, UI 1 and 2) |
| Design | Palette rows showed the raw command name and a "c" badge on every row | Command lists (palette, syntax, snippet choices) show caption + key binding in compact 32-DIP rows; kind badges only in Goto/symbol lists (files keep their icon); details only where the item has them (paths). Lists without any details use compact rows too | `03-command-palette-dark` |

### Round 3 — verification findings

| # | Finding | Fix | Proof |
| --- | --- | --- | --- |
| High | Command palette / Goto Anything opened scrolled by one row (selected row 0 under the input, Enter ran a hidden row): the first frame had no ranking, a 1-point list viewport, and `set_items` revealed row 0 against it | `Scroll::reveal` defers the bottom-edge reveal while the viewport is smaller than a row; `Scroll::set_viewport` (used by every list layout) completes it | `command_palette_first_frames_keep_the_selected_row_visible`, `goto_anything_first_frames_keep_the_selected_row_visible` (key binding → frame-by-frame poll + scene through ranking, UI 1 and 2, 300-file project; both fail without the fix: row at y=69/57 vs list top 100); renderer `--first-frames N` |
| Medium | Wrapped build output re-laid out everything about every 768 KB (250 ms at 6 MB) | Not the append path: the output view's large-document policy changed every 10,000 lines (minimap sampling) and at long lines, and each policy change rebuilt the language and invalidated the whole layout. The output view pins that policy (`large_file.enable_*`, `spell_check: false`); appends and prefix trims then use the existing incremental `Layout::change` path. `layout.rs` untouched | ignored timing test, 20 MB with wrap on, with a restart half way, with a 20 KB line every 3 MB, main-thread CPU per frame: slowest 0.5 ms (no wrap), 5.3 ms (wrap), 12.0 ms (wrap + restart, the new build's language start); correctness test covers wrap on/off and restart |
| Low | M2: message slot shorter than its text at heights 200–250 with UI ≥ 1.5 | Slot < 16 DIPs tall → the status bar shows the message (`message_in_find_panel`) | `find_message_moves_to_the_status_bar_when_the_panel_slot_is_too_short`; sweep asserts it |
| Low | L4: result rows `0xB000+row`, tab bar `0xD000+group`, `0xDFFE/0xDFFF`, review menu `0xF000`, hover popup `9001` | All moved into `StyleId` ranges | code |
| Low | L6: UI 2 at the 200-point minimum height left < 1 row in the palette | Tight overlay (no footer hint, 28-point input) when the window cannot hold input + row + footer | overlay test: heights 200–300 at UI 1/1.5/2 keep ≥ 1 whole row |
| Low | Tabs clipped by the strip edges kept close targets outside the visible tab / under the scroll and new-tab buttons, and lost the dirty dot | Close target only where it is entirely inside the visible tab; labels and the dirty bullet end inside the visible part | `clipped_tabs_keep_targets_and_dirty_marks_inside_the_visible_strip` (24 dirty tabs, scroll sweep) |
| Low (main) | At 300 px with side bar and UI 2 the Find input collapsed to 1 px under the close button | Label column shrinks (80 → 64 DIPs), then labels are dropped, so inputs keep ≥ 120 DIPs or the whole panel width | sweep now starts at 272 px and asserts input widths; `17-replace-300px-ui2-dark` |

### Not fixed (with reason)

- Gutter fold buttons: not drawn in the renders even with `fade_fold_buttons: false` (fold
  analysis did not publish in the headless scene), so their 10×10 drawing from 16-DIP chevrons
  was not reviewed visually — needs a native check.
- Toggles are not keyboard-focusable (no focus ring); keyboard users use the shortcuts listed in
  the tool tips.
- The side bar starts at y = 30 while the tab strip is 32 high; the constant is shared by hit
  testing and tests, and the offset is invisible with the new hairlines.
- Inline annotations (build errors) are clipped at the minimap without an ellipsis (minihtml).
- A partially scrolled tab next to the scroll buttons shows a sliver of its icon.
- Find in Files keeps the field order Find / Replace / Where (Sublime uses Find / Where / Replace);
  changing it touches focus order and input routing.
- Indent guides and the light `line_highlight` are very faint (scheme `guide` = `line`); left to a
  color-scheme pass.

## Goldens and fixtures (regenerated deliberately)

- `crates/velocity/fixtures/theme-scenes.json` (`VEL_UPDATE_GOLDENS=1`): no text content changed.
  Goto scenes: the overlay gains its container (fill, shadow, border box), rows use the new
  geometry (18-DIP badge, caption at x+36) and the matched characters are separate runs
  (`ren` + `der_document.rs` instead of overprinted `r`,`e`,`n`). Phantom/popup scenes: the
  popup gains a shadow and a border box repainted above the html, and an inset; one layer fewer
  because the tool-tip/popup text no longer carries a shadow run. Colours follow the new
  variables (chrome `#0f1a22`/`#e8e6df`, muted text).
- `crates/vel-theme/tests/resource-colors.json` (frozen compiled colours of the shipped theme):
  regenerated; the test gained the same `VEL_UPDATE_GOLDENS` switch as the other goldens.
- `crates/vel-theme/tests/compatibility.rs`: the close-button visibility check now reads layer 1
  (the glyph; layer 0 is the hover wash) on a selected tab. `app_lane_b_round18_tests.rs`: the
  dirty-tab colour check finds the dirty tab's label by geometry instead of the `•` that the tab
  caption no longer carries.

- Round 2: `resource-colors.json` refreshed again for the compacted theme (redundant selected-label
  rules merged, kind-label font in one rule, wash layers only on clickable icons);
  `theme-scenes.json` unchanged. `message_clip_stops_before_every_panels_first_button_at_each_scale`
  now also accepts buttons wrapped onto the next row.

## Behaviour touched

- `Panels::sync_output` lifts `read_only` while mirroring build output (the panel was empty).
- `App::scene` scrolls a group's tab strip once when its focused sheet changes.
- Find panels: toggle/button/close rectangles moved; wrap and in-selection are not shown in Find
  in Files, preserve case not in Find (hit testing uses the drawn rectangles; all
  `toggle_find_option` commands still work); panel heights 74/108/142 DIPs at normal widths,
  more when controls wrap, less (shrunk) in short windows.
- Build output is mirrored incrementally (≤ 128 KB per frame); the view catches up over frames.
- Command palette rows are 32 DIPs (caption + key binding); the palette shows more rows.
- With `show_tab_close_buttons: false` no invisible close target remains; tabs clipped at the
  strip edges have no close target outside their visible part (round 3).
- The build output view pins its large-document feature policy (minimap sampling, viewport
  highlighting, spelling/diff gates) so megabytes of output never rebuild its language or
  layout; it has no minimap, spelling or diff anyway (round 3).
- Very narrow find panels drop the input labels before the inputs collapse (round 3).
- A find message whose panel slot is too short appears in the status bar (round 3). The overlay's rectangle fits its rows (clicks below the last row are
  outside the overlay, as drawn). Tab close targets are 20×20 (were 12×12); tab-bar buttons
  20×20 centred.
- Definition hover is only requested inside the text area.
- Layout reserves the status bar only when it is visible.
- Status text no longer repeats a find-panel message.

## Performance

All against `d80d019` (exported with `git archive`, built in its own target dir), isolated
`HOME`/`SHELL`, temporary `--data-dir`, `/usr/bin/time -l` instructions retired.

| mode | d80d019 | this branch | change |
| --- | --- | --- | --- |
| `--headless-smoke` (median of 5, round 3) | 428.6M | 432.6M | +0.9% (one-time theme compile) |
| `--headless-smoke --carets 1000` (median of 5, round 3) | 10,618M | 10,608M | −0.1% |
| `--headless-view-features spikes/data/rust-10mb.rs` (median of 3 pairs, round 3) | 232.55G | 232.63G | +0.03% |

Round 1 reported `--headless-view-features` without its fixture: the default path resolves
`spikes/data` next to each build, which existed in neither tree, so both runs only measured
start-up. Round 2 passes the 10 MB fixture explicitly. Theme start-up got cheaper in round 2
(372 rules, 2,561 JSON values vs main's 330 / 2,586; 49.6 KB vs 70.5 KB).

App-level scene cost (`velocity --render-scenes DIR --bench-scene 400 --only … --appearance
dark --scale 2`, new in round 2): steady-state instructions per `App::scene` (CPU scene
construction for one frame, after a 20-frame warm-up), same tool built on `d80d019`:

| scenario (round 3) | d80d019 | this branch | change |
| --- | --- | --- | --- |
| editor | 1,819K | 1,828K | +0.5% |
| tabs (10 tabs, side bar) | 3,954K | 3,976K | +0.5% |
| groups_grid | 841K | 875K | +4.1% |
| sidebar (expanded tree, Git badges) | 456K | 486K | +6.6% |
| find (with tool tip) | 1,589K | 1,628K | +2.5% |
| replace | 1,538K | 1,569K | +2.0% |
| command_palette | 1,994K | 1,672K | −16.2% |
| goto_anything | 677K | 671K | −0.9% |

The remaining +30K instructions (≈ 3 µs) in side-bar scenes come from the extra theme layers
(hairline edges, tab dividers, per-row icon snapping and badge measuring); the reviewer's +28% /
+21% are gone.

## Still needs a native (real window) check

- Native title bar colour against the tab strip (`surface`) in both appearances.
- Hover transitions and tool-tip timing with a real mouse; tab close / tab-bar button hover.
- 1x external displays for hairline and icon crispness; 3x assets on any 3x display.
- CJK and emoji in UI labels with the corrected fallback cascade.
- Gutter fold buttons; status-bar encoding / line-ending menus (native `NSMenu`).
- Increase Contrast / Reduce Motion with the new translucent states.

## Images

Round 2 replaced these images to stay within the 40-image budget; the round-1 set is in git
history (`git show f5f0f8a:docs/design/after/01-window-tabs-dark.png`, and likewise for every
name below). The current `before/`/`after/` images belong to `ui-audit-r2.md`.

Round-1 names, `before/` and `after/` (1x images are native 1x renders, 2x crops are scaled to 60–70%):
`01-window-tabs-dark`, `02-window-tabs-light`, `03-command-palette-dark`,
`04-goto-anything-light`, `05-find-panels-dark@2x`, `06-find-panels-light@2x`,
`07-find-panels-dark@1x`, `08-find-panels-light@1x` (each stacks find with hover + tool tip,
find with options on, incremental find, replace, find in files), `09-regex-error-dark`,
`10-tab-bar-sidebar-dark`, `11-tab-bar-overflow-light`, `12-sidebar-git-dark`, `13-popups-dark`,
`14-hover-definition-light`, `15-completion-light`, `16-groups-grid-light`,
`17-replace-300px-ui2-dark` (round 3: input no longer collapses), `18-narrow-640x480-light`, `19-find-in-files-1000x200-dark`
(short window), `20-replace-ui1.5-760x560-light` (UI scale 1.5, narrow).
