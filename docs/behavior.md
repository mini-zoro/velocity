# User behaviour: decisions and audit

Rule: behave like Sublime Text 4. Where Velocity adds something Sublime does not have
(the graphical Settings page, the Simplified Chinese UI), the addition must not change
how a Sublime user's habits work. Sublime's default values live in its Default package,
which the clean-room rule forbids us to read; where a default cannot be confirmed from
public sources, the decision is the user's and is recorded here.

## Decisions (user, 2026-11-20)

| # | Topic | Decision | Status |
| --- | --- | --- | --- |
| D1 | ⌘, | **Revised 2026-11-23, a deliberate departure from Sublime:** ⌘, (Ctrl+, on Windows) opens the graphical Settings window, as in Typora and most Mac apps. The Default/User JSON pair stays one step away: Preferences › Settings (JSON)…, Preferences: Settings (JSON) in the palette, Ctrl+K, Ctrl+, on Windows, and the Edit buttons in the window. (Round 31 had ⌘, open the JSON pair.) | Done (Settings window, 2026-11-23) |
| D2 | Default UI language | `ui_language: "auto"`: the first supported macOS preferred language, otherwise English. Explicit language preferences override it. | Done (round 31 lane B) |
| D3 | Find in Files results | A "Find Results" tab like Sublime: scrollable, editable, double-click / Return jumps to the match; a "Use Buffer"-style toggle in the panel switches to the bottom output panel. | Done (round 31 lane C) |
| D4 | Opening files from Finder / Dock | `open_files_in_new_window: "never"` stays the default: files open as tabs in the current window. | Done (unchanged) |
| D5 | Closing windows / relaunch | Closed windows do not come back at launch; quitting restores the windows open at quit (`hot_exit`). Closing a non-project window with unsaved changes prompts. | Done (round 28 lane A) |
| D6 | Tab and chrome style | **2026-11-24, a deliberate departure from Sublime's default:** the Velocity theme looks like a Mac app. `file_tab_style` defaults to `"rounded"`, which now draws floating card tabs (Safari/Xcode style): the selected tab is a rounded card in the editor's colour with a hairline, and the focused group's card casts a soft shadow while other groups' selected cards are half-filled; no dividers or accent bar; hover is a rounded wash; close buttons are circular. `"angled"` and `"square"` keep the previous strip. Side-bar and list selections are inset rounded pills that take the system accent while focused; quick panels, popups and inputs are rounded with soft shadows and an accent focus ring. Third-party themes render as before. | Done (2026-11-24) |
| D7 | Default caret | **2026-11-25, a deliberate departure from Sublime:** `caret_style` defaults to `"blink"`, with a visible and a hidden half-second phase on macOS. Windows follows the system's interval. Editing and selection changes restart the visible phase; inactive or occluded windows do not animate the caret. All previous styles remain available. Reduce Motion makes `smooth` blink; the system's non-blinking preference makes the caret solid (macOS 15+ and Windows). | Done (2026-11-25) |

## Velocity-only additions (kept, opt-in or out of the way)
- Graphical Settings window (`specs/vel-settings-ui.md`, `docs/settings.md`): a standalone, single-instance utility window opened by ⌘, / Ctrl+, (D1). Curated pages, an Advanced page with every setting, and search over all settings. It writes the same User file as the JSON editor, which stays available from the menu, the palette and the window.
- Simplified Chinese UI and the i18n framework (`docs/i18n.md`); English text and behaviour are unchanged.

| Addition | Behavior |
| --- | --- |
| Finder: New Text File (round 45) | Opt-in Finder Sync action for the desktop, folder backgrounds and selections. The Finder menu follows the system language; the empty, uniquely named `.txt` file follows Velocity's UI language. Velocity opens it as a tab in the current window (D4), or opens a new window if needed. File creation errors appear in the status bar (U18). |
| Inline tab rename (round 46) | Double-click a permanent local text-file tab or choose Rename… / 重命名… from its context menu. The stem is selected in the tab; Return, clicking away, or leaving the key window accepts, and Escape cancels. The same exclusive rename as the side bar updates clones, other windows, lazy tabs, recents and the session without saving or discarding edits. Failures stay in the field and appear in the status bar (U18). Preview double-click still only promotes; double-clicking the empty strip still opens a new file. Settings, Find Results, read-only, binary/image and non-local sheets are not eligible. **Untitled tabs (user decision, 2026-11-23):** the same double-click or Rename… names the tab in place, like Sublime's `view.set_name`. The whole automatic first-line name is selected. Nothing is written to disk, and the name is the default name when the tab is first saved. It survives hot exit and session restore (`Sheet.name`). An empty name returns to automatic first-line naming. **Restart clarity (user decision, 2026-11-25):** an untitled tab's displayed name (normally its first line) is also written to the session (`Sheet.untitled_name`) and shown immediately after a restart, before the stored text is materialized; previously inactive untitled tabs showed the placeholder name until clicked. Opening the tab drops that stored name, and the text supplies it again, so editing the first line still renames the tab. |
| Long lines still wrap (2026-11-25, bug report) | A line wider than the viewport (at or over the 16 KiB long-line threshold) keeps `word_wrap: auto` wrapping while the document itself is modest. Before this, the long line disabled wrapping, so pasting text whose last line was about 169k columns left the viewport scrolled past every line and visually blank. Huge documents (at or over `large_file.viewport_bytes` or `viewport_lines`) and documents at or over `large_file.wrap_bytes` still keep wrapping off. |
| Tab click in an inactive window (2026-11-25, bug report) | One click on a file tab in a Velocity window that is not key selects that tab, then the window becomes key; previously the first click was spent on activation and the tab changed only on the second click. The tab-bar buttons answer the same way. Clicks in text still only activate, and `click_through` keeps opting the whole view in. |
| Rename field shows the first typed character (2026-11-25, bug report) | Typing over a long selected name (an untitled tab's whole first line) replaces it, and the field scrolls back so the new text shows from its start. Previously the field kept the old scroll position, so the first character sat just left of the visible edge. A single-line field never hides text on the left while it has room on the right; text wider than the field still keeps the caret inside the right edge. |
| Hidden windows hold no growing GPU memory (2026-11-25, bug report) | A minimized, hidden or fully covered window cannot get a drawable, so its frame is dropped after the scene is staged. The staged writes are now submitted on their own, so memory stays bounded, and such a window no longer retries at 60 Hz: it keeps its ordinary 200 ms poll and redraws when it becomes visible. Before this, a window left minimized grew by megabytes per second (2.5 GB peak observed) and used about 14% CPU until it was shown again. |
| Runtime memory guard (2026-11-26) | On the next idle tick after OS memory pressure, or once after a minute in the background or with every window hidden or minimized, Velocity releases rebuildable caches. A quiet editor also checks process footprint at most every 30 seconds and trims above 1 GiB plus twice the loaded document bytes, at most once every five minutes. The watchdog logs byte counts to stderr only. Documents, undo, unsaved state, view state, find history and published results are kept. |
| Shared rendering resources (2026-11-26) | Editor and Settings windows reuse the device, pipelines, glyph atlas and font caches. Each window keeps its own size, display scale and font settings. Memory pressure can release the shared atlas once; later draws rebuild glyph placements without changing the scene. Closing a window releases its surface and frame buffers. |
| `-b` cold start stays in the background (2026-11-26) | `velocity -b FILE` with no running instance opens the file without activating Velocity. Before this, AppKit read a word left over in the GUI child's launch arguments (the `--data-dir` folder after `--background`) as a document to open. That arrived as a foreground Finder open, which activated the app and added the profile folder. Velocity no longer treats launch arguments as documents: command-line files already arrive through the CLI request, and Finder, Dock and `open` deliver theirs by Apple event. |
| Covered windows release their first frame (2026-11-26) | A window that opens behind other windows, minimized or on another Space no longer keeps its prepared first frame (an offscreen texture of the window's pixel size, about 15 MB for 1200×832 on a Retina display) until it is shown. The wait for that frame to reach the screen only ended from a frame callback, which a covered window never gets; idle ticks now end it after the same 1 s, and showing the window draws a normal frame. |
| In-app updates (2026-11-27) | Velocity checks stable GitHub releases at most daily, after a 30-second minimum startup delay, unless `update_check` is off. The non-modal notice offers update actions. A verified update can restart through Hot Exit on macOS or Windows portable; Windows installer and unwritable builds link to the release. |
| Input method composition (2026-11-26) | Marked text is an underlined preview in the focused editor or field, with the IME's selection. In the editor, preedit appears at every caret, replacing any selections visually; the native marked range and candidate window belong to the primary caret. A normal commit applies at every caret as one undo step. Escape or empty preedit cancels; Return used by the IME does not submit a field. Leaving the surface discards preedit, except that inline tab rename accepts the displayed text on blur. The candidate anchor follows the focused field or editor viewport. |

The public [Search and Replace guide](https://docs.sublimetext.io/guide/usage/search-and-replace.html#results-format)
documents the separate view, context and match navigation. The [theme reference](https://www.sublimetext.com/docs/themes.html#icons)
distinguishes a buffer from an output panel. The [changelog](https://www.sublimetext.com/download)
documents ordered results and long-line truncation. The default-on setting, append/reuse
policy, exact report text, and Command-click behavior are Round 31 implementation choices
not established by these sources or decided by D3. No separate public Use Buffer menu or
palette entry was confirmed.

## Audit

Round 32, lane C, from `6dc73c6`; Sublime Text 4 on macOS. The reference point is
stable build 4215, with older changelogs used where they explicitly describe a behavior.
The community guide is useful for workflows; its cross-platform examples do not
establish macOS key bindings or current defaults. Public docs, API and release notes
only: no Sublime packages, menus, keymaps or binaries. Velocity's own shipped resources
are used as part of the tests.

This is a workflow audit, not a replacement for the [capability matrix](parity.md).
Named tests are executable evidence; a code reference alone means the path was
traced, not observed in AppKit. Tests dispatch shipped keys, built menu invocations,
mouse events, and native-open payloads without creating windows. Native picker,
LaunchServices/Dock delivery, Trash, dialog button placement, and OS focus are only
checked up to their platform boundary. A verdict of **same** is scoped to the
observable behavior stated in that row, not undocumented defaults, wording or pixels.
Estimates below are developer time with headless coverage; native review is separate.

The everyday edit, search, save and build workflows are broadly consistent with
Sublime. The main usability risks found were replacing a familiar keyboard entry point
(D1, fixed in round 31), losing the user's position on a routine open (B01, fixed here), and conflating closing
with quitting (D5, fixed in round 28). Keep the graphical Settings page optional and keep save or
discard decisions explicit. Where public sources don't establish the default or
the exact gesture, keep the existing behavior until it can be discussed.

Audit totals: **39 same**, **6 differs on purpose**, **13 differs — bug** (4 fixed in round 32,
9 scheduled; B10 was fixed by lane B in round 31 and is counted as same), and **19 uncertain**.

### 1. First launch

| Step | Sublime (source) | Velocity (test or code) | Verdict |
| --- | --- | --- | --- |
| Fresh window, untitled buffer, sidebar and welcome | Sublime Text 4215, fresh safe-mode profile on macOS (2026-11-22): opening one or three files gave one or three tabs and no sidebar. Opening only a folder showed FOLDERS and an editable empty editor (line 1, caret) without a tab, titled `untitled — <folder>`. [Build 4126][r4126] documents reusing the empty startup window. | `r41_fresh_input_is_not_a_document_until_edited`, `r41_open_files_and_folders_reuse_the_startup_input`: no welcome screen; folder-aware sidebar and an editable tabless buffer until the first edit. | **same** (U01, round 41 lane B) |
| UI language | No per-language startup UI is specified in [Sublime's settings guide][settings]. | Ships `ui_language: "auto"` since round 31: the first supported macOS preferred language, else English (`Workspace::sync_ui_language`, preferred-order tests). | **differs on purpose** (D2 and the localized UI addition; done in round 31) |

### 2. Opening

| Step | Sublime (source) | Velocity (test or code) | Verdict |
| --- | --- | --- | --- |
| ⌘O and File › Open Recent | [Command reference][commands] describes the open dialog; [Build 4143][r4143] documents macOS recents with no window. | `r32_open_dialog_and_recent_menu_focus_without_repositioning` goes through ⌘O with fixture picker results and a dynamically generated recent-file menu command. `r20_two_windows_recents_frame_and_close_rule` checks shared and system recents; `App::on_named_command` handles the no-window case. | **same** (dialog/recents dispatch; native picker not opened) |
| Finder, file dropped on Dock icon | [Settings reference][settings-ref] documents the macOS Finder/Dock routing switch. [Build 4126][r4126] documents Finder fixes, not the factory setting. | `route_matches_exact_roots_then_reuses_startup_and_respects_origin`, `two_apps_route_cli_and_finder_symlink_alias_to_the_original_window`: defaults to the current window for new ordinary files; already-open documents can take focus in their owner window. | **differs on purpose** (D4 preserves `"never"`; the Sublime factory value is not inferred) |
| Drag files/folders onto the window | [Build 4126][r4126] notes resolving macOS aliases on drag and drop. | AppKit path-drop callbacks forward literal paths through `App::on_open_paths` and `open_requests`; native path tests cover files, folders, literal colons and filesystem symlinks, not Finder alias objects or drop zones. | **uncertain** (U02) |
| Reopen a file already open, without a position | [Build 4143][r4143] records the macOS fix for jumping to the beginning on reopen. | Finder/Dock, ⌘O, Open Recent and bare CLI paths all use the same duplicate-open rule in `Workspace::open_mode`. `duplicates_focus_existing_tab_and_preserve_unsaved_edits`, `r32_open_dialog_and_recent_menu_focus_without_repositioning`, and `r33_cli_local_forwarded_and_native_reopen_preserve_position` cover sheet identity, selection and viewport. | **same (fixed in round 33 lane C)** (B01/U03) |
| CLI `-n`, `-a`, folder or `--project` | [CLI][cli]: force a new window, add folders to current window, or select a project; [Build 4126][r4126] notes cold `-n` reuse of the empty startup window. | `parse_window_flags_command_arguments_and_safe_mode`, `route_matches_exact_roots_then_reuses_startup_and_respects_origin`, `two_apps_route_to_existing_folder_project_and_file_without_replacing_dirty_tabs`. | **same** |
| CLI `-w`, explicit `file:line:col`, `-` | [CLI][cli]: wait until closed, optional location, and edit stdin (with stdout redirection when requested). | `real_cli_wait_interrupt_is_client_only_and_literal_colon_wins`, `multiple_files_stdin_and_one_based_unicode_locations`, `cli_stdin.rs` and `Workspace::open_stdin`. The picker regression also checks that a supplied CLI position still wins. | **same** (terminal refocus is not tested) |
| CLI reopen with no position vs explicit `:1:1` | [CLI][cli] documents optional coordinates; [Build 4143][r4143] confirms preserve-position behavior on macOS reopen without naming the CLI route. U03 applies that behavior to the CLI. | `r33_cli_local_forwarded_and_native_reopen_preserve_position` checks bare paths (including literal colons) over the single-instance socket and explicit line/column jumps, including `:1:1`. | **same (fixed in round 33 lane C)** (U03) |
| Open larger text, binary or a supported image | [Build 4126][r4126] identifies a distinct binary view status; [Build 4180][r4180] documents image views and their file patterns. The notes don't specify large-text thresholds. | `Tab::load_disk` streams large files with a read-only prefix; `r20_real_loader_opens_image_and_keeps_nonmatching_hex` checks decoded PNG vs read-only hex. | **same** (ordinary text/binary/PNG open outcome; threshold policy below) |
| More image formats and explicit Open as Image/Text | [Build 4107][r4107] (PSD/TGA), [Build 4151][r4151] (WebP), [Build 4180][r4180] (explicit open-as commands and image-tab context menu). | B04: bounded TGA and macOS WebP previews; File, palette and tab context commands remember the override by file for the running session. `r33_open_as_dispatch_enablement_and_session_choice` and decoder fixtures cover dispatch, hex fallback and limits. PSD preview is not supported. | **same (fixed in round 33 lane B)** for TGA/WebP and opening as image/text; PSD is excluded. |

### 3. Tabs

| Step | Sublime (source) | Velocity (test or code) | Verdict |
| --- | --- | --- | --- |
| Sidebar single-click preview, double-click/edit pins | [Navigation guide][nav] distinguishes replaceable italic sidebar tabs; [Build 4213][r4213] confirms double-click still works with `preview_on_click` off. | `Panels::mouse`, `Workspace::make_permanent`, `round18_review_sidebar_preview_palette_cancel_does_not_commit_recent_or_session` and `settings_effect_tests` cover preview replacement, option, pinning and not adding uncommitted previews to recents. Editing promotes the active transient sheet. | **same** (sidebar journey) |
| Goto preview vs sidebar preview | The [navigation guide][nav] distinguishes a tab-less Goto preview from an italic sidebar tab; confirmed in Sublime Text 4215 on macOS (U04). | An unopened Goto result replaces the editor view without a tab; an existing result selects its tab. The sidebar still uses an italic transient tab and honors `preview_on_click`. `r41_tabless_scene_and_commit_preserve_view`, `r41_sidebar_transient_tab_and_goto_click_acceptance`. | **same** |
| Placement of a new tab after clicking a middle tab | [Tab multi-select][tabs] defines placement for particular modifier gestures, not ordinary insertion order. | `Workspace::new_text_tab` and `Workspace::open_mode` append to the active group; first normal file replaces a pristine startup tab. | **uncertain** (U05) |
| ⌘W on last tab; blank named project | [Settings reference][settings-ref] documents the close preference; [Build 4121][r4121] explicitly preserves an empty project window with the option on. | `r32_cmd_w_keeps_a_named_project_without_folders`: empty named project stays open; empty anonymous window with the option on requests close. `final_close_leaves_no_sheet_and_typing_or_new_file_materializes_on_demand` covers staying open. | **differs — bug** (B02, fixed project exception) |
| Default for `close_windows_when_empty` | The [setting][settings-ref] and [Build 4121][r4121] describe the folder and named-project exceptions, not the macOS default. True is the U06 decision. | Shipped/fallback true. `r32_cmd_w_closes_anonymous_window_without_quitting_or_restoring_it` exercises the normal D5 close path and fresh Dock reopen / New Window construction; `r32_cmd_w_default_keeps_project_windows_and_prompts_before_closing_dirty_tab` covers both exceptions and the dirty-tab decision. | **same** (fixed in round 32 lane B) |
| Reopen closed tab (⌘⇧T) | [Mac shortcuts][mac-tabs] documents reopening the last closed tab. | `reopen_stack_is_window_local_and_restores_selection_and_scroll`, `reopen_skips_preview_closes_and_missing_entries_but_keeps_real_view_state`; authored key dispatches `reopen_last_file`. | **same** (file-backed tabs) |
| Tab multi-select and dirty marker | [Tab multi-select][tabs]: ⌘-click toggles membership and selected sheets split the pane; [theme reference][theme] documents the dirty state. | `App::handle_mouse_event` dispatches ⌘-click to `Workspace::click_sheet`; `round18_tabs_fit_scroll_dirty_colors_buttons_and_live_hiding` and `r20_selection_and_shift_anchor_survive_external_inserts` exercise marker/selection behavior. | **same** (basic toggle/layout and marker) |
| ⌥-click to replace the focused member | [Tab multi-select][tabs] documents this for the tab strip, tab dropdown and sidebar. | `r32_option_click_replaces_focused_tab_from_strip_dropdown_and_sidebar` exercises all three mouse paths, including opening a new file from the folder tree. Peers stay selected, the replaced member stays open, and the pane layout is unchanged. | **same** (fixed in round 32 lane B) |
| Reorder, move and tear off a tab | Velocity decision (2026-11-26), matching the Sublime Text 4215 black-box recording. | A left-button press stays a click until it moves 4 logical points. Before detaching, the lifted tab follows only the horizontal motion. Once the pointer is more than one tab height above or below the source strip, or past either side of the window by that distance, the source gap closes and a floating tab follows the original grab offset. Another Velocity editor's strip hides the floating image and previews the insertion slot. Gaps slide for 150 ms, or move instantly with Reduce Motion or `animation_enabled: false`. Dropping off the strips opens a window aligned to the floating tab and clamped to the screen. Dropping on another window's strip moves the live tab there and focuses that window; the source follows `close_windows_when_empty`. Escape, app deactivation, and either participating window closing cancel the gesture. | **Velocity decision** |

Pointer cursors follow the visible controls on both platforms: I-beam over editor text and
editable fields (including Find, quick panels, tab rename and Settings); resize over split
dividers and the sidebar edge; arrow over the rest of the chrome, including the gutter,
minimap, scrollbars, tabs and buttons. The pointer query is separate from IME geometry.

### 4. Editing

| Step | Sublime (source) | Velocity (test or code) | Verdict |
| --- | --- | --- | --- |
| ⌘D, ⌘⇧L and ⌥-drag | [Multiple selection][multi], [column selection][column], [Mac edit shortcuts][mac-edit]. | `resource_mousemap_reaches_real_press_drag_release_and_scroll_handlers` plus `find_incremental_wrap_skip_whole_word_and_case` and `split_lines_keeps_reversed_unicode_blank_cr_and_eof_edges`; keys dispatch `find_under_expand` and `split_selection_into_lines`. | **same** |
| Auto-pairs and auto-indent | [Settings reference][settings-ref] documents matching punctuation; [indentation guide][indent] describes auto-indent and per-syntax rules. | `editing.rs` dispatches pairing/skip/backspace and auto-indent; existing pair and indentation tests exercise atomic inserts and undo. | **same** (pair insertion and basic Enter indentation) |
| Undo grouping for typing a word, spaces and pairs | The [command reference][commands] documents undo and grouping commands, not the boundary heuristic for live typing or compound pair edits. | `typing_group_word_space_word_and_selection_break` covers a word group, intervening whitespace and selection breaks; Auto Pair inserts both characters in one transaction. | **uncertain** (U07) |
| Comment toggle, duplicate and move lines | [Mac edit shortcuts][mac-edit] lists ⌘/, ⌘⇧D and ⌃⌘↑/↓. | Velocity keymap sends `toggle_comment`, `duplicate_line` and `swap_line_up/down`; `app_round17_tests.rs` exercises syntax-aware commands, editor tests cover multi-selection line edits. | **same** |
| Soft undo and paste/indent | [Mac edit shortcuts][mac-edit] documents ⌘U and ⌘⇧V. | `soft_undo_each_typing_transaction_and_redo_invalidation`; `identical_paste_and_indent_collapses_all_requested_selections`; keymap dispatches `soft_undo`, `paste` and `paste_and_indent`. | **same** (basic command contract) |

### 5. Find

| Step | Sublime (source) | Velocity (test or code) | Verdict |
| --- | --- | --- | --- |
| ⌘F search as you type; ⌘I, Return, Escape | [Mac Find shortcuts][mac-find] and [search guide][search] distinguish ordinary Find (Return stays open) from Incremental Find (Return selects and dismisses). | `Panels::refresh_find_input` highlights and previews in both modes while typing (outside In Selection); `cmd_f_pending_navigation_keeps_every_key_and_direction`, `cmd_f_reports_restrictions_and_exact_count_in_scene`, and `incremental_find_shift_enter_selects_previous_and_closes`. | **same** (search-as-you-type and Return contract) |
| ⌘G/⇧⌘G cycle across the ends | [Search guide][search] documents next and previous, with or without the panel; [theme reference][theme] describes a wrap search control. Its platform-neutral key examples are not the macOS binding list. | `cmd_i_shift_enter_and_named_find_prev_respect_wrap`, `cmd_f_pending_navigation_keeps_every_key_and_direction`; `Panels::apply_find_next` honors wrap in both directions. | **same** (match selection) |
| Notification on a wrap | [Search guide][search] does not specify whether a wrap gets its own notification, where it appears, or its lifetime. | `Panels::apply_find_next` reports ordinal/count while Find is open; it does not emit a distinct wrapped message. | **uncertain** (U08) |
| Seed Find selection; ⌘E | [Settings reference][settings-ref] documents `find_selected_text`; [search guide][search] documents explicitly copying a selection to the search query. | `find_panel_seeding_table_and_close_after_all_values`, `regex_buffer_seeds_are_literal_through_find_and_find_in_files`; ⌘E dispatches `slurp_find_string`. The option is true in Velocity; implicit seeding accepts a nonempty single line. | **same** (option and explicit selection seeding; factory value/shape below) |
| Selection seeding default and multiline selection | The [setting][settings-ref] describes seeding but does not specify the default or selection shape. [Search guide][search] confirms the panel accepts multiline patterns explicitly. | `Panels::set_query_from_buffer`: implicit seed is single-line; paste and explicit slurp can carry hard line breaks, and regex seeding can escape literal text. | **uncertain** (U09) |
| Replace; close button and Escape | [Mac Find shortcuts][mac-find] and [search guide][search] document the Replace panel and next/all actions; [command reference][commands] documents hiding panels. | `find_and_replace_paste_hard_lines_and_keep_enter_as_search`, `replace_all_keeps_text_replaced_at_the_scope_start`, `find_panel_close_button_hides_the_panel`; Escape routes to `Panels::key`, closes the panel and restores an uncommitted Incremental Find preview. | **same** (ordinary Find/Replace lifecycle) |
| ⌘⇧F, Find Results tab / bottom panel, Return, double-click and F4 | [Search guide][search-results] documents separate view/context and jumps; [theme reference][theme] describes buffer/panel destinations. | Actual Find key/worker and result gestures exercised by `find_results_tab_return_double_click_and_command_click_follow_edited_regions`, `find_results_tab_toggle_persists_and_both_destinations_navigate`, and `r32_find_results_and_plain_text_do_not_advertise_wrap_off`. | **same** (D3 restores the documented path) |
| Use Buffer initial setting | [Search guide][search-results] confirms the destination choice, not its factory state. | `Panels::restore_session` defaults to true and restores saved per-window state. | **uncertain** (U10) |
| Append/reuse a single Find Results tab | [Search guide][search-results] describes a results view, not search-to-search reuse. | `find_results_tab_format_scopes_append_and_reopen`: per-window scratch tab, append a block, reveal the new block; closing the tab resets this container. | **uncertain** (U11) |
| Find Results report wording | [Search guide][search-results] illustrates context/navigable results; [Build 4166][r4166] discusses truncation and order, not exact text or plural forms. | Round 31's report has a search header, path headings, numbered context and final counts; `find_results_tab_format_scopes_append_and_reopen` checks its strings. | **uncertain** (U12) |
| ⌘-click a result | [Search guide][search-results] confirms Return and double-click, not ⌘-click. | `find_results_tab_return_double_click_and_command_click_follow_edited_regions`: ⌘-click opens the tracked match. | **uncertain** (U13) |
| Cancel a running Find in Files from the menu | [Build 4107][r4107] documents cancellation via the Find menu. | `r33_cancel_find_in_files_from_menu_and_palette` dispatches the menu and palette entries against both result destinations and verifies that the command is disabled at rest. | **same (fixed in round 33 lane C)** (audit B05; round 33 work order B14) |

### 6. Navigation

| Step | Sublime (source) | Velocity (test or code) | Verdict |
| --- | --- | --- | --- |
| ⌘P, preview a file, Escape back to origin or Return to open | [Mac navigation shortcuts][mac-nav], [navigation guide][nav], [Build 4121][r4121] on transient cleanup, and 4215 on macOS (U04). | Return or clicking a result commits a normal tab with the previewed selection and viewport. Escape, clicking away, and window focus loss restore the origin; an unaccepted file leaves no tab, session sheet, recent file, or watch. `r41_cancel_restores_origin_and_open_tab_navigation`, `cancel_goto_restores_an_existing_sidebar_preview_and_its_viewport`. | **same** |
| Goto Anything `@`, `:`, `#` | [Navigation guide][nav] documents symbols, one-based lines, and text search in the current or selected file. | `Panels::preview_current`, `file_at_symbol_previews_the_first_local_picker_result`, `goto_line_with_argument_jumps_but_no_argument_opens_goto`; `#` offers word-match targets. | **same** (operators and targets) |
| `#term` fuzzy match highlighting | [Navigation guide][nav] documents highlighting all fuzzy matches. | `r32_goto_word_highlights_cover_local_and_qualified_fuzzy_matches` checks byte ranges for all matched occurrences (not just the picker rows), and clearing them on Return, Right Arrow and Escape. | **same** (fixed in round 32 lane B) |
| Open a Goto item without leaving the picker | [Navigation guide][nav] documents Right Arrow for opening the current item and keeping Goto open. | `r32_goto_right_opens_and_retains_picker_but_escape_restores_origin`: unmodified Right opens the item at the end of the input; otherwise it edits the query. Escape restores the original view and discards uncommitted previews but leaves explicitly opened tabs open. | **same** (fixed in round 32 lane B) |
| Goto Symbol in Project and Goto Definition | [Mac navigation shortcuts][mac-nav] gives ⌘⇧R, F12 and a split variant; [Indexing][indexing] explains symbol indexing. | `project_symbols_definition_previews_unicode_locations_and_bounded_history`, `definition_uses_open_tabs_with_index_disabled_and_current_file_first` and the shipped bindings. | **same** (registered commands, open-buffer/index paths, jump/split; not language intelligence parity) |
| ⌃-/⌃⇧-, bookmarks | [Build 4107][r4107] documents jump history; [Mac bookmark shortcuts][mac-bookmarks] lists ⌘F2, F2 and ⇧F2. | Shipped keys dispatch `jump_back/forward`, `toggle/next/prev_bookmark`; `r19_nearby_bookmarks_and_marks_are_explicit_jumps` and `project_symbols_definition_previews_unicode_locations_and_bounded_history`. | **same** |

### 7. Saving

| Step | Sublime (source) | Velocity (test or code) | Verdict |
| --- | --- | --- | --- |
| ⌘S on untitled; File › Save All | [Save commands][commands] describe save, Save As and Save All. | ⌘S dispatches `save`; `App::save_tab_with_path` chooses Save As for an unnamed tab. `save_all_skips_clean_tabs_and_continues_after_cancel_and_failure`, `r20_failed_explicit_save_cancel_restores_format_and_save_all_recovers_each_tab` check file effects/cancellation; picker launch is traced. | **same** |
| Save with encoding, choose line endings | [Commands][commands] and [status controls][status] document encoding/line-ending commands and UI. | `r20_menu_encoding_bytes_and_selected_alias_survive_restore`, `r20_status_text_and_format_menu_table`: dispatch real File/status menu commands, verify disk bytes/labels. | **same** |
| `atomic_save` | The permitted [ST4 settings reference][settings-ref] and [release histories][releases] don't establish the factory value or file-system exceptions. | `settings_effect_tests::save_probe` reaches `vel-fileio` with the on/off setting; false is the shipped default. Where eligible, atomic writes stage a sibling then replace, without changing the save interaction. | **uncertain** (U19) |
| `save_on_focus_lost` | [Build 4121][r4121] documents interaction with reload prompts. | `focus_lost_saving_is_opt_in_and_skips_conflicts_readonly_and_untitled`; saves only a dirty file-backed view being left, never an untitled or conflicted file. | **same** (opt-in save of an eligible, changed file) |
| Disk changed: silently reload clean; keep/reload dirty | [Build 4107][r4107] documents `reload_file_on_change`; [Build 4143][r4143] refers to reload prompts. Dialog wording/default focus not specified. | `r20_external_rewrites_keep_encoding_and_respect_reload_opt_out`, `reload_decisions_and_failed_load_do_not_overwrite_edits`: clean auto-reloads with setting on, dirty changes go through the Keep Edits/Reload path. | **same** |
| Separate background reload option | [Build 4200][r4200] documents `reload_file_in_background` but not its default. | Defaults to `true` to retain Velocity's immediate reload. With `false`, clean inactive documents defer to the activation frame; dirty documents keep existing conflict handling. `r33_background_reload_clean_active_inactive_and_dirty` and `r33_background_reload_follows_shared_buffer_across_clones`. | **same (fixed in round 33 lane C)** (B07) |
| External deletion, `close_deleted_files` | [Build 4107][r4107] documents the option for deleted documents during session restore. Live deletion behavior is not established by that note. | `missing_files_do_not_abort_restore_and_close_deleted_files_keeps_dirty_text` covers restore; `close_deleted_files_closes_clean_watched_tabs` covers live clean deletion. Dirty text survives either path. | **same** (restore behavior; live policy is U14) |
| Live deletion with `close_deleted_files` on | [Build 4107][r4107] speaks specifically about restore. | A clean watched tab closes immediately; a dirty/retained one is marked deleted so the user can recreate or close it. | **uncertain** (U14) |

### 8. Closing and quitting

| Step | Sublime (source) | Velocity (test or code) | Verdict |
| --- | --- | --- | --- |
| Dirty tab: preserve, discard or cancel | [Commands][commands] distinguish ordinary close from forced close; [Build 4121][r4121] confirms prompting on non-Hot-Exit quit. Public details do not specify the dirty-tab macOS text. | `App::unsaved_changes_dialog` presents “Save changes?” plus tab name, Save / Don't Save / Cancel. The same dispatcher uses test-supplied close decisions, including save failure and Cancel. | **uncertain** (U15: exact wording/default/order, not whether to offer the three choices) |
| Close a non-project window, then relaunch | [Settings reference][settings-ref] describes preserving changes on app exit or closing a project window under Hot Exit. It does not fully document ordinary-window retention. | `App::prepare_window_close`, `dirty_window_cancels_and_saves_before_closing_with_hot_exit`, `closing_last_window_leaves_nothing_to_restore_in_each_hot_exit_mode`. | **differs on purpose** (D5 fixes the distinction between closing a window and quitting) |
| ⌘Q under `hot_exit`; relaunch | [Settings reference][settings-ref] documents retaining unsaved content and open files on exit; [Build 4107][r4107] expands Hot Exit controls. | `App::prepare_quit`, `closing_clean_window_then_quitting_only_restores_survivor_and_keeps_reopen_path`, and `hot_exit_quits_without_loading_twenty_hidden_documents`. With `hot_exit` disabled the close-decision path applies. | **same** (D5 also requires only windows open at quit) |
| Crash recovery | [Build 4126][r4126] explicitly addresses session integrity on crash; [Build 4143][r4143] documents exceptional prompts on very large unsaved files. Recovery interval/durability isn't specified. | `session.rs`, `safe_mode.rs`, `restore_io.rs`: checkpoint plus autosave, backups and guarded recovery; headless corrupt/interrupted session and lazy restore tests. Last unsnapshotted edits after a hard kill are not guaranteed. | **same** (recovery capability; no exact crash-window guarantee) |

### 9. Projects

| Step | Sublime (source) | Velocity (test or code) | Verdict |
| --- | --- | --- | --- |
| Add Folder, Save Project As | [Projects][projects] specifies folders, relative paths, and the definition/workspace split; [community projects guide][project-guide] explains adding and saving. | `add_folder` and `save_project_as` use live project state; `project_save_as_keeps_rules_and_unknown_keys_and_loads_inline_builds`, `project_save_as_preserves_relative_paths_unknown_folder_keys_and_absent_defaults`. | **same** |
| ⌃⌘P quick switch and project settings | [Community projects guide][project-guide], [Build 4143][r4143], [Settings][settings]. | Shipped ⌃⌘P runs `quick_switch_project` and selects a recent project; `settings_project_view_and_user_keymap_precedence_reload` and `two_apps_route_to_existing_folder_project_and_file_without_replacing_dirty_tabs`. | **same** (basic project switch and setting scopes) |
| Sidebar new file/folder, rename, delete to Trash, reveal | [Navigation guide][nav] identifies sidebar file operations; [Build 4143][r4143] describes reveal/open-containing-folder behavior. | `name_prompts_roundtrip_context_and_create_files_and_folders_without_clobbering`, `rename_and_trash_preserve_dirty_shared_tabs_undo_and_unrelated_buffers`; `app_context.rs` delegates Trash and Finder reveal to the native adapter. No live Finder/Trash was used. | **same** (commands/intent and filesystem model; native UI not established) |

### 10. Build

| Step | Sublime (source) | Velocity (test or code) | Verdict |
| --- | --- | --- | --- |
| ⌘B with Automatic; explicit system and ⌘⇧B variants | [Build systems][build] documents syntax/file/project eligibility, Automatic, and manual selection; [Mac shortcuts][mac-build] gives the keys. | `round18_build_dispatch_panel_save_all_restart_cancel_and_key_context`, `round18_build_with_choices_filter_and_remember_variants`, `round18_review_build_memory_tracks_matching_sets_and_only_picker_choices`. | **same** |
| Output panel, F4/⇧F4 to errors, Ctrl+C cancel | [Build systems][build] documents output and file/line regex; [Mac shortcuts][mac-build] gives Ctrl+C. | `round18_build_dispatch_panel_save_all_restart_cancel_and_key_context`, `cancel_after_transport_drop_preserves_already_displayed_output_and_f4_results`: actual process/output and result key action. | **same** (noninteractive builds) |
| Interactive build input | [Build 4200][r4200] and the [Build systems reference][interactive-build] specify an opt-in input box. | B09: `"interactive": true` adds a single-line field to the output panel. Return submits, Control-D closes stdin after queued lines, and input echoes have their own scope. `r33_build_panel_sends_echoes_and_closes_stdin`, `r33_build_panel_cancel_and_restart_clear_the_draft_and_pipe`, and the non-reading child test exercise the workflow. | **same (fixed in round 33 lane B)** |

### 11. Command palette and packages

| Step | Sublime (source) | Velocity (test or code) | Verdict |
| --- | --- | --- | --- |
| ⌘⇧P, type, choose, Escape | [Mac shortcuts][mac-general] and [command palette guide][palette] describe searchable registered commands. | The shipped keymap opens the overlay; `palette_filters_and_rechecks_actual_command_enablement` and the exact menu/palette dispatch tests check enablement and invocation. | **same** |
| Install, disable, enable a declarative package | [Package Control guide][package-control] documents palette install/enable/disable; [Packages][packages] describes resources and precedence. | `package_worker_install_disable_enable_remove_reloads_every_resource_and_keeps_buffers` uses a local fixture, reloads the keymap and other resources, and preserves dirty text. `app.rs` sends install and enablement to the package worker. | **same** (declarative workflow and `ignored_packages` behavior, not catalog compatibility) |
| Package manager and compatible resource types | [Packages][packages] and [Package Control guide][package-control] describe Sublime's ecosystem, which includes Python plugins. | Velocity has its own declarative package manager; archives can contain non-executable resources but the editor does not start Python hosts. See [1.0 scope](parity.md#scope-exclusions). | **differs on purpose** (Velocity-only declarative package manager) |

### 12. Settings and customization

| Step | Sublime (source) | Velocity (test or code) | Verdict |
| --- | --- | --- | --- |
| ⌘, and Preferences routing | [Settings][settings] and [Font settings][font] document the Default/User JSON pair, opened by ⌘,. | Since 2026-11-23 ⌘, / Ctrl+, and Settings… open the Settings window (`app_settings_window.rs`); Settings (JSON)… and Preferences: Settings (JSON) open the two-view JSON pair (`settings_entry.rs`). Tests: `settings_ui_entry_points_open_one_settings_window_and_keep_json_pairs`, `menu_roles_and_exact_shortcuts_survive_renaming_and_reload`. | **differs on purpose** (D1 revised by the user; the JSON pair itself is unchanged) |
| Syntax-specific settings and User key bindings | [Settings][settings], [Key bindings][keys] describe layer precedence and custom bindings. | `settings_entry.rs`, `settings_project_view_and_user_keymap_precedence_reload`, `menu_roles_and_exact_shortcuts_survive_renaming_and_reload`. | **same** (editor settings and bindings) |
| Select color scheme/theme and preview | [Themes][theme], [Color schemes][schemes], [Build 4121][r4121] distinguish scheme from chrome and describe the chooser. | `r19_appearance_preview_commit_cancel_and_broken_scheme`; Esc rolls back, accepting saves the chosen resource. | **same** |
| Font zoom (⌘+/⌘−) | [Font settings][font] documents editable `font_size`; [Build 4200][r4200] documents the reset preference, not the full Mac key aliases, increment or bounds. | Shipped `super+=` / `super+-` map to `Workspace::zoom_font`, one point per press, persist in User settings, and respect syntax/view overrides. | **uncertain** (U16: exact aliases and increment) |
| Graphical settings and language selection | [Settings][settings] documents JSON settings, not a visual settings editor or this UI language switch. | `settings_ui.rs`, `settings_entry.rs` and `set_ui_language` keep separate entry points and live catalogs. Lane B owns R3 Chinese names/descriptions and cleanup of Velocity's shipped JSON comments, as well as D2 auto. | **differs on purpose** (Velocity-only additions; use them without displacing D1) |

### 13. Status bar and notifications

| Step | Sublime (source) | Velocity (test or code) | Verdict |
| --- | --- | --- | --- |
| Position, selection, syntax, indentation, format, Git | [Theme status elements][status] document the status-message area and the format/syntax/indent controls; [Git integration][git] documents VCS status; [Build 4151][r4151] mentions the status column. | `status_position`, format-menu click dispatch in `app_status.rs`, `r20_status_text_and_format_menu_table`, `status_items_never_overlap_at_narrow_widths`. | **same** (field roles, not default flags or exact position formatting) |
| “wrap off” on Find Results and explicitly unwrapped text | [Theme status elements][status] describe the controls and messages with no word-wrap item; Wrap is a document/view setting, not a status item. | `large::hint` was reporting explicit `word_wrap:false` as if it were a large-file diagnostic, including for Find Results. Removed this label without changing wrapping; `r32_find_results_and_plain_text_do_not_advertise_wrap_off` verifies the rendered status in both UI languages. | **differs — bug** (B03, fixed) |
| Binary content label | [Build 4126][r4126] explicitly says binary files show Binary in the syntax position. | The hex view previously had no right-hand format label. `r32_status_labels_binary_without_offering_text_syntax` checks the rendered label and mouse dispatch in both UI languages. | **differs — bug** (B11, fixed) |
| Hide syntax or indentation fields | [Build 4107][r4107] documents `show_syntax` and `show_indentation`. | Both default to `true` in Velocity. Setting either to `false` removes that field and its click target; `r33_status_fields_and_switcher_follow_settings` and the Settings inventory/reader tests. | **same (fixed in round 33 lane C)** (B12) |
| Sidebar/panel switcher in status bar | [Theme status elements][status] describes the left-side switcher; [Build 4107][r4107] adds `show_sidebar_button`. The [public ST4 forum report](https://forum.sublimetext.com/t/st4-output-panel-select/57664) identifies the toggle and context-click gestures. | The existing Velocity theme icon toggles the sidebar on click; right-click opens the localized panel menu. The setting defaults to `true` in Velocity. `r33_status_fields_and_switcher_follow_settings`; offscreen scene `status_switcher`. | **same (fixed in round 33 lane C)** (B08) |
| Fresh encoding/line ending flags | [Theme status elements][status] describes the fields, but does not establish whether either is shown by default. | `status_position` defaults `show_encoding` and `show_line_endings` to true. | **uncertain** (U17) |
| Transient messages | [API status message][api-status] specifies a temporary window status message. | `Panels::expire_message` expires transient text after five seconds; the message may be rendered inside the Find panel when it owns feedback. | **same** (transient feedback, no claim for exact lifetime/placement) |
| Nonmodal vs modal errors | [Status API][api-status] and [error API][api-error] document separate channels. U18 decides which one Velocity uses for each operation. | `r32_recoverable_error_is_localized_transient_status` and `r32_save_and_hot_exit_failures_use_dialogs_and_keep_unsaved_text` cover the channel split. Close, overwrite, destructive reload and deletion still require a decision. | **same** (fixed in round 32 lane B) |
| Large-file progress and performance hints | [Theme status elements][status] describes a message area, not Velocity's file-size budgets. | `Tab::progress`, `large::hint`; loading/prefix and actual large-file policy diagnostics remain, without the wrap label. | **differs on purpose** (Velocity-only large-file diagnostics) |

#### U18 error and decision channels

Status errors use the normal five-second expiry and keep diagnostic details in the
console where applicable. The catalog translates app copy and templates, not paths,
subprocess output or operating-system diagnostics. Without a live window there is no
status bar; errors are logged rather than opening a standalone alert.

| Call site | Message | Channel | Reason |
| --- | --- | --- | --- |
| `App::run_named`, `run_named_depth`, selector/key/mouse dispatch | Invalid or missing argument (e.g. “missing encoding”), command not applicable, unavailable resource, or failed optional action; fallback “Action failed: {detail}” | Status bar | No edit or write needs a decision; the user can correct and retry. |
| `App::on_named_command` (bad arguments and no-window Open Recent), `on_open_paths`, `on_idle`, `on_reopen` | Arguments, request, open or window-creation diagnostic | Status bar, or log if windowless | An open request failed; it has not discarded any edits. |
| `vel_platform::macos_requests` window-factory fallback | “Could not open a window” with diagnostic | Log | No editor window exists to display feedback. |
| `App::on_appearance`, `on_frame` (workspace messages and font load) | Theme, font, restore/open or deferred workspace diagnostic | Status bar | Resources or state could not be loaded; this is not an overwrite or discard. |
| `release::Actions::poll` | “Package or release action failed: {detail}” | Status bar | Optional package, CLI installer or report action failed. |
| `App::format_action`, `save_tab`, `apply_save_action`, `decide_close_document_with`, project Save As | “Could not save: {detail}” (Save All groups failures); read-only, binary and non-regular targets or invalid encodings fail preflight | Dialog for write failures; status for preflight rejections | Failed persistence matters when the user expects edits to be on disk. A failed save never authorizes a close. The explicit UTF-8 retry choice stays available. |
| `App::prepare_window_close` | “Could not save project workspace: {detail}” | Dialog if dirty content cannot be kept, otherwise status | Fall back to save/discard/cancel for at-risk edits; a lost metadata checkpoint alone is recoverable. |
| `App::prepare_quit` | “Could not save Hot Exit session” with repair guidance | Dialog; quit is vetoed with unsaved content | Exiting now could lose the only copy. |
| `App::unsaved_changes_dialog`, `save_tab_with_path` | “Save changes?”; “File changed on disk” before overwrite | Dialog | Closing or overwriting can lose edits or disk changes. |
| `App::format_action` (Revert), `on_frame` (external reload conflict) | “Revert File?”; “File changed on disk” before reloading over unsaved edits | Dialog | Reload would discard dirty text. A load failure that preserves text is nonmodal. |
| `app_context` (Trash), `App::replacement_confirmation` | Move to Trash?; Replace in Files? | Dialog | Confirm a destructive operation first. A rejected preflight is nonmodal. |
| `App::poll_restore_safety`, `release::Actions` | Session Recovery; remove/upgrade package; crash report choice; CLI install/destination | Dialog | Explicit recovery, destructive or installation decision. About and package contributions are informational requests, not command failures. |

### Follow-up order

B10 (⌘, back to JSON) was done by lane B in round 31 and superseded by the user's
2026-11-23 decision (D1): ⌘, now opens the Settings window. Start with
B05 and B06, which affect frequent navigation/search use;
then B07 and B04, which need preservation of file state or decoder coverage. B08, B09,
B12, B13 and B14 can follow. The current code evidence for B08 and B12 supersedes the older
status-control rows in the capability matrix. Do not substitute decisions on the questions
below for these documented fixes.

### Verification

The full `/tmp/r14-review/gate.sh` returned `GATE_OK`: formatting, strict release
Clippy, 1,670 release workspace tests passed (zero failed), rustdoc, release build,
and plist unit checks. The four `r32_` regressions passed. The runs used a fixture
home and shell, temporary logs, and six Cargo build jobs. No native windows or
app bundles were created.

Five alternating pairs against the unchanged Round 31 binary (`6dc73c6`):

| Workload | Median paired retired-instruction change | Inside ±1% |
| --- | ---: | --- |
| View features, 10 MB Rust | −0.26% | Yes |
| One-caret smoke | −0.08% | Yes |
| 1,000-caret smoke | −1.34% | No (fewer instructions) |

An independent five-pair check of the 1,000-caret case measured −1.32%.
`scripts/bench-headless.py --instruction-gate` passed its one-sided regression
limit; the 1,000-caret result is still outside the requested symmetric band.
The runner used Darwin own-child counters because `/usr/bin/time -l` could not
read `kern.clockrate` in the sandbox. Raw results: `/tmp/velocity-r32.uM0iib/paired`
and `/tmp/velocity-r32.uM0iib/paired-confirm`.

[r4107]: https://www.sublimetext.com/download#build-4107
[r4121]: https://www.sublimetext.com/download#build-4121
[r4126]: https://www.sublimetext.com/download#build-4126
[r4143]: https://www.sublimetext.com/download#build-4143
[r4151]: https://www.sublimetext.com/download#build-4151
[r4166]: https://www.sublimetext.com/download#build-4166
[r4180]: https://www.sublimetext.com/download#build-4180
[r4200]: https://www.sublimetext.com/download#build-4200
[r4213]: https://www.sublimetext.com/download#build-4213
[releases]: https://www.sublimetext.com/download#changelog
[api-status]: https://www.sublimetext.com/docs/api_reference.html#sublime.status_message
[api-error]: https://www.sublimetext.com/docs/api_reference.html#sublime.error_message
[build]: https://www.sublimetext.com/docs/build_systems.html
[interactive-build]: https://www.sublimetext.com/docs/build_systems.html#interactive
[cli]: https://www.sublimetext.com/docs/command_line.html
[column]: https://www.sublimetext.com/docs/column_selection.html
[commands]: https://docs.sublimetext.io/reference/commands.html
[font]: https://www.sublimetext.com/docs/font.html
[git]: https://www.sublimetext.com/docs/git_integration.html
[indent]: https://www.sublimetext.com/docs/indentation.html
[indexing]: https://www.sublimetext.com/docs/indexing.html
[keys]: https://www.sublimetext.com/docs/key_bindings.html
[mac-bookmarks]: https://docs.sublimetext.io/reference/keyboard_shortcuts_osx.html#bookmarks
[mac-build]: https://docs.sublimetext.io/reference/keyboard_shortcuts_osx.html#build
[mac-edit]: https://docs.sublimetext.io/reference/keyboard_shortcuts_osx.html#editing
[mac-find]: https://docs.sublimetext.io/reference/keyboard_shortcuts_osx.html#find-replace
[mac-general]: https://docs.sublimetext.io/reference/keyboard_shortcuts_osx.html#general
[mac-nav]: https://docs.sublimetext.io/reference/keyboard_shortcuts_osx.html#navigation-goto-anywhere
[mac-tabs]: https://docs.sublimetext.io/reference/keyboard_shortcuts_osx.html#tabs
[multi]: https://www.sublimetext.com/docs/multiple_selection_with_the_keyboard.html
[nav]: https://docs.sublimetext.io/guide/usage/file-management/navigation.html
[package-control]: https://docs.sublimetext.io/guide/package-control/usage.html
[packages]: https://www.sublimetext.com/docs/packages.html
[palette]: https://docs.sublimetext.io/guide/extensibility/command_palette.html
[project-guide]: https://docs.sublimetext.io/guide/usage/file-management/projects.html
[projects]: https://www.sublimetext.com/docs/projects.html
[schemes]: https://www.sublimetext.com/docs/color_schemes.html
[search]: https://docs.sublimetext.io/guide/usage/search-and-replace.html
[search-results]: https://docs.sublimetext.io/guide/usage/search-and-replace.html#results-format
[settings]: https://www.sublimetext.com/docs/settings.html
[settings-ref]: https://docs.sublimetext.io/reference/settings.html
[status]: https://www.sublimetext.com/docs/themes.html#status-bar
[tabs]: https://www.sublimetext.com/docs/tab_multi-select.html
[theme]: https://www.sublimetext.com/docs/themes.html

## Decisions on the open questions (user, 2026-11-21)

All 19 questions below were decided by the user. "Keep" means Velocity's current behaviour is
now the confirmed design.

| # | Decision | Change |
| --- | --- | --- |
| U01 | Changed to Sublime: hide the sidebar by default in windows without folders, show it in windows with folders, and keep the initial editable buffer tabless until edited. No welcome page. | 2026-11-22, round 41 lane B. Sublime Text 4215 in a fresh safe-mode profile: `subl a.rs zh.md b.txt` showed three tabs and no sidebar; one file showed one tab and no sidebar; a folder alone showed FOLDERS, line 1 and a caret, no tab, and `untitled — <folder>`. |
| U02 | Keep: a drop anywhere on the window opens in the window (no per-pane drop zones) | none |
| U03 | **A**: a bare path from the CLI keeps an already-open file's position; explicit `:line[:col]` still jumps | Done (round 33 lane C) |
| U04 | Changed to Sublime: Goto previews unopened files without a tab; an already-open result uses its tab. Sidebar single-click still shows an italic transient tab. | 2026-11-22, Sublime Text 4215 on macOS: open a folder, press ⌘P and type `zh`; `zh.md` appears in the editor with no new tab. Return opens a tab; Escape restores the origin. The [navigation guide][nav] confirms the distinction. |
| U05 | Keep: new tabs are appended at the right of the current group | none |
| U06 | **B**: `close_windows_when_empty` defaults to `true`: ⌘W on the last tab of a window without a project closes the window (the app keeps running); project windows stay open | done (round 32 lane B) |
| U07 | Keep: typing runs are one undo step; an auto-pair is undone with its opening half | none |
| U08 | Keep: Find shows "N of M" in the panel; no separate wrap notice | none |
| U09 | Keep: single-line selections seed Find; multi-line selections do not; ⌘E unchanged | none |
| U10 | Keep: Find in Files results go to the Find Results tab by default (Use Buffer on) | none |
| U11 | Keep: each search is appended to the window's one Find Results tab | none |
| U12 | Keep the current report wording | none |
| U13 | Keep ⌘-click as an extra jump gesture in Find Results | none |
| U14 | Keep: with `close_deleted_files`, clean tabs close when their file is deleted; dirty tabs are never discarded | none |
| U15 | Keep the dirty-close prompt (Save / Don't Save / Cancel, default Save, never Don't Save) | none |
| U16 | Keep ⌘= / ⌘− zoom by one point | none |
| U17 | Keep encoding and line endings always visible in the status bar (can be turned off) | none |
| U18 | **B**: recoverable, reversible command failures are reported in the status bar; dialogs only for confirmations and possible data loss | done (round 32 lane B) |
| U19 | Keep `atomic_save` off by default | none |

### Round 43: U01 and U04 details

Source: 2026-11-22 side-by-side comparison on macOS, with fresh profiles and Sublime
Text 4215 in safe mode. The comparison used screenshots and the accessibility/menu tree.

| Journey | Sublime observation | Velocity regression coverage |
| --- | --- | --- |
| U01: Open an untouched window, with or without a folder | The empty window is titled `untitled`, or `untitled — <folder>` when a folder is open. | `r43_empty_titles_survive_goto_in_both_languages`: localizes `untitled` (`未命名` in zh-Hans) with and without the folder suffix. |
| U01: View › Side Bar | Show Open Files starts unchecked; the sidebar shows FOLDERS. The visibility action reads Hide Side Bar or Show Side Bar, with no check mark. | `r43_open_files_defaults_off_and_restores_each_explicit_choice`, `r43_side_bar_menu_caption_tracks_visibility_in_both_languages`: per-window choice persists; the localized action retains the shortcut display. |
| U01/U04: Open `zh.md`, then move through Goto results | FOLDERS highlights the active file, including the preview during Goto. The tabless preview from the empty folder window keeps the title `untitled — <folder>`. | `r43_folder_highlight_follows_view_without_reveal_or_focus_change`, `r43_empty_titles_survive_goto_in_both_languages`: highlight only rendered file rows, without touching the sidebar's selection, expansion or scroll. Explicit Reveal in Side Bar is unchanged. |
| U04: Read the Goto result detail | The second line shows `zh.md` at the root and `sub/dir/file` for nested files. The [public 20110203 changelog][goto-multi-root] documents a folder-name prefix for multiple roots; this was not part of the 4215 observation. | `r43_goto_publishes_relative_details_and_keeps_outside_paths`, `r43_goto_detail_abbreviates_home_on_component_boundaries`: one root uses a relative path, multiple roots add the containing folder name, and outside files retain their absolute path with `~` for HOME. |

Render coverage: `r43-empty-folder-goto` adds the tabless preview, visible folder-row
highlight, and relative result detail. `sidebar_folders_only` uses the new window
default; `sidebar`, `sidebar_focused`, and `sidebar_narrow` explicitly enable Open
Files. The `r41-goto-preview`, `r41-goto-committed`, and `r41-sidebar-preview` scenes
still cover the three distinct tab states.

[goto-multi-root]: https://www.sublimetext.com/2

## To discuss

No behavior changes are made on the basis of these questions. Recommendations are for
discussion, not new defaults.

| Question | What Sublime is known to do / Velocity now | Options and recommendation |
| --- | --- | --- |
| **U01. What should a fresh window show?** | The clean-profile Sublime Text 4215 observations are recorded in the U01 decision above. [Build 4126][r4126] documents empty-window reuse. | **Decided 2026-11-22.** The empty editor is immediately editable but is not an open document yet. New File opens a tab immediately. Explicit sidebar toggles override the folder-based default and survive session restore. |
| **U02. Should a window drop target a particular pane or just open in the current window?** | [Build 4126][r4126] documents alias resolution on macOS drops; details of drop zones aren't established. Velocity routes accepted paths through its native open dispatcher; Dock opens still follow D4. | A. Keep window-wide routing and verify actual Finder alias delivery separately. B. Add pane-specific drops. **Recommend A**; avoid inventing a pane rule. |
| **U03. Should bare CLI reopen preserve the existing location?** | [CLI][cli] documents optional positions; [Build 4143][r4143] describes macOS reopen without identifying the CLI route. | **Decided A.** Bare paths preserve the caret and viewport; explicit coordinates, including `:1:1`, still jump. |
| **U04. Should Goto's preview occupy a tab?** | No for an unopened file. Sublime Text 4215 on macOS confirmed the [navigation guide][nav] distinction on 2026-11-22: ⌘P → `zh` previews `zh.md` without adding a tab. Return opens it; Escape restores the origin. | **Resolved: A.** Keep sidebar preview tabs and show new Goto previews without tabs. Velocity still highlights the original tab while drawing the preview; the round 43 observation established the highlight in FOLDERS, not a different tab selection rule. |
| **U05. Where should an ordinary new tab go?** | [Tab multi-select][tabs] specifies some modifier-based placements, not the ordinary default. Velocity appends at the right of the current group. | A. Append. B. Insert immediately to the right of the current tab. **Recommend A** until the default is confirmed. |
| **U06. Should ⌘W close an empty anonymous window by default?** | [Settings reference][settings-ref] documents the option, and [Build 4121][r4121] documents the named-project exception. | User chose B: close the window, not the app. Preserve the project exceptions and D5. |
| **U07. What should one ⌘Z revert after a word, a space, or an auto-pair?** | [Commands][commands] document undo/soft undo but not typing boundaries. Velocity groups ordinary typing and makes the automatic pair one edit transaction. | A. Keep the grouping and atomic pair. B. Group continuous typing across more boundaries but retain atomic pairs. **Recommend A**; don't invent ST4's grouping without evidence. |
| **U08. Should Find say when it wraps?** | [Search guide][search] documents next/previous; exact wrap feedback isn't specified. Velocity gives ordinal/count in an open Find panel and no dedicated wrap notice. | A. Keep the ordinal and no notice. B. Show a brief wrap status with direction. **Recommend A** pending confirmation; a notification doesn't change matching. |
| **U09. Which selections should seed Find automatically?** | [Settings reference][settings-ref] documents the option, not the default or multiline case. Velocity defaults to true for a single line; explicit selection/paste can carry newlines. | A. Keep single-line auto-seed. B. Auto-seed nonempty multiline selections too. C. Default auto-seed off, retain ⌘E. **Recommend A**; avoid replacing a reusable query with a large scope selection unexpectedly. |
| **U10. D3: Which destination should a fresh Find in Files use?** | [Search guide][search-results] confirms the choice, not its factory setting. Velocity has Use Buffer on and remembers subsequent choices per window. | A. Default to the tab. B. Default to the bottom panel. **Recommend A** because it makes the D3 workflow directly available; leave the toggle and persistence either way. |
| **U11. D3: Should searches append to one tab?** | [Search guide][search-results] shows a separate view, not how subsequent searches are presented. Velocity appends to one Find Results tab per window and reveals the latest section. | A. Append/reuse. B. Replace the existing report on each search. C. Open a new tab per search. **Recommend A**; keep close as the way to start fresh. |
| **U12. D3: What wording should the result report use?** | Public [search guide][search-results] and [Build 4166][r4166] establish context/truncation, not exact report text. Velocity prints headings and numbered rows between lines such as `Searching 2 files for "foo" in <project>` and `3 matches across 2 files`. | A. Keep the current readable structure and copy. B. Match independently confirmed Sublime wording and plural cases. **Recommend A** until the exact copy can be established cleanly. |
| **U13. D3: Should ⌘-click jump to a match?** | [Search guide][search-results] confirms Return and double-click. Velocity also jumps on ⌘-click using the same tracked result range. | A. Keep ⌘-click as an additional jump. B. Reserve single modifier-click for editor selection; only Return/double-click jumps. **Recommend A** if the added gesture is desirable; keep the documented two gestures either way. |
| **U14. Should the deletion option close clean tabs immediately?** | [Build 4107][r4107] mentions deleted items in restored sessions, not live watching. Velocity closes clean watched tabs when the option is on and keeps dirty content. | A. Keep live+restore behavior. B. Apply closure only on restore; leave a deleted marker meanwhile. **Recommend A** pending evidence; never automatically discard a dirty tab. |
| **U15. How should the dirty-close prompt read and focus?** | Public [commands][commands] and [Build 4121][r4121] establish close/prompt concepts, not the exact macOS copy, button order or default. Velocity uses “Save changes?” plus the tab name with Save / Don't Save / Cancel. | A. Keep the current copy/defaults and all three choices. B. Adapt exact copy and focus/order to a cleanly confirmed prompt. **Recommend A**; default must never be Don't Save. |
| **U16. Which font zoom keys and steps should be supported?** | [Font settings][font] and [Build 4200][r4200] document size and reset, not all Mac aliases/steps. Velocity binds ⌘= and ⌘− and changes the User size by one. | A. Keep current keys and step. B. Keep them and add explicit shifted-plus and keypad aliases if confirmed. **Recommend A** pending source evidence or a request for broader keyboard coverage. |
| **U17. Show encoding and line endings on a fresh profile?** | [Theme status elements][status] document both controls, not their default flags. Velocity shows both with opt-out settings. | A. Keep both visible. B. Hide both until enabled. C. Show only non-default formats by default. **Recommend A** pending confirmation; save format remains discoverable. |
| **U18. Which operational errors should interrupt editing?** | [Status API][api-status] and [error API][api-error] establish both channels, not per-command policy. | User chose B: the status bar for recoverable errors, dialogs for confirmations and possible data loss. See the call-site inventory above. |
| **U19. Should atomic save be opt-in?** | The permitted [ST4 references][settings-ref] don't specify the shipping default or exceptional filesystems. Velocity ships false and enables staging/replacement when set and safe. | A. Keep false and document opt-in. B. Default true on eligible files. **Recommend A** pending a confirmed default and file-system acceptance checks. |

### Window pile-up at launch (user report, 2026-11-22)

Launching the packaged app showed about twenty windows (mostly "未命名", plus "设置 — Velocity").
The session restored exactly what it held; the windows had accumulated while using the app:

1. **Menu commands went to the wrong window.** A window that resigned key reinstalled its own
   menu on its next frame, and a background window reinstalled its menu whenever its model was
   rebuilt (recent files, resources). Menu items target the installing window, so right after
   opening a window (New Window, Settings, Dock reopen) File › Close File and Window › Close
   Window acted on the previous window: the window meant to be closed stayed open and was saved
   in the session. Fixed: only the key window installs the menu (`menu_install_wanted`), and a
   window takes the menu as soon as it becomes key, also during its startup frames.
   Verified natively: New Window then Window › Close Window closes the new window.
2. **Every Settings invocation opened another window.** Sublime Text 4215 (black box, menu
   driven): a second Settings brings the open settings window to the front, Close File in it
   closes the Default/User pair, and the window is titled `Preferences.sublime-settings`.
   Velocity opened a new "设置 — Velocity" window each time and closed one view at a time.
   Fixed to match; verified natively (`settings_window_is_reused_closes_as_a_pair_and_is_titled_by_its_file`).

Repeated quit/relaunch with one, two and three windows, Dock reopen while running or
windowless, Finder/CLI opens while running, cold launches with files, and a session written by
`bcfe83f` then restored by `72f6f0c` all kept the window count stable.
