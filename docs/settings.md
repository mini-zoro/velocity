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
| `word_wrap` | `"auto"` | Automatic wrapping still respects the large-file limits. `true`, including View > Word Wrap, explicitly wraps even multi-megabyte lines. |
| `close_find_after_find_all` | `true` | Supported: controls Find All panel closure. Explicit keyboard shortcuts still close. |
| `close_find_after_replace_all` | `true` | Supported: controls Replace All panel closure after a successful operation. Invalid expressions keep feedback visible. |
| `move_to_limit_on_up_down` | `false` | Supported: Up/Down on the first/last visual row reaches the document boundary, including extending selections. Pages are unaffected. |
| `auto_close_tags` | `true` | Supported: typing `/` after `<` in HTML/XML completes the innermost open tag. Template eligibility, including PHP and transitive HTML embeds, is computed when linking the syntax set and cached. Scans backwards at most 128 lines / 16 KiB using tag scopes. Comments, raw markup, attributes and embedded source are excluded. Cold/stale prefixes under 256 lines / 64 KiB with no long lines may be parsed through the caret line; larger prefixes wait for exact scopes, guarded by revision and selection. Leaving the view resolves a valid pending completion within that budget or cancels it before saving; later publications cannot edit a background buffer. |
| `auto_complete_trailing_symbols` | `false` | Supported: controls punctuation inferred from project-index evidence. Authored completion/snippet contents are preserved. |
| `auto_complete_trailing_spaces` | `false` (retained) | Opt-in inference requires at least three space-followed occurrences and at least 75% of occurrences. Adds a space only at the immediate end of line (including EOF), never before a non-whitespace character. Existing whitespace is left in place without adding another space. Authored completions are preserved. |
| `save_on_focus_lost` | `false` | Supported: saves only the dirty file-backed view being left on a tab switch or window/application deactivation. Shared buffers are written once; inactive tabs and lazy restored drafts are not loaded or saved. Untitled, loading, read-only, composing, deleted and externally changed documents are skipped. Failures use status text; no dialogs. |
| `word_separators` | `` ./\()"'-:,.;<>~!@#$%^&*\|+=[]{}`~? `` | Supported by word motions and selections; unchanged this round. |
| `sub_word_separators` | `"_"` | Extra boundaries for sub-word motion, selection, and deletion; configurable since [ST4 build 4142](https://www.sublimetext.com/download). The default retains Velocity's underscore behavior. |
| `disable_default_completions` | `false` | Set to `true` to suppress the built-in HTML and CSS rows without disabling snippets, user completions, or buffer words. Can be set per syntax. |
| `default_completions_selector` | Built-in HTML/CSS selector | Override the scopes eligible for built-in markup completions, including Tab expansion. Accepts a selector string or an array of fragments joined together. By default, Markdown prose is excluded but HTML code fences are eligible. |
| `open_files_in_new_window` | `"never"` | For ordinary file opens: `"finder_only"` affects Finder and Dock requests, and `"always"` also affects CLI opens. Legacy `true` is Finder/Dock only; `false` is `"never"`. An existing file or project can still be focused. |
| `auto_complete_selector` | `meta.tag, source - comment - string.quoted.double.block - string.quoted.single.block - string.unquoted.heredoc` | Supported: the public documentation's selector permits ordinary quoted strings and excludes comments and block/heredoc strings. |
| `auto_complete_use_history` | `true` (retained) | Supported. Public documentation describes the switch but does not establish the proposed `false` default. |
| `auto_complete_preserve_order` | `"some"` (retained) | Supported values: `"none"` ranks textual matches, `"some"` also considers likelihood/proximity, `"strict"` keeps source order. Legacy `true` maps to `"strict"`, `false` to `"some"`; unknown values use `"some"`. |
| `auto_complete_include_snippets_when_typing` | `true` | `false` hides snippets from automatic suggestions; manually invoked completion still includes snippets. The public documentation describes the switch; Velocity's default follows its snippet spec. |
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
| `index_exclude_gitignore` | `true`; boolean | Symbol indexing only. ST4 notes confirm the enabled default. |
| `goto_anything_exclude_gitignore` | `false`; boolean | Filters Goto Anything project file candidates. The workspace setting applies independently of indexing and the Find in Files session toggle. |
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
## Round 22 compatibility

- `atomic_save` defaults to `false`, as documented in the [Sublime Text 3 changelog](https://www.sublimetext.com/3).
  In-place saves keep the inode, hard links, and metadata. Before writing to the inode, Velocity journals a
  rollback clone on APFS where possible. Otherwise it keeps a complete staged revision. A failed write
  restores the old bytes where possible; otherwise the error identifies the retained recovery data. On launch
  Velocity recovers only the original inode and reports the result. Other processes may see an in-place write
  in progress. Use `true` for atomic visibility where the filesystem and permissions allow it.
- `atomic_save: true` syncs a staged file, preserves the existing metadata, and uses
  [rename(2)](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/rename.2.html).
  It adapts to a journaled write if replacement would break a hard link, drop metadata, or cannot rename over
  the target. Both modes save through symlinks and stage new files before publication. Interrupted stages
  tracked in the profile's `Recovery` folder are cleaned on startup or on the next save in that folder.
  The sidebar, Goto, and the index exclude the reserved `.velocity-save-*` names.
- `ignored_packages` applies to shipped language snippets, settings, completions, builds and syntax definitions too.
  Syntax settings use the grammar resource's basename, such as `DOT.sublime-settings` for Graphviz (DOT);
  User display-name settings override package settings and are merged before User stem settings.
  The optional `extensions` array takes precedence
  over built-in extension associations, but does not override a manually chosen syntax.
- Preferences › Key Bindings opens `Packages/User/Default (OSX).sublime-keymap`. Velocity also reads
  `Default.sublime-keymap` and the older `Velocity.sublime-keymap`, with the legacy bindings applied first.
  Other User keymap names produce a notice and are ignored. `Packages/User` may be a directory symlink.

## Live changes and when they apply

Find in Files has its own **Where > Use .gitignore** toggle. New windows and
sessions without the option start checked; the choice is saved with the window
session. It applies to the next search of project folders or explicit disk
locations, not an explicit open/current buffer scope. Changing it invalidates
pending replacements from the old search. Goto Anything's
`goto_anything_exclude_gitignore` is a separate workspace setting, default
`false`, and `index_exclude_gitignore` controls symbol indexing.
`regex_auto_escape` defaults to `true` and applies when text from an editor
selection seeds a regex query, not to typed or explicit command patterns.

**⌘,** and **Preferences › Settings…** open the graphical Settings sheet for
the current window. Search by setting name, key, description, or alias. The
sheet edits general User preferences; when a valid edit is applied, it writes
to `Packages/User/Preferences.sublime-settings`. Reset (✕) removes that override.

For the previous two-pane editor, choose **Edit in JSON** on the sheet,
**Preferences › Settings – JSON**, or **Preferences: Settings (JSON)** in the
command palette. The bundled file is read-only and the User file is editable.
Save the User file to publish changes to open windows. An explicit window toggle or a
higher-priority project, syntax, or view setting can still override a preference.
The sidebar, tabs, minimap, status bar and scrollbar geometry update without
reopening the window. Indexing options restart the index in the background.

| Setting | When an edit takes effect |
| --- | --- |
| `new_window_settings` | The next File › New Window. The new window's per-window and view overrides are a snapshot, below project and syntax settings. They do not change on a User-file reload or persist through session restore. This is a Velocity compatibility feature: Sublime Text replaced the old setting in [build 2091](https://www.sublimetext.com/blog/articles/sublime-text-2-build-2091). |
| `default_encoding`, `default_line_ending` | The next new document, including in an existing window. Existing documents retain their format; use the status bar to change it. |
| `fallback_encoding`, `detect_indentation`, `large_file.load_bytes` | The next file open. Reopen a file to choose a different decoding or loading policy. Already-open views retain detected or manually selected indentation. |
| `image_file_patterns` | Live for previews of open binary files; reopen to change how a document is initially loaded. |
| `vintage_start_in_command_mode` | When a view enters Vintage; an existing command/insert mode is not switched. |
| `hot_exit`, `remember_open_files` | The next applicable checkpoint, close or quit. Observe restored documents when opening the next session. |
| `use_simple_full_screen` | The next native window full-screen transition. An already drawn editor scene does not change. |
| `default_dir`, save and build options | The next Save As dialog, save or build respectively. |

No other shipped preference was found to require an application restart.
`show_at_startup` is not documented as a Sublime Text setting and is no longer
shipped. For tab styles, the bundled themes follow the square and rounded forms
described in the [public changelog](https://www.sublimetext.com/dev); `angled`
uses square geometry. The scrollbar default uses
[AppKit's preferred scroller style](https://developer.apple.com/documentation/appkit/nsscroller/preferredscrollerstyle).

<!-- BEGIN GENERATED SETTINGS -->
## Shipped and recognized settings

Generated by `crates/velocity/scripts/settings-reference.py`. A blank macOS
column means the general default applies. Links point to runtime readers,
including typed consumers, shipped theme guards and key-binding contexts.

| Setting | General default | macOS override | Takes effect | Description and read sites |
| --- | --- | --- | --- | --- |
| `added_words` | `` [] `` |  | Immediately | Add these words to the spelling dictionary. [vel-spell/lib.rs](../crates/vel-spell/src/lib.rs), [velocity/annotations.rs](../crates/velocity/src/annotations.rs) |
| `always_show_minimap_viewport` | `` false `` |  | Immediately | Visible default change; Visual Settings documents false (hover reveals it). https://docs.sublimetext.io/reference/settings.html#visual-settings [velocity/Velocity Light.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity%20Light.sublime-theme), [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `animation_enabled` | `` true `` | `` true `` | Immediately | Enable interface animations. [vel-ui/theme.rs](../crates/vel-ui/src/theme.rs), [velocity/panels.rs](../crates/velocity/src/panels.rs), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `atomic_save` | `` false `` |  | Next save | Write through a temporary file and rename it over the original. Even when true, files with hard links, another owner, or metadata that cannot be preserved are saved in place. In-place saves keep a rollback clone or a full staged revision for recovery. [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `auto_close_tags` | `` true `` |  | Immediately | Finish the innermost HTML or XML closing tag when a slash follows a less-than sign. [velocity/editing.rs](../crates/velocity/src/editing.rs) |
| `auto_complete` | `` true `` |  | Immediately | Show suggestions automatically while typing. [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_commit_on_tab` | `` false `` |  | Immediately | Commit the selected completion with Tab. [velocity/Velocity.sublime-keymap](../crates/velocity/resources/Packages/Default/Velocity.sublime-keymap), [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_cycle` | `` false `` |  | Immediately | Cycle through the list when moving past either end. [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_delay` | `` 50 `` |  | Immediately | Delay before automatic suggestions appear, in milliseconds. [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_include_snippets` | `` true `` |  | Immediately | Include snippets in completion results. [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_include_snippets_when_typing` | `` true `` |  | Immediately | Include snippet candidates in suggestions opened by typing. [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_preserve_order` | `` "some" `` |  | Immediately | Rank by match (none), match and likelihood (some), or source order (strict). [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_selector` | `` "meta.tag, source - comment - string.quoted.double.block - string.quoted.single.block - string.unquoted.heredoc" `` |  | Immediately | Limit automatic suggestions to code and tags, excluding comments and multi-line strings. [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_size_limit` | `` 4194304 `` |  | Immediately | Largest buffer, in bytes, eligible for automatic completion. [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_trailing_spaces` | `` false `` |  | Immediately | Infer a space at the immediate end of line from repeated, mostly consistent occurrences. [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_trailing_symbols` | `` false `` |  | Immediately | Include punctuation inferred from other occurrences of a completed word. [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_triggers` | `` [] `` |  | Immediately | Extra triggers accept a selector, optional characters and an optional rhs_empty flag. [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_use_history` | `` true `` |  | Immediately | Use prior completion choices when ordering candidates. [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_use_index` | `` true `` |  | Immediately | Include words from the project index. [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_when_likely` | `` true `` |  | Immediately | Offer suggestions when there is a likely match. [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_with_fields` | `` false `` |  | Immediately | Show automatic completions inside snippet fields. [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_find_in_selection` | `` false `` |  | Immediately | Restrict a newly opened find panel to a multi-line selection. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `auto_indent` | `` true `` |  | Immediately | Enter keeps the indentation of the current line. [velocity/editing.rs](../crates/velocity/src/editing.rs) |
| `auto_match_enabled` | `` true `` |  | Immediately | Insert matching brackets and quotes while typing. [velocity/Velocity.sublime-keymap](../crates/velocity/resources/Packages/Default/Velocity.sublime-keymap) |
| `binary_file_patterns` | `` ["*.jpg", "*.jpeg", "*.png", "*.gif", "*.ttf", "*.tga", "*.dds", "*.ico", "*.eot", "*.pdf", "*.swf", "*.jar", "*.zip"] `` |  | Immediately | Search omits these binary names while the sidebar continues to list them. [velocity/panels.rs](../crates/velocity/src/panels.rs), [velocity/workbench.rs](../crates/velocity/src/workbench.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `block_caret` | `` false `` |  | Immediately | Lane B round 18: display, tabs, window chrome and builds. Current reference: https://docs.sublimetext.io/reference/settings.html Detailed reference (same documentation project, archived): https://sublime-text-unofficial-documentation.readthedocs.io/en/latest/reference/settings.html Unspecified ST4 defaults retain Velocity's existing behavior. A filled insertion caret, independent of the blink style. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `bold_folder_labels` | Not shipped |  |  | [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme) |
| `build_env` | `` {} `` |  | Next build | Environment defaults; per-build env and path take precedence. [velocity/build.rs](../crates/velocity/src/build.rs) |
| `caret_extra_bottom` | `` 0 `` |  | Immediately | Extend the caret below the line. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `caret_extra_top` | `` 0 `` |  | Immediately | Extra caret height above and below the text line. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `caret_extra_width` | `` 0 `` |  | Immediately | Add pixels to the caret width. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `caret_style` | `` "solid" `` |  | Immediately | Choose the insertion caret's shape or blink behavior. [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/app.rs](../crates/velocity/src/app.rs), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `close_deleted_files` | `` false `` |  | Immediately | Close clean views when their files are deleted. [velocity/workbench.rs](../crates/velocity/src/workbench.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `close_find_after_find_all` | `` true `` |  | Immediately | Close Find after its Find All button runs. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `close_find_after_replace_all` | `` true `` |  | Immediately | Close Replace after its Replace All button runs. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `close_windows_when_empty` | `` false `` |  | Immediately | Close the window when its last document is closed. [velocity/app.rs](../crates/velocity/src/app.rs) |
| `color_scheme` | `` "auto" `` |  | Immediately | Editor color scheme resource name, or auto to follow appearance. [vel-color-scheme/lib.rs](../crates/vel-color-scheme/src/lib.rs), [velocity/theme_ui.rs](../crates/velocity/src/theme_ui.rs) |
| `copy_with_empty_selection` | `` true `` |  | Immediately | Copy and cut with no selection take the whole caret line. [vel-commands/lib.rs](../crates/vel-commands/src/lib.rs), [vel-config/settings.rs](../crates/vel-config/src/settings.rs) |
| `dark_color_scheme` | `` "Velocity Dark.sublime-color-scheme" `` |  | Immediately | Color scheme used for dark appearance when the scheme is automatic. [vel-color-scheme/lib.rs](../crates/vel-color-scheme/src/lib.rs), [velocity/settings_entry.rs](../crates/velocity/src/settings_entry.rs), [velocity/theme_ui.rs](../crates/velocity/src/theme_ui.rs) |
| `dark_theme` | `` "Velocity.sublime-theme" `` |  | Immediately | Theme used for dark appearance when the theme is automatic. [velocity/settings_entry.rs](../crates/velocity/src/settings_entry.rs), [velocity/theme_ui.rs](../crates/velocity/src/theme_ui.rs) |
| `default_completions_selector` | `` null `` |  | Immediately | Null uses the built-in scopes for HTML and CSS completions. [vel-complete/lib.rs](../crates/vel-complete/src/lib.rs), [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `default_dir` | `` "" `` |  | Next save | Initial directory for Save As; empty uses the document directory or the system default. [velocity/app.rs](../crates/velocity/src/app.rs) |
| `default_encoding` | `` "UTF-8" `` |  | Next new document | Initial encoding of new documents. Existing documents keep their encoding. [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [vel-fileio/lib.rs](../crates/vel-fileio/src/lib.rs) |
| `default_font_size` | Not shipped |  |  | [velocity/workbench.rs](../crates/velocity/src/workbench.rs) |
| `default_line_ending` | `` "system" `` |  | Next new document | "system" (LF on macOS), "unix" or "windows". Applies to a new document; use the status bar to change an existing document. [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [vel-fileio/lib.rs](../crates/vel-fileio/src/lib.rs) |
| `detect_indentation` | `` true `` |  | Next file open | Detects indentation on file open; an already open view keeps its detected or chosen indentation. [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `dictionary` | `` "Packages/Language - English/en_US.dic" `` |  | Immediately | Dictionary resource used for spell checking. [velocity/annotations.rs](../crates/velocity/src/annotations.rs) |
| `disable_default_completions` | `` false `` |  | Immediately | Suppress built-in HTML and CSS completions without hiding user snippets. [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `drag_text` | `` true `` |  | Immediately | Drag selected text with the mouse to move it; hold Option when releasing to copy. [velocity/mouse_select.rs](../crates/velocity/src/mouse_select.rs) |
| `draw_centered` | Not shipped |  |  | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `draw_control_characters` | `` false `` |  | Immediately | Show invisible control characters. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `draw_indent_guides` | `` true `` |  | Immediately | Draw guides for indentation levels. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `draw_minimap_border` | `` false `` |  | Immediately | Draw a border around the minimap. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `draw_unicode_bidi` | `` false `` |  | Immediately | Show bidirectional control characters. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `draw_unicode_white_space` | `` "none" `` |  | Immediately | ST4 4107 and 4126: https://www.sublimetext.com/download Keep the new display modes opt-in in Velocity. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `draw_white_space` | `` ["selection"] `` |  | Immediately | Public settings reference (https://sublime-text-unofficial-documentation.readthedocs.io/en/latest/reference/settings.html): selection by default. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `enable_tab_scrolling` | `` true `` |  | Immediately | Scroll overflowing tabs; false fits all tabs into the strip. [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme), [velocity/app.rs](../crates/velocity/src/app.rs), [velocity/app_chrome.rs](../crates/velocity/src/app_chrome.rs) |
| `ensure_newline_at_eof_on_save` | `` false `` |  | Next save | Add a final newline when saving. [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [vel-fileio/lib.rs](../crates/vel-fileio/src/lib.rs) |
| `fade_fold_buttons` | `` true `` |  | Immediately | Reveal fold controls on hover instead of showing them constantly. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `fallback_encoding` | `` "Western (Windows 1252)" `` |  | Next file open | Used when decoding non-UTF-8 data on the next open or reopen. [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [vel-fileio/lib.rs](../crates/vel-fileio/src/lib.rs), [velocity/annotations.rs](../crates/velocity/src/annotations.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `file_exclude_patterns` | `` ["*.pyc", "*.pyo", "*.exe", "*.dll", "*.obj", "*.o", "*.a", "*.lib", "*.so", "*.dylib", "*.ncb", "*.sdf", "*.suo", "*.pdb", "*.idb", ".DS_Store", "*.class", "*.psd", "*.db", "*.sublime-workspace", ".velocity-save-*"] `` |  | Immediately | File patterns omitted from the sidebar's file tree. [velocity/panels.rs](../crates/velocity/src/panels.rs), [velocity/workbench.rs](../crates/velocity/src/workbench.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `file_icon_theme` | Not shipped |  |  | [velocity/theme_ui.rs](../crates/velocity/src/theme_ui.rs) |
| `file_tab_style` | `` "angled" `` |  | Immediately | Bundled themes: "rounded" or "square". "angled" (legacy) and unknown styles use square tabs. [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme) |
| `find_selected_text` | `` true `` |  | Immediately | Seed Find, Replace and Incremental Find from a single-line selection. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `fold_buttons` | `` true `` |  | Immediately | Show fold controls in the gutter. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `folder_exclude_patterns` | `` [".svn", ".git", ".hg", "CVS", ".Trash", ".Trash-*"] `` |  | Immediately | https://docs.sublimetext.io/reference/settings.html#global-settings User arrays replace this list, including the .git exclusion. [velocity/panels.rs](../crates/velocity/src/panels.rs), [velocity/workbench.rs](../crates/velocity/src/workbench.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `font_face` | `` "Menlo" `` | `` "Menlo" `` | Immediately | Editor font family. [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/app.rs](../crates/velocity/src/app.rs), [velocity/app_clipboard.rs](../crates/velocity/src/app_clipboard.rs), [velocity/app_print.rs](../crates/velocity/src/app_print.rs), [velocity/render_scenes.rs](../crates/velocity/src/render_scenes.rs) |
| `font_options` | `` [] `` |  | Immediately | Lane B: drawing, font and window preferences. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `font_size` | `` 13 `` |  | Immediately | Size of editor text in points. [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/app_clipboard.rs](../crates/velocity/src/app_clipboard.rs), [velocity/app_print.rs](../crates/velocity/src/app_print.rs), [velocity/html_ui.rs](../crates/velocity/src/html_ui.rs), [velocity/view.rs](../crates/velocity/src/view.rs), [velocity/workbench.rs](../crates/velocity/src/workbench.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `git_diff_target` | `` "index" `` |  | Immediately | Choose the Git revision used for inline diffs. [velocity/annotations.rs](../crates/velocity/src/annotations.rs) |
| `goto_anything_exclude_gitignore` | `` false `` |  | Immediately | Filter the Goto Anything file list independently of symbol indexing and Find in Files. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `gutter` | `` true `` |  | Immediately | Show the editor gutter. [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `hide_new_tab_button` | `` false `` |  | Immediately | Hide theme-provided tab-strip controls. [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme), [velocity/app_chrome.rs](../crates/velocity/src/app_chrome.rs) |
| `hide_tab_scrolling_buttons` | `` false `` |  | Immediately | Hide the arrows for scrolling through tabs. [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme), [velocity/app_chrome.rs](../crates/velocity/src/app_chrome.rs) |
| `highlight_line` | `` false `` |  | Immediately | Public settings reference: https://sublime-text-unofficial-documentation.readthedocs.io/en/latest/reference/settings.html#visual-settings [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `highlight_line_number` | `` false `` |  | Immediately | Highlight caret-line numbers with the scheme's highlighted gutter color. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `highlight_modified_tabs` | `` false `` |  | Immediately | Use the theme's dirty-tab label color. [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme), [velocity/app.rs](../crates/velocity/src/app.rs) |
| `hot_exit` | `` "always" `` |  | Next close or quit | Choose when to retain unsaved changes for the next session. [vel-session/lib.rs](../crates/vel-session/src/lib.rs), [velocity/workbench.rs](../crates/velocity/src/workbench.rs) |
| `ignored_packages` | `` ["Vintage"] `` |  | Immediately | https://www.sublimetext.com/docs/vintage.html#enabling-vintage [velocity/vintage.rs](../crates/velocity/src/vintage.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `ignored_words` | `` [] `` |  | Immediately | Do not flag these words as spelling errors. [vel-spell/lib.rs](../crates/vel-spell/src/lib.rs), [velocity/annotations.rs](../crates/velocity/src/annotations.rs) |
| `image_file_patterns` | `` ["*.png", "*.jpg", "*.jpeg", "*.gif"] `` |  | Immediately | ST4 image selection setting: https://www.sublimetext.com/download These are the formats supported by Velocity's decoder. [velocity/open_requests.rs](../crates/velocity/src/open_requests.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `inactive_sheet_dimming` | Not shipped |  |  | [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme) |
| `indent_guide_options` | `` ["draw_normal"] `` |  | Immediately | Active guides disabled by default: https://www.sublimetext.com/updates/2/stable/release_notes.html [velocity/view.rs](../crates/velocity/src/view.rs) |
| `indent_subsequent_lines` | `` true `` |  | Immediately | Indent wrapped continuation lines. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `indent_to_bracket` | `` false `` |  | Immediately | Lane A round 18: editing, find and completion preferences. Align a new line with the column just after an unmatched parenthesis or bracket. [velocity/editing.rs](../crates/velocity/src/editing.rs) |
| `index_exclude_gitignore` | `` true `` |  | Immediately | Respect Git ignore rules while indexing. [velocity/intelligence.rs](../crates/velocity/src/intelligence.rs) |
| `index_exclude_patterns` | `` ["*.log"] `` |  | Immediately | https://www.sublimetext.com/docs/indexing.html#settings [velocity/intelligence.rs](../crates/velocity/src/intelligence.rs) |
| `index_files` | `` true `` |  | Immediately | Build a symbol index of project files. [velocity/intelligence.rs](../crates/velocity/src/intelligence.rs) |
| `index_include_patterns` | `` [] `` |  | Immediately | Restrict symbol indexing to matching patterns when this list is nonempty. [velocity/intelligence.rs](../crates/velocity/src/intelligence.rs) |
| `index_max_file_size` | Not shipped |  |  | [velocity/intelligence.rs](../crates/velocity/src/intelligence.rs) |
| `index_skip_unknown_extensions` | `` true `` |  | Immediately | Skip files with no known syntax while indexing. [velocity/intelligence.rs](../crates/velocity/src/intelligence.rs) |
| `index_workers` | `` 0 `` |  | Immediately | Index worker count. Zero selects it automatically. [velocity/intelligence.rs](../crates/velocity/src/intelligence.rs) |
| `inverse_caret_state` | `` false `` |  | Immediately | Reverse the caret's visible blink phase. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `large_file.checkpoint_interval` | `` 1024 `` |  | Immediately | Line interval between syntax checkpoints. [velocity/language.rs](../crates/velocity/src/language.rs) |
| `large_file.copy_bytes` | Not shipped |  |  | [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.diff_bytes` | `` 67108864 `` |  | Immediately | Size threshold for disabling inline diffs, in bytes. [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.enable_diff` | Not shipped |  |  | [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.enable_highlighting` | Not shipped |  |  | [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.enable_indexing` | Not shipped |  |  | [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.enable_minimap` | Not shipped |  |  | [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.enable_spell_check` | Not shipped |  |  | [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.enable_word_wrap` | Not shipped |  |  | [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.indexing_bytes` | `` 67108864 `` |  | Immediately | Size threshold for skipping symbol indexing, in bytes. [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.load_bytes` | `` 67108864 `` |  | Next file open | Huge documents: inclusive thresholds; explicit view feature commands override degradation. [velocity/workbench.rs](../crates/velocity/src/workbench.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `large_file.long_line_bytes` | `` 16384 `` |  | Immediately | Line length that activates large-file safeguards, in bytes. [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.max_checkpoints` | `` 128 `` |  | Immediately | Maximum retained syntax checkpoints. [velocity/language.rs](../crates/velocity/src/language.rs) |
| `large_file.max_line_bytes` | `` 16384 `` |  | Immediately | Maximum bytes highlighted on a long line. [velocity/language.rs](../crates/velocity/src/language.rs) |
| `large_file.max_lookback_lines` | `` 256 `` |  | Immediately | Limit on lines scanned before the viewport. [velocity/language.rs](../crates/velocity/src/language.rs) |
| `large_file.max_viewport_lines` | `` 512 `` |  | Immediately | Limit on lines prepared for viewport highlighting. [velocity/language.rs](../crates/velocity/src/language.rs) |
| `large_file.minimap_lines` | `` 10000 `` |  | Immediately | Line count threshold for disabling the minimap. [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.spell_bytes` | `` 16777216 `` |  | Immediately | Size threshold for disabling spell checking, in bytes. [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.viewport_bytes` | `` 67108864 `` |  | Immediately | File size at which highlighting is limited to the viewport, in bytes. [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.viewport_lines` | `` 1000000 `` |  | Immediately | Line count at which highlighting is limited to the viewport. [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.wrap_bytes` | `` 16777216 `` |  | Immediately | Size threshold for disabling automatic wrapping, in bytes. [velocity/large.rs](../crates/velocity/src/large.rs) |
| `light_color_scheme` | `` "Velocity Light.sublime-color-scheme" `` |  | Immediately | Color scheme used for light appearance when the scheme is automatic. [vel-color-scheme/lib.rs](../crates/vel-color-scheme/src/lib.rs), [velocity/settings_entry.rs](../crates/velocity/src/settings_entry.rs), [velocity/theme_ui.rs](../crates/velocity/src/theme_ui.rs) |
| `light_theme` | `` "Velocity Light.sublime-theme" `` |  | Immediately | Theme used for light appearance when the theme is automatic. [velocity/settings_entry.rs](../crates/velocity/src/settings_entry.rs), [velocity/theme_ui.rs](../crates/velocity/src/theme_ui.rs) |
| `line_diff_width` | Not shipped |  |  | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `line_numbers` | `` true `` |  | Immediately | Show line numbers next to the text. [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `line_padding_bottom` | `` 0 `` |  | Immediately | Add space below each line of editor text. [velocity/view.rs](../crates/velocity/src/view.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `line_padding_top` | `` 0 `` |  | Immediately | Add space above each line of editor text. [velocity/view.rs](../crates/velocity/src/view.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `margin` | `` 8 `` |  | Immediately | Set the horizontal inset for editor text. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `match_brackets` | `` true `` |  | Immediately | Highlight the matching bracket at the caret. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `match_brackets_angle` | `` false `` |  | Immediately | Match angle brackets. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `match_brackets_braces` | `` true `` |  | Immediately | Match braces. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `match_brackets_content` | `` true `` |  | Immediately | Highlight brackets around the text between them. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `match_brackets_square` | `` true `` |  | Immediately | Match square brackets. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `match_selection` | `` true `` |  | Immediately | Public settings reference (Visual Settings): other selected-word occurrences. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `match_tags` | `` true `` |  | Immediately | Match opening and closing markup tags. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `mini_diff` | `` true `` |  | Immediately | Show added, changed, and deleted lines in the gutter. [velocity/annotation_commands.rs](../crates/velocity/src/annotation_commands.rs), [velocity/annotations.rs](../crates/velocity/src/annotations.rs) |
| `minimap_horizontal_scrolling` | `` false `` |  | Immediately | Scroll the minimap horizontally with the editor. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `minimap_scroll_to_clicked_text` | `` false `` |  | Immediately | Keep clicked minimap text under the pointer instead of centering it. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `mouse_wheel_switches_tabs` | `` false `` |  | Immediately | Use a vertical wheel over tabs to switch tabs instead of scrolling the strip. https://docs.sublimetext.io/reference/settings.html#global-settings [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme), [velocity/app_mouse.rs](../crates/velocity/src/app_mouse.rs) |
| `move_to_limit_on_up_down` | `` false `` |  | Immediately | Let Up and Down reach document limits from the first and last visual rows. [velocity/editing.rs](../crates/velocity/src/editing.rs) |
| `new_window_settings` | `` {} `` |  | Next window | File > New Window snapshots this dictionary over User preferences. An edit here only affects the next New Window. Launch, session restore and Settings do not use it. [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `open_files_in_new_window` | `` "never" `` |  | Next file open | File opens can target the current window, Finder/Dock only, or all sources. [velocity/app.rs](../crates/velocity/src/app.rs) |
| `overlay_scroll_bars` | `` "system" `` |  | Immediately | "system" follows macOS Show scroll bars; "enabled" overlays editor text and fades when idle, while "disabled" reserves space for an always-visible bar. [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `package_index_url` | `` null `` |  | Immediately | Explicit HTTPS index; no registry, timer or network request is enabled by default. [velocity/app.rs](../crates/velocity/src/app.rs), [velocity/release.rs](../crates/velocity/src/release.rs) |
| `preview_on_click` | `` true `` |  | Immediately | Preview a sidebar file on one click; false selects it, and "only_left" restricts previews to left clicks. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `regex_auto_escape` | `` true `` |  | Immediately | Escape selected buffer text when prefilling a regular-expression search. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `relative_line_numbers` | `` false `` |  | Immediately | Number lines relative to the caret. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `reload_file_on_change` | `` true `` |  | Immediately | Build 4107: opt out to ask before replacing even an unchanged buffer. https://www.sublimetext.com/download [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `remember_open_files` | `` true `` |  | Next close or quit | Restore open files from the previous session. [velocity/app.rs](../crates/velocity/src/app.rs) |
| `result_base_dir` | `` "" `` |  | Next build | Base directory for relative build result paths. [velocity/build.rs](../crates/velocity/src/build.rs) |
| `result_file_regex` | `` "" `` |  | Next build | Fallback result matchers and relative path base for build output jumps. https://www.sublimetext.com/docs/build_systems.html#exec-target-options [velocity/build.rs](../crates/velocity/src/build.rs) |
| `result_line_regex` | `` "" `` |  | Next build | Match line locations in build output. [velocity/build.rs](../crates/velocity/src/build.rs) |
| `ruler_style` | `` "solid" `` |  | Immediately | Set the line pattern for rulers. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `ruler_width` | `` 1.0 `` |  | Immediately | Set the stroke width of rulers. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `rulers` | `` [] `` |  | Immediately | Columns at which to draw rulers. [vel-commands/text_ops.rs](../crates/vel-commands/src/text_ops.rs), [vel-config/settings.rs](../crates/vel-config/src/settings.rs) |
| `save_all_on_build` | `` true `` |  | Next build | Save dirty named files before invoking a build. [velocity/app_build.rs](../crates/velocity/src/app_build.rs) |
| `save_on_focus_lost` | `` false `` |  | Next save | Save changed files when their tab or window loses focus. [velocity/app.rs](../crates/velocity/src/app.rs) |
| `scroll_context_lines` | `` 3 `` |  | Immediately | Keep this many context lines near the caret when scrolling. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `scroll_past_end` | `` false `` |  | Immediately | Build 4107: https://www.sublimetext.com/download [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `scroll_speed` | `` 1.0 `` | `` 1.0 `` | Immediately | Keyboard scroll animation speed; zero jumps immediately. Pointer scrolling stays direct. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `selection_border_width` | Not shipped |  |  | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `set_unsaved_view_name` | `` true `` |  | Immediately | Name untitled tabs from the first line of text. [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `shift_tab_unindent` | `` false `` |  | Immediately | shift+tab unindents everywhere instead of only in leading white space or across lines. [velocity/Velocity.sublime-keymap](../crates/velocity/resources/Packages/Default/Velocity.sublime-keymap) |
| `show_definitions` | `` true `` |  | Immediately | Show definitions or references when hovering over a symbol. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `show_encoding` | `` true `` |  | Immediately | Show the document's encoding and line-ending format in the status bar. [velocity/app_status.rs](../crates/velocity/src/app_status.rs), [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `show_errors_inline` | `` true `` |  | Immediately | Show build diagnostics next to the source line. [velocity/html_ui.rs](../crates/velocity/src/html_ui.rs) |
| `show_full_path` | `` false `` |  | Immediately | macOS keeps basename titles unless a path option is enabled. [velocity/workbench.rs](../crates/velocity/src/workbench.rs) |
| `show_git_status` | `` true `` |  | Immediately | Show Git decorations in the sidebar. [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `show_git_status_in_status_bar` | `` true `` |  | Immediately | Show Git branch and status in the status bar. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `show_line_column` | `` "compact" `` |  | Immediately | "enabled" spells out the position; "compact" retains today's abbreviated item; "disabled" hides it. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `show_line_endings` | `` true `` |  | Immediately | Show the document's line endings in the status bar. [velocity/app_status.rs](../crates/velocity/src/app_status.rs), [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `show_minimap` | `` true `` |  | Immediately | Show a miniature overview of the document. [velocity/app.rs](../crates/velocity/src/app.rs), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `show_panel_on_build` | `` true `` |  | Next build | Reveal output when a build starts. [velocity/build.rs](../crates/velocity/src/build.rs) |
| `show_rel_path` | `` false `` |  | Immediately | Include the project-relative path in the title. [velocity/workbench.rs](../crates/velocity/src/workbench.rs) |
| `show_side_bar` | `` true `` |  | Immediately | Show the sidebar with open files and folders. [velocity/app.rs](../crates/velocity/src/app.rs), [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `show_status_bar` | `` true `` |  | Immediately | Show the status bar below the editor. [velocity/app.rs](../crates/velocity/src/app.rs), [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `show_tab_close_buttons` | `` true `` |  | Immediately | Show a close button on each tab. [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme), [velocity/app.rs](../crates/velocity/src/app.rs) |
| `show_tab_close_buttons_on_left` | `` false `` |  | Immediately | Put the close button and dirty marker on the left when the button is enabled. [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme), [velocity/app.rs](../crates/velocity/src/app.rs) |
| `show_tabs` | `` true `` |  | Immediately | Display the document tab bar. [velocity/app.rs](../crates/velocity/src/app.rs), [velocity/app_a11y.rs](../crates/velocity/src/app_a11y.rs) |
| `sidebar_on_right` | Not shipped |  |  | [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme) |
| `smart_indent` | `` true `` |  | Immediately | With auto_indent, the syntax's indentation rules add or remove a level. [velocity/editing.rs](../crates/velocity/src/editing.rs) |
| `spell_check` | `` false `` |  | Immediately | Check the spelling of text in supported scopes. [velocity/annotations.rs](../crates/velocity/src/annotations.rs), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `spelling_selector` | `` "markup.raw, source string.quoted - punctuation - meta.preprocessor.c.include, source comment - source comment.block.preprocessor, -(source, constant, keyword, storage, support, variable, markup.underline.link, meta.tag)" `` |  | Immediately | Public scope selector reference: https://sublime-text-unofficial-documentation.readthedocs.io/en/latest/reference/settings.html#system-and-miscellaneous-settings [velocity/annotations.rs](../crates/velocity/src/annotations.rs) |
| `sub_word_separators` | `` "_" `` |  | Immediately | Treat these characters as additional boundaries for sub-word commands. [vel-buffer/ops.rs](../crates/vel-buffer/src/ops.rs), [vel-commands/lib.rs](../crates/vel-commands/src/lib.rs), [vel-config/settings.rs](../crates/vel-config/src/settings.rs) |
| `sublime_merge_path` | `` "" `` |  | Immediately | Path to the Sublime Merge application. [velocity/review_ui.rs](../crates/velocity/src/review_ui.rs) |
| `syntax` | Not shipped |  |  | [velocity/workbench.rs](../crates/velocity/src/workbench.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `tab_completion` | `` true `` |  | Immediately | Tab completes the word before the caret; false limits Tab to exact snippet triggers. [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `tab_size` | `` 4 `` |  | Immediately | Spaces represented by a tab. [vel-commands/lib.rs](../crates/vel-commands/src/lib.rs), [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/app_clipboard.rs](../crates/velocity/src/app_clipboard.rs), [velocity/app_print.rs](../crates/velocity/src/app_print.rs), [velocity/appearance.rs](../crates/velocity/src/appearance.rs), [velocity/completion.rs](../crates/velocity/src/completion.rs), [velocity/editing.rs](../crates/velocity/src/editing.rs), [velocity/editor.rs](../crates/velocity/src/editor.rs), [velocity/language.rs](../crates/velocity/src/language.rs), [velocity/large.rs](../crates/velocity/src/large.rs), [velocity/details.rs](../crates/velocity/src/panels/details.rs), [velocity/panels.rs](../crates/velocity/src/panels.rs), [velocity/view.rs](../crates/velocity/src/view.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `theme` | `` "auto" `` |  | Immediately | UI theme resource name, or auto to follow appearance. [velocity/theme_ui.rs](../crates/velocity/src/theme_ui.rs) |
| `themed_title_bar` | `` true `` |  | Immediately | Apply the theme to the window title bar. [velocity/app.rs](../crates/velocity/src/app.rs) |
| `translate_tabs_to_spaces` | `` false `` |  | Immediately | Insert spaces instead of tab characters. [vel-commands/lib.rs](../crates/vel-commands/src/lib.rs), [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/app.rs](../crates/velocity/src/app.rs), [velocity/completion.rs](../crates/velocity/src/completion.rs), [velocity/editing.rs](../crates/velocity/src/editing.rs), [velocity/large.rs](../crates/velocity/src/large.rs), [velocity/details.rs](../crates/velocity/src/panels/details.rs), [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `tree_animation_enabled` | `` true `` |  | Immediately | Animate folder rows unless Reduce Motion is enabled. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `trim_automatic_white_space` | `` true `` |  | Immediately | Pressing Enter on a line that only holds indentation leaves it empty. [velocity/editing.rs](../crates/velocity/src/editing.rs) |
| `trim_only_modified_white_space` | `` true `` |  | Next save | Build 4107: https://www.sublimetext.com/download [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [vel-fileio/lib.rs](../crates/vel-fileio/src/lib.rs) |
| `trim_trailing_white_space_on_save` | `` false `` |  | Next save | Remove trailing whitespace when saving. [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [vel-fileio/lib.rs](../crates/vel-fileio/src/lib.rs) |
| `ui_font_face` | `` "" `` |  | Immediately | Empty uses the theme's UI font. A font family here overrides theme text only. [vel-ui/theme.rs](../crates/vel-ui/src/theme.rs) |
| `ui_font_size` | `` 12 `` |  | Immediately | Base font size for themed interface text. [vel-ui/theme.rs](../crates/vel-ui/src/theme.rs) |
| `ui_language` | `` "en" `` |  | Immediately | UI locale tag (for example en or zh-Hans), or auto (first supported macOS preferred language). Missing strings fall back through parent tags to English. [velocity/app.rs](../crates/velocity/src/app.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `ui_scale` | `` 1.0 `` |  | Immediately | Scale the interface without changing the editor font. [vel-ui/theme.rs](../crates/vel-ui/src/theme.rs) |
| `update_system_recent_files` | `` true `` |  | Next file open | ST4 4143: https://www.sublimetext.com/download [velocity/app.rs](../crates/velocity/src/app.rs) |
| `use_simple_full_screen` | `` false `` |  | Next full-screen transition | Use a simple full-screen window instead of a macOS Space. [velocity/app.rs](../crates/velocity/src/app.rs) |
| `use_tab_stops` | `` true `` |  | Immediately | With spaces, Tab and Backspace go to the next / previous tab stop. [vel-commands/lib.rs](../crates/vel-commands/src/lib.rs), [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/editing.rs](../crates/velocity/src/editing.rs) |
| `vintage_ctrl_keys` | `` null `` |  | Immediately | Null uses the documented platform default (enabled on macOS). https://www.sublimetext.com/docs/vintage.html#ctrl-keys [velocity/vintage.rs](../crates/velocity/src/vintage.rs) |
| `vintage_enabled` | `` false `` |  | Immediately | Enable Vintage modal editing. [velocity/Velocity Vintage.sublime-keymap](../crates/velocity/resources/Packages/Default/Velocity%20Vintage.sublime-keymap), [velocity/vintage.rs](../crates/velocity/src/vintage.rs) |
| `vintage_start_in_command_mode` | `` false `` |  | When a view enters Vintage | Start in command mode when a view enters Vintage. [velocity/vintage.rs](../crates/velocity/src/vintage.rs) |
| `vintage_use_clipboard` | `` false `` |  | Immediately | Use the system clipboard for Vintage registers. [velocity/vintage.rs](../crates/velocity/src/vintage.rs) |
| `word_separators` | `` "./\\()\"'-:,.;<>~!@#$%^&*\|+=[]{}`~?" `` |  | Immediately | Characters that end a word for double click, word motions and super+d. [vel-buffer/buffer.rs](../crates/vel-buffer/src/buffer.rs), [vel-buffer/ops.rs](../crates/vel-buffer/src/ops.rs), [vel-commands/lib.rs](../crates/vel-commands/src/lib.rs), [vel-config/settings.rs](../crates/vel-config/src/settings.rs) |
| `word_wrap` | `` "auto" `` |  | Immediately | Wrap long lines at the configured width or the viewport. [velocity/large.rs](../crates/velocity/src/large.rs), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `wrap_width` | `` 0 `` |  | Immediately | Wrap at this column. Zero uses the available width. [velocity/editor.rs](../crates/velocity/src/editor.rs), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `wrap_width_style` | `` "auto" `` |  | Immediately | Use a fixed wrap column even when it exceeds the viewport. [velocity/view.rs](../crates/velocity/src/view.rs) |
<!-- END GENERATED SETTINGS -->
