# Sublime Text 4 parity: macOS, through Build 4213

Velocity can cover the basic editing workflow: open a project, edit with multiple
carets, navigate by file or symbol, search and replace, use snippets and builds,
and recover unsaved work. That is **basic workflow parity**, not a claim of full
Build 4213 compatibility. In particular, search-result presentation, some tab
gestures, settings, image codecs, and declarative resource details still differ.
The 1.0 target excludes Python plugins, not declarative packages.

## Totals

<!-- BEGIN PARITY TOTALS -->
| Area | Done | Partial | Missing | Out of scope |
| --- | ---: | ---: | ---: | ---: |
| Editing and selection | 56 | 2 | 0 | 0 |
| Search and navigation | 57 | 6 | 0 | 0 |
| Files, projects and sessions | 54 | 7 | 1 | 1 |
| Windows, tabs and display | 73 | 8 | 0 | 1 |
| Language, completion and Vintage | 55 | 10 | 0 | 0 |
| Customization and resources | 43 | 11 | 0 | 0 |
| Build, Git and command line | 42 | 11 | 0 | 0 |
| Scope exclusions | 0 | 0 | 0 | 5 |
| **Total** | **380** | **55** | **1** | **7** |
<!-- END PARITY TOTALS -->

## How to read the audit

- Baseline audit: Velocity Round 34 lane B, based on `bc4c141`. Round 36 lane C
  rechecks Editing, Search, Files, Language, and Build against `bbf4965`.
  A **done** row has a runtime reader, executable command or named regression;
  it does not imply pixel parity, native UI automation, or identical performance.
  **Partial** says what works and what still differs. **Missing** means no
  equivalent reader or path was found, not merely that there is no default key.
- Sources are the [documentation index][index], its linked pages, the
  [public community documentation][community] for features not on that index,
  the [API reference][api] for externally visible semantics, and both public
  release histories. No Sublime installation resources were consulted for the
  comparisons. A docs-only reference is for the 4213 target, not an assertion
  that the feature first shipped in 4213. `Dnnnn.k` and `Vnnnn.k` are the
  one-based bullet ordinal within Build `nnnn` on [Download][releases] and
  [Dev Builds][dev], respectively, across all topic subheadings.
- Deduplication: one capability is counted once even when a dev build and its
  stable rollup both announce it. Multi-setting bullets have separate rows.
  Edition/platform-only, Python-only and maintenance additions are explicitly
  grouped in the coverage ledger; these groups count once per matrix row, not
  once per bullet. Bug fixes, security changes and implementation details are
  not automatically independent editor capabilities.
- Time estimates are engineering time for one developer with headless coverage;
  native UI/assistive-technology confirmation is separate. Negative evidence
  uses the corresponding runtime module and the [reader-linked settings
  inventory](settings.md), not the absence of a matching English caption.

## Editing and selection

| Feature | Sublime source (URL + build) | Velocity status | Evidence | Notes |
| --- | --- | --- | --- | --- |
| Add next occurrence, skip occurrence, undo last addition | [Multiple selection][multi] | done | `find_under_expand`, `find_under_expand_skip`, `soft_undo`; `find_incremental_wrap_skip_whole_word_and_case` | |
| All occurrences of the selection or caret word | [Multiple selection][multi] | done | `find_all_under`; `find_all_selected_literals_and_caret_words` | |
| Split into lines, or words for one line | [Multiple selection][multi], [D4107][releases] | done | `split_selection_into_lines`; `split_lines_keeps_reversed_unicode_blank_cr_and_eof_edges` | |
| Add caret above/below | [Multiple selection][multi] | done | `select_lines`; `column_add_handles_tabs_and_wide_characters` | |
| Collapse to single selection | [Multiple selection][multi] | done | `single_selection`; `primary_survives_document_order_and_single_selection` | |
| Rectangular pointer selection | [Column selection][column] | done | `mouse_select::DragSelect`; `app_mouse.rs` | Mac gesture; other OS bindings grouped below. |
| Rectangular selection by keyboard | [Column selection][column] | done | `select_lines` and the Mac Velocity keymap; `select_lines_probe_table_maps_both_ends_and_goals` | |
| Expand Selection (successive levels) | [D4107.151][releases], [V4050.27][dev] | done | `expand_selection` | |
| Expand to word | [Commands][commands] | done | `expand_selection`; `expand_word_line_indentation_and_split` | |
| Expand to line | [Commands][commands] | done | `repeated_line_expansion_takes_the_next_line` | |
| Expand to brackets | [Commands][commands] | done | `standalone_bracket_targets_and_recorded_macro_apply_once_per_command` | |
| Expand to indentation | [Commands][commands] | done | `expand_word_line_indentation_and_split` | |
| Expand to scope | [V4052.5][dev] | done | `expand_selection` with `to: scope`; `editing.rs` | Syntax scopes must be available. |
| Expand to tag | [Commands][commands] | done | `expand_selection` with `to: tag`; `editing.rs` | |
| Expand to Block; paragraph as a separate target | [D4121.25][releases] | partial | `r36_block_command_and_report_a_bug_menu` | Block now has its own menu and palette target for blank-line blocks. 4121 says Block took over the old Paragraph behavior, but does not define the newer Paragraph boundaries for indented prose or comments; Velocity's existing Paragraph behavior is unchanged. |
| Insert/delete graphemes at many carets | [Multiple selection][multi] | done | `insert_replaces_unicode_multicursors_and_paste_distributes`, `delete_zwj_and_combining_graphemes` | |
| Word motions and deletion | [Commands][commands] | done | `words_subwords_ends_and_stops`, `delete_words_forward_backward_and_newlines` | |
| `word_separators` | [Settings][settings] | done | `vel-buffer/ops.rs`; [setting readers](settings.md) | |
| `sub_word_separators` | [D4142][releases], [V4130][dev] | done | `vel-config/settings.rs` to `vel-buffer/ops.rs` | |
| `move_to_limit_on_up_down` | [Settings][settings] | done | `up_down_document_limit_table_including_extend_and_input_focus` | |
| Undo/redo edit groups | [Commands][commands] | done | `typing_group_word_space_word_and_selection_break` | Persistent history audited under Files. |
| Soft undo/redo including selection and granular typing | [Commands][commands] | done | `soft_undo_each_typing_transaction_and_redo_invalidation` | |
| Duplicate line or selection | [Commands][commands] | done | `duplicate_selected_text_or_caret_lines` | |
| Delete line, including multiple selected rows | [Commands][commands] | done | `run_command("delete_line")` via `vel-commands`; `line_commands_transform_separate_selected_blocks` | |
| Swap lines up/down | [D4107][releases], [V4050][dev] | done | `swap_blocks_and_preserve_final_newline_and_column` | |
| Join lines | [D4166][releases], [D4213][releases] | done | `join_lines_matches_sublime_black_box`, `join_preserves_interior_selection_and_bracket_spacing` | Selections stay attached. Rules confirmed against Sublime 4215 as a black box (2026-11-22): no space after `(`, `[`, `{` or before `)`, `]`, `}`; blank lines vanish; a selection's last line counts even when it ends at the first column, so Select All also drops the final newline. |
| Sort lines | [Commands][commands], [D4107][releases] | done | `sorting_without_a_selection_sorts_the_whole_buffer`, `sort_reverse_multiple_blocks_unicode_no_final_lf` | |
| Sort selection | [Commands][commands] | done | `sort_selection`; `selected_strings_move_between_variable_length_slots_without_touching_gaps` | |
| Permute lines: reverse, unique, shuffle | [Commands][commands] | done | `line_permutations_deduplicate_stably_and_preserve_line_structure`, `shuffle_is_reproducible_and_preserves_the_multiset` | |
| Transpose | [Commands][commands] | done | `transpose_unicode_graphemes_multicursor_and_selection` | |
| Upper/lower/title/swap case | [Commands][commands] | done | `unicode_case_length_changes_and_title_swap` | |
| Convert identifier case (camel, Pascal, snake, kebab and related forms) | [D4142.6][releases], [V4128.1][dev] | done | `identifier_cases_cover_acronyms_unicode_and_multi_caret_overlap` | |
| Wrap Paragraph | [D4107][releases], [V4050][dev] | done | `paragraphs_wrap_once_with_display_width_and_retain_blank_separators`, `wrapping_prefix_probe_table_keeps_comments_and_code_separate` | |
| Line and block comment commands | [D4142][releases], [V4130][dev] | partial | `round21_comment_flags_variants_and_round_trip`; `embedded_languages_use_their_own_comment_markers` | Numbered markers, indent and case flags work. Black-box Sublime 4215 (2026-11-22): an embedded language's marker set replaces the host's as a unit (`// var a = 1;` in `<script>`, never `// … -->`), and markers are inserted verbatim (`/*p { color: red; }*/`). How a selection crossing host and embedded boundaries is split remains unconfirmed. |
| Mark, kill/yank and exchange mark | [Commands][commands] | done | `named_mark_kills_yank_and_repeat_use_application_state` | |
| Bookmarks, navigation, clear, and select all | [Commands][commands], [D4166][releases] | done | `bookmark_regions_and_marks_follow_edits_navigation_and_kills`; `toggle_bookmark`, `select_all_bookmarks` | |
| Paste and paste-and-indent | [Commands][commands] | done | `clipboard_snippets_newline_tabs_and_typing_groups`; `paste_and_indent` | |
| `copy_with_empty_selection` | [Settings][settings] | done | `settings_effect_tests::edit_probe`; [reader](settings.md) | |
| Edit > Copy as HTML | [D4107.110][releases], [V4092.4][dev] | done | `copy_as_html` in `app_clipboard.rs`; clipboard mock tests | Bounded synchronous export; large selections are refused. |
| Indent, unindent and reindent | [Indentation][indent] | done | `indent_unindent_trim_multiselect_lines_once`; `reindent` reads syntax metadata | |
| `tab_size` | [Indentation][indent] | done | `EditorSettings::tab_size` in `vel-config/settings.rs` | |
| `translate_tabs_to_spaces` | [Indentation][indent] | done | `set_translate_tabs`, `clipboard_snippets_newline_tabs_and_typing_groups` | |
| `use_tab_stops` | [Indentation][indent] | done | `leading_spaces_backspace_to_tab_stops` | |
| `detect_indentation` | [Indentation][indent] | done | `indentation_choices_survive_resource_and_external_reload_and_project_watching` | |
| Convert file indentation between tabs and spaces | [Indentation][indent] | done | `expand_tabs`, `unexpand_tabs`, `line_prefix_transforms_preserve_caret_columns` | |
| `auto_indent` | [Indentation][indent] | done | `newline_with_blank` and `settings_effect_tests::edit_probe` | |
| `smart_indent` | [Indentation][indent] | done | `editing.rs`; `settings_effect_tests::edit_probe` | |
| `indent_to_bracket` | [Indentation][indent] | done | `bracket_indent_both_values_and_live_syntax_override` | |
| `trim_automatic_white_space` | [Indentation][indent] | done | `newline_with_blank`; [reader](settings.md) | |
| `shift_tab_unindent` | [Settings][settings] | done | `settings_effect_tests::edit_probe`; [reader](settings.md) | |
| Pair brackets and quotes | [Settings][settings] | done | `auto_match_enabled`, `editing.rs` | |
| `auto_close_tags` | [Settings][settings] | done | `close_tags_table_cold_stale_markup_void_quotes_comments_and_xml_case` | |
| Record/run/save macro and repeat last command | [Commands][commands], [D4107][releases], [V4075][dev] | done | `registry_domains_chains_macros_repeat_and_window_actions`, `panels/macro_commands.rs` | Includes find commands. |
| `chain` sequential dispatch | [D4107.121][releases], [V4103.3][dev] | done | `registry_domains_chains_macros_repeat_and_window_actions` | |
| Sum numeric selections in the status bar | [D4107][releases], [V4050][dev] | done | `app_status::selection_sum`, `Panels` status text; `r34_status_column_units_and_sum` | Automatic for multiple finite numeric selections; no sum for mixed text or empty carets. No setting in the public notes. |
| Undo jumps / Jump Back and Forward | [D4107][releases] | done | `jump_back`, `jump_forward` in `panels.rs`; `project_symbols_definition_previews_unicode_locations_and_bounded_history` | |
| `toggle_comment` comment `variant` | [D4142.77][releases], [V4130.16][dev] | done | `editing.rs` selects the requested metadata variant | |
| `append` with `disable_tab_translation` | [D4107.314][releases], [V4050.109][dev] | done | `vel-commands::text_ops::append`; `r35_append_translates_tabs_unless_disabled` | At EOF, normal append applies tab stops when `translate_tabs_to_spaces` is on. The command argument preserves tabs; it is not a preference. |

## Search and navigation

| Feature | Sublime source (URL + build) | Velocity status | Evidence | Notes |
| --- | --- | --- | --- | --- |
| Find next/previous, highlight and match count | [Search][search] | done | `find_next`, `find_prev`, `Panels::update_find_status` | |
| Literal vs regex Find/Replace | [Search][search] | partial | `vel-search/text.rs`, `vel-regex/perl.rs`, `vel-search/files/multiline.rs` | Literal and accepted Rust regexes retain the fast path. The fallback supports lookahead and lookbehind, numeric and named backreferences, atomic groups, `\G`, conditionals and Oniguruma-supported subroutine calls, including possessive repeats when using the fallback. Both engines share replacement formatting. Tests cover Sublime 4215's `(?=b)` to `-` and `^` to `> ` behavior, including the empty line after a final LF. Patterns that Rust already accepts (e.g. `a++` or `(?R)`) keep Rust semantics unless another Perl-only construct selects the fallback. Other Boost-only syntax and dialect differences remain; see [Round 40 limits](performance.md#round-40-perl-regex-fallback). |
| Match case | [Search][search] | done | `text::Options::case_sensitive`; `find_incremental_wrap_skip_whole_word_and_case` | |
| Whole word | [Search][search] | done | `text::Options::whole_word`; same regression | |
| Wrap on navigation | [Search][search] | done | `text::Options::wrap`; same regression | |
| Find in selection | [Search][search], [D4151][releases] | done | `Panels::capture_find_scope`, `toggle_find_option` | Zero-width selections and live remapping tested. |
| Incremental find and Escape restores position | [Search][search] | done | `Panels::cancel_incremental`; `panel_input_focus_find_replace_and_cancel_restore_preview` | |
| Find All | [Search][search] | done | `find_all`, `find_panel_seeding_table_and_close_after_all_values` | |
| Replace next/all, Preserve Case | [Search][search] | done | `replace_all`; `panel_input_focus_find_replace_and_cancel_restore_preview` | |
| `find_selected_text` | [Settings][settings] | done | `find_panel_seeding_table_and_close_after_all_values` | |
| `auto_find_in_selection` including `find_only` and `replace_only` | [V4097.8][dev], [Search][search] | done | `Panels::action`; `settings_effect_tests::find_probe` | |
| `close_find_after_find_all` | [D4107.193][releases], [V4092.7][dev] | done | `find_panel_seeding_table_and_close_after_all_values` | |
| `close_find_after_replace_all` | [D4107.194][releases], [V4092.8][dev] | done | `explicit_find_shortcuts_close_even_when_buttons_stay_open` | |
| `regex_auto_escape` | [D4151.19][releases], [V4145.5][dev] | done | `Panels::set_query_from_buffer`, `settings_effect_tests::find_probe` | Explicit command patterns remain verbatim. |
| `highlight_find_results_in_scrollbar` | [V4081.10][dev] | done | `Panels::scrollbar_find_highlights`; `r35_find_display_limits_preserve_navigation_and_mark_both_gutters` | Uses the minimap gutter when visible, otherwise the scrollbar track. Off by default to retain the previous display. |
| `find_scroll_highlights_limit` | [D4121.27][releases], [V4114.5][dev] | done | `Panels::scrollbar_find_highlights`; `r35_find_display_limits_preserve_navigation_and_mark_both_gutters` | Hide the marks when match count exceeds the configured limit (4096 when enabled; zero disables the cap). Text matches are unaffected. |
| `find_highlight_matches_max_size` | [D4121.27][releases], [V4114.5][dev] | done | `Panels::start_find`; `r35_find_display_limits_preserve_navigation_and_mark_both_gutters` | Byte limit for automatic literal highlights; explicit navigation and editing still work. Default 0 retains the earlier behavior. |
| `find_regex_highlight_matches_max_size` | [D4121.27][releases], [V4114.5][dev] | done | `Panels::start_find`; `r35_find_display_limits_preserve_navigation_and_mark_both_gutters` | Independent regex byte limit; default 0 retains the earlier behavior. |
| `use_find_clipboard` (macOS system Find pasteboard) | [D4121.49][releases], [V4115.12][dev] | done | `vel-platform::FindPasteboard`; `r35_find_pasteboard_has_its_own_channel_and_respects_seeding` | Default on per the documented opt-out. ⌘E writes the named Find pasteboard; explicit query and single-line selection seeding take precedence on Find focus. |
| Find in Files parallel traversal and ordered results | [D4151][releases], [V4130][dev] | done | `find_in_files_start`, `vel-search/files.rs::Task` | |
| Find/Cancel Find in Files menu item | [D4107.190][releases], [V4075.5][dev] | done | `Panels::cancel_files`; `r33_cancel_find_in_files_from_menu_and_palette`; `find_results_tab_records_escape_and_panel_close_cancellation` | Dedicated menu and palette command enabled while searching; closes the worker and records cancellation in the chosen destination. |
| Find in Files Where locations and include/exclude patterns | [Search][search], [File patterns][patterns], [D4151][releases], [V4149][dev] | partial | `r36_where_combines_scopes_filters_and_unique_live_buffers` | Recognizes explicit `./` roots, including quoted directory names, and searches a live buffer only once even for clones. The docs do not specify the base for relative literals in multi-root projects or escaping for literal quotes and commas. |
| Find in Files option Use .gitignore / `use_gitignore` | [D4107.186][releases], [V4072.1][dev], [Theme control][themes], [public support discussion][find-default] | done | `find_in_files_gitignore_controls_disk_walk_not_open_files_and_invalidates_replace`; `find_in_files_where_menu_sets_scope_without_changing_the_query` | Default on based on public fresh-install/default reports; independent of symbol indexing and Goto. Explicit live scopes still search unsaved buffers. |
| Find in Files result buffer vs panel | [Search][search], [Themes][themes], [D3](behavior.md) | done | `panels/find_results.rs`, `Panel::Results`; `find_results_tab_format_scopes_append_and_reopen`, `find_results_tab_toggle_persists_and_both_destinations_navigate` | One editable scratch tab per window, append and reveal each new search. The persistent Use Buffer toggle selects the destination for the next run. |
| Find Results jumps | [Search][search], [D4166][releases] | done | `Panels::jump_tab_result`, `Panels::jump_result`; `find_results_tab_return_double_click_and_command_click_follow_edited_regions` | Return, double-click, and D3's Command-click follow tracked locations on numbered rows. F4/Shift+F4 and Goto > Results walk the latest search in either destination. |
| Find in Files Preserve Case and confirmed replace | [D4107.187][releases], [V4095.10][dev] | done | `replace_in_files`, `FileChange::prepare_with_decode`, `Panels::report_file_replacements` | Uses on-disk transaction safeguards; asks to save edited target buffers first. Writes the report to the same search destination. |
| Find in Files large results and graceful degradation | [D4107][releases], [V4075.7][dev] | partial | `vel-search/files/multiline.rs`; `find_results_tab_streams_while_typing_within_frame_budget` | Bounded queue, previews and ordered batched output work. The notes do not specify when multiline regex falls back, which oversize rows are skipped, or how much earlier report text is retained under pressure. |
| `find_in_files_max_file_size` | [D4142.23][releases], [V4134.7][dev] | done | `vel-search::files::Limits`; `r35_find_in_files_input_size_context_and_error_policy` | Skips candidates above the configured byte count before decoding. Existing unlimited default retained. |
| `find_in_files_max_result_size` | [D4126][releases] | done | `Panels::ResultBudget`; `r35_find_in_files_output_budget_does_not_limit_replacement` | Caps each report without hiding matches from replacement; earlier D3 reports are retained. Default 0 keeps the previous unlimited output. |
| `find_in_files_context_characters` | [D4166][releases] | done | `vel-search::files::Line::limit_preview`; `r35_find_in_files_input_size_context_and_error_policy` | A positive setting trims Unicode columns without losing the original jump column. Zero keeps the existing disk and live-buffer preview rules. |
| `find_in_files_context_lines` | [D4151.15][releases], [V4149.5][dev] | done | `r36_context_setting_counts_both_sides_and_obeys_context_toggle`; [reader](settings.md) | Applies to the next search and honors Context. Velocity retains one as its default and bounds the value to 100 per side. |
| `find_in_files_suppress_errors` | [D4151.14][releases], [V4149.5][dev] | done | `vel-search::files::Limits`; `r35_find_in_files_input_size_context_and_error_policy` | Suppresses traversal/read diagnostics only. Default false retains error reporting. |
| `find_in_files_side_by_side` | [D4151.16][releases], [V4145.4][dev] | done | `Panels::open_result_target`; `r35_current_result_preserves_next_previous_and_opens_beside_tab` | Uses tab multi-select to keep the Find Results tab visible beside opened matches. Opt-in; panel destination unchanged. |
| Find in Files tab multi-select modifiers | [D4151][releases], [V4145][dev] | partial | `Panels::jump_tab_result`; [native check](manual-checks.md#6-round-36-parity) | D3's jump gestures stay in place. Check modifier delivery, focus and preview retention with a multi-selected tab group; the changelog does not specify the complete modifier matrix. |
| Multiline Find/Replace fields | [D4180][releases] | done | `Input::set_multiline` in `panels.rs` | |
| `show_panel`: `pattern` / `replace_pattern` | [D4126][releases], [V4123][dev], [Commands][commands] | done | `Panels::action` applies both arguments | Native declarative command, not a Python dependency. |
| Current result (retain position) | [D4166.19][releases], [V4153.11][dev] | done | `Panels::current_result`, `Builds::current_result`; `r35_current_result_preserves_next_previous_and_opens_beside_tab` | Goto > Results and palette reopen the current match without advancing the next/previous cursor. |
| Next/previous result across builds and file search | [Build systems][build], [Commands][commands] | done | `Panels::navigate_result`, `builds.navigate`; Goto > Results | |
| Goto Anything: fuzzy paths, preview, and recent ranking | [Goto][goto], [D4107][releases], [D4213][releases], [U04 round 43](behavior.md#round-43-u01-and-u04-details) | done | `goto_uses_shared_files_and_retries_unopened_file_symbol`, `r41_tabless_scene_and_commit_preserve_view`, `r43_goto_publishes_relative_details_and_keeps_outside_paths`, `fuzzy::Worker` | Unopened previews have no tab; sidebar previews still have an italic tab. File details are root-relative, prefixed by the folder name when multiple roots are open; outside paths abbreviate HOME. Ranker is original. |
| Goto Anything: `:` line/column | [Goto][goto] | done | `vel-search/goto.rs::Query`, `Panels::preview_current` | |
| Goto Anything: `@` symbol | [Goto][goto] | done | `goto_uses_shared_files_and_retries_unopened_file_symbol` | Includes combined file + symbol. |
| Goto Anything: `#` fuzzy word | [Goto][goto] | done | `panels.rs::Local::Words`; `vel-search/goto.rs::Target::Word` | |
| Goto Anything: keep/replace/side-by-side modifier actions | [Tab multi-select][tabs], [D4107][releases] | done | `intelligence::OpenOptions`, `panels.rs` overlay key dispatch | Exact modifier support depends on window/overlay context. |
| `goto_anything_exclude_gitignore` | [D4142.11][releases], [V4130.6][dev] | done | `goto_anything_gitignore_is_opt_in_and_cannot_readd_ignored_open_tabs` | Default false; worker refreshes on new overlay or changed project file list, not every in-place rule edit. |
| `goto_anything_file_preview` | [D4180.18][releases], [V4170.6][dev] | done | `Panels::show_quick_target`; `r34_goto_file_preview_off_still_accepts_target` | Off defers file preview until acceptance. Local navigation is unchanged; the default preview for unopened files has no tab (U04). |
| Command palette fuzzy filtering and dispatch | [Palette][palette], [V4094.1][dev] | done | `palette_filters_and_rechecks_actual_command_enablement` | Includes original bilingual captions/aliases. |
| `.sublime-commands` discovery and arguments | [Palette][palette], [Packages][packages] | done | `Configuration::load`; `vel-config/ui.rs` | Python-defined commands excluded. |
| Goto Symbol in File; location metadata | [D4107][releases], [V4050][dev] | done | `show_overlay` with `@term`; `panels/details.rs` | |
| Goto Symbol in Project | [Indexing][indexing], [D4107][releases] | done | `project_symbols_definition_previews_unicode_locations_and_bounded_history` | Includes short symbols, limited by syntax metadata. |
| Goto Definition | [Indexing][indexing], [Tab multi-select][tabs] | done | `definition_uses_open_tabs_with_index_disabled_and_current_file_first` | |
| Goto Reference | [Tab multi-select][tabs], [Commands][commands] | done | `goto_reference`, `intelligence.rs` | Lexical/reference metadata rather than language-server semantics. |
| Definitions hover, types and links | [Indexing][indexing], [D4107][releases], [D4166][releases] | done | `Panels::click_definition`, `show_definitions`, hover state in `panels.rs` | |
| `show_definitions` | [Indexing][indexing] | done | `panels.rs` hover checks | |
| `index_files` | [Indexing][indexing] | done | `intelligence::index_options`; operational settings sweep | |
| `index_workers` | [Indexing][indexing] | done | `intelligence::index_options`, `vel-index/service.rs` | Rust workers, not separate Sublime indexer processes. |
| `index_exclude_gitignore` | [Indexing][indexing], [D4107][releases], [V4050][dev] | done | `vel-index/service.rs::Ignores`; [reader](settings.md) | Default true. |
| `index_skip_unknown_extensions` | [Indexing][indexing], [D4107][releases] | done | `intelligence::index_options` | |
| `index_exclude_patterns` | [Indexing][indexing], [File patterns][patterns] | done | `vel-index/service.rs`; [reader](settings.md) | |
| `index_include_patterns` | [Indexing][indexing], [File patterns][patterns] | done | `vel-index/service.rs`; [reader](settings.md) | |
| Index status and pause/resume or enable | [Indexing][indexing], [D4180.30][releases], [V4170.1][dev] | partial | `r36_index_status_toggle_changes_effective_preference`; [native check](manual-checks.md#6-round-36-parity) | Status window has an enable control wired to the User `index_files` preference. Check mouse/Space and worker progress in the live auxiliary window. |
| Indexing logging and diagnostics | [Indexing][indexing] | partial | `Panels::refresh_index_report`, `intelligence.rs` | Status shows per-root progress and worker logs. The [API][api] documents a Python `log_indexing` switch; the editor guide does not define a distinct non-Python console toggle or message/filter contract. |
| Switch header/implementation including compound extensions | [Tab multi-select][tabs], [D4107.122][releases], [V4095.14][dev] | done | `switch_file`, `Workspace::switch_file` | |
| Run Find commands without panel focus | [D4107.177][releases], [V4097.11][dev] | done | `App::apply_named_action`, `Panels::action`, explicit find shortcuts test | |

## Files, projects and sessions

| Feature | Sublime source (URL + build) | Velocity status | Evidence | Notes |
| --- | --- | --- | --- | --- |
| Multi-file Open, New File, Save, Save As and Save All | [Commands][commands] | done | `tabs_cli_positions_edit_save_and_format_dirty_state`, `save_all_skips_clean_tabs_and_continues_after_cancel_and_failure` | |
| `open_file` with `encoded_position` | [D4142.71][releases], [V4127.18][dev] | done | `r36_open_file_encoded_position_override_preserves_legacy_suffixes` | True enables row/column parsing; false keeps the path literal. Absent preserves Velocity's legacy shortcut; CLI suffix handling is unchanged. |
| `open_file` overrides: transient, force group, clear right, clone | [D4151.49][releases], [V4149.16][dev] | partial | `r36_open_file_flags_keep_group_views_and_unselect_without_closing`; [native check](manual-checks.md#6-round-36-parity) | Reads all four flags. Clones share text and clear-to-right only deselects; Velocity makes it additive. Check preview promotion and placement; Velocity uses a provisional preview sheet rather than a guaranteed tabless view. |
| Fast Save All on large tab sets | [D4213][releases] | partial | `App::save_tab_with_path`, `vel-fileio` | Saves multiple tabs and handles shared documents. 4213 gives neither a target throughput nor the file-count, worker and storage conditions of its improvement. |
| `io_workers` for file I/O concurrency | [D4213.16][releases], [V4205.7][dev] | done | `vel_fileio::IoWorkers`, `Workspace::open_mode`; `r35_worker_limit_controls_real_job_admission`, `r35_io_workers_and_automatic_syntax_size_setting_reach_file_open` | Positive values bound regular open, reload and save jobs per window (1–64); zero retains the previous scheduling. Restore watchdog, binary preview and traversal have independent workers. |
| Open Folder in New Window | [D4121.21][releases], [CLI][cli] | done | `prompt_open_folder_in_new_window`; `folder_picker_request_mounts_only_in_a_new_workspace` | New workspace even if current one is empty; native picker is folder-only. |
| Open Recent files, folders, projects; clear history | [D4142][releases], [D4107][releases], [V4131][dev] | done | `open_recent_file`, `open_recent_folder`, `open_recent_project`, `clear_recent_items` | Native menu populated from process-wide history, including last-window-close state. |
| Reopen Closed File, window-local history | [D4151][releases], [V4131][dev] | done | `reopen_last_file`, `Workspace::closed_files` | |
| Recent Projects: remove deleted entries | [D4107.120][releases], [V4092.5][dev] | done | `r36_remove_deleted_projects_preserves_other_history_and_stale_windows` | Remove Deleted drops missing project paths from shared recents without clearing files, folders or still-present projects. |
| `update_system_recent_files` | [D4142.5][releases], [D4143][releases], [V4127.2][dev] | done | `App::publish_open_route` / recent-file reader in `app.rs` | macOS document recents. |
| Revert with unsaved-change confirmation | [D4142][releases] | done | `revert_cancel_confirm_and_failed_load_preserve_work` | |
| Save/reopen/convert with encoding | [Commands][commands], [D4126][releases] | done | `reopen_encoding`, `save_encoding`, `convert_to_encoding`; `vel-fileio/tests/encodings.rs` | |
| Central European (Mac) | [D4107.136][releases], [V4075.14][dev] | done | Encoding registry in `vel-fileio`, File menu reader/dispatch | |
| GB18030 | [D4126.2][releases], [V4123.1][dev], [V4124.1][dev] | done | `vel-fileio/tests/encodings.rs`; Format menu and status | |
| CP862 | [D4126.3][releases], [V4122.1][dev], [V4124.1][dev] | done | `r36_dos_codepages_roundtrip_and_strict_save`; File encoding menus | Explicit reopen/save, fallback and format persistence use the Hebrew DOS codec. |
| Nordic DOS CP865 | [D4142.15][releases], [V4128.2][dev] | done | `r36_dos_codepages_roundtrip_and_strict_save`; File encoding menus | Explicit reopen/save, fallback and format persistence use the Nordic DOS codec. |
| Newline formats and conversion | [Commands][commands], [D4121][releases] | done | `set_line_ending`, `vel-fileio/tests/io.rs` | LF, CRLF and CR. |
| `default_encoding` | [Settings][settings] | done | `vel-fileio::DecodeOptions`; [reader](settings.md) | |
| `fallback_encoding` | [Settings][settings] | done | `vel-fileio/tests/decoding_policy.rs` | |
| `default_line_ending` | [Settings][settings] | done | `vel-fileio::DecodeOptions`; [reader](settings.md) | |
| `atomic_save` | [Settings][settings] | done | `settings_effect_tests::save_probe`; `vel-fileio` | |
| `ensure_newline_at_eof_on_save` | [Settings][settings] | done | `settings_effect_tests::save_probe` | |
| `trim_trailing_white_space_on_save` including `not_on_caret` | [D4107.125][releases], [V4085.3][dev] | done | `vel-fileio` save options; `settings_effect_tests::save_probe` | |
| `trim_only_modified_white_space` | [D4107.126][releases], [V4050.18][dev] | done | `vel-fileio` save options; `settings_effect_tests::save_probe` | |
| `save_on_focus_lost` | [Settings][settings], [D4180][releases] | done | `focus_lost_saving_is_opt_in_and_skips_conflicts_readonly_and_untitled` | |
| `reload_file_on_change` | [D4107.166][releases], [V4050.52][dev] | done | `reload_decisions_and_failed_load_do_not_overwrite_edits`; [reader](settings.md) | |
| `reload_file_in_background` | [D4200.15][releases], [V4198.9][dev] | done | `workspace.rs` watcher/reload; `r33_background_reload_clean_active_inactive_and_dirty`, `r33_background_reload_follows_shared_buffer_across_clones` | Clean inactive buffers reload in the background by default; disabling defers their reload until activation. |
| `close_deleted_files` | [D4107.78][releases], [V4075.12][dev] | done | `close_deleted_files_closes_clean_watched_tabs`; `workbench.rs` restore | |
| Data-driven binary (hex) tab | [D4126][releases] | done | `vel-largefile/hex.rs`, `binary.rs` | Read-only byte view. |
| Binary syntax label | [D4126][releases] | done | `app_status.rs` format/status model | |
| Image previews (PNG, JPEG, GIF) | [D4180][releases] | done | `open_requests::decode_image`, `image_ui.rs` | GIF shows a static decoded frame. |
| TGA preview | [D4107.77][releases], [V4096.4][dev] | done | `image_decode.rs`, `r33_tga_preview_and_bounded_bad_headers` | Existing MIT/Apache-2.0 `image` crate, TGA feature; no new dependency. |
| PSD preview | [D4107.77][releases], [V4096.4][dev] | out of scope | `image_decode::tests::r35_psd_stays_in_byte_view`; [image codec list](https://docs.rs/image/0.25.10/image/codecs/index.html), [ImageIO thumbnail API](https://developer.apple.com/documentation/imageio/cgimagesourcecreatethumbnailatindex(_:_:_:)) | `image` has no PSD codec. A headless ImageIO probe advertises `com.adobe.photoshop-image`, but the thumbnail API does not guarantee composite-only decoding or bound layer/decompression work. Keep byte view until a vetted bounded decoder and fixtures for missing/malformed merged data exist. |
| WebP preview | [D4151.5][releases] | done | `vel-platform/image_io.rs`, `r33_webp_preview_and_bounded_bad_headers`, `image_webp` | macOS ImageIO, first frame only; no additional Rust codec dependency. |
| `image_file_patterns` | [D4180.19][releases], [V4170.9][dev], [File patterns][patterns] | done | `open_requests::image_matches`, `Tab::sync_image_file` | WebP/TGA included by default; explicit overrides win. |
| Explicit Open File as Image | [D4180.20][releases], [V4170.8][dev] | done | `r33_open_as_dispatch_enablement_and_session_choice` | File menu, palette and tab popup; in-memory per-file override; failure falls back to hex. |
| Explicit Open File as Text | [D4180.20][releases], [V4170.8][dev] | done | `r33_open_as_dispatch_enablement_and_session_choice` | Binary files use the read-only hex view; does not reinterpret binary bytes as editable text. |
| Image-tab context commands | [D4180.21][releases], [V4170.7][dev] | partial | `r33_open_as_dispatch_enablement_and_session_choice`; [native check](manual-checks.md#6-round-36-parity) | Check context menu and targeting for image, binary and text views. The notes do not list a separate image clipboard/zoom menu contract. |
| Browser printing (File > Print) | [D4107.109][releases], [V4092.3][dev], [V4093.2][dev] | done | `print_exports_current_selection_or_document_and_reaps_the_browser_file`, `vel-export::temporary_print_file` | Active scheme, including hashed colors; bounded background parse (8 MiB/250k lines, 30 s). No live browser test. |
| `.sublime-project` folders and project settings | [Projects][projects] | done | `vel-session/project.rs`; `settings_project_view_and_user_keymap_precedence_reload` | |
| Per-project build systems | [Projects][projects], [Build systems][build] | done | `vel-session/project.rs`, `velocity/build.rs` | |
| Quick Switch Project / open / save / close project | [Projects][projects], [Commands][commands] | done | `quick_switch_project`, `open_project`, `close_project`, `project_save_as_preserves_relative_paths_unknown_folder_keys_and_absent_defaults` | |
| Quick Switch Project modifier to open a new window | [D4107.131][releases], [V4088.7][dev] | partial | `r36_project_choice_modifier_and_companion_dispatch`; [native check](manual-checks.md#6-round-36-parity) | Build 4107 names Command on Mac. The chooser requests a separate window with Command, not Control. Check native click, new-window delivery and existing-window focus. |
| `open_project_or_workspace` file dispatch | [D4107.313][releases], [V4050.108][dev] | partial | `r36_project_choice_modifier_and_companion_dispatch` | Opens a named project or a same-stem companion and keeps an already active project. The [workspace guide][workspace-guide] describes multiple workspaces but not how a differently named file identifies its project. |
| `.sublime-workspace` interoperability | [Projects][projects] | partial | `r36_foreign_workspace_survives_velocity_checkpoints` | Recognizes and preserves foreign companion JSON, keeping Velocity checkpoints separately. [Public docs][workspace-guide] describe the stored state, not keys for file paths, unsaved text, layout or selection; no foreign state is imported. |
| `file_include_patterns` | [Projects][projects], [File patterns][patterns] | done | `vel-search/project.rs::Rules::with_includes` | Per-folder includes. |
| `file_exclude_patterns` | [Projects][projects], [File patterns][patterns] | done | `Rules::allows`, project settings sweep | |
| `folder_include_patterns` | [Projects][projects], [File patterns][patterns], [V4067][dev] | done | `Rules::with_includes`, project-root `//` matching | |
| `folder_exclude_patterns` | [Projects][projects], [File patterns][patterns], [V4067][dev] | done | `Rules::allows_entry`, ancestor pruning | |
| `binary_file_patterns` | [File patterns][patterns], [D4142][releases], [V4067][dev] | done | `Rules::allows_entry` searchable switch; [reader](settings.md) | Sidebar keeps these files. |
| Project symlinks / `follow_symlinks` | [Projects][projects], [D4142][releases] | done | `symlink_directories_expand_and_search_without_cycles_and_can_be_disabled` | |
| `*.ext`, `?`, relative paths, `//` root patterns and directory matching | [File patterns][patterns] | done | `vel-search/project.rs::Patterns`, `vel-search/tests/round20_pattern_timing.rs` | See Where and spell/syntax consumers separately. |
| Restore open tabs, splits, selections and scroll | [Projects][projects], [D4107][releases] | done | `vel-session/tests/session.rs`, `session_restore.rs`, `legacy_session_pixels_use_the_restored_font_size_and_padding` | |
| `hot_exit`, including last-window policy | [D4107.74][releases], [V4050.69][dev] | done | `HotExit::retain`, `legacy_hot_exit_false_and_recent_encoding_commands_are_wired` | |
| `hot_exit_projects` | [D4142.7][releases], [V4136.5][dev] | done | `Sessions::publish_project`, `App::prepare_window_close`; `r34_hot_exit_projects_is_independent` | On follows general Hot Exit; off omits unsaved workspace content and restores the dirty-close prompt. App recovery remains independent. |
| `remember_open_files` | [Settings][settings] | done | `settings_effect_tests` session exercise; `App::checkpoint_reason` | |
| `remember_layout` | [V4114.7][dev], [D4121][releases] | done | `Sessions::publish`, `Workspace::restore_window_inner`, `App::window_app`; `r34_remember_layout_with_and_without_hot_exit` | Auto keeps saved splits unless Hot Exit is off. Explicit true also starts a fresh window with the current split and first group focused. D5 still controls which windows return. |
| `remember_workspace` (OS virtual desktop/space) | [D4107.56][releases], [V4098.1][dev] | missing | `Window::frame` continues to restore geometry only | Public [AppKit window collection behavior][appkit-spaces] can join all Spaces or follow the active one, but does not expose a stable Space ID or a targeted move. No private APIs or inert preference shipped. |
| File-specific indentation after restarting | [D4121.32][releases], [V4117.3][dev] | done | `vel-session` sheet settings; `workbench.rs` restore | |
| Undo history across restart; refuse stale history after external change | [D4213][releases] | done | `vel-session/tests/native_history.rs`, `mismatch_version_mapping_and_corrupt_data_fail_cleanly`, `session_restore.rs` integrity checks | |
| `open_files_in_new_window` | [D4121][releases], [Settings][settings] | done | `open_requests::OpenWindow::setting`, `two_apps_route_to_existing_folder_project_and_file_without_replacing_dirty_tabs` | |
| `close_windows_when_empty` | [D4126][releases] | done | `App` close flow; [reader](settings.md) | |
| Native recents, associations, and document icons | [V4095.41][dev], [D4143][releases] | partial | `crates/velocity/packaging/test_bundle.py`; [native check](manual-checks.md#6-round-36-parity) | Static association metadata and recents routing have headless coverage. Finder icons, Dock file-open delivery and the native recent-documents list require LaunchServices/AppKit. |

## Windows, tabs and display

| Feature | Sublime source (URL + build) | Velocity status | Evidence | Notes |
| --- | --- | --- | --- | --- |
| Multiple native windows, move tab, merge windows | [Tab multi-select][tabs], [Commands][commands] | done | `move_tab_to_new_window`, `merge_all_windows`; menu model tests | Cocoa behavior not live-tested this round. |
| Tab/window/OS tab family on macOS | [D4107.106][releases], [V4050.46][dev], [D4142][releases] | partial | (c) Window menu, `workbench.rs`; [native check](manual-checks.md#6-round-35-windows-and-resources) | Editor tabs and window switching are separate. AppKit window tabbing is explicitly disabled; native tab traversal, merge and session behavior are not covered. |
| Split View shares the buffer; row/column/grid layouts | [Commands][commands], [D4107][releases] | done | `split_view`, `set_layout`; `vel-ui/workbench.rs` | |
| Focus or move a tab to a group | [Tab multi-select][tabs], [Commands][commands] | done | `focus_group`, `move_to_group`, `workbench.rs` | |
| Move all selected sheets via `move_to` or `move_to_neighbouring` | [D4126.39][releases], [Tab multi-select][tabs] | done | (a) `r35_move_selected_commands_keep_order_focus_and_caret_move` | The `group` and `direction` forms move selected sheets in order and keep focus. Caret `move_to` and single-sheet `move_to_group` are unchanged. Public release notes do not enumerate the window forms' arguments. |
| Jump to the last tab with Cmd+9 | [D4151.3][releases], [V4148.2][dev] | done | `select_by_index` with index -1 in Velocity's keymap | |
| Tab multi-select with keyboard, drag and mouse | [Tab multi-select][tabs], [D4107][releases], [V4050][dev], [V4097.6][dev] | done | (a) `r35_tab_chords_add_focus_and_deselect_without_closing`, `r35_tab_dropdown_command_click_toggles_membership`; drag dispatch in `app_mouse.rs` | Command-click toggles membership. Adding an already selected neighbor no longer removes it, and the left/right chords deselect without closing tabs. Native drag presentation is not attested. |
| Change focus without changing membership | [Tab multi-select][tabs], [V4098.2][dev] | done | (a) `r35_tab_chords_add_focus_and_deselect_without_closing` | Both documented Mac bracket chords move input focus among selected tabs without changing the set. |
| Remove just the focused tab from multi-selection | [Tab multi-select][tabs], [D4186.10][releases], [V4184.6][dev] | done | `unselect_current`, palette and `App::select_tab_action`; `r34_tab_insert_and_unselect_current` | Keeps the remaining tabs selected and the last selection intact. No new shortcut or setting. |
| Replace focused tab, preserve other selected tabs | [Tab multi-select][tabs] | done | (a) `r35_goto_modifiers_keep_expected_tabs_on_acceptance`; `Group::replace_focused` | Goto File and Goto Symbol in Project use the pre-preview selection for Shift append, Command append-and-clear-right, and Option replace-focus. Tab, dropdown and sidebar Option-click replace focus. This does not attest to the distinct Definitions popup modifiers. |
| Drag tabs to reorder/move and scroll the strip | [Tab multi-select][tabs], [D4213][releases] | done | `app_tabs.rs`; `enable_tab_scrolling` settings sweep | |
| New Tab button | [D4107.202][releases], [V4095.8][dev] | done | `app_tabs.rs` theme hit targets | |
| `hide_new_tab_button` | [D4107.202][releases], [V4095.8][dev], [Themes][themes] | done | `app_tabs.rs`; [reader](settings.md) | |
| `hide_tab_scrolling_buttons` | [Themes][themes] | done | `app_tabs.rs`; settings effect suite | |
| `show_tab_close_buttons_on_left` | [D4107.203][releases], [V4075.13][dev] | done | `tab_close_side_moves_target_and_painted_label_without_moving_the_tab` | |
| `file_tab_style` and dirty tab appearance | [D4107.198][releases], [V4095.1][dev], [Themes][themes] | done | `rounded_and_square_tab_styles_change_the_drawn_tab_not_the_hit_rect` | |
| `open_tabs_after_current` | [D4142.9][releases], [V4127.11][dev] | done | `Workspace::insert_opened_sheet`; `r34_tab_insert_and_unselect_current` | Defaults to append as decided in U05; no public factory value found. |
| `select_across_groups` | [D4126.17][releases], [V4122.4][dev] | done | `Workspace::open_from_sidebar`; `r34_sidebar_cross_group_and_right_geometry` | Opt into switching groups for a single sidebar file. Off keeps the active group, reusing the buffer through a shared view. |
| Open Files and folder sidebar, reveal file, refresh/collapse | [Projects][projects], [D4142][releases], [U01/U04 round 43](behavior.md#round-43-u01-and-u04-details) | done | `r43_open_files_defaults_off_and_restores_each_explicit_choice`, `r43_folder_highlight_follows_view_without_reveal_or_focus_change`, `reveal_in_side_bar`, `refresh_folder_list`, `close_folder_list` | Open Files defaults off and preserves saved per-window choices. Visible folder rows highlight the active or previewed file without revealing it. |
| Sidebar create, rename, trash, Reveal in Finder | [Projects][projects], [Commands][commands] | done | `context_menu.rs::sidebar_command`, `NativeFileOperations` tests | |
| Copy Path in sidebar context menu | [D4166.8][releases], [V4158.5][dev] | done | `copy_file_path`, `context_menu.rs` | |
| `preview_on_click` (left-only / disabled / enabled) | [D4107.169][releases], [V4075.10][dev] | done | `round18_sidebar_preview_setting_replacement_promotion_and_session_recent_files` | |
| `sidebar_on_right` | [D4200.1][releases], [V4198.1][dev] | done | `Panels::layout`; `r34_sidebar_cross_group_and_right_geometry` | Mirrors the sidebar and editor bounds without changing the default left placement. |
| Fresh window sidebar and untitled editor | [U01, Sublime Text 4215](behavior.md#round-43-u01-and-u04-details) | done | `r41_fresh_input_is_not_a_document_until_edited`, `r43_empty_titles_survive_goto_in_both_languages`, `r43_side_bar_menu_caption_tracks_visibility_in_both_languages` | Sidebar defaults to visible when the window has folders; its menu action changes caption without a check mark. The untouched, tabless window keeps its localized `untitled` title (plus folder name, if present) through a Goto preview. |
| `tree_animation_enabled` | [Themes][themes] | done | `panels.rs` geometry and Reduce Motion; [reader](settings.md) | |
| Full screen | [Distraction Free][distraction], [Commands][commands] | done | `toggle_full_screen`, `vel-platform/fullscreen.rs` | |
| `use_simple_full_screen` | [Distraction Free][distraction], [Settings][settings] | done | `toggle_full_screen_with_mode`; next native transition per [settings](settings.md) | |
| Distraction Free with its own layout/settings | [Distraction Free][distraction], [Settings][settings] | done | `toggle_distraction_free`; `numeric_ruler_and_distraction_zoom_use_the_real_settings_loader` | |
| `auto_hide_tabs` | [D4107.205][releases], [V4050.58][dev], [V4067][dev] | done | `App::tabs_visible`; `r34_typing_chrome_and_native_pointer_policy` | Editor typing hides tabs; pointer activity reveals them unless explicitly hidden. |
| `auto_hide_status_bar` | [D4107.205][releases], [V4050.58][dev] | done | `Panels::status_visible`; `r34_typing_chrome_and_native_pointer_policy` | Editor typing hides status paint and hit targets without changing the explicit Show Status Bar state. |
| `auto_hide_menu` / `reveal_menu` | [D4107.205][releases], [D4113.12][releases], [V4050.58][dev], [V4108.7][dev] | out of scope | Native Mac menu bar in `vel-platform/macos.rs` | Windows/Linux window-local menu behaviors. |
| `themed_title_bar` | [D4107.197][releases], [V4094.3][dev] | done | `app.rs`, `vel-platform/macos.rs` | |
| `show_rel_path` | [D4107.206][releases], [V4050.59][dev] | done | `workspace.rs` title; [reader](settings.md) | |
| `show_full_path` | [Settings][settings] | done | `workspace.rs` title; [reader](settings.md) | |
| `show_project_first` | [D4107.206][releases], [V4050.59][dev] | done | `Workspace::window_title`; `r34_project_title_and_unsaved_syntax_name` | Opt-in project-first title; preserves the file name, path preferences and dirty marker. |
| `set_unsaved_view_name_for_syntax` | [D4200.16][releases], [V4198.10][dev] | partial | (b) `Tab::name_with_untitled`; `r34_project_title_and_unsaved_syntax_name` | Boolean opt-out of first-line titles for non-plain syntax; the public notes name the key but do not describe its type, default, or whether it applies to specific syntaxes. |
| `click_through` (macOS inactive window pointer action) | [D4213.34][releases], [V4205.8][dev] | done | AppKit `acceptsFirstMouse:` on the editor view; `r34_typing_chrome_and_native_pointer_policy` tests the handler decision | Headless up to the native activation boundary. A file tab answers the activating click even with the setting off, so one click selects the sheet (bug report, 2026-11-25). |
| `hide_pointer_while_typing` | [D4107.118][releases], [V4074.7][dev] | done | Editor text input uses `NSCursor.setHiddenUntilMouseMoves`; `r34_typing_chrome_and_native_pointer_policy` tests the setting | D4107 lists the behavior under Windows/Linux. macOS uses the public AppKit method for the same opt-in behavior; no native event loop in tests. |
| `show_sidebar_button` | [D4107.134][releases], [V4074.3][dev] | done | `app_status.rs`, settings effect suite | |
| `show_indentation` | [D4107.134][releases], [V4074.3][dev] | done | `app_status.rs`, settings effect suite | |
| `show_syntax` | [D4107.134][releases], [V4074.3][dev] | done | `app_status.rs`, settings effect suite | |
| `show_line_column` | [D4142.10][releases], [V4138.2][dev] | done | `app_status.rs`; [reader](settings.md) | |
| `selection_description_column_type` | [V4075.11][dev] | done | `app_status::selection_column`; `r34_status_column_units_and_sum` | One-based tab-expanded scalar columns remain the default; characters, UTF-16 and bytes are available. Exact Sublime enum spellings are not public in the cited notes. |
| `show_encoding` | [Settings][settings] | done | `app_status.rs` settings/format tests | |
| `show_line_endings` | [Settings][settings] | done | `app_status.rs` settings/format tests | |
| Minimap navigation, drag and visible viewport | [Themes][themes], [D4107][releases] | done | `minimap_drag_reaches_both_ends_without_jumping_in_long_wrapped_and_folded_documents` | |
| `minimap_horizontal_scrolling` | [D4142.8][releases], [V4136.4][dev] | done | `view.rs`; [reader](settings.md) | |
| `minimap_scroll_to_clicked_text` | [D4107][releases] | done | `view.rs`; [reader](settings.md) | |
| `draw_minimap_border` | [D4166][releases], [Themes][themes] | done | `view.rs`; [reader](settings.md) | |
| `always_show_minimap_viewport` | [Themes][themes] | done | `vel-ui/theme.rs`; [reader](settings.md) | |
| `word_wrap` | [Settings][settings] | done | `large.rs`/view and settings effect tests | |
| `wrap_width` | [Settings][settings] | done | `view.rs`; [reader](settings.md) | |
| `wrap_width_style` | [D4107.132][releases], [V4092.6][dev] | done | `view.rs`; [reader](settings.md) | |
| Indent folding and unfold, fold by level | [Commands][commands] | done | `fold`, `unfold`, `fold_by_level`; `view.rs` | |
| Syntax-based folding, fold and unfold | [D4142.1][releases], [V4130.1][dev] | partial | (b) `vel-highlight` syntax/indent fold model; `view.rs` | Scope and indentation folding work. The notes do not specify which scope wins when syntax folds and indentation folds overlap, or how fold levels traverse embedded scopes. |
| Scope fold trailing-newline option | [V4134.4][dev], [V4145][dev] | partial | (b) `fold_include_trailing_newline` across fold commands and gutter; `r34_fold_style_and_trailing_newline` | Excludes the last newline by default, includes it when enabled. Public notes describe the behavior but not a preference key, default, or syntax metadata schema. |
| `fold_style` | [D4151.2][releases], [V4145.1][dev] | done | Indentation candidates in `fold_state.rs`, used by fold commands and gutter; `r34_fold_style_and_trailing_newline` | Can disable syntax-based folding in favor of indentation; auto preserves the existing source of candidates. |
| `fold_buttons` | [Themes][themes] | done | `view.rs` gutter; settings effect suite | |
| `relative_line_numbers` | [D4107.116][releases], [V4074.4][dev] | done | `view.rs`; [reader](settings.md) | |
| `line_numbers` | [Themes][themes] | done | `view.rs`; [reader](settings.md) | |
| `highlight_gutter` | [D4107.204][releases], [V4050.57][dev] | done | `View::scene` caret-row gutter fill; `r34_gutter_and_control_style_reach_scene` | Independent of text-line and line-number highlighting. |
| `highlight_line_number` | [D4107.204][releases], [V4050.57][dev] | done | `view.rs` active gutter color; [reader](settings.md) | |
| `rulers` | [Color schemes][schemes], [Settings][settings] | done | `numeric_ruler_and_distraction_zoom_use_the_real_settings_loader` | |
| `ruler_style` | [D4142.12][releases], [V4134.8][dev] | done | `view.rs`; settings effect sweep | |
| `ruler_width` | [D4166.18][releases], [V4164.4][dev] | done | `view.rs`; settings effect sweep | |
| `draw_white_space` leading/trailing/all/selection | [D4107.127][releases], [V4083.2][dev], [V4050.19][dev] | done | `view.rs` drawing and settings scene sweep | |
| `draw_unicode_white_space` incl. extra invisible glyphs | [D4107.128][releases], [D4126.15][releases], [V4123.10][dev], [V4050.20][dev] | done | `view.rs`; `app_lane_c_round20_tests.rs` Unicode scene coverage | Original glyph rendering; not a claim of identical artwork. |
| `draw_unicode_bidi` | [D4126.16][releases], [V4122.3][dev] | done | `view.rs`; settings scene sweep | |
| `control_character_style` | [D4107.119][releases], [V4094.4][dev] | partial | (b) C0/DEL style in `vel-render::invisibles`; `r34_gutter_and_control_style_reach_scene` | Hex, name and caret displays work. The public notes do not list accepted values, the default, or the set of affected control code points. |
| `caret_style` blink and style | [D4107.114][releases], [V4050.15][dev], [Font][font] | done | `view.rs`; [reader](settings.md) | |
| `scroll_context_lines` | [D4107.117][releases], [V4075.9][dev] | done | `view.rs`; settings effect suite | |
| `scroll_past_end` boolean or fraction | [D4107.123][releases], [V4095.15][dev] | done | `view.rs`; settings effect suite | |
| System scrollbar click behavior | [D4151.64][releases], [V4149.19][dev] | partial | (c) `view.rs` track-hit tests; [native check](manual-checks.md#6-round-35-windows-and-resources) | Model honors the page/jump preference and Option inversion. Live System Settings changes and AppKit overlay hit testing are untested. |
| Custom scrollbar mousemap | [V4103.2][dev] | done | (a) `r35_scrollbar_mousemap_is_scoped_to_visible_track` | Loads `Scroll Bar.sublime-mousemap` separately. Track bindings can replace the built-in action and keep their release target when the pointer leaves the track. |
| GPU rendering | [GPU Rendering][gpu], [D4107.11][releases], [V4050.1][dev] | done | `vel-render` Metal/wgpu surface and CPU scene path | GPU backend differs from ST's OpenGL. |
| `hardware_acceleration` fallback and GPU diagnostics | [GPU Rendering][gpu], [D4107.11][releases], [V4050.1][dev] | partial | (c) `vel-render` Metal renderer, headless modes in `cli.rs`; [native check](manual-checks.md#6-round-35-windows-and-resources) | CPU-only headless modes are not a software window renderer. Metal initialization failure, software fallback, and equivalent startup console diagnostics are not covered; no inert preference added. |
| `font_face` | [Font][font] | done | `settings_effect_tests` font probe; `vel-platform/fonts.rs` | |
| `font_size` and zoom | [Font][font] | done | `Workspace::zoom_font` and `increase_font_size` / `decrease_font_size` | |
| `default_font_size` | [D4200.14][releases], [V4194.6][dev] | done | `Workspace::zoom_font` reset path; `app_lane_b_round17_tests.rs` | |
| Select font from Preferences | [D4166.7][releases], [V4156.1][dev], [Font][font] | done | (a) `r35_font_picker_preselects_current_and_writes_on_acceptance` | Preferences > Font > Choose lists installed macOS font families in Velocity's quick panel. Selecting one updates `font_face`; cancel leaves it unchanged. Native enumeration remains a live check. |
| `font_options` | [Font][font], [Ligatures][ligatures] | partial | (b) `view.rs` typography; [reader](settings.md) | Common style flags and ligature controls work. Font docs do not enumerate macOS antialiasing/hinting options or establish how multiple flags combine on Retina. |
| Ligatures, opt-out and font fallback | [Ligatures][ligatures], [Font][font] | done | `vel-text` shaping and `vel-platform/fonts.rs`; `font_options` | |

## Language, completion and Vintage

| Feature | Sublime source (URL + build) | Velocity status | Evidence | Notes |
| --- | --- | --- | --- | --- |
| Buffer-word completions | [Completions][completion] | done | `vel-complete::BufferWords`; `completion.rs` popup/commit | |
| Context-aware project completions | [Completions][completion], [Indexing][indexing], [D4107][releases], [V4050][dev] | done | `vel-complete::CompletionEngine`, `intelligence.rs` | Statistical suggestions, not LSP. |
| Declarative completion files and scope matching | [Completions][completion], [V4073][dev] | done | `vel-complete` parser tests; `completion.rs` | Uses `.sublime-completions`. |
| Completion triggers containing tabs | [V4053.7][dev] | done | `r36_tab_delimits_annotation_not_inserted_text` | The [completion reference][completion-reference] specifies a tab-delimited annotation. Raw and rich trigger annotations display separately; committed text contains only the completion. |
| Kind, annotation, details and match highlights | [Completions][completion], [D4107][releases], [V4073][dev] | done | `popup_scene_uses_kind_colors_match_color_and_kind_footer` | Built-in HTML/CSS and JSON resources, not callbacks. |
| Completion fallback theming | [V4053.8][dev], [Themes][themes] | partial | `Completion::paint`, `vel-ui/theme.rs` | Completion draws using fallback styles if theme keys are missing. 4053 does not list the synthesized classes or precedence for mismatches between legacy and newer kind/annotation rules. |
| `auto_complete`, show, dismiss and commit | [Completions][completion] | done | `completion.rs::intelligence_command`, `poll_completion` | |
| `auto_complete_delay` | [Completions][completion] | done | `completion.rs` scheduling; [reader](settings.md) | |
| `auto_complete_size_limit` | [Completions][completion] | done | `completion::settings` to completion worker; [reader](settings.md) | |
| `auto_complete_commit_on_tab` | [Completions][completion] | done | `completion.rs`, keymap context and commit test | |
| `auto_complete_with_fields` | [Completions][completion] | done | `completion::settings`; [reader](settings.md) | |
| `auto_complete_cycle` | [Completions][completion] | done | `completion.rs` popup movement tests | |
| `auto_complete_when_likely` | [D4107.93][releases], [V4099.1][dev], [Completions][completion] | done | `completion.rs`, `vel-complete`; [reader](settings.md) | |
| `auto_complete_preserve_order` | [D4107.94][releases], [V4052.1][dev], [Completions][completion] | done | `preserve_order_documented_values_and_history_switch` | |
| `auto_complete_include_snippets_when_typing` | [D4107.95][releases], [V4052.2][dev], [Completions][completion] | done | `automatic_snippet_inclusion_and_default_selector_table` | |
| `auto_complete_use_index` | [D4107.96][releases], [V4052.3][dev], [Completions][completion] | done | `completion.rs::settings` to worker; [reader](settings.md) | |
| `auto_complete_use_history` | [D4107.97][releases], [V4074.2][dev], [V4059.2][dev] | done | `preserve_order_documented_values_and_history_switch` | |
| `auto_complete_include_snippets` | [D4107.156][releases], [V4050.32][dev] | done | `completion.rs::settings`, `vel-complete` | |
| `ignored_snippets` | [D4107.157][releases], [V4050.33][dev] | done | [reader](settings.md); `completion.rs`/package loading | |
| `auto_complete_trailing_symbols` | [D4107][releases], [Completions][completion] | done | `completion_suffix_settings_reach_the_commit_path` | |
| `auto_complete_trailing_spaces` | [Completions][completion] | done | `inferred_spaces_need_repeated_evidence_and_a_safe_right_side` | |
| `auto_complete_selector` | [Completions][completion], [V4072][dev] | done | `explicit_trigger_selector_and_rhs_table` | Just-typed position. |
| `auto_complete_triggers` including `rhs_empty` | [Completions][completion] | done | `explicit_trigger_selector_and_rhs_table` | |
| `tab_completion` and exact snippet priority | [Completions][completion] | done | `tab_best_match_decision_table_and_snippet_field_selection_indent_exceptions` | |
| `disable_default_completions` | [Completions][completion] | done | `completion.rs`; [reader](settings.md) | |
| `default_completions_selector` | [Completions][completion] | done | `completion.rs` markup eligibility; [reader](settings.md) | |
| Snippet files: scope, palette, tab trigger and duplicate picker | [Completions][completion], [Snippets][snippets] | done | `suffix_trigger_longest_and_duplicate_choice_through_key_dispatch`, `scoped_snippets_are_filtered_in_the_real_palette` | |
| Snippet fields, mirrors and regex transforms | [Snippets][snippets] | done | `variables_nested_defaults_regex_unicode_and_literal_dollars` | |
| Snippet environment variables and custom shell variables | [Snippets][snippets], [Completions][completion] | done | `documented_snippet_variable_table_at_a_unicode_column`, `snippet_metadata_includes_go_rails_and_user_shell_variables` | |
| Language matching by extension, filename and first line | [Syntax][syntax] | done | `vel-highlight::detect_syntax`, `vel-syntax/tests/roots.rs` | |
| `syntax_detection_size_limit` | [D4113.7][releases], [V4110.2][dev] | done | `Workspace::configure_language`; `r35_io_workers_and_automatic_syntax_size_setting_reach_file_open` | A positive byte limit skips automatic detection above the threshold. Explicit syntax assignments still work; 0 preserves detection. |
| `set_file_type` and `scope:` names | [D4107][releases], [Commands][commands] | done | `Workspace::set_file_type`, `detect_syntax` | |
| Show Scope Name details and origin | [D4107.49][releases], [V4087.4][dev], [D4151][releases] | partial | `round21_scope_popup_and_copy_link`, `html_ui.rs` | Scope stack and copying work. 4142 documents clickable context origins; Velocity still lacks per-frame origin links, and legacy/anonymous context presentation is not specified. |
| .sublime-syntax contexts, includes, prototypes and variables | [Syntax][syntax], [D4107][releases] | done | `vel-syntax/tests/semantics.rs`, `vel-highlight/tests/scopes.rs` | |
| Syntax `branch` / `fail` backtracking | [D4107.34][releases], [D4107.38][releases], [V4050.72][dev], [Syntax][syntax] | done | `vel-syntax/tests/semantics.rs`, `vel-highlight/tests/incremental.rs` | |
| Syntax `version: 2` and v1 compatibility | [D4107.39][releases], [Syntax][syntax], [V4075][dev] | done | `vel-syntax/tests/semantics.rs` | |
| Syntax `extends` and multiple inheritance | [D4107.40][releases], [Syntax][syntax], [V4075][dev] | done | `vel-syntax/tests/semantics.rs` | |
| Syntax `hidden_extensions` | [D4107.41][releases], [Syntax][syntax], [V4075][dev] | done | `vel-syntax` header and syntax lookup | |
| Syntax `embed` / `escape`, lazy embed loading | [D4107][releases], [V4075][dev], [Syntax][syntax] | partial | `external_embed_is_resolved_only_when_entered`; `dynamic_escape_is_escaped_and_clones_converge` | Embed and escape actions work and external context resolution waits for entry. The notes do not specify parse-cache lifetime, eviction, or what is retained after leaving an embed. |
| Syntax `pop` plus push/set/embed/branch; `clear_scopes` | [D4107][releases], [V4075][dev], [Syntax][syntax] | done | `vel-syntax/tests/semantics.rs` | |
| .tmLanguage compatibility | [Syntax][syntax], [D4142][releases] | done | `vel-syntax/tests/tm_language.rs` | |
| .tmPreferences: comments, indent, folding, symbols | [Syntax][syntax], [Scope naming][scopes] | partial | `cdata_and_independent_setting_precedence`; `symbol_only_metadata_matches_the_full_preference_merge` | Core comment, indent and symbol settings resolve by scope. Edge precedence for conflicting folding selectors and numbered comment sets across nested scopes is not specified. |
| Built-in TypeScript, TSX, JSX | [D4107.33][releases], [V4075.1][dev], [V4087.5][dev] | done | `language.rs`; corpus registrations in `vel-syntax/tests/roots.rs` | Exact upstream revisions not audited. |
| Built-in Zsh | [D4200.5][releases], [V4194.4][dev] | partial | `official_assertions_never_regress`; `language.rs` syntax lookup | Zsh is detected and its corpus passes. 4200 announces a built-in grammar but does not specify changes to patterns, scopes or shell embedding relative to earlier grammar revisions. |
| Built-in TOML | [D4200.5][releases], [V4198.4][dev] | partial | `official_assertions_never_regress`; `language.rs` syntax lookup | TOML works with the checked-in corpus. 4200 does not specify its exact scopes or behavior for incomplete dotted keys and malformed multiline strings. |
| Later grammar updates incl. Haskell and C# rewrites | [D4142][releases], [D4213][releases], [V4130][dev] | partial | `official_assertions_never_regress`; `vel-syntax/tests/roots.rs` | Both have working highlighting. Release notes announce rewrites but do not publish a scope-by-scope change list or malformed-input recovery expectations for those revisions. |
| Syntax test files, scope and symbol expectations | [Syntax][syntax], [V4087.3][dev] | partial | `official_assertions_never_regress`; `r35_syntax_tests_menu_palette_output_legacy_and_result_links` | The app runs saved syntax tests and links diagnostics. Scope and symbol assertions work; symbol coverage flags and exact diagnostics for unsupported directives are not fully specified by the public syntax guide. |
| Syntax test reindent and symbol transformation assertions | [D4107.44][releases], [D4186.17][releases], [V4069.8][dev], [V4184.11][dev] | partial | `reindent_variants_execute_and_detect_a_broken_rule`; `symbol_runner_checks_names_exhaustiveness_and_partial_files` | The runner checks reindent and transformed names. Changelogs name the assertions but do not define all modifier combinations or the expected failure format for mixed local/indexed transformations. |
| `run_syntax_tests` incl. `syntax` and non-UTF-8 diagnostics | [D4180.33][releases], [V4170.17][dev], [V4170.18][dev] | done | `Builds::start_syntax_tests`; `r35_syntax_tests_menu_palette_output_legacy_and_result_links` | Menu/palette run one saved test file using the existing scope, symbol and indentation runner. Optional syntax override; decodes legacy input with diagnostics and links failures by file/line. |
| Scope naming and scope-based consumers | [Scope naming][scopes] | done | Highlighting and metadata in `vel-syntax`, `vel-scope`, schemes | Naming is a convention; depends on the grammar. |
| Selector prefix matching, OR/AND/NOT, grouping | [Selectors][selectors] | done | `vel-scope/src/lib.rs::Selector` parser and tests | |
| Immediate-child selector `>` | [Selectors][selectors], [D4213][releases] | done | `vel-scope::child_regression` | |
| Spell Check, suggestions, ignore/add words | [Spell Checking][spell] | done | `vel-spell/tests/spelling.rs`; `review_ui.rs` | |
| `dictionary` and non-UTF-8 .aff/.dic pairs | [Spell Checking][spell], [D4107.228][releases], [V4070.2][dev] | done | `r36_legacy_dictionary_set_decodes_words_and_affixes` | Package and system pairs honor supported `SET` codecs; decoding rejects malformed bytes instead of silently substituting. No claim about every Hunspell charset label. |
| `spelling_selector` | [Spell Checking][spell] | done | `vel-spell` scope filter and settings test | |
| `spell_check` | [Spell Checking][spell] | done | `review_ui.rs`, `annotations.rs` | |
| `added_words` | [Spell Checking][spell] | done | `vel-spell/tests/spelling.rs`; [reader](settings.md) | |
| `ignored_words` | [Spell Checking][spell] | done | `vel-spell/tests/spelling.rs`; [reader](settings.md) | |
| `show_spelling_errors` | [D4142.10][releases], [V4138.2][dev] | done | `annotations.rs`/review UI; settings tests | |
| Next/previous misspelling | [Spell Checking][spell] | done | `next_misspelling` / `prev_misspelling` review UI | |
| Vintage enable, modes, operators, registers and motions | [Vintage][vintage] | done | `vel-vintage/tests/commands.rs`, `vintage.rs` | Core Vi subset, not Vim compatibility. |
| `vintage_start_in_command_mode` | [Vintage][vintage] | done | `vintage.rs`; [reader](settings.md) | |
| `vintage_use_clipboard` | [Vintage][vintage] | done | `vintage.rs`; [reader](settings.md) | |
| `vintage_ctrl_keys` and macOS bindings | [Vintage][vintage] | done | `vintage.rs`, Velocity Vintage keymap | |
| Vintage Ex and integration points | [Vintage][vintage] | partial | `indent_contexts_ex_requests_and_clipboard` | `:w`, `:wq`, `:q`, `:e` and clipboard dispatch are tested. The Vintage guide delegates full Ex support to VintageEx and does not define its addressing, ranges, abbreviations or plugin integration. |

## Customization and resources

| Feature | Sublime source (URL + build) | Velocity status | Evidence | Notes |
| --- | --- | --- | --- | --- |
| Default, platform and User settings | [Settings][settings] | done | `Configuration::load`, `vel-config/settings.rs` | Velocity defaults are original. |
| Per-syntax, project and per-view settings | [Settings][settings] | done | `settings_project_view_and_user_keymap_precedence_reload` | |
| Changing a setting with a key binding | [Settings][settings], [Key bindings][keys] | done | `set_setting` / `toggle_setting`; `vel-commands` | |
| JSONC settings, preservation of comments/layout and newline convention | [D4121.28][releases], [V4114.6][dev], [V4069][dev], [V4074][dev], [Settings][settings] | done | `vel-config/json.rs`, `vel-config/tests/config.rs` | |
| Settings troubleshooting, malformed value diagnostics | [Settings][settings] | done | `validate_layer`, `settings_entry.rs`; menu/settings test suite | |
| Default/User/syntax-specific Preferences editors | [Settings][settings] | done | `settings_entry.rs`, `settings_user_tab_save_updates_existing_window_and_preserves_explicit_toggles` | |
| Single-chord and sequence key bindings | [Key bindings][keys] | done | `vel-config/keymap.rs`, `vel-config/tests/config.rs` | |
| Modifier-only/double modifier key sequences | [Key bindings][keys], [D4107][releases] | done | `vel-config/keymap.rs`; `vel-platform/input.rs` | |
| Mac keypad keys in bindings | [D4107.105][releases], [V4050.45][dev], [Key bindings][keys] | partial | (c) `editor::binding_key`; [native check](manual-checks.md#6-round-35-windows-and-resources) | Digits, Enter and arithmetic keys have separate identities. Physical keypad events, decimal separator layouts, and labels have not been checked in AppKit. |
| `noop` key binding blocks fallback | [D4142.70][releases], [V4134.6][dev] | done | `vel-commands/text_ops.rs` registry, `Configuration::load` | |
| Key contexts: selector, setting, regex, selection/panel state | [Key bindings][keys] | done | `vel-config/keymap.rs`; scoped dispatch tests | Python `on_query_context` excluded. |
| Mousemaps and mouse context expressions | [D4151.47][releases], [V4149.15][dev], [Key bindings][keys] | done | `vel-config/keymap.rs`, `app_mouse.rs` | Native declarative feature, not Python. |
| Mouse Bindings Preferences row | [D4166.6][releases], [V4153.3][dev] | done | `edit_settings` Mouse Bindings resource in Main menu | |
| Mouse Bindings palette entry | [D4180.32][releases], [V4178.1][dev] | done | `Velocity.sublime-commands`, `edit_settings` | |
| .sublime-menu IDs, submenu merge, availability and checkmarks | [Menus][menus] | done | `vel-config/ui.rs::resolve_menu`, `vel-config/tests/context_menus.rs` | Main/edit/tab/sidebar/mount/Where/widget implemented. |
| Native shortcut labels match active keymap | [Menus][menus], [Key bindings][keys] | partial | (c) `menu_roles_and_exact_shortcuts_survive_renaming_and_reload`; [native check](manual-checks.md#6-round-35-windows-and-resources) | Menu model resolves the active binding. German layout, 2026-11-22: AppKit relocates ⌘/ to ⌘ß. Sublime 4215 (black box) toggles the comment on ⌘ß and ignores ⌘⇧7; Velocity now runs a menu command when AppKit matched its relocated equivalent (`relocated_menu_equivalents_are_told_apart_from_declined_chords`). Native recheck of the fix pending. |
| macOS Pinyin composition at multiple carets | Sublime 4215 black box, 2026-11-22 | done | `multi_caret_preedit_has_one_primary_native_range_and_one_commit`, `multi_caret_composition_scene_native_queries_and_mouse_discard`, `marked_field_underline_matches_text_ink_and_backing_scale` | Marked text appears at each selection and AppKit reports the primary composition. The preview is not a buffer edit; commit is one undo step. Fields use a one-point foreground underline. [Native recheck pending](manual-checks.md#3-chinese-input-method--4-min). |
| Additional native menu resources (build, console, panel and status-specific variants) | [Menus][menus] | partial | (b) `r35_widget_settings_select_named_context_menu_only_for_widget`, `ContextMenuKind::resource_name` | All seven documented menu filenames load, including the widget `context_menu` filename override. The reference does not identify distinct resource names, args or merge precedence for additional build, console and status variants. |
| Loose and ZIP .sublime-package resources | [Packages][packages] | done | `vel-config/resources.rs`, `vel-packages/tests/packages.rs` | Declarative resources only. |
| Package priority, User overrides, resource reload | [Packages][packages] | done | `Workspace::poll_resource_reload`, `context_menu_resources_layer_and_hot_reload_with_the_workspace` | |
| `ignored_packages` and priority rules | [Packages][packages], [V4123][dev] | done | `Configuration::load`, settings effect suite | |
| Install/update/remove declarative packages | [Packages][packages] | partial | (b) `release.rs`, `verified_https_install_upgrade_digest_failures_and_redirect_policy` | Velocity installs, upgrades and removes declarative archives. The package docs confirm archive/override behavior, not account, catalog, or upgrade scheduling contracts. Python Package Control is out of scope. |
| Platform filter in .sublime-commands | [D4107.242][releases], [V4094.9][dev] | done | `palette_menu_deduplication_platform_filter_and_binding_hints`; `vel-config/ui.rs` | |
| .sublime-color-scheme globals, CSS color formats/functions, variables | [Color schemes][schemes] | done | `css_and_recursive_color_mod`, `overrides_rebind_base_variables_and_merge_properties` | |
| Legacy .tmTheme | [Color schemes][schemes] | done | `legacy_plist_and_auto_pair` | |
| Customize/convert hidden .tmTheme resources | [D4107.214][releases], [V4095.25][dev], [V4096.5][dev] | partial | (b) `settings_entry.rs`; `real_resource_loader_isolates_bad_syntaxes_schemes_and_archives` | Hidden legacy schemes load when explicitly selected. Notes name converter support but do not give the command arguments or customization output filename, format and visibility rules. |
| Scheme selectors and hashed highlighting | [Color schemes][schemes], [V4093.2][dev] | done | `gradients_adjustments_and_tie_breaking`; highlighting/export integration | |
| Scheme style `glow` | [D4107.209][releases], [V4050.62][dev], [Color schemes][schemes] | done | `real_scheme_loader_drives_bracket_glyphs_glow_and_connected_selection_corners` | |
| Scheme style `underline` | [D4107.210][releases], [V4074.5][dev], [Color schemes][schemes] | done | `vel-color-scheme` font style parser; `view.rs` | |
| Scheme style `stippled_underline` | [D4107.213][releases], [V4075.16][dev], [Color schemes][schemes] | done | `vel-color-scheme` font style, `vel-render` decorations | |
| Scheme style `squiggly_underline` | [D4107.213][releases], [V4075.16][dev], [Color schemes][schemes] | done | `vel-color-scheme` font style, `vel-render` decorations | |
| Scheme `inactive_selection_border` | [D4107.211][releases], [V4074.6][dev], [Color schemes][schemes] | done | `view.rs`, `vel-color-scheme/tests/round17.rs` | |
| Scheme bracket, tag, gutter, diff, rulers and guide assets/colors | [Color schemes][schemes], [D4107][releases] | partial | (b) `vel-color-scheme/tests/schemes.rs`, `view.rs`, `review_ui.rs` | The main surfaces read these properties. The reference does not specify paint precedence where bracket/tag highlights, diff marks, selections and guides overlap. |
| Selection-corner style | [V4052.6][dev], [Color schemes][schemes] | done | `borderless_round_selection_reuses_contours_and_square_uses_rectangles` | |
| .sublime-theme class/attributes/parents/platforms matching | [Themes][themes] | done | `documentation_matching_corpus`, `ordered_distinct_parents_and_negation` | Matching logic distinct from widget coverage. |
| Theme settings predicates including typed object values | [D4107.218][releases], [V4095.27][dev], [Themes][themes] | done | `cache_settings_platform_state_and_replacement`, `vel-theme/tests/compatibility.rs` | |
| Theme variables, inheritance and customization commands | [Themes][themes], [D4107.130][releases], [V4095.7][dev] | done | `inheritance_user_rebinding_and_cycles`, `settings_entry.rs` theme/scheme overrides | |
| Four layered fills/textures, repeat and nine-slice | [Themes][themes], [V4060.3][dev] | done | `vel-theme/textures.rs`, `png_densities_rgba_and_nine_slices` | |
| Theme transitions, animation and virtual controls | [Themes][themes], [D4107][releases] | partial | (c) `legacy_css_colors_units_and_transitions`; [native check](manual-checks.md#6-round-35-windows-and-resources) | Headless interpolation and matching are tested; live timer cadence, virtual control state changes, and repaint during window activation have not been checked. |
| OS appearance and separate light/dark scheme and theme | [D4107.54][releases], [V4095.3][dev], [Themes][themes], [Color schemes][schemes] | done | `installed_user_theme_override_reload_appearance_and_corrupt_fallback` | |
| A bundled dark theme | [D4107.196][releases], [V4095.2][dev] | done | Velocity original theme assets and appearance test | |
| Theme `title_bar.style` | [D4107.216][releases], [Themes][themes] | done | `vel-theme/registry.rs`, bridge in `app.rs`/platform | Native style use not live-tested. |
| Theme `sheet_contents` on text/image/HTML | [D4107.219][releases], [V4095.28][dev], [Themes][themes] | done | `app.rs` sheet composition, `vel-theme/registry.rs` | |
| Theme `sheet_contents.background_modifier` | [D4107.220][releases], [V4095.29][dev], [Themes][themes] | done | `vel-theme/properties.rs`, sheet composition | |
| Theme `tab_control` focus/selection/state attributes | [D4107.221][releases], [V4095.30][dev], [Themes][themes] | done | `vel-theme/registry.rs`, `app_tabs.rs` scene | |
| File icon themes | [D4213.5][releases], [V4205.9][dev], [V4206][dev], [Themes][themes] | partial | (b) `file_icons_precedence_and_compound_extensions`, `theme_ui.rs` | Global then active-theme mapping works. The 4213 reference does not settle duplicate PNG basenames across packages, path vs. compound-extension collisions, or case sensitivity. |
| minihtml sheets, popups, phantoms and annotations | [minihtml][minihtml], [D4107.239][releases], [V4065.1][dev] | done | `html_sheet_survives_hot_exit_and_remains_read_only`, `phantoms_reserve_flow_and_track_edits_undo_while_annotations_do_not_add_rows` | Built-in surfaces; Python constructors excluded. |
| minihtml tags, links and image protocols | [minihtml][minihtml], [V4075.42][dev], [V4073][dev] | partial | (b) `links_images_tooltips_and_command_data`, `inline_subset_entities_and_bounded_parser` | Core tags, command/browser links and local/data resolution work. URL canonicalization for escaped file/resource paths and animated GIF playback are not specified by the reference. |
| Remote minihtml images | [D4186.20][releases], [V4184.12][dev], [minihtml][minihtml] | done | (a) `r35_loopback_https_image_decodes_paints_and_reuses_cached_pixels`, `r35_remote_minihtml_placeholder_then_pixels_on_all_three_surfaces`, `r35_anonymous_https_bounds_bytes_redirects_and_credentials` | Verified HTTPS in interactive popups, phantoms and sheets; byte-bounded LRU and placeholders. Public notes announce internet images without a setting, permission gate or limit details. [Velocity policy](../specs/vel-theme-minihtml.md#remote-images). |
| minihtml CSS variables, units, colors, and predefined classes | [minihtml][minihtml] | partial | (b) `css_invalid_values_keep_prior_declarations_and_scheme_css_overrides`, `dark_light_predefined_variables_fallback_and_cycles` | Basic cascade and palette classes work. Invalid nested color modifiers, custom-property cycles and unit rounding at fractional scales are not established. |
| minihtml `display: inline-block` and `text-align` | [V4085.6][dev], [minihtml][minihtml] | done | `vel-minihtml/tests/layout.rs`, inline-block golden | |
| minihtml `white-space: pre` and `pre-wrap` | [D4180.50][releases], [minihtml][minihtml] | done | `preserved_wrapping_nonbreaking_space_and_unicode_remain_intact` | |
| minihtml `white-space: nowrap` | [D4107.245][releases], [minihtml][minihtml] | partial | (b) `lists_whitespace_headings_and_hidden_content` | Basic no-soft-wrap overflow works. Exact break behavior at mixed nowrap/normal inline runs and nested inline-block boundaries is unverified. |
| minihtml `title` attribute tooltips | [V4085.7][dev], [minihtml][minihtml] | done | (a) `r35_html_sheet_title_hover_is_painted_and_dismissed`, `links_images_tooltips_and_command_data` | HTML sheets paint tooltips from clipped hover regions like popups and phantoms. Empty titles suppress inherited text. No claim of identical hover delay or pixels. |

## Build, Git and command line

| Feature | Sublime source (URL + build) | Velocity status | Evidence | Notes |
| --- | --- | --- | --- | --- |
| Build system selection, auto selector and variants | [Build systems][build] | done | `variant_environment_automatic_selection_and_custom_target_rejection`, `round18_selector_scoring_table_including_embedded_and_excluded_scopes` | |
| `cmd` (argv) and `shell_cmd` | [Build systems][build] | done | `vel-build/tests/build.rs`; `velocity/build.rs` | |
| `working_dir`, `path`, `env`, platform overrides and variable expansion | [Build systems][build] | done | `round18_environment_path_working_directory_and_wrap_table`, `round18_variant_overrides_effective_platform_base_and_can_clear_shell_cmd` | |
| `file_regex`, `line_regex` and result base directory | [Build systems][build] | done | `relative_file_regex_and_line_only_results_use_latest_header` | |
| Build output, cancel/restart processes and navigate errors | [Build systems][build] | done | `round18_restart_kills_child_and_grandchild_before_new_process_starts`, `round19_sustained_output_keeps_ui_polling_progress_with_regex` | |
| Inline build diagnostics and show/hide errors | [Build systems][build], [D4107][releases], [V4050.101][dev] | done | `build_errors_use_html_annotations_without_phantom_rows_and_escape_markup`, settings effect suite | Python annotation API excluded independently. |
| `save_all_on_build` | [Build systems][build], [V4067][dev] | done | `settings_effect_tests` runs real build/save operations | |
| `show_panel_on_build` | [Build systems][build] | done | `settings_effect_tests` build probe; output changes even with hidden panel | |
| Build output `word_wrap` | [Build systems][build] | done | `round18_environment_path_working_directory_and_wrap_table` | |
| Build `file_patterns` and `keyfiles` eligibility | [Build systems][build] | done | `vel-build` eligibility matcher, `velocity/build.rs` | |
| Build `quiet`, `encoding` and output `syntax` | [Build systems][build] | done | `Exec` options and result panel setup in `build.rs` | |
| Build `interactive` stdin panel | [D4200.2][releases], [V4198.2][dev], [Build systems][build] | done | `vel-build/process.rs`, `r33_build_panel_sends_echoes_and_closes_stdin` and cancel/restart/backpressure tests | Line input on Unix; Return and Control-D, scoped echo, bounded nonblocking pipe. Not a terminal emulator. |
| Alternate/custom build targets | [Build systems][build] | partial | `variant_environment_automatic_selection_and_custom_target_rejection` | Built-in `exec` and variants work. Public build docs describe Python commands for custom `target`/`cancel`; they specify no portable non-Python execution contract. Python is excluded. |
| Build child camera/microphone access on macOS | [D4151.66][releases], [V4145.18][dev] | partial | `vel-build` worker; [native check](manual-checks.md#6-round-36-parity) | External builds launch; TCC permission prompts, app attribution and denial/retry behavior need a live macOS check. |
| Git sidebar status, ignored dimming and ancestor badges | [Git integration][git] | done | `badges_both_layers_blobs_folders_and_literal_incremental_refresh`; `panels.rs` | |
| `show_git_status` | [Git integration][git] | done | `workspace.rs`; [reader](settings.md) | |
| Git branch/detached status and ahead/behind | [Git integration][git] | done | `unmerged_index_unborn_detached_worktrees_and_cheap_ahead_behind`, `app_status.rs` | |
| Git reftables | [D4213.13][releases], [V4207.3][dev] | done | `vel-git::reftable`; `git_generated_reftables_cover_head_branch_status_and_worktrees`; [Git format](https://git-scm.com/docs/reftable) | Read-only refs for HEAD, branch/status and HEAD blobs, SHA-1 and SHA-256, including linked worktrees. No writes, ref logs, or nested submodule worktree status. Pinned Gitoxide does not yet read reftable refs. |
| Diff markers against disk or Git HEAD | [Incremental diff][diff], [Git integration][git] | done | `vel-diff/tests/incremental.rs`, `review_ui.rs`, `git_diff_target` | |
| Next/previous modification | [Incremental diff][diff] | done | `next_modification`, `prev_modification` review actions | |
| Show/hide or toggle inline diff | [Incremental diff][diff] | done | `toggle_inline_diff`, `review_ui.rs` | |
| Revert Diff Hunk | [D4107.150][releases], [V4053.1][dev], [Incremental diff][diff] | done | `revert_hunk`; `vel-diff/tests/incremental.rs` | |
| `mini_diff` | [Incremental diff][diff] | done | `review_ui.rs`; [reader](settings.md) | |
| `git_diff_target` | [Git integration][git], [Incremental diff][diff] | done | `review_ui.rs`; [reader](settings.md) | |
| Git gutter and inline diff style controls | [Incremental diff][diff], [Color schemes][schemes] | partial | `review_ui.rs`, `vel-color-scheme`; [native check](manual-checks.md#6-round-36-parity) | Markers and inline palettes are wired. Check add/change/delete colors, contrast and deleted-hunk rendering with overridden values in light and dark schemes. |
| Open repository in Sublime Merge | [Git integration][git] | partial | `git_open_repository`; [native check](manual-checks.md#6-round-36-parity) | Configured executable receives the repository root. The public integration guide does not specify discovery or its handshake; check actual repository activation. |
| File history in Sublime Merge | [Git integration][git] | partial | `git_file_history`; [native check](manual-checks.md#6-round-36-parity) | Configured executable receives the file path. Check the history view for a file with spaces and multiple commits. |
| Folder history in Sublime Merge | [Git integration][git] | partial | `git_folder_history`; [native check](manual-checks.md#6-round-36-parity) | Configured executable receives the folder path. Check that the view includes commits for nested descendants as described in 4180. |
| Line history in Sublime Merge | [Git integration][git] | partial | `git_line_history`; [native check](manual-checks.md#6-round-36-parity) | Passes the file and one-based line; verify that Merge shows the selected line's history rather than just its blame view. |
| Blame in Sublime Merge | [Git integration][git] | partial | `git_blame_file`; [native check](manual-checks.md#6-round-36-parity) | Passes the file to the configured adapter. Confirm the annotated document is the requested file, including names with spaces. |
| `sublime_merge_path` | [Git integration][git] | done | `review_ui.rs` command and palette gating, settings effect suite | Empty hides handoff actions. |
| Install CLI launcher and configure PATH / EDITOR | [CLI][cli] | done | `release.rs` install action, `cli.rs` usage | Tool name is Velocity; no `subl` alias installed. |
| Open files/folders and handle literal `--` | [CLI][cli] | done | `cli.rs`, `open_requests::open_locations` | |
| `file:line[:column]`, and literal colons in filenames | [CLI][cli], [D4180.35][releases], [V4177.2][dev] | done | `real_cli_wait_interrupt_is_client_only_and_literal_colon_wins` | |
| Single instance and continued CLI forwarding | [CLI][cli] | done | `instance.rs`, `real_cli_forwards_to_legacy_wire_without_claiming_the_profile` | |
| `--project` | [CLI][cli] | done | `Request::new`, `App::accept_request`; window-routing tests | |
| `--command` with JSON arguments after open | [CLI][cli], [D4200.9][releases], [V4198.6][dev] | done | `parse_window_flags_command_arguments_and_safe_mode`, `poll_cli_commands` | Waits for file loads, not for Python. |
| `-n` / `--new-window` | [CLI][cli], [D4126][releases] | done | `open_requests::route`, window-routing fixtures | Reuses untouched startup window per documented release behavior. |
| `--launch-or-new-window` | [CLI][cli] (example 4131) | done | `cli.rs`, `Request::new`, `open_requests.rs` launch tests | |
| `-a` / `--add` | [CLI][cli] | done | `Request::new`, `RouteRequest` and app routing | |
| `-w` / `--wait` and interrupt handling | [CLI][cli] | done | `real_cli_wait_interrupt_is_client_only_and_literal_colon_wins` | |
| `-b` / `--background` | [CLI][cli] | done | `App::apply_open_request`, `real_nested_cli_with_leaked_gui_markers_forwards_and_reports_wait_eof` | |
| `-s` / `--stay` | [Mac CLI][oldcli], [CLI][cli] | done | `cli.rs`, `return_pid` policy in `open_requests.rs` | |
| Edit standard input (`-`) | [CLI][cli] | done | `Workspace::open_stdin`, `instance::Stdin` | |
| Return edited stdin bytes to stdout | [CLI][cli] | done | `instance.rs`, wait/stdin fixtures in `round21_cli.rs` | |
| `--help` and `--version` | [CLI][cli] | done | `real_help_version_and_benchmark_help_do_not_start_gui` | |
| `--safe-mode` isolated clean settings/session | [Safe Mode][safe], [D4107.72][releases], [V4050.68][dev], [CLI][cli] | done | `safe_mode::fresh_run`, `safe_mode_prunes_empty_dead_runs_without_touching_sessions_or_symlinks` | Native Option-start is a separate path below. |
| Hold Option at launch for Safe Mode | [Safe Mode][safe], [D4107.72][releases] | done | `Platform::option_held_at_launch`, `main.rs` | No native launch in this audit. |
| Console and `console_max_history_lines` | [D4107.133][releases], [V4088.4][dev], [Commands][commands] | done | `r36_console_limit_evicts_oldest_and_zero_disables_history`; [reader](settings.md) | Diagnostic console retains at most the configured line count. Velocity retains its previous 80-line default; zero disables history. Python REPL is out of scope. |
| `open_terminal` | [D4213.6][releases], [V4205.3][dev] | partial | `r34_terminal_uses_context_dir_and_literal_argv`; `app_terminal.rs` | Terminal.app and configured executables receive the context directory to the launch boundary. 4213 does not document the custom emulator setting/argument schema or quoting rules. |
| Help > Report a Bug | [D4107.73][releases] | done | `r36_block_command_and_report_a_bug_menu` | Help opens Velocity's public issue tracker through the existing URL action. |
| Update dialog theme attributes | [D4142.48][releases], [V4130.11][dev], [Themes][themes] | partial | `release.rs`; [native check](manual-checks.md#6-round-36-parity) | Update UI uses themed controls; verify available, current, and failed states under a legacy-style theme. The changelog does not enumerate the added attribute selectors. |
| Revert to fresh installation and clear cache guidance | [Revert][revert], [Safe Mode][safe] | partial | [Velocity troubleshooting](settings.md#troubleshooting-a-profile); `safe_mode.rs` | Safe Mode and a fresh data directory isolate settings, sessions and profile caches. Sublime's reset steps name its own data/cache paths; there is no shared destructive reset or documented cache directory contract. |

## Scope exclusions

| Feature | Sublime source (URL + build) | Velocity status | Evidence | Notes |
| --- | --- | --- | --- | --- |
| Python host, APIs and events, plugin console/porting | [API reference][api], [API environments][pyenv], [Porting][port], release ledger below | out of scope | 1.0 decision; `velocity/build.rs` inventories but never executes `.py` resources | Includes Python 3.8 beta, 3.14 upgrade, legacy 3.3 disable setting, plug-in callbacks, EventListener, Python REPL and Python Package Control. Declarative effects scored separately. |
| Windows/Linux editor, bindings, rendering, installers and repositories | [OS compatibility][os], [Linux repositories][linux], release ledger below | out of scope | `vel-platform` macOS native implementation | Includes Win ARM, Linux ARM, X11/Wayland, GTK, Linux distro repositories, platform-specific switch-project bindings and compatibility fixes. |
| Sublime license, older installers and ST3/ST4 side by side | [Portable keys][license], [Previous builds][previous], [Side by side][sidebyside] | out of scope | Velocity has its own binary, license, profile and distribution | Vendor provisioning, registration, legacy product installation and migration. |
| Older macOS support and vendor-only workarounds | [OS compatibility][os], release ledger below | out of scope | `vel-platform/macos.rs` native bootstrap and different renderer | Includes App Translocation helper fixes, old media/graphics and modal-loop workarounds; test supported macOS separately. |
| Maintenance bullets mentioning additions (not new capabilities) | [Download][releases], [Dev Builds][dev], ledger below | out of scope | Scope rule above | Fixes to already-added sheets, files, tokens and analogous conditions do not expand the matrix. |

## Verification

### Round 36 lane C (`bbf4965`)

The full gate returned `GATE_OK`: formatting, strict release Clippy, 1,736
workspace tests with no failures, rustdoc, release build, static bundle
metadata, and CLI forwarding. All 13 `r36_` regressions passed. The syntax
runner reported 126,600 passed, zero failed and zero skipped; the parser
differential test and the 10 MB syntax benchmark also completed. The live
checks remain open in [the checklist](manual-checks.md#6-round-36-parity).

Five alternating release pairs against clean `bbf4965` on the same fixture
gave median retired-instruction changes of **+0.78%** for view features,
**+0.93%** for 1,000 carets, and **+0.30%** for one caret. No GUI windows or
packaging commands were run.

### Round 34 lane B (`bc4c141`)

The full gate returned `GATE_OK`: formatting, strict release Clippy, 1,711
workspace tests with no failures, rustdoc, release build, bundle metadata
checks, and CLI forwarding. The non-reading build-child test timed out on the
first run and passed on the full rerun. All eleven `r34_` tests passed. AppKit
click-through, pointer hiding, and terminal launch were tested to the native
boundary; no GUI windows were opened.

Five alternating release pairs, measured with `proc_pid_rusage` against
`bc4c141`, differed by **+0.17%** for view features, **-0.41%** for 1,000
carets, and **+0.41%** for one caret (median paired retired instructions).

### Earlier audit (`efe8b5b`)

Formatting and strict workspace Clippy passed. Focused headless regressions
passed, including Print with a mocked browser, the static localization catalog,
the Find in Files menu and traversal, Goto Anything filtering, the new-window
folder request, and the settings effect sweep. No native windows were opened.

The full gate was not green: it reported 1,602 passed and 13 failed in the run
before the final Print test and catalog fixes. Those two passed on focused
reruns. Of the other failures, the Git and session-writer tests passed in
isolation, as did the settings sweep; the build output poll-count failure
reproduced in isolation. There was no successful full rerun.

Three alternating release-binary pairs against `efe8b5b`, measured with
`proc_pid_rusage`, gave median paired retired-instruction deltas of
**-0.23%** for features, **-0.31%** for 1,000 carets, and **-0.89%** for
one-caret smoke. All are within ±1%. The features workload used the same
10 MB fixture and a 420-second watchdog for both binaries after the default
180-second watchdog interrupted the first attempt.

## Remaining work by user impact

| Priority | Remaining gap family | Estimate |
| --- | --- | --- |
| 1 | Search: remaining result-tab modifiers and unresolved large-result edge cases | 1–3 days |
| 2 | Remaining tab-selection and group-layout edge cases | 1–3 days |
| 3 | Remaining image-tab context actions and format edge cases (PSD is out of scope) | 1–3 days |
| 4 | Safe session restores, workspace interoperability, and macOS Space placement if a public API becomes available | 4–8 days |
| 5 | Font picker, remaining scope-fold and control-character compatibility, live AppKit and keyboard-layout checks | 3–6 days |
| 6 | External terminal configuration compatibility and native Indexing Status checks | 1–2 days |
| 7 | Syntax test runner and grammar revisions, stricter theme/file icon/minihtml compatibility, completion and regex edges | 1–3 weeks, depending on fixture breadth |
| 8 | Verified/full Sublime Merge discovery, handoff and context actions | 1–2 weeks |
| 9 | Renderer/worker policy customization and performance equivalence outside the measured budgets | 1–2 weeks |

Python is deliberately not in the backlog for 1.0. It would require separate
scoping for runtime isolation, resource semantics, events and ecosystem support.

## Changelog coverage ledger

<!-- BEGIN PARITY LEDGER -->
The stable page publishes 16 build sections and the dev page publishes
124 in the inclusive 4050–4213 range. Some numbers have no published
section on either page. Each published section was reviewed, including
entries that introduce a feature without the word "Added". Counting that
word anywhere in a bullet (including bug-fix prose), reconciliation is:

| History | Published builds | Added bullets | Linked to matrix rows | Explicitly grouped below |
| --- | ---: | ---: | ---: | ---: |
| [Download][releases] | 16 | 170 | 113 | 57 |
| [Dev Builds][dev] | 124 | 212 | 128 | 84 |

Grouped bullet ordinals (the corresponding scope-exclusion rows above cover these):

**Historical macOS or OpenGL workaround (8):**
D4151.65, D4142.64, V4148.22, V4130.18, V4095.38, V4092.13, V4092.14, V4088.9.

**Maintenance mentioning an item being added (9):**
D4142.47, D4121.12, D4121.16, V4136.10, V4114.12, V4114.14, V4114.21, V4103.8, V4052.4.

**Python-only API, host, callbacks or console (90):**
D4213.18, D4213.19, D4200.3, D4200.18, D4200.19, D4180.60, D4166.32, D4166.39, D4151.50,
D4151.51, D4151.52, D4142.69, D4142.74, D4142.75, D4126.38, D4121.58, D4107.26, D4107.289,
D4107.290, D4107.291, D4107.299, D4107.300, D4107.301, D4107.307, D4107.311, D4107.315,
D4107.316, D4107.317, D4107.320, D4107.322, D4107.326, D4107.331, D4107.334, D4107.335,
D4107.336, D4107.337, D4107.344, D4107.345, D4107.346, V4205.24, V4205.25, V4198.16,
V4194.1, V4194.13, V4174.13, V4156.3, V4156.4, V4148.19, V4145.14, V4145.15, V4138.1,
V4136.11, V4130.17, V4114.30, V4100.13, V4100.14, V4100.16, V4099.24, V4098.18, V4096.16,
V4096.17, V4096.19, V4095.48, V4095.49, V4092.15, V4088.10, V4085.8, V4084.3, V4083.15,
V4083.21, V4083.22, V4083.23, V4081.13, V4081.15, V4081.17, V4081.18, V4075.47, V4074.13,
V4069.15, V4067.11, V4064.2, V4063.12, V4057.4, V4057.5, V4053.13, V4050.10, V4050.107,
V4050.112, V4050.113, V4050.114.

**Windows/Linux-only feature or platform fix (34):**
D4180.7, D4166.44, D4151.53, D4142.51, D4126.23, D4107.64, D4107.69, D4107.102, D4107.141,
D4107.143, D4107.144, D4107.145, D4107.233, V4206.20, V4174.16, V4164.11, V4149.20,
V4136.12, V4123.11, V4096.1, V4095.42, V4095.47, V4092.2, V4086.6, V4083.25, V4081.21,
V4075.35, V4075.36, V4074.9, V4072.4, V4072.5, V4050.23, V4050.42, V4050.65.

[Download][releases] builds with no bullet containing "Added" (5):
4192, 4189, 4169, 4152, 4143.

[Dev Builds][dev] builds with no bullet containing "Added" (64):
4212, 4211, 4210, 4199, 4196, 4195, 4191, 4190, 4188, 4187, 4185, 4183, 4181, 4177, 4175,
4173, 4172, 4171, 4168, 4167, 4165, 4160, 4159, 4155, 4154, 4150, 4147, 4146, 4141, 4140,
4139, 4137, 4131, 4129, 4125, 4120, 4119, 4118, 4117, 4116, 4115, 4112, 4111, 4109, 4106,
4105, 4104, 4102, 4101, 4090, 4089, 4079, 4078, 4077, 4076, 4073, 4068, 4062, 4061, 4058,
4056, 4055, 4054, 4051.
<!-- END PARITY LEDGER -->

[index]: https://www.sublimetext.com/docs/
[releases]: https://www.sublimetext.com/download
[dev]: https://www.sublimetext.com/dev
[community]: https://docs.sublimetext.io/
[api]: https://www.sublimetext.com/docs/api_reference.html
[pyenv]: https://www.sublimetext.com/docs/api_environments.html
[port]: https://www.sublimetext.com/docs/porting_guide.html
[multi]: https://www.sublimetext.com/docs/multiple_selection_with_the_keyboard.html
[column]: https://www.sublimetext.com/docs/column_selection.html
[settings]: https://www.sublimetext.com/docs/settings.html
[patterns]: https://www.sublimetext.com/docs/file_patterns.html
[commands]: https://docs.sublimetext.io/reference/commands.html
[comments]: https://docs.sublimetext.io/reference/comments.html
[completion-reference]: https://docs.sublimetext.io/reference/completions.html
[workspace-guide]: https://docs.sublimetext.io/guide/usage/file-management/projects.html#workspaces
[search]: https://docs.sublimetext.io/guide/usage/search-and-replace.html
[find-default]: https://forum.sublimetext.com/t/st4-find-in-files-not-searching-all-subfolders/58866/12
[goto]: https://sublime-text-unofficial-documentation.readthedocs.io/en/latest/file_management/file_navigation.html
[palette]: https://sublime-text-unofficial-documentation.readthedocs.io/en/latest/extensibility/command_palette.html
[snippets]: https://sublime-text-unofficial-documentation.readthedocs.io/en/latest/extensibility/snippets.html
[indent]: https://www.sublimetext.com/docs/indentation.html
[indexing]: https://www.sublimetext.com/docs/indexing.html
[tabs]: https://www.sublimetext.com/docs/tab_multi-select.html
[appkit-spaces]: https://developer.apple.com/documentation/appkit/nswindow/collectionbehavior-swift.struct#Spaces
[projects]: https://www.sublimetext.com/docs/projects.html
[distraction]: https://www.sublimetext.com/docs/distraction_free.html
[font]: https://www.sublimetext.com/docs/font.html
[gpu]: https://www.sublimetext.com/docs/gpu_rendering.html
[ligatures]: https://www.sublimetext.com/docs/ligatures.html
[completion]: https://www.sublimetext.com/docs/completions.html
[syntax]: https://www.sublimetext.com/docs/syntax.html
[scopes]: https://www.sublimetext.com/docs/scope_naming.html
[selectors]: https://www.sublimetext.com/docs/selectors.html
[spell]: https://www.sublimetext.com/docs/spell_checking.html
[vintage]: https://www.sublimetext.com/docs/vintage.html
[keys]: https://www.sublimetext.com/docs/key_bindings.html
[menus]: https://www.sublimetext.com/docs/menus.html
[packages]: https://www.sublimetext.com/docs/packages.html
[schemes]: https://www.sublimetext.com/docs/color_schemes.html
[themes]: https://www.sublimetext.com/docs/themes.html
[minihtml]: https://www.sublimetext.com/docs/minihtml.html
[build]: https://www.sublimetext.com/docs/build_systems.html
[git]: https://www.sublimetext.com/docs/git_integration.html
[diff]: https://www.sublimetext.com/docs/incremental_diff.html
[cli]: https://www.sublimetext.com/docs/command_line.html
[oldcli]: https://www.sublimetext.com/docs/2/osx_command_line.html
[safe]: https://www.sublimetext.com/docs/safe_mode.html
[revert]: https://www.sublimetext.com/docs/revert.html
[os]: https://www.sublimetext.com/docs/os_compatibility.html
[linux]: https://www.sublimetext.com/docs/linux_repositories.html
[previous]: https://www.sublimetext.com/docs/previous_versions.html
[sidebyside]: https://www.sublimetext.com/docs/side_by_side.html
[license]: https://www.sublimetext.com/docs/portable_license_keys.html
