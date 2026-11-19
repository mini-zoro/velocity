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
   - New setting `ui_language`: `"auto"` (follow macOS preferred languages: Chinese, Simplified → `zh-Hans`,
     otherwise English), `"en"`, `"zh-Hans"`. Default `"auto"`? — **default `"en"`** to keep today's behaviour for
     existing users; `"auto"` documented. (Decide after checking how macOS apps usually behave; record the choice.)
   - Preferences › Language (语言) submenu with three checked rows: English / 简体中文 / Follow System (跟随系统),
     running `set_ui_language {"language": …}`, which writes the User preference.
   - Palette entries: "Preferences: Language: English", "Preferences: Language: Simplified Chinese" (and their Chinese
     captions).
   - Applies live: native menus are rebuilt, panels re-laid out, the palette and status bar update; no relaunch.
2. **Catalog.**
   - `Packages/Default/Localization/zh-Hans.sublime-localization` (JSON with comments, our own format):
     `{"strings": {"File": "文件", "New File": "新建文件", "Replaced {count} occurrences": "已替换 {count} 处", …}}`.
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
     source has a `zh-Hans` entry; no orphan entries; placeholder sets match. (Collect `tr` literals with a test that
     scans the sources, or with a `build.rs`-generated list.)
   - Switching through the user's path: `run_named("set_ui_language", …)` changes the menu model, the palette rows,
     panel labels and a status message; switching back restores English; an unknown language falls back to English
     with a status note.
   - Render scenes in `zh-Hans` for the design agent (`--render-scenes … --language zh-Hans`), and a layout test that
     no label overflows its control in the main scenes.
   - Performance: translation lookups must not show up per frame (cache translated strings per language revision);
     retired instructions for typing and scene builds within 1%.


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

## Rules

- One codex lane implements A, B and D after lanes A and C merge (it touches many files). The design agent styles C
  and reviews the Chinese layout. Keep the catalog and `tr` plumbing in their own module (`crates/velocity/src/i18n.rs`
  or a small crate `vel-i18n`).
- No regressions: fmt, clippy `-D warnings`, `cargo test --workspace --locked --release`, rustdoc `-D warnings`.
