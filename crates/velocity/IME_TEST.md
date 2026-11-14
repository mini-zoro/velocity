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
