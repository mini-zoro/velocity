# Command coverage — rounds 13–17

Audit date: 2026-11-16. Sources are public documentation only; no installed Default
package, bundled Python, private assets or editor binaries were inspected.

The table is the union of the complete `Registry::standard()` inventory, literal
command names in all 39 pages linked from the official documentation index plus
its API reference, literal command mentions in the public ST4 changelog, and the
round-13 spec's requested operations. This is a reproducible bounded audit, not a
claim that the editor publishes an exhaustive ST4 command list. Names marked
“registry inventory” or “spec” have no literal occurrence in those scanned pages.
The API reference documents `run_command` and command domains but does not provide
a complete command catalog; API methods themselves are not counted as commands.

“implemented” means an executable body in vel-commands/vel-buffer, including registry
orchestration; it does not assert exact undocumented Sublime behavior. “partially”
means registration with a required host, or incomplete documented argument coverage.
“missing” means no callable registration (even when another crate supplies a core).
“out of scope” identifies plugin/tooling/Vintage responsibilities with a reason.
Each name counts once, even if it appears in multiple sources. Generic host forwarding
is deliberately counted as partial, not implemented. Application hosts may implement
more than this crate's standalone guarantee.

**Counts: 61 implemented, 123 partially, 12 missing, 3 out of scope; 199 total.**

Public sources:

- [Documentation index](https://www.sublimetext.com/docs/) (39 linked pages inspected).
- [API reference](https://www.sublimetext.com/docs/api_reference.html) (command domains and invocation contract).
- [Key bindings](https://www.sublimetext.com/docs/key_bindings.html), [menus](https://www.sublimetext.com/docs/menus.html), [settings](https://www.sublimetext.com/docs/settings.html), [build systems](https://www.sublimetext.com/docs/build_systems.html), [Vintage](https://www.sublimetext.com/docs/vintage.html).
- [ST4 changelog](https://www.sublimetext.com/download) and [4142 identifier conversion announcement](https://www.sublimetext.com/blog/articles/sublime-text-4142). The announcement confirms the feature; it does not publish conversion argument names or acronym rules.
- [Legacy official command catalog](https://www.sublimetext.com/docs/1/commands) supplies high-level wrap/permutation/selection/ROT13 semantics. Its camelCase ST1 names are **not** silently counted as literal ST4 snake_case names. Modern spellings requested by the spec are marked accordingly.

`title_case`, `swap_case`, `split_selection_into_lines`, `chain` and touched-line
whitespace trimming already had cores. This round adds missing adapters/edge tests;
chain retains the existing registry's per-child undo, macro/repeat handling, errors
and recursion limit. New text editing includes paragraph reflow, stable unique/reverse/
seeded shuffle on lines and selected strings, selection sorting, five identifier styles,
marks/bookmarks, paragraph expansion, append, selection complement/endpoints and ROT13.
All exact behavior choices lacking public evidence are `ST-BEHAVIOR: unverified`
in [BEHAVIOR.md](../vel-buffer/BEHAVIOR.md). `copy_as_html` and `print` are intentionally
reported as missing command integrations: the new export crate supplies their API,
and lane A wires browser/clipboard menus in the next round.

| Command | Status | Evidence | Coverage / remaining work |
| --- | --- | --- | --- |
| `add_folder` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `add_jump_record` | missing | changelog | No registration or executable adapter in vel-commands; window/application integration remains. |
| `add_word` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `append` | implemented | build_systems, changelog | EOF text insertion implemented; force/scroll-to-end are host presentation policy. |
| `auto_complete` | partially | changelog | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `build` | partially | key_bindings | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `camel_case` | implemented | spec; 4142 feature announcement (exact adapter schema unverified) | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `cancel_build` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `chain` | implemented | changelog | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `clear_bookmarks` | implemented | spec | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `clear_fields` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `clone_file` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `close` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `close_file` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `close_pane` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `close_project` | partially | changelog | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `close_selected_tabs` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `close_split` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `close_window` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `commit_completion` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `convert_ident_case` | implemented | spec; 4142 feature announcement (exact adapter schema unverified) | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `convert_syntax` | out of scope | key_bindings | Syntax-definition maintenance tool, outside text editing; no Python plugin runtime. |
| `copy` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `copy_as_html` | missing | spec | vel-export::copy_as_html returns HTML/plain flavours; clipboard host/menu wiring remains absent. |
| `copy_path` | missing | menus | No registration or executable adapter in vel-commands; window/application integration remains. |
| `current_result` | missing | changelog | No registration or executable adapter in vel-commands; window/application integration remains. |
| `cut` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `decrease_font_size` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `delete_word` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `detect_indentation` | partially | indentation | Registered host hook; Velocity reuses file-load detection and updates the view settings. |
| `diff_changes` | missing | key_bindings | No registration or executable adapter in vel-commands; window/application integration remains. |
| `drag_select` | partially | round-17 spec | Registered host hook; Velocity maps mouse coordinates and arguments to its selection gesture. |
| `duplicate_line` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `edit_project` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `exec` | partially | changelog | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `exit_insert_mode` | out of scope | vintage | Vintage command; owned by vel-vintage, not this registry. |
| `expand_selection` | partially | spec | Word, line, indentation and paragraph work; syntax scope/brackets require a host hook. |
| `expand_selection_to_paragraph` | implemented | spec | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `expand_tabs` | partially | indentation | Registered host hook; Velocity converts leading whitespace in one transaction; set_translate_tabs updates this view. |
| `find_all` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `find_all_under` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `find_in_files` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `find_next` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `find_prev` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `find_under` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `find_under_expand` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `find_under_expand_skip` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `find_under_prev` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `focus_group` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `focus_neighboring_group` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `focus_selected_tab` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `focus_side_bar` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `fold` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `fold_by_level` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `fold_tag_attributes` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `git_blame_file` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `git_file_history` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `git_folder_history` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `git_line_history` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `git_open_repository` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `goto_definition` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `goto_line` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `goto_reference` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `goto_symbol_in_project` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `hide_auto_complete` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `hide_panel` | partially | changelog | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `ignore_word` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `increase_font_size` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `indent` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `insert` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `insert_best_completion` | partially | registry inventory | Default body inserts indentation; language completion needs the host. |
| `insert_snippet` | partially | registry inventory | Inline expansion only by default; file resources and live fields require the host. |
| `invert_selection` | implemented | legacy invertSelection; adapter spelling unverified | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `join_lines` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `jump_back` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `jump_forward` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `kebab_case` | implemented | spec; 4142 feature announcement (exact adapter schema unverified) | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `left_delete` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `lower_case` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `move` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `move_to` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `move_to_group` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `move_to_neighboring_group` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `new_file` | partially | menus | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `new_pane` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `new_window` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `next_bookmark` | implemented | spec | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `next_field` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `next_misspelling` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `next_modification` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `next_result` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `next_view` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `noop` | implemented | changelog, key_bindings | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `open_dir` | partially | changelog | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `open_file` | partially | changelog | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `open_project` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `open_project_or_workspace` | missing | changelog | No registration or executable adapter in vel-commands; window/application integration remains. |
| `open_terminal` | missing | changelog | No registration or executable adapter in vel-commands; window/application integration remains. |
| `pascal_case` | implemented | spec; 4142 feature announcement (exact adapter schema unverified) | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `paste` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `permute_lines` | implemented | spec | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `permute_selection` | implemented | spec | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `prev_bookmark` | implemented | spec | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `prev_field` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `prev_misspelling` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `prev_modification` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `prev_result` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `prev_view` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `print` | missing | spec | vel-export::write_print_file returns a persistent HTML path; browser/menu wiring remains absent. |
| `prompt_open` | missing | menus | No registration or executable adapter in vel-commands; window/application integration remains. |
| `prompt_open_file` | partially | menus | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `prompt_open_folder` | missing | menus | No registration or executable adapter in vel-commands; window/application integration remains. |
| `prompt_open_project` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `prompt_save_as` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `quick_switch_project` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `recent_projects` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `redo` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `redo_or_repeat` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `reindent` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `replace_all` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `replace_completion_with_next_completion` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `replace_in_files` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `replace_misspelling` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `replace_next` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `reset_font_size` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `reverse_lines` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `revert_hunk` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `revert_modification` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `right_delete` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `run_macro` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `run_syntax_tests` | out of scope | changelog | Package-author tooling; vel-syntax provides the standalone syntax-tests CLI. |
| `save` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `save_all` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `save_project_as` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `scroll_lines` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `select_all` | implemented | key_bindings | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `select_all_tabs` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `select_bookmarks` | implemented | legacy allBookmarks; adapter spelling unverified | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `select_by_index` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `select_lines` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `select_single_tab` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `select_tab` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `select_tab_to_left` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `select_tab_to_right` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `select_to_mark` | implemented | spec | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `set_build_system` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `set_dictionary` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `set_encoding` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `set_file_type` | partially | changelog | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `set_layout` | partially | key_bindings, menus | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `set_line_endings` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `set_mark` | implemented | spec | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `set_setting` | partially | settings | Registered view-setting action; Velocity applies arbitrary JSON through its named dispatcher. |
| `show_at_center` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `show_build_systems` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `show_dictionary` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `show_indexing_status` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `show_input_panel` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `show_output_panel` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `show_overlay` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `show_panel` | partially | build_systems, changelog | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `single_selection` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `slurp_find_string` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `slurp_replace_string` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `snake_case` | implemented | spec; 4142 feature announcement (exact adapter schema unverified) | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `soft_redo` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `soft_undo` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `sort_lines` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `sort_selection` | implemented | spec | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `split_selection` | implemented | legacy splitSelection; adapter spelling unverified | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `split_selection_into_lines` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `swap_case` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `swap_line_down` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `swap_line_up` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `swap_with_mark` | implemented | spec | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `switch_file` | missing | changelog | No registration or executable adapter in vel-commands; window/application integration remains. |
| `title_case` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `toggle_bookmark` | implemented | spec | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `toggle_comment` | partially | changelog | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `toggle_distraction_free` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `toggle_find_option` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `toggle_full_screen` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `toggle_inline_diff` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `toggle_minimap` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `toggle_record_macro` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `toggle_setting` | partially | settings | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `toggle_side_bar` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `transpose` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `trim_trailing_white_space` | implemented | spec | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `trim_trailing_whitespace` | implemented | spec | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `undo` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `unexpand_tabs` | partially | indentation | Registered host hook; Velocity converts leading whitespace in one transaction; set_translate_tabs updates this view. |
| `unfold` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `unfold_all` | partially | registry inventory | Registered host action/hook; this crate cannot execute it without an implementing host. |
| `unindent` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `unselect_current` | missing | changelog | No registration or executable adapter in vel-commands; window/application integration remains. |
| `upper_case` | implemented | registry inventory | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `upper_snake_case` | implemented | spec; 4142 feature announcement (exact adapter schema unverified) | Executable text operation or registry orchestration; headless tests cover its implementation. |
| `wrap_lines` | implemented | changelog | Executable text operation or registry orchestration; headless tests cover its implementation. |

New adapter examples (Velocity's documented schema; undocumented ST argument details
remain unverified):

```json
[
  {"command":"wrap_lines","args":{"width":80}},
  {"command":"permute_lines","args":{"operation":"unique"}},
  {"command":"permute_selection","args":{"operation":"shuffle","seed":42}},
  {"command":"sort_selection","args":{"case_sensitive":false,"reverse":false}},
  {"command":"convert_ident_case","args":{"case":"camelCase"}},
  {"command":"expand_selection","args":{"to":"paragraph"}},
  {"command":"next_bookmark","args":{"extend":true}},
  {"command":"chain","args":{"commands":[["select_all"],["upper_case"]]}}
]
```

Identifier formats accept `snake_case`, `camelCase`, `PascalCase`, `kebab-case`,
`UPPER_SNAKE`; `to` aliases `case`. Fixed-style aliases are listed in the table.
Shuffle accepts an optional unsigned 64-bit seed; without one the adapter chooses
an invocation seed. `wrap_lines` uses the first configured ruler, then 70 columns,
when width/column is absent. Bookmark navigation also accepts `select` as an alias
for `extend`. Invalid permutation/style names and nonpositive wrap widths return
errors before editing. Case conversion at an empty whitespace/punctuation caret
leaves text intact.
