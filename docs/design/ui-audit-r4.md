# UI audit, round 4 (2026-11-20)

Starting point: main `b3e5707` (design round 3 merged; lane B round 21 — command line, single
instance, Goto ranking, index; lane C round 21 — everyday fixes, Find in Files, wrapped long
lines; lane A round 22 — crash-safe saving, packaged resources, accessibility). Scope: the
verifier's eight leftovers, then a design pass on the elements those lanes merged without one.
Same method and clean-room rules as `ui-audit-r1.md`: offscreen renders through the real
`App::scene` → Metal path with CoreText fonts, public Sublime Text documentation for
conventions, all values and artwork our own.

`after/`: this branch, 14 renders at 1x (dark and light). Eight show the states this round
touched — `r4-find-*`, `r4-replace-*` (the Find panel), `r4-goto_symbol-*` (Goto Symbol rows
before a project is indexed), `r4-narrow_tabs-*` (the fitted tab strip) — and six show the new
states: `r4-multiline_replace-*` and `r4_multiline_replace_ui2-*` (six pasted lines in the Find
field, UI 1 and 2), `r4-region_focus-*` (Cmd/Ctrl+F6 on a tab). There is no `before/` set this
round: none of the fixes changes a state those scenes capture (they live in image decoding,
label matching, caption fitting, the caret cache and a light scheme under the dark theme, each
covered by its test), and new states have no earlier counterpart.

## Findings and fixes

Each entry: scenario · problem — fix — test.

### Verifier leftovers

1. **`[hex]` flicker while an image is decoded again** · `image_view` — a settings change, a
   zoom or a reload made `ImagePreview::sync` drop the decoded picture (and the failure flag)
   before queueing the new decode, so for a frame or more the tab read "picture.png [hex]" and
   the sheet showed bytes. Fix: when the same file is queued again the last image and failure
   stay on screen until the new result arrives (keyed by path, settings revision, editor
   revision and the binary snapshot). Test:
   `app_chrome::tests::image_redecoding_keeps_the_picture_and_the_label` (a settings change,
   then three frames: picture present, `failed` clear, label `picture.png`, size still `6 × 4`).
2. **`//` rules in `image_file_patterns`** · `image_view` — the decoder matched the rules
   against the project folder but `Tab::is_image_file` (used for the label and the `[hex]`
   fallback) matched the file's own folder, so a `//art/*.png` rule decoded a picture whose tab
   said "[hex]". Fix: the tab remembers the project folder a frame supplies
   (`sync_image_file`), and the label's decision uses it; before any frame, with a
   project-relative rule and no folder yet, a picture is assumed. Tests:
   `app_chrome::tests::project_relative_image_patterns_follow_the_project_folder` (label and
   decode before and after a frame; a file outside the rule stays `[hex]`).
3. **A rename does not update the label** · `narrow_tabs` — the label understood which file it
   was for, but the decoded-picture decision could outlive the path it was made for. Fix: the
   remembered decision is keyed by `(settings revision, path, folder)`, so a renamed file is
   classified again under its new name (and a `.dat` name is bytes, not a picture). Test:
   `app_chrome::tests::renaming_a_picture_updates_its_label` (`old.png` → `new.png` keeps the
   picture and the `6 × 4` report; → `picture.dat` reads `[hex]`).
4. **A stale "W × H" in the status bar** · `image_view` — the status message was only replaced
   when a new decode finished, so a picture replaced on disk kept reporting its old size, and a
   report could outlive the document it described. Fix: the size is reported again for the new
   image, the failure note follows the decode state, and a message that belongs to an image is
   cleared when the document changes. Test:
   `app_chrome::tests::image_status_follows_the_picture_on_disk` (6 × 4 → replaced file →
   watcher reload → 9 × 3; another tab shows no `9 × 3`).
5. **The Goto ellipsis test swept three widths** · `goto_symbol` — with the test widened to
   400–1100 DIP it found two real defects: a matched caption cut three bytes long ("src") got
   no ellipsis at all (it was clipped whole, 0.5–0.6 DIP past its room), and the frozen form
   placed a stray marker where none fits. Fix: the cut is fitted within the room the ellipsis
   itself needs, the marker is drawn after the prefix as drawn, and one that cannot be seen is
   not drawn. Test: `panels::tests::long_matched_captions_end_in_an_ellipsis` (701 widths: runs
   tile the caption in order, every run ends inside its clip, every cut past the narrowest band
   ends in an ellipsis).
6. **An open image that changes on disk is not re-decoded** · `image_view` — the watcher
   rebuilt the binary snapshot but nothing asked for a new decode. Fix: the decoder's identity
   includes the binary snapshot, so a reload decodes again (and keeps the old picture until it
   does). Test: the same `image_status_follows_the_picture_on_disk` drives it through a real
   watcher and a rewritten PNG.
7. **3,000 carets on a 12 KB line** · `selections_3000` — every caret rebuilt the row's display
   units (`Layout::column`), 5.83 G instructions per frame. Fix: a per-row cache of the row's
   units, prefix widths and invisible gaps, built with lane C's projected-column window
   (`unit_window`/`units_in`/`invisible_gaps`), keyed by row and — for wrapped or folded rows —
   the measured segment's start and source column. `rect_at` and caret drawing share it through
   one `Option<RowColumns>`. Result: 60 M instructions and 4.4 ms per frame (dark, 3000 calls,
   load average 14), against 391 ms on main; the target was 16 ms. Lane C's projected-column
   tests (`wrapped_fold_tail_tabs_chips_and_clicks_use_projected_columns`,
   `folded_tail_uses_projected_invisible_columns_for_paint_and_hit_testing`) and the fold-tail
   band tests still pass. No new test: measured with `--bench-scene 300`.
8. **A selected tab's text on the dark theme with a light scheme** · `narrow_tabs` — the
   selected tab takes the scheme's background (`tint_index: 0`), while its label took the
   theme's near-white: 1.2:1. Fix: when the scheme's luminosity differs from the theme's, the
   selected label takes the scheme's own foreground (`var(--foreground)`), about 12:1. Test:
   `app_chrome::tests::selected_tabs_of_the_other_luminosity_keep_a_readable_label` (dark
   theme, light scheme: the label's ink is the scheme's `#20303a`, contrast ≥ 7:1; it fails
   without the theme rules).

### Merged elements that had no design pass

9. **Lane C: multi-line find and replace fields** (`r4-multiline_replace`,
   `r4-multiline_replace_ui2`) — fields grow to four hard rows (18 DIU each) and the panel
   grows with them; past four rows the field scrolls, but nothing said so: the caret moved to
   off-screen rows silently. Fix: a 2-DIP cue on the field's right edge, its thumb proportional
   to the visible fraction and its position to the first visible row, in the field's own ink
   (90% focused, 60% otherwise) — and through the theme's matched colour rather than a fixed
   blue. Caret, selection, marked-text and IME rects already share `visible_rows`, so they stay
   on the painted rows. Test:
   `panels::tests::multi_line_find_fields_grow_to_four_rows_then_scroll` (UI 1 and 2: 28 →
   82/118 DIP for six rows, four rows painted, cue inside the field; two rows: no cue).
10. **Lane B: Goto Symbol rows** (`goto_symbol`) — rows carried a badge letter from the debug
    name of an internal enum (`Function`, `Type`, `Other`) and every kind took the "navigation"
    chip, and local symbols take the kind labels of the other pickers. Fix: symbols map to the
    kinds the badges know (`function`, `type`, `namespace`, `variable`, `markup`, `symbol`);
    `T`/`N`/`V`/`M`/`O` chips now take their own colours. Project symbol rows keep their
    project-relative path on the details line and kind:line on the annotation. Test:
    `panels::intelligence_tests::project_symbol_rows_badge_their_kind` (the item's kind, the
    `f` letter in the function kind's ink).
11. **Lane A: the region focus ring** (`r4-region_focus`) — Cmd/Ctrl+F6 painted a 2-DIP ring in
    a fixed blue at the region's outer edge: on a tab it covered the neighbour's divider and the
    strip's edge, and it ignored every scheme. Fix: 1.5 DIP in the scheme's accent, inset 1.5
    DIP inside the region. Test:
    `app::accessibility_adapter::tests::region_focus_ring_only_exists_while_the_region_has_focus`
    (no ring without region focus, the scheme accent with it, none again on the editor).
12. **Lane A: recovered-save notices** — these arrive as resource reports and are shown through
    `panels.message`, which keeps them for five seconds like other messages, or until the next
    one. Checked in `session_restore`: a failed restore names what was recovered and keeps the
    text as an unsaved "Recovered <name>" document, so the notice is informational and the tab
    carries the state; five seconds is enough to read one line and the document remains.
    Nothing further changed here. The "Recovered" tab label and its message are covered by
    `session_restore`'s own tests.

## Held back

Chinese-UI work (`vel-i18n`, menus, palette) is lane B's; the Chinese layout pass
(`ui_language: "zh-Hans"` scenes) follows once it merges, with the review findings in
`specs/vel-ui-menus-settings-i18n.md` as its list.
