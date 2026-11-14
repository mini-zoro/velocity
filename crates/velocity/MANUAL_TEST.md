# Native AppKit NSTextInputClient manual checklist (P1)

Adapted from the repository’s MIT OR Apache-2.0 P0 checklist. Hardware results are pending.

Run outside the sandbox:

```sh
cd /Users/zoro/dev/velocity
cargo build --release -p velocity
./target/release/velocity --log-input crates/velocity/fixtures/ime-and-unicode.rs
```

Record machine, macOS version, active input source, keyboard layout, display/scale/refresh and the
JSONL filename. Test each relevant row with Simplified Chinese Pinyin, then the Japanese input
source where indicated. Position the caret by clicking or arrows; select with Shift+arrows or drag.
Leave the observed-result column empty until tested.

| Case and steps | Expected result | Observed result / pass/fail / log reference |
| --- | --- | --- |
| Pinyin: click after `→`, type `nihao` | Inline marked text with underline and preedit caret; document contains transient preedit (Undo/cancel restores the original); candidate window adjacent to caret | |
| Choose `你好` with Space or candidate number | Exactly one `你好` inserted, marked text cleared; confirmation Enter/Space does not insert an extra newline/space | |
| Continue typing English after commit | New characters insert once at the new caret; no duplicated IME commit | |
| Shift toggles English while composition is active | Native IME's selected commit/cancel behavior is reflected once; no stale preedit; subsequent English input works | |
| Backspace inside `nihao` preedit | Marked text updates through IME; previously committed document text is not deleted | |
| Esc during preedit | Native composition cancels, underline/candidate UI clears; document content remains as before composition | |
| Move within a preedit using native IME keys | Preedit cursor and selected conversion range update in UTF-16 code units, never splitting a surrogate | |
| Scroll a few pixels with an active visible composition | Marked text stays inline and candidate anchor follows the visible caret, with no 2× Retina offset | |
| Scroll composition offscreen and back | Candidate anchor stays inside the client area while offscreen; real caret position is restored on return | |
| Resize with an active composition | Underline, text, cursor and candidate anchor follow current geometry; no stretched/stale atlas text | |
| Move window between 1× and 2× displays | Glyphs rerasterize; caret/candidate placement keeps logical-point alignment | |
| Japanese: enter Kana, Space to convert, move conversion selection, then confirm | Kana/marked range is visible; candidate conversion commits once; caret ends after committed text | |
| Ctrl+Cmd+Space, choose an emoji in the native picker | Emoji inserts once and advances appropriately; record a missing commit separately from a missing image | |
| Option+E then E using an English layout | `é` inserted once, dead-key preedit cleared, no extra apostrophe or `e` | |
| Pinyin full-width punctuation: `，。！？（）` | Correct full-width glyphs and two-cell spacing; no duplicate ASCII punctuation | |
| Select text, then start Pinyin and confirm | Selection remains well-defined; commit replaces selected text exactly once; subsequent deletion is safe | |
| Switch focus away and back during preedit | No stale marked text; IME can be enabled and used again after focus returns | |
| Compare fixture's `你好`, Kana and punctuation | Correct fallback glyphs; no tofu, inappropriate Latin glyphs, clipping or baseline jumps | |
| Compare `😀 🦀 🚀 👩‍💻 ❤️ 🇨🇳` | Simple color emoji visible; inspect ZWJ, variation selectors and flags for correct single-cluster display | |
| Compare NFC `é` and decomposed `é`; Backspace each | Legible placement; one Backspace deletes one extended grapheme | |
| Shift+arrows and mouse drag across Chinese/emoji and newlines | Selection rectangles follow cell widths and line boundaries; Backspace/typing replaces selection | |
| PageUp/PageDown, horizontal and fractional trackpad scrolling | Smooth point-based movement, gutter fixed; caret is revealed by subsequent arrow/edit input | |
| Type quickly with `--log-input`, then close | Every delivered keyboard and IME event has a timestamp; changes have receipt→submit/present-return records | |

Native logging records key events, standard command selectors, Insert/Marked/Unmark,
explicit replacement ranges, selected/marked document UTF-16 ranges and latency to
queue submission/present-return. A commit without prior marked text must work. The
buffer contains inline transient preedit; cancellation restores its origin snapshot,
and the whole composition is one Undo step. `unmarkText` keeps the current text once.

Additional native range/geometry cases (all pending):

| Case and steps | Expected result | Observed result / pass/fail / log reference |
| --- | --- | --- |
| Japanese: select committed 日本語, invoke input source's reconversion, change and confirm | Explicit UTF-16 replacement range replaces exactly that text; one Undo restores it | |
| Place caret after a non-BMP emoji, invoke reconversion of adjacent CJK | Offsets count emoji as two UTF-16 units; no adjacent scalar is damaged | |
| English: hold E and choose an accent from the press-and-hold menu | AppKit replacementRange changes the original E once; selected/marked ranges remain valid | |
| Emoji picker with no composition and a nonempty selection | Exactly one emoji replaces the selection, with correct color and candidate position | |
| Start composition over a selected word, cancel, Undo/Redo, then paste | Original selection/text restore on cancel; subsequent history/paste is correct | |
| Reconversion requests attributedSubstring at document edges / emoji | Actual range is clipped to valid scalar boundaries; no crash or half-surrogate text | |
| Drag from the text to a window on a display left/above the primary display | Candidate rect remains in screen points with correct sign; no Retina multiplication | |
| Hide/occlude/unocclude during composition and move between 60/120 Hz displays | No idle redraw loop; candidate anchor and display rate refresh correctly | |
| About/Hide/Quit, Undo/Redo/Cut/Copy/Paste/Select All from menus and shortcuts | Native actions reach editor commands; clipboard text round-trips Unicode | |
| Toggle system light/dark, resize continuously, enter/leave fullscreen | Fresh geometry and colors, no stretched text; native fullscreen works with tabbing disabled | |

Check environment.fonts and final_render_state.fonts for resolved faces, warnings and
CoreText raster counts. Some OS font formats require the retained native face's bitmap
fallback. Check CJK, combining marks, emoji colors/alpha and ZWJ/flags visually.
The layout is a unidirectional grid, not paragraph bidi or automatic wrapping.

Candidate queries implement both firstRectForCharacterRange:actualRange: and
characterIndexForPoint:. View→window→screen conversion uses logical points, including
flipped-view conversion. Record requested/actual UTF-16 ranges and resulting screen
placement when any reconversion or picker case fails. No native input or visual pass
is claimed from headless tests alone.

## Round 4 editing smoke test

Use disposable copies of files. Record results here; these native cases were not run in the sandbox.

- [ ] Open two files with Cmd+O and via `velocity first.txt second.txt:2:3`; check the one-based Unicode position and tab labels.
- [ ] Type, select, cut/copy/paste, Enter with indentation, Tab/Shift+Tab; save with Cmd+S, Save As with Cmd+Shift+S, and reopen to compare bytes.
- [ ] Add carets with Cmd+click, Cmd+D and Ctrl+Alt+Up/Down; type/delete/paste at every caret. Split a multiline selection with Cmd+Shift+L; all carets and selections must render.
- [ ] Cmd+Z/Cmd+Shift+Z restores the full multi-caret transaction; grouped typing undoes by the buffer's rules. Cmd+U/Cmd+Shift+U restores soft selection history. Escape leaves one selection.
- [ ] Switch with Ctrl+Tab/Ctrl+Shift+Tab and Cmd+1…9; each tab retains its text, history and scroll. Closing a dirty tab/window offers Save/Cancel/Discard; cancelling leaves edits intact.
- [ ] Open/save UTF-8 with/without BOM, UTF-16LE/BE with BOM, Windows-1252, LF/CRLF/CR and mixed endings. Confirm displayed encoding and majority line endings, BOM retention and permissions. Unrepresentable saves must report an error and preserve the original file.
- [ ] Set `trim_trailing_white_space_on_save` to true then `"not_on_caret"`; test all caret lines, `trim_only_modified_white_space`, inserted lines and final-newline cleanup. Undo cleanup as one transaction.
- [ ] Change an open file externally: clean views reload, dirty views ask Keep Edits/Reload. Repeat with an atomic rename and after a previous save.
- [ ] Edit User preferences/keymap with comments/trailing commas; change tab size/font/gutter and override a shortcut. Verify the new binding wins and project/view settings keep their precedence.
- [ ] Enter `(`, `[`, `{`, quotes, and matching closing characters; confirm pairing/skip contexts. Repeat on the actual keyboard layout, including shifted shortcuts, keypad keys and an optional modifier-tap user binding.
- [ ] Repeat the IME rows above with multiple carets: marked text is a transient primary preview; commit inserts once per caret and one Undo removes the committed conversion. Explicit native replacement ranges replace only their specified region.
- [ ] Open the 100 MB fixture: a loading tab appears promptly, UI stays responsive, progress advances, and edits cannot overwrite the file before loading succeeds.
- [ ] While testing startup outside the sandbox, verify asleep/locked/occluded runs produce explicit errors in about two seconds, then unlock/uncover and rerun successfully. Inspect skipped_drawables, first_callback_main_ms and first_real_scanout separately.


## Round 5: widgets, navigation, search and side bar

Run the reviewer fixture command from RUNBOOK.md outside the sandbox. These are human acceptance
checks; use the automatic `--bench-panels`/search example/matrix for measurements. Leave results
blank until observed. Keep the entire IME section above, and repeat its core cases in both the
Goto input and Find/Replace/Where inputs (including Undo, horizontal scrolling and Retina moves).

| Case and steps | Expected result | Observed result / pass/fail / log reference |
| --- | --- | --- |
| Cmd+P; type `ime`, Up/Down/PageDown; Esc | Fuzzy glyph highlights, kind/path details and a bounded list; transient preview disappears; original tab, selection and viewport return | |
| Cmd+P `ime-and-unicode.rs:8:3`; Enter | Target uses one-based scalar coordinates and becomes a pinned tab | |
| Cmd+P `:8`; Esc, then repeat and Enter | Preview moves within current file; Esc restores selection; Enter retains it | |
| Cmd+P `#input`; select a word; Enter | Fuzzy word rows preview the correct UTF-8 position | |
| Cmd+R / `file@symbol` | Current-file syntax symbols appear with transformed names; selection previews, Escape restores, Enter commits | |
| Cmd/Ctrl+Enter on a Goto result | Two visible panes; each scrolls independently; clicking switches active target; Close Split palette action restores one pane | |
| Cmd+Shift+P `save`; inspect hint; run safe commands repeatedly | Captions, binding hints and per-invocation recent ordering; arrows and Enter work; Escape preserves document | |
| Pinyin/Japanese/emoji in Goto and find inputs | Composition stays inside the focused input; candidate rectangle follows wide graphemes; confirmation never accepts a panel prematurely; Undo changes only that input | |
| Cmd+F; toggle regex/case/word/wrap/selection; Cmd+G / Shift+G | Correct highlights and forward/backward wrap; invalid Perl lookaround/backreference shows an error without altering text | |
| Cmd+I and type incrementally; Escape | Viewport updates during typing, complete matches arrive asynchronously, no stale results after an edit/tab switch | |
| Select a region, Cmd+Alt+F; preserve case; Replace All; Undo | Only selected matches change; cat/CAT/Cat map to replacement casing; one undo restores all; repeated shrinking replacements remain in the tracked selection | |
| Regex `(cat)([0-9]+)` -> `${2}:$1`; Replace All | Captures expand correctly; zero-width regex progresses without hanging | |
| Cmd+E / Cmd+Shift+E; Alt+Enter | Find/replacement seeded from selection/word; Find All creates a multi-selection | |
| Cmd+Shift+F; Where `<project>, *.rs, -ignored*`; Enter | Streaming Find Results has file headers, numbered context, highlights and summary; Up/Down/Enter or double-click jumps | |
| Edit without saving; Files Where `<open files>` | Search sees the unsaved buffer text; IO/decoding failures are reported | |
| Fill Replace, run Files search, invoke Replace in Files; Cancel, then confirm on disposable files | Cancel writes nothing; confirmation reports counts; originals kept beside each file; changed/dirty targets rejected; backup bytes restore originals | |
| Expand fixture folders, create/rename/delete a file externally; adjust ignore rules | Lazy tree and Goto index update; .gitignore and file/folder/binary exclusions agree with their documented scopes | |
| Cmd+K, Cmd+B; Ctrl+0; navigate Open Files and folders | Visibility toggles, keyboard focus stays in tree, single-click previews and double-click/Enter pins | |
| Project: Add Folder…; choose a second directory | New root appears and indexes; canceling native picker leaves roots intact | |
| Open many files; wheel over tabs; close, dirty an input, drag tabs | Scrolling, close hit targets, dirty dots, reorder and active document identity remain correct | |
| Resize panels/window; hover/press controls; scroll output/results/list | Rows remain clipped/virtualized; caret/selection and active surface route correctly; status stays readable | |
| Click document while Find remains visible, then resume panel input | Document edits and panel edits target their own buffers; overlay always captures above both | |

Native geometry, font quality, appearance, Metal validation, refresh-rate behavior and GUI timing
remain pending reviewer evidence. Attach the run directories, completed table and observed hardware
configuration; do not replace missing scanout values with CPU timings.


## Round 6: highlighting, language commands and view features

All native observations and timing cells below remain pending reviewer runs. Use the round-6
RUNBOOK commands on an awake, unlocked display; attach actual physical dimensions and JSONL.

| Action | Expected | Observed / run directory |
| --- | --- | --- |
| Open Rust, Python, shell/shebang, compound-extension and hidden-extension files | Detected syntax appears in the status bar; first real highlighted screen is correlated to positive presentedTime | |
| Click status syntax; use Set Syntax in Cmd+Shift+P; choose another language | Filtered picker works with keyboard/IME; syntax-specific settings apply without text/history loss | |
| Set color_scheme auto; change macOS appearance, then set an explicit scheme | Original dark/light palette switches live; explicit selection stays fixed | |
| Override a scheme variable/global in Packages/User; edit an already highlighted file | Token colors and background/caret/selection/gutter/find/guides/rulers/minimap update without stale palette spans | |
| View bold/italic, solid/stippled/wavy underline; enable no_bold/no_italic | Real face variants and decorations appear; CJK/emoji fallback and baseline/grid remain correct | |
| Type quotes/brackets in code, strings and comments; repeat while scopes update | Pairing uses current scopes; closing-quote skip remains usable; pending selector contexts do not pair on unknown scopes | |
| Cmd+/ on multiple carets, overlapping selected lines, and reversed selections; Cmd+Alt+/ | Line/block delimiters follow syntax metadata; toggling and undo restore original text | |
| Enter around Rust braces, Python blocks and temporary continuation rules; Reindent | Correct indentation/tab stops; asynchronous reindent never overwrites a later edit | |
| Cmd+R, filter and move through symbols; Escape, Enter; repeat after editing | Metadata names/transforms and positions update; preview/cancel/commit preserve source selection | |
| Move carets around nested brackets and HTML/XML tags; toggle individual matching settings | String/comment brackets are ignored; tag matching can stay enabled with bracket matching disabled | |
| Toggle wrap; resize; set wrap_width/style and continuation indentation | Graphemes remain intact; Up/Down/Page, mouse hit tests, selections and reveal follow visual rows | |
| Compose Pinyin/Kana/emoji across a wrap boundary; commit, cancel, undo | Candidate rectangle covers the first visual fragment; committed text and layout use the same snapshot | |
| Toggle whitespace selection/all/none/leading/trailing, guides, rulers, relative numbers and gutter | Only requested decorations appear; scheme globals and clipping agree | |
| Set smooth/phase/blink/solid caret, extra width and line padding; move between editor and panels/windows | Animation resets on input; inactive selections and focus render correctly; no idle continuous redraw for solid caret | |
| Fold via gutter, selection, indentation/brackets/block scopes, level and tag attributes | Ellipsis markers and suffix text remain visible; hidden lines are skipped by navigation and highlighting requests | |
| Edit/undo before and inside folds; unfold, unfold all; toggle fold-button fading | Named fold regions track edits; viewport and candidate geometry stay valid | |
| Show/hide minimap, click, drag beyond its bounds and release; resize or edit/newline | Token colors match the document; viewport scrolls without changing selections; indicator and border obey settings | |
| Run automatic open/scroll/typing with default and 4K physical sizes, then Metal validation | Positive highlighted scanout, full CPU timing, isolated minimap GPU timing and clean validation; no external input in automatic runs | |
