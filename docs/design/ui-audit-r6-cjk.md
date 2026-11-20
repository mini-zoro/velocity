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

## Round 26: punctuation at soft breaks

The wrap calculation uses the East Asian subset of [UAX #14](https://www.unicode.org/reports/tr14/).
The [Unicode 17 property files](https://www.unicode.org/Public/17.0.0/ucd/) supply
opening, closing, nonstarter, and conditional Japanese starter classes, including
small kana and the prolonged sound mark. Ambiguous horizontal ellipses attach
in CJK context; Latin-only lines retain the old break policy.

The row keeps closing punctuation if it fits; otherwise the break moves back
so the preceding grapheme can accompany it on the next row. Opening brackets
move with their following text. This is not optical hanging beyond
`wrap_width`. An impossibly narrow row falls back to a grapheme boundary to
make progress, even if a punctuation group cannot fit intact. A single
oversize grapheme still stays whole. Tabs, continuation indent, and measured
advances remain part of the budget, and incremental wrapping uses the same
boundary decisions.

| Check | Coverage |
| --- | --- |
| Chinese, Japanese and mixed text | Four widths with both measured and fixed-grid advances, plus an emergency narrow case |
| Hit testing and caret movement | Left/right and vertical movement, pointer midpoints and UTF-16 geometry at the adjusted breaks |
| ASCII | Exact absolute byte endpoints from 723d6ab on `fixtures/intel-project/tools.rs` at widths 9, 17 and 29 |
| Incremental updates | Full-versus-incremental randomized comparisons, now with brackets, small kana, and ellipses |

The round-26 scene was rebuilt and inspected at 1x and 2x in dark and light
appearances. The fourth wrapped row now starts with "路"; the "。" fits at the
end of the third. This session could not acquire a Metal adapter, so both sides
of this comparison use the offscreen text renderer. The round-25 Metal images
above are unchanged.

| 2x comparison | 723d6ab | Round 26 |
| --- | --- | --- |
| Dark | [Before](before/r26-cjk-zh-dark@2x.png) | [After](after/r26-cjk-zh-dark@2x.png) |
| Light | [Before](before/r26-cjk-zh-light@2x.png) | [After](after/r26-cjk-zh-light@2x.png) |

### Round 26 instructions

Five alternating process pairs on the same fixtures, with `723d6ab` as the
baseline. The full-width scan classifies boundaries only after a fixed-grid
row overflows; this avoids extra work on fitting rows edited at many carets.
All paired medians met the 2% budget.

| Scenario | Baseline median | Round 26 median | Median paired delta |
| --- | ---: | ---: | ---: |
| Features | 223,249,806,089 | 224,074,342,628 | +0.35% |
| 1,000 carets | 4,037,880,091 | 4,059,590,430 | +0.32% |
| Single-caret smoke | 308,652,889 | 309,199,423 | +0.13% |
| CJK typing and scroll | 1,201,435,519 | 1,197,677,826 | -0.30% |

Left for later: per-glyph advances for custom fonts whose wide classes are
not uniform, and an interactive IME candidate-window check.
