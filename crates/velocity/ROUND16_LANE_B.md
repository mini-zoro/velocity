# Round 16 Lane B review

Baseline: current main/lane-b `99d1bb4` after the reviewer rebase. Changes remain uncommitted.
No GUI, packaging, `cargo clean`, or writes below `target/package-macos/`.
Edits in `editing.rs` remain inside the fold functions/helpers. The review required small
integration hooks in Lane A's `Editor::sync` (retain guides on language reset),
`Workspace::snapshot/prepare_sheet_inner`, `Tab::install`, `Workspace::reopen_last_file`
(live per-view folds and explicit cache invalidation), `Configuration::load`
(application-only ignored packages), and application menu construction (published macro catalog).
This third review additionally adds the buffer's explicit undo marker/glue API and an optional
VerticalLayout synchronization hook, plus the requested Editor boundary interception. The command
registry, text operations, panels and session serialization remain unchanged by these fixes.
The tests cover their existing selection, history, shared-view and resource paths.

| Spec | Implementation (file:function) | Headless evidence |
| --- | --- | --- |
| 5.1 | `editing.rs:fold_navigation_command`, `annotations.rs:committed`, `fold_state.rs:FoldStates`, `view.rs:pointer`: reveal hidden selection endpoints, jump markers, expose selected/deleted hidden bytes | `fold_navigation_reveals_endpoints_steps_over_markers_and_edits_visible_text`, `default_character_motions_and_boundary_deletion_do_not_edit_hidden_bytes`, marker mouse test (actual glyph, both caret boundaries, hit testing and IME geometry at wrap widths 0/1/80) |
| 5.2 | `editing.rs:set_folded`, `layout.rs:Layout::configure/change/visual_at/column/offset`, `vel-render/src/fold.rs:outermost`: retain the forest; project outer intervals | `nested_folds_survive_outer_unfold_in_wrapped_and_unwrapped_layouts`, pure projection table |
| 5.3 | `editing.rs:fold_command`, Lane B keymap block: replace level; zero/unfold-all; levels 1–9 and tag attributes | `fold_levels_replace_the_forest_and_zero_unfolds_without_analysis`, menu/palette resolution |
| 5.4 | `language.rs:add_indent_fold/analyze`: finish at the last nonblank line body; avoid duplicate syntax/indentation headers | `indentation_folds_leave_trailing_blank_lines_and_final_newline_visible` (including EOF); `fold_style` left unchanged, see below |
| 6.1 | `vel-render/src/guides.rs:columns/GuideIndex::new`, `language.rs:analyze`, `view.rs:scene`: zero-origin columns, stop before text, continue across blanks | guide table, `lazy_guide_storage_preserves_leading_and_intervening_blank_runs`, and `tree_matches_a_linear_block_model` property test |
| 6.2 | `GuideIndex::active`, `view.rs:paint_guide`, `scene.rs:SolidRect::stippled_guide`, `gpu.rs` and `quads.wgsl`: containing block only; one primitive for stippled strokes | guide geometry test, structural scene fixture, device-free shader validation |
| 6.3 | `Preferences.sublime-settings`: sourced visual defaults | `verified_visual_defaults_and_whitespace_array_and_string_forms` |
| 6.4 | `view.rs:selected_word/word_occurrences/scene/paint_long_line`: exact whole words, visible rows/windows, immediate cancellation on edit, one forward geometry walk and one outline primitive per match | `occurrence_outlines_are_whole_words_visible_and_cancel_immediately_on_edit`; paired CPU probe |
| 7.1 | `vintage.rs:vintage_key/vintage_text/vintage_command`, `vel-vintage/src/insert.rs:observe_insert/replay_insert`: normal host insertion, record committed deltas for counts/dot/undo | `vintage_insert_routes_ordinary_commands_and_records_host_changes_for_dot_and_counts`, `vintage_insert_pair_deletion_arrows_and_indent_match_ordinary_editing`, existing IME/multicaret tests; popup navigation/acceptance and committed completion undo/dot replay |
| 7.2 | `vel-vintage/src/lib.rs:handle_key`: external selections become visual operator ranges | external d/c/y tests, including reversed exclusive selections |
| 7.3 | `vintage.rs:configure_vintage`, preferences: ignored packages plus legacy enable alias; platform Ctrl default | defaults test and existing live enable/disable test |
| 7.4 | `vel-vintage/src/lib.rs:process`, `motion.rs:motion`, `vintage.rs:decode`: Ctrl+E/Y host scrolling, Alt+W/B subwords | `vintage_external_selections_are_visual_and_documented_keys_reach_the_host` |
| 8.1 | `annotations.rs:SPELLING_SELECTOR/Config::new`, preferences: documented narrower scopes | `spelling_selector_checks_prose_comments_and_strings_but_skips_markup_and_embedded_code` |
| 8.2 | `review_ui.rs:review_action`, `annotations.rs:prepare_navigation`: explicit navigation loads a dictionary without changing spell_check or viewport diagnostics | `spelling_navigation_does_not_enable_spell_check`; existing navigation tests |
| 8.3 | `vel-spell/src/lib.rs:system_spelling_directory/system_dictionaries/from_system_directory`, `review_ui.rs:review_action`: list/load macOS Hunspell pairs with package dictionaries | `system_dictionary_discovery_loading_and_directory_boundary` |
| 9 | `Main.sublime-menu`, `Velocity.sublime-commands`, keymap: Edit/Selection/Find/Goto/Tools and matching palette rows; original captions | `menus_and_palette_have_registered_commands_and_matching_arguments`, `every_menu_and_palette_command_resolves_through_app_dispatch` |
| Bookmarks | `view.rs:visible_bookmarks/bookmarked_line/scene`: borrowed `Editor::bookmarks()` slice, binary viewport lookup and one symbol per touched line | `fold_marker_mouse_unfolds_and_bookmarks_have_a_gutter_symbol` |

Behavior sources are cited beside the implementation/settings, using public documentation only:

- Public settings reference: <https://sublime-text-unofficial-documentation.readthedocs.io/en/latest/reference/settings.html>.
- Official Vintage documentation: <https://www.sublimetext.com/docs/vintage.html>.
- Official changelog builds 4143 (fold endpoints), 4151 (fold_style existence), 4107 (system dictionaries): <https://www.sublimetext.com/download>.
- Official release notes confirm disabled active guides: <https://www.sublimetext.com/updates/2/stable/release_notes.html>.

Defaults changed: draw_white_space `"none"` → `["selection"]`; highlight_line true → false;
indent_guide_options removes draw_active; match_selection becomes true; spelling_selector becomes
the documented narrower selector; vintage_ctrl_keys becomes true on macOS (false elsewhere).
Vintage remains off by default, now also expressed by ignored_packages `["Vintage"]`.

The public changelog confirms fold_style exists but does not document its values. Its behavior
is unchanged. Public spelling documentation confirms the navigation command names, but I could
not confirm the default Ctrl+F6 / Ctrl+Shift+F6 shortcuts: the existing bindings are unchanged.
Tools → Macros now consumes Lane A's published `macro_file_items` catalog. Menu construction
performs no filesystem reads; every menu/palette command resolves through the real app dispatcher.
Cmd+K Cmd+0 retains the existing `unfold_all` binding; duplicate level-0 and level-1 rows were removed.
The primary MarkdownEditing usage documentation cited in the keymap confirms the unfold-all chord.
Public fold documentation does not specify inserting at a fold's end. The reviewer fallback is
implemented: editing hidden interiors or an endpoint projected on the header unfolds the affected
fold first. Visible brace/tag closing lines and manual whole-line endpoints stay collapsed.

The native checklist is at the top of `MANUAL_TEST.md`; no native row has been marked passed.

Reviewer fixes and user-path regression tests (all names are searchable in `src/round16_review*_tests.rs`
or `vel-vintage/src/review_tests.rs`; explicit timing cases are in `src/round16_review_perf.rs`):

| Finding | Implementation | Headless test |
| --- | --- | --- |
| 1 | `vel-vintage/src/insert.rs:prepare_host_command/observe_insert/invalidate_insert`, `lib.rs:history`: classify host edits, invalidate foreign history, clamp saved endpoints while retaining direction/primary/goals | `vintage_host_undo_redo_and_external_commands_do_not_become_insert_steps`, `vintage_individual_insert_events_survive_host_history_commands`, `history_clamps_stale_saved_endpoints_to_valid_scalar_boundaries` |
| 2 | `layout.rs:Layout::offset`: folded end only on the same visual row; retain main's wrapped-segment boundary rule | `fold_header_clicks_and_vertical_motion_cross_braces_tags_and_indentation` (0/7/12/80) |
| 3 | `editing.rs:set_folded/fold_navigation_command`, `fold_state.rs:FoldStates::update`: ordered merge walks | `many_folded_regions_and_carets_delete_and_insert_without_revealing_unrelated_folds`, `five_thousand_folds_and_carets_have_a_small_constant_edit_cost` |
| 4 | `vel-vintage/src/insert.rs:observe_insert/left_points/compose_pair`: ordered selections/change ownership, flat shared payload, changed slices only | `ten_thousand_caret_vintage_insert_tracks_normal_editing_cost`, `aggregate_host_insert_maps_preserve_unicode_payloads_and_dot_without_a_document_diff`, `host_completion_tracks_distinct_unicode_payloads_at_each_caret_for_undo_and_dot` |
| 5 | `editing.rs:fold_navigation_command`: reveal edits inside folds and at hidden indentation endpoints | `typing_after_fold_markers_reveals_only_hidden_indentation_endpoints` |
| 6 | `view.rs:visible_bookmarks/bookmarked_line`: binary search borrowed sorted regions; no per-frame bookmark allocation | `bookmark_gutter_borrows_visible_regions_and_follows_mapped_edits`, `two_hundred_thousand_bookmarks_do_not_scan_the_document_on_paint` |
| 7a | resources and `app.rs` real command enable/dispatch path, no unresolved-command allowlist | `every_menu_and_palette_command_resolves_through_app_dispatch`, `menus_and_palette_have_registered_commands_and_matching_arguments` |
| 7b | `app.rs:application_menu/native_menu`: use the published macro catalog | `tools_macros_use_the_published_catalog_and_replay_its_invocation` |
| 7c | keymap: one documented Cmd+K Cmd+0 and one Cmd+K Cmd+1 | `folded_keymap_chords_are_unique_and_resolve_to_the_documented_commands` |
| 8a | `editing.rs:folded_view_state/restore_folded_view_state`, `workbench.rs:snapshot/prepare_sheet_inner`, `workspace.rs:Tab::install/reopen_last_file`, `fold_state.rs:update`: persist the live forest and explicitly invalidate restored views; unchanged frames stay O(1) | `nested_revealed_folds_round_trip_sessions_and_same_identity_restore_invalidates_cache` (JSON, native history, lazy cloned sheets) |
| 8b | `vel-spell/src/lib.rs:system_dictionaries`, `review_ui.rs:review_action`: skip optional directory/entry failures | `optional_system_directory_skips_missing_broken_and_non_directory_entries`, `optional_system_dictionary_errors_leave_the_picker_usable` |
| 8c | `workspace.rs:Configuration::load`, `vintage.rs:configure_vintage`: Preferences layers determine ignored packages | `project_ignored_packages_cannot_enable_or_disable_vintage_or_package_resources` |
| 8d | `vel-vintage/src/lib.rs:handle_key`: reject unsupported Alt keys before recording/pending-command mutation | `passthrough_alt_keys_leave_pending_commands_and_recorded_macros_unchanged` |
| 8e | `guide_state.rs:Guides`, `language.rs:change/poll/inherit_guides`, `editor.rs:sync`, `view.rs:scene`: keep and rebase the published index through typing, newline edits, undo/redo and batched shared edits | `guides_keep_the_published_blank_and_active_block_geometry_during_typing_and_row_edits` |

Second review fixes (the earlier review's accepted behavior and tests remain):

| Finding | Implementation (file:function) | Real-path headless test |
| --- | --- | --- |
| 1. Visual host-redo crash | `vel-vintage/src/lib.rs:handle_key/feed/apply_motions/apply/pending_key`: rebuild external selections, cancel pending state, and clamp visual/mark/motion/restored offsets before use | `visual_host_redo_refreshes_unicode_endpoints_before_the_next_motion`; `randomized_unicode_vintage_keys_and_host_history_keep_valid_selections` (seed `0x906d16b247a58921`, 2,500 events) |
| 2. Wrapped EOL reveals | `layout.rs:folded_segment_end`, `view.rs:visual_line_boundary`: shared binary-search endpoint rule; use the projected physical row (including hidden indentation endpoints), preserve a separate closing row and the non-final wrap boundary | `wrapped_header_home_end_and_command_arrows_preserve_fold_visibility` (Rust/HTML/Python/plain, widths 0/7/12/80) |
| 3. Host undo per key | `vel-vintage/src/insert.rs:observe_insert`, `lib.rs:enter_insert/history`: retain native word grouping, break only at session boundaries, count transactions restored by each native group | `vintage_preserves_native_word_undo_and_groups_a_complete_vi_insert_session`; existing host undo/redo regressions |
| 4. Visible fold-end edit | `editing.rs:fold_navigation_command`: only an empty projected suffix auto-reveals at insertion; visible closing lines and following whole-line-fold rows stay folded | `typing_at_visible_closing_lines_keeps_folds_but_hidden_indentation_ends_reveal` (typing/Enter, app undo/redo, widths 0/12/80) |
| 5. Dot after host history | `vel-vintage/src/insert.rs:history_insert_entry/finish_host_insert`: keep entry position, record net text once, cancel a consumed count; retire unfaithful sessions | `dot_after_host_undo_keeps_append_entry_and_drops_unreplayable_sessions`; `host_undo_repeats_surviving_counted_append_once_and_retires_complex_entries`; `dot_replays_surviving_net_text_after_host_history_without_further_typing` |
| 6. Recording memory | `vel-vintage/src/insert.rs:Session/compose_pair`, `vintage.rs:State::engine`: boxed cold state, one ordered net delta and shared initial rope, instead of a per-key caret record; at most 32 disjoint spans per original caret | `ten_thousand_caret_insert_recording_stays_bounded_through_a_long_session`; `disjoint_insert_locations_bound_metadata_and_cannot_replay_an_older_change` |

All new user paths use `App::dispatch_key_binding`, `Editor::text_input` and the real host
undo/redo commands. The three stress timing tests stay explicitly runnable and ignored during
parallel workspace testing. Vintage's bound is now **3× ordinary editing**, with no fixed time
allowance. Replay metadata is bounded even when carets move between disconnected edits.
Sessions with unrelated host edits, expired maps, an incompatible caret shape, more than 32
disjoint spans per caret, or history across a change/open-line entry cannot be replayed
faithfully; dot does nothing. Text editing and undo/redo remain available. Ordinary contiguous
insert/count/completion/macro replay retains its tests.

The second review added no Lane A core changes and changed no defaults. The earlier Lane A integration
hooks listed above are retained. Undo after an automatic reveal still restores text without
restoring the fold. Native checks remain pending in `MANUAL_TEST.md`.

Third review fixes (all key-driven tests are in `src/round16_review_app_tests.rs`):

| Finding | Fix (file:function) | Headless test |
| --- | --- | --- |
| Visual J with mixed regions | `vel-vintage/src/lib.rs:handle_key/render_visual/join`: publish the inclusive visual selection, expand empty carets and clamp the final row | `visual_join_accepts_a_drag_and_a_line_start_caret`; `every_visual_operator_accepts_mixed_host_regions` |
| Unwrapped folded BOL | `editor.rs:intercept_text_command`, `view.rs:visual_line_boundary`: use projected geometry for wrapped or folded layouts | `wrapped_header_home_end_and_command_arrows_preserve_fold_visibility` (all four widths, marker endpoints, Shift); `nested_unwrapped_indent_markers_keep_command_home_and_shift_home_projected` |
| Completed insert undo | `vel-buffer/src/buffer.rs:mark_undo_group/glue_undo_groups`, `vel-vintage/src/lib.rs:feed/escape`, `insert.rs:observe_insert/prepare_host_command`: retain native word undo during Insert, glue surviving groups at Escape | `completed_insert_glues_words_newlines_backspace_and_ime_for_both_undo_paths`; `insert_host_undo_stays_per_word_until_escape_then_glues_the_surviving_session` |
| Stale macro/event geometry | `vel-buffer/src/layout.rs:VerticalLayout::synchronize`, `layout.rs:MotionLayout/Layout queries`, `vintage.rs:feed_vintage`, `view.rs:motion_layout_options/synchronize_layout`: refresh only for changed revision/options before internal keys; clamp stale query coordinates | `vi_macros_refresh_layout_between_edit_and_vertical_or_block_motion`; `projected_boundaries_refresh_after_edits_and_wrap_changes_without_a_frame`; `layout_queries_clamp_stale_rows_and_unicode_offsets_after_real_edits` |

Native Insert editing is documented at https://www.sublimetext.com/docs/vintage.html#whats-not.
The public documentation and changelog did not confirm the precise mark/glue behavior. Completed
session gluing is our behavior matching vi semantics, as requested. No defaults change in this pass.
External transforms still invalidate replay/history without absorbing unrelated edits; auto-unfold
undo still restores text without restoring a fold. Existing regression tests cover those paths.

Final validation passes on the frozen third-review source: fmt; strict workspace/all-targets
release clippy with `--locked -- -D warnings`; `cargo test --workspace --locked --release`
(default parallelism: 828 passed, 9 ignored); and
`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked`.
The three owned ignored stress tests and both ignored feature probes were explicitly run with
no concurrent compilation. Native FSEvents and Lane A's existing manual timings remain ignored.

Nine alternating pairs compare the final CLI and common-source feature probes against frozen
main `99d1bb4`. The common probe source is byte-identical to the baseline copy. All measured
candidate hashes match the gate binaries and final sources. CLI wall-time pass: **True**.
The spec permits retired instructions on a busy host; all CLI and feature-probe instruction
quantiles must remain within 5%. Times, process CPU/RSS and every raw pair are retained, including
any scheduling outliers. This pass changes no defaults.

| CLI metric | p50 | p95 | p99 |
| --- | ---: | ---: | ---: |
| features/highlight_minimap_scroll_cpu/stats_ms | -0.46% | +0.17% | -2.01% |
| features/highlighted_typing_cpu/stats_ms | -26.90% | +0.99% | +0.21% |
| features/highlighted_typing_cpu/edit_stats_ms | +4.16% | +1.16% | -2.47% |
| features/highlighted_typing_cpu/scene_stats_ms | -4.14% | +0.99% | +0.66% |
| typing1/headless_summary/edit_and_scene_cpu_ms | +0.89% | +1.02% | -0.16% |
| typing1000/headless_summary/edit_and_scene_cpu_ms | -1.98% | -2.79% | +0.25% |

| Feature scene | Time p50 / p95 / p99 | Instructions p50 / p95 / p99 |
| --- | --- | --- |
| small: folded_occurrence_scroll | -4.64% / -3.97% / -4.76% | -2.70% / -2.46% / -2.36% |
| small: folded_scroll | +3.36% / +3.35% / +3.57% | +2.47% / +2.15% / +2.15% |
| small: occurrence_scroll | +3.67% / +3.44% / +3.30% | +2.81% / +2.44% / +2.45% |
| small: plain_scroll | +3.70% / +3.34% / +3.72% | +2.91% / +2.61% / +2.26% |
| small: typing_churn | +0.36% / +0.34% / -1.02% | -0.62% / -1.56% / -14.85% |
| 10 MB: real_scroll | -6.62% / -5.42% / -4.90% | -5.47% / -8.16% / -10.35% |

| CLI process | Instructions | CPU time | Peak RSS |
| --- | ---: | ---: | --- |
| features | +0.34% | +1.15% | 753.11 → 761.48 MiB (+1.11%) |
| typing1 | +0.33% | +0.22% | 16.44 → 16.64 MiB (+1.24%) |
| typing1000 | +0.09% | -0.65% | 29.92 → 29.83 MiB (-0.31%) |

| Explicit stress test | Plain ms | Feature ms | Ratio |
| --- | ---: | ---: | ---: |
| folds | 1.5708 | 3.0587 | 1.947× |
| vintage | 0.8232 | 1.0439 | 1.268× |
| bookmarks | 0.0160 | 0.0178 | 1.115× |

The 10,000-caret Vintage bound remains 3× ordinary editing, without a fixed margin.
Retained replay metadata after 10 and 100 keys: [480096, 480096] bytes.
Whole-process peak RSS for that test is 274.56 MiB, including host
history and text storage; it is not a measure of replay metadata alone.

Seven idle pairs (steady 0.5–2.5 s window): 0.711% → 0.702% of one CPU core; peak RSS 32.22 → 32.69 MiB.

Raw evidence and reproduction scripts: `/tmp/velocity-r16-review3`. Structured results, raw counters, exact commands,
binary/source hashes and prior accepted review evidence are in the round 16 review record (kept out of the public repository).
Native display/IME/drag interactions, event-to-present latency, GPU/scanout and FSEvents remain
for the reviewer; see the unchecked third-review rows in `MANUAL_TEST.md`.
