# Round 6: CJK editor advances

Regular editor rows retain integer display columns and project them to points using
measured wide advances. The primary font and its macOS cascade supply one reference
advance per class (Han, hiragana, katakana, Hangul, full-width forms, emoji, flags,
and other wide symbols). Ratios to the primary cell scale with font size. Tabs and
synthetic gaps keep their column widths. Rulers stay on the unprojected grid.

`RowColumns` carries the byte/column prefixes and the sparse pixel adjustment.
Painting caches the adjustment with the line's units and walks it in order. A row
with no adjusted glyphs has no allocated projection. A wrap budget is still
specified in cells, but the break uses their point equivalent and the row's
measured advances. Option-drag inverts the projection at both ends and selects
logical columns across all intervening rows. The horizontal bound is still the
widest observed line or the caret's line, now measured in points. Large-file
policy, windowed long lines, and hex continue to use the fixed grid.

Fully proportional layout would change narrow text, programming ligature
geometry, and the mapping between existing column commands and the display.
That would replace more of the editor than is needed to remove the visible
tracking in wide runs.

## Checks

| Finding | Fix | Test |
| --- | --- | --- |
| Wide text had extra tracking | Use measured class widths in both the shaper and the row projection | Native font ink-box gaps at 1x and 2x versus natural shaping; caret positions at every grapheme boundary |
| Pointer and range geometry had to agree with ink | Invert advances at grapheme midpoints and preserve the selected visual segment at a wrap boundary | Caret paint, click, drag, double-click, Option-drag, multiline and partial cluster selection, UTF-16 bounds |
| Integer wrap width used the wrong physical budget | Wrap against measured width while preserving source tab phase and hanging indent | Wrap endpoints, indentation guides, tab arrows, whitespace dots, and fixed rulers |
| Scrolling could use columns that were wider than the ink | Use measured line ends and preserve pixel reading anchors | Widest-line clamp, horizontal reveal, and vertical overscroll |
| Decorations and popups needed the same projection | Use measured endpoints for fills, underlines, overlays, and anchors | Find highlight consumer, Replace All, word/bracket matches, spelling, inline diff, phantoms, minimap, popup placement, preedit, IME and accessibility adapters |
| Large-file, hex and export are separate paths | Retain the grid for large/long rows and hex; HTML/print already use text flow | Large-policy grid test and existing workspace tests |

The device-free regression matrix exercises Han, mixed Latin/Han and emoji rows.
The Find and Find in Files highlight consumer is shared; the new test supplies
ranges directly instead of automating the native Find in Files interaction.

## Visual evidence

Both `before/` and `after/` contain `r6-cjk-zh-dark@1x.png`,
`r6-cjk-zh-dark@2x.png`, `r6-cjk-zh-light@1x.png` and
`r6-cjk-zh-light@2x.png`. They were inspected at both scales. The baseline was
`33066de` with the same fixture added. The long paragraph gains several
characters per visual row, and the selection and tab arrow follow the tighter text.

Run `velocity --render-scenes DIR --only r6-cjk-zh --language zh-Hans --scale 1,2`
with fixture `HOME` and `SHELL`. The implementation session had no Metal adapter, so
`before/r6-cjk-zh-*` were made with the offscreen software text snapshot (the real
scene, shaper and glyph rasterizer, without theme textures and effects). The review
re-rendered `after/r6-cjk-zh-*` through the real Metal path. For a like-for-like Metal
comparison, `before/r6-cjk-sidebar-zh-dark@2x.png` (main `33066de`) and
`after/r6-cjk-sidebar-zh-dark@2x.png` (this round) show the round-5 `r5_sidebar` scene
with its Chinese paragraph: letter-spaced before, set solid after.

## Instructions

Five alternating process pairs, release builds, same fixture environment. The
`features` workload used the same `rust-10mb.rs` file for both builds.
`--headless-cjk` typed 200 Han characters and painted 240 scroll frames in a
10,000-line Chinese buffer with wrapping off; measured wrapping is tested separately.
The baseline CJK harness differs only in not installing wide metrics. The
counts are Darwin `proc_pid_rusage(RUSAGE_INFO_V4).ri_instructions`; BSD
`time -l` could not read `kern.clockrate` in this session.

| Scenario | Baseline median | Candidate median | Median paired delta | Ratio of medians |
| --- | ---: | ---: | ---: | ---: |
| Features | 202,584,589,387 | 204,775,198,061 | +0.23% | +1.08% |
| 1,000 carets (idle repeat) | 4,109,829,356 | 4,010,967,966 | -1.46% | -2.41% |
| Single-caret smoke | 306,696,803 | 307,236,787 | +0.22% | +0.18% |
| CJK typing and scroll | 1,154,521,312 | 1,185,852,966 | +2.84% | +2.71% |

The first 1,000-caret run overlapped workspace compilation. Its five pairs had
medians of 4,016,153,716 and 3,919,510,533 (-2.41%) and a median paired delta of
-7.60%. The table gives the nine-pair repeat after the gate and clippy had
finished. The paired statistic is within the ±2% band; the ratio of the
separate medians lies slightly outside it on the faster side.

The full review gate returned `GATE_OK` with 1,567 passing tests and no
failures. The focused `cjk_` run passed all 10 tests, and the final strict clippy
check passed.

Left for later: locale-specific punctuation break rules (a wrapped row can start
with "。" or "，"; Chinese line breaking keeps those at the end of the previous row)
and per-glyph advances for custom fonts whose wide classes are not uniform. An
interactive IME candidate window was not exercised.
