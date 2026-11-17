# Editing, input, find and completion settings

Settings use the normal live view layers, including project, syntax and view overrides.
These rows belong to Lane A, round 18; scrolling and display settings belong to Lane B.
Descriptions and implementations are original. Public sources were checked on 2026-11-17;
no Sublime application resources or Default package were consulted.

| Key | Velocity default | Support |
| --- | --- | --- |
| `indent_to_bracket` | `false` | Supported: Enter aligns after an unmatched `(` or `[` with content on its line, ignoring strings/comments. Checks line/column limits before requesting scopes, then scans at most 64 lines / 8 KiB; preserves hanging and bracket-pair indentation. Requires auto indentation; unavailable exact scopes keep normal indentation. |
| `find_selected_text` | `true` | Supported: opening or re-running Find, Replace or Incremental Find seeds a single-line selection. Empty selections keep the existing query. |
| `auto_find_in_selection` | `false` | Supported: a multi-line selection enables In Selection when opening a find-family panel. |
| `close_find_after_find_all` | `true` | Supported: controls Find All panel closure. Explicit keyboard shortcuts still close. |
| `close_find_after_replace_all` | `true` | Supported: controls Replace All panel closure after a successful operation. Invalid expressions keep feedback visible. |
| `move_to_limit_on_up_down` | `false` | Supported: Up/Down on the first/last visual row reaches the document boundary, including extending selections. Pages are unaffected. |
| `auto_close_tags` | `true` | Supported: typing `/` after `<` in HTML/XML completes the innermost open tag. Template eligibility, including PHP and transitive HTML embeds, is computed when linking the syntax set and cached. Scans backwards at most 128 lines / 16 KiB using tag scopes. Comments, raw markup, attributes and embedded source are excluded. Cold/stale prefixes under 256 lines / 64 KiB with no long lines may be parsed through the caret line; larger prefixes wait for exact scopes, guarded by revision and selection. Leaving the view resolves a valid pending completion within that budget or cancels it before saving; later publications cannot edit a background buffer. |
| `auto_complete_trailing_symbols` | `false` | Supported: controls punctuation inferred from project-index evidence. Authored completion/snippet contents are preserved. |
| `auto_complete_trailing_spaces` | `false` (retained) | Opt-in inference requires at least three space-followed occurrences and at least 75% of occurrences. Adds a space only at the immediate end of line (including EOF), never before a non-whitespace character. Existing whitespace is left in place without adding another space. Authored completions are preserved. |
| `save_on_focus_lost` | `false` | Supported: saves only the dirty file-backed view being left on a tab switch or window/application deactivation. Shared buffers are written once; inactive tabs and lazy restored drafts are not loaded or saved. Untitled, loading, read-only, composing, deleted and externally changed documents are skipped. Failures use status text; no dialogs. |
| `word_separators` | `` ./\()"'-:,.;<>~!@#$%^&*\|+=[]{}`~? `` | Supported by word motions and selections; unchanged this round. |
| `auto_complete_selector` | `meta.tag, source - comment - string.quoted.double.block - string.quoted.single.block - string.unquoted.heredoc` | Supported: the public documentation's selector permits ordinary quoted strings and excludes comments and block/heredoc strings. |
| `auto_complete_use_history` | `true` (retained) | Supported. Public documentation describes the switch but does not establish the proposed `false` default. |
| `auto_complete_preserve_order` | `"some"` (retained) | Supported values: `"none"` ranks textual matches, `"some"` also considers likelihood/proximity, `"strict"` keeps source order. Legacy `true` maps to `"strict"`, `false` to `"some"`; unknown values use `"some"`. |
| `auto_complete_include_snippets_when_typing` | `false` (retained) | Supported. Public documentation describes exclusion when false but does not establish the proposed `true` default. |
| `auto_complete_triggers` | `[]` (retained) | Explicit entries support selectors, optional character sets and `rhs_empty`. Scope tests use the just-typed character, on the completion worker. `rhs_empty` requires EOF or immediate whitespace at every caret. The published docs do not establish the proposed combined default trigger list. |

Sources and evidence boundaries:

- [Settings layering](https://www.sublimetext.com/docs/settings.html) and
  [community settings reference](https://docs.sublimetext.io/reference/settings.html):
  live layers, selection seeding and saving on tab/application deactivation.
- [Indentation](https://www.sublimetext.com/docs/indentation.html): bracket alignment and
  its explicit false default.
- [Completions](https://www.sublimetext.com/docs/completions.html): order value set, selector
  example, inferred suffixes, snippet variables, kinds and explicit trigger overrides.
  The selector shown is a documented example; the page does not label it a shipped default.
- [ST4 changelog](https://www.sublimetext.com/download), [development history](https://www.sublimetext.com/dev):
  inferred trailing symbols disabled by default, Find close controls, automatic selectors
  evaluated on the just-typed character (4072), read-only/reload fixes for focus saves.
- [HTML/XML behavior changes](https://www.sublimetext.com/2to3): closing tags on `</`.
- [Historical movement setting](https://www.sublimetext.com/updates/2/stable/release_notes.html):
  movement to document limits. Its historical macOS default is not used as evidence for ST4;
  the current requested cross-platform false default is retained.
- [Community completion triggers](https://docs.sublimetext.io/guide/extensibility/completions.html):
  optional character/selector fields and the immediate-right-character whitespace test.
- [Snippet format](https://docs.sublimetext.io/guide/extensibility/snippets.html): suffix
  tab triggers and scope selectors. The official completions variable table defines
  `TM_LINE_INDEX` as the zero-based **line number** and `TM_SCOPE` as the syntax's **base scope**; that takes precedence over the
  community description of the complete caret scope.

The public settings pages do not enumerate all shipped defaults. New preference fallbacks
use the task's specified values where no shipped value is published; existing behavior is
retained for history, snippets while typing, the trigger list, order (`"some"`), inferred spaces (`false`) and empty-caret find seeding. Native
comparison of undocumented details remains on the manual checklist. No key is accepted as
a silent no-op.

Tab inserts the best matching completion directly. An exact in-scope snippet trigger wins even
when the typing popup excludes snippets or `tab_completion` is false; Enter commits the popup's
selected completion. Snippet fields, indentation and selections retain their existing paths.
Tab-trigger replacement does not create a selection: `$SELECTION` and
`$TM_SELECTED_TEXT` are empty, and `$TM_CURRENT_WORD` uses the text after removing the trigger.
Any variable outside the built-in table requests scope-selected `shellVariables`, including
template and user variables. Built-ins such as `TM_SCOPE`, and snippets without variables, need
no parse. Exact fallback scopes are cached by buffer, revision and syntax, with at most a
256 KiB prefix and fewer than 256 preceding lines. Long lines (over 8 KiB) and viewport mode
never trigger a fallback parse. Scoped snippets and metadata may instead use a mapped published
scope when no intervening edit touched the caret line. Otherwise Tab inserts its ordinary tab;
an explicit metadata-dependent insertion is skipped until scopes are available.

Palette filtering uses only current or mapped published scopes and never parses synchronously.
Without scopes it initially shows snippets unfiltered, then refilters after publication only
while the same panel, query, document revision and caret remain active.

## Implementation and headless coverage

Application regressions live in `crates/velocity/src/app_round18_tests.rs` and use named
commands, key dispatch, text input, real resource loading and existing panel workers.

| Item | Implementation | Regression |
| --- | --- | --- |
| Settings 1 | `editing.rs: bracket_indents`, `newline_with_blank` | `bracket_indent_both_values_and_live_syntax_override`; `bracket_alignment_batches_carets_ignores_quoted_delimiters_and_undoes_once` |
| Settings 2 | `panels.rs: Panels::action` | `find_panel_seeding_table_and_close_after_all_values` |
| Settings 3 | `panels.rs: Panels::action`, `Panels::key`; Lane A keymap block | Above table; `explicit_find_shortcuts_close_even_when_buttons_stay_open` |
| Settings 4 | `editing.rs: Editor::language_command` | `up_down_document_limit_table_including_extend_and_input_focus` |
| Settings 5 | `editing.rs: closing_tag_values`, `exact_scopes` | `close_tags_table_cold_stale_markup_void_quotes_comments_and_xml_case`; `close_tags_rejects_an_edit_stale_comment_and_handles_unicode` |
| Settings 6 | `vel-complete: CompletionEngine::query_cancel`; `completion.rs: insert_completion_item` | `completion_suffix_settings_reach_the_commit_path`; component suffix/source table |
| Settings 7 | `app.rs: save_on_focus_lost`, `sync_active_view`, `on_frame` | `focus_lost_saving_is_opt_in_and_skips_conflicts_readonly_and_untitled`; `focus_loss_tab_transition_saves_the_previous_document`; `focus_save_failure_is_status_only_and_preserves_disk_contents` |
| Snippets 1.1 | `vel-snippet: Snippet::expand`; `completion.rs: expand_snippet`, `system_fullname` | `documented_snippet_variable_table_at_a_unicode_column`; `snippet_environment_uses_system_name_and_document_base_scope` |
| Snippets 1.2 | `vel-snippet: tab_trigger_range`; `completion.rs: intelligence_command`; `panels.rs: Panels::action` | `suffix_trigger_longest_and_duplicate_choice_through_key_dispatch`; `identifier_snippet_triggers_keep_the_existing_word_boundary`; `selected_snippet_choice_is_rejected_after_a_document_change` |
| Snippets 1.3 | `panels.rs: query_quick` | `scoped_snippets_are_filtered_in_the_real_palette` |
| Completions 2.1 | `completion.rs: intelligence_command`, `poll_completion` | `tab_best_match_decision_table_and_snippet_field_selection_indent_exceptions`; existing stale-popup regressions |
| Completions 2.2 | `completion.rs: settings`; `vel-complete: query_cancel` | `triggers_run_on_text_input_and_panel_input_stays_isolated`; `explicit_trigger_selector_and_rhs_table`; `an_inactive_trigger_adds_no_scope_lookups_to_normal_typing` |
| Completions 2.3 | `vel-complete: CompletionSettings`, `PreserveOrder`, `BufferWords`, `query_cancel` | `automatic_snippet_inclusion_and_default_selector_table`; `preserve_order_documented_values_and_history_switch`; `partial_order_uses_likelihood_while_normal_order_uses_textual_match` |
| Completions 2.4 | `completion.rs: Completion::paint`, `kind_class`, `matched_ranges`; `vel-complete: parse_kind` | `popup_scene_uses_kind_colors_match_color_and_kind_footer`; namespace kind component test |
| Startup | `App` and `Workspace` real resource constructors | `fresh_bundled_startup_has_no_modal_or_status_diagnostics` |
| 10,000 carets | Same input path, background publication and completion commit | `many_caret_input_hooks_timing` (explicitly ignored by the default suite; run with `--ignored --nocapture`) |

Menu integration needs no new command rows this round. The existing `find_all`, `replace_all`,
`auto_complete` and `insert_snippet` commands pick up the new behavior. No dependency changed,
so `Cargo.lock` remains unchanged.

Review regressions additionally cover `inferred_spaces_need_repeated_evidence_and_a_safe_right_side`,
`default_ranking_keeps_proximity_and_legacy_booleans`,
`exact_snippet_beats_multiple_candidates_on_tab_with_and_without_popup`,
`stale_auto_pairs_and_base_scope_variables_never_parse`,
`scoped_trigger_and_comment_variables_share_one_prefix_parse`,
`punctuation_trigger_boundaries_and_trigger_variable_context`,
`hanging_indentation_and_bracket_pairs_keep_normal_enter_behavior`,
`close_tags_skip_raw_markup_nonmarkup_and_advance_past_existing_angle`,
`large_stale_tag_completion_defers_and_rejects_a_later_edit`,
`already_open_find_reseeds_using_the_same_selection_table`,
`palette_snippets_use_caret_after_comment_and_skip_unneeded_scope_work`,
`focus_save_only_leaves_the_active_file_and_never_materializes_lazy_drafts`, and
`focus_save_writes_a_shared_buffer_once_and_preserves_safety_skips`.

`large_document_input_hooks_timing` and `many_word_many_caret_suffix_query_timing` are
explicitly ignored timing tests; run them alongside `many_caret_input_hooks_timing`.

Verification regressions: `palette_maps_scopes_and_refilters_only_the_unchanged_open_query`,
`snippet_metadata_includes_go_rails_and_user_shell_variables`,
`exact_trigger_wins_with_typing_popup_and_disabled_tab_completion`,
`scoped_tab_budget_requires_an_untouched_published_caret_line`,
`leaving_view_finishes_or_cancels_tags_before_focus_save`,
`inferred_spaces_need_repeated_evidence_and_a_safe_right_side`,
`bracket_alignment_checks_long_line_limits_before_parsing`, and
`html_template_roots_close_tags_with_cold_and_stale_scopes`.
Run `palette_and_long_line_scope_budget_timing` explicitly for the 1.63 MB stale JavaScript
palette (<10 ms median, zero parses) and 63 KB stale-line Enter checks.

## Round 18 Lane B: display, tabs, chrome, sidebar and builds

All drawing controls resolve through the active editor's layered settings, including live
syntax overrides. Symbol-index controls remain application-global as described by the
[settings documentation](https://www.sublimetext.com/docs/settings.html).

Sources checked on 2026-11-17: [current settings reference](https://docs.sublimetext.io/reference/settings.html),
[the detailed reference from the same documentation project](https://sublime-text-unofficial-documentation.readthedocs.io/en/latest/reference/settings.html),
[ST4 release notes](https://www.sublimetext.com/download), [theme settings](https://www.sublimetext.com/docs/themes.html#settings),
[build systems](https://www.sublimetext.com/docs/build_systems.html), and
[environment expansion](https://sublime-text-unofficial-documentation.readthedocs.io/en/latest/reference/build_systems/exec.html).
The current community build-system URL fails to load; its reference navigation points to the
official build documentation. No application package or proprietary resource was consulted.

| Setting | Default / values | Behavior and evidence |
| --- | --- | --- |
| `block_caret` | `false`; boolean | Block drawing combines with existing caret styles. The current reference does not specify its default; false preserves current insertion carets. |
| `caret_extra_top`, `caret_extra_bottom` | `0`; nonnegative pixels | Extend the drawn caret; detailed reference documents zero. |
| `inverse_caret_state` | `false`; boolean | Inverts blink opacity, including solid carets. Public references do not establish its default; retained false. |
| `highlight_line_number` | `false`; boolean | Uses `gutter_foreground_highlight` for all caret lines, falling back to foreground. Name confirmed by ST4 notes; default is unlisted, so prior unhighlighted numbers remain. |
| `draw_minimap_border` | `false`; boolean | Round 17's reviewed four-edge viewport outline remains; reference confirms false. |
| `minimap_scroll_to_clicked_text` | `false`; boolean | False centers clicked text; true places it at the pointer's y coordinate. Both use the reviewed map and anchored drag. The round-18 specification supplies the geometry; public notes only name the option. |
| `highlight_modified_tabs` | `false`; boolean | A dirty `tab_label` attribute enables the theme's setting-guarded dirty-label color. Theme docs confirm the setting and attribute; default retained. |
| `enable_tab_scrolling` | `true`; boolean | False fits all tab hit rectangles into the available strip. Detailed reference confirms true. |
| `hide_new_tab_button`, `hide_tab_scrolling_buttons` | `false`; boolean | Hide theme-provided controls and remove their hit targets/width reservation. Theme docs confirm names; false retains theme visibility. |
| `show_full_path` | `false` on this macOS implementation; boolean | Explicit true displays the absolute path. Detailed reference establishes false on macOS. |
| `show_rel_path` | `false`; boolean | Prefer a containing project's relative path, with full-path/basename fallback. ST4 notes establish the name; no new default is inferred. |
| `preview_on_click` | `true`; boolean or `"only_left"` | False selects a sidebar file without opening; double click always opens it. Previews replace only clean transient tabs. Edit/save/double click promote and record recency. Transient sheets are omitted from session order. Reference confirms preview/promotion; the persistence rule follows this round's requested contract, which the public page does not explicitly describe. |
| `show_panel_on_build` | `true`; boolean | False preserves the current panel; diagnostics and status updates continue. Detailed reference establishes true. |
| `save_all_on_build` | `true`; boolean | Tools toggle saves dirty named tabs before starting. Untitled tabs are not build input files. This round requests the setting and enabled default; the public build page does not specify a preference default. |
| `show_line_column` | `"compact"`; `"enabled"`, `"compact"`, `"disabled"` | Full/abbreviated/hidden position item; selection and encoding items remain. ST4 confirms the name but does not state a default. Existing abbreviated presentation is retained. |
| `tree_animation_enabled` | `true`; boolean | Interpolate folder-row positions on expand/collapse, restart from current positions, and hit-test the drawn rows. False, disabled global animations, or native Reduce Motion make changes immediate. Detailed reference establishes true. |
| `scroll_speed` | `1.0`; nonnegative number | Keyboard page/recenter/Goto line animation only; zero jumps immediately. Wheel/trackpad and Reduce Motion remain immediate. Endpoints use visual rows to survive zoom/scale changes. Both references establish the speed range; detailed reference establishes 1.0. |
| `binary_file_patterns` | documented array of 13 globs | `*.jpg`, `*.jpeg`, `*.png`, `*.gif`, `*.ttf`, `*.tga`, `*.dds`, `*.ico`, `*.eot`, `*.pdf`, `*.swf`, `*.jar`, `*.zip`. Search/Goto omit them; sidebar retains them. Detailed reference establishes the array. |
| `index_exclude_gitignore` | `true`; boolean | Symbol indexing only. File lists still include gitignored paths. ST4 notes confirm the enabled default. |
| `index_files` | `true`; boolean | Application-global off cancels symbol indexing without changing file lists. Both indexing/reference documentation establish true. |

Build environment resolution expands environment variables and file/project variables without
re-expanding substituted contents. `path` overrides `env.PATH`; omitted `working_dir` uses the
current file's directory. All builds capture the user's `$SHELL -l -c` environment on a worker, cache successes until
`path`/`env` change, and execute commands with that environment. Startup failure/timeout falls
back to inherited values with a status message. Snippet transformations use `vel-snippet`,
including first-only replacement and `g`. Selectors use `vel_scope::Selector` and exact caret
scopes. Automatic matching is computed once per request. Root-only selectors avoid parsing;
stale scopes needed for embedded selectors use a five-millisecond synchronous budget.
An explicit Tools build-system choice bypasses matching.

The build output `word_wrap` option supports false/true. The public build documentation does
not establish a default, so Velocity follows the round-18 specification: true. `interactive: true` produces an explicit
unsupported message instead of waiting on stdin. Output queues, retained output and diagnostics
are bounded; cancellation/restart kills the old process group before the replacement starts;
reaping remains asynchronous and pipe drain has a short deadline.
The requested finish/cancel/quiet formatting and matching-set Build With memory have headless tests.

Opening or accepting settings/syntax/palette commands preserves parked scroll positions unless
selection or text changes. Explicit recenter/find/result navigation keeps its reveal behavior.
The appearance menu/palette entries reference Lane A's `select_color_scheme` and `select_theme`;
those implementations are not yet present at baseline `dad6e3d`. Menu contract tests register
those declared WindowCommand names only, without implementing the pickers in this lane.
