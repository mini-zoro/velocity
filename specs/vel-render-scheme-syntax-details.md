# Spec: mouse, word wrap and fonts, minimap and distraction free, color schemes, syntax engine details

Status: P2, round 17 (these are bugs a daily user hits; `vel-settings-coverage.md` and sections 1-3 of
`vel-snippets-builds-layouts-details.md` move to round 18, sections 4-5 of the latter to round 19). Lane A: sections
A, D (except D4/D5/D7), E, F1-F3. Lane B: sections B, C, D4/D5/D7, A3/A4, F4.

Background: a reviewer detail audit (2026-11-16, main at 66dc521) compared these areas with Sublime Text 4 and
confirmed most items with throwaway headless probes (now deleted). Line numbers are approximate and from 66dc521;
lane B's round 16 moves `view.rs` lines. Items marked *(verify)* rest on the auditor's memory of Sublime: confirm them in
the documentation or changelog first and cite the source in a code comment; if a behaviour cannot be confirmed, keep
today's behaviour and say so. Sources: https://www.sublimetext.com/docs/ (color_schemes, minihtml, themes, syntax,
distraction_free, indentation, ligatures, settings, column_selection), https://docs.sublimetext.io, the ST4 changelog.
Clean-room: implement the described behaviour with our own code, resources and wording; do not download, read or copy
Sublime's Default package, themes or color schemes.

## Lane A

### A. Mouse (`vel-config/src/ui.rs`, `crates/velocity/src/mouse_select.rs`, `app.rs` mouse matching, `vel-commands`)

1. **`.sublime-mousemap` compatibility:** `MouseBinding.button` (`ui.rs` ~70) is a number, so a user mousemap
   written as `"button": "button1"` fails as a whole file. Accept `"button1"`…`"button16"`, `"scroll_up"` /
   `"scroll_down"` and numbers; skip one malformed binding with a warning instead of the whole file; add a
   `drag_select` command (`by`: words/lines, `extend`, `additive`, `subtractive`) mapped onto `mouse_select::DragSelect`;
   look up scroll bindings in the scroll handler. Keep today's built-in mouse behaviour when the mousemap is empty.
2. **Shift + double/triple click extends by word/line** from the existing anchor *(verify)* (`DragSelect::begin`
   ~105 keeps the anchor only for character clicks today).

### D. Color schemes (`crates/vel-color-scheme`, the scheme loader in `crates/velocity/src/workspace.rs` ~783-809)

1. **One bad value must not reject the whole scheme** (`lib.rs` ~312-329, 376-380, 397): today every string global is
   parsed as a color, so the documented `selection_corner_style: "cut"`, `block_caret_corner_style: "round"` (the
   documented default), `selection_corner_radius: "2"`, `line_diff_width: "2"`, a tmTheme `shadowWidth "6"`, an
   unknown `font_style`, a `foreground_adjust` without `background`, one bad color, an unknown variable or a non-color
   variable each make the scheme fail and fall back to the built-in one. Add a type table for non-color globals
   (corner styles as an enum, widths/radii as numbers or numeric strings); report other errors per key / per rule as
   warnings, skip that item and keep the rest; only unparsable JSON fails the file.
2. **Missing globals get derived defaults, not the opaque foreground** (`view.rs` `color` ~1886 returns the
   foreground): a classic tmTheme without `gutter` / `gutter_foreground` paints the gutter in the text color with
   invisible line numbers; a missing `line_highlight` covers the caret line. Fill the compiled scheme's globals at
   compile time so that drawing code needs no change: `invisibles` = foreground at 0.35 alpha (documented); `gutter`
   = background; `gutter_foreground`, `line_highlight`, `rulers`, `guide` / `active_guide` / `stack_guide`, `find_highlight`,
   `misspelling`, `line_diff_*` derived from foreground/background (our own values, say so in a comment);
   `selection_border`, `minimap_border`, `fold_marker` and other borders transparent (not drawn) when missing. Add a
   table test with a scheme that only defines background/foreground/selection.
3. **`blend()` / `blenda()` percentage direction** (`color.rs` ~109-125, 240-253): Sublime documents that it follows
   the CSS Color Module Level 4 draft of 5 July 2016 (minihtml.html), which says "0% produces the specified <color>
   and 100% produces the base color" (https://www.w3.org/TR/2016/WD-css-color-4-20160705/). Today the percentage
   weights the specified color (`#000` + `blend(#fff 25%)` gives 64 instead of 191). Fix both adjusters and update our
   own resources written for the old meaning (`Theme - Velocity/Velocity.sublime-theme` ~2722 `blend(black 8%)`, the
   theme generator `generate.py` ~162, color schemes, tests) so that what users see today does not change.
4. **`.hidden-tmTheme`** (build 4107) loads like `.tmTheme` and is hidden from the scheme picker (`workspace.rs` ~783).

### E. Syntax engine (`crates/vel-syntax`, `crates/vel-highlight`, `crates/vel-syntax/src/indent.rs`)

1. **`extends`: anonymous contexts of the child replace the parent's** (`yaml_load.rs` `ContextNamer` ~849 names them
   `#anon_<ctx>_<index>`, unique per file only; `syntax_set.rs` `resolve_extends` ~1080-1140 merges by name). A
   Markdown child adding a front-matter rule with `embed_scope` breaks the parent's anonymous contexts. Make the
   names unique per syntax file (file index or path hash) or rename the child's anonymous contexts and their
   references before merging; same for `embed_scope` wrappers and `with_prototype`. Load-time only.
2. **Stack depth limit:** at 100 frames (`parser.rs` ~1342-1350) every push/branch/embed returns nothing and the token
   is skipped, so the closing tokens never match and the rest of the file loses highlighting (99 nested JSON arrays;
   ~35 nested JavaScript brackets). Raise the limit (≥ 1,000), and at the limit refuse only the push while pops,
   escapes and non-pushing rules still match; record a warning. No hot-path cost.
3. **Unknown context references:** `push`/`set` to a missing context fail at match time (`r.resolve(..)?`, `parser.rs`
   ~425/530/2525-2828), the error bubbles to the highlighter (`vel-highlight/src/lib.rs` ~527) and highlighting stops
   for the rest of the file (`complete = true` ~1200). Collect unresolved references at link time
   (`syntax_set.rs` ~1451) into `warnings()` and point them at an empty context. A missing `include` stays a warning.
4. **Load-time panic:** `push: [[]]` panics in `yaml_load.rs` ~555 (`v[0].as_vec().unwrap()[0]`) and
   `SyntaxSet::load_roots` (`cache.rs` ~60) does not isolate failures, so one broken user syntax can crash loading.
   Replace unchecked indexing in `parse_pushargs` / `parse_embed_op` and friends with errors; isolate failures per
   file (a broken file is reported and skipped). Add a fuzz-style table of malformed syntax snippets.
5. **`clear_scopes` with `pop: N` + `set`** (`parser.rs` ~2660-2700, `unwind_frames_below_top` ~2179-2204): the
   restore of cleared scopes is emitted before the lower frames pop, so with `c` having `clear_scopes: 2` and
   `pop: 2, set: e` from `a > b > c` the scope stack is `source.cl2 meta.main meta.c meta.e` instead of
   `meta.main meta.a meta.e`. Unwind frame by frame in the same order for `Pop(N)` and `set`. Scope-stack model test.
6. **Authoring warnings:** unknown `{{variables}}` expand to "" silently, invalid regexes never match silently, keys
   with wrong value types are ignored (`yaml_load.rs` ~525-555, 722-745). Report them through `warnings()` (a regex
   syntax check without full compilation is enough). Load-time only; keep the lazy compilation.
7. **Reindent special cases** (`indent.rs` ~143-162, ~210-240, ~350-365 hard-code SQL, PHP, shell, Python rules): the
   5 failing assertions of the 126,600 in the bundled syntax tests are all reindent ones (C++ L770, CSS L77, JS L682,
   JSX L34, Zsh L107). Make bracket counting skip string and comment scopes; move language rules into indentation
   preference data (our own `.tmPreferences` resources) where possible. Target: those 5 pass, nothing else regresses.

### F. Commands and key bindings (`vel-commands`, `editing.rs` / `app.rs` command hooks, your keymap block)

1. **Indentation commands** (indentation.html): `expand_tabs` / `unexpand_tabs` (leading indentation only, using
   `tab_size`, `set_translate_tabs` argument, one undo step, batch edit for large files) and `detect_indentation` as a
   command (reuse the load-time detection, write the view layer). Lane B adds the View › Indentation menu rows; keep
   these command names.
2. **`set_setting`** (settings.html: `{"setting": …, "value": …}` on the view layer) next to `toggle_setting`.
3. **Default macOS bindings** *(verify each against the docs pages, not the Default package)*: ctrl+super+shift+f
   toggles distraction free (keep super+shift+alt+enter as an alias), super+plus next to super+= for
   `increase_font_size`, ctrl+alt+up / ctrl+alt+down for `scroll_lines`. Run the keymap conflict tests.

## Lane B

### B. Word wrap and fonts (`crates/velocity/src/view.rs`, `crates/vel-render/src/font.rs`, `crates/velocity/src/layout.rs`)

1. **`draw_centered` with `wrap_width: 0` squeezes the text into one column** (`View::margin` ~379 uses
   `wrap_width.max(1)`; `wrap_width` defaults to 0, so plain `draw_centered: true` triggers it too). 0 means the
   window width (distraction_free.html): no centring margin, or centre on the window. Share one
   `effective_wrap_columns()` with `apply_style` (~436-452). Headless test.
2. **Font fallback** (`Font::load_fallbacks` ~171-176 loads only PingFang SC and Apple Color Emoji): Hangul, Thai,
   Devanagari, Hebrew, Arabic, math alphanumerics, Braille and private-use characters render as boxes. On a missing
   glyph ask CoreText (`CTFontCreateForString` / the default cascade list for the user's languages) through
   `vel-platform`, cache the chosen face per code point and the loaded faces per file. Right-to-left ordering is a
   separate later item; glyphs must at least be visible.
3. **Ligatures and `font_options`** (ligatures.html): ASCII runs never reach the shaper (`font.rs` ~223 table lookup),
   so `->` / `!=` never form ligatures in fonts that have them; `view.rs` ~920 reads only `no_bold` / `no_italic`.
   Support the documented `no_liga` / `no_clig` / `no_calt` / `dlig` / `ssXX` feature switches and the antialiasing
   options *(verify the list)*. Keep the ASCII fast path for fonts without GSUB `liga`/`calt` and for runs without
   ligature-prone punctuation; cache shaped runs; caret positions inside a ligature split the cluster width. This is
   the scroll hot path: paired headless `features` numbers ≤ 5% regression with a ligature-free font, and report the
   cost with a ligature font.
4. **Font file loading** (`font.rs` ~47 memory-maps only `/System/Library/Fonts/*`; PingFang lives under
   `/System/Library/AssetsV2/…`, so ~78 MB are copied, and the first non-ASCII character stalls 22-134 ms): map every
   font file, and load fallbacks only on demand (item 2).
5. **Zoom** (`app.rs` ~1083-1100 writes the view layer; reset hard-codes 13): `increase_font_size` /
   `decrease_font_size` change `font_size` in the user Preferences (all views, persisted); `reset_font_size` removes
   the user entry (docs.sublimetext.io) or writes `default_font_size` when set (build 4200). One glyph-cache rebuild.
6. *(verify)* **Wrap opportunities:** wrap after punctuation in `word_separators` (`/`, `-`, `.`, `,`, …) and between
   CJK characters, not only at white space (`layout.rs` `make_line` ~291-318), in the same pass.
7. **Line height from the font:** `view.rs` ~418 uses `20 × font_size / 13 + padding`; use ascent + descent + leading
   (rounded to device pixels) plus `line_padding_top/bottom`, the real ascent for the baseline. Hit testing and
   autoscroll follow `line_height`. Check that the default font's line height stays the same (or say how it changes).

### C. Minimap, scroll past end, distraction free, full screen (`view.rs`, `workbench.rs`, `workspace.rs`, `panels.rs`, `vel-platform`)

1. **Distraction-free settings apply to new tabs:** the layer is only replaced on existing tabs
   (`workbench.rs` ~543-555); `Tab::new` / `new_file` / `open_mode` / clones / group moves / session restore must
   apply the window's layers through one `apply_window_layers(&mut tab)` (state on the window).
2. **Minimap scale:** today `y = row / len_lines × height` (`view.rs` ~1798/1813/1840, click ~720-732), so a 10-line
   file is spread over the whole height and a long file becomes a band; wrapped lines are ignored. Use a fixed scale
   per visual line (our own value, e.g. 1/8 of the line height, at least 1-2 px), scroll the minimap with the view when
   it is taller than the control, map clicks and drawing through the same function, and draw only the visible part.
   `minimap_horizontal_scrolling` (build 4142) *(verify)*.
3. **`toggle_minimap` and the other View toggles (side bar, tabs, status bar) are window state,** remembered per
   window in the session; in distraction-free mode they are remembered separately (distraction_free.html). Today
   `panels.rs` ~1106 writes the active editor's view layer.
4. **`scroll_past_end` accepts 0.0-1.0** (build 4107; `vel-config/src/settings.rs` ~226 is a bool and 0.5 reads as
   false): true = 1.0, false = 0.0, `clamp_scroll` (`view.rs` ~514) allows scrolling `f × viewport` past the end.
5. **`always_show_minimap_viewport` defaults to false** (docs.sublimetext.io settings; ours is true at
   `Preferences.sublime-settings` ~90) — a visible default change, cite the source; the minimap's left border line
   (`view.rs` ~1864-1874) is drawn only with `draw_minimap_border` (already listed in `vel-settings-coverage.md`).
6. *(verify)* **`use_simple_full_screen`**: full screen without a separate Space (borderless window over the screen
   frame, auto-hidden menu bar and Dock, frame restored on exit); distraction free uses the same path.

### D. Color scheme drawing (`view.rs`, `vel-render`)

4. **`brackets_options` / `bracket_contents_options` / `tags_options`** accept `foreground` (recolor the glyphs) and,
   for brackets, `bold` / `italic` (color_schemes.html); today `underline()` (`view.rs` ~1942) draws an underline for
   any unknown value.
5. **`font_style: glow`** (build 4107) is parsed but never drawn: draw a soft glow under the glyphs only for runs that
   use it (extra pass limited to those runs; measure).
7. **Selection corners:** draw multi-line selections as one outline with `selection_corner_style` (`round` default,
   `cut`, `square`) and `selection_corner_radius` instead of per-line rectangles with separate borders (`view.rs`
   ~1255-1265). Depends on lane A's D1 parsing those keys; until it lands, read them leniently yourself.

### A. Mouse (platform side)

3. **Scroll bar track clicks** follow the macOS "Click in the scroll bar to" setting (build 4151;
   `AppleScrollerPagingBehavior`; Option inverts it): page by page or jump (`view.rs` ~690-706 always jumps).
4. **Dragging selected text** starts only after `NSDragAndDropTextDelay` when the user set it (build 4121;
   `mouse_select.rs` ~279 uses a 4 px threshold only). Expose the defaults through `vel-platform`; keep lane A's
   `mouse_select.rs` edits (A1/A2) in mind — add your check in a separate function.

### F. Rulers

4. **`rulers` entries** may be `[column, "style"]` with `ruler_style` (solid/dotted/stippled, build 4142) and
   `ruler_width` (build 4166); one malformed entry drops only itself (today `Vec<usize>` drops the whole list,
   `settings.rs` ~212). Draw dotted/stippled rulers with the existing stippled primitive.

## Already correct (keep, with their tests)

Column selection (option/middle drag, cmd add, cmd+shift subtract), word/line drag units and autoscroll, gutter line
selection, visual-line up/down in wrapped text, option+left/right word motion, `wrap_width_style`,
`indent_subsequent_lines`, `word_wrap: "auto"`, fractional font sizes, distraction-free defaults and full-screen
restore, `drag_text`, user color scheme overrides (variables/globals merged, rules appended), hashed foregrounds,
`color()` adjusters other than the blend direction, syntax version 1/2 differences, `extends` variables and
`meta_append`, `branch`/`fail` with the 128-line limit, `embed`/`escape`, `apply_prototype`, `with_prototype`,
`meta_include_prototype`, `clear_scopes: true`, syntax test symbol assertions.

## Rules

- Worktrees `/Users/zoro/dev/velocity-lane-a` / `/Users/zoro/dev/velocity-lane-b`, rebased on main first. Do not
  commit. Lane A owns `vel-syntax`, `vel-highlight`, `indent.rs`, `vel-color-scheme`, the scheme loader suffix list,
  `mouse_select.rs` (A1/A2), mousemap parsing, `vel-commands` and its own keymap block; lane B owns `view.rs`,
  `vel-render`, `layout.rs`, `workbench.rs`, the window toggles in `panels.rs` (C3 only), `vel-platform`, Preferences,
  menus and its own keymap block. Theme/scheme resources written for the old `blend()` meaning belong to lane A (D3).
- Headless tests per item: malformed scheme and syntax tables, derived-globals table, blend golden values, extends
  with anonymous contexts in parent and child, deep nesting, unresolved references, scope-stack model, reindent
  syntax tests, mousemap parsing, margin/wrap geometry, fallback coverage per script (font tests may need the
  system font service; mark native-only checks clearly), minimap mapping, layer application to new tabs, ruler
  parsing. Native checks (ligatures, glow, corners, full screen, scroll bar paging, drag delay) go to MANUAL_TEST.md.
- Performance: B2-B4 and B7 touch font loading, shaping and layout, C2 and D4/D5/D7 the scene: paired headless
  `features` / `typing1` / `typing1000` against main, ≤ 5% regression (retired instructions if the machine is busy),
  plus the first-non-ASCII stall and RSS before/after for B2/B4. Lane A: syntax/highlight throughput must not regress
  (existing highlight benches), load-time warnings must not slow cached loading noticeably.
- No regressions; fmt, clippy `-D warnings`, `cargo test --workspace --locked --release`, rustdoc `-D warnings`.
