# Editing, input, find and completion settings

Settings use the normal live view layers, including project, syntax and view overrides.
These rows belong to Lane A, round 18; scrolling and display settings belong to Lane B.
Descriptions and implementations are original. Public sources were checked on 2026-11-17;
no Sublime application resources or Default package were consulted.

| Key | Velocity default | Support |
| --- | --- | --- |
| `indent_to_bracket` | `false` | Supported: Enter aligns after an unmatched `(` or `[` with content on its line, ignoring strings/comments. Checks line/column limits before requesting scopes, then scans at most 64 lines / 8 KiB; preserves hanging and bracket-pair indentation. Requires auto indentation; unavailable exact scopes keep normal indentation. |
| `find_selected_text` | `true` | Supported: opening or re-running Find, Replace or Incremental Find seeds a single-line selection. With no selection seed, macOS can supply the system Find string when `use_find_clipboard` is enabled; otherwise the existing query stays. |
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
| `inverse_caret_state` | `false`; boolean | Inverts the animated caret's idle phases. Edits and selection changes still start with a visible caret. |
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
not establish a default, so Velocity follows the round-18 specification: true.
The [`interactive` build option](https://www.sublimetext.com/docs/build_systems.html#interactive)
adds a single-line input box to the output panel on Unix. Return sends a UTF-8 line plus newline,
Control-D sends EOF after pending input, and Control-C cancels the build. Echoes use
`markup.inserted.build-input`. A line is limited to 4 KiB and the queue to eight lines, plus one
in flight. A full queue leaves the draft in place. Noninteractive builds keep stdin closed.
Cancellation/restart kills the old process group; reaping and pipe drain remain asynchronous.
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

### The Settings window

**⌘,** (**Ctrl+,** on Windows), **Velocity › Settings…**, **Preferences ›
Settings…** and **Preferences: Settings** in the command palette open the
Settings window. This is a deliberate departure from Sublime Text, where ⌘,
opens the JSON files (see D1 in `docs/behavior.md`). There is only one Settings
window: invoking it again brings it to the front with the search field focused.
It closes with its close button, ⌘W / Ctrl+W or Esc (Esc first clears a
search). Changes apply immediately; there is no Save or Apply button.

The sidebar lists the categories, with a search field above them:

| Category | Sections |
| --- | --- |
| General | Language; Startup (reopen files, keep unsaved changes); Window Behavior; Settings File (Edit Settings in JSON…) |
| Appearance | Theme (light and dark variants for Automatic); Color Scheme (light and dark variants); Window (status bar, full path, title bar, scroll bars, animations) |
| Editor | Editor Font (font family popup, size, live preview); Indentation (tab width, spaces, detection, auto indent, live preview); Word Wrap; Display; Editing; Auto Complete |
| Files | New Documents (encoding, line endings); Saving; Changes on Disk |
| Find | Find; Find Panel; Find in Files; Goto Anything |
| Side Bar & Tabs | Side Bar (including hidden files and folders); Tabs; Git; Index |
| Keys & Vintage | Bindings (Key Bindings…, Mouse Bindings…); Keyboard; Vintage |
| Packages | Packages Folder (Browse Packages); Customize (color scheme, theme); Disabled Packages |
| Advanced | Every setting with its key, a group filter, *Show modified only*, *Reset all* and *Edit in JSON* |

Curated pages use checkboxes, popups, radio buttons, number fields with
steppers, text fields and lists. They show captions, not setting keys; the
**?** at the end of a row shows the description and when the change takes
effect. A note beside a section heading applies to every option in it.
Options that only matter when another is on are dimmed until it is; options
whose `show_when` condition is false are hidden, as on Advanced. The font
popup lists the installed families the first time it opens; type to jump to
a family. The page layout lives in `Packages/Default/settings-pages.json`.

Search covers every setting, not just the curated ones: type a key, alias, or
a name or description in English or Simplified Chinese, regardless of the
display language. Matches on the curated pages come first, with the same
controls and no keys, each captioned with where it lives (*Editor › Word
Wrap*); click the caption to show the option on its page. The rest follow under
*More in Advanced* as Advanced rows with their keys. Typing while the sidebar
or a control has focus also searches; ⌘F / Ctrl+F returns to the field.

On Advanced each row shows the name and key, then the description, then (when
the change applies later) when it takes effect, each on its own line left of the
control; a list's add field and the row's *Edit in JSON* link stay in the
control column. Number fields use the same up/down stepper everywhere.
Descriptions name keys for the platform: *Option* on macOS reads *Alt* on
Windows (tokens `{alt}`, `{command}` and `{os}` in `settings-ui.json`).

Keyboard: Tab / Shift+Tab move through the search field, the category list and
the controls; Up/Down change category in the list and move between controls on
a page; Space toggles a checkbox; Enter opens a popup or edits a field;
Left/Right change a radio choice; Esc cancels an edit, clears the search, then
closes the window.

Every control writes `Packages/User/Preferences.sublime-settings`, the file
the JSON editor uses, through the same guarded writer: comments and formatting
are kept, the file is replaced atomically, and a value changed outside the
window is never overwritten (the row reports the conflict). Open editor windows
apply each change at once; edits saved in the JSON file appear in the window
within half a second. Reset (✕ on Advanced) removes an override; *Reset all*
restores the file as it was when the window opened.

The JSON pair stays one step away: **Preferences › Settings (JSON)…**,
**Preferences: Settings (JSON)**, **Ctrl+K, Ctrl+,** on Windows, *Edit
Settings in JSON…* on General and *Edit in JSON* on Advanced open the
two-pane editor. Its bundled file is read-only; the User file is editable.
Save the User file to publish changes to open windows.

The Settings window is a utility window: it is not saved in the session or
restored by hot exit, files opened from Finder or the command line go to an
editor window, it is not listed in the Window menu (or, on Windows, in the
Ctrl+F6 cycle and has no menu bar), and quitting never prompts for it. Nothing
of it is loaded before it first opens, and closing it frees it.

### When changes apply

`ui_language` defaults to `"auto"`: use the first supported macOS preferred
language, or English if none match. Set `"en"` or `"zh-Hans"` to override it.
An explicit window toggle or a
higher-priority project, syntax, or view setting can still override a preference.
The sidebar, tabs, minimap, status bar and scrollbar geometry update without
reopening the window. Indexing options restart the index in the background.

| Setting | When an edit takes effect |
| --- | --- |
| `new_window_settings` | The next File › New Window. The new window's per-window and view overrides are a snapshot, below project and syntax settings. They do not change on a User-file reload or persist through session restore. This is a Velocity compatibility feature: Sublime Text replaced the old setting in [build 2091](https://www.sublimetext.com/blog/articles/sublime-text-2-build-2091). |
| `default_encoding`, `default_line_ending` | The next new document, including in an existing window. Existing documents retain their format; use the status bar to change it. |
| `fallback_encoding`, `detect_indentation`, `large_file.load_bytes` | The next file open. Reopen a file to choose a different decoding or loading policy. Already-open views retain detected or manually selected indentation. |
| `image_file_patterns` | Live for previews of open binary files. File › Open as Image / Open as Text overrides it for that file until Velocity quits. |
| `vintage_start_in_command_mode` | When a view enters Vintage; an existing command/insert mode is not switched. |
| `hot_exit`, `remember_open_files` | The next applicable checkpoint, close or quit. Observe restored documents when opening the next session. |
| `use_simple_full_screen` | The next native window full-screen transition. An already drawn editor scene does not change. |
| `default_dir`, save and build options | The next Save As dialog, save or build respectively. |

No other shipped preference was found to require an application restart.
`show_at_startup` is not documented as a Sublime Text setting and is no longer
shipped. For tab styles, `rounded` (the default, decision D6 in `docs/behavior.md`)
draws floating card tabs: a rounded card for the selected tab, rounded hover washes,
circular close buttons and no dividers; `square` and `angled` draw the square strip
with dividers and an accent bar, after the forms described in the
[public changelog](https://www.sublimetext.com/dev). The scrollbar default uses
[AppKit's preferred scroller style](https://developer.apple.com/documentation/appkit/nsscroller/preferredscrollerstyle).

### Related behavior without a preference

- `unselect_current` removes just the focused tab from a tab multi-selection.
  It does not close the tab or clear a sole selected tab.
- The status bar reports a sum when every region of a multi-selection contains
  a finite number. `show_line_column` affects the coordinate, not the sum.
- `remember_workspace` is not shipped on macOS. Public AppKit APIs do not expose
  a persistent Space identifier and targeted restore; window frames still restore.

### Troubleshooting a profile

Use `velocity --safe-mode` to start with isolated settings and no restored session.
Use `velocity --data-dir PATH` with a new directory for a persistent clean test profile.
Neither operation resets the original profile. To rule out a stale renderer cache in a
disposable profile, quit that instance and move `PATH/Cache/Renderer` aside; leave
`Packages` and `Sessions` intact. If `VELOCITY_CACHE_DIR` names a custom index cache,
give the test instance a new directory there too. Do not apply Sublime Text's product-specific
data or cache deletion steps to Velocity.

<!-- BEGIN GENERATED SETTINGS -->
## Shipped and recognized settings

Generated by `crates/velocity/scripts/settings-reference.py`. A blank macOS
column means the general default applies. Links point to runtime readers,
including typed consumers, shipped theme guards and key-binding contexts.

| Setting | General default | macOS override | Takes effect | Description | Notes |
| --- | --- | --- | --- | --- | --- |
| `added_words` | `` [] `` |  | Immediately | Treat these extra words as valid spellings. | [vel-spell/lib.rs](../crates/vel-spell/src/lib.rs), [velocity/annotations.rs](../crates/velocity/src/annotations.rs) |
| `always_show_minimap_viewport` | `` false `` |  | Immediately | Keep the visible portion of the document marked in the minimap. | With this off, the viewport marker appears on hover. [Visual settings](https://docs.sublimetext.io/reference/settings.html#visual-settings) [velocity/Velocity Light.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity%20Light.sublime-theme), [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `animation_enabled` | `` true `` | `` true `` | Immediately | Animate interface transitions unless Reduce Motion is on. | [vel-ui/theme.rs](../crates/vel-ui/src/theme.rs), [velocity/app_tab_drag.rs](../crates/velocity/src/app_tab_drag.rs), [velocity/panels.rs](../crates/velocity/src/panels.rs), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `atomic_save` | `` false `` |  | Next save | Write to a temporary file and replace the original when possible. | Write through a temporary file and rename it over the original. Even when true, files with hard links, another owner, or metadata that cannot be preserved are saved in place. In-place saves keep a rollback clone or a full staged revision for recovery. [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `auto_close_tags` | `` true `` |  | Immediately | Finish an HTML or XML closing tag as you start typing it. | Finish the innermost HTML or XML closing tag when a slash follows a less-than sign. [velocity/editing.rs](../crates/velocity/src/editing.rs) |
| `auto_complete` | `` true `` |  | Immediately | Show suggestions automatically while typing. | [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_commit_on_tab` | `` false `` |  | Immediately | Insert the highlighted suggestion when you press Tab. | [velocity/Default (Windows).sublime-keymap](../crates/velocity/resources/Packages/Default/Default%20%28Windows%29.sublime-keymap), [velocity/Velocity.sublime-keymap](../crates/velocity/resources/Packages/Default/Velocity.sublime-keymap), [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_cycle` | `` false `` |  | Immediately | Wrap from either end of the suggestion list to the other. | [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_delay` | `` 50 `` |  | Immediately | Wait this many milliseconds before showing automatic suggestions. | [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_include_snippets` | `` true `` |  | Immediately | Include snippets in completion results. | [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_include_snippets_when_typing` | `` true `` |  | Immediately | Include snippets in automatic suggestions as you type. | Include snippet candidates in suggestions opened by typing. [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_preserve_order` | `` "some" `` |  | Immediately | Choose how much of the original suggestion order to preserve. | Rank by match (none), match and likelihood (some), or source order (strict). [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_selector` | `` "meta.tag, source - comment - string.quoted.double.block - string.quoted.single.block - string.unquoted.heredoc" `` |  | Immediately | Choose where automatic suggestions can appear. | Limit automatic suggestions to code and tags, excluding comments and multi-line strings. [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_size_limit` | `` 4194304 `` |  | Immediately | Set the largest document size, in bytes, for automatic suggestions. | [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_trailing_spaces` | `` false `` |  | Immediately | Suggest a trailing space based on other uses of the completed word. | Infer a space at the immediate end of line from repeated, mostly consistent occurrences. [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_trailing_symbols` | `` false `` |  | Immediately | Suggest punctuation that usually follows the completed word. | Include punctuation inferred from other occurrences of a completed word. [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_triggers` | `` [] `` |  | Immediately | Add scopes and characters that open automatic suggestions. | Extra triggers accept a selector, optional characters and an optional rhs_empty flag. [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_use_history` | `` true `` |  | Immediately | Rank suggestions using your previous completion choices. | Use prior completion choices when ordering candidates. [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_use_index` | `` true `` |  | Immediately | Include words from the project index in suggestions. | [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_when_likely` | `` true `` |  | Immediately | Offer automatic suggestions where a completion is likely. | [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_complete_with_fields` | `` false `` |  | Immediately | Show automatic suggestions inside snippet fields. | [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `auto_find_in_selection` | `` false `` |  | Immediately | Limit Find or Replace to a multiline selection when opening the panel. | Restrict a newly opened find panel to a multi-line selection. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `auto_hide_status_bar` | `` false `` |  | Immediately | Hide the status bar while typing in the editor; show it again on pointer activity. | [Build 4107](https://www.sublimetext.com/download#build-4107); opt-in, independent of Show Status Bar. [velocity/app.rs](../crates/velocity/src/app.rs) |
| `auto_hide_tabs` | `` false `` |  | Immediately | Hide the tab bar while typing in the editor; show it again on pointer activity. | [Build 4107](https://www.sublimetext.com/download#build-4107); opt-in, independent of Show Tabs. [velocity/app.rs](../crates/velocity/src/app.rs) |
| `auto_indent` | `` true `` |  | Immediately | Carry the current indentation onto a new line. | Enter keeps the indentation of the current line. [velocity/editing.rs](../crates/velocity/src/editing.rs) |
| `auto_match_enabled` | `` true `` |  | Immediately | Insert the closing bracket or quote as you type. | [velocity/Default (Windows).sublime-keymap](../crates/velocity/resources/Packages/Default/Default%20%28Windows%29.sublime-keymap), [velocity/Velocity.sublime-keymap](../crates/velocity/resources/Packages/Default/Velocity.sublime-keymap) |
| `binary_file_patterns` | `` ["*.jpg", "*.jpeg", "*.png", "*.gif", "*.ttf", "*.tga", "*.dds", "*.ico", "*.eot", "*.pdf", "*.swf", "*.jar", "*.zip"] `` |  | Immediately | Treat matching files as binary when listing files for navigation or search. | Search omits these binary names while the sidebar continues to list them. [velocity/panels.rs](../crates/velocity/src/panels.rs), [velocity/workbench.rs](../crates/velocity/src/workbench.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `block_caret` | `` false `` |  | Immediately | Use a filled block for the insertion caret. | A filled insertion caret, independent of the blink style. [Settings reference](https://docs.sublimetext.io/reference/settings.html); [archived reference](https://sublime-text-unofficial-documentation.readthedocs.io/en/latest/reference/settings.html) [velocity/view.rs](../crates/velocity/src/view.rs) |
| `bold_folder_labels` | Not shipped |  |  |  | [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme) |
| `build_env` | `` {} `` |  | Next build | Set environment variables shared by build systems. | Environment defaults; per-build env and path take precedence. [velocity/build.rs](../crates/velocity/src/build.rs) |
| `caret_extra_bottom` | `` 0 `` |  | Immediately | Extend the caret below the line by this many pixels. | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `caret_extra_top` | `` 0 `` |  | Immediately | Extend the caret above the line by this many pixels. | Extra caret height above and below the text line. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `caret_extra_width` | `` 0 `` |  | Immediately | Add pixels to the caret width. | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `caret_style` | `` "blink" `` |  | Immediately | Choose how the insertion caret looks or blinks. | On/off by default: half-second phases on macOS, and the system blink interval on Windows. [User decision D7](behavior.md#decisions-user-2026-11-20). [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `click_through` | `` false `` |  | Immediately | Let the first click also act in a Velocity window it activates on macOS. | [Build 4213](https://www.sublimetext.com/download#build-4213); macOS only. Off keeps the first click for activation. [velocity/app.rs](../crates/velocity/src/app.rs) |
| `close_deleted_files` | `` false `` |  | Immediately | Close unmodified documents whose files are deleted from disk. | [velocity/workbench.rs](../crates/velocity/src/workbench.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `close_find_after_find_all` | `` true `` |  | Immediately | Close the Find panel after Find All. | Close Find after its Find All button runs. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `close_find_after_replace_all` | `` true `` |  | Immediately | Close the Replace panel after Replace All. | Close Replace after its Replace All button runs. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `close_windows_when_empty` | `` true `` |  | Immediately | Close a window when you close its last document, unless it has folders or a saved project. | [velocity/app.rs](../crates/velocity/src/app.rs) |
| `color_scheme` | `` "auto" `` |  | Immediately | Choose an editor color scheme or follow the system appearance. | [vel-color-scheme/lib.rs](../crates/vel-color-scheme/src/lib.rs), [velocity/theme_ui.rs](../crates/velocity/src/theme_ui.rs) |
| `console_max_history_lines` | `` 80 `` |  | Immediately | Keep at most this many recent diagnostic lines. Zero disables history. | [Build 4107](https://www.sublimetext.com/download#build-4107). Bounds Velocity's diagnostic history, not a Python REPL. The default preserves Velocity's previous 80-line history. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `control_character_style` | `` "hex" `` |  | Immediately | Label control characters with hexadecimal values, names, or caret notation. | [Build 4107](https://www.sublimetext.com/download#build-4107). Applies to C0/DEL labels when `draw_control_characters` is on. Velocity provides `hex`, `name` and `caret`. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `copy_with_empty_selection` | `` true `` |  | Immediately | Copy or cut the entire line when no text is selected. | Copy and cut with no selection take the whole caret line. [vel-commands/lib.rs](../crates/vel-commands/src/lib.rs), [vel-config/settings.rs](../crates/vel-config/src/settings.rs) |
| `dark_color_scheme` | `` "Velocity Dark.sublime-color-scheme" `` |  | Immediately | Use this scheme in dark mode when Color Scheme is automatic. | [vel-color-scheme/lib.rs](../crates/vel-color-scheme/src/lib.rs), [velocity/settings_entry.rs](../crates/velocity/src/settings_entry.rs), [velocity/theme_ui.rs](../crates/velocity/src/theme_ui.rs) |
| `dark_theme` | `` "Velocity.sublime-theme" `` |  | Immediately | Use this theme in dark mode when Theme is automatic. | [velocity/settings_entry.rs](../crates/velocity/src/settings_entry.rs), [velocity/theme_ui.rs](../crates/velocity/src/theme_ui.rs) |
| `default_completions_selector` | `` null `` |  | Immediately | Select where built-in HTML and CSS suggestions are offered. | Null uses the built-in scopes for HTML and CSS completions. [vel-complete/lib.rs](../crates/vel-complete/src/lib.rs), [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `default_dir` | `` "" `` |  | Next save | Set the initial directory for the save dialog. | Initial directory for Save As; empty uses the document directory or the system default. [velocity/app.rs](../crates/velocity/src/app.rs) |
| `default_encoding` | `` "UTF-8" `` |  | Next new document | Set the initial encoding of new documents. | Initial encoding of new documents. Existing documents keep their encoding. [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [vel-fileio/lib.rs](../crates/vel-fileio/src/lib.rs) |
| `default_font_size` | Not shipped |  |  |  | [velocity/workbench.rs](../crates/velocity/src/workbench.rs) |
| `default_line_ending` | `` "system" `` |  | Next new document | Set the line ending format for new documents. | "system" (LF on macOS), "unix" or "windows". Applies to a new document; use the status bar to change an existing document. [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [vel-fileio/lib.rs](../crates/vel-fileio/src/lib.rs) |
| `detect_indentation` | `` true `` |  | Next file open | Detect tabs or spaces and indentation width when opening a file. | Detects indentation on file open; an already open view keeps its detected or chosen indentation. [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `dictionary` | `` "Packages/Language - English/en_US.dic" `` |  | Immediately | Choose the dictionary resource used for spell checking. | [velocity/annotations.rs](../crates/velocity/src/annotations.rs) |
| `disable_default_completions` | `` false `` |  | Immediately | Hide built-in HTML and CSS suggestions without hiding user snippets. | Suppress built-in HTML and CSS completions without hiding user snippets. [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `drag_text` | `` true `` |  | Immediately | Move selected text by dragging. Hold Option to copy it. | Drag selected text with the mouse to move it; hold Option when releasing to copy. [velocity/mouse_select.rs](../crates/velocity/src/mouse_select.rs) |
| `draw_centered` | Not shipped |  |  |  | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `draw_control_characters` | `` false `` |  | Immediately | Show marks for invisible control characters. | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `draw_indent_guides` | `` true `` |  | Immediately | Show a guide for each indentation level. | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `draw_minimap_border` | `` false `` |  | Immediately | Outline the minimap with a border. | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `draw_unicode_bidi` | `` false `` |  | Immediately | Mark invisible characters that change text direction. | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `draw_unicode_white_space` | `` "none" `` |  | Immediately | Mark Unicode spaces other than ordinary spaces and tabs. | Unicode whitespace and control character markers are opt-in. [Builds 4107 and 4126](https://www.sublimetext.com/download) [velocity/view.rs](../crates/velocity/src/view.rs) |
| `draw_white_space` | `` ["selection"] `` |  | Immediately | Choose where to show marks for spaces and tabs. | By default, show spaces and tabs only in the selection. [Visual settings](https://sublime-text-unofficial-documentation.readthedocs.io/en/latest/reference/settings.html#visual-settings) [velocity/view.rs](../crates/velocity/src/view.rs) |
| `enable_tab_scrolling` | `` true `` |  | Immediately | Scroll overflowing tabs instead of shrinking them all to fit. | Scroll overflowing tabs; false fits all tabs into the strip. [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme), [velocity/app.rs](../crates/velocity/src/app.rs), [velocity/app_chrome.rs](../crates/velocity/src/app_chrome.rs) |
| `ensure_newline_at_eof_on_save` | `` false `` |  | Next save | Add a final newline when saving a file. | [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [vel-fileio/lib.rs](../crates/vel-fileio/src/lib.rs) |
| `fade_fold_buttons` | `` true `` |  | Immediately | Hide fold buttons until the pointer is over the gutter. | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `fallback_encoding` | `` "Western (Windows 1252)" `` |  | Next file open | Try this encoding when a file cannot be decoded as UTF-8. | Used when decoding non-UTF-8 data on the next open or reopen. [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [vel-fileio/lib.rs](../crates/vel-fileio/src/lib.rs), [velocity/annotations.rs](../crates/velocity/src/annotations.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `file_exclude_patterns` | `` ["*.pyc", "*.pyo", "*.exe", "*.dll", "*.obj", "*.o", "*.a", "*.lib", "*.so", "*.dylib", "*.ncb", "*.sdf", "*.suo", "*.pdb", "*.idb", ".DS_Store", "*.class", "*.psd", "*.db", "*.sublime-workspace", ".velocity-save-*"] `` |  | Immediately | Omit matching files from the sidebar’s file tree. | [velocity/panels.rs](../crates/velocity/src/panels.rs), [velocity/workbench.rs](../crates/velocity/src/workbench.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `file_icon_theme` | Not shipped |  |  |  | [velocity/theme_ui.rs](../crates/velocity/src/theme_ui.rs) |
| `file_tab_style` | `` "rounded" `` |  | Immediately | Choose the shape of document tabs. | Bundled themes: "rounded" (floating card tabs, the default) or "square". "angled" (legacy) and unknown styles use square tabs with dividers and an accent bar. [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme) |
| `find_highlight_matches_max_size` | `` 0 `` |  | Immediately | Skip automatic literal match highlighting above this many bytes. Zero removes the limit. | [Build 4121](https://www.sublimetext.com/download#build-4121). Explicit Find Next/Previous, Find All and Replace still work. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `find_in_files_context_characters` | `` 0 `` |  | Immediately | Limit characters per result line. Zero keeps the standard preview. | [Build 4166](https://www.sublimetext.com/download#build-4166). A positive value uses Unicode scalar columns and preserves the source jump column. Zero uses the previous disk previews and live-buffer formatting. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `find_in_files_context_lines` | `` 1 `` |  | Immediately | Set how many lines appear before and after each match when Context is on. | [Build 4151](https://www.sublimetext.com/download#build-4151). Applies to the next search with Context enabled; Velocity accepts 0–100 lines on each side and defaults to one. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `find_in_files_max_file_size` | `` 0 `` |  | Immediately | Skip files larger than this many bytes. Zero removes the limit. | [Build 4142](https://www.sublimetext.com/download#build-4142). Applies on the next search to disk files and live buffers. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `find_in_files_max_result_size` | `` 0 `` |  | Immediately | Limit the displayed result bytes. Zero removes the limit; replacements still include hidden matches. | [Build 4126](https://www.sublimetext.com/download#build-4126). Applies to the next report, not to replacements or previous reports. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `find_in_files_side_by_side` | `` false `` |  | Immediately | Keep the Find Results tab visible beside the file opened from a match. | [Build 4151](https://www.sublimetext.com/download#build-4151). Only applies when opening a match from a Find Results tab, not the output panel. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `find_in_files_suppress_errors` | `` false `` |  | Immediately | Hide file access and traversal errors in search results. | [Build 4151](https://www.sublimetext.com/download#build-4151). Applies to the next search; invalid search expressions still report an error. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `find_regex_highlight_matches_max_size` | `` 0 `` |  | Immediately | Skip automatic regex match highlighting above this many bytes. Zero removes the limit. | [Build 4121](https://www.sublimetext.com/download#build-4121). Separate from the literal-search size limit. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `find_scroll_highlights_limit` | `` 4096 `` |  | Immediately | Hide scrollbar marks above this match count. Zero removes the limit. | [Build 4121](https://www.sublimetext.com/download#build-4121). 4096 is Velocity's bound when marks are enabled, not a verified Sublime default. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `find_selected_text` | `` true `` |  | Immediately | Fill the Find field from a single-line selection. | Seed Find, Replace and Incremental Find from a single-line selection. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `fold_buttons` | `` true `` |  | Immediately | Show fold controls in the gutter. | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `fold_include_trailing_newline` | `` false `` |  | Immediately | Include the final line break when collapsing a syntax-based fold. | [Builds 4134 and 4145](https://www.sublimetext.com/dev#build-4145); excludes the final newline by default. Velocity key; the public notes do not name the option. Manual selection folds are unaffected. [velocity/editing.rs](../crates/velocity/src/editing.rs), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `fold_style` | `` "auto" `` |  | Immediately | Use syntax-aware folds with indentation fallback, or use indentation only. | [Build 4151](https://www.sublimetext.com/download#build-4151); `auto` retains syntax-aware folding with an indentation fallback; `indentation` ignores syntax candidates. [velocity/fold_state.rs](../crates/velocity/src/fold_state.rs) |
| `folder_exclude_patterns` | `` [".svn", ".git", ".hg", "CVS", ".Trash", ".Trash-*"] `` |  | Immediately | Omit matching folders from the sidebar’s file tree. | User arrays replace this list, including the .git exclusion. [Global settings](https://docs.sublimetext.io/reference/settings.html#global-settings) [velocity/panels.rs](../crates/velocity/src/panels.rs), [velocity/workbench.rs](../crates/velocity/src/workbench.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `font_face` | `` "Menlo" `` | `` "Menlo" `` | Immediately | Choose the editor’s font family. | [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/app.rs](../crates/velocity/src/app.rs), [velocity/app_clipboard.rs](../crates/velocity/src/app_clipboard.rs), [velocity/app_print.rs](../crates/velocity/src/app_print.rs), [velocity/render_scenes.rs](../crates/velocity/src/render_scenes.rs) |
| `font_options` | `` [] `` |  | Immediately | Set rendering options for editor text, such as ligatures. | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `font_size` | `` 13 `` |  | Immediately | Size of editor text in points. | [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/app_clipboard.rs](../crates/velocity/src/app_clipboard.rs), [velocity/app_print.rs](../crates/velocity/src/app_print.rs), [velocity/html_ui.rs](../crates/velocity/src/html_ui.rs), [velocity/view.rs](../crates/velocity/src/view.rs), [velocity/workbench.rs](../crates/velocity/src/workbench.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `git_diff_target` | `` "index" `` |  | Immediately | Compare changes against the Git index or HEAD. | [velocity/annotations.rs](../crates/velocity/src/annotations.rs) |
| `goto_anything_exclude_gitignore` | `` false `` |  | Immediately | Omit Git-ignored files from Goto Anything. | Filter the Goto Anything file list independently of symbol indexing and Find in Files. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `goto_anything_file_preview` | `` true `` |  | Immediately | Preview file results before accepting a selection in Goto Anything. | [Build 4180](https://www.sublimetext.com/download#build-4180); default keeps the transient tab (U04). When off, file results open only on acceptance. Local line/word/symbol navigation still previews. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `gutter` | `` true `` |  | Immediately | Show the gutter next to the document. | [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `hide_new_tab_button` | `` false `` |  | Immediately | Hide the new tab button in the tab bar. | Hide theme-provided tab-strip controls. [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme), [velocity/app_chrome.rs](../crates/velocity/src/app_chrome.rs) |
| `hide_pointer_while_typing` | `` false `` |  | Immediately | Hide the mouse pointer after typing in the editor until it moves. | [Build 4107](https://www.sublimetext.com/download#build-4107) documents the typing behavior for Windows/Linux. Velocity uses AppKit on macOS; opt-in. [velocity/app.rs](../crates/velocity/src/app.rs) |
| `hide_tab_scrolling_buttons` | `` false `` |  | Immediately | Hide the arrows for scrolling through tabs. | [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme), [velocity/app_chrome.rs](../crates/velocity/src/app_chrome.rs) |
| `highlight_find_results_in_scrollbar` | `` false `` |  | Immediately | Show match marks in the scrollbar or minimap gutter. | [Dev build 4081](https://www.sublimetext.com/dev#build-4081). Off by default to retain Velocity's previous display. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `highlight_gutter` | `` false `` |  | Immediately | Highlight the gutter beside the rows containing a caret. | [Build 4107](https://www.sublimetext.com/download#build-4107). Independent of editor-line and line-number highlighting; off preserves the existing gutter. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `highlight_line` | `` false `` |  | Immediately | Highlight the line containing the caret. | [Visual settings](https://sublime-text-unofficial-documentation.readthedocs.io/en/latest/reference/settings.html#visual-settings) [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `highlight_line_number` | `` false `` |  | Immediately | Highlight the line number at the caret. | Highlight caret-line numbers with the scheme's highlighted gutter color. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `highlight_modified_tabs` | `` false `` |  | Immediately | Use the theme’s modified label color for unsaved tabs. | Use the theme's dirty-tab label color. [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme), [velocity/app.rs](../crates/velocity/src/app.rs) |
| `hot_exit` | `` "always" `` |  | Next close or quit | Choose when to keep unsaved edits for the next session. | [vel-session/lib.rs](../crates/vel-session/src/lib.rs), [velocity/workbench.rs](../crates/velocity/src/workbench.rs) |
| `hot_exit_projects` | `` true `` |  | Next close or quit | Allow project workspace files to retain unsaved changes under the Hot Exit policy. | [Build 4142](https://www.sublimetext.com/download#build-4142); default follows Hot Exit for project workspaces. Off keeps saved-file references but excludes unsaved contents and prompts before a dirty project window closes. App recovery is separate. [velocity/app.rs](../crates/velocity/src/app.rs) |
| `ignored_packages` | `` ["Vintage"] `` |  | Immediately | Disable the packages in this list. | [Enabling Vintage](https://www.sublimetext.com/docs/vintage.html#enabling-vintage) [velocity/vintage.rs](../crates/velocity/src/vintage.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `ignored_words` | `` [] `` |  | Immediately | Do not flag these words as spelling errors. | [vel-spell/lib.rs](../crates/vel-spell/src/lib.rs), [velocity/annotations.rs](../crates/velocity/src/annotations.rs) |
| `image_file_patterns` | `` ["*.png", "*.jpg", "*.jpeg", "*.gif", "*.webp", "*.tga"] `` |  | Immediately | Open matching files in the image viewer. | These are the formats supported by Velocity's decoder. WebP uses macOS ImageIO; PSD is not supported. [Build 4180](https://www.sublimetext.com/download#build-4180) [Preview loader](../crates/velocity/src/image_decode.rs) [velocity/open_requests.rs](../crates/velocity/src/open_requests.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `inactive_sheet_dimming` | Not shipped |  |  |  | [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme) |
| `indent_guide_options` | `` ["draw_normal"] `` |  | Immediately | Choose whether to show all guides or emphasize the active indent. | Add "draw_active" to highlight the current indentation level. [Active-guide default](https://www.sublimetext.com/updates/2/stable/release_notes.html) [velocity/view.rs](../crates/velocity/src/view.rs) |
| `indent_subsequent_lines` | `` true `` |  | Immediately | Keep the original line’s indentation on wrapped lines. | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `indent_to_bracket` | `` false `` |  | Immediately | Align the next line just after an unmatched opening bracket. | Align a new line with the column just after an unmatched parenthesis or bracket. [velocity/editing.rs](../crates/velocity/src/editing.rs) |
| `index_exclude_gitignore` | `` true `` |  | Immediately | Respect Git ignore rules when building the symbol index. | [velocity/intelligence.rs](../crates/velocity/src/intelligence.rs) |
| `index_exclude_patterns` | `` ["*.log"] `` |  | Immediately | Skip matching files when building the symbol index. | [Indexing settings](https://www.sublimetext.com/docs/indexing.html#settings) [velocity/intelligence.rs](../crates/velocity/src/intelligence.rs) |
| `index_files` | `` true `` |  | Immediately | Build a symbol index of project files. | [velocity/app.rs](../crates/velocity/src/app.rs), [velocity/intelligence.rs](../crates/velocity/src/intelligence.rs) |
| `index_include_patterns` | `` [] `` |  | Immediately | When the list is nonempty, index only matching files. | [velocity/intelligence.rs](../crates/velocity/src/intelligence.rs) |
| `index_max_file_size` | Not shipped |  |  |  | [velocity/intelligence.rs](../crates/velocity/src/intelligence.rs) |
| `index_skip_unknown_extensions` | `` true `` |  | Immediately | Skip files with no known syntax when indexing. | [velocity/intelligence.rs](../crates/velocity/src/intelligence.rs) |
| `index_workers` | `` 0 `` |  | Immediately | Set the number of index workers. Zero chooses automatically. | [velocity/intelligence.rs](../crates/velocity/src/intelligence.rs) |
| `inverse_caret_state` | `` false `` |  | Immediately | Reverse the visible and hidden phases of the caret blink. | Reverse the caret's visible blink phase. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `io_workers` | `` 0 `` |  | Next file open | Limit concurrent document reads and writes per window. Zero keeps standard scheduling. | [Build 4213](https://www.sublimetext.com/download#build-4213). Applies to regular document opens, reloads and writes submitted by a window. Zero keeps the previous scheduling. [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `large_file.checkpoint_interval` | `` 1024 `` |  | Immediately | Set the number of lines between cached syntax checkpoints. | [velocity/language.rs](../crates/velocity/src/language.rs) |
| `large_file.copy_bytes` | Not shipped |  |  |  | [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.diff_bytes` | `` 67108864 `` |  | Immediately | Disable inline diff markers at this file size in bytes. | [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.enable_diff` | Not shipped |  |  |  | [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.enable_highlighting` | Not shipped |  |  |  | [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.enable_indexing` | Not shipped |  |  |  | [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.enable_minimap` | Not shipped |  |  |  | [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.enable_spell_check` | Not shipped |  |  |  | [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.enable_word_wrap` | Not shipped |  |  |  | [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.indexing_bytes` | `` 67108864 `` |  | Immediately | Skip indexing files at or above this size in bytes. | [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.load_bytes` | `` 67108864 `` |  | Next file open | Use large-file loading for files at or above this size in bytes. | Huge documents: inclusive thresholds; explicit view feature commands override degradation. [velocity/workbench.rs](../crates/velocity/src/workbench.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `large_file.long_line_bytes` | `` 16384 `` |  | Immediately | Enable large-file safeguards when a line reaches this length in bytes. | [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.max_checkpoints` | `` 128 `` |  | Immediately | Limit how many syntax checkpoints are kept. | [velocity/language.rs](../crates/velocity/src/language.rs) |
| `large_file.max_line_bytes` | `` 16384 `` |  | Immediately | Limit how many bytes of a long line are syntax-highlighted. | [velocity/language.rs](../crates/velocity/src/language.rs) |
| `large_file.max_lookback_lines` | `` 256 `` |  | Immediately | Limit how far above the visible area syntax highlighting can scan. | [velocity/language.rs](../crates/velocity/src/language.rs) |
| `large_file.max_viewport_lines` | `` 512 `` |  | Immediately | Limit how many visible lines are prepared for syntax highlighting. | [velocity/language.rs](../crates/velocity/src/language.rs) |
| `large_file.minimap_lines` | `` 10000 `` |  | Immediately | Hide the minimap when the document reaches this many lines. | [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.spell_bytes` | `` 16777216 `` |  | Immediately | Disable spell checking at this file size in bytes. | [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.viewport_bytes` | `` 67108864 `` |  | Immediately | Limit highlighting to the visible area at this file size in bytes. | [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.viewport_lines` | `` 1000000 `` |  | Immediately | Limit highlighting to the visible area at this document line count. | [velocity/large.rs](../crates/velocity/src/large.rs) |
| `large_file.wrap_bytes` | `` 16777216 `` |  | Immediately | Disable automatic wrapping at this file size in bytes. | [velocity/large.rs](../crates/velocity/src/large.rs) |
| `light_color_scheme` | `` "Velocity Light.sublime-color-scheme" `` |  | Immediately | Use this scheme in light mode when Color Scheme is automatic. | [vel-color-scheme/lib.rs](../crates/vel-color-scheme/src/lib.rs), [velocity/settings_entry.rs](../crates/velocity/src/settings_entry.rs), [velocity/theme_ui.rs](../crates/velocity/src/theme_ui.rs) |
| `light_theme` | `` "Velocity Light.sublime-theme" `` |  | Immediately | Use this theme in light mode when Theme is automatic. | [velocity/settings_entry.rs](../crates/velocity/src/settings_entry.rs), [velocity/theme_ui.rs](../crates/velocity/src/theme_ui.rs) |
| `line_diff_width` | Not shipped |  |  |  | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `line_numbers` | `` true `` |  | Immediately | Show line numbers next to the text. | [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `line_padding_bottom` | `` 0 `` |  | Immediately | Add space below each line of editor text. | [velocity/view.rs](../crates/velocity/src/view.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `line_padding_top` | `` 0 `` |  | Immediately | Add space above each line of editor text. | [velocity/view.rs](../crates/velocity/src/view.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `margin` | `` 8 `` |  | Immediately | Set the space between the gutter and the text. | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `match_brackets` | `` true `` |  | Immediately | Highlight the bracket paired with the one at the caret. | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `match_brackets_angle` | `` false `` |  | Immediately | Include angle brackets in bracket highlighting. | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `match_brackets_braces` | `` true `` |  | Immediately | Include braces in bracket highlighting. | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `match_brackets_content` | `` true `` |  | Immediately | Highlight enclosing brackets when the caret is between them. | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `match_brackets_square` | `` true `` |  | Immediately | Include square brackets in bracket highlighting. | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `match_selection` | `` true `` |  | Immediately | Highlight other occurrences of the selected word. | [Visual settings](https://sublime-text-unofficial-documentation.readthedocs.io/en/latest/reference/settings.html#visual-settings) [velocity/view.rs](../crates/velocity/src/view.rs) |
| `match_tags` | `` true `` |  | Immediately | Highlight matching opening and closing markup tags. | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `mini_diff` | `` true `` |  | Immediately | Mark added, changed, and deleted lines in the gutter. | [velocity/annotation_commands.rs](../crates/velocity/src/annotation_commands.rs), [velocity/annotations.rs](../crates/velocity/src/annotations.rs) |
| `minimap_horizontal_scrolling` | `` false `` |  | Immediately | Scroll the minimap horizontally with the editor. | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `minimap_scroll_to_clicked_text` | `` false `` |  | Immediately | Keep clicked text under the pointer instead of centering it. | Keep clicked minimap text under the pointer instead of centering it. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `mouse_wheel_switches_tabs` | `` false `` |  | Immediately | Switch tabs by wheeling vertically over the tab bar. | Use a vertical wheel over tabs to switch tabs instead of scrolling the strip. [Global settings](https://docs.sublimetext.io/reference/settings.html#global-settings) [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme), [velocity/app_mouse.rs](../crates/velocity/src/app_mouse.rs) |
| `move_to_limit_on_up_down` | `` false `` |  | Immediately | Let Up and Down move to the start or end at the outermost rows. | Let Up and Down reach document limits from the first and last visual rows. [velocity/editing.rs](../crates/velocity/src/editing.rs) |
| `new_window_settings` | `` {} `` |  | Next window | Override preferences for windows created with New Window. | File > New Window snapshots this dictionary over User preferences. An edit here only affects the next New Window. Launch, session restore and Settings do not use it. [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `open_files_in_new_window` | `` "never" `` |  | Next file open | Choose when opening a file creates a new window. | File opens can target the current window, Finder/Dock only, or all sources. [velocity/app.rs](../crates/velocity/src/app.rs) |
| `open_tabs_after_current` | `` false `` |  | Next file open | Insert new tabs immediately after the focused tab instead of at the end. | Tabs, window chrome, display, sessions and external actions. [Build 4142](https://www.sublimetext.com/download#build-4142); default keeps appending (U05). [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `overlay_scroll_bars` | `` "system" `` |  | Immediately | Overlay the editor, reserve space, or follow macOS. | "system" follows macOS Show scroll bars; "enabled" overlays editor text and fades when idle, while "disabled" reserves space for an always-visible bar. [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `package_index_url` | `` null `` |  | Immediately | Choose the address used to look up packages. | Explicit HTTPS index; no registry, timer or network request is enabled by default. [velocity/app.rs](../crates/velocity/src/app.rs), [velocity/release.rs](../crates/velocity/src/release.rs) |
| `preview_on_click` | `` true `` |  | Immediately | Choose which clicks preview a file from the sidebar. | Preview a sidebar file on one click; false selects it, and "only_left" restricts previews to left clicks. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `regex_auto_escape` | `` true `` |  | Immediately | Escape regex characters when filling the Find field from a selection. | Escape selected buffer text when prefilling a regular-expression search. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `relative_line_numbers` | `` false `` |  | Immediately | Number lines by their distance from the caret. | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `reload_file_in_background` | `` true `` |  | Immediately | Reload unmodified inactive documents without waiting for activation. | Turn this off to wait until activation before reloading a clean inactive document. [Build 4200](https://www.sublimetext.com/download#build-4200) [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `reload_file_on_change` | `` true `` |  | Immediately | Reload unmodified documents when the file changes on disk. | Turn this off to ask before reloading even an unchanged buffer. [Build 4107](https://www.sublimetext.com/download) [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `remember_layout` | `` "auto" `` |  | Next close or quit | Restore pane splits with Hot Exit by default, or explicitly enable or disable remembering splits. | "auto" preserves restored splits when Hot Exit is enabled. Set true to also inherit the current split in new windows, or false to start unsplit. [Build 4121](https://www.sublimetext.com/download#build-4121); `auto` keeps saved splits unless Hot Exit is disabled. `true` also inherits splits in new windows without copying tabs; `false` restores in one group. [velocity/workbench.rs](../crates/velocity/src/workbench.rs) |
| `remember_open_files` | `` true `` |  | Next close or quit | Restore open files from the previous session. | [velocity/app.rs](../crates/velocity/src/app.rs) |
| `result_base_dir` | `` "" `` |  | Next build | Resolve relative file names in build results from this directory. | [velocity/build.rs](../crates/velocity/src/build.rs) |
| `result_file_regex` | `` "" `` |  | Next build | Match file names and locations in build output. | Fallback result matchers and relative path base for build output jumps. [Build system options](https://www.sublimetext.com/docs/build_systems.html#exec-target-options) [velocity/build.rs](../crates/velocity/src/build.rs) |
| `result_line_regex` | `` "" `` |  | Next build | Match line locations in multiline build output. | [velocity/build.rs](../crates/velocity/src/build.rs) |
| `ruler_style` | `` "solid" `` |  | Immediately | Choose the line pattern for column guides. | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `ruler_width` | `` 1.0 `` |  | Immediately | Set the thickness of column guides. | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `rulers` | `` [] `` |  | Immediately | Draw vertical guides at the specified columns. | [vel-commands/text_ops.rs](../crates/vel-commands/src/text_ops.rs), [vel-config/settings.rs](../crates/vel-config/src/settings.rs) |
| `save_all_on_build` | `` true `` |  | Next build | Save all modified named files before running a build. | Save dirty named files before invoking a build. [velocity/app_build.rs](../crates/velocity/src/app_build.rs) |
| `save_on_focus_lost` | `` false `` |  | Next save | Save modified files when the tab or window loses focus. | Save changed files when their tab or window loses focus. [velocity/app.rs](../crates/velocity/src/app.rs) |
| `scroll_context_lines` | `` 3 `` |  | Immediately | Keep this many lines of context when scrolling the caret into view. | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `scroll_past_end` | `` false `` |  | Immediately | Allow scrolling beyond the last line. | [Build 4107](https://www.sublimetext.com/download) [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `scroll_speed` | `` 1.0 `` | `` 1.0 `` | Immediately | Scale the distance moved by each scroll gesture. | Keyboard scroll animation speed; zero jumps immediately. Pointer scrolling stays direct. [velocity/view.rs](../crates/velocity/src/view.rs) |
| `select_across_groups` | `` false `` |  | Immediately | Allow selecting a single sidebar file to focus an existing tab in another group. | [Build 4126](https://www.sublimetext.com/download#build-4126) describes the old behavior as opt-in. When off, a matching file uses a shared view if a different group already has it. [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `selection_border_width` | Not shipped |  |  |  | [velocity/view.rs](../crates/velocity/src/view.rs) |
| `selection_description_column_type` | `` "display" `` |  | Immediately | Count tab-expanded columns, characters, UTF-16 units, or bytes in the status bar. | [Build 4075](https://www.sublimetext.com/dev#build-4075); default preserves one-based display columns. Velocity's values are `display`, `character`, `utf16` and `byte`. [velocity/status_position.rs](../crates/velocity/src/status_position.rs) |
| `set_unsaved_view_name` | `` true `` |  | Immediately | Use the first line of text to name untitled tabs. | [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `set_unsaved_view_name_for_syntax` | `` true `` |  | Immediately | Allow automatic first-line titles in unsaved buffers with a syntax other than plain text. | [Build 4200](https://www.sublimetext.com/download#build-4200). Opt out for syntax-backed buffers without changing plain text, explicit titles, or the master naming switch. [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `shift_tab_unindent` | `` false `` |  | Immediately | Let Shift+Tab unindent even outside leading whitespace. | shift+tab unindents everywhere instead of only in leading white space or across lines. [velocity/Default (Windows).sublime-keymap](../crates/velocity/resources/Packages/Default/Default%20%28Windows%29.sublime-keymap), [velocity/Velocity.sublime-keymap](../crates/velocity/resources/Packages/Default/Velocity.sublime-keymap) |
| `show_definitions` | `` true `` |  | Immediately | Show definitions or references when hovering over a symbol. | [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `show_encoding` | `` true `` |  | Immediately | Show the document’s encoding in the status bar. | Show the document's encoding and line-ending format in the status bar. [velocity/app_status.rs](../crates/velocity/src/app_status.rs), [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `show_errors_inline` | `` true `` |  | Immediately | Show build diagnostics next to the relevant source line. | [velocity/html_ui.rs](../crates/velocity/src/html_ui.rs) |
| `show_full_path` | `` false `` |  | Immediately | Include the file’s full path in the window title. | macOS keeps basename titles unless a path option is enabled. [velocity/workbench.rs](../crates/velocity/src/workbench.rs) |
| `show_git_status` | `` true `` |  | Immediately | Show Git status indicators in the sidebar. | [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `show_git_status_in_status_bar` | `` true `` |  | Immediately | Show the Git branch and status in the status bar. | [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `show_indentation` | `` true `` |  | Immediately | Show the document’s indentation style and tab width in the status bar. | [Build 4107](https://www.sublimetext.com/download#build-4107) [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `show_line_column` | `` "compact" `` |  | Immediately | Show the caret’s line and column in full, in brief, or not at all. | "enabled" spells out the position; "compact" retains today's abbreviated item; "disabled" hides it. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `show_line_endings` | `` true `` |  | Immediately | Show the document’s line ending format in the status bar. | [velocity/app_status.rs](../crates/velocity/src/app_status.rs), [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `show_minimap` | `` true `` |  | Immediately | Show a miniature overview of the document. | [velocity/app.rs](../crates/velocity/src/app.rs), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `show_panel_on_build` | `` true `` |  | Next build | Show the output panel when a build starts. | Reveal output when a build starts. [velocity/build.rs](../crates/velocity/src/build.rs) |
| `show_project_first` | `` false `` |  | Immediately | Put the project name before the file name in the window title. | [Build 4107](https://www.sublimetext.com/download#build-4107); default keeps the file name first. [velocity/workbench.rs](../crates/velocity/src/workbench.rs) |
| `show_rel_path` | `` false `` |  | Immediately | Include the path relative to the project in the window title. | [velocity/workbench.rs](../crates/velocity/src/workbench.rs) |
| `show_side_bar` | `` "auto" `` |  | Immediately | Auto shows the sidebar only while the window has folders. Set true or false in User settings to override this default. A window's own sidebar toggle takes precedence and survives session restore. | [velocity/app.rs](../crates/velocity/src/app.rs), [velocity/panels.rs](../crates/velocity/src/panels.rs), [velocity/sidebar_folders.rs](../crates/velocity/src/sidebar_folders.rs) |
| `show_sidebar_button` | `` true `` |  | Immediately | Show the sidebar and panel switcher in the status bar. | [Status bar theme elements](https://www.sublimetext.com/docs/themes.html#status-bar) [velocity/details.rs](../crates/velocity/src/panels/details.rs) |
| `show_status_bar` | `` true `` |  | Immediately | Show the status bar below the editor. | [velocity/app.rs](../crates/velocity/src/app.rs), [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `show_syntax` | `` true `` |  | Immediately | Show the document’s syntax or binary label in the status bar. | [Build 4107](https://www.sublimetext.com/download#build-4107) [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `show_tab_close_buttons` | `` true `` |  | Immediately | Show a close button on each tab. | [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme), [velocity/app.rs](../crates/velocity/src/app.rs) |
| `show_tab_close_buttons_on_left` | `` false `` |  | Immediately | Move close buttons and unsaved markers to the left. | Put the close button and dirty marker on the left when the button is enabled. [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme), [velocity/app.rs](../crates/velocity/src/app.rs) |
| `show_tabs` | `` true `` |  | Immediately | Show the document tab bar. | [velocity/app.rs](../crates/velocity/src/app.rs) |
| `sidebar_on_right` | `` false `` |  | Immediately | Place the sidebar on the right side of the window. | [Build 4200](https://www.sublimetext.com/download#build-4200); default keeps the sidebar on the left. [velocity/Velocity.sublime-theme](../crates/velocity/resources/Packages/Theme%20-%20Velocity/Velocity.sublime-theme), [velocity/app.rs](../crates/velocity/src/app.rs) |
| `smart_indent` | `` true `` |  | Immediately | Adjust indentation using the current syntax’s rules. | With auto_indent, the syntax's indentation rules add or remove a level. [velocity/editing.rs](../crates/velocity/src/editing.rs) |
| `spell_check` | `` false `` |  | Immediately | Check spelling in supported parts of a document. | [velocity/annotations.rs](../crates/velocity/src/annotations.rs), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `spelling_selector` | `` "markup.raw, source string.quoted - punctuation - meta.preprocessor.c.include, source comment - source comment.block.preprocessor, -(source, constant, keyword, storage, support, variable, markup.underline.link, meta.tag)" `` |  | Immediately | Choose the syntax scopes to check for spelling mistakes. | Check spelling in these syntax scopes. [Scope selector](https://sublime-text-unofficial-documentation.readthedocs.io/en/latest/reference/settings.html#system-and-miscellaneous-settings) [velocity/annotations.rs](../crates/velocity/src/annotations.rs) |
| `sub_word_separators` | `` "_" `` |  | Immediately | Set extra boundaries for navigation within a word. | Treat these characters as additional boundaries for sub-word commands. [vel-buffer/ops.rs](../crates/vel-buffer/src/ops.rs), [vel-commands/lib.rs](../crates/vel-commands/src/lib.rs), [vel-config/settings.rs](../crates/vel-config/src/settings.rs) |
| `sublime_merge_path` | `` "" `` |  | Immediately | Choose the location of the Sublime Merge application. | [velocity/review_ui.rs](../crates/velocity/src/review_ui.rs) |
| `syntax` | Not shipped |  |  |  | [velocity/workbench.rs](../crates/velocity/src/workbench.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `syntax_detection_size_limit` | `` 0 `` |  | Next file open | Skip automatic syntax detection above this many bytes. Zero removes the limit. | [Build 4113](https://www.sublimetext.com/download#build-4113). Oversize files default to Plain Text, unless a syntax is explicitly assigned. [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `tab_completion` | `` true `` |  | Immediately | Let Tab complete the word at the caret, not just exact snippet triggers. | Tab completes the word before the caret; false limits Tab to exact snippet triggers. [velocity/completion.rs](../crates/velocity/src/completion.rs) |
| `tab_size` | `` 4 `` |  | Immediately | Spaces represented by a tab. | [vel-commands/lib.rs](../crates/vel-commands/src/lib.rs), [vel-commands/text_ops.rs](../crates/vel-commands/src/text_ops.rs), [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/app_clipboard.rs](../crates/velocity/src/app_clipboard.rs), [velocity/app_print.rs](../crates/velocity/src/app_print.rs), [velocity/appearance.rs](../crates/velocity/src/appearance.rs), [velocity/completion.rs](../crates/velocity/src/completion.rs), [velocity/editing.rs](../crates/velocity/src/editing.rs), [velocity/editor.rs](../crates/velocity/src/editor.rs), [velocity/fold_state.rs](../crates/velocity/src/fold_state.rs), [velocity/language.rs](../crates/velocity/src/language.rs), [velocity/large.rs](../crates/velocity/src/large.rs), [velocity/details.rs](../crates/velocity/src/panels/details.rs), [velocity/panels.rs](../crates/velocity/src/panels.rs), [velocity/status_position.rs](../crates/velocity/src/status_position.rs), [velocity/view.rs](../crates/velocity/src/view.rs), [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `terminal_command` | `` [] `` |  | Immediately | Set an executable and arguments for Open Terminal. An empty list uses macOS Terminal. | Empty uses Terminal.app. Otherwise [executable, ...arguments]; $dir is substituted as one argument. Only global preferences can launch a command. [Build 4213](https://www.sublimetext.com/download#build-4213); global preference, read on each `open_terminal`. An empty list opens macOS Terminal. For a custom executable, use an argument list such as `["/path/to/terminal", "--directory", "$dir"]`; `$dir` is a literal directory argument, not shell input. The public changelog does not specify the configuration schema. [velocity/app_terminal.rs](../crates/velocity/src/app_terminal.rs) |
| `theme` | `` "auto" `` |  | Immediately | Choose an interface theme or follow the system appearance. | [velocity/theme_ui.rs](../crates/velocity/src/theme_ui.rs) |
| `themed_title_bar` | `` true `` |  | Immediately | Apply the selected theme to the window title bar. | [velocity/app.rs](../crates/velocity/src/app.rs) |
| `translate_tabs_to_spaces` | `` false `` |  | Immediately | Insert spaces when you press Tab. | [vel-commands/lib.rs](../crates/vel-commands/src/lib.rs), [vel-commands/text_ops.rs](../crates/vel-commands/src/text_ops.rs), [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/app.rs](../crates/velocity/src/app.rs), [velocity/completion.rs](../crates/velocity/src/completion.rs), [velocity/editing.rs](../crates/velocity/src/editing.rs), [velocity/large.rs](../crates/velocity/src/large.rs), [velocity/details.rs](../crates/velocity/src/panels/details.rs), [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `tree_animation_enabled` | `` true `` |  | Immediately | Animate changes in the sidebar’s file tree. | Animate folder rows unless Reduce Motion is enabled. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `trim_automatic_white_space` | `` true `` |  | Immediately | Clear auto-inserted indentation when leaving an empty line. | Pressing Enter on a line that only holds indentation leaves it empty. [velocity/editing.rs](../crates/velocity/src/editing.rs) |
| `trim_only_modified_white_space` | `` true `` |  | Next save | Trim whitespace only on lines changed since the last save. | [Build 4107](https://www.sublimetext.com/download) [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [vel-fileio/lib.rs](../crates/vel-fileio/src/lib.rs) |
| `trim_trailing_white_space_on_save` | `` false `` |  | Next save | Remove trailing whitespace when saving a file. | [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [vel-fileio/lib.rs](../crates/vel-fileio/src/lib.rs) |
| `ui_font_face` | `` "" `` |  | Immediately | Override the theme’s interface font family. | Empty uses the theme's UI font. A font family here overrides theme text only. [vel-ui/theme.rs](../crates/vel-ui/src/theme.rs) |
| `ui_font_size` | `` 12 `` |  | Immediately | Set the base font size for interface text. | [vel-ui/theme.rs](../crates/vel-ui/src/theme.rs) |
| `ui_language` | `` "auto" `` |  | Immediately | Choose a language or follow the first supported macOS preferred language. | "auto" follows the first supported macOS preferred language. Set a tag like "en" or "zh-Hans" to override it. Untranslated text falls back to English. [velocity/workspace.rs](../crates/velocity/src/workspace.rs) |
| `ui_scale` | `` 1.0 `` |  | Immediately | Scale the interface without changing the editor font. | [vel-ui/theme.rs](../crates/vel-ui/src/theme.rs) |
| `update_system_recent_files` | `` true `` |  | Next file open | Add opened files to the macOS recent documents list. | [Build 4143](https://www.sublimetext.com/download) [velocity/app.rs](../crates/velocity/src/app.rs) |
| `use_find_clipboard` | `` true `` |  | Immediately | Share the Find string on focus and when copying a selection to Find on macOS. | [Build 4121](https://www.sublimetext.com/download#build-4121) describes opting out of the macOS integration. Explicit patterns and single-line selection seeding take precedence over pasteboard text. [velocity/panels.rs](../crates/velocity/src/panels.rs) |
| `use_simple_full_screen` | `` false `` |  | Next full-screen transition | Go full screen without creating a separate macOS Space. | [velocity/app.rs](../crates/velocity/src/app.rs) |
| `use_tab_stops` | `` true `` |  | Immediately | When indenting with spaces, Tab and Backspace move to tab stops. | With spaces, Tab and Backspace go to the next / previous tab stop. [vel-commands/lib.rs](../crates/vel-commands/src/lib.rs), [vel-config/settings.rs](../crates/vel-config/src/settings.rs), [velocity/editing.rs](../crates/velocity/src/editing.rs) |
| `vintage_ctrl_keys` | `` null `` |  | Immediately | Use Vintage Ctrl shortcuts or leave them at the platform default. | Null uses the documented platform default (enabled on macOS). [Vintage Ctrl keys](https://www.sublimetext.com/docs/vintage.html#ctrl-keys) [velocity/vintage.rs](../crates/velocity/src/vintage.rs) |
| `vintage_enabled` | `` false `` |  | Immediately | Enable Vintage modal editing. | [velocity/Velocity Vintage.sublime-keymap](../crates/velocity/resources/Packages/Default/Velocity%20Vintage.sublime-keymap), [velocity/vintage.rs](../crates/velocity/src/vintage.rs) |
| `vintage_start_in_command_mode` | `` false `` |  | When a view enters Vintage | Enter command mode instead of insert mode when Vintage starts. | [velocity/vintage.rs](../crates/velocity/src/vintage.rs) |
| `vintage_use_clipboard` | `` false `` |  | Immediately | Use the system clipboard for the default Vintage register. | [velocity/vintage.rs](../crates/velocity/src/vintage.rs) |
| `word_separators` | `` "./\\()\"'-:,.;<>~!@#$%^&*\|+=[]{}`~?" `` |  | Immediately | Set word boundaries for selection and navigation. | Characters that end a word for double click, word motions and super+d. [vel-buffer/buffer.rs](../crates/vel-buffer/src/buffer.rs), [vel-buffer/ops.rs](../crates/vel-buffer/src/ops.rs), [vel-commands/lib.rs](../crates/vel-commands/src/lib.rs), [vel-config/settings.rs](../crates/vel-config/src/settings.rs) |
| `word_wrap` | `` "auto" `` |  | Immediately | Wrap long lines to fit the editor or a set column. | [velocity/large.rs](../crates/velocity/src/large.rs), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `wrap_width` | `` 0 `` |  | Immediately | Wrap at this column. Zero uses the available width. | [velocity/editor.rs](../crates/velocity/src/editor.rs), [velocity/view.rs](../crates/velocity/src/view.rs) |
| `wrap_width_style` | `` "auto" `` |  | Immediately | Let the wrap column follow the window or stay fixed. | [velocity/view.rs](../crates/velocity/src/view.rs) |
<!-- END GENERATED SETTINGS -->
