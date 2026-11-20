# Spec: menu bar, settings and UI review; Simplified Chinese UI with a language switch

Status: P2, round 23 (after lane A round 22 and lane C's bug-fix round merge). Sections A-C come from a review of the
menu bar, the settings and the UI (findings are added below by the review). Section D is a Velocity extension: Sublime
Text has no built-in UI localization (its changelogs only mention file encodings such as GB18030; community packages
replace `.sublime-menu` files). We add a supported, switchable Simplified Chinese UI without breaking package menus or
key bindings.

Sources:
- https://www.sublimetext.com/docs/menus.html (menu files, entry keys, the seven customizable menus)
- https://www.sublimetext.com/docs/settings.html, https://docs.sublimetext.io/reference/settings.html
- https://docs.sublimetext.io/reference/commands.html, reference/keyboard_shortcuts_osx
- ST4 changelog https://www.sublimetext.com/download
- Apple Human Interface Guidelines (menus, the app menu, key equivalents) and NSMenuItem documentation.

Clean-room: never download, read or copy Sublime's Default package, menus, keymaps or themes. Translations are our
own work; do not copy community localization packages.

## A. Menu bar (known so far; the review adds more)

1. **Key equivalents are not shown** *(code-confirmed, P2)*. `vel-platform/src/macos.rs` `append_menu` sets a key
   equivalent only for Settings (⌘,). Sublime shows each command's key binding next to its menu item.
   - Derive the shortcut from the active keymap: the binding whose command and args match the item exactly and whose
     context is empty or always true. Prefer single-chord bindings; for multi-chord bindings (⌘K ⌘B) show nothing as
     a key equivalent (AppKit cannot) — keep them in the palette instead.
   - Show it as a real `NSMenuItem` key equivalent plus modifier mask, but make sure AppKit never fires it on its own
     (`performKeyEquivalent:` must still go through Velocity's key dispatch so contexts, Vintage and overlays win).
     Test the equivalent on non-US keyboard layouts (the keymap's key names vs the layout's characters).
   - Update when the keymap changes (User key bindings reload live).
   - Test: menu model rows carry the expected shortcut for a table of commands (New File ⌘N, Save ⌘S, Find ⌘F,
     Goto Anything ⌘P, Toggle Side Bar has a chord → none, …); a User key binding change updates the row.
2. **Missing customizable menus** *(code-confirmed, P3)*: `Side Bar Mount Point.sublime-menu` (top-level folders),
   `Find in Files.sublime-menu` (the "…" button in the Find in Files panel, the Where field helpers),
   `Widget Context.sublime-menu` (text inputs in panels: cut/copy/paste/select all, and Sublime's input history).
   Load them from packages with the same precedence as the other menus.
3. Review items (filled in by the review): menu order on macOS, the app menu (About, Settings…, Services,
   Hide/Hide Others/Show All, Quit), Help menu content, Window menu standard items, captions and ellipses, enabled and
   checked states for every row, submenus that are empty, dynamic menus (Open Recent, Syntax, Build System, Macros,
   Color Scheme, Theme, Encoding, Line Endings, Indentation, Layout), context menus.

## B. Settings (filled in by the review)

Coverage of `Preferences.sublime-settings` against the documented settings, comments, defaults, `docs/settings.md`,
the settings window (`edit_settings`), syntax-specific and project settings, live application, error reporting for
invalid JSON or wrong types, settings that are read but never applied (or applied but not documented).

## C. UI (filled in by the review; styling goes to the design agent)

## D. Simplified Chinese UI with a language switch *(P2)*

Goal: everything Velocity itself shows can appear in Simplified Chinese, switchable at runtime, with English as the
default and the fallback for anything untranslated.

1. **Setting and switch.**
   - `ui_language` defaults to `"en"`. It accepts a BCP-47 tag or `"auto"`, which selects the first supported
     macOS preferred language. Try the exact tag, then its parents; English is the final fallback.
   - Preferences › Language (语言) and "Preferences: Language: …" in the palette are generated from the catalog
     inventory plus English and Follow System. Picker names come from catalog metadata. The choices run
     `set_ui_language {"language": …}`, which writes the User preference.
   - Applies live: native menus are rebuilt, panels re-laid out, the palette and status bar update; no relaunch.
2. **Catalog.**
   - `Packages/<package>/Localization/<tag>.sublime-localization` is JSON with comments. The only shipped catalog is
     `Packages/Default/Localization/zh-Hans.sublime-localization`; English needs none. Catalogs declare
     `language_name`, optionally `english_name`, and a `strings` object.
     Keys are the English source text, so menu and palette files need no changes and third-party package captions
     still show (in English) when untranslated.
   - Users and packages may add or override translations: `Packages/User/Localization/zh-Hans.sublime-localization`,
     and any package's `Localization/zh-Hans.sublime-localization`, merged with the usual package precedence.
   - Rust UI strings go through `tr("…")` / `tr_fmt("… {name} …", &[("name", value)])`, looked up in the same catalog.
     Placeholders are named; a translation must use exactly the same placeholder set (tested).
3. **Coverage.** Menus (main, app menu, all context menus, dynamic menu captions that are not file or syntax names),
   command palette captions, quick panels and overlays (Goto Anything placeholders, "No matches", …), the find,
   replace, incremental find and Find in Files panels (labels, tooltips, buttons, status), the status bar (Line/Column,
   Spaces/Tab Size, encoding names stay as their standard names), dialogs and alerts (unsaved changes, reload, revert,
   errors), status messages, the settings window titles, build output headers, the about panel, accessibility labels
   and announcements (VoiceOver must read Chinese too). Not translated: file names, syntax names, scope names, setting
   keys, command names, color scheme and theme names, user content.
4. **Command palette search.** In Chinese mode, palette rows show the Chinese caption, and the fuzzy matcher matches
   both the Chinese caption and the English caption (so `save` still finds 保存). Highlights apply to whichever
   matched. Goto Symbol and file lists are unaffected.
5. **Layout.** Chinese strings differ in width; panels, buttons, tabs, the status bar and menus must lay out without
   clipping or overlap at ui_scale 1 and 2. CJK text uses the system font's CJK fallback (PingFang SC); check baseline
   and vertical metrics in buttons and the status bar.
6. **Quality of the translations.** Use standard macOS Simplified Chinese terminology (文件, 编辑, 选择, 查找, 显示,
   前往, 工具, 项目, 窗口, 帮助, 设置…), consistent terms across menus, palette and panels (a glossary in the catalog
   header), full-width punctuation where Chinese UI uses it (…, ：), no machine-translation artefacts. Keep menu
   captions short.
7. **Tests.**
   - Catalog completeness: every caption in the shipped menus and palette and every `tr`/`tr_fmt` literal in the
     source has an entry in every shipped catalog; no orphan entries; placeholder sets match. Partial third-party
     catalogs are valid and fall back per key.
   - Switching through the user's path: `run_named("set_ui_language", …)` changes the menu model, the palette rows,
     panel labels and a status message; switching back restores English; an unknown language falls back to English
     with a status note.
   - Render scenes in `zh-Hans` for the design agent (`--render-scenes … --language zh-Hans`), and a layout test that
     no label overflows its control in the main scenes.
   - Performance: translation lookups must not show up per frame (cache translated strings per language revision);
     retired instructions for typing and scene builds within 1%.

**Round 28 closing status:** The locale framework is data-driven, with English and Simplified Chinese still the
only shipped languages. A temporary User catalog exercises live resource publication and removal, menu and palette
generation, fallback, and translated panels, status, and accessibility. Completeness and draw-list audits enumerate
the shipped catalogs. See [the localization guide](../docs/i18n.md). The existing native VoiceOver and IME review
items below are unchanged.

The round-27 smoke1 comparison mixed deployment targets. The `e163180` reference targeted macOS 14 and used
Mach-O chained fixups; the round-27 binary targeted macOS 11 and used legacy rebasing and binding. A
`proc_pid_rusage` loader probe attributed roughly 5.6 million extra instructions to pre-main startup, not to
resource loading. Compare release binaries built with the same `MACOSX_DEPLOYMENT_TARGET=14.0` as the package;
`otool -l` can confirm both the minimum OS and the fixup format. No runtime or minimum-OS change is needed.

## Review findings (2026-11-19, main `340ca17`)

### Menu bar
1. **Key equivalents** (see A1). The fix: add `shortcut: Option<String>` to `MenuItem`/`MenuEntry`, derive it in
   `application_menu()` from the active keymap (single chord, empty or always-true context, exact command+args match;
   none for multi-chord), and `append_menu` sets it. Never rely on AppKit to fire it: `performKeyEquivalent:` already
   swallows ⌘/⌃ events and `offer_key` dispatches, so contexts, Vintage and overlays keep priority. Skip bindings without
   ⌘ (plain F-keys, ⌃G); AppKit would bypass key dispatch. Map key names to US characters and set the modifier mask so
   non-US layouts display correctly.
2. **App menu**: add `Settings…` (⌘,, id-based lookup), `Services` (`NSApp.servicesMenu`), `Hide Others` (⌥⌘H),
   `Show All`, in HIG order; keep About / Hide / Quit.
3. **Window menu**: Close (⇧⌘W), Move Tab to New Window, Merge All Windows.
4. **Help menu**: attach with `NSApp.setHelpMenu` so AppKit adds the search field; add Documentation, Release Notes,
   Open Crash Reports (crash reports already exist).
5. **Menu content**:
   - Open Recent becomes File / Folder / Project submenus; hide Clear Items when empty.
   - File: drop "Close All Files" (palette only), move Split View to View › Layout, add Convert to Encoding.
   - View: Show Console, Word Wrap and Show Minimap as checkable items; Enter Full Screen caption.
   - Hide empty submenus (Macros).
   - Tools: Developer › New Plugin / New Snippet / New Build System.
   - Remove View › Preferences; Color Scheme / Theme live only under Preferences.
6. **Checked states**: Layout (`set_layout`), bookmarks, Indentation (spaces vs tabs via `expand_tabs`/`unexpand_tabs`).
7. **Enabled states**: menu rows that currently error. Make `command_enabled` return false, or make the command a no-op
   with a status note:
   - `save` / `prompt_save_as` / save-with-encoding on an untitled buffer must open Save As, not error;
   - `toggle_comment` in Plain Text;
   - `fold` before analysis is ready;
   - `replace_in_files` before a Find in Files run;
   - `run_macro` while recording;
   - `add_folder` without args must prompt for a folder (functional bug);
   - `new_pane` when too small.
8. **Context menus**:
   - add `Side Bar Mount Point.sublime-menu` with a root/non-root distinction (`app_context.rs` only checks the path);
   - add `Find in Files.sublime-menu`, which needs the "…" button in the Find in Files panel for its options
     (in_selection, preserve_case, use_gitignore);
   - add `Widget Context.sublime-menu` for panel text inputs, which needs a right-click path in the panel input
     controls;
   - fix captions: Cut / Copy / Paste / Select All in the editor menu; one term for reveal-in-Finder;
   - add Copy as HTML.

### Settings
1. `set_translate_tabs` argument is ignored (`editing.rs:1973` `convert_leading_tabs`); true must also change
   `translate_tabs_to_spaces`.
2. `show_encoding` and `show_line_endings` default to false although they aren't listed in the shipped settings;
   Sublime shows encoding and line endings in the status bar. Default them to true in the shipped settings and
   `app_status.rs`.
3. `edit_syntax_settings` uses `Packages/Default/Preferences.sublime-settings` as its base file instead of the syntax's
   own defaults, so the two panes show unrelated files (`settings_entry.rs:222`).
4. Wrong types are silently ignored (`vel-config/src/settings.rs:192-215`). Collect mismatched known keys and report
   them once. Invalid JSON messages should name the file, line and column without the raw serde text.
5. Add missing documented settings: `default_dir`, `mouse_wheel_switches_tabs`, `result_file_regex`,
   `result_line_regex`, `result_base_dir`, `build_env`, `ui_font_face`, `show_at_startup`, `new_window_settings`.
   `index_exclude_patterns` semantics are covered.
6. `docs/settings.md` covers ~24 of ~200 keys. Generate the table from the shipped settings and cross-reference the
   read sites; mark keys that are read but never applied.
7. Add a settings-effect test: for each key, set a non-default valid value in the User layer, build the scene, and
   compare a scene digest. Keys with no difference are listed. This becomes a regression test.
8. `Preferences (OSX).sublime-settings` is nearly empty; add the macOS-specific values Sublime carries there
   (scroll speed, animation) or document why.

### UI text
- `format!("{:?}", kind)` shows Rust enum names (`panels.rs:1506`, `:1541`).
- Messages that leak internals: "command X needs a native window" (`app.rs:2075`), "Syntax scopes are unavailable…"
  (`editing.rs:1977`), raw serde errors, and "Syntax analysis is still running; retry folding shortly".
- Terminology: Side Bar versus Sidebar; Search in Files / Find in Files / Replace in Files; Show in File Browser versus
  Reveal in Finder; Open Find / Open Replace versus Find… / Replace…
- Status bar: ` · ` separators and abbreviations belong in the catalog.

### Simplified Chinese (prerequisites and plan)
1. **Prerequisite (must merge before i18n):** replace caption-based menu lookups with ids.
   - `app.rs:1106-1120` finds View and Syntax by caption; after translation the Syntax submenu would be empty.
   - `macos.rs:452` finds Preferences by caption; after translation Settings… would not reach the app menu.
   - Add `id: "help"` and `id: "preferences"` to `Main.sublime-menu`.
   - `release.rs:110` already falls back to id.
2. **String inventory** (deduplicated about 600-700, about 450 to translate): main menu ~180 (encoding names are not
   translated), context menus ~40, palette 192, dynamic palette entries ~35, panels ~80, status bar ~35, dialogs ~12,
   status messages 46, app menu 4 + new standard items, window titles ~8, build headers ~10, accessibility ~30.
3. **Translation difficulties:**
   - `format!` sentences (`panels.rs:2479`, `:2483`); use `tr_fmt` with named placeholders and no plural branch for
     Chinese.
   - `join(" · ")` separators should be in the catalog.
   - `push_str` fragments (`large.rs::hint`) must be translated inside the function.
   - HTML popup titles (`html_ui.rs`).
   - App name stays "Velocity"; translate only the verbs (关于 Velocity, 隐藏 Velocity).
4. **Live switching:** menus are rebuilt each time, so a language change must trigger `changed()` (settings observer).
   The app menu is built once in `build_menu`; rebuild it, or build it from resources plus `tr()`.
   Panels and the status bar regenerate every frame. The palette must be re-prepared if it is open. Accessibility
   labels regenerate per frame.
5. **Catalog:** `vel-i18n` crate with `Catalog`/`Language`/`tr`/`tr_fmt` and a per-language revision cache. Strings
   live in `Packages/*/Localization/zh-Hans.sublime-localization` (JSON with comments, English source text as keys,
   named placeholders, an optional glossary and package-prefix allow-list). Load them through the normal resource
   precedence, with User overriding packages and Default. The format from section D stands; validate placeholder sets.
6. **Bilingual palette matching:** add `alias: Option<Candidate>` to palette candidates (prepared once when the palette
   opens, about 0.1 ms for 192 rows), score both and take the better; highlight columns come from the caption that
   matched. CJK matching needs no matcher change: `normalized()` uses `char::to_lowercase`, and boundaries are
   per UTF-8 byte.
7. **CJK rendering was verified good:** the fallback to PingFang via `SystemFontFallback` works, ascent and descent
   take the run maxima, the editor grid gives CJK 2 cells, and status-bar and palette captions rendered without
   clipping at ui_scale 1 and 2 in dark and light. No renderer change is needed; add Chinese scenes as regression.
8. **Sequencing:**
   1. id-based menu lookups and the app menu built from resources;
   2. `vel-i18n` plus menus and palette in Chinese, with live switching;
   3. status messages and panel labels;
   4. dialogs and window titles;
   5. bilingual matching;
   6. accessibility labels and alerts;
   7. Chinese render scenes and the design pass.

## Review findings (round 24)

Reviewed Velocity's expanded menu trees and context resources without opening
windows. The comparison was behavioral and clean-room: [Sublime Text 4 release
notes](https://www.sublimetext.com/blog/articles/sublime-text-4), [tab
multi-select](https://www.sublimetext.com/docs/tab_multi-select.html), the
[public changelog](https://www.sublimetext.com/download), and [Apple's
menu guidelines](https://developer.apple.com/design/human-interface-guidelines/menus).
No Sublime menu files or artwork were consulted.

| Surface | Finding and disposition |
| --- | --- |
| Velocity / Window | Put a divider between Settings and Services; keep Hide, Hide Others and Show All together, with Quit in the last group. AppKit still owns Services, About, the Hide family, window-list rows, native window actions, and the Help search field. About and New Window retain the standard direct-action labels. |
| File | Move Split View here, separate from layout presets. Group creation/opening, encoding, saving and closing. Revert warns when applicable; its label now has an ellipsis. Recent paths and standardized codec IDs are data. Print and Open Folder in New Window still need native workflows; there are no placeholder rows. |
| Edit / Selection | Add Paste and Indent to Edit and the editor popup; leave Select All in Edit. Move match selection and the Tab Selection submenu into Selection. Keep indentation actions in Line; Wrap Selection With Tag requires a writable text selection. Copy as HTML remains a known gap: a rich clipboard export is a separate feature, not a mislabeled plain-text Copy. |
| Find / Find in Files | Separate document-scoped In Selection from file search. It is disabled without an original scope, even if Find highlighted a match itself. The Where popup offers Current File, Open Files and Open Folders without altering the query. Direct scope choices replace Where; Preserve Case remains a toggle. The unsupported per-search Use .gitignore switch is not shown. |
| View | Keep the layout presets, line endings, display toggles and syntax tree. New/Close Group live under Groups. Remove duplicate tab navigation and font controls; verify effective wrap (including auto), spell check, minimap, tabs, sidebar and status bar checkmarks. |
| Goto / Tools | Keep next/previous file under Switch File and put next/previous search or build result under Goto > Results. Tools keeps Show Build Results and one New Build System row. Build System and Macros remain dynamic. |
| Project | Edit/Close require an active project; Recent/Quick Switch require history. Add Folder and Save Project As can start a new project. |
| Preferences / Help | Keep these in their macOS roles, with Window and Help trailing. Add ellipses to comparison-window and Indexing Status actions. The settings editors remain in Preferences apart from the app-menu Settings entry. |
| Editor / sidebar / mount / tab popups | Match editor and tab labels for Copy File Path and Reveal in Side Bar. Keep folder-specific operations on the mount/sidebar, and close-by-target on the tab. Reveal in Finder is consistent across surfaces. Widget history, spelling suggestions and third-party labels still preserve user/package text. |

The palette has 14 original 16-DIP command masks at 1×, 2× and 3×, including
the File document and Edit pencil. Developer actions share the Tools/Build
family. Textures use the same theme ink as file icons. Command identity comes
from the source family, not the winning caption or alias. Existing typed file
icons and symbol badges are unchanged; a missing texture uses the neutral dots.
Appearance previews retain their own row layout.

The zh-Hans pass covers menu rows, encoding descriptions, palette commands,
authored prefixes in dynamic pickers and quick-panel footers. Names of macros
and other resources, paths, history and standardized codec labels are preserved
verbatim. Scattered English alias hits stay below direct and word-aligned hits.
The Find in Files Where button shares Close's center line and hover treatment
and has a localized tooltip. Other panels, the status bar, dialogs, window
titles and accessibility remain out of scope for this pass.

## Closing status (round 26)

The round-24 menu decisions supersede earlier placement suggestions. “Done” refers to the
implemented behavior and the checked model; it does not imply a live AppKit test. Software
package names, paths, syntax and scope names, author content, and underlying compiler or
third-party diagnostics are data, not catalog strings.

| Finding | Status | Evidence or remaining work |
| --- | --- | --- |
| Menu bar 1: key equivalents | Partly | Round 23 derives single-chord shortcuts from the live keymap, including exact arguments and contexts; AppKit does not own command dispatch. Check the display and dispatch on real non-US layouts. |
| Menu bar 2: app menu | Done in round 24 | Settings, Services, the Hide family and Quit are grouped in native order; Settings uses the Preferences ID. |
| Menu bar 3: Window menu | Done in round 23 | Close Window, Move Tab to New Window and Merge All Windows are wired; availability follows the transferable sheets and destination windows. |
| Menu bar 4: Help menu | Done in round 23 | Help is registered with AppKit; the model includes Documentation, Release Notes and Open Crash Reports. |
| Menu bar 5: contents and dynamic rows | Done in round 24 | File/Folder/Project recents, encoding conversion, layout, display state, dynamic macros and Preferences are in the reviewed locations. Split View ultimately belongs in File. |
| Menu bar 6: checks | Done in round 24 | Layout, bookmarks, indentation and display checks use effective state. |
| Menu bar 7: availability | Done in round 24 | Save As for untitled files, Add Folder prompting, and the comment, fold, replace, macro and new-group guards are wired. |
| Menu bar 8: context menus | Partly | Mount, Where and Widget menus are loaded and routed; Clipboard captions and Copy as HTML are done. A per-search Use .gitignore control needs a real option threaded through traversal; no inert row is shown. |
| Settings 1: `set_translate_tabs` | Done in round 23 | The conversion updates the indentation preference when requested. |
| Settings 2: status format defaults | Done in round 23 | Both encoding and line endings default to visible and have status-menu tests. |
| Settings 3: syntax defaults editor | Done in round 23 | The read-only side shows the syntax's own defaults rather than the general preferences. |
| Settings 4: invalid types and JSON | Done in round 23 | Known type mismatches are reported once; JSON diagnostics identify the source and position. |
| Settings 5: missing keys | Partly | Supported file, build, UI and next-window preferences are shipped. `show_at_startup` was removed as undocumented rather than shipped as a no-op; adding it would require defining a real startup behavior. |
| Settings 6: reference | Done in round 26 (lane C) | Generated reader-linked reference, explicit lifecycle notes and a stale-reference check. |
| Settings 7: setting effects | Partly | The round-26 matrix applies all 184 shipped keys with readers, compares scenes, and runs actual save, build and session operations. Full-screen mode is documented as next native window transition. Some worker settings are checked at their resolved engine inputs; full behavioral coverage would require per-option completion, index, highlighting and annotation fixtures. |
| Settings 8: macOS overlay | Done in round 23 | Platform preferences document the deliberate macOS scroll/animation defaults rather than copying the Sublime package. |
| UI text: debug enum captions | Done in round 26 | Authored display names replace debug enum spellings, including package inventory resource kinds. |
| UI text: internal error copy | Partly | Common action and settings errors use readable messages. Raw lower-level or third-party details can still surface; structured diagnostics are needed for further translation without losing the cause. |
| UI text: terminology | Done in round 24 | Side Bar, Find in Files, Reveal in Finder, and Find/Replace labels are consistent on reviewed surfaces. |
| UI text: status separators and abbreviations | Done in round 25 | Localized status templates assemble format, position and selection pieces; codec IDs remain standard. |
| Chinese 1: menu IDs | Done in round 23 | View/Syntax and Preferences/Help are role- or ID-based. |
| Chinese 2: inventory | Done in round 26 | The generated literal, menu and palette checks are backed by the draw-list audit with a commented data allow-list. |
| Chinese 3: interpolated, fragment and minihtml copy | Partly | Named templates, status fragments and the definition/scope popups are localized. Raw diagnostics and HTML authored by packages are not translated; untranslated app-owned error paths outside the fixtures still need review. |
| Chinese 4: live switching | Partly | Menus, palette, panels, status, titles and accessibility model update live. An already-open scope popup or an already-produced package report/choice list can retain its original text; refresh those snapshots on the language revision. |
| Chinese 5: catalog and precedence | Done in round 23 | Default, package and User translations use validated named placeholders and revision-based lookup. |
| Chinese 6: bilingual palette | Done in round 24 | Caption and English alias both match, with word-aligned alias highlights and command identity preserved. |
| Chinese 7: CJK rendering | Done in round 26 | Bilingual offscreen scenes, measured CJK advances, and layout regression checks; the separate design lane owns the final visual review. |
| Chinese 8: sequencing and accessibility | Partly | Model coverage includes translated labels and announcements. Live VoiceOver speech and the IME candidate window were not run: test with VoiceOver and a Simplified Chinese IME in a native window at both scales. |
| Round 24: Velocity / Window | Done in round 24 | AppKit-owned roles and native menu grouping were retained. |
| Round 24: File organization | Done in round 24 | Split View, Save/Open/Close groups, and Revert confirmation/caption match the reviewed model. |
| Round 24: Print | Not done | Needs a native print or print-preview workflow and lifecycle-managed HTML (the exporter alone does not print). |
| Round 24: Open Folder in New Window | Not done | Needs a folder-only picker and a new-workspace request that transfers the chosen root without adding it to the current window. |
| Round 24: Edit / Selection | Done in round 26 | Round-24 reshuffle and availability checks plus Copy as HTML in Edit, the editor popup and the palette. |
| Round 24: Find / Find in Files | Partly | Selection scope, Where shortcuts and Preserve Case are done. Per-search .gitignore policy is still missing; the control is intentionally absent. |
| Round 24: View | Done in round 24 | Layout, groups, wrapping and other display checks follow the view model; duplicate controls removed. |
| Round 24: Goto / Tools | Done in round 24 | Search and build results share Goto › Results; build and macro menus remain dynamic. |
| Round 24: Project | Done in round 24 | Project requirements and history predicates gate the corresponding rows. |
| Round 24: Preferences / Help | Done in round 24 | Native menu roles, settings entry points and dialog ellipses are in place. |
| Round 24: editor, sidebar, mount and tab popups | Done in round 24 | Shared labels and targeted actions; spelling/history/package data are unchanged. |
| Round 24: palette icons | Done in round 24 | Authored command-family masks with theme tint and neutral fallback. |
| Round 24: zh-Hans pass | Done in round 26 | Rounds 25–26 added the ancillary UI copy and audit beyond the original menu/palette pass. Live checks and snapshot exceptions are recorded above. |

Copy as HTML exports nonempty regions in document order (a caret-only selection means the
whole document). It preserves tabs and the document's line-ending convention in plain text
and uses inline styles in HTML because consumers can discard the document stylesheet.
The synchronous path caps the required syntax prefix at 256 KiB and 4,096 lines, uses a
250 ms cooperative syntax budget, and limits HTML to 2 MiB. If it cannot produce a complete
highlighted item within those bounds, it leaves the clipboard intact. Plain Copy remains
available for a bigger selection. Native consumer-by-consumer paste (Mail, Pages, Notes) was
not exercised in this headless round.

For the text audit, run `velocity --render-scenes DIR --language zh-Hans --audit-text`.
It writes `cjk_runs` and unexpected `ascii_runs` into the layout JSON, without Metal or
windows. The Rust regression uses the same classifier.

## Parity follow-up (lane B)

File > Print opens an active-scheme HTML snapshot in the default browser. The worker
handles the current selection or the whole document, and the window gives the browser
a grace period before reclaiming the temporary file. Open Folder in New Window uses
the folder picker and constructs a separate workspace without mounting in the origin.
The Where menu's Use .gitignore check defaults on, persists in the window session, and
filters disk traversal without filtering explicit live-buffer scopes.
See [parity evidence](../docs/parity.md) for tests and separate Goto Anything behavior.

## Rules

- One codex lane implements A, B and D after lanes A and C merge (it touches many files). The design agent styles C
  and reviews the Chinese layout. Keep the catalog and `tr` plumbing in their own module (`crates/velocity/src/i18n.rs`
  or a small crate `vel-i18n`).
- No regressions: fmt, clippy `-D warnings`, `cargo test --workspace --locked --release`, rustdoc `-D warnings`.
