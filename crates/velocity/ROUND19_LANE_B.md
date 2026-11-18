# Round 19 Lane B: special-file and final verification fixes

Uncommitted on top of `e7c42d9`; no reset, amend or commit. Starting source:
`/tmp/velocity-r19-special-fixes/start.tar`. The sustained-output timing change
from `e7c42d9` is unchanged: `<50 ms` only with `VELOCITY_STRICT_TIMING=1`,
with the data-per-poll ratio and one-second bound always enforced. No GUI,
packaging script, `cargo clean` or `target/package-macos/` operation was used.

## Findings and user-path tests

| Finding | Fix (`file:function`) | Headless regression |
| --- | --- | --- |
| 1: FIFO/special files hang restore, lazy activation or save | `vel-fileio/regular.rs:metadata/read/write` is the shared document boundary for explicit/default/background decoding, binary detection/snapshots, session history, reload/touch comparison and all saves. It follows regular symlinks, rejects other types, opens with `O_NONBLOCK`, verifies the opened fd with `fstat`, then clears the flag. Writes never truncate before checking the fd; raced-in replacements and newly created targets are also protected. `restore_io.rs:run` moves path resolution, probes, history/file reads and worker setup off the startup thread, with a two-second window deadline, four concurrent readers and cooperative decoder cancellation. Timeout preserves the original session and opens retained text unsaved (or a blank window); recovery does not reread clean disk histories. Failed restored/lazy documents share consistent read-only error views; fresh opens retain their existing refusal and CLI acknowledgements. | `r19_special_startup_and_lazy_click_reject_fifo_socket_and_device_in_every_layout` (3 types × 4 layouts, tab/key dispatch and another restart); `r19_special_saves_save_all_and_save_on_close_never_open_a_fifo`; `r19_special_restore_watchdog_backs_up_and_keeps_dirty_text`; `r19_special_open_and_external_reload_reject_nonregular_files_nonmodally`. Core fd-race and cancellation tests: `checked_document_opens_reject_a_special_file_swapped_after_stat`, `checked_regular_symlinks_clear_nonblock_and_cancel_history_decoding`. |
| 2: second failed window receives a reserialized temporary backup | `session.rs:Sessions::new` and the startup/window constructors retain the original snapshot independently of the bounded lookup cache, with strong references held by each workspace. `session_restore.rs:original/backup` shares those bytes and window identities. Before removing the source, it writes every window's numbered, byte-identical backup beside that source, private from creation (`0600`). Later failures select their existing backup; incomplete copies are not accepted. | `r19_special_both_failed_windows_get_numbered_original_byte_backups` checks both backups already exist after the first failure, exact source bytes, private permissions, location and distinct reports even after deterministic cache eviction. |
| 3: Open File only focuses a failed view | `workspace.rs:open_mode` retries the failed logical document in place; `shared_document.rs:retry_failed_document` reconnects siblings through the normal checked restore/attachment path. Aliases use the repaired canonical identity. | `r19_special_reopen_retries_error_and_error_close_never_prompts`: repair the file, Open File again, same sheets, real text and editable views. |
| 4: clean error views prompt after the disk file is repaired | `workspace.rs:Tab::dirty` and `workbench.rs:document_contents_dirty` exclude error placeholders. `shared_document.rs:fail_shared_document` turns actual retained edits into an editable, unsaved recovery document, which still prompts and supports Save As. Two older missing-file tests now assert a clean read-only failed view instead of treating the empty placeholder as unsaved contents. | `r19_special_reopen_retries_error_and_error_close_never_prompts` leaves a queued Cancel unconsumed while both error views close. `r19_special_lazy_watchdog_recovers_retained_edits_as_unsaved_text` times out a hidden dirty clone, cancels close, then saves its exact recovered text. |
| 5: duplicated filename in IO notices | `regular.rs:error` adds the path exactly once; restore/progressive/binary errors and `open_requests.rs:Router::apply/open_locations` use it. Special-file refusals are non-modal in native and headless dispatch. | `r19_special_open_and_external_reload_reject_nonregular_files_nonmodally` checks the exact occurrence count and an empty modal queue for all three types. Existing CLI batch tests retain ordered failures and subsequent successful opens. |
| 6: externally replaced binary shows an error on its first restore | `workbench.rs:prepare_sheet_inner` probes the source before importing a clean disk-backed history. Binary sources start the hex snapshot worker immediately; dirty retained histories keep their edits and undo/selection behavior. | `r19_special_clean_text_becomes_hex_on_first_restore`: 4 MiB NUL-containing replacement, first launch, hex/read-only, unchanged bytes after Cmd+S. Existing binary lazy-property tests remain active. |
| 7: saving silently overwrites an external replacement | `shared_document.rs:save_conflict/acknowledge_overwrite`, `Workspace::save` and `app.rs:save_tab_with_path` compare raw dev/inode/size/mtime identity (session `DiskStamp` including size when a retained history has no live identity). Cmd+S, Save All and save-on-close share Overwrite/Cancel. Cancel preserves both versions; a further revision requires another decision. The unchanged-touch path refreshes the accepted revision. The opened writable fd is checked again before truncation. | `r19_special_changed_disk_requires_overwrite_and_cancel_preserves_both_versions`: atomic 4 MiB replacement, Cancel then Overwrite for all three commands; `checked_document_opens_reject_a_special_file_swapped_after_stat` checks a replacement during open remains untouched. |
| 8: tiny groups overlap neighboring tab bars | `workbench.rs:workbench_action` trials New Pane and refuses cells below the minimum. `vel-ui/workbench.rs:drag_with_minimum`, `app_mouse.rs` and `App::resize_views/scene` retain at least a tab bar plus three text lines (and 120 logical px width). `vel-platform:WindowHandle::set_minimum_content_size` applies the current layout minimum to the native window. | `r19_special_panes_keep_tab_bars_inside_their_group_when_split_dragged_and_resized`: repeated commands, mouse splitter drag, window resize, every tab hit inside its group and an exact-target middle close. |

No menu, binding or User preference defaults changed. Pane minimums implement the
requested usability limit; they are not claimed as a documented Sublime pixel
constant. Existing clean-room citations remain. The fd contract cites POSIX and
Apple's public `open(2)`/`fstat(2)` documentation in `regular.rs`; no Sublime
Default package, menu, keymap or binary was inspected.

The extended verifier model now also replaces files with FIFOs, Unix sockets and
symlinks to `/dev/zero` between checkpoint and restart. Its own oracle uses the
checked reader, so verification cannot hang reading those paths. Cleanup removes
the symlink itself and never chmods or writes its device target. Hidden views
remain lazy until actual user operations select them. Existing eager/lazy models,
close targets/prompts, text preservation and workbench/session invariants remain.

Logs, sealed binaries and paired measurements are retained under
`/tmp/velocity-r19-special-fixes/`. Native checks added to the top of
`MANUAL_TEST.md` remain unchecked because GUI use is prohibited. Tests that may
capture login state use the runtime fixture provider; benchmark child processes
receive an isolated HOME/SHELL.

The separate `vel-build/tests/build.rs` transport timing probe also subtracts
only sleep beyond its requested 16 ms cadence. It keeps its strict 50 ms UI gap,
minimum poll count, 90% data-poll ratio, diagnostic count, bounded transport and
lossless-tail assertions. A concurrent compile/sweep attempt had 158/158 data
polls but a raw 96.66 ms scheduling gap; the corrected probe passed under that
load with 155/155 data polls, a 16.23 ms UI gap and 559.05 ms total excluded
oversleep (`build-timing-last.log`). No production build code changed, and the
App sustained-output test changed in `e7c42d9` remains byte-identical.

## Verification

A final parallel run exposed lookup-cache eviction between two failing windows.
The coordinator and workspace now retain strong references to the launch snapshot;
the two-window test explicitly evicts the cache before restoring its second window.
It verifies numbered backups, exact original bytes and private permissions.


- Formatting and `git diff --check`: pass (`completed-fmt.log`).
- `cargo clippy --workspace --all-targets --release --locked -- -D warnings`:
  pass (`completed-clippy.log`).
- `RUST_TEST_THREADS=6 cargo test --workspace --locked --release`:
  **1,156 passed, zero failed, 30 ignored**, 114 suites
  (`completed-workspace.log`). The 10 new App tests use production startup,
  named/key/mouse dispatch, the native close/save decision functions and real
  temporary filesystem data. Two additional fileio tests cover descriptor
  races, regular symlinks, flag clearing and decoder cancellation.
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked`: pass
  (`completed-doc.log`).
- Extended eager, lazy and failed-load models: **40 × 1,000 each**, plus the
  **40 × 500** layout sweep: **140,000 final-build operations**, four tests pass
  (`sweep-completed.log`). Across the three larger models: **4,748 startup restores,
  17,243 exact-target closes and 9,304 prompts**, zero unexplained losses or
  focus mismatches. Failed sources: **81 chmod-000, 87 directories, 84 deletions,
  101 FIFOs, 88 sockets and 82 symlinks to `/dev/zero`**. The hidden-view variant
  leaves hidden sheets lazy instead of pre-opening them.
- `scripts/check-release-login.py`: all-targets release build contains **no
  login-fixture markers** (`release-login-completed.log`). This does not assemble a
  package or run the application.

Sealed App test executable SHA-256:
`8897149367882c8c6efb3b1554d917f33c825a5922109004d3cf3df833ffd68d`.
Shipping/all-targets SHA-256:
`48a69a29c3cc6b61803f355e7c0c78eb41ef64bc90fc586d532d5d98269ae5f1`.
`source-completed.json` records the implementation hashes and starting HEAD; the
sealed files match the built executables. Existing native/privacy/Finder checks
and the new pane/restore/conflict checklist remain pending in `MANUAL_TEST.md`.

## Per-key App performance

Nine alternating pairs use the unchanged App harness (source SHA-256
`453d0633b38a50b1532d8583aca5f17bd0d8f489ff0dc96da75781316cf5046f`)
in the sealed `385dcdd`, `c87df2e` and current test executables. After native font
initialization and 200 warm keys, it measures 1,000 keys through real App input,
changed-state bookkeeping and scene construction. It excludes startup and uses
calling-thread retired instructions; both populated panes use actual wrapped
rows. No native window or GPU renderer runs.

| App case | `385dcdd` instructions/key | `c87df2e` before | Current candidate |
| --- | ---: | ---: | ---: |
| Single pane, one caret | 3,886,642 | 4,079,444 (+4.96%) | 3,845,812 (−1.05%) |
| Two populated wrapped panes | 4,194,318 | 4,312,923 (+2.83%) | 4,215,298 (+0.50%) |

Both cases pass the **+1.5%** limit. “Before” is the frozen round-18 baseline;
the improvement includes the earlier round-19 fixes. These are fixture-specific
instruction counts, not native input/scanout latency. Raw samples, per-phase
counters, process-wide counts and hashes: `app-completed/report.json`.

Three final alternating whole-process pairs against `c87df2e` pass the **5% retired-instruction limit**: **features −0.26%, typing1 −1.27%, typing1000 −0.64%** (`paired-completed/instructions.json`). Earlier nine-pair samples are retained separately in `paired-last/`. The final measured files match the built binaries.

The secondary wall-time report did not pass every percentile under concurrent load: `typing1/headless_summary/edit_and_scene_cpu_ms/p50` +6.71%. The allowed retired-instruction gate is the performance result; raw wall samples remain in `paired-completed/report.json`.

---

# Previous recovery verification (now in 53deb99)

Uncommitted on top of `70fb921`; no reset, amend or commit. Starting source:
`/tmp/velocity-r19-recovery-fixes/start.tar`. No GUI, packaging script,
`cargo clean`, or `target/package-macos/` operation was used.

## Findings and user-path tests

| Finding | Fix (`file:function`) | Headless test |
| --- | --- | --- |
| A: unreadable cloned files panic on startup or later attachment | `shared_document.rs:require_editor_view/ensure_shared_view/fail_shared_document` checks IDs before capture, state application or adoption and attaches every sibling consistently to one read-only error document. `workbench.rs:prepare_sheet_inner/reconcile_installs/sync_shared` use those boundaries. Load errors report their path non-modally and still permit focus/close operations. `snapshot` never retains an empty error placeholder as saved text. | `r19_recovery_unreadable_clones_restore_and_materialise_without_panics`: chmod 000, directory replacement and deletion, each with original/clone/three-view/all-hidden layouts; tab dispatch, rejected edits, validation and another restart. |
| A: unexpected restore failure creates a launch loop | `session_restore.rs:restore_window_from` catches errors/panics per window, moves the source session to a timestamped `.bak`, recovers available text into new unsaved tabs (or opens a blank window), and reports the backup path non-modally. `prepare_sheet` also contains later lazy-restore panics. Global startup, project-workspace startup, Open Project and additional native windows use the boundary. | `r19_recovery_every_restore_step_backs_up_and_preserves_recoverable_text`: all 12 stages, byte-identical backups, dirty text, no modal entries and another launch. `r19_recovery_failed_window_does_not_block_another_window_or_blank_fallback` and `r19_recovery_hidden_view_panic_is_contained_on_tab_dispatch`: window isolation, no-text fallback and a later tab click. |
| B: binary lazy views truncate files | `shared_document.rs:Tab::inherit_document/inherit_document_mode` is shared by Clone File, lazy attachment, ownership transfer and completed loads, carrying binary/read-only, format/path, dirty/deleted and language/feature/metric state. `workspace.rs:Tab::transfer_document_io` moves workers with the actual buffer; pending view state stays per sheet. `Workspace::save` rejects binary/hex identity before a text write, independently of read-only or the snapshot. | `r19_recovery_binary_lazy_views_never_write_through_the_text_path`: 4 MiB NUL-containing files, visible orientations/all-hidden, clicks before/after completion, Cmd+S/insert/Cmd+S, unchanged bytes and forced removal of read-only plus the snapshot. `r19_recovery_clone_and_lazy_attachment_copy_the_same_document_properties`: property table for clean, dirty, deleted and binary documents. |
| C: symlinked Save As creates a different identity | `workspace.rs:absolute` canonicalizes the existing parent of a new filename; open, Save As, restore and `open_requests.rs:same_path` use it. | `r19_recovery_save_as_symlink_has_one_identity_and_receives_external_changes`: Save dispatch, actual watcher reload, real-path open focuses the same sheet, alias open after restart creates no duplicate. |

The existing eager/lazy verifier models remain. The new
`r19_recovery_extended_failed_load_lazy_state_machine` runs 40 × 1,000 operations
with unreadable files, directory replacements and deletions between checkpoint
and startup. Damage persists throughout the following session, including later
tab clicks; hidden views are not pre-opened. Every operation checks attachments
and workbench invariants; sessions validate and Hot Exit retains all dirty text.
Exact clean text is exempt only while its sole disk source is unavailable. Dirty
text must survive exactly unless explicitly discarded or saved. The smaller
`r19_recovery_failed_load_lazy_state_machine` runs in the normal gate.

No defaults, menus or bindings changed. Existing source citations and previously
verified behavior remain. No Sublime Default package/menu/keymap/binary was read,
downloaded or copied. Workspace edits are limited to lifecycle fields, document
I/O ownership, binary save guards and path identity; Lane A's reload application
and status text are unchanged.

## Verification

Logs, sealed binaries and benchmark samples:
`/tmp/velocity-r19-recovery-fixes/`.

- `cargo fmt --all -- --check` and `git diff --check`: pass.
- `cargo clippy --workspace --all-targets --release --locked -- -D warnings`:
  pass (`clippy-complete.log`).
- `RUST_TEST_THREADS=6 cargo test --workspace --locked --release`:
  **1,144 passed, zero failed, 30 ignored**, 114 suites (`workspace-complete.log`).
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked`: pass
  (`rustdoc-final.log`).
- `scripts/check-release-login.py`: **no login-fixture markers** in the
  all-targets release binary (`release-login.log`); no packaging script was run.
- Extended eager, lazy and failed-load models: **40 × 1,000 each**, plus the
  existing **40 × 500** layout sweep: **140,000 operations, four tests passed**
  (`sweeps-complete.log`). Across the three larger models: **4,751 startup
  restores, 17,212 exact-target closes, 9,398 prompts**, zero unexplained losses
  or focus mismatches. The failed-load model damages **180 unreadable files,
  181 directory replacements and 172 deleted sources**. Tests leave hidden
  sheets lazy and verify that failed-load views remain read-only.

Updated older fixtures to expect canonical parents for new filenames and the
non-modal failed-load report. The focus-save fixture now selects the original
through real tab dispatch *before* corrupting an inactive adapter's cached
encoding; it still proves one save and unchanged safety skips. The failed-load
model recognizes read-only Save As failures as sequential-close stop conditions,
and distinguishes unavailable clean disk text from retained dirty contents.

The final App measurement uses the identical harness (SHA-256
`453d0633b38a50b1532d8583aca5f17bd0d8f489ff0dc96da75781316cf5046f`),
native fonts, 200 warm keys and 1,000 measured keys in each of nine alternating
pairs. Calling-thread retired instructions include key dispatch, committed input,
changed_state and App::scene, excluding startup. Whole-process counts are also
retained in `app-per-key-complete/report.json`.

| App case | 385dcdd instructions/key | Current instructions/key | Current vs 385dcdd |
| --- | ---: | ---: | ---: |
| Single pane, single caret | 3,782,712 | 3,712,998 | **-1.84%** |
| Two populated wrapped panes | 4,202,756 | 4,223,168 | **+0.49%** |

Both pass the **+1.5%** per-key limit. The first run overlapped the full gate and
sweeps and reported +2.43% for two panes, with substantial sample variation
(`app-per-key/report.json`). An isolated repeat passed at -0.65%/+0.42%, and the
final run above uses the final built test executable after all other checks
finished. No harness or threshold was changed to resolve the timing variation.

Nine paired whole-process retired-instruction runs against **c87df2e** also pass
the <=5% limit: **features -0.50%, typing1 -0.42%, typing1000 -0.02%**
(`paired-headless/instructions.json`). The secondary wall-time report has three
failures under concurrent load: feature scroll p99 +12.09%, highlighted typing
p99 +6.64% and scene p50 +7.85%. The requested retired-instruction gate is used
under load; wall-time results are retained rather than reported as passing.

Final executable SHA-256 values:

- Shipping/all-targets binary:
  `14f16e63322450d095b30983f004fdbba118d7d1c3784372867901dae16531a4`.
- App test executable:
  `53b2d23df3a48b6b00919afa8fe5ce93b6fa3e1f95e392877af68eb96699f445`.

Sealed copies match the final built files. App/build tests use the runtime login
fixture provider; production benchmark children receive a private HOME/SHELL.
Native checks, including actual macOS privacy denial and Finder presentation,
remain unchecked in `MANUAL_TEST.md` because GUI use is prohibited. No defaults
changed and no requested fix remains unimplemented.

---

# Previous verification pass (now in 70fb921)

Uncommitted on top of `bdb5a7a`; no reset, amend or commit. The source at the
start of this pass is archived in `/tmp/velocity-r19-verification-fixes/start.tar`.
No GUI, packaging script, `cargo clean`, or `target/package-macos/` operation.

## Current findings and user-path tests

| Finding | Fix (`file:function`) | Headless test |
| --- | --- | --- |
| Restored lazy clones lose edits on close/save | `workbench.rs:restore_window/prepare_sheet_inner/reconcile_installs` relink every sibling to one buffer before editing. Lazy views keep live view IDs, rebased selections/folds and edit ownership. `prepare_close_sheet` and `app_tabs.rs:can_close_tab` / `app.rs:can_close/can_close_many` use the session document identity. A sibling attaches to the live buffer before ownership is transferred on close. | Adopted `b19_restored_lazy_clone_close_preserves_edits_for_both_decisions_and_later_save`, `b19_restored_lazy_clone_save_as_attaches_to_live_buffer_and_history`, and `b19_restored_lazy_clone_attaches_rebased_selection_and_folds` in `app_lane_b_round19_verification_tests.rs`. Both visible-original/visible-clone orientations, queued Save/Discard answers left unconsumed, Save As with a still-lazy sibling, later Cmd+S, checkpoint/restart and undo. |
| Preference-write JSON errors are modal, anonymous or repeated | `workbench.rs:set_user_preference/zoom_font` include the physical filename; the existing settings-write completion branch publishes non-modal resource diagnostics. `settings_reports.rs:Scope` routes them to the initiating workspace and deduplicates by canonical filename and revision (contents + mtime). Only one current revision is retained per source; removed files are pruned, and parsing/IO stays outside the shared lock. `Configuration::load` preserves each settings diagnostic's filename/line. The previous last-valid-file cache remains in effect. | `r19_verify_preference_writers_report_broken_file_nonmodally_once_per_revision` covers Save All's toggle and larger/smaller/reset font writes, byte-identical broken files, new revisions and no modal queue entries. `r19_verify_settings_errors_report_only_in_initiating_window_and_not_on_unrelated_reload` saves through Cmd+S, lets the other window reload first, changes an unrelated file, closes the reporter and opens another window. |
| Closing one selected tab collapses the others | `workspace.rs:close` retains surviving selected sheets and focuses the neighboring selected sheet. | Strengthened `r19_review_single_close_and_middle_click_preserve_other_selected_sheets` checks the exact selection, visible count and focus after Cmd+W, close_file and middle click. |
| close_all is unresolved | `workbench.rs:apply_window_layers` registers the window command; File > Close All Files invokes the existing sequential close adapter. | `r19_verify_close_all_is_registered_in_file_menu_and_stops_on_cancel` resolves the real menu row, saves, discards, cancels and then resumes. The state machine includes close_all. |
| Sustained-output timing includes test-thread scheduler stalls | `app_lane_b_round19_tests.rs:r19_app_sustained_build_output_has_steady_progress_and_lossless_tail` subtracts only oversleep beyond the requested 16 ms pause. Poll work and missed data polls still count. The <50 ms assertion, >=90% data polls, bounded output and lossless tail checks remain. | Same App-path sustained-output test, exercised with the full suite at six test threads. Raw excluded oversleep and adjusted UI gap are printed. |

Saving a known-path file now uses `app_tabs.rs:apply_save_action` and the same
`save_tab_with_path` helper in native and headless named/key dispatch. A native
window is still needed to choose an untitled path. This lets the regressions test
actual Cmd+S instead of bypassing command dispatch.

The verifier's state machine is adopted in `app_lane_b_round19_lazy_sweep_tests.rs`:
all presets/custom layouts, new/close pane, clone/split, focus/move/click/drag,
every close variant with Save/Discard/Cancel and failed saves, edits, filesystem
deletion and real startup restoration. The lazy variant does not pre-open hidden
restored sheets and asserts they remain lazy. Both assert one buffer identity per
document, exact close targets, no unexplained dirty-text loss, coherent focus,
workbench/session validation and final Hot Exit retention. Filesystem deletion
chooses documents with live contents: a clean, solely disk-backed lazy history
intentionally has no independent bytes with which to model deletion of its source.

No preference defaults changed. The public
[commands reference](https://docs.sublimetext.io/reference/commands.html#clone-file)
confirms shared-buffer clone semantics; the
[tab multi-select guide](https://www.sublimetext.com/docs/tab_multi-select.html)
does not specify the remaining selection after a single close. Preserving that
selection follows the user's requested fallback. The current commands reference
omits close_all; its command name and Close All behavior are confirmed by public
[Sublime forum usage](https://forum.sublimetext.com/t/close-all/3841). No Default
package, menu or keymap was downloaded, read or copied.

Workspace edits are limited to settings diagnostic collection/delivery, the
initiating save's report ownership and tab close selection. Lane A's file/text/
buffer/editor/layout internals and reload application/status-text logic are
unchanged.

## Verification of this pass

Logs, raw samples and sealed executables are under
`/tmp/velocity-r19-verification-fixes/`.

- `cargo fmt --all -- --check` and `git diff --check`: pass.
- `cargo clippy --workspace --all-targets --release --locked -- -D warnings`: pass (`clippy-final.log`).
- `RUST_TEST_THREADS=6 cargo test --workspace --locked --release`: **1,136 passed, zero failed, 29 ignored**, 114 suites (`workspace-final.log`).
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked`: pass (`rustdoc-final.log`).
- `scripts/check-release-login.py`: **all-targets release has no login-fixture markers** (`release-login.log`). This is a build/scan check, not the packaging script.
- Extended eager and lazy sweeps: **40 x 1,000 each**, 80,000 total operations; 11,475 exact-target closes, 6,269 prompts, 3,140 startup restores, zero unexplained text losses or focus mismatches (`sweeps-final.log`). Both ignored tests were run explicitly.
- Sustained App output: final captured module run delivered **149/149 data polls**, **23.30 ms** longest adjusted UI gap, excluding 608.93 ms total scheduler oversleep over the run (`round19-complete.log`). The same assertion passes in the six-thread workspace gate.

Nine alternating App pairs use identical harness source (SHA-256
`453d0633b38a50b1532d8583aca5f17bd0d8f489ff0dc96da75781316cf5046f`)
in every revision, native fonts, 200 warm keys and 1,000 measured keys. The
calling-thread counter measures dispatch, committed input/named commands,
changed_state and App::scene. No native window or GPU renderer is involved;
whole-process counters are retained separately in `app-per-key/report.json`.

| App case | 385dcdd instructions/key | c87df2e instructions/key | Current instructions/key | Current vs 385dcdd |
| --- | ---: | ---: | ---: | ---: |
| Single pane, one caret | 3,899,761 | 4,092,040 (+4.93%) | 3,823,645 | **-1.95%** |
| Two populated wrapped panes | 4,201,364 | 4,323,056 (+2.90%) | 4,216,951 | **+0.37%** |

Both pass the +1.5% App-path limit. These are the current measurements from this
harness; the verifier's earlier -9.4%/+0.5% used its different keystroke probe.

Nine paired whole-process runs against c87df2e pass the <=5% retired-instruction
limit: **features -0.24%, typing1 -0.27%, typing1000 -0.52%**
(`paired-headless/instructions.json`). The secondary wall-time runner initially
passes 17/18 metrics; single-caret summary p50 is +10.75% while the full gate is
running (median one-minute load 9.76 on 8 CPUs). The isolated recheck of that
single case is recorded below.

The nine-pair isolated typing1 recheck passes all three wall-time metrics:
p50 **-13.35%**, p95 **-1.75%**, p99 **-2.86%**. Retired instructions remain
**-0.21%** (`paired-typing1-isolated/report.json` and `instructions.json`). This
resolves the sole timing failure without changing code or weakening a gate.

Measured executable SHA-256 values:

- Shipping/all-targets binary: `077aaf4d24df5e441363e27086435872961b6ee1e8ba143c7b114ca74b0da54e`.
- App test executable: `8849ad611d1373e06ddfe20a86fb092ab0f2b26253cef878e40933130dbe22bd`.

Sealed copies match the final built files. All tests which may capture a login
environment install the runtime fixture provider; production benchmark children
use a private HOME/SHELL. Native dialogs/Finder/window checks remain unchecked in
`MANUAL_TEST.md` because GUI use is prohibited.


---

# Previous review pass (now in bdb5a7a)

The following records the earlier review and its measurements. Its initial HEAD
was `c87df2e`; the App-key baseline is `385dcdd`.
No GUI, packaging script, `cargo clean`, or `target/package-macos/` operation.
Before-review sources are archived at `/tmp/velocity-r19-review/before-review.tar`.
Lane A's file I/O, text/buffer, editor/editing, layout internals, workspace reload
paths and status text were not edited. M3 is implemented in `vel-config`'s resource
loader, before the existing reload publishes settings.

## Findings, implementations and user-path tests

Test names below are in `src/app_lane_b_round19_review_tests.rs`, except where a
file is named. All App tests use real key/named/mouse dispatch, actual buffers,
loaders and temporary data directories. Native dialogs supply the same decision
function with a UI answer; headless tests inject only that answer/path.

| Finding | Implementation (`file:function`) | Test |
| --- | --- | --- |
| H1 Vintage reveal | `app.rs:dispatch_key_binding` always captures `CommandStates::Single` for the active view before interception, including Vintage's Pending resolution. | `r19_review_vintage_pending_motions_and_search_reveal`: G, gg, 80 j motions, dd near EOF after parking the view, /search and n. |
| H2 invalid reduced layouts | `vel-ui/src/workbench.rs:Workbench::set_layout` focuses a received sheet when the target was empty. `normalize` repairs focus/selection/anchor, missing/duplicate references and orphan sheets before `workbench.rs:Workspace::snapshot`. Repairs are logged only. | `r19_review_reducing_layout_repairs_empty_receiver_and_keeps_single_selection`: exact keyboard repro, validation and stale-anchor checkpoint. |
| H2 Hot Exit and isolation | `session.rs:repair_window/collect/publish` repairs each stored window independently, remaps conflicting sheet identities, retains orphan document histories and falls back to a single group/default view metadata if validation still fails. `app.rs:prepare_quit` is the native quit decision path exercised headlessly. | `r19_review_quit_salvages_dirty_documents_and_other_windows`; `session::tests::r19_checkpoint_repairs_previous_window_without_blocking_next_publish`: broken references, invalid coordinates, conflicting IDs and two dirty windows. |
| H3 close one view | `app_tabs.rs:tab_close_sheets` limits close/close_file/close_by_index to the active/clicked sheet; only Close Selected closes the selected set. Closing a shared buffer's non-final view never asks. | `r19_review_single_close_and_middle_click_preserve_other_selected_sheets`; `r19_review_split_close_never_prompts_or_discards_original`; `r19_close_button_closes_only_clicked_selected_view`. |
| H4 empty splitting | `workbench.rs:split_view/clone_file` creates just one untitled view for an empty window. Split computes its insertion before cloning/mutation. | `r19_review_clone_is_single_selection_split_is_multiple_and_empty_is_atomic`, validating both commands and serialized sessions. |
| M1 Clone vs Split | `workbench.rs:workbench_action` separates clone_file (new view alone selected) from split_view (adjacent views selected). `Workbench::move_active` implements documented move_to_group behavior. Removed all five old round-18 select_single_tab fixture workarounds. | `r19_review_clone_is_single_selection_split_is_multiple_and_empty_is_atomic`; corrected `r19_split_multiselect_mouse_context_and_moves_share_buffer_not_selection`; existing round-18 anchor tests. |
| M2 layout selections | `Workbench::set_layout` retains the receiver's focus and one selection, rather than unioning removed groups' selections. | `r19_review_reducing_layout_repairs_empty_receiver_and_keeps_single_selection`. |
| M3 JSON error/non-modal/last-valid | `vel-config/src/settings_source.rs:retain_valid` preserves one last-valid version per physical settings source. Invalid data produces a resource warning naming the file and line; the existing resource-report channel displays it non-modally. Other valid sources continue to apply and repaired sources resume normally. | `r19_bad_user_json_reports_file_and_line_without_modal` (19 → invalid → 21); `r19_review_settings_errors_retain_each_file_and_other_valid_changes_apply`; corrected `fixed_user_json_reloads_after_an_in_place_repair` and `launch_with_nonexistent_readonly_and_corrupt_data_roots`. All require no modal messages. |
| M4 restored settings identity | `vel-session::SettingsSheet` persists resource identity and labels; `workbench.rs:sheet_state/restore_settings_tab/restore_window/prepare_sheet_inner` restores read-only defaults and syntax without confusing resources with writable files. | `r19_review_settings_identity_syntax_readonly_survive_real_session`: actual saved session, Default/User labels, JSON language and blocked edit. |
| M5 binding verification | The public macOS table confirms Cmd+Option+1…5 and Ctrl+1…4 / Ctrl+Shift+1…4. The requested additional bindings are not documented there; existing behavior is preserved, with an explanatory keymap source comment. No guessed bindings were added. | `r19_layout_bindings_and_group_commands_keep_views`; `r19_review_documented_macos_group_bindings_use_key_dispatch`. |
| M6 directory variables | `settings_entry.rs:apply_settings_action` uses `vel_build::Variables` for file/folder/project/packages expansion, rejects absent/relative directories and never creates paths. | `r19_review_open_dir_expands_window_variables_without_creating_paths`; existing Browse Packages test. |
| Browse Packages | `vel-platform/src/macos.rs:WindowHandle::show_directory` uses NSWorkspace openURL to open the folder itself. File reveal still uses Finder selection. | Headless Browse Packages/variable tests verify the expanded target; actual Finder presentation is native-only. |
| File menu order | `Main.sublime-menu` keeps New File first and puts Split View with the file/window view commands; all Split View menu/palette entries use split_view. | `r19_all_menu_palette_and_context_rows_resolve_except_incoming_line_endings`; Split/Clone command tests. |
| Settings syntax and labels | `workbench.rs:restore_settings_tab` assigns JSON syntax and distinct Default/User titles on creation and restore. | `r19_review_settings_identity_syntax_readonly_survive_real_session`. |
| close_pane geometry | `Workbench::close_pane` chooses the cell overlapping the removed cell after its area is absorbed, so tabs follow the growing cell. | `r19_review_grid_close_sends_tabs_to_absorbing_cell`; layout sweeps. |
| Unmodified/deleted contents | `workbench.rs:document_dirty/document_contents_dirty` reads the shared owner or lazy document. Close Unmodified uses the real dirty state. Close Deleted checks file existence and asks only for edited contents; ordinary close preserves the existing deleted-document prompt. | `r19_review_unmodified_uses_owner_and_deleted_checks_contents`; `r19_close_deleted_checks_disk_and_keeps_existing_and_untitled` covers cancellation and externally missing files; `r19_review_regular_close_preserves_deleted_document_prompt` preserves ordinary-close behavior. |
| Prompt test path | `app.rs:decide_close_document/save_tab_with_path` is shared by native can_close/save_tab and the injected test decisions. Save errors retain the tab; decisions remain sequential and cancel stops. | `r19_review_close_save_failure_uses_shared_decision_and_save_path`; `r19_close_variants_prompt_sequentially_save_decline_and_stop_on_cancel`. |
| Native font test gap | `app.rs:scene` initializes native font metrics during tests too; there is no cfg!(test) bypass. | `r19_review_scene_initializes_native_font_metrics`; benchmark asserts initialization in all revisions. |
| Sweep gaps | Added random every-preset/custom set_layout, split_view, clone_file, closes and focus on empty groups. After every operation check IDs, document identity and workbench validity; periodically validate a complete session. | `r19_review_layout_state_machine_with_layouts_splits_empty_groups_and_closes` (8×160); `r19_review_extended_layout_state_machine_40x500` (20,000 operations). |

## Confirmed behavior retained

The prior sustained-output transport, runtime login-provider injection, immediate
process-group cancellation, shared-buffer independent view state, no-clobber User
creation, default-side protection, live reload, customization templates and status
menus remain. Existing round-18/build/tab/settings tests run in the full gate.

- `output.rs` parses and trims outside the mailbox lock, with bounded publishing,
  lossless tails and total diagnostic counts. App-path sustained-output test is
  `r19_app_sustained_build_output_has_steady_progress_and_lossless_tail`; producer
  test is `round19_sustained_output_keeps_ui_polling_progress_with_regex`.
- `environment.rs:set_login_capture_provider` is runtime injection, with no
  test-login Cargo feature. `scripts/check-release-login.py` performs only an
  all-targets release build and scans the shipping binary; it does not package.
- Status popup menu rows reuse installed resources because Lane A's
  `panels::status_format_menu` and `set_line_ending` are absent at this HEAD.
  Resolution tests conditionally defer only the missing command; no stub exists.

## Public sources and preserved uncertainties

No Sublime Default package, menu or keymap was downloaded, read or copied. Sources
are cited beside functional behavior in code/resources:

- [Commands](https://docs.sublimetext.io/reference/commands.html): clone_file shares
  a buffer in the same group; close/close_file close the active view;
  move_to_group moves the current file; open_dir opens the default file manager.
- [Tab multi-select](https://www.sublimetext.com/docs/tab_multi-select.html) and
  [ST4 changelog](https://www.sublimetext.com/download): selected tabs display side
  by side; File/Split View uses multi-select. New Pane's selected-sheet handling
  remains as previously verified. No confirmation for closing/moving an entire
  selection through close_file/move_to_group: these commands act on one view.
- [macOS keyboard shortcuts](https://docs.sublimetext.io/reference/keyboard_shortcuts_osx.html#split-window)
  documents the column/grid shortcuts and groups 1…4. It does **not** document
  Cmd+Option+Shift+2/3, Ctrl+5…9 / Ctrl+Shift+5…9, or the requested Cmd+K New Pane,
  New Pane without Move, Close Pane and neighbor focus/move chords. Their current
  behavior remains; commands/menus remain available.
- [Settings](https://www.sublimetext.com/docs/settings.html) and
  [settings customization](https://docs.sublimetext.io/guide/customization/settings.html):
  default/User panes, settings files and platform naming.
  [Build variables](https://www.sublimetext.com/docs/build_systems.html#variables)
  define the shared expansion used by open_dir.
- [Scheme customization](https://www.sublimetext.com/docs/color_schemes.html#customization)
  and [theme customization](https://www.sublimetext.com/docs/themes.html#customization)
  define matching-name overrides. Legacy XML scheme conversion/customization is
  unconfirmed and retains its existing status message.

No preference default changed in this review pass. Corrected command semantics
are supported by the sources above; Default/User labels are original Velocity UI.

## Validation and performance

Final logs and sealed executable hashes are under `/tmp/velocity-r19-review/`.

- `cargo fmt --all -- --check` and `git diff --check`: pass.
- `cargo clippy --workspace --all-targets --release --locked -- -D warnings`: pass (`clippy-complete.log`).
- `cargo test --workspace --locked --release`: **1,129 passed, zero failed, 27 ignored**, 114 suites (`workspace-last.log`).
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked`: pass (`rustdoc-last.log`).
- `scripts/check-release-login.py`: all-targets shipping executable contains no login-fixture markers (`release-login-last.log`).
- Extended sweep: **40×500 passes** on the final candidate (`extended-sweep-last.log`).
- Sustained producer/App progress tests pass in the final full gate. Earlier isolated App run after the review changes delivered 135/135 data polls with a 24.50 ms maximum gap (`r19-tests.log`); transport code was unchanged by this review.

The App harness measures 1,000 keys after resource setup, native metric
initialization and 200 warm keys, through real dispatch, committed text or named
commands, changed_state and App::scene. Two populated shared-buffer panes assert
actual wrapped rows. There is no native window/GPU renderer. Identical harnesses
are injected into both baselines; comparisons alternate order for nine pairs.

The harness now uses calling-thread retired instructions (`thread_selfcounts`,
THSC_CPI) to isolate the App path, with the ABI checked against
[Apple's public XNU header](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/resource_private.h).
It also retains whole-process per-key counts including resource/analysis workers.
The initial process-only run varied with asynchronous work and reported +3.26%
for wrapped panes. App-thread profiling then identified repeated per-grapheme
selection searches in `View::scene`; frames containing only carets now skip those
searches. Selection rendering still uses the existing path. The always-captured
Vintage state remains active in all final measurements.

| App case | `385dcdd` instructions/key | `c87df2e` before | Final candidate |
| --- | ---: | ---: | ---: |
| Single pane, one caret | 3,834,354 | 3,988,856 (+4.03%) | 3,762,938 (−1.86%) |
| Two populated wrapped panes | 4,171,621 | 4,269,906 (+2.36%) | 4,196,507 (+0.60%) |

Both App-thread cases pass +1.5%. Whole-process per-key medians were
4,247,560 → 4,150,632 (−2.28%) and 9,047,123 → 9,030,947 (−0.18%).
Raw samples, phase counters and hashes: `app-last/report.json`. These are
fixture-specific retired-instruction measurements, not input-to-scanout latency.
The independent review's +0.9%/+0.6% used a different native-font probe.

Nine alternating paired whole-process runs against `c87df2e`: **features -0.28%**, **typing1 -0.51%**, **typing1000 -0.19%** retired instructions. All pass the 5% limit. The supplied paired runner also passes every wall-clock check. Evidence: `paired-last/instructions.json` and `paired-last/report.json`; the sealed binary equals the final all-targets shipping executable.


Reproduce the App pairing with `scripts/measure-app-keys.py`, supplying the
385dcdd/c87df2e test executables and `candidate-app-last` from the evidence
directory. The current ignored test is `app::lane_b_round19_perf::app_key_cost`.
Native verification remains unchecked in the first section of `MANUAL_TEST.md`.
