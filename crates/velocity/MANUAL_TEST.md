# Round 11: Vintage and user grammar review

Run outside the sandbox on an awake, unlocked display:

```sh
cd /Users/zoro/dev/velocity-lane-a
cargo build --release -p velocity
zsh crates/velocity/scripts/review-round11-gui.zsh
```

The script freezes the binary, records its hash, creates an isolated profile with both original
TextMate fixtures and enables Vintage in command mode. It prints the profile/grammar/log paths.
`--prepare-only` creates those fixtures without starting a native application.

- [ ] See COMMAND in the status bar and a readable block caret. Try `2d3w`, `u`, `gg`, `G`,
  `f`/`;`, `ciw`, `v`, `V`, Ctrl+V, `>`, registers, `q`/`@` and `.`. Pending prefixes/counts
  and visual line/block selections should update on screen. Confirm multiple carets still work.
- [ ] Press `i`, activate Pinyin/Japanese, compose and choose candidates. Preedit and candidate
  geometry should match normal editing; Escape after committing returns to command mode.
  In command mode, `h/j/k/l` should act immediately with the input method still selected.
- [ ] Use `/one`, `?one`, `n` and `N`; Escape cancels a prompt without editing. Try `d/three`.
  Use the input panel's arrows, clipboard and IME. Open Find or another tab during a prompt.
- [ ] In `editor.rs`, reindent the second line with `==`; `u` restores indentation in one step.
  Verify clipboard `yw`/`p` using another application. Check `:w`, `:wq`, `:q`, `:q!`,
  `:e`, `:e!` and `:w <temporary path>` with dirty/save-cancel/failure paths.
- [ ] Ctrl+Alt+V disables/re-enables Vintage for the active view. Edit User preferences while
  open to test all four settings. Turning it off closes a vi prompt and restores normal typing.
  Reopen using a fresh profile to confirm Vintage is off by default.
- [ ] `notes.vnote` detects Velocity Notes; `xml-notes.vxnote` detects Velocity XML Notes.
  Check TODO/string/comment colors, syntax picker and palette entries. Edit the printed JSON
  grammar's name/rules, add another nested grammar, then remove it: open views and choices
  should reload while retaining unsaved edits. Try a malformed definition and read-only cache.
- [ ] Test split/clone views, Retina scaling, status clipping and Metal validation. Record the
  display, input source and input JSONL path with observed results. Native rows remain unverified
  until a human runs them; the headless tests initialize no window server or GPU.

Earlier checklists follow.

# Native AppKit NSTextInputClient manual checklist (P1)

Round-nine tester status: **Chinese IME confirmed working by the human tester**. Hot Exit is
currently under human test. The individual legacy rows below retain their original evidence
cells; Japanese/reconversion/display-specific cases are not marked passed by that confirmation.

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


## Round 7 — workbench/session/build acceptance (native review pending)

Run the isolated-profile commands at the top of RUNBOOK.md. Fill the last column with evidence,
actual scale/size/refresh and any failure; these rows have not been executed in the sandbox.

| Action | Expected behavior | Reviewer observation |
| --- | --- | --- |
| Launch with a non-existent data directory | Empty editor opens, User tree may be created, no missing-Packages exception | |
| Launch with corrupt session / read-only profile / a file occupying the data path | Empty usable editor; corrupt original preserved; IO failures reported without data loss | |
| All seven layout presets, normalized custom cells, splitters and New/Close Pane | Cells cover content, divider hits respect spanning cells, tabs stay with their groups | |
| Cmd-click / Shift-click / Tab Selection commands | Selected sheets appear horizontally inside one group with connector styling | |
| Sidebar Cmd/Shift multi-select; Goto Cmd+Enter | Additive sheets stay visible and previews pin on acceptance | |
| Move/close multiple selected tabs | Order/selection preserved; the last dirty shared views ask once when policy requires | |
| Clone File, separate carets/scroll/folds, type/undo/redo, close current owner | One document/history, independent view state, no stale text or detached-view panic | |
| IME in lower/right group, scroll inactive selected sheet, return focus | Candidate anchor, hit testing, selection and view-local scroll use correct offsets | |
| Cmd+Shift+N; close one window; CLI -n/-a/-w and stdin | One application, independent windows; wait ends when requested sheets close | |
| Full screen; distraction-free from normal and already-full-screen; customize its settings | Chrome/settings restore correctly and sidebar visibility is remembered per mode | |
| Quit/relaunch dirty/untitled text, redo, folds, syntax, sidebar and search state | Retained state restored, hidden clean tabs lazy; round-nine native history removes the former private-history limitation | |
| Hot Exit always / only_on_quit / disabled; remember_open_files false | Close/quit/crash policies follow RUNBOOK; disabled never silently discards dirty project docs | |
| Edit a dirty document on disk while quit, then restore | Buffer text retained as dirty, restored undo history dropped | |
| Open/Save As/Close/Edit/Quick Switch Project; nested fresh destination | Folder rules, inline builds and unknown keys preserved; workspace tied to project | |
| Leave a project idle after editing | Debounced session writes complete without an autosave/settings-reload loop | |
| Cmd+B with Workbench Example; F4/Shift+F4; output double-click | Both streams appear, exit/time shown, correct diagnostic location and inline badge | |
| Build With variants, Cancel Process Tree, quiet, syntax and wrapping | Variant overrides apply, tree terminates, UI stays responsive, output follows settings | |
| Native automatic startup/typing/scroll with four groups | Positive scanout/complete JSONL; budgets from RESULTS/RUNBOOK assessed with no missing timestamps | |


## Round 8 — intelligence and Hot Exit acceptance (native review pending)

Run the exact commands in RUNBOOK.md. Do not run GUI/Metal in the sandbox.

- [ ] Cmd+Shift+R: type `render`, inspect kind badges and file:line, arrow through previews,
  Esc restores original selection/scroll and Enter pins the target. Repeat with unsaved names.
- [ ] F12/Cmd+Alt+Down on `unique_target` jumps to another already-open file. `shared_name`
  opens multiple definition rows. Shift+F12 lists references when syntax metadata publishes
  indexed references. Missing metadata yields no reference locations. Ctrl+- and Ctrl+Shift+-
  traverse cross-file jumps and remain bounded after 130 jumps.
- [ ] Hover over a symbol, click a location, then disable `show_definitions` and confirm it
  stays hidden. Open Help → Indexing Status, watch percentage/logs, close only that window,
  and verify editor focus, folders and session state are intact.
- [ ] Ctrl+Space/Auto Complete: arrows, cycling, Enter and mouse commit. At the bottom of a
  view the popup flips upward. Check annotations/kinds/details and a second window/group.
  Type rapidly, switch tabs and move the caret while queries are pending; stale results never
  replace text. Set `auto_complete_commit_on_tab`, `auto_complete_cycle`, delay, selector,
  triggers, `auto_complete_with_fields` and size limits and verify each behavior.
- [ ] Type `fn`, Tab, rename the first field, Tab to args, Shift+Tab back, then Tab to `$0`.
  Expand `${1:name} = $1; ${2:value} $0` from an inline/User snippet: field outlines and mirrors
  update live with Chinese/emoji and multiple carets. Esc or moving outside the field exits.
  Verify undo/redo treats a primary edit and all its mirrors as one transaction. Clone the file
  into another group: passive painting must preserve fields; editing the other clone must
  cancel stale fields in the source view.
- [ ] Add/change `.sublime-snippet` and `.sublime-completions` resources in Packages/User;
  snippet names appear as `Snippet: …` and tab triggers/resource suggestions update. Runtime
  folders and unsaved text must remain in the window.
- [ ] Hot Exit: edit unsaved named and untitled files with expanded/mirrored snippets, retain
  an undo/redo branch, create multiple windows/groups and open the index status window. Quit
  and relaunch the same profile. Text, selections, folders, layouts and normal undo/redo return;
  popup/field sessions and the separate status window are transient. Repeat with `hot_exit`
  always/only_on_quit/disabled, Cmd+W and a forced process exit after autosave. Keep the existing
  read-only-session failure guard: quit must not silently discard dirty text when saving fails.
- [ ] Start with fresh, read-only and corrupt profiles. Cache creation/fallback and subsequent
  session writes must not reload settings or drop folders added through Open Folder. Verify
  indexing exclusions, disabling/re-enabling indexing and closing an unsaved indexed buffer.
- [ ] Retest Chinese IME commits/candidate placement with a popup and inside a snippet field.
  IME preedit must suppress completion and never publish preview text to the index/session.
- [ ] Record five native intelligence runs, physical size/scale/refresh and memory. Use the
  separate `--intel-words 0` run for the real ten-file memory target and retain the default
  100k-word stress footprint separately. Compare
  p50/p95 typing to 2/4 ms and definition jumps to 20 ms; retain scanout and submit separately.


## Round 9 — full Hot Exit and review UI (human verification pending)

Use the exact disposable-profile commands at the top of RUNBOOK.md. Chinese IME confirmation
remains accepted; repeat one composition only to check the new Hot Exit boundary. Record native
results/log references here; headless tests do not check these boxes.

- [ ] **First priority — full history:** type two words, move/select text, soft undo/redo, text undo once, quit and relaunch. Continue Cmd+U/Cmd+Shift+U and Cmd+Z/Cmd+Shift+Z: text, selection direction/primary, groups, redo cursor and saved-state dirty indicator match the uninterrupted session.
- [ ] Quit halfway through typing a word, relaunch and finish it without moving the caret; one Undo removes the same typing group. Repeat after an actual save: undo/redo must return to the clean saved point.
- [ ] Clone a file into two groups; alternate text edits and selection-only moves, close one historical editing view, quit/relaunch, then undo/redo from the remaining view. Each edit retains its owner; clones retain independent selections/scroll and share text undo.
- [ ] Keep one clone hidden, quit/relaunch, activate it, save another session and relaunch again. No extra selection events appear in soft undo; clean hidden documents remain lazy.
- [ ] Change a dirty file externally while quit. Relaunch retains unsaved contents as dirty and drops text/soft history. For a changed clean file, load current disk contents and drop history. Missing/read-only/corrupt profiles still recover as documented.
- [ ] Commit one Chinese IME composition, soft-select elsewhere and quit/relaunch. The composition remains a single undoable edit; no transient preedit returns.
- [ ] In the fixture sidebar, inspect `? M ! A+ M+ D+ U!`; missing and staged-deleted paths are visible. Folder badge priority ends at Unmerged; ignored files/folders are dimmed. Hover badge areas for status tooltips, including the Open Files section.
- [ ] Check branch name and `? / M / S / !` counts against the fixture. Turn `show_git_status_in_status_bar` off; turn `show_git_status` off/on. Set an invalid `git_binary`: behavior continues through gix.
- [ ] With `sublime_merge_path` empty, clicking branch status does nothing. Configure a real smerge-compatible executable; branch/context actions open repository, file/folder history, line history and blame for the intended path. Index bytes remain unchanged by Velocity status/diff reads.
- [ ] Add/modify/delete lines, including a deletion at EOF without a final newline. Inspect color-scheme `line_diff_*` markers; Ctrl+. / Ctrl+, navigate with wrap. Test `mini_diff` true/false/"auto" both inside and outside the repository.
- [ ] Switch `git_diff_target` between index/head; stage/save/change the file externally. Markers and revert sources update to the selected reference. A non-repository save resets its saved-file reference.
- [ ] Cmd+K Cmd+/ and right-click Show/Hide Diff Hunk display original lines **beneath** current content, with changed scalar shading. Cmd+K Cmd+; shows only this hunk. Scroll, resize, wrap, move Up/Down across the read-only original rows and click below them; caret and IME candidate geometry stay aligned. An edit hides old expansions.
- [ ] Cmd+K Cmd+Z, Revert Modification and context Revert Diff Hunk restore exactly one hunk; Undo restores the edit. Repeat after grouped undo/redo; no stale hunk reverts another version.
- [ ] F6 toggles squiggles; with selector `comment, string`, comments/strings are checked and code identifiers are not. Change `misspelling` in the scheme. Scroll through long documents and two clone viewports: checks follow only visible exact scopes without an idle redraw loop.
- [ ] Ctrl+F6 / Ctrl+Shift+F6 select/reveal misspellings beyond the viewport and wrap. Move the caret or edit while a navigation request is pending; the old request must not jump later.
- [ ] Right-click a misspelling; accept a suggestion, Undo, Add to Dictionary and Ignore Word. Suggestions are version checked, Escape dismisses the menu, and words survive relaunch. Existing User preference comments/keys survive.
- [ ] View → Dictionary lists English and `Packages/User/review.dic`; select the custom dictionary. Test a syntax-specific `spell_check` setting, invalid dictionaries and a package-traversal path; no incorrect underlines or crash.
- [ ] Run automatic native on/off typing, 4K typing, full-history restore and Metal validation commands. Retain complete JSONL and assess ≤5% typing regression, 2/4 ms absolute typing and ≤300 ms native first-window targets separately from CPU results.

## Round 10: theme and minihtml acceptance

No GUI or Metal execution was performed in the sandbox. Leave these boxes unchecked until a
human runs them. Preserve the round-nine IME confirmation above; repeat the panel-input cases
because the UI now shares proportional font metrics with hit testing and candidate geometry.

Run `zsh crates/velocity/scripts/review-round10-gui.zsh /absolute/path/to/round9/velocity` on an
awake, unlocked display. It builds once, runs the new automatic native demo with Metal validation
at ordinary/4K logical sizes, then alternates round-nine/current four-group typing and scrolling.
It finally opens a disposable manual profile. Record its printed evidence directory, machine,
OS, scale/refresh, all completion/error records and any Metal diagnostics.

- [ ] Open the command palette and run **Theme & minihtml: Review Demo**. Inspect all three
  phantom placements, the right-hand annotation, the scrollable hover and the HTML sheet tab.
  Wrapped paragraphs, list markers and links must keep distinct baselines; emoji/CJK and
  bold/italic/code faces must align with their measured boxes.
- [ ] In the HTML sheet click Light/Dark, then switch OS appearance with `theme: "auto"`.
  Explicit `theme` stays selected; `dark_theme`/`light_theme` choose the auto pair. Check native
  title-bar appearance, then set `themed_title_bar: false` and verify system appearance returns.
- [ ] Visit Goto, palette, sidebar expanded/selected rows, transient/dirty tabs, find/replace
  buttons, context menu and status bar in both appearances. Hover/pressed/selected states,
  match colors, kind badges, file icons, close/fold glyphs and minimap tint must be legible.
  Drag a scroll puck and switch `overlay_scroll_bars` between enabled/disabled/system.
- [ ] Add a User theme fragment that changes a status/sidebar color, font size and content margin;
  verify live reload. Try `ui_scale` 0.75, 1, 1.5 and 2; in a search field click CJK/emoji text,
  extend selection and compose Pinyin. Caret, selection, hit testing and IME anchor must agree.
  Move between displays to check @2x/@3x resource selection and glyph/texture sharpness.
- [ ] Use the palette's **Theme & minihtml: Popup flags …** entries. Default popups close on
  selection changes; flag 16 keeps them; 4 closes on movement/click/scroll; 8 permits movement
  toward/inside the popup and closes on movement away; 32 closes on character input; 48 keeps
  selections while closing on typing. Escape closes the popup. Completion cooperation is
  exercised by the automatic demo's flag-18 popup beside the completion menu.
- [ ] Scroll a popup larger than its maximum height. Click `subl:` actions and the public HTTPS
  link, and hover titles. Link/tooltip hit regions must follow scroll/resize and stay clipped.
- [ ] Edit and undo text before an inline/below/block phantom. Text, carets, hit testing and
  phantom links must retain the same tracked anchor and reserved space. Annotations must not
  add editable rows or change vertical navigation.
- [ ] Open `fixtures/theme-ui.sublime-project` and `workbench-error.txt`, select **Theme annotations**
  in Build With and build. The error appears as a right-aligned HTML annotation with a colored
  left border; literal `<markup> &` is displayed as text. Toggle `show_errors_inline` off/on.
- [ ] In the HTML sheet click commands/browser links, scroll, resize, close/reopen the app and
  confirm its name/content return read-only. Text commands must not change its buffer.
- [ ] Repeat with fresh, unwritable and corrupt profiles, then a malformed/missing User theme
  and missing image. Confirm fallback/diagnostics and continued editing. Record actual native
  timings against the round-ten budgets; CPU structural goldens are not a pixel-quality result.
# Round 12: huge files, long lines and hex

Run `zsh crates/velocity/scripts/review-round12-gui.zsh target/release/velocity
/tmp/velocity-r12-fixtures` outside the sandbox for three automatic native runs. Each performs
100 scripted scroll/offset/edit actions and exits. Inspect its `large_file_native` record,
all load telemetry, Metal validation and presentation logs. No native result is claimed here.

For interactive checks, open the generated fixtures with `target/release/velocity -n --log-input
--data-dir /tmp/velocity-r12-manual /tmp/velocity-r12-fixtures/log-4gb.log
/tmp/velocity-r12-fixtures/line-50mb.js /tmp/velocity-r12-fixtures/binary-4gb.bin`.

1. On the 4 GB log, verify a visible prefix while the tab still shows a percentage, and the
   status bar says read-only. Scroll/select/copy within that prefix. Typing must do nothing;
   Save and Save As must be disabled and must not create a partial destination. Close during
   loading, open another file, and verify no late content replaces it. Reopen and let loading
   finish; the prefix restriction should clear and the file should be editable. Repeat with
   a large UTF-16 BOM file and a Windows-1252 file containing an invalid UTF-8 byte late in it;
   fallback may show an empty provisional view, but must retain progress and original encoding.
2. In the 50 MB JS line, wait for “Preparing line geometry” to clear, then scroll far right,
   move to the middle/end, select across the viewport and type/delete. Verify no full-line
   flash or stalled scrolling. Check Shift+arrows, Option+word arrows, Home/End, a second
   caret and pointer hit tests. Start Pinyin/emoji composition in the middle; candidate/marked
   rectangles and committed text must follow the caret. Cancel composition and undo/redo.
   Newline inserts LF without scanning an oversized line for automatic indentation.
3. Verify the large-document status hint and each **Large File: Enable … for This View**
   palette command. Explicit re-enabling can perform expensive work; automatic policy should
   remain active in another view. Viewport syntax can start approximate; a normal line after
   an oversized comment/string must receive repaired scopes without the scene thread parsing it.
4. In the binary, verify 16 hex bytes plus ASCII per row. Ctrl+G `0xee6b2800` goes to the
   `END` tail. Select those three bytes: Copy gives `45 4e 44`, **Hex: Copy Text** gives `END`.
   Try a partial final row, decimal offsets, malformed input and offsets beyond EOF. Editing,
   IME commits and Save must be blocked. Select All then Copy must report the clipboard size
   limit without trying a 4 GB allocation. Scroll with the scrollbar and trackpad. Modify a
   disposable source externally; a fresh snapshot should appear without mapping faults.
5. Close/restart with huge or binary tabs open, including a loading tab. Restore must use a
   disk reference and worker loading, with the same save restrictions, rather than treating
   the old prefix as a complete document. Verify ordinary small-file typing/scroll/IME still
   behave as in previous rounds.


## Round 13: release wiring and VoiceOver (human review required)

Run on an unlocked **macOS 14+** desktop, outside the sandbox. The script prepares a disposable
profile, a Unicode document, a windowed document and a local declarative package with a
Python marker. The isolated `vel-crash` tests exercise actual panic/signal hooks. The script
does not install the CLI or any package, or change the home directory. Choose a fresh fixture
directory; the script refuses to replace an earlier profile.

```sh
cargo build --release --locked -p velocity
scripts/package-macos.sh /tmp/velocity-r13-bundle
```

The reviewer retains the existing minimum-macOS-14 decision and the DMG signing fix. Inspect
`packaging-report.json`: prefer `hdiutil create` and require strict deep codesign of the mounted
app for distribution; a `makehybrid` fallback is not evidence of a distributable signature.

Release actions:

For the next-launch prompt, first create a uniquely named synthetic report in the normal
local crash directory (this does not crash the app or alter previous reports), then launch:

```sh
python3 - <<'PY'
from pathlib import Path
import uuid
folder = Path.home() / 'Library/Logs/Velocity'
folder.mkdir(parents=True, mode=0o700, exist_ok=True)
report = folder / ('reviewer-fixture-' + uuid.uuid4().hex + '.crash')
with report.open('x') as stream:
    stream.write('Synthetic Velocity reviewer report; no document contents.\n')
report.chmod(0o600)
print(report)
PY
```

Launch the prepared bundle with a fresh fixture profile:

```sh
zsh crates/velocity/scripts/review-round13-gui.zsh \
  /tmp/velocity-r13-bundle/Velocity.app /tmp/velocity-r13-review
```

After quitting, relaunch that same profile without rerunning fixture setup:

```sh
VELOCITY_GUI_PROCESS=1 /tmp/velocity-r13-bundle/Velocity.app/Contents/MacOS/velocity \
  --data-dir /tmp/velocity-r13-review/profile
```

- [ ] Launch: the native crash alert offers **Open Report Folder** and **Dismiss**. Open reveals
  `~/Library/Logs/Velocity`; dismiss opens nothing. The shown report becomes
  `.seen` and remains readable. Launch the executable again with the same profile,
  without creating another report: the alert does not recur. Help → Open Crash Reports Folder
  reveals that same local directory. No report upload occurs. The synthetic report can be
  removed manually after review.
- [ ] Velocity → Install Command Line Tool… shows exact source/destination paths and two bin
  choices. Select `~/.local/bin` (use Alternative when system bin is preferred).
  Cancel writes nothing; confirm creates the symlink; repeat reports an existing installation.
  Run `~/.local/bin/velocity --help`. For actual file forwarding,
  use `--data-dir /tmp/velocity-r13-review/profile`. Do not select
  `/usr/local/bin` for this disposable review. Unrelated existing commands must yield an error.
  If a new local symlink was installed for this review, remove that symlink after checking its target.
- [ ] Cmd+Shift+P → Package: Install from File… → `Reviewer Fixture.sublime-package` in the
  fixture root. Keep typing while the worker runs. Set Syntax: Reviewer Fixture, Snippet:
  Reviewer Fixture and the `reviewer` build system become available without reopening documents.
  Package: List reports resources and **partially supported: no plugin API**; select its row
  to inspect contribution paths. The Python marker never executes.
- [ ] Package: Disable/Enable removes/restores its resource choices live. Unsaved text, undo,
  selections, folders and open tabs survive. The preferences retain `KeepDisabled` and unrelated
  settings. Package: Remove removes its contributions and reports recoverable backup paths.
  Replacing an installed archive also reports the previous archive’s backup.
- [ ] Package: Install from URL… accepts a reviewer-controlled HTTPS archive URL, then a package
  name, including URLs with query parameters. Invalid TLS, HTTP URLs and invalid ZIPs show errors
  while the editor stays usable. No public service is configured or needed for local checks.
- [ ] Package: Check for Upgrades is absent with `package_index_url: null`. With an explicit
  reviewer-controlled HTTPS version-1 index (schema in `vel-packages/README.md`), it appears.
  Nothing fetches until invoked. Available/current versions appear before an installation
  confirmation. Cancel retains the archive; confirmation checks digest/metadata and retains
  disabled state. Restore the setting to null after review.

VoiceOver: enable with **Cmd+F5** (or the system accessibility shortcut). **VO** below means
Control+Option. Use Accessibility Inspector alongside VoiceOver to inspect the public AX values.

- [ ] Interact with the workspace/text area (VO+Shift+Down). The editor has role `AXTextArea`,
  a document label and the visible/windowed text value. Read `Hello 你好 😀 é` in order. Arrow,
  Shift+Arrow and mouse selection changes announce their current caret/selection rather than
  stale text. Typing, native IME preedit/commit and Undo update the value and selection once.
- [ ] Inspect `AXSelectedText`, `AXSelectedTextRange(s)` and `AXInsertionPointLineNumber`.
  UTF-16 offsets count 😀 as two units; combining marks retain valid scalar boundaries. Reverse
  a multiline selection: insertion line follows the actual primary caret. Add multiple carets:
  AX exposes the primary only; typing still edits all editor carets. Setting an AX range updates
  the primary editor selection, and no half surrogate is exposed or replaced.
- [ ] Query `AXLineForIndex`, `AXRangeForLine`, `AXStringForRange` and `AXBoundsForRange` in
  Inspector. Include newline, end-of-document and empty ranges. Bounds align with text at 1×/2×,
  after horizontal/vertical scrolling, wrapping, folds, window resize, UI scaling and splits.
  Screen bounds use points; a Retina display must not double offsets or sizes.
- [ ] Open `project/windowed.txt`, move near the end and query its value. Each value has at most
  **65,536 UTF-16 units**; Help reports its document offset and first line. Selected/parameterized
  ranges are relative to that value; insertion line remains document-relative. Move the caret
  beyond the old window and confirm the next value/range queries track it. Whole-value AX
  replacement is unavailable for a partial window. No full-document flattening or UI pause.
- [ ] Traverse with Cmd+F6 / Cmd+Shift+F6 and VO navigation. Tabs expose document names, selected
  state and separate Close buttons; Return/Space or VO+Space activates focused controls. Visit
  both tabs, close/cancel a dirty tab, then switch groups. The primary editor follows the selected
  group. Closing the quick panel restores the editor’s focus.
- [ ] The side bar is an outline with labelled open-file/project rows, disclosure depth and
  expansion state. Select/open files and expand/collapse directories with AX actions. Hidden
  rows do not become duplicate elements. Goto/palette rows expose captions, badges/details and
  selected state; filtering and Enter/VO+Space choose the announced row.
- [ ] Find/Replace and Find in Files fields have distinct Find/Replace/Where labels. Regex,
  case, whole-word, wrap, selection and preserve-case controls have named actions; Find Next,
  Find All and Replace All buttons work. Standard Tab/Shift+Tab retains input routing; the
  explicit Cmd+F6 traversal includes non-text controls.
- [ ] Status text announces line/column, selections, encoding and current message. The syntax
  popup opens its picker. Completion rows, context-menu choices and visible minihtml popup
  text/links have semantic names/actions. Activate a popup link and confirm its normal action.
- [ ] System Settings → Accessibility → Display → Reduce motion, toggled **while running**:
  theme tint/opacity transitions and texture sequences settle immediately; no theme animation
  resumes on resource reload. Toggle Increase contrast: chrome control boundaries strengthen.
  Toggle back and confirm ordinary animations/appearance return. Custom syntax color contrast
  remains the scheme author’s choice.

Record macOS version, display scale, VoiceOver version/verbosity, exact failing action and AX
attribute values. GUI behavior, actual speech and screen-coordinate alignment are **pending
human review**; headless tests do not establish these outcomes. Binary hex documents currently
have a read-only description rather than a navigable AX hex-text value.

## Round 14: native opens, context menus and round-12 regressions

Run on an unlocked macOS 14+ desktop, outside the sandbox, using a bundle prepared by the
reviewer. This round does **not** run the packaging script. The headless plist test only
serializes document-type declarations; it does not assemble, sign or launch an application.
Replace the app path below with the reviewer's updated bundle. Use a new fixture directory.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --release
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps
/usr/bin/python3 -B -m unittest discover -s crates/velocity/packaging -p 'test_bundle.py'
cargo build --release --locked -p velocity
python3 crates/velocity/scripts/review-round14-fixtures.py /tmp/velocity-r14-native --huge

APP=/absolute/path/to/Velocity.app
FIXTURE=/tmp/velocity-r14-native
PROFILE="$FIXTURE/profile"
DOCS="$FIXTURE/Project with spaces"
# Cold LaunchServices launch: the filename travels in the open Apple event, not argv.
/usr/bin/open -na "$APP" "$DOCS/one.rs" --args --data-dir "$PROFILE"
# With the first instance still running, route to its key window.
/usr/bin/open -a "$APP" "$DOCS/two.txt"
# CLI locations still use the existing per-profile socket.
"$APP/Contents/SharedSupport/bin/velocity" --data-dir "$PROFILE" "$DOCS/one.rs:120:7"
```

- [ ] **Cold Finder Open With:** quit Velocity, then Finder → Open With → this bundle for
  `one.rs`. Repeat for `any-file.velocity-blob`, a folder, and `literal:12:3`. Text opens in
  a tab, binary data in read-only hex, folders in the sidebar, and the native colon filename
  remains literal. Text/data are Alternate Editor handlers; folders are Alternate Viewer
  handlers. Check the receiving process remains alive rather than handing off an empty GUI.
- [ ] **Double-click:** on a disposable fixture extension, Get Info → Open With → Velocity →
  Change All…, quit, and double-click. Verify cold and running launches. Restore the previous
  association afterward. Velocity must not take document ownership merely by being installed.
- [ ] **Dock drop:** drag several files and a folder onto Velocity's Dock icon, first cold and
  then running. While startup/loading is busy, issue another open. Requests retain order and
  are applied after window initialization. Drop a duplicate, a symlink alias and an edited
  open file: focus its existing tab while preserving its text, selections and scroll.
- [ ] **Window drop:** drop files/folders over the editor, tab row and sidebar, including
  Unicode/space-containing paths. Existing text dragging/input behavior remains usable. Drop
  `huge.log` and the binary: retain background prefix/huge-file and hex behavior. Use an
  inaccessible or subsequently removed file and confirm a path-specific error, with later
  valid entries still opened.
- [ ] **Running instance/windows:** open two windows, make the second key and use Finder Open
  With and `open -a` again. Only that key window receives the files. Close all windows without
  quitting, then click the Dock icon: create a fresh window. Repeat with a Finder open and a
  CLI request while no window exists. The retained template polls IPC without rendering.
  `velocity --data-dir "$PROFILE" -w "$DOCS/two.txt"` must return when its sheet/window closes.
  Force a secondary LaunchServices process with
  `open -na "$APP" "$DOCS/literal:12:3" "$DOCS/two.txt" --args --data-dir "$PROFILE"`:
  its queued batches must reach the primary before the temporary receiver quits. Repeat while
  the primary quits; takeover must retain all recovered windows and dirty documents.

Menu resources are original Velocity files in `Packages/Default`: `Context.sublime-menu`,
`Tab Context.sublime-menu` and `Side Bar.sublime-menu`. Enabled packages and User extend their
stable IDs with the same resource precedence as the menu bar. `velocity_dynamic` inserts the
existing spelling, diff and Git choices. Commands receive resource args plus literal `paths`,
clicked `group`, group-local `index`, and the window-local `event: {x,y}` point. Internal sheet
identity keeps targets stable while workers/IPC progress during tracking.

- [ ] **Native tracking:** right-click, two-finger click and Control-click on each surface.
  Verify native menu appearance, keyboard navigation, Escape dismissal, disabled rows and
  checks. Keep a menu open while indexing/loading and IPC complete; select an action, then
  type/scroll immediately. No borrow panic, hung tracking loop or menu-bar replacement occurs.
  During the spelling-worker wait, Escape, typing and scrolling cancel the deferred popup.
  A User menu `chain` and a window command such as `toggle_setting` inherit the clicked tab's
  context and check its state even when another tab was previously active.
- [ ] **Editor:** right-click inside either of two selections and Copy: preserve all regions.
  Right-click outside all selections: move the caret first. Repeat in an inactive split.
  Cut/Copy/Paste/Select Document, cached/worker spelling replacements, dictionary actions,
  diff show/revert and configured Git actions work on the pointed document/hunk/word. Copy
  Full File Path, Locate in Project Tree and Show in File Browser use the pointed document.
- [ ] **Tabs:** right-click an inactive tab and use Close This Tab, Keep Only This Tab, Close
  Following Tabs and Close Saved Tabs. Targets follow that group's visual order. Dirty
  prompts, Save/Discard/Cancel and shared views retain their usual behavior. Create Untitled
  File opens in the clicked group; path/reveal actions use the clicked tab.
- [ ] **Sidebar:** Open Selected Entry expands directories/opens files. Create File/Folder
  and Change Entry Name use the existing input panel; Escape cancels, an existing destination
  reports an error, and separators/traversal names are rejected. Rename an open dirty file and
  a parent directory: retarget all sheets/descendants, keep undo and watch the new path.
  Move Entry to Trash first asks confirmation; Cancel changes nothing. Confirm moves to Finder
  Trash, including symlink entries, with no permanent-delete fallback. Open buffers remain
  readable, marked `[deleted]` and dirty, and can be saved. A previously clean deleted buffer
  must prompt on close and survive Hot Exit; saving either shared view clears the markers.
  Clipboard/reveal/permission errors surface.
- [ ] Copy Full Path and Copy Project-Relative Path have the expected text. Search This Folder
  opens Find in Files with that directory in Where. Detach Project Folder removes the root
  without deleting its files; configured Git choices retain the selected path. Repeat with
  User menu overrides, disabled packages and a hot resource reload.
- [ ] **Accessibility:** VoiceOver reads native menu items and their disabled/checked state;
  no second custom context list appears in the workspace AX tree. Dismissal leaves the round-13
  editor/tab/sidebar/input AX elements and focus behavior intact.

Round-12 regressions and change checks:

- [ ] CLI `one.rs:120:7` centers line 120 at startup and over IPC, even after restoring an old
  scroll position. Repeat in a resized/split/wrapped view and while a large file is loading.
- [ ] In light and dark themes at multiple UI scales, the strip above the sidebar matches its
  themed background and contains no black clear-color rectangle.
- [ ] Window title is the file name plus dirty marker and optional ` — project`; switching,
  typing, saving and external changes update it. No debug/native AppKit title or encoding/EOL
  suffix. Unchanged frames do not call the native title setter.
- [ ] UI labels/headings use system sans-serif regular/bold/italic faces at their actual sizes.
  No CoreText message about requesting private `.SFNS-*` names or substituting Times appears.
  The renderer side is `vel-render/src/native.rs` (`font_location` uses the CoreText UI font for
  system names).
- [ ] A normal launch creates no interactive telemetry file. Quit, then opt in with the direct
  command below. Confirm the exact default name `interactive-<runid>-<pid>.jsonl`, valid JSONL,
  and the JSON mode/PID; no compile-time source directory receives output. Unset
  `VELOCITY_LOG_DIR` and repeat to check `~/Library/Logs/Velocity/`. Explicit `--output` is an
  opt-in destination, creates a new file and refuses to overwrite one.

```sh
VELOCITY_GUI_PROCESS=1 VELOCITY_LOG_DIR="$FIXTURE/logs" \
  "$APP/Contents/MacOS/velocity" --data-dir "$PROFILE" --log-input "$DOCS/one.rs"
```

- [ ] **Reviewer regressions:** Cmd+F/Replace/Incremental/Find in Files toggle; Escape from
  editor focus and each find panel's close button hide the panel/match paint without losing
  Find Next. Scroll and mouse moves stay coalesced on the display link; keys/clicks remain
  immediate. The resource-stamp walk runs at most every 500 ms; an explicit package reload
  takes effect immediately.
- [ ] **Real-event profile:** while scrolling a 10 MB file using the trackpad, sample the app
  (for example `sample PID 10 -file /tmp/velocity-r14-native/native-sample.txt`). Inspect
  native menu/title work: no per-frame `setMainMenu`/menu reconstruction or unchanged title
  mutation. Record receipt-to-submit p50/p95, display refresh rate and frame gaps using lane
  B's native scroll gate after integration. Headless CPU numbers do not establish native
  AppKit, font rasterization, GPU, scanout, VoiceOver or Finder/Dock outcomes.

Paired CPU reviewer commands (use the retained clean `4b65f50` binary as BEFORE):

```sh
python3 crates/velocity/scripts/measure-typing.py \
  --before /tmp/velocity-r14-baseline/velocity-main-4b65f50 \
  --after target/release/velocity --runs 10 --output-dir /tmp/velocity-r14-typing-review
python3 crates/velocity/scripts/compare-scroll.py \
  --old /tmp/velocity-r14-baseline/velocity-main-4b65f50 \
  --new target/release/velocity --pairs 10 --output-dir /tmp/velocity-r14-scroll-review
```

Run pairs sequentially with no concurrent compilation. Retain raw JSONL, binary hashes and
summary confidence intervals. Typing and scrolling each have a **≤5% regression** budget;
report CPU budgets separately from pending native measurements.
