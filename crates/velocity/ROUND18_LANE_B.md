# Round 18 Lane B third-verifier fixes

Continued uncommitted on `ac6f530`, preserving the existing preference-test
synchronization fix. No reset, amend, commit, GUI, packaging, `cargo clean`, or
`target/package-macos/` operation was performed. Evidence and start-of-turn
backups: `/tmp/velocity-lane-b-r18-third/`.

## Findings and user-path tests

| Finding | Fix (`file:function`) | Headless regression |
| --- | --- | --- |
| N3 growing spill / delayed completion | `vel-build/output.rs: channel/State::trim/State::next`: replace the disk spool with a bounded RAM tail (8 MiB, trim to 6 MiB at LF). No spill file exists; disk usage is zero. Count results while accepting output, publish at most 256 new diagnostics per UI poll, retain bounded result metadata, and move the completed tail/results as one batch. `build.rs: Builds::poll` consumes the worker totals and maps retained diagnostics to current panel rows. | `round18_third_burst_is_bounded_and_publishes_tail_after_exit` (400 MiB, bounded text and zero spill bytes, tail before Finished, retained diagnostic row); `round18_final_output_spill_and_line_boundary_retention` (24 MB, all 24,000 errors counted, first retained diagnostic on row 1); `round18_third_diagnostics_arrive_before_completion_without_replay` (40,000 diagnostics while producer is paused); existing 240,000-result timing |
| N3 cancellation priority | `output.rs: State::next`: cancellation discards queued text before delivery, publishes final counts then Cancelled, including an already-exited producer. Process-tree cancellation remains unchanged. | `round18_third_cancel_discards_burst_backlog_immediately`; `round18_third_cancel_after_producer_exit_discards_retained_tail`; existing process-group, continuous-daemon, restart and early-close tests |
| N3 under-limit tail | Under 8 MiB no transport data is dropped. Oversized output notices report dropped bytes. UI retention also stays at 8→6 MiB and whole-line boundaries. | `round18_verify_stalled_build_poll_preserves_tail_before_finished` (600/1200/2000 ms at 1.07 MB, 3000 ms at 7.87 MB, exact bytes and no truncation); `round18_final_spill_is_bounded_lossless_and_ordered` (stream order retained); oversized-line continuation test |
| N1 edit + geometry | `view.rs: apply_style/capture_before_edit`: use the existing edit's old text, old options/folds, old caret and old scroll for capture and visibility. With no folds, old physical row bounds reject a plainly offscreen caret before reconstructing a layout. Reconstruct via public Layout queries only on the rare edit-plus-reflow path, then transform once. | `round18_third_remote_edit_and_geometry_use_pre_edit_coordinates` (9,990→10,090 lines/gutter, sidebar, resize, global font zoom, wrapped/unwrapped, visible/hidden caret); `round18_third_offscreen_reflow_uses_old_row_bounds` (far-offscreen caret, both wrap modes, zero old-layout captures); retained zero-expensive-anchor-work typing/wheel test |
| N2 swallowed viewport + undo | `view.rs: transform_text_anchor/project_text_anchor`: collapsed points stick left; clamping replaces the canonical position. Use indexed change lookup and the newest-edit fast path. Boundary anchors need no snapshots; eight bounded, view-local persistent before/after snapshots recover the exact swallowed bookmark across undo/redo (which has buffer transforms but no shared layout-edit snapshot). A reveal, user wheel or document change supersedes those saved bookmarks. | `round18_third_undo_swallowed_viewport_restores_exact_reading_position` (delete line 10→EOF and select-all/delete, both wrap modes, immediate undo or resize first, later resize and repeated redo/undo; original top ±1 line); `round18_third_deletion_bookmarks_do_not_capture_without_reflow` (real delete/undo in a clone, zero capture/visibility queries) |
| Debug output test | Parsing/counting moves off UI replay; ANSI-free lines avoid a copy, and no-regex builds skip line preprocessing. The existing 24 MB test stays unscaled. | `round18_final_output_spill_and_line_boundary_retention` in both debug and release |
| Probe hygiene | `environment.rs: inherited_environment`: test builds default to a private HOME/SHELL/ZDOTDIR and strip startup hooks. A dev-only `test-login` feature covers integration/app copies of the library as well as unit tests; explicit login tests validate their isolated child fixture. Unit capture commands also use a fixture environment. Production build environment inheritance is retained. | `round18_third_task_defaults_cannot_capture_real_login_profiles`; `round18_third_default_build_capture_uses_fixture_home_and_shell`; all existing shell, timeout, cache and restart tests |

Transport result records retain at most 32,768 diagnostics and 16 MiB of
message/path/record bytes, plus the 8 MiB text tail and a 1 MiB partial-line
scratch buffer. Total error counts include discarded output. The UI does no
pipe I/O, spill I/O or regex parsing. Bookmark saves reuse only visibility already
captured for a reflow; ordinary deletions never reconstruct the old layout. Pending small output remains incremental;
completion and Cancel never replay the discarded prefix. The native panel/editor
rendering checklist still needs desktop review.

## Final checks

Release/locked workspace, default parallelism: **1,096 passed, zero failed** (114 suites; 25 existing ignored tests). Debug: **22 build-crate tests**, **62 round-18 App tests**, and the workbench build-result test pass, using default parallelism. The 24 MB/24,000-error test is unscaled in debug. Fmt, strict release clippy, strict rustdoc, and all **24 ignored headless timings** pass. Logs: `gate-bounded-final.log`, `gate-*.log`, `gate-summary.json`, `test-counts.json`, and `ignored-status.json`.

Final timing evidence (headless):

```text
ROUND18_BUILD_OUTPUT polls=8 p50_ms=0.128 p95_ms=4.179 max_ms=4.179 retained_bytes=7546030 diagnostics=32768
R18_QUIET_BUILD cpu_ms=14.338
round18 project walk cold-index OS-cache-warm files=60001 baseline_median_ms=117.922 candidate_median_ms=114.325 delta_pct=-3.051
```

The focused 400 MiB probe delivered the tail/Finished in 7.48 ms after the producer exit marker; burst cancellation took 0.103 ms. Tests enforce <500 ms completion and <100 ms cancellation in both profiles; logs are `third-final-focused.log`, `third-lazy-final.log`, and the release/debug gate logs. These are CPU/poll-loop measurements; native presentation remains on the manual checklist.

Nine alternating pairs of the supplied headless scenarios versus the verified main `385dcdd` binary: features **+0.27%**, typing1 **+0.36%**, typing1000 **-0.78%**. Retired instructions pass the +0.5% single-caret and +5% other limits. The raw wall-time report is retained separately; the accepted gate uses retired instructions on the shared host. Both binaries used the same temporary HOME/SHELL, and the production candidate has no `test-login` feature. Evidence: `paired-bounded/instructions.json`, `paired-bounded/report.json`, `production-features.json`, and `production-build.jsonl`.

No preference defaults, build selection, navigation dispatch, settings writer,
`layout.rs`/`make_line`, editor, editing or buffer internals changed. The retained
flaky-test fix still synchronizes worker publication and the serial preference
queue, with the earlier 50 isolated and ten concurrent module runs recorded below.

The earlier review records follow unchanged.

---

# Round 18 Lane B pre-merge preference-test verification

Started clean on `lane-b` at `ac6f530` (WIP round 18 lane B fixes 3). This
follow-up remains uncommitted. Evidence and original-file backups are in
`/tmp/velocity-lane-b-r18-reload/`.

## Root cause and synchronization

This was a test timing artifact. The original test reproduced on isolated run 4:
`wait_resources()` forced a configuration stamp check while the serial queue was
performing the atomic preference writes. Creating/renaming a temporary file changed
the `Packages/User` directory stamp before the write acknowledgements were consumed.
The trace shows an intermediate load starting with three writes still pending,
then the one authoritative settled load after drain. The assertion counted both
publications as settled reloads (revision 3 instead of 2). File watching was disabled
in this fixture. No duplicate reload of an already settled completed write was found.
The failure trace is `trace-04.log`; its three publications started with pending-write
states `true`, `true`, `false`.

`app_lane_b_round17_tests.rs` test
`blocked_preference_queue_defers_one_reload_and_tracks_multiple_keys_per_write`
now synchronizes the external resource publication and the serial queue drain.
A queue fence runs after both actual zoom writes and the multi-key atomic write
have sent their acknowledgements. Repeated forced disk checks assert exactly one
pending external publication, exactly one settled publication, and no rediscovery
of the completed writes. All live overrides, eleven tabs, on-disk keys, the external
syntax setting, and the empty diagnostic list remain checked. Timeouts are deadlock
guards; no elapsed-time window determines the expected reload count.

`workspace.rs: poll_resource_reload` is a `#[cfg(test)]` helper: force the disk
check, await the real resource worker completion channel, and publish through the
existing `poll` path with the original settings-write generation. Runtime polling,
deferral, the shared writer, and reload publication code are unchanged.

## Verification

- **50/50 isolated processes passed**, each running the exact reported test.
- **10/10 simultaneous full-module processes passed** with default Rust test
  parallelism: 29 passed and one existing ignored timing test per process, or 290
  passed cases in total. Per-run logs and JSON summaries are under `isolated/`,
  `parallel-module/`, `isolated-summary.json`, and `parallel-module-summary.json`.
- Full workspace gate: **1,086 passed, zero failed** (114 suites, 25 existing
  ignored tests); fmt, strict release clippy, and strict rustdoc pass.
- Native behavior and the manual checklist below are unchanged by this test-only
  follow-up. No GUI, packaging script, `cargo clean`, or operation on
  `target/package-macos/` was performed. Earlier timing evidence below belongs to
  the preceding committed fix-3 verification.

The preceding round-18 implementation and verification record follows unchanged.

---

# Round 18 Lane B verification fixes

Started clean on `lane-b` at `c8260ac` (WIP round 18 lane B fixes 2), based on
main `385dcdd`. This second-verification follow-up remains uncommitted.
No reset/amend, GUI, packaging,
`cargo clean`, or `target/package-macos/` operation was performed.
Evidence and start-of-turn backups: `/tmp/velocity-lane-b-r18-final/`.
The main binary is the previously verified `git archive 385dcdd` build at
`/tmp/velocity-lane-b-r18-review/main-velocity`.

Changes in this follow-up are confined to build workers/adapters, view anchoring,
and Lane B's tests/docs. No `layout.rs`, `make_line`, editor, buffer, editing,
file-I/O, workspace reload, navigation or panel-overlay implementation changed.

## Second-verifier findings

| Finding | Fix (`file:function`) | Headless user-path regression |
| --- | --- | --- |
| Cost: single-caret anchor scans | `view.rs: apply_style`: lazily capture full text anchors and caret visibility only before a geometry/fold reflow, using the old layout. Remove the duplicate visibility query. Ordinary active typing and wheels do not call anchor `column`/`offset`/caret checks; painted row boundaries provide cheap reading bookmarks. | `round18_final_typing_and_wheel_skip_anchor_visibility_work` (real App input and wheels, one/four panes, zero additional expensive anchor queries; resize does query); retained geometry/fold/minimap/session tests; paired instruction gate below |
| 1 continuous detached output / bounded readers | `vel-build/process.rs: reader/run`: readiness-based reads, cancellation checked between reads, absolute 500 ms post-exit deadline. `output.rs: delivery/Sender::send`: FIFO delivery independent of pipe readers, at most 1 MiB queued text, 128 inline entries and the bounded public channel; excess uses an anonymous temporary file on workers. Finished follows already-read output even through UI stalls. Deadline/I/O loss is visibly marked. | `round18_final_continuous_daemon_finishes_and_cancels_promptly` (App exec/poll, <700 ms exit / <100 ms cancel); `round18_final_spill_is_bounded_lossless_and_ordered`; retained 600–2000 ms stall/tail, group-kill and restart-isolation tests |
| 2 shared login base | `environment.rs: cache_key/launch_environment_with`: capture identity contains inherited shell/home/profile locations and profile mtimes; build `env`, `path`, and program-specific `SHELL` only overlay the captured base. Warm-up, negative cache and in-flight capture are shared across systems. | `round18_final_login_alternating_builds_share_base` (isolated App, 3 s profile, A/B tools, eight later builds, one capture); `round18_final_login_cache_shared_across_build_overrides` (hanging profile, one wait and eight immediate alternate fallbacks); retained PATH table/profile-edit tests |
| 3 restart during capture | `environment.rs: launch_environment_with`: a cancelled worker stops waiting without dropping the cache-owned capture. | `round18_final_login_double_build_keeps_shared_capture` (real Cmd+B twice during a barrier-held initial capture; replacement and later builds all use the one result); `round18_final_login_cancelled_waiter_preserves_capture` |
| 4 quiet-build CPU | `process.rs: reader`: `poll` waits up to 25 ms, replacing 2 ms busy polling; read deadlines and cancel checks remain bounded. | `round18_final_quiet_build_cpu_timing` (ignored, explicitly measured, isolated App and 3 s quiet process); continuous-daemon and cancel test above |
| 5 trim whole lines | `build.rs: Builds::poll`: crossing 8 MiB trims through the next LF to at most 6 MiB, then adds the truncation notice. Result row offsets stay synchronized. | `round18_final_output_spill_and_line_boundary_retention` (24,000 long diagnostics, stalled App poll, complete error count/tail and retained result rows); `round18_final_oversized_output_line_discards_its_continuation` (producer barrier across a 9 MiB line, next displayed line whole); retained 240,000-result timing |
| 6 parallel login-test stability | `environment.rs` tests: independent cache and explicit inherited environment per fixture; healthy-shell tests wait for publication with a generous deadline instead of requiring a success inside the first-build 2 s fallback budget. App login tests run in isolated child processes with private HOME/SHELL. | `round18_login_user_shell_banner_cmd_path_cache_and_retry`; `round18_verify_login_warmup_fallback_and_end_sentinel`; parallel repetition results below |
| 7 edits above parked text | `view.rs: apply_style/edit_touched_caret/project_text_anchor`: map reading bookmarks through buffer changes for unfocused panes or edits away from that pane's caret; the typing pane follows its caret. | `round18_final_shared_edits_anchor_parked_and_focused_receivers` (1,000 inserted lines, actual text identity, focused/unfocused clone, caret-follow versus remote edit, subsequent wheel and second edit); `round18_final_long_line_paint_anchors_remote_edits` (the unwrapped long-line drawing path) |
| 8 unfold above viewport | `view.rs: apply_style`: fold changes trigger the same text-anchor reprojection as geometry changes. | `round18_final_unfold_above_preserves_visible_text` (real fold/unfold commands, top text/caret and no-op repeat); existing folded/settings suite |
| 9 reflow during keyboard animation | `view.rs: animate_scroll_from/apply_style/advance_scroll`: retain a text endpoint, finish the old animation before reflow, then project it using the new layout. | `round18_final_reflow_finishes_animation_at_text_destination` (resize/sidebar/wrap/zoom/unfold mid-PgDn, settled caret/hit test and no stale frame overwrite); retained typing/IME/zero/Reduce Motion interruption suites |

The spill file is private, anonymous and removed on delivery teardown; no UI-thread file
I/O or preference writer is added. Normal small output stays in memory. A stalled UI can
consume temporary disk space proportional to spooled output for that build, while queued
text/metadata and panel retention remain bounded. Pipes close after the fixed grace even if
a detached daemon keeps printing.

## Earlier verification fixes retained

| Finding | Fix (`file:function`) | Headless user-path regression |
| --- | --- | --- |
| 1 output tail during UI stalls | `vel-build/process.rs: reader/run`: nonblocking pipe reads observe a fixed 500 ms post-exit grace; worker delivery spills independently of UI backpressure. Finished follows all already-read data. `build.rs: poll/result`: 8→6 MiB retention adds a visible truncation notice and adjusts diagnostic rows. | `round18_verify_stalled_build_poll_preserves_tail_before_finished` (600/1200/2000 ms App poll pauses, exact byte count, marker before Finished); existing detached-pipe/restart/tree-kill tests; retention and ragged-output tests |
| 2 explicit destination reveal | `app.rs: run_named_depth`: history, Goto, positioned opens, results, bookmarks and marks reveal independently of selection identity. Plain sheet/group switches preserve parked viewports. | `round18_verify_jump_history_reveals_unchanged_destination_and_tabs_stay_parked` (the three-Back cross-file repro, Forward, bookmarks/marks and real tab switches); existing result/open/Goto destination tests |
| 3 reflow in every pane | `view.rs: apply_style/capture_text_anchor/project_text_anchor/remember_scroll`: each view retains a buffer offset, fractional row and horizontal subcell offset. Reproject only when width/options/metrics change; keep visible carets visible. Public layout queries only; canonical offsets avoid repeated-zoom drift. `app.rs: scene/sync_active_view/resize_views/reload_font_views`: retain all sheet views/anchors; assign actual pane geometry before reflow; font changes preserve views. | `round18_verify_geometry_sidebar_resize_and_single_group_zoom_anchor`; `round18_verify_clone_zoom_resize_and_font_reload_anchor_every_pane`; `round18_verify_unwrap_long_line_keeps_horizontal_reading_position`; `round18_verify_unwrap_keeps_visible_caret_and_horizontal_park`; existing folded/syntax reflow tests |
| 4 slow/failed login profiles | `environment.rs: Cache::prepare/refresh`, `launch_environment_with`: first request waits at most 2 s while a capture may continue to 15 s. Cache failure for five minutes or until a shell/startup-file mtime change; later builds fall back immediately, with one background retry and late-success promotion. `app_build.rs: apply_build_action`, `show_build_systems`: prewarm on the first build-system menu/picker selection. | `round18_verify_login_failure_backoff_slow_success_and_profile_edit` (hanging profile, four immediate fallbacks, profile edit retry, 3 s profile later used, capture once); `round18_verify_login_warmup_fallback_and_end_sentinel` |
| 5 shell fallback/logout output | `environment.rs: capture_shell_in/collect_login`: unsupported/nonzero or missing-marker user shells retry bash once; zsh remains the first attempt when selected. Unique start/end sentinels exclude banners and logout/EXIT assignments. | `round18_verify_login_warmup_fallback_and_end_sentinel` (rejecting shell, missing start marker, prefix and EXIT output); existing banner/PATH and bounded detached-login tests |
| 6 real headless tab changes | `app.rs: select_tab_action/apply_named_action/apply_action`: native and headless routes share `workspace.select`, replacing the headless no-op. | `round18_review_tab_and_group_commands_preserve_destination_viewports` now asserts actual sheet/index changes; the new history test also proves tab switching |
| 7 build memories and signals | `build.rs: remember/remembered`: bounded 16-entry per-window map keyed by the sorted matching set; only picker choices are stored and membership stays validated. `finish_line_with_signal`: platform `strsignal` descriptions, with explicit SIGUSR1/SIGUSR2 names. | `round18_verify_picker_remembers_multiple_matching_sets_and_signal_name` (RustB→PyA→RustB→PyA without new prompts, SIGUSR1); existing picker membership, explicit-choice and SIGKILL tests |

No preference defaults changed in this follow-up. The accepted output wrapping default
remains true per the round-18 spec; the public build docs define the option but do not
establish its default. Exact Sublime signal-summary wording remains unconfirmed; the
signal wording here is Velocity's own. Other accepted settings/behaviors are retained.

The [public build-system documentation](https://www.sublimetext.com/docs/build_systems.html)
supports selector matching, picker memory and explicit selection. The
[zsh startup/shutdown documentation](https://zsh.sourceforge.io/Doc/Release/Files.html)
identifies login and logout files; the source comments cite it for cache invalidation
and sentinel isolation. No bundled Sublime resources were accessed.

## Earlier review fixes retained

The following tests exercise the shared command/sidebar/build paths. Existing tests
that expected the old no-op minimap setting, hidden solid caret, unwrapped build default,
Save All from `exec`, or appearance registry stubs have been corrected.

| Finding | Fix (`file:function`) | Headless regression |
| --- | --- | --- |
| H1 remembered builds | `build.rs: Builds::remember/remembered`, `app_build.rs: apply_build_action`: remember only picker choices, keyed by sorted matching names/selectors; validate membership; explicit Tools choices bypass Automatic. | `round18_review_build_memory_tracks_matching_sets_and_only_picker_choices`; `round18_build_with_choices_filter_and_remember_variants` |
| H2 layout changes | `view.rs: apply_style/capture_text_anchor/project_text_anchor`: one persistent anchor per sheet preserves top buffer offset and fractional row for every reflow, including syntax overrides; retain visible carets. | `round18_review_layout_settings_anchor_wrapped_text_and_keep_visible_caret`; `round18_review_syntax_layout_override_preserves_reading_position`; original parked-view command table |
| M1 find panel | `app.rs: dispatch_key_binding`: intercepted find/accept Enter and Shift+Enter explicitly reveal even when the selection is unchanged. | `round18_review_find_enter_reveals_the_same_match_after_wheel_scroll` |
| M2 typing/IME | `view.rs: reveal/finish_scroll_animation`: finish the old destination before computing the new reveal; animation cannot overwrite it later. | `round18_review_typing_and_ime_reveals_finish_page_animation`; original interruption/zero/Reduce Motion sequences |
| M3 detached pipes | `vel-build/process.rs: run/spawn_after`: kill group; bound nonblocking reads after exit to 500 ms; independent worker delivery preserves already-read data before Finished; restart never joins the old worker. `environment.rs: collect_login`: pipe reads share the startup deadline. | `round18_detached_output_pipe_finishes_cancels_and_restarts_without_waiting`; `round18_login_detached_pipe_obeys_deadline_and_retries`; child/grandchild kill and restart isolation tests |
| M4 login environment | `environment.rs: launch_environment_with/capture_shell_in`: use `$SHELL -l -c` with bash fallback, capture strictly between unique NUL sentinels, apply to argv builds too; cache successes and back off failures without holding the mutex during profile I/O. | `round18_login_user_shell_banner_cmd_path_cache_and_retry`; `round18_verify_login_failure_backoff_slow_success_and_profile_edit`; environment/PATH table; cancellation test waits for real child output after any login warning |
| M5 sheet switches | `app.rs: command_states/target_changed`, `app_mouse.rs: handle_mouse_event`: compare the destination sheet's own immutable selection snapshot and change ID. | `round18_review_tab_and_group_commands_preserve_destination_viewports`; `round18_review_sidebar_open_files_preserves_parked_scroll` |
| M6 gutter cost | `view.rs: scene/line_number_color`: compute visible caret lines once; gutter rows only query that set, including long lines. | `round18_review_gutter_precomputes_only_visible_caret_lines`; `round18_review_gutter_40000_carets_timing` (ignored, explicitly run) |
| M7 minimap/wrap | `view.rs: pointer_with_option`: true keeps the drawn clicked row at pointer y, false retains centering/viewport drag. `vel-build/lib.rs: resolve`: output wrapping defaults to true per the spec. | `round18_minimap_switch_places_clicked_text_and_preserves_drag_geometry`; `round18_review_minimap_switch_clicks_inside_the_drawn_viewport`; `round18_environment_path_working_directory_and_wrap_table` |
| M8 Save All preference | `app_mouse.rs: apply_view_action`, `workbench.rs: set_user_preference`: set User layer across tabs, enqueue atomic read/modify/write on the existing queue, track pending overrides. Check state reads effective settings. | `round18_review_save_all_is_a_persisted_user_preference_and_exec_does_not_save` |
| L1 matching cost | `app_build.rs: apply_build_action`, `build.rs: matching/caret_scopes`: compute once in Automatic, skip explicit builds, root selectors avoid parsing; embedded/exclusion selectors get a 5 ms stale-scope budget. | `round18_review_large_root_selector_build_avoids_a_full_caret_parse`; `round18_review_build_matching_120000_lines_timing`; embedded-scope scoring table/adapter |
| L2 output retention | `build.rs: poll`: crossing 8 MiB trims to 6 MiB in one operation; result offsets stay synchronized. | `round18_review_signal_output_and_retention_hysteresis`; `round18_ragged_build_output_timing_is_incremental_bounded_and_navigable` |
| L3 caret layering | `view.rs: scene`: invert only blinking styles; solid remains visible. Opaque block fills precede text, with clipped background-color glyphs above them. Existing translucent Vintage caret layering is preserved. | `round18_caret_block_extensions_and_inverse_phase_render_both_values`; `round18_display_syntax_override_uses_the_resource_loader`; existing Vintage rendering tests |
| L4 preview recents | `app.rs: sync_workspace_after_panels`, `workbench.rs: sync_shared/snapshot`: temporary overlay protection does not commit a preview; explicit promotion/Goto accept still does. Pending previews are omitted from sessions. | `round18_review_sidebar_preview_palette_cancel_does_not_commit_recent_or_session`; original sidebar preview lifecycle/session tests |
| L5 signal summary | `vel-build/process.rs: Event::Finished`, `build.rs: finish_line_with_signal`: retain the signal number and print an English name, e.g. signal 9 (Killed). | `round18_review_signal_output_and_retention_hysteresis`; existing finish/exit/cancel/quiet table |
| L6 build context | `vel-config/keymap.rs: set_app_context/resolve`, `app.rs: dispatch_key_binding`: app-owned `build_running` context; no preference revision changes; works with panel focus. | `round18_review_build_context_does_not_mutate_settings_and_works_in_find_panel`; F7/Ctrl+C dispatch test |
| L7 result source/exec | `app_build.rs: apply_build_action`: headless navigation honors active search results; only `build` performs Save All. | `round18_review_headless_result_navigation_obeys_active_result_source`; `round18_review_save_all_is_a_persisted_user_preference_and_exec_does_not_save` |
| Merged appearance rows | Menu/palette tests resolve actual merged commands, with no injected registrations. | `round18_menu_palette_rows_resolve_against_the_lane_a_command_contract`; startup and all-menu resolution tests |

## Sources and defaults

- [Official build documentation](https://www.sublimetext.com/docs/build_systems.html)
  confirms automatic matching, picker memory, explicit choices, environment overrides,
  PATH, quiet and boolean output wrapping. It does **not** establish a `word_wrap`
  default: the accepted round-18 implementation follows the explicit specification with
  output wrapping **true**. Both values remain supported. Main editor wrapping is unchanged.
- No public signal-summary wording was found there. The requested signal number plus
  ordinary English POSIX signal name is Velocity's own wording, not a claim of exact
  Sublime output. Finish/exit/cancel/quiet formatting otherwise stays as accepted.
- The minimap setting keeps default false and implements this review's specified geometry.
  Public release notes name it but do not establish that geometry; the code comment records
  the source and the spec requirement. Both click modes retain the reviewed drag mapping.
- [Community settings reference](https://docs.sublimetext.io/reference/settings.html)
  documents scroll speed and sidebar previews; the existing detailed-reference citations
  and [ST4 changelog](https://www.sublimetext.com/download) still support the previously
  accepted settings/defaults. No other preference defaults changed in this follow-up.
- Native checks remain unobserved because the sandbox has no GUI. The reviewer checklist
  is the first section of `MANUAL_TEST.md`. No Sublime Default package, theme, color scheme
  or bundled build-system files were downloaded or copied for these fixes.

## Final verification and performance

Fmt and `git diff --check` pass. Strict release/locked/all-target workspace clippy and
strict rustdoc pass. The release/locked workspace gate uses default parallelism:
**1,086 passed, zero failures**, across 114 suites; 25 tests are ignored.
All **24 headless timing tests** were explicitly run, serially and without concurrent
compilation, and pass. Only native FSEvents delivery and the desktop checklist remain manual.

Login repetition: **30/30 trials pass**, each running seven library login tests plus two
isolated App login tests: **270 passed cases**. Four trial processes run concurrently;
Rust test parallelism remains at its default within each suite. The two originally flaky
positive-profile tests are isolated from global environment/cache state and wait for actual
publication. The first stress run exposed startup timing assumptions in the new fixtures;
startup/release barriers corrected them without changing the production two-second budget.
Both stress runs are retained in the evidence directory.

Nine alternating pairs use the supplied `tools/scroll-gate/paired-headless.py` against
main `385dcdd`. Darwin `rusage_info_v4` captures retired instructions from those same children
before reaping, including startup and workers. No local compilation or tests ran concurrently.
One-minute host load spans **3.00–19.53** on eight logical CPUs; the requested
instruction fallback is the acceptance measure. Single-caret typing passes the stricter
**+0.5%** limit; both other workloads pass **+5%**.

| Workload | Main median instructions | Candidate median instructions | Change | Limit |
| --- | ---: | ---: | ---: | ---: |
| features | 239,262,318,069 | 239,735,159,153 | +0.20% | 5% |
| typing1 | 427,976,035 | 429,613,373 | +0.38% | 0.5% |
| typing1000 | 10,652,551,068 | 10,666,049,649 | +0.13% | 5% |

Wall metric deltas span **-0.69% to +4.98%**;
the wall gate passes.

Additional explicit measurements:

- Three-second quiet build, isolated App path: **15.182 ms process CPU**, including
  App polling and worker threads. Pipe readiness waits replace the former 2 ms busy polling.
- 60,001-file cold-index walk, OS filesystem cache warm: **110.117 → 109.909 ms (-0.19%)**.
  Project traversal/event code is unchanged; incremental events still avoid full re-walks.
- 40,000-caret gutter off/on: **10.528 → 10.698 ms/frame**.
- 120,000-line build matching: root median **0.000292 ms**; embedded-scope
  budget median/max **5.648/6.954 ms**.
- Ragged output, 240,000 errors: **162 polls**, median/p95/max
  **2.097/2.220/3.726 ms**, **6,291,424 retained bytes** and
  **32,768 retained diagnostics**, complete error count and correct result rows.
  The 24,000-long-diagnostic and oversized-line App tests additionally check whole-line
  retention, spill delivery, final tail and the visible truncation notice.

Evidence: `gate-final.log`, `gate-status.json`, `clippy.log`, `rustdoc.log`,
`ignored-status.json`, `ignored-velocity.log`, `ignored-render.log`,
`ignored-complete.log`, `project-walk.log`, `login-stress/summary.json`,
`paired-final/report.json`, `paired-final/instructions.json`, and `check-summary.json`
in `/tmp/velocity-lane-b-r18-final/`.

Main SHA-256: `522cad5b7d8cf2a8705a62317b5d657a54687b55e9e5718d6a8c4559a5a8872a`.

Candidate SHA-256: `1c1644caf86240f181ae20034bf027fbe701bdf66e56a03aea19aaf089ba5eb9`.

## Item and headless-test mapping

Test names below are exact Rust test names. `app` means the shared native/headless adapter,
not a substitute command implementation. Sidebar tests call the real `Panels::mouse` path.

| Item | Implementation (`file:function`) | Tests |
| --- | --- | --- |
| B1 caret | `view.rs: View::drawn_caret`, `View::scene`; block, extra height, inverse opacity with existing styles | `round18_caret_block_extensions_and_inverse_phase_render_both_values`; `round18_display_syntax_override_uses_the_resource_loader` |
| B2 gutter | `view.rs: View::line_number_color`, normal and long-line number paint | `round18_gutter_uses_every_caret_line_and_long_line_numbers` |
| B3 minimap | `view.rs: View::pointer_with_option`; implement pointer-relative clicks; preserve reviewed drag mapping; preserve round 17 border | `round18_minimap_switch_places_clicked_text_and_preserves_drag_geometry`; existing `minimap_border_draws_four_viewport_edges_without_hover_through_dispatch` and round 17 click/drag sequences |
| B4 tabs | `app.rs: App::scene`; `app_chrome.rs: App::paint_tab_buttons`, `App::tab_button_mouse`; fit/scroll geometry, dirty label attributes and theme controls | `round18_tabs_fit_scroll_dirty_colors_buttons_and_live_hiding` (including 83 narrow fitted tabs and repeated button presses) |
| B5 title | `workbench.rs: Workspace::window_title`; absolute/relative/basename title policy | `round18_title_full_relative_basename_and_status_modes` |
| B6 previews | `panels.rs: Panels::mouse`, `Panels::activate_side`; `workbench.rs: Workspace::make_permanent`, `sync_shared`, `snapshot`; small lifecycle hooks in `workspace.rs` and tab double click in `app_mouse.rs` | `round18_sidebar_preview_setting_replacement_promotion_and_session_recent_files`; `round18_sidebar_next_preview_replaces_only_clean_transient_tabs` |
| B7 build panel | `build.rs: Builds::start`; `app_build.rs: App::apply_build_action` | `round18_build_dispatch_panel_save_all_restart_cancel_and_key_context` |
| B8 position status | `vel-ui/src/status.rs: position_mode`; `panels.rs: Panels::paint` | `round18_title_full_relative_basename_and_status_modes` |
| B9 tree | `panels.rs: Panels::rebuild_side`, `tree_row_position`, `mouse`, `paint`; `vel-ui/src/theme.rs: ThemeUi::reduce_motion` | `round18_tree_motion_expand_collapse_repetition_disabled_and_reduce_motion` |
| B10 file lists/index | default binary patterns; existing incremental `vel-search/src/project.rs: Tree::apply/refresh`, `Rules::allows` and existing global symbol-index adapter preserved | `round18_binary_patterns_cover_file_lists_but_leave_sidebar_entries`; `round18_file_events_are_local_and_ignore_rules_do_not_hide_target_from_search`; `round18_index_files_toggle_leaves_searchable_file_lists_including_gitignored_target`; existing `ignored_files_stay_ignored_on_direct_updates_and_unsaved_edits` tests both index-exclude values |
| A8 scroll speed | `view.rs: View::prepare_keyboard_scroll`, `animate_scroll_from`, `advance_scroll`; app keyboard and deferred Goto center hooks | `round18_keyboard_scroll_speed_repetition_units_pointer_and_settled_hit_testing`; `round18_scroll_speed_changes_duration_without_changing_target_across_zoom`; `round18_goto_line_keyboard_animation_keeps_the_final_center_and_selection`; `round18_keyboard_scroll_interruption_and_live_zero_speed_converge` |
| Extra command reveal | `app.rs: App::run_named_depth`, `dispatch_key_binding`, panel text input; local mouse dispatch comparisons in `app_mouse.rs` | `round18_command_reveal_table_preserves_parked_views_and_follows_real_changes`; `round18_accepting_a_status_bar_syntax_row_preserves_the_parked_scroll` |
| Extra menu/palette | owned Main menu, palette and build keymap block; Goto rows, View > Preferences appearance rows, Tools build rows | `round18_menu_palette_rows_resolve_against_the_lane_a_command_contract`, existing menu/palette resolution tests updated for declared application-hosted commands |

## Build details

| Spec | Implementation | Tests |
| --- | --- | --- |
| 3.1 environment/PATH | `vel-build/src/environment.rs: launch_environment_with`, `launch_environment`, `expand_environment`, `collect_login`; worker-only login startup with a two-second first wait and bounded background capture; success/failure invalidation follows shell/profile mtimes; path/env overlay the shared base per build; `BuildSystem::resolve` uses the current file directory | `round18_environment_path_working_directory_and_wrap_table`; `round18_environment_expands_against_base_and_file_variables_without_recursion`; `round18_verify_login_failure_backoff_slow_success_and_profile_edit`; `round18_login_failure_and_timeout_are_bounded_and_do_not_block_the_caller` |
| 3.2 variables | `vel-build/src/variables.rs: Variables::expression` reuses `vel-snippet` | `round18_snippet_transform_first_global_captures_case_and_empty_matches` |
| 3.3 selectors/choices | `vel-build/src/lib.rs: BuildSystem::match_score`; `build.rs: Builds::caret_scopes`, `matching`, `choices`; `app_build.rs` asks once and retains picker system/variant choice per matching set | `round18_selector_scoring_table_including_embedded_and_excluded_scopes`; `round18_build_selector_adapter_synchronously_scores_stale_embedded_scopes`; `round18_build_with_choices_filter_and_remember_variants`; `round18_variant_overrides_effective_platform_base_and_can_clear_shell_cmd` |
| 3.4 flow/output | `vel-build/src/process.rs: Task::spawn_after`, `cancel`, worker `run`; cancellation kills the process group immediately and reaps it asynchronously without delaying replacement; `build.rs: Builds::poll`, `finish_line` bounds queues/output/diagnostics and keeps retained result row indices correct | `round18_restart_kills_child_and_grandchild_before_new_process_starts`; `round18_build_dispatch_panel_save_all_restart_cancel_and_key_context`; `round18_build_output_format_table_and_unsupported_stdin`; explicit `round18_ragged_build_output_timing_is_incremental_bounded_and_navigable`; existing streaming/UTF-8/full-queue-drop tests |
| 3.5 menus/keys | F7 build; Ctrl+C has running-build context; Tools build radios/results/Save All; original untitled JSON template suggests `.sublime-build`; interactive builds report unsupported | `round18_build_dispatch_panel_save_all_restart_cancel_and_key_context`; `round18_menu_palette_rows_resolve_against_the_lane_a_command_contract` |

Finish summaries follow the requested `[Finished in …]`, optional exit code, `[Cancelled]`,
and quiet behavior. There is no invocation/directory header. Status reports the complete parsed
error count even when older output/diagnostics have been discarded. Login fallback remains
visible in the completion status. No process or pipe join runs on the UI thread.
