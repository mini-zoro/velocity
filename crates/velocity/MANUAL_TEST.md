# Round 20 Lane C review (native verification pending)

Use a separate test profile. Do not run a build or capture the login environment with
the reviewer's real profile. The release tests exercise the filesystem, command
dispatcher, menu model, image scene and two-window session state without AppKit.
The following checks require a native macOS desktop and remain unchecked:

- [ ] **File events and sidebar:** Open and expand a non-git folder. Create, rename,
  and delete regular files (including ignored binary output) via Finder or a terminal.
  Only the affected open listing should update. Repeat while an older listing is
  in flight and with a followed symlink. Option+Right expands descendant folders;
  Option+Left closes the subtree. Select a file, insert siblings earlier in the sort
  order, then confirm Enter and Shift-click still act from the selected file. Reveal a
  document outside a named project, then explicitly add its folder via a symlink.
  Only the explicit add should edit the project; check that the symlink path and
  project comments survive reopening. Hide Open Files, relaunch, and verify that
  setting was remembered for that window only.
- [ ] **Cascading and frames:** Open three windows. New Window should cascade from the
  front window. Move and resize each window, quit, and relaunch. Repeat after changing
  monitor resolution or disconnecting a display; frames should remain on-screen.
  Restore a session fixture with one absurd frame and one valid frame; both windows
  should open and quit normally.
- [ ] **Window menu:** Check Minimize (Cmd+M), Zoom, Bring All to Front, selection
  from the dynamic window list, and Cmd+` with several windows, including minimized
  ones. Native macOS tab merging is still disabled. Drag a dirty tab onto another
  Velocity window's tab strip and verify selection, Undo, and the destination's
  tab size and wrapping. With `close_windows_when_empty` enabled, repeat with the
  last tab in a folderless window, with Save and Close, and with Close Selected Tabs.
  They should close the empty window; a window with project folders should remain.
  Dragging onto the desktop to create a new window is unchanged and not implemented.
- [ ] **System recents/windowless File menu:** Open regular documents and check the
  Dock's recent documents. Set `update_system_recent_files` to false; the system list
  should clear once and stay empty on later opens. After closing every window, test
  File > New Window, Open and Open Recent, including canceling an Open dialog.
- [ ] **Services and Edit menu (unchanged):** Record whether AppKit adds Emoji &
  Symbols and Dictation when a Velocity window is active. Confirm the reference
  editor's Services/Hide Others/Show All behavior before implementing parity.
- [ ] **Proxy icon and edited dot (unchanged):** Confirm the reference editor's
  Cmd-click path and edited-dot behavior before enabling native document state.
- [ ] **Images and hex:** Open a transparent PNG, uppercase `.JPG` or `.PNG`, and GIF,
  then resize the pane. Switch quickly between large images, then add a folder; the
  folder edit should not wait for the decoder. Cmd+A on a hex or image tab, then open
  a text file, switch tabs, and jump back.
  Press Right, then Cmd+E, Cmd+Shift+E, Cmd+Option+G, and Cmd+Option+Shift+G.
  These text-only commands should leave the byte selection unchanged and report
  that text search is unavailable. From EOF, use Goto Line and enter a byte offset.
  In Cmd+P, try `:5`, `@`, and `#word`; these local text targets should not move the
  caret, but a regular file query should still work.
  Check aspect ratio, dimensions, read-only behavior and the fallback to hex
  with `image_file_patterns: []` or a damaged image.
- [ ] **Invisible characters:** Set `draw_unicode_white_space` to `"all"` and enable
  `draw_unicode_bidi` and `draw_control_characters`. Check hex labels for U+200B,
  U+202E and U+0001. Arrow across, select and copy each placeholder; the clipboard
  must contain the original character. Click on either side of a placeholder's midpoint:
  the caret should land on the corresponding side. Repeat with wrapping and the settings
  off, and compare NBSP and U+3000 under `"punctuation"` and `"all"`. With wrapping
  on, type and delete at multiple carets on lines containing placeholders. Compare
  the wrap breaks and tab alignment to those after reopening the same text.
- [ ] **Find in Files Where:** Search a project with Where set to
  `<project>, a/*/b/*.rs, -//vendor/`. The query should find `a/sub/b/main.rs`
  but not `a/b/direct.rs`. Try `[ab].rs` and `"{a,b}.rs"`; those are literal names.

Headless review timings (run sequentially with the fixture HOME/SHELL and no build
running during measurement):

```sh
cargo test --release --locked -p vel-search --test round20_pattern_timing -- --ignored --nocapture
cargo test --release --locked -p vel-search round20_random_file_pattern_comparison -- --ignored --nocapture
cargo test --release --locked -p vel-index --test round20_cold_timing -- --ignored --nocapture
cargo test --release --locked -p velocity round20_invisible_key_timing -- --ignored --nocapture
cargo test --release --locked -p velocity round20_invisible_multicaret_timing -- --ignored --nocapture
cargo test --release --locked -p velocity round20_hex_image_default_command_sweep -- --ignored --nocapture
cargo test --release --locked -p velocity r20_recursive_sidebar_timing -- --ignored --nocapture
```

For follow-up paired runs, use the same timing harnesses in a temporary export of `1ed95d4`.
Set `VELOCITY_INDEX_FIXTURE` to a disposable corpus path and reuse it for both
measurements; each run uses a fresh index cache. Alternate baseline and candidate,
check `uptime`, and compare CPU time or retired instructions when the machine is busy.
Use `VELOCITY_INDEX_WATCH=0` for the no-watcher control.

# Round 20 Lane A final review on b56986f

These fixes remain uncommitted on `b56986f`. This section supersedes the earlier
review notes below where they describe hash-first decoding or normalized legacy
proofs. Every build, test and probe uses a disposable HOME and SHELL; release
checks run in this worktree's own `target`. No GUI or packaging run is claimed.

The four choice/revision rules remain unchanged: only explicit encoding commands
change the choice; every completed load/save records raw identity plus effective
codec; changed bytes use BOM → explicit choice → valid UTF-8 → NUL-gated UTF-16 →
binary → fallback; main-format codecs belong to their stored revision, never an
explicit choice. Plain save and fallback do not promote an encoding to a choice.

Additional guarantees in this review:

- Keep My Edits invalidates the previous saved point unconditionally. Undo/redo
  cannot make old disk text appear saved against the acknowledged external revision.
  A successful save establishes the new saved point normally.
- Legacy normalized histories prove unchanged disk text only when their stored
  BOM and **every** decoded newline convention also match. Unknown or mixed
  representations cannot prove equality; a real format change conflicts.
- Reopen stages reading/decoding (or the binary task) before changing the choice,
  cancelling jobs or clearing a pending conflict. An unsuccessful read preserves them.
- Replacement decoding always retains its file-named non-modal notice, including
  unchanged restores and touches. Notices do not enter `workspace.messages`.
- A matching saved size/mtime starts progressive decoding with the revision codec
  immediately. The raw digest is computed during that same full streaming pass.
  A final digest mismatch discards the attempt and runs the changed-byte policy;
  any rare intermediate preview is replaced by the correct completed result.
  Changed stamps use the policy directly, with no whole-file hash pre-pass. If its
  final digest proves a touch, the revision codec still governs the accepted text.

The adopted independent harness is `src/app_round20_matrix_tests.rs`. It replaces
our older v3 adaptation, keeps the independent oracle and all original tables,
and adds the verifier's plain-save, unreadable recovery and Keep Edits tables.
Its documents are synthetic; the real `385dcdd` snapshot JSON/byte fixtures are
embedded and relocated into TempDirs. It never reads a verifier checkout or an
absolute fixture path. The cross products are ignored; small focused regressions
remain in the normal gate. Reports default to a process-specific temp directory.

```sh
# Use your fixture HOME/SHELL, CARGO_HOME/RUSTUP_HOME as required by the runner.
R20_MATRIX_REPORT="$TMPDIR/velocity-r20-matrix" \
cargo test --release --locked -p velocity round20_matrix -- \
  --ignored --nocapture --test-threads=1
```

`r20_matrix_probe_unchanged_large_restore_latency` measures the full headless
restore path from `restore_window` through the first content installed by the App
frame, and through final completion. It uses >100 MiB raw UTF-8 and Shift JIS
fixtures, with/without native history, three restores per case. To enforce the
≤1.5× parent median gate, set `R20_RESTORE_PARENT_REPORT` to the TSV produced by
the same probe in archived `3015ba2` (the parent before hash-first decoding).
Without that optional baseline it still asserts progressive, correct completion.

Focused regression tests:

| Finding | Test |
| --- | --- |
| Keep Edits then all undo/redo points, hot exit, save and touch | `r20_matrix_probe_keep_then_undo_stale_clean` |
| Legacy BOM/newline-only changes, open and while closed | `r20_matrix_probe_legacy_line_ending_and_bom_changes`; `r20_matrix_legacy_dirty_closed_format_change_conflicts` |
| Failed Reopen preserves choice, dirty text, revision and pending conflict | `r20_matrix_probe_failed_reopen_keeps_choice` |
| Replacement notices across reload/unchanged and changed restores | `r20_matrix_probe_replacement_on_reload_and_restore` |
| Same size/mtime with different raw bytes, both load modes and history settings | `r20_matrix_same_stamp_different_bytes_finishes_with_policy_text` |
| Progressive unchanged large restore latency | `r20_matrix_probe_unchanged_large_restore_latency` |

Native manual checklist (pending; sandbox has no GUI):

- [ ] Open `base\n`, type `+k`, rewrite externally as `base\nexternal\n`, Keep My
  Edits, undo and redo to every point. Dirty state and close prompting remain
  correct. Save and touch: saved state/text/undo remain stable.
- [ ] Restore a dirty main-format UTF-8/1252/Shift JIS/UTF-16 document after only
  its BOM or line endings changed. The edits remain and the conflict is offered.
- [ ] With a pending conflict, make its file unreadable and Reopen with another
  encoding. The original choice, text and conflict remain after failure.
- [ ] Open malformed Shift JIS fallback bytes, restart and touch: the file-named
  replacement notice stays non-modal and no alert appears.
- [ ] Restore an unchanged >100 MiB UTF-8 and Shift JIS file: early content,
  responsive frames and correct final encoding; undo still works afterward.
- [ ] Rewrite bytes while preserving size/mtime, then restore: the completed view
  contains the correct text and codec. A rare interim preview may be corrected.

Known limits retained: wrapped typing on one line of 100,000 CJK characters still
scales with line length (~3.1 ms/key versus main's ~6.7 ms); single-caret unwrapped
CJK was 15→74 µs over 1,000 keys. No typing/layout/rope path is changed here.

The adopted ignored suite passes **8,587 matrix cells / 18 tables / 0 failures**,
including the original 7,831 cells, 252 plain-save-choice cells, 288 unreadable
recovery cells and 216 Keep Edits cells. Its separate 12 restore timing cases also
pass. The original focused choice/revision, malformed fallback, dirty legacy BOM
and unreadable-placeholder regressions are retained in the normal gate.

Fresh archived-parent/current measurements (three warm restores per case):

| Raw fixture / native history | Parent `3015ba2` first / complete (ms) | Current first / complete (ms) | First ratio |
| --- | ---: | ---: | ---: |
| UTF-8, 136 MiB / with history | 8.9 / 494.4 | 8.9 / 498.2 | 1.000× |
| UTF-8, 136 MiB / without history | 8.9 / 321.8 | 8.9 / 323.5 | 1.000× |
| Shift JIS, 112 MiB / with history | 10.2 / 655.0 | 10.3 / 645.4 | 1.010× |
| Shift JIS, 112 MiB / without history | 10.3 / 470.6 | 10.4 / 468.7 | 1.010× |

These are medians on the full App restore/frame path; source fixtures are >100
MiB on disk. All first snapshots pass ≤1.5× parent. Compared with the reviewed
hash-first result (about 107 ms UTF-8 and 92–95 ms Shift JIS), early content is
restored. The parent archive's production loading sources match `3015ba2`.

Seven alternating whole-process Darwin `RUSAGE_INFO_V4` retired-instruction pairs
against frozen main `385dcdd`, sampled after child exit before reaping:

| Workload | Main median instructions | Current median instructions | Change |
| --- | ---: | ---: | ---: |
| `typing1` | 427,906,757 | 303,546,722 | −29.06% |
| `typing1000` | 10,640,129,340 | 4,143,179,768 | −61.06% |

Both pass the ≤5% regression gate. This includes setup, App input/editing,
background work and CPU scenes; it is not per-key latency or GUI presentation.
Runs began after builds and matrix work finished and inherited fixture HOME/SHELL.
No defaults or typing/layout/rope implementations changed.

The final gate passes in this worktree's own target directory: fmt; strict release
clippy for the workspace/all targets with `-D warnings`; **1,073 workspace tests
passed / 0 failed / 40 ignored**; rustdoc with `RUSTDOCFLAGS="-D warnings"`; and
**126,600 syntax assertions / 203 syntaxes / 242 files / 0 failures or skips**.
Lane B's preference reload-count test passed unchanged. The final release binary
SHA-256 is `652af5e5738cbaade100d028e216c6914931d80d0fa950bda9dddca81a97aca8`,
identical to the binary used for the paired typing measurements. No commit or
amend was made; native checklist items above remain pending.
Evidence: `/tmp/velocity-r20-final-review/verification.json`, `final/gate.json`,
`matrix-final/*.tsv`, `parent-report/probe_large_restore.tsv`,
`parent-provenance.json` and `instructions-final/report.json`. Earlier gate logs
in `gate-before-restoring-focused-tests` predate the four retained tests.

---

# Round 20 Lane A revision/choice model on 3015ba2

This section supersedes the earlier decode-policy record below. Work remains
uncommitted on `3015ba2`; no reset, amend or commit. Evidence and original-file
backups: `/tmp/velocity-r20-model-review/`. All builds, probes and test children
use fixture `HOME=/tmp/velocity-r20-model-review/fixture-home`, `SHELL=/bin/false`,
explicit Cargo/rustup directories and this worktree's own `target`. No GUI,
packaging script, `cargo clean` or access to `target/package-macos/`.

The four rules are:

1. Explicit Reopen/Save with Encoding (including aliases), and `set_encoding`,
   set the persistent choice. Plain save and fallback never create or change it.
   The displayed caption follows the effective format; it need not be the
   retained preferred codec after a fallback.
2. Each complete text load/save records raw byte count + BLAKE3 identity and its
   effective codec. Matching bytes decode with that revision's codec, independent
   of the choice. Touches preserve live text, selections, format and undo. Raw
   comparison also detects one changed CRLF among otherwise unchanged lines.
3. Changed bytes use BOM, then a strictly decodable explicit choice, valid UTF-8,
   NUL-gated UTF-16 evidence, binary, then configured fallback. Strong binary
   signatures override text choices. **NUL-bearing content counts as binary or
   UTF-16 evidence and may override a non-UTF-16 choice.** A malformed configured
   fallback uses replacement characters and a file-named non-modal notice.
4. Main-format sessions have no explicit-choice information. Their text codec belongs
   to the stored revision only. An unchanged disk stamp, or the saved text/digest
   retained by native history, validates that revision; otherwise detection runs.
   Unchanged dirty restores do not start a load that blocks focus-loss save.
   A legacy binary view's default UTF-8 label does not mask NUL/UTF-16 evidence.
   Changed small revisions (at most 8 KiB) validate before native history import;
   confirmed rewrites keep the existing history-discard behavior. Large same-size
   revisions validate off-thread with retained edits intact.

`vel-fileio::decide_decoding` remains the sole pure policy. The foreground loader
and progressive worker check a saved raw identity **before** choosing a decoder.
Ordinary opens need no extra pass. No last-effective speculative prefix is
published. Legacy text validation runs privately and cannot publish a prefix.
Binary terminal results, including a failed UTF-16 attempt, use `binary::Task`;
unsaved text is preserved on an external binary rewrite until explicit Reopen.
Known source size applies the large-file policy before the first prefix.
Load notices never enter `workspace.messages`. An unreadable restored tab remains
a watched, read-only error placeholder and retries when permission/timestamps
change. Hot-exiting an error placeholder again preserves its original native
revision proof and view ownership rather than serializing its empty buffer.
Choosing Keep My Edits acknowledges the exact external raw revision, so
a subsequent unchanged touch is quiet.

The App matrix is in `app_round20_revision_matrix_tests.rs`: the independent
verifier's 34 byte samples and 34 states, with real main-format JSON/bytes from
its archived `385dcdd` snapshot generator. The fixtures and generators are checked
in under `src/fixtures/main-385dcdd-matrix/`; session schema is not hand-edited.
Two additional dirty UTF-16/UTF-8 BOM sessions were generated afresh with main's
production snapshot code and exercise another 16 touch/undo cases. The shared
policy receives validated legacy saved-text proof even when BOM routing chose
the codec directly.
The oracle now implements these four rules rather than fallback/save promotion.
It asserts text, effective codec, persistent choice, exact save bytes, non-modal
notices and unchanged text/selection/undo through reload, touch, restart,
Reopen/Save variants, inactive activation and failed-read recovery. Its formerly
informational Keep My Edits re-prompt is now a failure. Dirty restores retaining
edits on binary disk changes are treated as protected text, not a harness failure.

Run the full matrix, including ignored large cross products, with fixtures:

```sh
HOME=/tmp/velocity-r20-model-review/fixture-home SHELL=/bin/false \
CARGO_HOME=/Users/zoro/.cargo RUSTUP_HOME=/Users/zoro/.rustup \
V3_REPORT=/tmp/velocity-r20-model-review/matrix-final-pass \
cargo test --release --locked -p velocity v3_ -- --include-ignored --test-threads=1
```

The regular workspace gate includes the smaller legacy restore, inactive-tab,
transient-preview, failed-read recovery and Keep My Edits regressions, plus
`r20_explicit_choice_and_revision_encoding_are_independent`,
`r20_malformed_fallback_revision_survives_touch_and_restart`,
`r20_main_dirty_bom_history_touch_keeps_undo`, and
`r20_main_error_placeholder_keeps_revision_proof_on_restart`. Existing round-20
expectations were updated where they asserted the superseded promotion model.
No editor, layout, rope or typing transaction implementation changes.

Native manual checklist (pending; this sandbox has no GUI):

- [ ] Choose Shift JIS, UTF-8 or UTF-16 LE; externally force a fallback, edit and
  plain-save. Touch/restart: text, effective encoding and undo stay unchanged.
  Write new bytes in the explicit chosen codec: it is tried again, and text is correct.
- [ ] Restore main-format auto 1252/UTF-16 and explicitly reopened Shift JIS/GBK
  sessions; touch dirty and clean tabs, including inactive tabs. Undo survives;
  a real external byte change still offers the normal conflict handling.
- [ ] Reopen a PNG/NUL binary as either UTF-16 order and restart in both load
  modes: streaming hexadecimal view and file-named status notice, no alert/blank page.
- [ ] Restore an unreadable file, restart again while unreadable, then grant
  permission and touch it: the tab recovers with its effective codec, original
  history and selection.
- [ ] Keep My Edits after a real rewrite, then touch: edits remain dirty and no
  repeated conflict appears. Use malformed Shift JIS fallback input: replacement
  text plus the same non-modal notice on foreground/background paths.
- [ ] Restore a several-MiB legacy-encoded revision after external UTF-8 rewrite:
  no mojibake preview appears. Verify encoding/line-ending status menus and checked rows.

Known limits retained: wrapped typing on one line of 100,000 CJK characters still
scales with line length (~3.1 ms/key versus main's ~6.7 ms); single-caret unwrapped
CJK was 15→74 µs over 1,000 keys. Neither path is changed in this review.

Final validation on this source:

- Fmt, strict release clippy with all targets, `cargo test --workspace --locked
  --release`, and rustdoc with `RUSTDOCFLAGS="-D warnings"` all pass in this
  worktree's own target directory: **1,071 tests passed, 0 failed, 31 ignored**.
- The independent syntax runner passes **126,600 assertions**, 203 syntaxes /
  242 files, zero failures or skips.
- The full App matrix passes **7,831 cells / 15 tables / 0 failed cells**. The
  additional 16 dirty-BOM touch/undo cells and repeated-unreadable-restart checks
  pass in the regular gate. Its legacy BOM-less UTF-16 row now asserts correct
  text/codec rather than merely logging the old binary-view default.
- `blocked_preference_queue_defers_one_reload_and_tracks_multiple_keys_per_write`
  is unchanged and passed this full run. The original focus-save, first-prefix
  large-file policy, and confirmed-rewrite history-discard tests also pass;
  the old restore-error assertion now checks the required status-only placeholder.
- No setting defaults or editor/layout/buffer/rope typing implementations changed.

Seven alternating retired-instruction pairs against frozen main `385dcdd`:

| Workload | Main median instructions | Current median instructions | Change |
| --- | ---: | ---: | ---: |
| `typing1` | 428,455,679 | 304,072,857 | **−29.03%** |
| `typing1000` | 10,708,611,441 | 4,225,845,118 | **−60.54%** |

Both pass the ≤5% regression gate. These are Darwin `RUSAGE_INFO_V4` counts
for complete headless child processes, measured after exit before reaping:
setup, editing, background work and CPU scenes are included. They are not
per-key latency or GUI measurements. Runs started after the builds and matrix
finished; all children inherited fixture HOME/SHELL.

Current candidate SHA-256:
`c6772b3a1edfbb20383184ef17d49ea79f2638389ea287d2d01a5904cb25871a`.
The frozen main binary is
`59b6864104f4f8f1a12f5b7aa6afcc95f5df6dc5a9e5ba75839b59b9cd142272`.

Final records (earlier logs in the parent directory are superseded attempts):
`/tmp/velocity-r20-model-review/validation.json`, `final/gate.json`,
`matrix-final-pass/*.tsv`, `matrix-report.json`, and
`instructions-final/report.json`. `main-generator-provenance.json` verifies
that the archived main production sources used for the additional fixtures
match `385dcdd`; the original 40 verifier fixture files remain byte-identical.

---

# Round 20 Lane A decode-policy review on a5fa68e

These changes are uncommitted on `a5fa68e` (`WIP round 20 lane A fixes 2`).
No reset, amend or commit. Evidence, scripts and original-file backups are in
`/tmp/velocity-r20-policy-review/`. Every build/test/probe and child uses fixture
`HOME=/tmp/velocity-r20-policy-review/fixture-home`, `SHELL=/bin/false`, explicit
Cargo/rustup paths, and this worktree's own `target` for the final gate. No GUI,
packaging script, `cargo clean`, or access to `target/package-macos/`.

`vel-fileio::decide_decoding(PrefixFacts, DecodeOptions)` is the sole pure
policy. Prefix facts come from at most 8 KiB; the policy does no IO, logging or
global diagnostic bookkeeping. Both streaming executors consume its strict
attempt list. Converted UTF-8 goes directly to the rope snapshot builder, with
no second content sniff or second BOM removal. Open, Reopen, both reload sizes, restore with/without native
history, `History::current_text`, and touch comparison all use that decision.
No typing, layout, rope, rendering or setting-default changes.

| Evidence/state | Decision |
| --- | --- |
| BOM | Try its codec first. A contradictory preference gets a file-named notice. Malformed BOM data retains the existing lossless configured fallback, with a notice and automatic provenance. |
| Recognized binary signature | Refuse text even with a preferred codec. Open/restore use the existing streaming hex task; reload/Reopen keep the live text. The same file-named status notice is used everywhere. |
| Explicit preference | Try it strictly before automatic detection. NUL-preserving non-UTF-16 codecs cannot turn NUL-containing binary/UTF-16 data into text. A failed preference remains separate from this revision's effective codec. |
| Automatic text | Valid UTF-8, then the UTF-16 heuristic only with NUL evidence, then binary rejection or the configured fallback. Pure CJK/short ambiguous UTF-16 without evidence is not guessed. |
| Unchanged raw identity | Decode with the last effective codec, accepting this speculative attempt only if the complete byte count and BLAKE3 digest match. Otherwise follow the ordinary decision. No hash pass precedes progressive snapshots. Touches preserve the buffer, selections, format and undo; a changed mtime alone does not invalidate clean restored history. |
| Successful save | The effective codec becomes the accepted preference when it differs from the previous choice. A later explicit pending choice supersedes that acceptance. Both the effective codec and save acceptance survive sessions. |
| Dirty hot exit | Persist the last saved/load raw digest independently of the dirty retained rope. A touch matches it; an actual byte change, including a single CRLF-to-LF conversion, conflicts. |
| Main-format session without provenance | Its stored encoding is a strong first-try hint after BOM/binary evidence. Explicit `false` in newer sessions remains automatic. The fixture is real main snapshot JSON, not a hand-written approximation. |

The table tests explicitly distinguish ambiguous bytes from determinable text:
a valid user-requested legacy reinterpretation wins. For example the GBK trap
sample is also valid UTF-8; without a matching explicit choice the specified
UTF-8 rule wins. A decoder cannot infer the original codec from those bytes.

| Finding | Implementation | Headless test |
| --- | --- | --- |
| One policy / B | `decoding::decide_decoding`, `load_cancellable`, progressive worker; explicit choice and valid UTF-8 precede the NUL-gated heuristic. | `decode_policy_fact_truth_table`; `decode_policy_exhaustive_inputs_and_streaming_table`; `r20_decode_policy_app_path_table` |
| A: save after fallback | `Workspace::save`, `Tab::decode_options`, session/history state accept the saved effective codec and retain native history on a proven unchanged restore. | `r20_save_after_fallback_accepts_effective_codec_touch_and_restart`; `r20_saved_effective_codec_does_not_mask_a_new_pending_choice` |
| C: dirty restored touch | `save_with_identity`, digest readers, session `Document::disk_content`, `Tab::unchanged_reload`. Hash raw bytes during existing read/write passes; no UI rope scan. | `r20_hot_exit_dirty_touch_uses_persisted_raw_identity` (foreground/background; saved or loaded identity; selection/revision, undo/redo and real-change conflict) |
| D: main-format Shift JIS | Legacy provenance defaults in `Document` and native `History`; ordinary restore uses the shared policy. | `r20_main_format_shift_jis_session_uses_legacy_hint` (foreground/background, with/without history) |
| Failed load recovery | `Tab::install` clears stale raw identity on a terminal failed load, while provisional prefixes retain their baseline. | `r20_failed_background_restore_can_recover_the_original_revision` (with/without native history) |
| One CRLF changes | Raw identity replaces normalized text plus dominant-ending comparison. | `r20_one_mixed_line_ending_change_is_a_disk_change` |
| Notice wording | `DecodeNotice::message` supplies the same file-named non-modal wording; automatic binary notices never claim a stored choice. | `r20_decode_policy_app_path_table`; existing `r20_load_override_notices_are_status_only_and_identical_across_paths` |

Policy coverage: **960** combinations in the fact truth table; **2,016** actual
foreground/background decodes (21 byte samples × eight preferred/effective
states × two save states × three fallbacks × two executors), plus equal/different
raw identity decisions. Boundary samples include NULs after the 8 KiB sample,
UTF-8 BOM plus NULs, and a literal U+FEFF following the source BOM; both executors
use the same bounded detection rule. The App table covers **1,344** path cases over the same
21 samples and eight document states: foreground/background reload, clean
restore with/without native history, and open/Reopen. Its disk-backed native
histories also call `current_text` through the public history API. All App cases
check that notices bypass `workspace.messages` and native/modal overlays.

Legacy fixture provenance: `src/fixtures/main-385dcdd-shift-jis-session.json` was
emitted by main's unmodified `Workspace::snapshot` and serde serializer in a
fresh `git archive 385dcdd`. The exact generator and reproduction notes live next
to it. The test only relocates `Document.path` after deserialization, retaining
the original JSON keys and history payload.

Final verification on this source:

- `cargo fmt --all --check`.
- `cargo clippy --workspace --all-targets --release --locked -- -D warnings`.
- `cargo test --workspace --locked --release`: **1,062 passed, 0 failed,
  21 ignored**, including the full policy tables and recovery regression.
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked`.
- Syntax runner: **126,600 passed / 0 failed** (203 syntaxes, 242 files).
- The unchanged Lane B preference reload-count test passed this full run.
- The worktree's release build passed. Final gate logs are in
  `/tmp/velocity-r20-policy-review/final/`; scripts and the original-file backups
  are in its parent directory.

Seven alternating paired retired-instruction runs against the verified frozen
`385dcdd` binary, with fixture HOME/SHELL and no concurrent compilation:

| Workload | Main instructions | Final instructions | Change |
| --- | ---: | ---: | ---: |
| typing1 | 429,429,071 | 304,318,369 | -29.13% |
| typing1000 | 11,506,969,159 | 4,421,372,528 | -61.58% |

Darwin RUSAGE_INFO_V4 counters cover the complete headless child process after
exit and before reaping: setup, editing, background threads and CPU scenes.
These are whole-process instruction medians, not per-key App latency or native
frame timings. All samples, executable hashes and host loads are in
`/tmp/velocity-r20-policy-review/instructions-final/report.json`.
`editor.rs`, `editing.rs`, `layout.rs`, `vel-buffer` and `vel-text` have no diff
from `a5fa68e`. No new flatness claim is made for the known CJK limits below.


Native checklist (pending; no GUI available):

1. Reopen Shift JIS, externally replace it with UTF-8 `naïve café ✓!`, delete
   ` ✓!` and save. Touch and restart: text/checked encoding remain UTF-8 and
   undo survives. Repeat below/above the large-file threshold.
2. Hot-exit a dirty tab, restore and touch its unchanged file. Verify no conflict,
   unchanged caret/selections and working undo/redo. A real rewrite must conflict.
3. Change only one CRLF to LF in an otherwise CRLF file behind a dirty tab. The
   conflict must appear even though the text and dominant convention match.
4. Restore a main-format Shift JIS session. Verify Japanese text, clean state,
   caption and subsequent reload. Repeat with lazy/shared tabs.
5. Try the 1252/GBK/UTF-8 heuristic trap samples, BOM overrides and PNG replacement.
   Check correct chosen/automatic decoding and file-named status notices with no
   alerts; binary reloads retain text and binary restores display the hex view.
6. Restore large UTF-8/Shift JIS files: the first progressive prefix must remain
   prompt. Keep the existing many-caret and fold/scroll native checks below.

Known limits retained: the verifier measured roughly 3.1 ms/key on a single
100,000-character wrapped CJK line (main 6.7 ms); one-caret unwrapped CJK grew
15–74 µs over 1,000 keys. No further typing optimization or new flatness claim is
made here. Older records below are historical, including superseded decode and
normalized-touch descriptions.

## Previous round-20 record (committed as a5fa68e)

# Round 20 Lane A follow-up on 123af57

The remaining review fixes are uncommitted on `123af57` (`WIP round 20 lane A
fixes`). No reset or amend. Evidence, scripts and original-file backups are in
`/tmp/velocity-r20-final-review/`. Every build/test/probe and its children use
fixture `HOME=/tmp/velocity-r20-final-review/fixture-home`, `SHELL=/bin/false`,
explicit Cargo/rustup directories, and this worktree's own `target`.
No GUI, packaging, `cargo clean`, or `target/package-macos/` access.

| Finding | Final behavior and implementation | Headless App-path test |
| --- | --- | --- |
| 1 — failed explicit decode | `vel-fileio::load_cancellable` and the progressive worker return a diagnostic when detection/fallback takes over. `Tab::record_decode` marks the current effective codec as automatic. `Tab::preferred_encoding` and session `Document::preferred_encoding` retain the original user choice independently, including shared/lazy views and explicit-save cancellation. Old sessions without the new field still use their existing provenance. BOM and clear BOM-less UTF-16 evidence can override a legacy/UTF-8 preference for that revision. The next reload retries the retained preference. | `r20_failed_decode_keeps_preference_across_rewrites_and_restore`: Shift JIS → UTF-8 → BOM-less UTF-16 → Shift JIS → UTF-8; explicit UTF-8 → Windows-1252 fallback → UTF-16 → UTF-8. Foreground/background and restart during fallback; current codec/provenance, retained choice, notices, clean state and session wire are checked. |
| 2 — modal notices | `Workspace::load_notices` is consumed by `Panels::poll` using `set_message`; it never reaches `show_error`/native alerts. File-worker errors, reload errors and BOM/binary/fallback notices share this status path. Foreground/background errors use identical path formatting. Automatic binary rejection does not claim a stored choice was ignored. Earlier tests no longer accept the modal queue as evidence of a notice. | `r20_load_override_notices_are_status_only_and_identical_across_paths`: automatic/explicit binary rejection, BOM override, foreground/background wording and restore; checks status text, empty `workspace.messages` and no overlay. Finding 1's test also requires status-only fallback diagnostics. |
| 3 — large-file touch | `load_background_for_reload` compares the decoded text with `ChangeTracker::saved_text` on the worker: byte length first, then exact chunk equality. This avoids hash collisions and UI scans. External reloads publish no temporary prefixes; an unchanged decoded tree is retired on the worker. Completion uses `Tab::unchanged_reload` to acknowledge only stamps, preserving the buffer ID/revision, selections and all history. Dirty edits and pending format edits remain intact. Foreground no-op events follow the same rule. | `r20_large_file_touch_keeps_clean_and_dirty_history_and_selections`: actual mtime-only changes through the watcher, small/forced-large paths, clean/dirty tabs, auto-reload on/off, buffer/revision/view/selection identity and undo/redo; same-sized real rewrites still reload or conflict. |

No setting defaults, layout internals, rope implementation, rendering or typing
algorithms changed in this follow-up. The documented unknown Hexadecimal reopen
value and automatic UTF-8 save fallback remain unchanged.

Known timing limits retained deliberately:

- A single long wrapped CJK line still has per-key growth with line length. The
  verifier measured about **3.1 ms/key at 100,000 CJK characters**, versus **6.7 ms**
  on main. This is a verifier observation, not a fresh measurement here. Per the
  review instruction, no further wrapped-line optimization was attempted.
- One-caret unwrapped CJK typing grows from about **15 to 74 µs/key** over 1,000
  keys despite beating main. No small isolated fix was established in this
  follow-up; the existing input/rope behavior is retained. This is not a flatness
  claim. Earlier full-App measurements and their exact boundary remain below.

Native checklist (pending; no GUI available):

1. Explicitly reopen Shift JIS, rewrite through UTF-8, BOM-less UTF-16 and Shift
   JIS again, restarting during fallback. Check correct text/current codec and
   that the original choice is retried. Repeat after choosing UTF-8.
2. Reload files with a conflicting BOM or PNG content. All notices must appear
   in status text without an alert. Automatic binary notices must not mention a
   stored choice. Restore a binary replacement into the existing hex view.
3. Touch a large clean file with saved undo, then a dirty file. Verify unchanged
   text, caret/selection, undo/redo and no disk-change prompt. Repeat with
   auto-reload disabled and with pending encoding/line-ending edits.
4. Rewrite actual content using the same byte count. Verify clean auto-reload,
   or the existing conflict UI for dirty/opt-out tabs, including shared views.


Final verification on this source:

- `cargo fmt --all --check`.
- `cargo clippy --workspace --all-targets --release --locked -- -D warnings`.
- `cargo test --workspace --locked --release`: **1,053 passed, 0 failed,
  21 ignored**, including doctests and the three new App regressions.
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked`.
- Syntax runner: **126,600 passed / 0 failed**, 203 syntaxes, 242 files.
- Explicit model reruns: the existing rope model ran 5,000 seeds plus 1000493,
  40 batches each; the layout model ran 2,048 seeds plus 1000493 and 23028,
  24 transactions × five widths × two indent settings (492,000 comparisons).
  Both exceed the requested 2,000 seeds and passed without changing model code.
- Lane B's existing tests are unchanged. Its previously reported preference
  reload-count test passed this full gate too.

Seven alternating paired retired-instruction runs against the frozen clean
`385dcdd` binary, with fixture HOME/SHELL and no concurrent compilation:

| Workload | Main instructions | Follow-up instructions | Change |
| --- | ---: | ---: | ---: |
| typing1 | 428,134,308 | 303,578,147 | -29.09% |
| typing1000 | 10,653,859,480 | 4,147,375,201 | -61.07% |

Darwin RUSAGE_INFO_V4 counters cover the complete headless child process after
exit and before reaping: setup, editing, background work and CPU scenes. They
are not per-key App timings or native latency. No new flatness claim is made for
the two known limits above. `instructions/report.json` retains all samples,
executable hashes and host loads. Gate/model logs, the exact scripts and source
fingerprints are in `/tmp/velocity-r20-final-review/`. The final gate uses this
worktree's own target directory; no defaults, layout or rope code changed.

## Previous round-20 record (committed as 123af57)

# Round 20 Lane A review fixes on b9f5dcf

The review fixes are uncommitted on `b9f5dcf` (`WIP round 20 lane A`), based on
main `385dcdd`. No reset or amend was used. Evidence and original-file backups are
in `/tmp/velocity-r20-review/`. All tests, probes and their children use fixture
`HOME=/tmp/velocity-r20-review/fixture-home` and `SHELL=/bin/false`; Cargo and rustup
retain their explicit toolchain directories. The gate uses this worktree's own
`target`. No GUI, packaging script, `cargo clean`, or `target/package-macos/`.

No preference default changes in this review. UTF-16 captions without “with BOM”
now write without a BOM, including encoding arguments using codec names. The
existing strict encode error remains; UTF-8 recovery is explicit and non-modal.
Pure BMP CJK without a BOM or byte-order evidence is deliberately not guessed as
UTF-16. The preceding round's source references/default decisions remain below.

| Finding | Fix | Headless test |
| --- | --- | --- |
| H1 | `Tab::encoding_chosen`, `decode_options`, session `Document` and disk-backed `History` retain encoding provenance. Automatic tabs redetect after rewrite/restore; chosen codecs yield to BOM/binary with a diagnostic. Shared and lazy views retain the choice. | `r20_detected_encoding_redetects_external_rewrites_and_restore`; `r20_user_choice_yields_to_conflicting_bom_and_binary`; `r20_explicit_save_aliases_persist_encoding_provenance`; existing user-choice survival tables |
| M1 | `hex::detect_utf16` scores both orders, validates bounded surrogate samples, and permits form feed/tab/CR/LF. ASCII and valid surrogate evidence must clearly favor one order; pure CJK remains ambiguous. | `bomless_utf16_review_surrogate_endianness_and_controls_table`; existing PNG/binary negatives and byte-identical round trips |
| M2 | `load_background_with_options` streams forced UTF-8 directly and other codecs through encoding_rs. Clean restored history attaches to the completed snapshot without rereading the file. History view IDs and soft undo survive. | `r20_large_restores_publish_utf8_and_shift_jis_prefix_before_completion` (136 MiB UTF-8 / 112 MiB Shift JIS, with/without native history); `r20_clean_history_restore_preserves_soft_undo_and_saved_typing_undo` |
| M3 | `Line` has its original 40-byte representation and `Arc<[Segment]>`; final-grapheme and lookahead metadata are separate and allocated only for wrapped rows. `Segment` also has its original 40-byte geometry. A single wrap dependency stays inline; fully hidden unwrapped rows share one empty slice, avoiding per-row allocations and frees. Stable-count edits mutate unique slices; moved breaks rebuild only edited lines. | `ordinary_line_metadata_remains_forty_bytes`; clean-main paired `--headless-view-features`; full-App flatness gate |
| M4 | `encoding_format` derives BOM solely from the name; checked menu rows preserve bytes. | `r20_menu_encoding_bytes_and_selected_alias_survive_restore`: four UTF-16 and two UTF-8 variants, save/reopen/click-current bytes |
| L1 | `tree::repair` chooses a boundary that fits both leaves, using the ordinary builder if no two-leaf boundary exists. | `merged_multibyte_leaf_split_seed_1000493`; 5,000-seed String/query/snapshot model plus seed 1000493 |
| L2 | `rewrap_suffix` binary-searches prefix maxima of exact lookahead, including non-adjacent dependencies and oversized extending graphemes. | `nonadjacent_grapheme_lookahead_matches_full_layout`; `oversized_grapheme_lookahead_seed_570`; expanded 2,048-seed layout model |
| L3 | `ColumnCache` keeps tab-expanded checkpoints per line/revision, rebases them through changes and scans only invalidated suffixes during typing; its status-only storage is boxed outside the scene data. No 16 KiB switch to scalar-only columns. | `r20_status_columns_remain_tab_expanded_across_long_line_and_edits`; existing status table updated to the correct long-line value |
| L4 | `Format::display_name` preserves alias captions and session restores retain them. | `r20_menu_encoding_bytes_and_selected_alias_survive_restore`; `r20_explicit_save_aliases_persist_encoding_provenance` (ISO 8859-1 and ISO 8859-9) |
| L5 cancel | Failed explicit saves restore the previous format and provenance before offering the cancellable recovery. | `r20_failed_explicit_save_cancel_restores_format_and_save_all_recovers_each_tab` |
| L5 save-all | Encoding failures queue one recovery per tab; accepting or dismissing advances to the next offer. | `r20_failed_explicit_save_cancel_restores_format_and_save_all_recovers_each_tab` |
| L5 minimap | Retain the existing cheap insertion path: the next analysis publication supplies fresh width. No synchronous document scan added. | `r20_pure_insert_minimap_width_is_refreshed_by_next_publication`: new revision, correct width, within one second |
| L5 soft redo | `set_history_limits` bounds selection redo immediately, outside the typing path. | `reducing_history_limits_bounds_selection_soft_redo` |
| Equal-start ChangeSet | Insertion plus replacement at the same start takes the full layout fallback. | `equal_start_insert_and_replace_map_uses_full_geometry` |

The rope model runs 5,000 seeds plus 1000493, each with 40 batches; it checks
String content, all byte/char/UTF-16/line conversions, chunks, slices, summaries,
invariants and immutable snapshots after each batch. The layout model adds the
emoji modifier to its edit tokens and includes seeds 1000493 and 23028 in addition
to 0–2047: 492,000 incremental/full comparisons, over widths 1, 2, 7, 13 and 80
with both indentation settings. Exact tests retain the review's original
lookahead repro and the newly exposed seed-570 oversized-grapheme dependency.
Dense, fold/phantom and Lane B's existing geometry tests are included in the gate.

The unchanged Lane B test
`blocked_preference_queue_defers_one_reload_and_tracks_multiple_keys_per_write`
passed the review-fix full workspace run. The preceding investigation found one
failure in three isolated runs, so its flakiness is not limited to full-suite
load. It remains assigned to Lane B; no edit was made to it.

Native checklist (not run: no GUI is available):

1. Open detected UTF-16 BOM and Windows-1252 files, transcode externally to UTF-8,
   and verify redetection. Replace one with a PNG: retain the current text on
   reload with a binary diagnostic, and restore as hex on next launch.
2. Explicitly reopen Shift JIS/ISO aliases, rewrite externally, and restart.
   Verify the choice and checked caption survive. A conflicting UTF-8 BOM must
   take precedence with a non-modal message; restoring must not dirty the tab.
3. Save each UTF-16/UTF-8 BOM variant from the encoding status menu and click the
   checked row again. Verify the disk BOM and status caption stay consistent.
4. Fail an explicit Windows-1252 save containing emoji, press Escape and verify
   the original encoding remains. Run Save All with two failing tabs and accept
   or dismiss each UTF-8 offer independently.
5. Restore large UTF-8 and Shift JIS files. A read-only prefix must appear while
   loading continues, then become editable with selections and undo intact.
6. Move/type past a 16 KiB tab prefix, change tab width, and undo. Columns must
   stay tab-expanded. Confirm minimap width catches up after ordinary insertion.
7. Fold/unfold, scroll, rewrap and type code/CJK at many carets. Check wrap/fold
   geometry, one typing-run undo step and the existing soft-undo boundaries.

Lane B's menu integration is still required: View › Line Endings has Unix,
Windows and Mac OS 9 rows using `set_line_ending {type: unix|windows|cr}`. Encoding
status clicks use Reopen with Encoding / Save with Encoding, all codec captions
from `ENCODINGS` (BOM variants and ISO aliases included), with one checked row;
line-ending status clicks use the same three ending rows.


Final review verification (supersedes the historical results below):

The exact review sources passed `cargo fmt --all --check`, release workspace
clippy with `-D warnings`, release locked workspace tests, warning-free workspace
docs, and the syntax runner. Workspace: **1,050 passed / 0 failed / 21 ignored**;
syntax: **126,600 passed / 0 failed** over 203 syntaxes and 242 files. The expanded
rope and layout models above ran as part of this gate. Logs are `gate-*.log` and
`gate.json` in `/tmp/velocity-r20-review/`.

The ignored full-App timing test passed again with the identical timed loop on
clean `385dcdd`. Samples include key binding dispatch, text input/command routing,
changed-state handling, workspace and panel polling; exclude native delivery,
painting, GPU and scanout. No compilation ran concurrently. First/last numbers
below are medians of keys 1–100 and 901–1000, in milliseconds per key.

| 10,000 carets | Main first / last | Review first / last | Rule |
| --- | ---: | ---: | --- |
| code/wrapped | 26.9057 / 298.5014 | 8.1601 / 11.7643 | ratio 1.442 ≤1.5 |
| code/unwrapped | 1.0287 / 2.0795 | 1.0125 / 1.2498 | ratio 1.234 ≤1.5 |
| cjk/wrapped | 60.1755 / 455.5301 | 13.9363 / 19.6756 | ratio 1.412 ≤1.5 |
| cjk/unwrapped | 22.1701 / 31.5546 | 1.3088 / 2.1345 | unwrapped +0.8256 ms ≤1 ms |

| One caret | Main first / last | Review first / last |
| --- | ---: | ---: |
| code/wrapped | 0.01408 / 0.06679 | 0.01021 / 0.01438 |
| code/unwrapped | 0.01379 / 0.06542 | 0.01154 / 0.01963 |
| cjk/wrapped | 0.01554 / 0.10063 | 0.01250 / 0.04354 |
| cjk/unwrapped | 0.01729 / 0.10767 | 0.01650 / 0.07554 |

All one-caret matching blocks remain within main +5%; all many-caret first
blocks are no slower than main. Existing run undo and soft undo/redo assertions
run after the timed keys. Data: `flatness-main.json`, `flatness-candidate.json`.

Seven alternating whole-child retired-instruction pairs (Darwin RUSAGE_INFO_V4,
read after exit before reaping; includes setup/editing/background work/scenes):

| Workload | Main | Review | Change |
| --- | ---: | ---: | ---: |
| typing1 | 427,953,788 | 303,757,355 | -29.02% |
| typing1000 | 10,638,667,451 | 4,141,190,267 | -61.07% |

Seven alternating `--headless-view-features` pairs against the frozen clean main
binary, using the same Rust fixture. These are CPU scene/command timings, not
native latency. Fold/unfold includes the named-region command and projection
rebuild; scroll waits for exact asynchronous styles before timing; highlighted
typing includes edit plus CPU scene; rewrap excludes the transaction itself.

| Metric | Main p50 ms | Review p50 ms | p50 change | p95 change | p99 change |
| --- | ---: | ---: | ---: | ---: | ---: |
| Scroll | 0.298520 | 0.303895 | +1.80% | -6.07% | -4.29% |
| Fold | 9.625187 | 9.356542 | -2.79% | -0.67% | -0.98% |
| Unfold | 0.902187 | 0.851875 | -5.58% | -5.92% | -9.55% |
| Rewrap | 0.008208 | 0.002000 | -75.63% | -75.38% | -67.26% |
| Highlighted typing | 0.821374 | 0.558458 | -32.01% | -54.80% | -53.88% |

Every final feature P50/P95/P99 is within the 5% limit or improved. The feature
report stores all pairs and executable hashes. Earlier failing runs remain in
the review evidence directories and are not presented as passes. Two strict
App attempts on this final binary failed during variable host load at different
workloads; they remain in `shared-hidden-rows-flatness-contention` and
`shared-hidden-rows-flatness-second`. The final complete run passed the original
assertions; no threshold, key count or input-path step was changed.

Existing vel-text benchmarks ran seven alternating pairs against main across 24
operations (100 MB ASCII, 1 GB ASCII, 100 MB Unicode). Every per-operation retired
instruction count is within +3%; maximum +1.83% for `ascii-1gb/sequential_uniform_100000`.
Complete workers are within +1.95%. The five vel-largefile budget B
workers are within +0.59%. Their code/binaries did not change during the
subsequent layout/status-cache refinements; source hashes are retained. Reports
include wall/CPU samples too: host load reached 12–21 on an 8-CPU machine, so the
requested busy-host instruction comparison is the regression gate, not a claim
that every wall-time percentile met 3%.

All probes and tests use the fixture HOME/SHELL declared above. Reproduce the
full-App gate after the workspace release test build, without concurrent builds:

```sh
env HOME=/tmp/velocity-r20-review/fixture-home SHELL=/bin/false \
  CARGO_HOME=/Users/zoro/.cargo RUSTUP_HOME=/Users/zoro/.rustup \
  VELOCITY_R20_TYPING_BASELINE=/tmp/velocity-r20-review/flatness-main.json \
  cargo test --release --locked -p velocity r20_typing_cost_stays_flat -- \
  --ignored --nocapture --test-threads=1
```

## Original round 20 implementation record (b9f5dcf)

The section below is historical. Its pre-rope scroll/features comparison already
contained the layout change and does **not** establish the clean-main fold/unfold
budget. The review-fix comparison and final gate results above supersede it.

# Round 20 Lane A: encodings, reloads, status text and typing cost

Started clean on `lane-a` at `385dcdd`; no commit was made. The approved history,
layout and rope extensions are applied in this worktree. Original backups,
matching-main measurements and logs are in `/tmp/velocity-lane-a-r20/`.
No GUI, packaging script, `cargo clean` or `target/package-macos/` was used.

| Item | Implementation (file:function) | Headless user-path test |
| --- | --- | --- |
| A1 | `workspace.rs:Tab::decode_options/Workspace::poll` decode reloads with the tab's codec first, including background loads, and detect only after decoding fails. | `r20_external_rewrites_keep_encoding_and_respect_reload_opt_out`; fileio preferred-codec table |
| A2 | `workbench.rs:prepare_sheet_inner` uses the stored codec; the approved one-call change in `vel-session/history.rs:current_text` also covers saved history. Restore does not manufacture an edit. | `r20_session_restore_uses_stored_encoding_without_becoming_dirty`: serialized sessions with/without history and synchronous/worker loads, including Windows-1252 bytes that are also valid UTF-8 |
| A3 | `hex.rs:detect_utf16` checks at most 8 KiB for one-sided NULs, valid surrogates and text control characters; fileio shares it. | `bomless_utf16_detection_and_roundtrip_table`; `r20_app_encoding_roundtrip_table_and_save_defaults`: LE/BE, BOM choices, all endings, PNG/binary negatives |
| A4 | `app.rs:format_action/offer_utf8_save` retains strict errors and presents an explicit non-modal **Save as UTF-8** choice. | `r20_unrepresentable_save_offers_explicit_utf8_recovery`: bytes/codec retained until Enter accepts recovery |
| A5 | `vel-commands` registry and `app.rs:format_action` support public `reopen`, `save {encoding}`, `set_line_ending` aliases and checked state. | `r20_encoding_and_line_ending_command_dispatch_table`, including a copied user key binding |
| A6 | `vel-fileio:Format::display_encoding/LineEnding::display_name`, `panels.rs:status_position/status_format_menu`, and `vel-ui/status.rs` supply BOM captions, platform line-ending names and popup models. | `r20_status_text_and_format_menu_table`; existing status selection table |
| A7 | `vel-fileio:DecodeOptions/DecodeReader::ending` and `progressive.rs` use `default_line_ending` only when there are no breaks. | `no_line_break_uses_the_configured_convention_in_every_decoder`; App first-newline save check |
| A8 | `vel-config:EditorSettings::default` and our preferences set `trim_only_modified_white_space` to true. Explicit false keeps trim-all behavior. | `default_trimming_preserves_untouched_lines`; App save-default check; existing trim-all test explicitly selects false |
| A9 | `workspace.rs:Workspace::poll` sends a clean tab with reload disabled through the existing external-change prompt. Reload is not a user edit. | Real external rewrite table: sync/worker, true/false, dirty/clean protection and no new jump origin |
| A10 | Unchanged: current documentation/ST4 changelog did not confirm the proposed Hexadecimal reopen value/menu entry. Existing binary opens use `binary::Task`. | Existing binary engine/user-path tests; no speculative behavior asserted |
| B1 | `vel-ui/status.rs:column` expands tabs for status text, counts CJK scalars once and uses the scalar summary past the existing 16 KiB prefix limit. | Exact tab-size 4/8, CJK and 128 KiB tab-prefix status table |
| C | `layout.rs:change/rewrap_suffix` retains stable wraps and rechecks the final grapheme; `editing.rs:publish_direct_change` preserves valid publications and shared-view replay; `vel-text/tree.rs:edit_node/Leaf::seek` shares bounded fragments and avoids redundant Unicode scans; `vel-buffer:transact/clear_redo/prune` verifies edit-end carets and avoids redundant history walks. | 2,048-seed layout and String models, dense/fold/phantom and immutable-snapshot oracles, App undo/soft-undo and full-App timing gate; existing rope/buffer, scroll and large-file tests |

Only the trimming default changes existing behavior. `reload_file_on_change: true`
is now explicit and preserves automatic reload as the default. Both are supported
by the [ST4 Build 4107 changelog](https://www.sublimetext.com/download).
The [settings reference](https://docs.sublimetext.io/reference/settings.html#system-and-miscellaneous-settings)
confirms automatic UTF-16 detection and `default_line_ending`; the heuristic is our
own. The [commands reference](https://docs.sublimetext.io/reference/commands.html)
confirms the aliases. The [menu guide's status-bar animation](https://docs.sublimetext.io/guide/customization/menus.html#available-menus)
confirms Unix / Windows / Mac OS 9 and the format popup sections. The changelog's
encoding-restore fix is in **4213**, not the spec's 4200. Build 4152 supports the
inference that status columns expand tabs. Relevant implementation comments cite
these sources. Automatic UTF-8 fallback on an encode error could not be confirmed;
strict failure plus an explicit recovery choice preserves today's behavior.

The typing profile found repeated soft-history walks, growing physical-line scans
in wrapping/minimap/guides, copied batch leaves, repeated Unicode summaries for
endpoint checks, and a growing pass over resulting caret fragments. Ordinary undo
and soft undo keep their existing boundaries, including word/space/punctuation runs
and CJK. Temporary stage probes were removed. Native `/usr/bin/sample` was unavailable;
the stage evidence is instrumented CPU work, not a native latency claim.

The general wrapper retains stable segments, reconsiders previous lookahead when a
word or combining mark can move a break, and converges at a shifted old break only
when tab phase and continuation indentation agree. An append that still fits rechecks
the complete cached final grapheme. All content uses the same correctness rules:
spaces, tabs, CJK, wide characters and combining marks. Folds, inline phantoms,
cross-segment deletions and uncertain geometry use `make_line`. Cached physical byte
lengths and borrowed chunk descriptors avoid scanning growing prefixes. The version
guard rejects geometry refreshed inside a Vintage command before publication.

The layout oracle uses 2,048 seeds × 24 transactions × five widths (1, 2, 7, 13, 80)
× both indentation values: 491,520 incremental/full-layout comparisons. It includes
multi-carets, newlines, insertions, deletions, spaces, tabs, CJK, emoji, regional
indicators and combining marks. Dense 160-caret, fold/phantom and Lane B geometry
tests also pass.

The rope retains immutable flat bases and bounded fragment metadata, with cached
cumulative summaries and shared insertion strings. Ordinary flat leaves remain at
6 KiB; edited leaves allow at most 128 fragments and 24 KiB before materialization.
No fragment retains another edited leaf. Copying is confined to touched paths and
bounded leaves; compaction is amortized across edits. Dense one-off batches keep
the flat-copy path. Run caching is disabled for small batches that cannot amortize
it. Public APIs and snapshot semantics are unchanged.

Before the rope change, instrumented `Text::apply_edits` took 0.286 ms at key 50
and 1.283 ms at key 950: touched copy-on-write leaves copied more old text as the
document grew. These are individual stage samples, not block medians. The final
comparison below measures the entire App input path. Repeated typing now shares
old bytes and copies bounded metadata on touched paths; leaf compaction is amortized.

The new String model uses 2,048 seeds × 24 batches and checks every byte/scalar/UTF-16
and line conversion after each batch, plus slices, chunks, summaries and retained
snapshots. It covers empty batches/edits, adjacent/repeated positions, replacements,
Unicode, CRLF and lone CR; actual overlaps must fail atomically. Production tests
exercise grown-fragment deletion and equal-length runs with different Unicode text.
The buffer verifies carets at already validated replacement ends numerically;
arbitrary selections retain full validation, including an invalid near-match case.

A paired single-caret regression exposed one more display cost: patched-fragment
lookup summarized the entire selected fragment, including a large unchanged suffix.
`Leaf::seek` now returns that fragment directly when byte lengths or the final-piece
rule already choose it. The unchanged Unicode prefix metrics are cached in the inline
leaf, preventing repeated scans during highlighted scene construction. The bounded
constructor stays inline; the Unicode prefix measurement is a separate helper.
This removes redundant work without weakening queries.

The ignored App timing gate starts with `"x\n".repeat(carets)`, places each caret
before its x, and types 1,000 committed characters using repeating `let x = 1; `
or `你好世界中文输入`, fixed wrap width 80 on/off, and 1 / 10,000 carets. Every sample
includes App key dispatch, panel/text routing, named action draining, `changed_state`,
workspace polling and panel polling. There are no excluded warm-up keys. Native
receipt, IME composition, scene drawing, GPU presentation and scanout are outside
this measurement. History limits are 1 GiB so the run remains available for undo.
Soft undo/redo and undo back to the original text are checked after timing; a
separate App command test checks exact existing run boundaries.

Full-App median milliseconds per key (first / last 100 of 1,000 keys):

| 10,000-caret workload | Main `385dcdd` | Worktree | Last / first | Accepted rule |
| --- | ---: | ---: | ---: | --- |
| code/wrapped | 27.2950 / 305.0282 | 7.8119 / 10.9921 | 1.407 | ratio ≤1.5 |
| code/unwrapped | 1.0775 / 2.5468 | 0.9761 / 1.2146 | 1.244 | ratio ≤1.5 |
| cjk/wrapped | 61.1613 / 464.0509 | 13.5446 / 18.3550 | 1.355 | ratio ≤1.5 |
| cjk/unwrapped | 22.4656 / 32.7093 | 1.2603 / 2.0452 | 1.623 | unwrapped +0.7849 ms ≤1 ms |

| One-caret workload | Main first / last | Worktree first / last | Matching-block check |
| --- | ---: | ---: | --- |
| code/wrapped | 0.0138 / 0.0689 | 0.0102 / 0.0146 | ≤5% pass |
| code/unwrapped | 0.0158 / 0.0673 | 0.0108 / 0.0188 | ≤5% pass |
| cjk/wrapped | 0.0178 / 0.1076 | 0.0132 / 0.0419 | ≤5% pass |
| cjk/unwrapped | 0.0182 / 0.1105 | 0.0159 / 0.0735 | ≤5% pass |

The final ignored gate passes (`final-app-result.json`, `final-app.json`, `final-app.log`).
Pre-final failures and probes remain archived; they do not supersede this final build.
Reproduce without concurrent compilation:

```sh
VELOCITY_R20_TYPING_BASELINE=/tmp/velocity-lane-a-r20/realistic-run1/main.json \
  cargo test --release --locked -p velocity r20_typing_cost_stays_flat -- \
  --ignored --nocapture --test-threads=1
cargo test --release --locked -p velocity layout::round20_tests
cargo test --release --locked -p vel-text batch_string_model_and_immutable_snapshots_over_2048_seeds
```

On another host, inject only the identical timing harness into a clean `385dcdd`
checkout with its own target, and capture with `VELOCITY_R20_TYPING_RECORD`.
JSON keys include pattern/wrap/caret count, with first/last and all ten block medians.
Record mode measures main's known growth without asserting it as correct. The
candidate requires wrapped last ≤1.5×first; unwrapped accepts that ratio or ≤1 ms
growth; first must be no slower than main, with ≤5% matching-block checks at one caret.

Required checks run in this worktree's own target and all pass:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --release --locked -- -D warnings
cargo test --workspace --locked --release
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
```

The workspace reports **1,034 passed, 0 failed, 21 ignored** across 114 summaries, including doctests.
`worktree-r20-final-gate.json` records command status and duration. Additional
vel-text debug and no-default-feature tests pass. The workspace includes both
2,048-seed models and Lane B's existing wrap/fold tests; no Lane B test was edited.

Seven alternating whole-child retired-instruction pairs against clean `385dcdd`:

| Benchmark | Main instructions | Worktree instructions | Change |
| --- | ---: | ---: | ---: |
| typing1 | 428,473,909 | 300,511,301 | -29.86% |
| typing1000 | 10,659,791,738 | 4,083,289,343 | -61.69% |

`instructions-r20-final/report.json` retains all pairs, hashes and host load. Darwin
`RUSAGE_INFO_V4` is read after exit before reaping. This counts each complete
`--headless-smoke --carets 1|1000` child, including setup, editing, undo/redo,
background work and CPU scenes. It is distinct from the full-App per-key gate.

The existing vel-text benchmark was built with `cargo bench --locked -p vel-text
--bench perf --no-run`. Its unchanged worker operations ran before/after in 21
alternating pairs: 100 MB and 1 GB ASCII loads, 100 MB mixed-Unicode load and all
1 GB operations (24 reported operations). Matching benchmark-only Darwin counters
were added to both builds; the baseline rope is `385dcdd`. The 1 GB input is a
deterministic tenfold repetition of the provided 100 MB fixture because the 1 GB
fixture was absent. The Unicode fixture uses the benchmark's existing generator.

All 24 operations pass the **3% retired-instruction budget**, as do the complete
workers. This is the busy-host comparison; it is not a claim that every wall-time
sample passed. The final load averages were [11.6337890625, 8.15283203125, 7.46826171875].
The initial seven-pair load sample was noisy, so the final sample size was raised
to 21 pairs before collecting it; the earlier observations remain archived.
`core-bench-instructions-extended/report.json` and `observations.json` retain all
wall/CPU/counter results and binary hashes.

| Existing rope operation | Before instructions | After instructions | Change |
| --- | ---: | ---: | ---: |
| ascii-100mb/load | 384,519,944 | 375,099,379 | -2.45% |
| ascii-1gb/batch_lines_10000 | 2,526,206 | 2,551,633 | +1.01% |
| ascii-1gb/batch_lines_100000 | 25,142,634 | 24,514,763 | -2.50% |
| ascii-1gb/batch_uniform_100000 | 84,573,936 | 84,530,263 | -0.05% |
| ascii-1gb/byte_char | 45,142,154 | 45,138,911 | -0.01% |
| ascii-1gb/byte_line | 465,004,085 | 468,985,179 | +0.86% |
| ascii-1gb/byte_point | 538,459,182 | 541,017,550 | +0.48% |
| ascii-1gb/byte_point_utf16 | 581,885,471 | 584,959,284 | +0.53% |
| ascii-1gb/byte_utf16 | 46,020,678 | 46,020,678 | +0.00% |
| ascii-1gb/char_byte | 17,020,678 | 17,020,678 | +0.00% |
| ascii-1gb/chunks | 28,143,335 | 27,883,014 | -0.92% |
| ascii-1gb/clone | 1,112,927 | 1,112,927 | +0.00% |
| ascii-1gb/delete | 25,882,229 | 26,230,698 | +1.35% |
| ascii-1gb/insert | 27,648,841 | 27,978,843 | +1.19% |
| ascii-1gb/line_byte | 477,542,070 | 481,407,338 | +0.81% |
| ascii-1gb/lines | 3,639,346,641 | 3,638,081,677 | -0.03% |
| ascii-1gb/load | 3,680,217,409 | 3,686,664,950 | +0.18% |
| ascii-1gb/point_byte | 591,641,755 | 594,661,821 | +0.51% |
| ascii-1gb/point_utf16_byte | 613,956,797 | 617,115,584 | +0.51% |
| ascii-1gb/sequential_lines_10000 | 71,027,783 | 71,448,354 | +0.59% |
| ascii-1gb/sequential_lines_100000 | 677,409,913 | 682,973,883 | +0.82% |
| ascii-1gb/sequential_uniform_100000 | 166,772,142 | 169,797,845 | +1.81% |
| ascii-1gb/utf16_byte | 17,020,678 | 17,020,678 | +0.00% |
| unicode-100mb/load | 701,635,467 | 695,401,058 | -0.89% |

Wall-time medians above 3% in this run: `ascii-1gb/batch_lines_10000` +7.15%, `ascii-1gb/byte_point_utf16` +51.93%, `ascii-1gb/chunks` +3.14%.
These are not counted as passing wall-time results. Retired instructions are the
reported regression measure under load; native and quiet-host latency remain
outside this headless counter result.

Seven paired existing large-file budget runs compare the pre-rope build with the
final core, including index maintenance for insertions. Both wall and whole-worker
instruction medians pass 3%. Whole-worker counters include load/index setup; very
short navigation operations are at or below timer resolution.

| Large-file operation | Before / after wall ns | Instruction change |
| --- | ---: | ---: |
| load | 621,804,916.0 / 622,433,500.0 | +0.800% |
| insert | 267,729.5 / 267,437.5 | -0.002% |
| word | 41.0 / 42.0 | +0.003% |
| end | 0.0 / 0.0 | +0.004% |
| window | 84.0 / 84.0 | -0.002% |

Seven paired existing scroll/features CPU runs compare the pre-rope build with the
final core. Every recorded P50/P95/P99 metric passes 3%, including highlighted
editing and scene construction. This baseline already contains the general layout
changes, isolating the rope change. The separate typing1/typing1000 gate above uses
main `385dcdd` as requested. No renderer, GPU or native window is exercised.

| Existing scroll/features metric | Before ms | After ms | Change |
| --- | ---: | ---: | ---: |
| highlight_minimap_scroll_cpu/stats_ms/p50 | 0.28860 | 0.29263 | +1.39% |
| highlight_minimap_scroll_cpu/stats_ms/p95 | 0.36018 | 0.35819 | -0.55% |
| highlight_minimap_scroll_cpu/stats_ms/p99 | 0.40504 | 0.40183 | -0.79% |
| highlighted_typing_cpu/stats_ms/p50 | 0.78777 | 0.56056 | -28.84% |
| highlighted_typing_cpu/stats_ms/p95 | 1.27589 | 0.59489 | -53.37% |
| highlighted_typing_cpu/stats_ms/p99 | 1.31379 | 0.61811 | -52.95% |
| highlighted_typing_cpu/edit_stats_ms/p50 | 0.02090 | 0.01427 | -31.70% |
| highlighted_typing_cpu/edit_stats_ms/p95 | 0.03497 | 0.02456 | -29.78% |
| highlighted_typing_cpu/edit_stats_ms/p99 | 0.04135 | 0.03247 | -21.47% |
| highlighted_typing_cpu/scene_stats_ms/p50 | 0.75900 | 0.54642 | -28.01% |
| highlighted_typing_cpu/scene_stats_ms/p95 | 1.25017 | 0.57280 | -54.18% |
| highlighted_typing_cpu/scene_stats_ms/p99 | 1.28433 | 0.59029 | -54.04% |


The unchanged test is
`app::lane_b_round17_tests::blocked_preference_queue_defers_one_reload_and_tracks_multiple_keys_per_write`.
It fails intermittently even when selected alone: **1 of 3 isolated runs** observed
`resource_revision` **3 rather than 2** at line 1530. The other two isolated runs,
the full workspace run, and both Lane B sweeps at 32 and 16 threads pass. This is
not specific to a loaded suite; the evidence does not establish a load-only cause.
The test captures a revision after a fixed three-second polling window without
waiting for every asynchronous configuration publication to finish; a publication
race is a hypothesis, not a confirmed root cause. Logs and
`preference-load-results.json` are ready for Lane B; no test or preference-reload
implementation was changed.

Lane B menu integration (text/model is ready; drawing/hit testing stays Lane B's):

- View > Line Endings: **Unix**, **Windows**, **Mac OS 9**, using
  `set_line_ending {"type":"unix"|"windows"|"cr"}` and `command_checked`.
- Encoding status item: **Reopen with Encoding** and **Save with Encoding**
  submenus from `status_format_menu(Encoding)` / `ENCODINGS`, using public
  `reopen {encoding}` / `save {encoding}` and the supplied enabled/check states.
- Line-ending status item: the same three choices from
  `status_format_menu(LineEndings)`.

Native checklist, pending human review on macOS 14+ (no GUI in the sandbox):

- [ ] Reopen Shift JIS; rewrite externally with reload on/off, clean/dirty tabs and
  low `large_file.load_bytes`. Check prompts, retained edits, no jump-history entry
  and the chosen encoding after restart (with and without saved history).
- [ ] Open BOM-less UTF-16 LE/BE, UTF-8 with BOM and a real PNG/binary. Save text
  byte-identically without edits. With Windows default endings, a one-line file
  gains CRLF on its first inserted newline; existing line conventions win.
- [ ] Paste an emoji into Windows-1252 and Cmd+S. Escape from **Save as UTF-8**
  retains bytes/codec; accepting saves valid UTF-8 and clears dirty state. Repeat
  Save As to a chosen destination; the editor remains usable throughout.
- [ ] Enable format status fields. Check BOM captions and Unix / Windows /
  Mac OS 9. Once Lane B connects menus, check enable/check state and keyboard
  selection. In `\tab`, tab size 4 to 8 changes column 7 to 11; CJK counts once;
  huge lines remain responsive.
- [ ] Enable save trimming, edit one line and save: untouched trailing spaces
  survive by default. Set modified-only trimming false and verify trim-all.
- [ ] Type realistic code/spaces and CJK with 10,000 carets, wrapped/unwrapped;
  undo/redo whole typing runs and soft undo/redo individual keys. Check tabs,
  combining marks, wide characters, indentation, folds and phantoms for wrap,
  scroll and caret geometry. Native latency remains a manual measurement.

# Round 19 Lane B review fixes (native verification pending)

Timing note: `r19_app_sustained_build_output_has_steady_progress_and_lossless_tail` enforces the strict 50 ms UI-gap budget only with `VELOCITY_STRICT_TIMING=1`; the default gate keeps the data-per-poll ratio (which detects lock starvation) and a 1 s bound, because a loaded parallel gate can deschedule the test's own poll work.

No GUI or packaging was run. Headless evidence, command/test mapping, public
sources and performance boundaries are in `ROUND19_LANE_B.md`. Leave these boxes
unchecked until observed on a macOS desktop.

- [ ] **Special document types:** hot-exit a cloned text file, replace its path
  with a FIFO, Unix socket or symlink to `/dev/zero`, and restart. Repeat with the
  original, clone, three views and all views hidden. Startup and later tab clicks
  stay responsive; affected views are read-only, clean and report the path once
  in the status. With edited text still open, Cmd+S, Save All, Save As and Save on
  close refuse the special path and retain the edits. Restore a regular file at
  that path and Open File again: the existing error tabs become live views.
- [ ] **Restore watchdog and backups:** simulate stalled document IO on a test
  volume. The two-second restore deadline opens recovered unsaved text or a blank
  window and names the session backup non-modally. When two windows fail, both
  numbered backups reside beside the session and retain its original bytes.
- [ ] **Changed disk contents:** replace a clean text file with a binary before
  restart; its first restored view is hex/read-only. Replace the disk file while
  its text buffer has unsaved edits; Cmd+S, Save All and Save on close offer
  Overwrite/Cancel. Cancel preserves both versions, Overwrite saves the buffer.
- [ ] **Minimum pane size:** repeatedly run New Pane, drag both splitter axes
  to the edge and resize the window. Every group retains its tab bar and at least
  three text lines; further splits are refused. Middle click closes the tab
  under the pointer. Check native window minimums after font zoom and hiding or
  showing the tab bar.

- [ ] **Unavailable restored clones:** hot-exit a clean cloned file; make it
  unreadable, replace it with a directory, or deny access through macOS privacy
  controls. Relaunch with original/clone/three views selected, then with both
  hidden. Startup and later tab clicks remain usable; the affected views are
  read-only and report the filename non-modally. Close them, keep other dirty
  text, and restart again. Unexpected restore failures report a timestamped
  session backup; recovered text opens unsaved and other windows remain usable.
- [ ] **Binary clones:** clone a 4 MiB file containing NULs and restart. Click
  either view immediately and after loading completes; both stay hex/read-only.
  Cmd+S and attempted typing never change the file's bytes. Repeat with both
  views initially hidden behind another tab.
- [ ] **Symlink Save As:** save a new filename through a directory symlink.
  Modify it externally through the real directory and observe the reload.
  Opening its real path focuses the existing document, including after restart.

- [ ] **Vintage reveal:** in a 3,000-line document, enable Vintage; Esc, G, gg,
  80 × j, dd near the bottom, /search and n must keep the resulting caret visible.
  Repeat after scrolling the caret offscreen and in a wrapped pane.
- [ ] **Layout reduction and Hot Exit:** Cmd+Option+3, Ctrl+Shift+3, Ctrl+1,
  Cmd+Option+2; keep dirty text in this and another window, quit and reopen.
  Both windows' contents return. A layout merge retains one selected view in the
  receiving group. Repeat every column/row/grid preset and New Pane custom cells.
- [ ] **Groups:** exercise Cmd+Option+1…5 and Ctrl+1…4 / Ctrl+Shift+1…4, all Groups
  menu entries, neighboring focus/move, New Pane with/without moving, and Max
  Columns before row splitting. Closing bottom-left in a grid moves its tabs to
  the cell that grows into its space. No selection, anchor, folds or text is lost.
- [ ] **Clone/Split/close:** Clone File selects only the clone; moving it leaves
  the original in place. File › Split View displays both adjacent shared views
  with independent carets/scroll/folds. On an empty window, either command creates
  one untitled view. Cmd+W, middle click and the tab close button close one view,
  including when four tabs are selected; the other selected tabs stay side by side
  and the neighboring tab receives focus. Closing a split never prompts while the
  original remains. Restart with a dirty untitled original/clone pair and only one
  selected view. Edit the visible view and close it: no prompt, no lost text.
  Repeat with the original visible, then Save As, close, edit the remaining view
  and Cmd+S; verify the saved contents and undo survive another restart.
  Close Selected handles the set, asking only when the final view of a dirty
  buffer closes; Cancel stops the remaining operation.
- [ ] **Close variants:** test File > Close All Files and context
  Close/Others/Right/Unmodified/Selected/Deleted with Save, Don't Save and Cancel,
  including Save As cancellation and a write failure. A dirty cloned buffer is
  not offered by Close Unmodified. Delete clean and modified files both from Finder and the side bar: Close Deleted checks the
  paths now and asks only for edited contents. Ordinary Cmd+W retains the existing
  prompt for a document marked deleted.
- [ ] **Settings windows:** Cmd+, and Velocity › Settings… open a separate window
  with read-only defaults on the left and the writable User file on the right.
  The labels distinguish Default/User; both use JSON highlighting for settings.
  Existing User bytes are preserved, missing files receive the template. Restart
  the settings window: labels, syntax and read-only protection return.
- [ ] **Live settings/errors:** save font_size 19, then invalid JSON on line 2.
  All windows keep 19; only the window that saved the file reports the filename
  and line non-modally, once for that revision. Try Save All's preference toggle
  and font larger/smaller/reset while JSON is broken: no alert or file overwrite.
  Change an unrelated settings file and open/close another window: no repeated
  error. Save a new broken revision: one new report. Fix it to 21 and observe live
  apply. Repeat with a syntax-specific settings file while changing a different
  valid file; the valid change must still apply.
- [ ] **Settings entries:** test settings, key/mouse bindings, syntax-specific and
  distraction-free settings, scheme/theme pickers, both customize commands and
  font larger/smaller/reset. Customize `auto` in each appearance: status identifies
  the active variant. Left resources remain read-only, existing overrides survive.
  Browse Packages opens the Packages folder itself in Finder. An open_dir command
  with $file_path/$folder opens the expanded directory and never creates a path.
- [ ] **Menus and format status:** New File leads the File menu; Split View sits
  with file/view commands. Test Copy Path, Reveal in Side Bar and Reveal in Finder.
  Encoding status offers Reopen/Save with Encoding; line-ending status and View ›
  Line Endings offer checked Windows/Unix/Mac OS 9 rows. Command execution awaits
  Lane A's set_line_ending merge; no placeholder command was added.
- [ ] **Builds/performance:** stream 4 KiB diagnostics for three seconds and a
  distinctive tail. Keep steady progress and correct totals; stall consumption,
  cancel and restart. Repeat native typing in one pane and two wrapped shared
  panes, font/geometry changes while reading a parked clone, and optional tab
  controls becoming visible after a theme reload.

The cited macOS shortcut table confirms column/grid layouts and groups 1…4.
It does not document Cmd+Option+Shift+2/3, Ctrl+5…9 / Ctrl+Shift+5…9 or the
requested Cmd+K pane chords. Their bindings remain unchanged. Public sources also
do not establish multi-selected close_file/move_to_group behavior, so those commands
act on the active view; Close Selected and New Pane explicitly handle a set.
Legacy XML scheme customization retains its status message and existing behavior.

---

# Round 18 Lane B third verification (native review pending)

Continued uncommitted on `ac6f530`, keeping the pre-merge preference-test fix.
No GUI or packaging was run. Headless evidence is in `ROUND18_LANE_B.md` and
`/tmp/velocity-lane-b-r18-third/`; all native boxes remain unchecked.

- [ ] In two groups, park the receiver around row 5000 in a 9,990-line file. Insert
  100 lines at BOF from its clone; verify gutter widening preserves the same text.
  Repeat with wrapping, a hidden caret, and sidebar/resize/font changes before
  the next frame. Verify the hidden caret does not pull the parked pane into view.
- [ ] Delete line 10 through EOF, or select-all/delete, from the other clone.
  Undo restores the receiver's original top line; repeat redo/undo, with resize
  before and after undo, in both wrap modes. Ordinary typing/deleting and wheel
  scrolling remain responsive.
- [ ] Run a large-output build. The panel shows a retained tail, a visible byte-count
  truncation notice when transport data was dropped, correct error totals, and
  navigable retained diagnostics. Finished follows the tail promptly. Cancel while
  output is queued, including just after producer exit: Cancelled appears promptly
  and the backlog stops. Verify progress/results while the build is still running.

Headless: **1,096 release tests**, **22 debug build-crate tests**, **62 debug round-18 App tests**, the debug workbench build test, and **24 ignored timings** pass. Fmt, strict clippy and rustdoc pass. Nine-pair retired instructions: features **+0.27%**, typing1 **+0.36%**, typing1000 **-0.78%**. Native presentation remains unobserved.

The preceding native checklist and verification record follow unchanged.

---

# Round 18 Lane B pre-merge test synchronization

Follow-up to `ac6f530`: the blocked-preferences test now awaits the resource worker
and fences the existing serial write queue. Its extra publication was a pending-write
stamp scan counted by a timing-sensitive assertion; exactly one settled reload is
still required. Runtime code and native behavior are unchanged.

**50/50 isolated runs** and **10/10 simultaneous full-module runs** pass (290 module
cases, default test parallelism). Full gate: **1,086 passed, zero failed** (114 suites,
25 existing ignored tests); fmt, strict release clippy, and strict rustdoc pass.
Root-cause trace, synchronization details and logs: `ROUND18_LANE_B.md` and
`/tmp/velocity-lane-b-r18-reload/`. The existing unchecked native checklist follows.

---

# Round 18 Lane B verification fixes (native review pending)

Started clean at `c8260ac`, based on main `385dcdd`; follow-up is uncommitted.
No GUI is available. Leave these boxes unchecked until observed on a macOS desktop.
Finding/function/test mapping and measured evidence are in `ROUND18_LANE_B.md`.
Do not run packaging for this review.

Headless gate: **1,086 passed, zero failed**; fmt, strict clippy and rustdoc pass.
All **24 ignored headless timings** pass. Login stress: **30/30 parallel trials**, 270
passed cases (four concurrent processes; default Rust test parallelism within each).
Nine-pair retired instructions versus main `385dcdd`: **+0.20% / +0.38% /
+0.13%** for features/typing1/typing1000. Single caret passes **+0.5%**;
others pass +5%. Busy host load 3.00–19.53; no native performance claim.
Quiet build: **15.182 ms CPU / 3 s**. Project walk: **110.117 → 109.909 ms**.
40,000-caret gutter off/on: **10.528 → 10.698 ms/frame**.

- [ ] **Reading position:** on 5,000 long wrapped lines, park at line 3000 and toggle
  word wrap, width 40/80, centered text, gutter/line numbers, rulers, font size and line
  padding. Top text stays anchored; a previously visible caret stays visible. Repeat with
  caret parked away and with syntax-specific font/wrap settings, zoom and display scaling.
  Toggle the sidebar, resize narrow/wide, enter/exit distraction-free full screen, and reload
  Menlo/Monaco. Repeat with the same document cloned into two groups at lines 4000/5000;
  both visible panes keep their own top text and visible carets through repeated zoom in/out.
  Unwrap a very long single line while reading its middle: retain horizontal reading position.
  Insert 1,000 lines above a parked pane from its clone: preserve the visible text, while the
  typing pane follows its caret. Repeat with the recipient focused and the edit away from
  its caret. Unfold a large region above the viewport; visible text stays anchored.
- [ ] **Commands and focus:** settings, syntax, palette, theme, toggles and copying preserve
  a parked unwrapped view. Motion, edit, undo/redo, Find and Goto reveal. Switch tabs/groups
  using commands, tabs and sidebar Open Files: each restores its own parked viewport.
  Jump f0:3000, scroll f0 to top, jump f1:100 then f1:5000 and press Ctrl+- three times:
  f0:3000 must be visible. Repeat Forward, bookmarks and marks with an unchanged selection.
  In Find with one match, scroll away and press Enter/Shift+Enter repeatedly; match reappears.
- [ ] **Keyboard animation:** compare 0/0.5/1/2, repeated Page Up/Down, center and Goto.
  Type newlines or commit IME text during motion; the caret remains visible after settling.
  Wheel/trackpad/minimap interruptions and Reduce Motion remain immediate. Verify final
  hit testing and changing zoom/scale or speed to zero during motion. Resize, toggle the
  sidebar/wrapping, zoom or unfold during Page Down: settle at the same text destination
  with the caret visible; later animation frames must not overwrite the reflow.
- [ ] **Caret and gutter:** solid plus inverse stays visible. Compare block false/true,
  blinking styles, top/bottom extensions, rounded corners, glyph contrast, ligatures,
  CJK/emoji, selections, multiple carets and Vintage. Compare highlighted line numbers
  on wrapped and unwrapped long lines, with a syntax override and many carets.
- [ ] **Minimap:** false centers an outside click; true keeps the drawn clicked text at
  pointer y, including a click inside the viewport. Repeated stationary drags remain
  stationary; both ends are reachable. Round-17 border and release-over-status behavior stay.
- [ ] **Tabs/title:** compare dirty-tab colors, scrolling versus fitting, new/scroll buttons,
  resizing and repeated toggles; hidden buttons have no hit target. Compare basename/full/
  project-relative title paths, untitled/deleted/dirty files and paths outside project roots.
- [ ] **Preview tabs:** true/false/only_left, single/double click, replace clean previews,
  edit/save/double-click/Goto promotion, and dirty/pinned protection. Open and cancel palette
  or appearance pickers while previewing: no recents entry or persisted session tab/order.
  Restart with a preview active; only permanent tabs return.
- [ ] **Tree motion:** expand/collapse repeatedly and click moving rows. Compare animation
  off, global animations off, Reduce Motion, and toggling Reduce Motion during transition.
- [ ] **Build choices:** Automatic Rust → Python → several Rust matches prompts correctly.
  Pick RustB, then PyA, return to Rust and Python: both choices persist without another picker.
  A picker variant is remembered only for its matching systems; removing it falls back.
  Explicit Tools choices apply to all file types. Build With lists matching systems only.
- [ ] **Login environment:** launch from Dock with Homebrew/nvm/conda PATH in the user's
  login shell; verify argv and shell builds. Repeat without another successful capture;
  alternate systems with different path/env overrides: all share one captured base, with
  each override applied separately. Touch a login profile to invalidate. A hanging profile
  costs one 2 s wait; switching systems still falls back promptly. A healthy 3 s profile
  serves later builds in either system. Press Cmd+B twice during the initial capture: the
  replacement uses its eventual result; cancelling the build does not kill shared capture.
  Selecting a build system starts background capture. Unsupported shells fall back to bash.
  Profiles with stdout banners, EXIT/.zlogout assignments or detached jobs cannot inject
  variables or hang build completion; only bytes between start/end markers count.
- [ ] **Build flow/output:** start/cancel/restart children and grandchildren; no group child
  remains, old output never reaches the next run, and detached pipes cannot delay Finished.
  Pause polling for 0.6–2 s near the end of a 1.08 MiB output stream: the tail marker precedes
  Finished without silent loss. Repeat with 3 s stalls and 22 MB. A detached daemon printing
  every 50 ms must finish within about 700 ms after the main process exits and cancel within
  100 ms. Any discarded daemon output is visibly marked. Over 8 MiB, the panel marks its
  6 MiB retained tail; the first line after the notice must be a complete line.
  Default output wraps; explicit false disables it. Check incremental large output,
  diagnostics/results after trimming, SIGUSR1/SIGKILL names, exit code, quiet and Cancelled.
  F7/Ctrl+C work with document/find/output focus and cancel binds only while running.
- [ ] **Save All on Build:** toggle via Tools in tab A, check same state in tab B and after
  restart. False skips saving everywhere; true saves named dirty tabs and skips untitled.
  Direct exec never saves. New Build System opens the untitled JSON template; interactive
  builds report unsupported. Check show-panel off/on and build/search result navigation.
- [ ] **Project events/status:** create/rename/delete below gitignored target with indexing
  off/on; search/Goto file lists stay current. Check native FSEvents load during builds.
  Compare compact/enabled/disabled line-column status without hiding other status items.
- [ ] **Merged menus:** exercise all Goto rows and Preferences color-scheme/theme rows,
  plus UI: Select Color Scheme / UI: Select Theme palette entries against the real pickers.

# Round 19 Lane A final verification fixes (native review pending)

Start: clean `lane-a` at `71a516d`, following the user's two WIP commits and main
`dad6e3d`. These fixes remain uncommitted; no reset or amendment was performed.
Evidence and start-of-review backups: `/tmp/velocity-lane-a-r19-final-review/`.
The only tracker change in Lane B's `workspace.rs` is the small
`discard_settings_write` method. App changes share explicit `open_file` dispatch;
the scroll/reveal, layout, rendering, build, platform, session and resource-menu areas
are untouched. No GUI, packaging script, `cargo clean` or package target was used.

| Finding | Fix | Headless user-path test |
| --- | --- | --- |
| 1: existing Finder tab | `open_requests.rs:open_path` marks duplicate-file selection as an explicit request before selecting, preserving unfocused origins; literal `open_file` dispatch shares the headless/native handler. | `r19_existing_tab_open_requests_preserve_unfocused_origin` (native open-path router, native socket accept, CLI socket accept and named `open_file`); existing new-file request test |
| 2: bounded multi-carets | `navigation.rs:Jump::compact/same_position/advance` retains all selections through 64 regions, including direction/primary; larger selections retain the primary plus original count. Incremental mapping remains bounded independently of the live caret count. | `r19_bounded_multicaret_back_forward_maps_all_regions` (5, 64 and 65 regions, Back/Forward and eviction); corrected existing small-selection assertion; 10,000-caret gate |
| 3: click jitter | `History::mouse_down` continues a native multi-click within a four-pixel radius on the same button. | `r19_multiclick_is_one_gesture` now jitters each press; existing distinct-click/drag test |
| 4: typing sessions | `Guard::with_class` assigns one identity to insert, auto-pair/snippet, left/right delete and committed IME text, independent of arguments. `History::observe` branches Forward only when it actually records a new origin; distance splits remember the session start. | `r19_typing_arguments_merge_and_preserve_forward_history`, `r19_editing_distance_is_measured_from_session_start`, existing repeated-command/session tests |
| 5: failed-write flash | `appearance.rs:poll_appearance_commits` revokes failed pairs before rollback; `workspace.rs:discard_settings_write` removes only matching overrides, preserving newer commits and unrelated zoom keys. | `r19_failed_appearance_write_revokes_reload_override_before_error_frame` (scheme/theme/variant, blocked writer, in-flight reload, one status, no modal queue, zoom interleaving) |
| 6: numeric vi Ex address | `vintage.rs:feed_vintage` treats accepted numeric `:N` as an explicit jump per this review's requirement. | `r19_vintage_ex_line_number_is_a_jump` (focused vi input panel, Back/Forward) |
| 7: timing robustness | Test-only probes time the observation/rebase hooks, counting nested calls once; compare 300 matching App insertions with history enabled/disabled, in alternating order. | `r19_multicaret_history_300_key_stall_gate` (history hooks ≤1 ms/key; paired full-run overhead ≤10%; 60 Back and 60 Forward recalls after eviction) |
| 8: hover retry cadence | Document the observed native idle cadence as up to 200 ms. The 100 ms deadline remains a retry target; scopes still stop retrying after two seconds. No app scroll/reveal changes. | Existing `r19_hover_unpublished_scopes_use_slow_bounded_wakes` verifies suppression of 8 ms busy frames, deadline/timeout and publication without pointer movement; native cadence remains manual |

No preference defaults changed. Existing menu/palette integration requirements are
listed below. B17's `track_settings_write(rx, &pairs)` is already integrated at commit
time by `71a516d`; this follow-up preserves that integration and adds failure revocation.

The numeric vi address behavior is explicitly requested here. Current
[Vim help for `:[range]`](https://vimhelp.org/motion.txt.html#%3A%5Brange%5D) says it
does not modify Vim's jumplist, unlike `G`; the code comment records this discrepancy.
The other navigation/appearance source citations remain in place.

Correction to the previous **1.926 ms** claim: that historical experiment used
`app_round18_tests::input`: panel interception, `Editor::text_input` and named-action
draining. Its fixture was 10,000 carets on `"x\n".repeat(10_000)`, two shared-buffer
sheets, 60 tab-switch origins, ten warm-up insertions and 300 measured insertions,
with change-map retention reduced to 160. It **omitted `App::changed_state`**, thus
omitting the native input route's shared-buffer synchronization and reveal work. It
was not a full App-path latency measurement. The replacement explicitly calls that
same shared post-input adapter in both paired runs; it makes no worst/median claim
about `text_input`, which has a separately reported pre-existing growth cost.

The failed-write regression was also checked with mutation testing: removing just
`discard_settings_write(&failed)` makes the test fail because Amber replaces `auto`
in the very frame that shows the save error. The call was restored before the full
gate. Evidence: `failed-write-mutation.log` and `mutation.json`.

Native reviewer checklist (all pending in this headless sandbox):

- [ ] With one/two already open, leave one at line 30, unfocus, Finder-open two and
  refocus. Back returns to one:30; Forward restores two's existing location. Repeat
  CLI `two:50` and the `open_file` command, with both new and existing tabs.
- [ ] Leave five carets (including directed selections) before/after jumps. Back and
  Forward restore all five; 64 regions survive, 65+ restore the primary. Type through
  change-map eviction and test 10,000 carets with many history entries for stalls.
- [ ] Double/triple click with small trackpad jitter: one history origin. Distinct
  clicks beyond the radius remain separate. A drag remains one gesture.
- [ ] After Back, type 60 lines using varied text, auto-pairs, delete and IME. No
  typing-origin flood, and Forward remains reachable. A new explicit jump branches it.
- [ ] Delay a Preferences write, force its failure while a resource reload is in
  flight, and check the very frame showing the error: original appearance already
  restored, one non-modal status, no subsequent flash. Repeat with variants/themes
  and an interleaved zoom or a later successful appearance commit.
- [ ] In Vintage, submit `:30` then `:70` through the input panel; Back/Forward visit
  both origins. Counted `j/k` remain absent from history. Hover an edited word without
  moving; scope retry follows the native idle cadence (up to 200 ms), ending by 2 s.

Final-review history timing (`history-timing-current.log`) passes the new gate:
**0.278 ms worst aggregate history-hook time per key**, with 10,000 live carets and
60 entries across 300 measured insertions (ten warm-ups). The full shared App input
adapter measures **147.747 ms median with history / 147.177 ms disabled**, with totals
**44.889 s / 45.049 s (−0.355%)**. Both runs keep identical shared-buffer selections,
undo limits, retained-map pressure and input lengths, alternate execution order for
each key, and include `changed_state`. Test-only nested probes surround history
observation/rebase, including snapshot construction; production has no timing probes
or history-disable flag. The test recalls all 60 entries in both directions after map
eviction. This replaces the unstable full-input worst/median criterion.

Final verification (`gate.json`) passes formatting, strict release/locked workspace
clippy, `cargo test --workspace --locked --release` at default parallelism, and
`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked`.
The complete suite reports **1,016 passed, 0 failed, 20 ignored** across 114 test/doc-test
suites. The ignored 10,000-caret timing is run separately as reported above. The
3,000-step navigation sweep completes **117 real sheet switches** with bounded
stacks, valid Unicode boundaries and workbench invariants (`navigation-sweep-final.log`).

Nine alternating whole-child retired-instruction pairs against main `dad6e3d`
pass the 5% gate (`instructions/report.json`):

| Benchmark | Main median instructions | Candidate median instructions | Change |
| --- | ---: | ---: | ---: |
| `typing1` | 428,006,624 | 428,138,276 | +0.031% |
| `typing1000` | 10,916,058,218 | 10,973,431,540 | +0.526% |

The baseline is built from `git archive dad6e3d` plus the exact pinned permissive
package fixtures, with build provenance in `baseline.json`. The counter report
retains every sample, host load and binary SHA-256. It measures each entire headless
child using Darwin `RUSAGE_INFO_V4` after exit before reaping; no GUI/window/drawing
latency is claimed. The paired App/history test also excludes native presentation.
No commit, reset or amendment was made; `HEAD` remains `71a516d`.

Reproduce the focused timing and navigation checks:

```sh
cargo test -p velocity --release --locked r19_multicaret_history_300_key_stall_gate -- --ignored --nocapture
cargo test -p velocity --release --locked r19_randomized_navigation_sweep -- --nocapture
python3 /tmp/velocity-lane-a-r19-final-review/retired-instructions.py \
  --base /tmp/velocity-lane-a-r19-final-review/main-dad6e3d.bin \
  --candidate target/release/velocity --pairs 9 --output /tmp/velocity-r19-final-instructions-repeat
```

# Round 19 Lane A earlier verification follow-up (historical measurements below)

The N1–N8 follow-up remains uncommitted on `lane-a`, base main `58fd98c`.
Evidence and review-start backups: `/tmp/velocity-lane-a-r19-navigation-review/`.
No GUI, packaging, `cargo clean` or `target/package-macos/` access. Lane B's settings
write tracking and renderer/layout/platform/session/menu/palette areas are untouched.

| Finding | Fix (`file:function`) | Headless user-path test |
| --- | --- | --- |
| N1 | `navigation.rs:push/Jump::advance/History::rebase`: retain only the primary region and original caret count; borrow change maps; round-robin at most four records / 256 maps per revision only under retention pressure. Restore lazily; finish anchors when closing their buffer. | `r19_multicaret_history_records_only_primary`, `r19_multicaret_history_300_key_stall_gate`, existing 10,050-edit test |
| N2 | `navigation.rs:Workspace::navigation_request`, `workspace.rs:open`: explicit file requests capture the actual origin even before OS focus returns. | `r19_unfocused_cli_finder_and_open_preserve_origin` (CLI socket accept, native Finder request, direct open) |
| N3 | `navigation.rs:Guard::command_args/History::observe`, `editor.rs:run`: stable name/argument identities, editing-session distance rule, explicit jump classification. | `r19_repeated_editing_commands_preserve_goto_origin` (all reported sequences, including 140 Cmd+D presses), `r19_nearby_bookmarks_and_marks_are_explicit_jumps` |
| N4 | `panels/details.rs:preview_find/cancel_incremental`, `panels.rs:finish_find_navigation/navigate_find/close_panel`: suppress preview records; accept a single origin/final transition; cancel incremental find silently. | `r19_find_preview_is_one_session_and_cancel_is_silent` (one character at a time, Enter/ordinary close/incremental cancel, symmetric Forward), `r19_find_all_accepts_session_without_closing_panel` |
| N5 | `app_mouse.rs:handle_mouse_event`, `navigation.rs:History::mouse_down`: click counts above one at the same point/button continue the gesture. | `r19_multiclick_is_one_gesture`, existing distinct-click/drag test |
| N6 | `appearance.rs:commit_appearance/poll_appearance_commits`: promote preview directly to committed User settings before the queued write; roll back only on failure, once. | `r19_commit_keeps_preview_visible_until_write_completes` (scheme and theme, blocked serial writer), `r19_malformed_preferences_roll_back_once` |
| N7 | `intelligence.rs:restore`, `panels.rs:action`: drop missing closed-file targets, show `<path> no longer exists`, create no tab/file. | `r19_deleted_closed_history_file_is_skipped` |
| N8 | `panels.rs:busy/poll`, `panels/details.rs:message_deadline`: after the initial hover delay, retry unpublished scopes at 100 ms intervals for at most two seconds. | `r19_hover_unpublished_scopes_use_slow_bounded_wakes` (no 8 ms busy frames, publication without moving, timeout, zero parses) |
| Tab-switch coverage | The 3,000-step randomized sweep and picker tests use actual `workspace.select` switches; the existing tab-click route also exercises activation. | `r19_randomized_navigation_sweep`, `r19_picker_dismisses_on_tab_commands_and_window_focus_loss`, `r19_history_tab_switch_edit_mapping_closed_view_and_primary` |
| Counted Vintage motions | `vintage.rs:feed_vintage`, `navigation.rs:Guard::command_kind`: non-jump motions never record, independent of count/distance; explicit vi jumps retain origins. | `r19_counted_vintage_nonjump_motions_never_record`, corrected `r19_vintage_parsed_motions_and_jump_origins` |

No preference defaults changed in this follow-up. Public documentation remains cited in
the implementation. The requested primary-only history and non-jump vi behavior follow
the verifier's observations; the public docs do not specify those storage details.

**B17 integration is still required after rebase.** At the marked `// lane B API` site,
register the real queued receiver at commit time with
`track_settings_write(rx, &[(String, Option<Value>)])`, using the exact root/variant pairs
already carried by the commit. No second stale-reload guard has been built. Until that
generic API lands, this branch retains its existing old-reload limitation.

Native follow-up checklist (unverified in this headless sandbox):

- [ ] With 10,000 carets and many jump origins, type through 128/256 edits: no periodic
  history pause; Back restores only the primary position. Continue past 10,000 edits.
- [ ] In `one.txt` at line 30, unfocus, open `two.txt:50` via CLI; refocus and Back returns
  to line 30. Repeat with Finder and an already-open file.
- [ ] After Goto Line, run page/line/document-limit motions, repeated Cmd+D, selection
  expansion, paste, swap line, word delete, comment, line delete and undo. Back reaches
  the pre-Goto position. Check single/double/triple clicks and drag identities separately.
- [ ] Type a long Find query slowly; Back visits only accepted find positions. Incremental
  Find Escape restores its origin. In Vintage, `20j`/`20k` never record; `G`/`gg` still do.
- [ ] Commit each picker while disk work is delayed: no flash to the old appearance.
  An unwritable/malformed Preferences file rolls back once with one non-modal message.
  After B17 rebase, repeat during an in-flight reload and verify external edits still apply.
- [ ] Close a named file, delete it externally and Back: no empty tab opens; status names
  the missing path. Hover an edited embedded-source word without moving: published scopes
  reveal it within a slow retry; unpublished scopes cease wakes after two seconds.
- [ ] Preview and actually switch tabs by Ctrl+Tab, `next_view` and tab click; every route
  restores exactly. Repeat with Vintage and IME composition and on OS focus loss.

Reproduce the dedicated timing gate separately from the parallel workspace suite:

```sh
cargo test -p velocity --release --locked r19_multicaret_history_300_key_stall_gate -- --ignored --nocapture
cargo test -p velocity --release --locked r19_randomized_navigation_sweep -- --nocapture
```

Follow-up verification: strict release/locked clippy, fmt, default-parallel workspace
tests and rustdoc with `RUSTDOCFLAGS="-D warnings"` pass. The workspace reports
**955 passed, 0 failed, 15 ignored** across 114 test/doc-test suites; the dedicated
timing test is ignored in the parallel suite and run explicitly in isolation.
`gate.json`, `workspace-final.log`, `clippy-final.log`, `fmt-final.log` and
`rustdoc-final.log` contain the final results. The randomized sweep completes 3,000
steps with **117 real sheet switches**, maintaining Unicode boundaries, bounded
Back/Forward stacks and workbench invariants (`navigation-sweep.log`).

The isolated 10,000-caret timing creates 60 genuine origins by alternating shared-buffer
sheets (line-limit command runs now merge). It reduces only change-map retention to
160 so that maintenance and eviction occur within 300 keys, while keeping the default
undo/memory limits. After ten warm-up keys, **300 keys measured 1.926 ms median,
3.764 ms worst (1.954×)**. Key 128 took **1.884 ms**. Every one of the 60 Back and
60 Forward recalls restores the correctly mapped primary caret after eviction.
An earlier timing trial had one 4.008 ms spike (2.091× its 1.917 ms median); the
diagnostic repeat measured 3.572/1.912 ms (1.868×). All trials remain in `stall.log`,
`stall-diagnostic.log` and `stall-final.log`; no samples or bounds were removed.
These are headless measurements, not native latency claims.

Nine alternating whole-child retired-instruction pairs against main `58fd98c` pass
the 5% regression gate (`instructions-final/report.json`):

| Benchmark | Main median instructions | Candidate median instructions | Change |
| --- | ---: | ---: | ---: |
| `typing1` | 425,385,087 | 424,844,640 | −0.127% |
| `typing1000` | 10,630,925,948 | 10,606,019,284 | −0.234% |

The report retains all samples, host load and both binary hashes. Counters use Darwin
`RUSAGE_INFO_V4` after each headless child exits, before reaping; they include setup,
editing, background work and CPU scenes. No GUI or packaging action was performed.
Reproduce with `/tmp/velocity-lane-a-r19/retired-instructions.py --pairs 9`, the base
`/tmp/velocity-lane-a-r19/main-58fd98c` and candidate `target/release/velocity`.

# Round 19 Lane A first review fixes (historical verification below)

Base main `58fd98c`, branch `lane-a`, uncommitted. Review evidence and review-start
backups: `/tmp/velocity-lane-a-r19-review/`. The clean main binary remains at
`/tmp/velocity-lane-a-r19/main-58fd98c`. No GUI or packaging script was run.
No layout, renderer, platform, session, menu or palette files were changed. The mouse
adapter has only a mouse-down identity hook; app changes dismiss appearance pickers
and gate history at the window focus boundary.

| Finding | Implementation | Headless test through the user's path |
| --- | --- | --- |
| 1: synchronous hover parsing | `panels.rs:hover_at` captures only a bounded word; delayed `poll` uses published scopes and suppresses an unpublished result | `r19_hover_stale_embedded_pointer_moves_never_parse` (HTML/Markdown, 255 × ~960 bytes, 51 moves, <1 ms/move, zero parses) |
| 2: background history | `navigation.rs:History::observe` checks active sheet/window; `Tab::install` suppresses programmatic reload/restoration | `r19_background_reload_preserves_history_and_forward`, `r19_lazy_background_load_never_records_selection` |
| 3: clicks/Vintage | `app_mouse.rs:handle_mouse_event` gives every down its own identity; `vintage.rs:feed_vintage` identifies parsed motions and remembers vi jumps | `r19_mouse_clicks_are_distinct_and_drag_is_one_jump`, `r19_vintage_parsed_motions_and_jump_origins` |
| 4: pending preference writes | `appearance.rs:commit_appearance` retains the exact root/variant pairs written atomically; `poll_appearance_commits` publishes only successful writes | `r19_appearance_commit_persists_exact_pairs_after_inflight_reload` (blocked writer, old reload, one write, subsequent external edit) |
| 5: broken themes | `theme_ui.rs:validate_choice/compile_choice` caches validation without mutating UI; picker errors use status only, cannot accept | `r19_broken_theme_is_nonmodal_and_cannot_commit` |
| 6: expired/closed jumps | `navigation.rs:rebase` now spreads primary-anchor mapping under retention pressure and before close; `intelligence.rs:restore` reopens paths at Unicode line/column, skips closed untitled views | `r19_history_survives_change_log_eviction_and_editing_sessions`, `r19_closed_file_reopens_at_saved_line_and_column`, corrected `r19_find_result_and_closed_view_history` |
| 7: hover eligibility | `panels.rs:hover_eligible`: code, HTML class/id and embedded source; excludes comments, other strings and plain text | corrected `r19_hover_scope_table_and_stale_prefix_budget` |
| 8: editing sessions | `History::observe` coalesces nearby typing and character/word motions; explicit jumps retain their origin and local edits add no typing origin | `r19_history_survives_change_log_eviction_and_editing_sessions`, corrected `r19_history_tab_switch_edit_mapping_closed_view_and_primary` |
| 9: real overrides | preview uses the User layer's actual precedence, restores only its temporary key, preserves real View/Syntax values; masked commits report status | `r19_commit_preserves_view_and_syntax_overrides`, corrected `r19_picker_filters_input_preselects_fixed_and_survives_appearance_change` |
| 10: mode variants | `appearance_selection.rs` previews `row.key`; inactive variants make no visible change, with mode annotations | `r19_variants_preview_the_committed_system_mode`, `r19_auto_variants_restore_exactly_and_commit_once` |
| 11: malformed Preferences | failed write leaves file and committed memory unchanged, restores preview, emits one status delivery | `r19_malformed_preferences_roll_back_once` |
| 12: duplicates | `appearance.rs:Catalog::load` deduplicates scheme filenames while retaining merged User overrides | `r19_scheme_rows_deduplicate_user_overrides` |
| 13: dismissal | Ctrl+Tab interception, named tab commands, active-sheet change and window focus loss all dismiss through cancellation | `r19_picker_dismisses_on_tab_commands_and_window_focus_loss`, `r19_appearance_dismissal_and_window_view_overrides` |
| 14: many tabs | preview recolors visible sheets only; committed changes mark background adapters stale and refresh on activation | `r19_preview_touches_only_visible_views_and_activation_is_lazy` (31 tabs; verifies untouched background settings/schemes) |

Lane B integration: **the generic old-reload race guard is still supplied by B17**.
No second pending-key override mechanism was added. At the marked `// lane B API`
call, replace `track_settings_write(rx)` with
`track_settings_write(rx, &[(String, Option<Value>)])` at commit time after rebasing onto B17. Every
persisted key/value pair is available there, including `auto` plus the selected dark/light
variant, while the file itself is written exactly once. The current test covers an old reload
finishing before the blocked write and verifies that later external edits remain effective;
the generic guard's after-drain race belongs to Lane B's tests/integration.

Public sources are cited at the implementation sites:

- [Indexing](https://www.sublimetext.com/docs/indexing.html#goto-definition) and
  [ST4 changelog](https://www.sublimetext.com/download): indexed symbol hover, definitions,
  references and jump records. The exact eligibility table follows section 5.2 and the
  reviewer's detail-audit clarification. Hover never performs synchronous scope parsing.
- [Settings precedence](https://www.sublimetext.com/docs/settings.html#settings-files):
  User, Syntax and View layers; a syntax-specific scheme can mask a saved User choice.
- [Color schemes](https://www.sublimetext.com/docs/color_schemes.html#customization) and
  [themes](https://www.sublimetext.com/docs/themes.html): filename merging and resource formats.
  Compiled choices are reused. No Sublime Default resources were read or copied.
- [OpenFlags.CLEAR_TO_RIGHT](https://www.sublimetext.com/docs/api_reference.html#sublime.OpenFlags.CLEAR_TO_RIGHT)
  deselects sheets to the right; existing side-by-side/clone/group behavior is unchanged.
  Its headless tests remain `r19_goto_side_by_side_and_clear_to_right_sheet_outcomes`,
  `r19_side_by_side_existing_other_group_clones_and_retains_group` and
  `r19_reference_preview_arguments_cancel_and_commit`.

No settings defaults were changed and no Preferences keys were added. `show_definitions`
remains `true`, as documented in Indexing. The ten-line local-edit merge threshold is an
internal conservative policy requested by this review, not a claimed Sublime default.

Rows still needed from Lane B (menu/palette resources untouched):

| Menu row | Command / args |
| --- | --- |
| Goto > Goto Anything… | `show_overlay {"overlay":"goto"}` |
| Goto > Goto Symbol… | `show_overlay {"overlay":"goto","text":"@"}` |
| Goto > Goto Symbol in Project… | `goto_symbol_in_project` |
| Goto > Goto Definition / Goto Reference | `goto_definition` / `goto_reference` |
| Goto > Goto Line… | `goto_line` |
| Goto > Jump Back / Jump Forward | `jump_back` / `jump_forward` |
| Goto > Switch File > Next / Previous | `next_view` / `prev_view` |
| Preferences/Settings > Color Scheme… / Theme… | `select_color_scheme` / `select_theme` |

Palette rows remain **UI: Select Color Scheme** (`select_color_scheme`) and
**UI: Select Theme** (`select_theme`), with matching missing Goto commands. Public docs
confirm the selection UI and command behavior; the exact palette wording is retained
from the prior round (no Default package inspection).

Native checklist (all unverified: there is no GUI in this sandbox):

- [ ] Click lines 3, 8, 13, 18; each Ctrl+- returns one click. A drag makes one record.
  Check multi-caret primary/direction and scroll restoration; mix typing/word/arrow keys
  after a Goto jump, then return directly to its origin.
- [ ] In Vintage, use `jjj`, `G`, `gg`, searches, `n/N`, `*/#`, `%`, sentence/paragraph,
  viewport and mark motions. Ctrl+- returns to each jump origin, while local runs merge.
- [ ] Modify a background tab using a formatter or git checkout, finish a lazy background
  load and switch OS window focus. No history entry or forward-stack change appears.
- [ ] Make two Goto jumps, type >10,000 characters, then return. Close a named target,
  go back and verify reopen at the saved Unicode line/column; closed untitled views skip.
- [ ] Hover code, comments, strings, plain text, HTML class/id/other attributes and embedded
  script/style/Markdown source. Only eligible published scopes produce a popup; rapid edits
  and pointer moves never stall. Self definitions are absent; reference links work.
- [ ] Exercise F12/Shift+F12, both shortcut aliases and `side_by_side`/`clear_to_right`
  across groups. Preview/cancel/commit keep transient sheets and selected groups consistent.
- [ ] In both pickers, preview by arrow/mouse and filter/IME; no file writes occur. Enter
  writes once. Escape/outside click/other panel/Ctrl+Tab/`next_view`/OS focus loss restore.
  Test fixed choices, `auto`, both variants and OS mode changes during preview.
- [ ] Test real View and Syntax overrides plus another window. Preview matches commit;
  real overrides survive and masking is explained in status. Same-named User schemes
  appear once. Broken schemes/themes cannot commit and never open a modal.
- [ ] With 31 tabs and several visible groups, only visible views preview; background views
  refresh on activation. Check redraw, title bar, popup colors, Retina and regular screens.
- [ ] Break or make User Preferences unwritable immediately before Enter. The file and
  committed appearance survive; exactly one non-modal "could not save Preferences: …"
  status appears. After B17 rebase, repeat commits while a resource reload is in flight,
  then externally edit Preferences to verify the generic pending-write reconciliation.

Verification: fmt, strict release/locked workspace clippy and rustdoc with warnings
denied pass. Full workspace tests at default parallelism report **944 passed, 0 failed,
14 ignored** across 114 test/doc-test suites. The final repeat also covers excluded-hover
idle termination and dismissing a picker after its source tab becomes hidden. Logs:
`workspace-complete.log`, `clippy-final.log`, `rustdoc-final.log`, `fmt.log`.
The deterministic navigation sweep uses 2,000 commands, edits and history recalls with
Unicode boundary, bounded-stack and workbench invariants (`navigation-sweep.log`).

Nine alternating retired-instruction pairs against clean main `58fd98c` pass the ≤5% gate:

| Complete headless child | Main median instructions | Candidate median instructions | Change |
| --- | ---: | ---: | ---: |
| Typing, 1 caret | 426,740,627 | 427,071,554 | +0.078% |
| Typing, 1,000 carets | 11,371,195,714 | 11,229,219,202 | −1.249% |

`instructions-final/report.json` includes all samples, load, counter boundaries and binary
SHA-256 hashes. The host was busy (one-minute load 16.25–19.27), so retired instructions
were used as permitted. The measurement covers each entire child, including setup,
editing, background services and CPU scenes, read after exit before reaping using Darwin
`RUSAGE_INFO_V4`. It measures no GUI or presentation. The smoke benchmark instantiates
`Editor` directly; the separate observer timing below attaches history to an app workspace.

Explicit user-path timings on this host:

- 255-line stale embedded HTML / Markdown, 51 pointer moves: worst **0.006 / 0.003 ms**,
  **zero synchronous parses**, including the delayed eligibility poll (`hover-timing.log`).
- 31 tabs, single visible view: arrow preview **0.138 ms median / 0.212 ms max**;
  background settings and scheme identities stay untouched (`preview-timing.log`).
- 10,000 carets: history observation **95.1 ns/hook** median over 21 × 1,000 hooks;
  an insertion with history attached **0.929 ms**, retaining all carets (`history-timing.log`).

These are headless observations, not native latency claims. The native checklist above
remains unverified. Baseline SHA-256:
`a646060381fa1119836256ede9369a96ee029cc41a0745c31c47c269e10a269b`;
candidate SHA-256:
`9d28d7679a452b566a508d7eebac199b0260c4e6d465efe210c88cf226c54e2f`.

Reproduce headlessly without packaging:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --release --locked -- -D warnings
cargo test --workspace --locked --release
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo test -p velocity --release --locked r19_randomized_navigation_sweep -- --nocapture
cargo test -p velocity --release --locked r19_jump_history_10000_caret_timing -- --ignored --nocapture
python3 /tmp/velocity-lane-a-r19/retired-instructions.py \
  --base /tmp/velocity-lane-a-r19/main-58fd98c --candidate target/release/velocity \
  --pairs 9 --output /tmp/velocity-r19-review-instructions-repeat
```

Root `Cargo.lock` is unchanged. No commit was created.

# Round 17 Lane B: fonts, layout, scheme drawing and window controls

Headless tests drive real settings/resource loaders and named/key/mouse dispatch. Native
appearance, AppKit window transitions and GPU costs remain unobserved in this sandbox.
Detailed item/test mapping and retained measurement commands are in `ROUND17_LANE_B.md`.
The fourth verification starts at WIP `7925499` on main `58fd98c`; it stays uncommitted.
The full gate passes: **969 tests, 0 failures**, fmt, strict clippy and rustdoc.
The Lane B module passes **10/10 runs at four test threads** (29 passed each). All 14
application and 2 renderer/font ignored timing tests pass explicitly. Nine paired runs
pass the retired-instruction gate (features -0.37%, typing1 +0.42%, typing1000 -0.13%).
The wall gate also passes every percentile. Nine fresh font processes measure a median
first-non-ASCII call of **0.087 ms** and RSS **7.203 / 7.578 / 20.000 MiB** before primary,
after primary and after fallback. The asynchronous face is ready in 66.726 ms (median).
Details, raw boundaries, ligature costs and ragged-selection measurements are in
`ROUND17_LANE_B.md`; this pass's evidence is in `/tmp/velocity-lane-b-fourth/`.
Sealed read-only system fonts, including the private PingFangUI font, are mapped after
canonical-path/mount/firmlink checks; AssetsV2 and user fonts use retained copies. Actual
host paths, mapping choices and refreshed RSS measurements are in the review record.
The current docs do not establish B6's proposed punctuation/CJK wrapping rule; the settings
reference explicitly excludes word wrapping from `word_separators`. That behavior stays as
before. `always_show_minimap_viewport` changes from true to the documented false; hover or
minimap dragging reveals the viewport. New ruler preferences retain the existing solid stroke.

Run an unbundled reviewer-built release binary with an expendable profile; no packaging step
is needed. Leave these boxes unchecked until observed on an unlocked macOS desktop.

- [ ] **Font fallback (native font service):** start with Menlo and an ASCII file. Insert
  Hangul `한`, Thai `ไ`, Devanagari `अ`, Hebrew `א`, Arabic `ع`, math `𝒜`, Braille `⠿`,
  Chinese `你好` and emoji `🦀 ❤️ ☺️ ✔️ ⚠️ ©️ ↔️ ☀️ ♥️ ✌️ 1️⃣ 👩‍💻 🇨🇳 👍🏽`. The first frame stays
  responsive while the font cascade resolves, then missing glyphs repaint without another input event. Private-use glyphs
  require an installed font that actually covers them. Try a Nerd Font private-use glyph.
  Compare bare `❤` and text-presentation `❤︎` with emoji `❤️`. Compare `⛑︎` before and
  after loading an emoji: it must retain its text face. Also try `⌚︎ 🌡︎ 🖥︎ a⌚︎b`: where
  the system provides only color glyphs, they remain visible and stop requesting frames
  after the cascade resolves. Compare bare `⌚ ⌛ ⏰ ☕ ⚡` before and after their VS15 text
  forms: the bare forms stay emoji, while bare `❤` stays text. Try `1︎⃣ #︎⃣ *︎⃣` plus
  VS16 and selector-free keycaps: no enclosing mark or selector produces a box; VS15
  retains an available text face. Check bold/italic fallback and
  emoji bitmap orientation. Replace/truncate a disposable user-installed font while it is
  loaded; the loaded snapshot stays usable. RTL ordering is outside this round.
- [ ] **Ligatures and antialiasing:** use Fira Code (external, not bundled). Compare `-> != ===`
  with `no_liga`, `no_clig`, `no_calt`, `dlig` and `ss01` enabled individually. A syntax token
  boundary stops the ligature. Move/click every caret position within a ligature and select
  half of it: each occupied monospace cell has its own input position. Compare `no_antialias`
  and `gray_antialias`; macOS keeps grayscale for `subpixel_antialias` (no RGB subpixel pipeline).
- [ ] **Font metrics and zoom:** compare the recorded Menlo 13pt line height with main at
  both 1x and Retina density, including gutter, minimap, wrapped rows, scrolling and drag
  autoscroll. Zoom with Cmd+= / Cmd+- in several tabs, create another tab, restart, then reset.
  The top visual row/fraction stays fixed in active/background tabs and across a 1x/2x screen
  move. Restore old pixel sessions at line 1000 with default settings, font size 16 and
  4px top padding. Save an old-format lazy tab after zooming, restart and visit it: its row
  stays fixed. Restore a file shortened from 3000 to 101 lines; it stays visible. Repeat with
  background loading. Hold zoom for ten repeats while frames run: every press persists;
  subsequent external User edits still apply. Block a package download, zoom and edit a
  syntax preference: the resource loader stays idle after publishing that external change,
  then reloads once the queued writes complete. An in-place repair of broken User JSON
  reloads. Opening the syntax picker in the status bar keeps the current scroll.
  All tabs use the persisted User size;
  an explicit distraction-free size keeps its precedence while zoom updates the normal User
  size. Reset removes that key or uses `default_font_size`.
  User comments and unrelated preferences survive. Switch font family and split panes too.
- [ ] **Minimap:** compare 10-line, long and wrapped files, including folds. Short files stay
  near the top; each visual row keeps its fixed pitch. Scroll to top/middle/end and click the
  corresponding minimap row. Hold a press and send stationary drag events: scrolling stays
  still. Small moves scroll proportionally; drag the viewport from top to bottom within the
  control on long/wrapped/folded files: the final line is reached, and the box stays beneath
  the pointer during and after release. Release over the status bar, wheel-scroll, click the
  minimap again, then drag text: no stale capture or caret jump. With a long file at the
  top, middle and end, click outside the viewport box: the line drawn beneath the pointer
  becomes the editor centre. Drag immediately afterward: no initial jump. Repeat outside
  presses near the top, middle and bottom, dragging in each direction to the control's end:
  both document ends are reachable, with no movement from stationary events. Toggle
  `minimap_horizontal_scrolling`; only that setting pans
  token positions horizontally. Hover/drag reveals the viewport; `draw_minimap_border` alone
  enables its outline. Hidden minimaps do no sampling work.
- [ ] **Window state:** toggle minimap, tabs, status bar and side bar in two windows. Enter
  distraction-free mode, choose different visibility there, create/open/clone/move tabs and
  restore a session. Every tab follows its window; both modes remember their separate choices.
- [ ] **Simple full screen:** test normal and distraction-free entry with
  `use_simple_full_screen: true`. The window fills its current screen without a new Space,
  retains keyboard focus, and auto-hides menu bar/Dock. Exit restores the exact frame/style.
  Repeat on another screen and with two windows; close one while the other is full screen.
  Also verify ordinary native full screen and entering distraction-free mode from it.
- [ ] **Scroller paging (native preferences):** choose each macOS “Click in the scroll bar to”
  behavior. Track clicks page or jump accordingly; Option reverses it; dragging the puck stays
  continuous. Repeat with `scroll_past_end` false, true and 0.5, at the final page. The last
  line stays visible after scrolling or dragging the thumb all the way to the end.
- [ ] **Text drag delay (native preferences):** explicitly set `NSDragAndDropTextDelay`,
  restart and drag existing selected text before/after that delay and beyond 4px. Early
  movement starts a normal selection and never becomes text dragging later in that gesture;
  a quick release places the caret. Positive values are milliseconds (try 300); zero removes
  the delay and negative disables text dragging. A permitted drag moves/copies as before.
  Unset retains the prior distance-only behavior. Release, focus loss and cancellation clear the gesture.
- [ ] **Scheme drawing:** use an original scheme with bracket/tag `foreground`, bracket
  `bold italic` and each underline type. Foreground alone recolors glyphs without underlining.
  Put `font_style: glow` on one token: the soft glyph-shaped pass affects only that run.
  Compare disabled glow costs with the recorded CPU evidence and measure native GPU cost.
- [ ] **Selection/block caret corners:** compare `round`, `cut` and `square`, radius 0/2/large,
  transparent borders, jagged multiline selections, wrapped/folded lines, horizontal scrolling,
  very long lines and split panes. Test borderless fills too, and Select All on ragged code at
  50/200/1000 visible rows. Connected selections have no internal per-row border seams;
  empty carets retain the cheap rectangular path. Inspect concave as well as convex joins.
- [ ] **Rulers and indentation menus:** try `[80,[90,"dotted"],"bad",[100,"stippled"]]`, global
  `ruler_style` and fractional `ruler_width`; the bad entry affects only itself. Check clipping
  during horizontal scrolling. Run all View > Indentation rows and palette equivalents, then
  undo conversions once. Each conversion updates the tab typing preference.

# Round 18 Lane A review fixes (native review pending)

Baseline: main `77c071c`; branch `lane-a`, uncommitted. Latest verification evidence is in
`/tmp/velocity-lane-a-r18-verification/`. No GUI or packaging run was performed. All native boxes
below remain unverified. Lane B's files and the merged keymap blocks are preserved.

- [ ] Fresh startup with a disposable profile and bundled resources: launch twice, then reload.
  No modal or warning status appears. Resource diagnostics remain reachable in `Velocity.log`.
- [ ] Compare `indent_to_bracket` off/on and a live syntax override: `call(one,` aligns;
  Python `result = compute(` and JSON `[\n    1,` keep normal hanging indentation;
  `foo(|)` still expands a pair on Enter. Try comments, strings, tabs, Unicode and many carets.
  On a stale 63 KB single-line JavaScript file, Enter must not synchronously parse scopes.
- [ ] Compare `move_to_limit_on_up_down` off/on, Shift extension, wrapping and a focused input.
- [ ] Find/Replace/Incremental Find: compare single-line, multiline and empty selections with
  `find_selected_text` off/on. Select new text and invoke Find again while the panel is open.
  Compare `auto_find_in_selection`, both close-after-all settings and explicit shortcuts.
- [ ] HTML/XML `</`: nested/void/self-closing tags, case-sensitive XML, namespaces, quoted `>`,
  comments, CDATA and script/style. Markdown inline/fenced code must not close tags.
  In `<div><|>` type `/`: the caret finishes after `>`. Immediately edit the syntax/comment
  context before `/`. Try a large stale file; completion may wait for exact scopes and must
  cancel after another edit or caret move. Very long viewport lines without exact scopes
  stay literal. PHP and user syntaxes embedding HTML also work before scope publication.
  A focused input must not edit the document.
- [ ] Auto-pair `(`, `[`, `{`, double/single quotes immediately after another keystroke and
  during macro replay in a large file: no synchronous document parse or typing pause.
- [ ] Snippets: verify the documented variable table, zero-based **line** `TM_LINE_INDEX`,
  system full name and syntax-base `TM_SCOPE`. Tab-trigger variables for `sel` and `s.` have
  empty selections and compute current word after removing the trigger. Real selections remain.
  Test punctuation suffixes and longest trigger. `cat.`, `elif?` and `result.` must not match
  interior word triggers. Duplicate triggers offer choices; stale choices do not insert.
  At the blank line after `// comment`, the palette includes `source.rust - comment` snippets.
  Go/Rails template variables and custom `.tmPreferences` shell variables expand correctly.
  A scoped trigger in a large stale file uses an untouched published caret line, otherwise
  inserts an ordinary tab until scopes arrive; it must not pause to parse the file.
- [ ] Open the palette immediately after editing a 1.63 MB minified JS file: no pause.
  Unavailable scopes initially leave snippets visible; current scopes refilter an unchanged
  open query. Changing the query or closing the palette cancels that deferred filtering.
- [ ] Tab with one or several candidates inserts the best match directly. With candidates
  `vec`, `vector`, `vec_len`, the exact `vec` snippet wins. Type `fn` with the typing popup
  visible and snippets excluded: Tab expands the exact snippet even with `tab_completion: false`;
  Enter still commits the selected popup row. Preserve snippet fields, indentation and selected
  text. Test popup commits after an intervening edit.
- [ ] Completion triggers: selector-only, characters-only and combined entries, `rhs_empty`,
  `.`, `::`, comments and embedded code. Typing in panels stays isolated.
- [ ] Suffix/order defaults: inferred spaces **false**, preserve order **"some"**. When spaces
  are enabled, one assignment is insufficient evidence; `foo(cou|)` completes to `foo(count)`.
  Repeated evidence may add spaces at the immediate end of line, including EOF. Existing
  whitespace remains single; `.`, `(`, `:`, quotes and `>` never gain an inferred space.
  Compare punctuation inference,
  existing suffixes and authored contents. Check `none`/`some`/`strict`, legacy booleans
  (`true` → strict, `false` → some), proximity and history.
- [ ] Popup rendering in both themes: kind containers/letters, match colors, selected row,
  footer/details, custom Unicode kind and narrow panes.
- [ ] Focus saves off/on: save only the file being left on tab/application changes. Other dirty
  loaded tabs and lazy hot-exit drafts stay unsaved. Cloned views save once. Untitled, read-only,
  loading, deleted, composing and externally changed files never save or show a save dialog.
  I/O/encoding failures produce a status message without a modal loop.
  Type `</` in a large stale HTML file and switch immediately: a valid pending completion
  finishes within its scope budget or is canceled before saving. Later background publication
  must not edit the document or make a just-saved buffer dirty. Check with focus saves off too.

Public documentation does not establish new defaults for history, snippets while typing,
the trigger list, inferred spaces or preserve order. Existing defaults remain `true`, `false`,
`[]`, `false` and `"some"`. Empty-caret find seeding is unchanged. See `docs/settings.md` for
sources, implementation links and headless coverage. No new menu rows are needed.

The syntax cache schema is now `velocity-syntax-roots-v1-14`, retaining the load-time
HTML/XML embedding flag without forcing lazy contexts to load on cached startup.

Headless verification (this review): fmt, strict release/locked workspace clippy over all
targets, `cargo test --workspace --locked --release` with default parallelism, and rustdoc
with `RUSTDOCFLAGS="-D warnings"` all pass. The suite reports **914 passed, 0 failed,
13 ignored** across 114 executables/doc-test suites. All four round-18 ignored timing
tests were run explicitly and passed. Logs: `clippy.log`, `workspace.log`, `rustdoc.log`,
`many-caret.log`, `large.log`, `suffix.log`, `scope-budget.log`.

Six alternating pairs against main `77c071c` pass the ≤5% performance gate using the
requested busy-host fallback, Darwin retired instructions (`instructions/report.json`):

| Benchmark | Main instructions | Candidate instructions | Change |
| --- | ---: | ---: | ---: |
| Typing, 1 caret | 425,786,878.5 | 425,627,868 | −0.04% |
| Typing, 1,000 carets | 10,710,958,721.5 | 10,763,835,596 | +0.49% |

The report retains samples, loads, hashes and counter details, and its candidate hash matches
the final release binary. These are complete headless child-process totals (including setup
and background work), read through `proc_pid_rusage(RUSAGE_INFO_V4)` after exit before
reaping; they do not measure native presentation. One-minute load ranged **14.42–15.50**
during this check. Retired instructions are the accepted performance result for this pass;
no native presentation or paired wall-time result is claimed.

The ignored timings below are single observations, not statistical comparison claims.
The large stale HTML case measures slash insertion; completion then waits for exact scopes.
The minified viewport case remains literal when exact scopes cannot be provided.

| One-caret input case | Scopes ready (ms) | Scopes stale (ms) |
| --- | ---: | ---: |
| 20,000-line JavaScript `</` | 0.062 | 0.018 |
| 1.47 MB HTML `</` | 0.073 | 0.017 |
| 20,000-line Rust Enter, bracket alignment on | 0.077 | 0.022 |
| 1.73 MB minified HTML, viewport mode | — | 0.022 |
| 63 KB single-line JavaScript Enter | — | 0.050 |
| 1.63 MB minified JavaScript palette (seven-run median) | — | 0.240 |

Both new stale-line/palette cases assert zero synchronous parses. The palette median is
asserted below 10 ms; all seven samples were below 0.8 ms.

| 10,000-caret input hook | Setting off (ms) | Setting on (ms) |
| --- | ---: | ---: |
| Ordinary typing | 0.979 | 0.945 |
| Trigger character `.` | 0.915 | 0.916 |
| HTML slash | 12.935 | 17.478 |
| Enter / bracket alignment | 384.333 | 117.356 |
| Typing with suffix inference | 1.125 | 1.085 |

Suffix commits took 1.473 ms off / 13.275 ms on; the 2,000-word × 2,000-caret worker query
took **43.630 ms off / 44.013 ms on**. Missing exact scopes retain normal indentation;
these batch timings include that safe fallback and asynchronous worker scheduling. Individual
wall times are sensitive to the busy host and do not establish an on/off speedup or regression.

Reproduce without GUI or packaging:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --release --locked -- -D warnings
cargo test --workspace --locked --release
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo test -p velocity --release --locked many_caret_input_hooks_timing -- --ignored --nocapture
cargo test -p velocity --release --locked large_document_input_hooks_timing -- --ignored --nocapture
cargo test -p velocity --release --locked palette_and_long_line_scope_budget_timing -- --ignored --nocapture
cargo test -p vel-complete --release --locked many_word_many_caret_suffix_query_timing -- --ignored --nocapture
python3 tools/scroll-gate/paired-headless.py \
  --base /tmp/velocity-lane-a-r18-review/main-target/release/velocity \
  --candidate target/release/velocity --bench typing1 --bench typing1000 --pairs 6 \
  --output /tmp/velocity-r18-review-repeat
python3 /tmp/velocity-lane-a-r18-verification/retired-instructions.py \
  --base /tmp/velocity-lane-a-r18-review/main-target/release/velocity \
  --candidate target/release/velocity --pairs 6 --output /tmp/velocity-r18-instructions-repeat
```

The baseline is an export of `77c071c` using the exact checked-in third-party submodule
revision. No dependencies or menu rows changed; `Cargo.lock` is unchanged.

# Round 17 Lane A final verification fixes (native review pending)

The latest regressions use `App::run_named`, real key dispatch and the package/resource
loader. Evidence for this pass is in `/tmp/velocity-lane-a-r17-final-review/`; earlier
measurements below are historical. Syntax cache schema `velocity-syntax-roots-v1-13`
invalidates cached diagnostics that lack an originating resource path. Native checks
remain unobserved in this sandbox.

The final gate passes: fmt, strict release/locked workspace clippy over all targets,
`cargo test --workspace --locked --release` at default parallelism (**807 passed, 0 failed,
4 ignored**, 114 test executables), and rustdoc with `-D warnings`. The standalone syntax
runner reports **126,600 passed / 0 failed / 0 skipped**, including **35/35 reindent
variants** (203 syntaxes, 242 files), unchanged from the incoming verified candidate. The
first gate attempt hit the existing TLS fixture's empty port-file publication race in
`vel-packages/tests/https.rs:74`; the unchanged full rerun passes. Both logs are retained.
No golden content, insert-session code or Lane B drawing/platform files changed in this pass.

Fresh five-run medians against frozen main `99d1bb4` (headless CPU only):

| Metric | Main | Final candidate | Change |
| --- | ---: | ---: | ---: |
| syntax parse MB/s | 5.043145 | 5.000526 | -0.85% throughput |
| full highlight MB/s | 3.753227 | 3.772233 | +0.51% throughput |
| highlight edit p95 ms | 0.002459 | 0.002459 | unchanged |
| completeness query ns (nine pairs) | 0.954861 | 0.954861 | unchanged |
| cached syntax load ms, including content hashes | 11.383291 | 11.288042 | -0.84% time |
| YAML load ms | 871.061333 | 866.217666 | -0.56% time |
| 100-line syntax window ms | 0.010500 | 0.010375 | -1.19% time |
| convergence ns | 125 | 167 | +42 ns (+33.6%), one clock tick |

Throughput remains within 3%, and cached loading does not slow down. The convergence
sample spans three versus four clock ticks; the measured Mach timebase is 125/3 ns per
tick. Two alternating complete syntax-benchmark pairs use **207,762,213,832 →
204,935,798,129 retired instructions (-1.36%)**, within 3%, counted with Darwin
`proc_pid_rusage(RUSAGE_INFO_V4)` after exit and before reaping. These are whole-process
totals, including YAML/cache loading, warmup, five 10 MB parse passes, windows and convergence;
they do not isolate the convergence case. Raw wall-clock samples and the one-tick increase
remain in the evidence. No throughput measurement was discarded or rerun.

Twenty-one alternating typing pairs all pass the 5% gate, milliseconds:

| Bench | p50 main → candidate | p95 main → candidate | p99 main → candidate |
| --- | --- | --- | --- |
| typing1 | 0.200020 → 0.200230 (+0.10%) | 0.347796 → 0.345809 (-0.57%) | 7.117584 → 7.104674 (-0.18%) |
| typing1000 | 5.244021 → 5.251167 (+0.14%) | 14.574602 → 14.527039 (-0.33%) | 27.041032 → 26.992232 (-0.18%) |

Logs, samples, load conditions, timer resolution, binary identities and the focused patch
are in `/tmp/velocity-lane-a-r17-final-review/`. The candidate SHA-256 is
`3be783265c86a85682db88c9775bcf938ef4822dae5b5b251c03395261a1a7f5`; the typing baseline is
the frozen `/tmp/velocity-lane-a-r17/velocity-main-99d1bb4` binary. Benchmark-generated tracked
timing files were restored after measurement. All changes remain uncommitted; no GUI,
packaging script or `cargo clean` was used.

- [ ] **Cold/stale tab conversion:** select Bash and immediately run Expand Tabs. Repeat
  after highlighting finishes, insert `#`, and immediately run it again. Test Unexpand Tabs
  too. In a `<<-EOF` heredoc, content and tab-prefixed terminator bytes remain unchanged;
  code indentation converts in one undo step. Include a Ruby embedded heredoc whose target
  falls back to Plain Text. Repeat with Find focused.
- [ ] **Literal reindent:** enable spaces and reindent a function containing a case branch
  with `cat <<-EOF`. Mixed tab/space content and `\tEOF` remain byte-identical. Try Python
  triple strings and C++ raw strings too; ordinary following code still reindents. A bare
  multiline JavaScript template must not shift the following function.
- [ ] **Reachable, non-modal diagnostics:** launch from Finder with two missing references
  in a User syntax, malformed mousemap/scheme entries and a corrupt installed package.
  Status summaries name the first offending file and `Velocity.log`; no warning opens an
  alert. Open `<data-dir>/Velocity.log` with File > Open and check every filename and reason.
  The default macOS path is `~/Library/Application Support/Velocity/Velocity.log` (unless
  `--data-dir` or `VELOCITY_DATA_DIR` overrides it). Restart and reload resources: details
  append to the same log, and healthy neighboring resources still work. A bundled-only
  profile produces no warning status or alerts.
- [ ] **Warning provenance:** install a healthy User syntax named like a bundled syntax
  with a missing optional target. Bundled diagnostics must not become User status warnings.
  Add a bad reference to the User syntax: only its own warning is counted. Repeat using
  loose packages and Installed Packages, then restart from the cache.
- [ ] **Empty Vintage repeat:** after `dd`, try `i<Esc>.`, `A<Esc>.` and `R<Esc>.`.
  Dot repeats the empty insertion; it must not delete another line. Undo restores `dd`
  without a phantom step. The fix is at the command boundary; insert-session code is untouched.
- [ ] **Shell lexical cases:** reindent Bash and Zsh branches containing `${#a[@]}`,
  `${x:-y}` and nested parameter substitutions. Subsequent `;;` and patterns stay at the
  correct level. A one-line `case $x in a) echo a;; esac` inside a function must not deindent
  the next statement or the function's closing brace.

# Round 17 Lane A earlier review fixes (historical headless evidence)

All eight reviewer findings have dedicated headless regressions. The implementation remains
uncommitted. The original round-17 evidence below is retained; validation and measurements
for this earlier review pass are in `/tmp/velocity-lane-a-r17-review/`.

Headless validation for the review fixes: fmt, strict release/locked workspace clippy over all
targets, `cargo test --workspace --locked --release` at default parallelism (**800 passed,
0 failed, 4 ignored**) and rustdoc with `-D warnings` pass. The syntax suite remains
**126,600/126,600**, including **35/35 reindent variants** (203 syntaxes, 242 files).
The three legacy scalar forms load and highlight through both fresh and cached real package
loading, beside a corrupt neighboring archive. Warnings are retained in `SyntaxSet::warnings()`;
only the presentation channel changes. The cache schema for that pass was `velocity-syntax-roots-v1-12`
so older linked references cannot survive this fallback repair.

Fresh five-run medians against frozen main `99d1bb4` (headless CPU only):

| Metric | Main | Review candidate | Change |
| --- | ---: | ---: | ---: |
| syntax parse MB/s | 4.732084 | 4.754804 | +0.48% throughput |
| full highlight MB/s | 3.533281 | 3.513311 | -0.57% throughput |
| completeness query ns (nine pairs) | 1.041667 | 0.984028 | -5.53% time |
| cached syntax load ms, including content hashes | 11.769500 | 11.712000 | -0.49% time |
| YAML load ms | 921.537791 | 923.119083 | +0.17% time |
| 100-line syntax window ms | 0.011042 | 0.010666 | -3.41% time |
| convergence ms | 0.000125 | 0.000125 | unchanged |
| highlight edit p95 ms | 0.002542 | 0.002625 | +3.27% (+83 ns) |

The machine's one-minute load was 6–12 on eight cores during these wall-clock measurements;
the near-threshold edit sample is supplemented with retired-instruction measurements below.
Throughput stays within 3%; cached loading does not slow down. Raw logs, binary identities,
and every sample are in `before/`, `after/`, `bench-conditions.json` and `bench-summary.json`
under the review evidence directory.

Two alternating complete highlight-benchmark pairs, counted with Darwin
`proc_pid_rusage(RUSAGE_INFO_V4).ri_instructions` after exit and before reaping, give
**199,998,475,736.5 → 200,637,924,495 instructions (+0.32%)**, within 3%. The process includes
five full 10 MB passes, edit samples and syntax loading; these are process totals, not isolated
edit counters. `highlight-instructions.json` records all samples and binary hashes. The 83 ns
wall-clock edit-p95 increase above remains visible rather than being omitted from the evidence.
The unchanged structural golden also passes **12 additional runs in four parallel processes**;
`golden-repeats.json` and individual logs retain those results.

Twenty-one alternating typing pairs all pass the 5% gate, milliseconds:

| Bench | p50 main → candidate | p95 main → candidate | p99 main → candidate |
| --- | --- | --- | --- |
| typing1 | 0.215126 → 0.215521 (+0.18%) | 0.369756 → 0.373337 (+0.97%) | 7.012600 → 7.035121 (+0.32%) |
| typing1000 | 5.420146 → 5.427042 (+0.13%) | 15.010235 → 14.985519 (-0.16%) | 27.744109 → 27.756192 (+0.04%) |

The reviewer can reproduce the gate with the commands retained in the original round-17
section below. For these measurements use `/tmp/velocity-lane-a-r17-review/paired` as the output
for `paired-headless.py` and compare against `/tmp/velocity-lane-a-r17/velocity-main-99d1bb4`.
The candidate hash is `5d88d22062e2020fa2ad6b1f7e691b265be7e0a4aad6b8fd3f584b6eb338a506`.
Direct native `--bench-open` remains unchecked in this sandbox; its message-queue prerequisite
is covered by fresh `App`/`Workspace` loading, cache reuse and resource reload tests. No GUI,
packaging script or `cargo clean` was used. The previously unverified implicit Shift+multi-click
and Cmd+plus defaults remain unchanged; explicit mousemap mappings are covered headlessly.

- [ ] **Launch diagnostics:** launch a fresh profile twice; no syntax alerts or warning status.
  Install a User syntax with two missing references: one status naming its file and
  `Velocity.log`, with both details in that log. Repeat with bad scheme globals, a User
  override of a bundled scheme filename and two malformed mousemap entries; healthy values
  and bindings work, with one non-modal summary per resource kind. Reload and restart from cache.
- [ ] **Shell reindent:** inside a function, reindent `case "${1}" in`, `${x:-y}` and `${#a[@]}`.
  Clauses remain at column 4, bodies at 8. Put a function inside a case branch: its brace stays
  at 4 and `esac` at 0. Try brace-style Zsh cases, including a single-line case followed by another.
- [ ] **Compatible user grammars:** load quoted capture indices, an empty `scope:` and numeric
  extensions `[man, 1, 2, 3]`, next to a broken package. Select each syntax, check highlighting
  and reopen a `.1` file. Only discarded malformed data should produce warnings.
- [ ] **Fold ink:** use a foreground/background-only scheme or legacy tmTheme, fold a selection
  and move the caret away. The ellipsis remains visible. Missing borders remain optional.
- [ ] **Vintage repeat:** `dd`, then `rb` on `b`, `~` on a digit or `guu` on lowercase text,
  then `.`. Dot repeats the attempted no-op; Undo restores the deletion without a phantom step.
- [ ] **Missing embeds:** open a Ruby `<<~'HTML'` heredoc without its embedded HTML syntax.
  Inspect `text.plain` inside it; highlighting resumes after the terminator. A missing context
  within an installed syntax stays empty and its escape still works.
- [ ] **Indentation focus/content:** leave Find focused while invoking all three indentation
  commands. They act on the document and retain the query/focus. With cold, current or stale
  scopes, convert a shell `<<-EOF` heredoc in either direction: content and terminator tabs remain byte-identical.
  Also try a multiline string, Undo/Redo and read-only documents.
- [ ] **Theme fixture:** compare native light/dark phantom, popup and annotation scenes. The
  headless golden now explicitly snapshots the cold dark frame and waits for the matching
  diff publication before the light frame; its frozen JSON content is unchanged.

# Round 17 Lane A: mousemaps, schemes, syntax and indentation (native review pending)

Run a reviewer-built release binary on an unlocked macOS desktop with a disposable profile.
No GUI, AppKit input, Metal, packaging script or `cargo clean` is used in this lane. Keep
these boxes unchecked until observed. Lane B owns native selection corners, bracket glyph
styles/glow, drag delay, scrollbar paging, fonts, minimap and window-state drawing.

- [ ] **A1 mousemaps:** install an original User mousemap with `button1`, numeric buttons,
  `button2`, `button16`, `scroll_up`/`scroll_down`, press/release commands and contexts. Add
  one malformed entry and an unrelated corrupt installed archive. Healthy entries still
  work. Try `drag_select` with `by: words/lines`, `extend`, `additive`, `subtractive`, drag
  past the viewport and release. Secondary/Control-click overrides take precedence when
  configured; an empty mousemap keeps the existing native/context/column-selection behavior.
- [ ] **A1 focus / A2 verification limit:** click/scroll Find and the command palette with
  document-editing mouse bindings installed: document text stays unchanged. Click back into
  the document: its binding runs. Explicit Shift+double/triple-click mappings using
  `extend: true` retain the anchor and extend by word/line in either direction. The implicit
  Shift+multi-click default remains unchanged because the allowed docs do not establish its
  anchor rule; compare it with the reviewer's current editor before proposing that change.
- [ ] **D1/D2/D6 schemes:** select a minimal legacy tmTheme and an explicitly assigned
  `.hidden-tmTheme`. The latter stays out of the picker. Gutter/line numbers, caret line,
  whitespace, guides, rulers, find matches and diffs remain legible. Missing borders are transparent; folded ellipses retain foreground ink. Add bad colors/variables/rules alongside valid ones and change
  numeric-string radii/widths and corner enums; healthy properties survive with diagnostics.
- [ ] **D3 palette:** compare our light/dark schemes and theme with main, including dimmed
  inactive sheets. Colors must look unchanged. User expressions follow the corrected base
  percentage: black `blend(white 25%)` is RGB 191; the inverse is RGB 64. Compare alpha blends.
- [ ] **E1–E6 syntax:** install original parent/child grammars with anonymous contexts,
  embed wrappers and prototypes. Add one malformed grammar and an unrelated corrupt archive.
  Healthy grammars still load on the first launch and on cache reuse. Highlight deeply nested
  JSON/JavaScript, including closing delimiters after the resource limit. Missing references
  warn at load time and cannot abort the highlighting worker. Check diagnostics for unknown
  variables, malformed regexes and wrong value types, then fix/reload the individual file.
- [ ] **E7 reindent:** reindent C/C++ strings/comments containing bracket/comment characters,
  CSS comment-led lines, JS switch cases/templates, JSX attributes with quoted/interpolated
  `>`, shell cases, PHP transitions and Python dedents. Include embedded languages and User
  indentation preference overrides. Undo once; async completion must not overwrite later edits.
- [ ] **F1/F2 commands:** use View > Indentation (Lane B's rows) for `expand_tabs`,
  `unexpand_tabs`, `detect_indentation`. Try mixed leading whitespace, interior tabs, multiple
  directed selections, clones, read-only files and a large buffer. Conversion is one undo
  step; repeat conversion is a no-op. `set_translate_tabs: true` updates only this view's
  typing style. `set_setting {setting, value}` accepts arbitrary JSON on the view layer.
- [ ] **F3 keys:** Ctrl+Cmd+Shift+F toggles distraction free and its existing alias still
  works. Ctrl+Option+Up/Down scroll by a line; Ctrl+Shift+Up/Down still add selection rows.
  Repeat with Find focused. Cmd+= keeps zooming; Cmd+plus is unchanged because its alias
  was not confirmed in the allowed documentation. Test physical keys/layouts after Lane B's
  window/zoom changes are integrated.

- [ ] **Validation-found Vintage regression:** enable Vintage, delete and put back a
  combining grapheme plus Han text, apply `~` to the uncased Han character, then Undo/Redo.
  Undo targets the put rather than a phantom case-change transaction; Unicode carets stay valid.

Headless evidence and frozen main binaries: `/tmp/velocity-lane-a-r17/`.
The compiled-color goldens were captured from main 99d1bb4 before the expression change;
all explicit scheme globals, rule/gradient colors and compiled theme color properties,
including evaluated sheet modifiers in both appearances, are compared exactly.
The syntax corpus now reports the pinned tree's 16 genuine optional missing references;
its test permits only those targets, while rejecting other authoring/linking warnings. Bundled
syntax diagnostics stay in the console. User syntax, scheme and mousemap warnings each produce
one non-modal status summary, including when the syntax cache is reused.

Round-17 validation: fmt, strict locked release clippy (workspace/all targets), the full
locked release workspace tests at default parallelism (**790 passed, 0 failed, 4 intentionally
ignored**) and rustdoc with warnings denied all pass. Syntax tests improve from **126,595 passed /
5 failed / 0 skipped** to **126,600 passed / 0 failed / 0 skipped** (203 syntaxes, 242 files;
35/35 reindent variants). The checked-in corpus baseline now locks in those five fixes.

The workspace property test also found an existing Vintage no-op history bug. The new seed
and `kk2xP~u` reproduce against the frozen main sources (`vintage-main-repro.log`); `Vintage::apply`
now counts only real buffer transactions. Its deterministic Unicode regression and the saved
property-test seed pass. The scope of that small compatibility fix is limited to Vintage's
history adapter; the buffer's accepted no-op filtering remains in effect.

All 112 explicit compiled scheme colors and 882 compiled theme color properties match their
frozen pre-change values. Our minihtml fixture percentages and the theme's legacy array-tint
adapter were migrated too; their existing display-list goldens retain their original colors.

Five-run benchmark medians, main → candidate (10,000,000 bytes, 297,030 lines):

| Benchmark / metric | Before | After | Change |
| --- | ---: | ---: | ---: |
| syntax-bench parse MB/s | 4.965831 | 5.062297 | +1.94% throughput |
| highlight_bench full MB/s | 3.555764 | 3.751701 | +5.51% throughput |
| highlight_bench edit p95 ms | 0.002625 | 0.002459 | -6.32% time |
| completeness_bench optimized query ns (nine pairs) | 0.984028 | 0.954861 | -2.96% time |
| syntax-bench cached load ms, including content hashes | 11.065500 | 11.299375 | +2.11% (+0.234 ms) |
| syntax-bench YAML load ms | 865.052625 | 854.753042 | -1.19% time |
| syntax-bench 100-line window ms | 0.010375 | 0.010541 | +1.60% time |
| syntax-bench convergence ms | 0.000125 | 0.000125 | unchanged |

Syntax/highlight regressions remain within 3%; cached loading has no noticeable increase.
The bench binaries were run without concurrent compilation in this lane. These are headless
CPU measurements and do not establish native drawing/scanout performance.

Twenty-one alternating typing pairs against the frozen main binary, milliseconds:

| Bench | p50 main → candidate | p95 main → candidate | p99 main → candidate |
| --- | --- | --- | --- |
| typing1 | 0.200042 → 0.200417 (+0.19%) | 0.345681 → 0.344500 (-0.34%) | 7.057019 → 7.062445 (+0.08%) |
| typing1000 | 5.263500 → 5.266334 (+0.05%) | 14.604685 → 14.572817 (-0.22%) | 27.088861 → 27.054803 (-0.13%) |

All six typing metrics pass the 5% gate. `paired/report.json` preserves every sample and binary
identity: main `12bae0fbc43a0f23f1944d73976b3448cdd24ec5a73f8796bbaf5014ed9ed3f5`,
candidate `53fa9e79b2101506cdf1b2a4169cfab9fb4da8224c9eddfdaf48e4f71627344a`.
The complete syntax/highlight JSON and command logs are in `before/` and `after/`, with a combined
`bench-summary.json`, under `/tmp/velocity-lane-a-r17/`.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --release --locked -- -D warnings
cargo test --workspace --locked --release
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo run --release --locked -p vel-syntax --bin syntax-tests
cargo run --release --locked -p vel-highlight --example highlight_bench
cargo run --release --locked -p vel-highlight --example completeness_bench
cargo run --release --locked -p vel-syntax --bin syntax-bench
python3 tools/scroll-gate/paired-headless.py \
  --base /tmp/velocity-lane-a-r17/velocity-main-99d1bb4 --candidate target/release/velocity \
  --bench typing1 --bench typing1000 --pairs 21 --output /tmp/velocity-r17-review-paired
```

# Round 16 Lane A: editing, bookmarks/marks and macros (native review pending)

Use a reviewer-built binary with an isolated profile. The sandbox runs headless tests only;
leave every native box unchecked until review. Lane B owns the menu/palette resource rows and
bookmark gutter drawing. `Editor::bookmarks()` exposes the complete mapped regions without an
allocation; `Panels::macro_file_items()` exposes package macro invocations with our own captions.
`run_macro_file` without arguments opens that catalog as a chooser.

- [ ] **1.1 Undo:** at BOF, Backspace; select uppercase text and Upper Case; unindent a line
  without indentation. None creates a dirty marker or an undo step. Undo a real edit, run each
  no-op, then Redo: the edit returns. Repeat with Unicode, several carets, a clone, snippet
  mirrors, language highlighting and an inline annotation. A later real edit updates all of them.
- [ ] **1.2 Column regions:** Ctrl+Shift+Up/Down adds the previous/next visual row at both
  endpoints of a forward or reversed selection. Repeat through short lines, tabs, CJK, wraps
  and a collapsed fold. Follow with Up/Down: the goal column agrees, the active new region is
  revealed, and no endpoint lies inside hidden text.
- [ ] **2.1 Brackets/scopes/tags:** Ctrl+M from before/after either delimiter and inside the
  pair; Ctrl+Shift+M repeatedly selects contents then delimiters then the enclosing pair.
  Include empty/adjacent pairs, unmatched punctuation, Rust/Python/JavaScript strings and
  comments containing their own brackets. An unmatched string bracket cannot pair with code.
  Cmd+Shift+Space expands a scope. Cmd+Shift+A expands nested HTML/XML tags, including quoted
  `>` attributes and fake tags in comments. Run indentation expansion through the menu/command;
  it has no default key binding. Repeat with reversed and multiple selections.
- [ ] **2.2 Comments:** Cmd+/ on an empty HTML/CSS selection comments its whole line;
  pressing it twice immediately restores the line even before highlighting catches up.
  From inside a single/multiline block comment, Cmd+/ and Cmd+Option+/ remove its delimiters.
  Include an empty line, an indented line, multiple carets on one line, and Rust line/block
  comments. Undo restores the complete previous state.
- [ ] **2.3–2.5 Transpose/wrap/permutation:** Ctrl+T advances after swapping adjacent Unicode
  graphemes, and circulates differently sized selected strings. Cmd+Option+Q retains indentation
  and the active language's `//`, `#`, `--` comment prefixes on every output line. Plain Text
  and Markdown keep list/heading markers only on their original first line. Test positive
  `wrap_width`, zero with the first ruler, and explicit command width. Unique/shuffle/reverse
  with only carets permutes the entire buffer, including an unterminated last line.
- [ ] **2.6–2.7 Join/indent:** Cmd+Shift+J retains forward/reversed selected text and endpoint
  positions while joining several lines, including a whitespace-only intervening line. Keep
  Cmd+J as its existing chord prefix. Spacing before closing brackets and indentation of empty
  lines retain the earlier behavior, pending exact documented rules (verification note below).
- [ ] **3 Bookmarks:** Cmd+F2 stores full directed regions or empty carets. F2/Shift+F2 wraps
  between them, Option+F2 selects all, Cmd+Shift+F2 clears them; `select_bookmarks` remains an
  alias. Type before/between bookmarks, Undo/Redo, save/quit/relaunch, clone, and restore after
  an external disk change invalidates history. Regions and direction survive. After Lane B's
  drawing is integrated, verify one gutter icon on every covered bookmarked line, clipped to
  the visible gutter and following edits/folds.
- [ ] **3 Marks:** Cmd+K then Cmd+Space sets the mark; Cmd+A selects to it, Cmd+W deletes to
  it, Cmd+X exchanges it, Cmd+Y yanks the last killed text, Cmd+G clears it (second keys also
  have Cmd held). Test multiple carets, Unicode, edits before the mark and a session restart.
  Kill in one tab and yank in another; the application kill ring distributes matching counts.
  `clear_bookmarks {"name":"mark"}` clears only the mark and its primary-region metadata.
- [ ] **4 Macros:** Ctrl+Q starts/stops one application macro; switch tabs/windows while
  recording and keep the recording indicator/context. Record typing, an Add Line Before macro,
  Find Next/Prev/Under/All and a replacement; tab/window commands must stay out of the recording.
  Ctrl+Shift+Q applies each recorded command once across all carets. The find result must be
  selected before the next recorded edit. Change the visible query before playback: captured
  find options still reproduce the recording. Retest `redo_or_repeat`.
- [ ] **4 Files/UI:** after Lane B's menu rows are integrated, use Tools Record/Playback/Save
  Macro and Macros entries. Save writes JSON in the profile's `Packages/User`; an occupied
  name gets a free numeric suffix, and all existing files/backups remain intact. Save three
  recordings using the default name. Cancel the naming panel; choose an invalid name; test
  an unwritable profile. Play a macro from another loose/installed package, override/reload it,
  disable that package, and try malformed/recursive files. Errors restore recording state and
  preserve editor usability. The catalog and no-argument chooser list effective package files.

Verification limits: the public command reference confirms the first-ruler fallback and
transpose semantics. The official changelog confirms find recording (4107) and maintained
Join Lines selections (4213). Build 4166 only says that Join Lines and empty-line indentation
improved; it does not specify the exact closing-bracket spacing or blank-line rule. The public
macro guide does not specify one undo step per playback. Those three rules and the existing
70-column hard-wrap fallback remain unchanged; source comments record the references.

Headless validation and retained evidence (no native AppKit/Metal/scanout result is implied):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --release --locked -- -D warnings
cargo test --workspace --locked --release
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo test -p velocity --release --locked command_hook_timings_large_buffer_many_carets -- --ignored --nocapture
python3 tools/scroll-gate/paired-headless.py \
  --base /tmp/velocity-lane-a-r16/velocity-main-32506af --candidate target/release/velocity \
  --bench typing1 --bench typing1000 --pairs 21 --output /tmp/velocity-lane-a-r16/paired-final
```

The baseline was frozen before edits. Current `main` at `1433d8b` has identical editor/build
sources to `32506af`; its only additional changes are packaging forwarding fixtures, which
are not compiled into the compared editor. Raw timing logs, hashes and summaries are retained
under `/tmp/velocity-lane-a-r16/`. Packaging and `cargo clean` are not run for this round.

Initial submission validation: fmt, strict release clippy, the full locked release workspace tests
(default parallelism: 749 passed, 3 intentionally ignored), and rustdoc with warnings denied
all passed. The ignored command-hook timing test was then run explicitly and passed.
The existing `empty_window_sidebar_omits_input_adapter_before_and_after_preview` regression
also passed; the hidden input adapter hook was retained.

Paired CPU typing results: 21 alternating pairs, median of each run's reported percentile,
main → candidate milliseconds (the combined editing/CPU scene path; no native/GPU claim).

| Bench | p50 main → candidate | p95 main → candidate | p99 main → candidate |
| --- | --- | --- | --- |
| typing1 | 0.200896 → 0.201500 (+0.30%) | 0.349953 → 0.345797 (−1.19%) | 7.099487 → 7.157677 (+0.82%) |
| typing1000 | 5.263104 → 5.267021 (+0.07%) | 14.557619 → 14.606450 (+0.34%) | 27.084054 → 27.147966 (+0.24%) |

All six metrics pass the 5% gate. Earlier 5-/7-pair runs crossed the threshold on variable
samples; their raw data is retained in `paired-initial` and `paired-current`. The final run
above followed all builds and tests, with no concurrent compilation in this lane.
`paired-final/report.json` retains binary SHA-256 values and all paired samples.

Command hooks, milliseconds per command averaged over 20 invocations with 10,000 carets:

| Buffer | Insert empty text (no-op) | Type with 10,000 bookmarks | Type without bookmarks | Type while recording |
| --- | --- | --- | --- | --- |
| 1 MiB | 0.5096 | 1.6481 | 1.1092 | 1.0711 |
| 32 MiB | 0.3009 | 1.2454 | 0.9913 | 1.0213 |

The same probe includes one-caret cases in `hook-timings.log`. No-op comparisons touch only
the edited slices. Bookmark mapping uses the current change set and preserves region order;
macro recording appends one command independently of selection count and history length.
The review fixes below restore a stable sort after bookmark mapping; replacement-boundary
bias can reverse their order. The sort is linear for already sorted insert/delete mappings.
Bracket commands use the existing interval tree to collect only enclosing/touching pairs,
and scope expansion walks only the active contiguous occurrence, with the root in constant time.

Round-16 review corrections (all native boxes remain pending):

- [ ] Record Find Next with Cmd+G and Enter, Find Prev with Shift+Enter, then an edit. Replay
  with Ctrl+Shift+Q while a different query is focused. Compare text/carets with the manual
  sequence. Each Find records exactly once. Repeat with Replace Next followed by an edit.
- [ ] Save a user macro and invoke its Tools entry; replay package macros containing Find
  and a subsequent edit. Find finishes before the edit. A tab/window action that closes or
  changes the starting view stops playback before later edits, with status-bar feedback.
  Repeat macro-backed editing keys with the Find field focused: they edit the input, and
  the document stays untouched. Document-started playback still applies its steps to the document.
- [ ] Bookmark at columns 2 and 4 of `a\nb\nc\nd\n`; replace bytes 2..5 with `X`, then
  navigate both ways. Toggle at another column on a bookmarked line, and over a partially
  overlapping region: remove the matching bookmark. Repeat after edits move the bookmark.
- [ ] Set a mark at the caret and delete to it. No character, dirty state, undo step or kill
  entry changes. Repeat with one empty and one nonempty mark/caret pair, then Yank.
- [ ] Paste and Indent text identical to the selection, alone and alongside a different
  replacement. Every affected selection collapses as after a normal paste; only real byte
  changes dirty the document or publish layout/language changes.

The new regressions use `App::run_named` and the shared native/headless key dispatcher for
macro recording, naming-panel completion and playback. The optional native window handle
isolates clipboard/dialog/frame operations; the Registry, action queue, panel interception,
workspace/tab switching and ordered macro executor are the production implementations.
Bookmark toggling uses the review's line/overlap fallback: the public command reference says
only active regions, and the build-4166 changelog does not define the improved toggle rule.
Sources are cited in `ops::toggle_bookmark`. Legacy round-seven restore probes cover an
unsaved cursor-zero journal and no-op steps before/after cursor/save indexes.

Review validation: fmt, strict locked release clippy, the full locked release workspace
tests (default parallelism: 762 passed, 0 failed, 3 intentionally ignored), and
rustdoc with warnings denied all pass. The hook timing test passes when run explicitly;
the sidebar regression remains passing. The two existing macro-backed keymap regression
tests now drive the application dispatcher, preserving their original assertions for line
editing and empty-pair deletion. No GUI, packaging script, `cargo clean` or commit was used.

Review paired typing results, 21 alternating pairs against the **same frozen baseline**
(SHA-256 `4b105c8e6fc69e4105122828ba0261bab4b5dde99b06df96b645de94efab4383`), main → candidate milliseconds:

| Bench | p50 main → candidate | p95 main → candidate | p99 main → candidate |
| --- | --- | --- | --- |
| typing1 | 0.201146 → 0.201416 (+0.13%) | 0.346060 → 0.346509 (+0.13%) | 7.120734 → 7.078790 (-0.59%) |
| typing1000 | 5.260395 → 5.267375 (+0.13%) | 14.595638 → 14.585129 (-0.07%) | 27.107928 → 27.103455 (-0.02%) |

All six metrics pass the 5% gate. Source/binary hashes, raw samples and gate logs are in
`/tmp/velocity-lane-a-r16-review/`; `paired/report.json` contains the binary identity.
Reproduce with the earlier paired command, using the review evidence directory for `--base`
and `--output` (`/tmp/velocity-lane-a-r16-review/paired`).

Review hook timings, milliseconds per command averaged over 20 invocations, 10,000 carets:

| Buffer | No-op insert | Type with 10,000 bookmarks | Plain typing | Recording |
| --- | --- | --- | --- | --- |
| 1 MiB | 0.3093 | 1.3123 | 1.0383 | 1.0770 |
| 32 MiB | 0.2933 | 1.3217 | 1.1055 | 1.1046 |

The restored stable region sort shows no buffer-size-dependent growth in this probe.
The new bookmark-toggle overlap lookup uses sorted interval endpoints rather than scanning
all bookmarks once per caret. Full one-caret/many-caret data is in `hook-timings.log`.

Second-review native checklist (pending; headless regressions use the production key dispatcher):

- [ ] With the document caret between `()` and Find text `abc`, press Backspace: input `ab`,
  document unchanged. Repeat Cmd+Backspace and Ctrl+Shift+K with Find, the command palette,
  and the Save Macro name field focused. Undo edits only the input. With document focus,
  the same keys retain their usual behavior, including empty-pair deletion.
- [ ] Add an unrelated corrupt `.sublime-package` and an unreadable loose file to a disposable
  profile. Cmd+Enter, Ctrl+Shift+K, Cmd+Backspace, empty-pair Backspace and a User macro still
  work. The corrupt package produces a warning; valid User macros remain in the chooser.
  A corrupt Default archive falls back to our built-in macros. Test a loose override of an
  installed macro, then remove/reload it: the archive supplies the next playback.
- [ ] With 100 MB of unrelated loose data, invoke the first macro after launch and after a
  settings/resource reload. Check responsiveness. The command path loads only its requested
  file; the chooser uses the workspace's background filename index.
- [ ] Record typing across tabs, stop, replay, Undo and Redo or Repeat. Edit and undo an input
  field while the document's recorded macro stays intact. No new recorder is allocated for
  each host dispatch (headless allocation probe: 10,000 handoffs allocate zero times).
- [ ] Run `[close, select_all, left_delete, save]` from a clean tab next to a dirty file.
  The origin closes, playback stops with status feedback, and the neighbor stays unchanged
  and unsaved. Repeat through a nested `chain` and a hand-written macro file; also change the
  active view with New File. Test normal dirty-close prompting and close-last-window behavior.

The second-review code keeps the sidebar hook and its
`empty_window_sidebar_omits_input_adapter_before_and_after_preview` regression.
The earlier review's focused-input assertion above was corrected: a key-bound macro file
uses the input's own registry unless document playback is already running.

Second-review validation: fmt, strict locked release clippy (all targets), the full locked
release workspace tests at default parallelism (770 passed, 0 failed, 4 intentionally ignored),
and rustdoc with warnings denied pass. Both the 100 MB macro timing and the existing many-caret
hook timing pass when invoked explicitly. The zero-allocation test is
`vel-commands/tests/registry_dispatch.rs::registry_handoffs_allocate_no_placeholder_state`;
its companion real-key regression covers shared recording, local repeat and input undo.

Same 100 MB fixture before/after this review's fixes (100 loose files of 1,000,000 bytes;
five fresh application instances, real Cmd+Enter dispatch, startup/reload work outside the timer):

| Command | Before median | After median |
| --- | --- | --- |
| First macro key | 15.863 ms | 0.090 ms |
| Warm macro key | 0.034 ms | 0.031 ms |
| First macro after resource reload | 15.346 ms | 0.091 ms |

The first sample after the change took 0.455 ms; all five first-key samples were below 0.5 ms.
The same fixture directory was reused, without changing its contents. No unrelated package
contents are retained by the macro cache; only parsed commands for invoked paths survive.

Second-review paired typing, 21 alternating pairs against the same frozen baseline hash above:

| Bench | p50 main → candidate | p95 main → candidate | p99 main → candidate |
| --- | --- | --- | --- |
| typing1 | 0.217666 → 0.219500 (+0.84%) | 0.367498 → 0.368387 (+0.24%) | 7.065017 → 7.030640 (-0.49%) |
| typing1000 | 5.458208 → 5.465333 (+0.13%) | 15.024710 → 15.220507 (+1.30%) | 27.924256 → 28.129881 (+0.74%) |

All six metrics pass the 5% gate. Candidate SHA-256:
`12bae0fbc43a0f23f1944d73976b3448cdd24ec5a73f8796bbaf5014ed9ed3f5`.
Logs, the reused fixture, before/after samples and `paired/report.json` are retained in
`/tmp/velocity-lane-a-r16-review2/`. Reproduce the timing with
`cargo test -p velocity --release --locked macro_first_key_with_large_loose_package -- --ignored --nocapture`.
The paired command uses the same `--base` as the first review, with
`--candidate /tmp/velocity-lane-a-r16-review2/velocity-candidate` and
`--output /tmp/velocity-lane-a-r16-review2/paired`. No native checks, packaging, clean or commit
was performed.

Earlier checklists follow.

# Round 16 Lane B: folding, guides, Vintage, spelling and menus (native review pending)

Use a reviewer-built binary and a disposable profile. Do not replace or rebuild the running
`target/package-macos/` bundle. All boxes below remain unchecked: the sandbox ran CPU-only
scenes, command/state tests and resource resolution, with no AppKit, fonts, GPU or native input.

- [ ] **5.1:** collapse a block, then enter it using Find, Goto Line, Goto Definition and
  undo/redo. Its hidden selection endpoints become visible, including in a cloned view.
  Left/Right and Shift+Left/Right cross one marker as one unit. Type at either boundary,
  replace a selected marker and Backspace/Delete across it: affected hidden text is exposed.
  Click the text marker and the gutter arrow separately; expanding must retain the selection.
- [ ] **5.2–5.3:** collapse an inner block, then its outer block, with and without wrapping.
  No inner line leaks through the outer fold; expanding the outer retains the inner fold.
  Cmd+K Cmd+1…9 replaces the current fold level; Cmd+K Cmd+0 and Cmd+K Cmd+J expand all.
  Cmd+K Cmd+T collapses tag attributes. Repeat after edits, undo, session restore and splits.
- [ ] **5.4:** Python indentation blocks retain trailing blank lines and the final newline
  when collapsed, including EOF without a newline. `fold_style` values could not be confirmed
  from public documentation; this setting's behavior remains as before this round.
- [ ] **6.1–6.3:** tabs/spaces and blank lines show guides at 0, tab, 2×tab, stopping before
  the text. Enable `draw_active`: only the containing block's guide changes color, without
  lighting another block at the same column. `solid` produces a continuous stroke; otherwise
  guides are stippled. Repeat with wrap, folds, zoom, horizontal/fractional scrolling and
  1×/2× displays. Fresh-profile defaults: selection whitespace, no line highlight, normal
  guides only. Explicit string and array whitespace settings both work.
- [ ] **6.4:** select a whole Unicode word: other whole-word occurrences in the visible
  viewport have outlines, including wrapped rows and horizontally windowed long lines.
  Partial words and carets produce none. Edit, clear selection, switch tabs and disable
  `match_selection`: old outlines disappear immediately. Folded text has no outline leak.
- [ ] **7.1–7.2:** in Vintage insert mode compare Enter/auto-indent, paired Backspace,
  arrows/Shift+arrows, completion navigation/acceptance, snippets and Chinese/Japanese IME
  with ordinary editing. Counts and dot replay the committed changes, including completion
  and pair edits; `u` restores the insertion. Cmd+D, Find and mouse selections feed c/d/y
  as visual selections, preserving direction and exclusive endpoints.
- [ ] **7.3–7.4:** a fresh profile leaves Vintage ignored/off. Remove Vintage from
  `ignored_packages` to enable it; check the `vintage_enabled` alias and live changes.
  macOS enables control chords by default; explicit false restores ordinary Ctrl bindings.
  Ctrl+E/Ctrl+Y scroll one line without moving the caret; Option+W/Option+B move by subword.
- [ ] **8.1–8.3:** prose, quoted source strings and comments receive spelling diagnostics;
  HTML tag/attribute names and values, embedded JS/CSS identifiers, keywords and links do not.
  With spell checking OFF, Ctrl+F6/Ctrl+Shift+F6 navigate and wrap without enabling squiggles
  or changing preferences. Their existing shortcut defaults remain pending native comparison.
  View → Dictionary lists packaged pairs and valid `.dic`/`.aff` pairs in
  `~/Library/Spelling`; selecting either uses that dictionary. Test missing/broken pairs.
- [ ] **9 and 3.2 gutter integration:** inspect Edit/Selection/Find/Goto/Tools and the palette.
  Check captions, arguments, fold levels, marks/bookmarks, case conversion, wrap/permutation
  and macro resources. All rows now resolve; disabling must follow the current context.
  Bookmark symbols appear once per touched line, without obscuring line numbers/fold buttons.
  Check Tools → Macros with package/user macros, background catalog refresh, overrides and
  ignored packages; run an entry and compare it with the command palette/chooser.
- [ ] **Review 1/4/8d — Vintage:** type `one`, Escape, `a`, type `two`, Cmd+Z, Escape, `u`.
  Repeat with Cmd+Shift+Z before Escape and Ctrl+R after `u`; the editor stays usable and
  selections remain valid. Repeat with an Upper Case command during insert, several carets,
  Unicode completion, dot and macro replay. An unhandled Option+character in command mode
  must reach the host without changing a pending count or the recorded macro.
- [ ] **Review 2/5 — fold boundaries:** for Rust braces, HTML tags and Python indentation,
  click right of a folded header and move Up/Down through it. Repeat unwrapped and at
  wrap widths 7, 12 and 80. Type immediately after an indentation marker: the hidden
  endpoint expands and the new text agrees with the caret. Type or press Enter at a visible
  brace/tag closing line, or after a manual whole-line fold: the fold stays collapsed.
  Undo restores the text; it does not restore an automatically expanded fold.
- [ ] **Review 8a/8e — restore/lag:** reveal an outer fold with Find, move away, keep its
  inner fold, then quit/relaunch with cloned tabs using different fold states. Revealed
  folds stay revealed. During rapid typing/newline insertion, Undo/Redo and clone edits,
  blank-line guides and the active block retain their last published geometry until analysis
  replaces it; they do not blink to the current line's own indentation.
- [ ] **Review 6/8b/8c — large/optional data:** scroll a document with 200,000 bookmarks;
  keep gutter/typing responsive. With `~/Library/Spelling` missing, unreadable or a regular
  file, the dictionary chooser still offers packaged dictionaries. Opposing project-level
  `ignored_packages` settings cannot override user/default Preferences or enable Vintage.
- [ ] **Second review 1 — Unicode history:** start with `éa` and a final newline, then
  Escape, `i`, type `x`, Escape, `u`, `l`, `v`, Cmd+Shift+Z, Left. No crash or split scalar.
  Repeat in character/line/block visual modes, with marks and pending motions, host Undo/Redo,
  source transforms, several carets, composed accents and emoji.
- [ ] **Second review 2 — visual boundaries:** Cmd+Left/Right, repeated Cmd+Right and
  Shift+Cmd+Right on brace/tag/indentation headers and wrapped marker segments leave folds
  collapsed. Start both before and after an indentation marker; Cmd+Left/Right must
  neither reveal hidden text nor produce an out-of-range caret. Home/End retain document
  navigation. Repeat without folds and at widths 7/12/80.
- [ ] **Second review 3/5 — insert groups and repeat:** type `two two` one key at a time:
  Cmd+Z removes the last word, exactly as with Vintage off; Cmd+Shift+Z restores it. Escape,
  `u` undoes the whole vi insert, Ctrl+R restores it. Type `one`, Escape, `a`, type `two`,
  Cmd+Z, type `xy`, Escape, `.`: text is `onexyxy`. A session invalidated by an unrelated
  source transform must not repeat an older insertion. After host Undo in a counted append,
  dot repeats only the surviving net text once. History across change/open-line entries
  retires dot when the entry effect cannot be reconstructed. Compare completion, pair edits,
  count/macro replay and IME with ordinary editing.
- [ ] **Second review 6 — long insert:** use 10,000 carets and type at least 100 keys,
  then Escape, Undo, Redo and dot. Recording metadata stays bounded by the caret count;
  host text/history storage may grow with inserted content. Keep typing responsive.
- [ ] **Third review 1 — mixed Visual operators:** Escape, drag `ab`, Cmd-click the
  start of `cd`, then Shift+J in `ab\ncd\nef\ngh\n`. No crash. Repeat delete,
  change, yank, indent, case, replacement and paste with forward/reversed selections,
  empty lines and Unicode carets; Undo/Redo keep valid selections.
- [ ] **Third review 2 — projected Home/End:** repeat Home/End and Cmd+Left/Right,
  with and without Shift, at widths 0/7/12/80. Start before/on/after each marker,
  including the hidden end of an indentation fold and nested folds. Folds stay closed.
- [ ] **Third review 3 — completed insertion:** `A`, type `two three four` one key
  at a time. While inserting, Cmd+Z removes a native word group and redo restores it.
  After Escape, either Cmd+Z or `u` removes the entire session and one redo restores it.
  Repeat with Enter, Backspace, IME commits, `cw`, `o` and `3A`.
- [ ] **Third review 4 — input before frames:** record a vi macro with insertion,
  Escape and `j`/`k` or Ctrl+V plus motion; replay it repeatedly. Also type several
  edits and immediately Cmd+Left/Right while changing wrap settings. Check selections,
  line geometry and responsiveness before/after the next frame.
- [ ] **Native performance:** repeat actual scroll/typing/idle profiling with folds and a
  selected word, at the reviewer's display size/refresh/scale. Record event→submit/present,
  process CPU and memory; headless paired results establish only the CPU scene/command budget.
  The review report records paired CLI times/instructions, CPU/RSS, steady idle CPU and
  the matching full-size scene probe, with raw pairs retained for scheduling-noise review.

# Round 15: opening, saving, projects and sessions (native review pending)

Use a reviewer-built binary/bundle with a disposable profile and fixture tree. Do not rebuild or
replace the currently running `target/package-macos` bundle. No GUI, Finder, dialogs or menu
tracking were run in the sandbox; leave these boxes unchecked until native review.

- [ ] **1.1–1.3 opening:** CLI-open a missing filename and one under a missing parent. Both
  produce named new tabs; Save creates only the former and reports the latter's path. Mix
  unreadable/special paths with valid files in CLI/Finder batches: later files open, errors
  list all failed paths, the CLI returns its ack. Open a directory normally (new window),
  then with `-a` / `--add` (current window). Repeat with several windows and no live window.
- [ ] **1.4 stdin:** run `printf 'original\n' | velocity --data-dir PROFILE - > /tmp/vel-r15-out`,
  edit to include Unicode, close the tab and compare stdout. Repeat after Save As and by
  closing the whole window / quitting; the caller waits and receives the final edited text.
- [ ] **1.5 symlinks:** project root contains a linked directory with a link back to an ancestor.
  With follow_symlinks omitted, expand once and use Goto Anything / Find in Files; linked
  content appears, cycles do not expand forever. Set explicit false and repeat. Save the
  project: the absent key stays absent and false stays false.
- [ ] **2.1 Save All:** dirty named/untitled tabs, clean tabs and a read-only active hex tab.
  Save All remains enabled; only dirty documents save. Cancel an untitled dialog and fail
  another save, then confirm later dirty files still save and skipped buffers remain dirty.
- [ ] **2.2–2.4 deleted files and names:** delete a loaded file externally; its title shows
  deleted and closing prompts. Save recreates it. Restore a session with missing files;
  other tabs restore. With close_deleted_files true, clean missing tabs close but dirty
  contents remain. Close prompt buttons are Save / Don't Save / Cancel; its filename has
  no dirty dot. Untitled names and Save suggestions use the first line, capped at 50 scalars.
- [ ] **2.5–2.6 empty/reopen/recent:** close the last tab: empty tab bar, no new Untitled tab.
  Type, Cmd+N and double-click empty tab-bar space to create on demand. Close a saved file
  with a directed selection and scrolled viewport; Cmd+Shift+T / File → Reopen Closed File
  restores both. File → Open Recent lists files/folders/projects; Clear Items persists across
  relaunch. A recent folder opens a new window. Test independent reopen stacks in two windows.
- [ ] **2.7–2.8 policy/errors:** set hot_exit to boolean false, make dirty text and close/quit:
  normal Save/Don't Save/Cancel behavior. Unreadable small and background-loaded files must
  report the actual path once, without repeated errors on tab access.
- [ ] **3.1–3.2 projects:** switch a dirty untitled window into a project using the menu and
  running-instance --project. With Hot Exit, quit/relaunch and recover the departed untitled
  window as well as the project. With Hot Exit disabled, verify prompts (menu) or a separate
  window (CLI). Change an opened project's file externally: watching still reloads it.
- [ ] **3.3–3.5 folder rules:** Save Project As into a sibling directory, then reopen it;
  original relative roots still resolve and unknown top-level/folder keys remain. Include
  patterns apply before excludes. Default filters hide VCS folders, .DS_Store, compiled
  objects and workspace files, while target remains visible. A .gitignore-only file appears
  in Goto Anything and Find in Files; the side bar retains its existing ignore behavior.
- [ ] **4.1–4.2 settings/encoding:** load detected indentation, manually choose width/style,
  reload resources and externally reload the file: manual choices remain. Exercise File →
  Reopen with Encoding and Save with Encoding (Windows 1251, UTF-16 LE/BE with BOM, UTF-8
  with/without BOM), default_encoding and fallback_encoding display names. Invalid fallback
  reports once and uses Windows-1252; invalid data after a BOM falls back. Reopen cancellation
  or decode failure must preserve unsaved text. Retest syntax-over-project settings precedence.
- [ ] **Review (a) and (c):** interleave tabs from two groups, close an earlier tab (focus stays),
  close active middle/rightmost tabs (choose right then left in the same group). Trash a
  project root from the side bar: only successfully trashed roots detach, including from the
  saved project; other roots/unknown keys/relative paths survive. Test failed trash/save too.
- [ ] **Review (d), LaunchServices:** while a primary runs, force a secondary `open -na` with
  no document event: no ordinary secondary window/session is left behind and it exits after
  the quiet interval. Repeat with the primary blocked in a native save dialog and several
  Finder/Dock batches during relay: UI does not wait on the socket, requests retain order,
  no spurious timeout failure/duplicate local opens, and the receiver exits after the last ack
  plus a quiet interval. Quit the primary during relay: takeover restores all dirty windows,
  installs the crash handler and opens all retained paths. These timing/routing cases need
  native evidence; deterministic headless tests cover the relay state/protocol only.
- [ ] **Review 1 / 10 / 11, empty preview:** close the last tab, single-click a side-bar
  file and preview several Goto Anything results, including replacement of the only
  transient tab. Escape leaves an empty tab bar and Open Files has no ghost Untitled.
  Typing creates one visible tab. Save As / Save and Close are disabled while empty.
  Quit with dirty content in another window and relaunch: all retained content survives.
  Preview cancellation does not add to Reopen Closed File. Close two real files, delete
  the latest on disk, then Cmd+Shift+T: skip it and restore the older file's view state.
  Compare the deleted-file reopen behavior with Sublime on the reviewer's native machine.
- [ ] **Review 2 / 3 / 4, filesystem changes:** expand a folder containing dangling/self/
  ancestor symlink loops and unreadable children; healthy siblings remain visible. Run
  Replace in Files twice with empty user excludes: our recovery backups remain hidden
  from side bar, Goto Anything and Find in Files and keep their original contents.
  Create/delete/rename files and subfolders while the index is running: native watcher
  events refresh Goto Anything (the headless index thread test disables native watching
  to exercise unavailable-watcher polling; FSEvents delivers no events in the sandbox).
  Delete/recreate a clean tab's file with identical then different content (also cloned
  tabs): deleted markers clear, clean tabs reload, real local edits retain a conflict.
- [ ] **Review 5 / 6 / 8 / 9, session details:** repeatedly switch a clean saved-file window
  between projects / Close Project, quit and relaunch: no extra departed windows return.
  Repeat with dirty/untitled contents: those windows recover. Make a dirty session file
  unreadable or invalid on disk, restore and activate it: retained unsaved text is shown
  and survives another quit/relaunch. Hidden restored saved tabs do not show `[new]`;
  activate a lazily restored file with two-space indentation after appearance setup:
  detection runs, and later appearance/resource reloads keep manual indentation choices.
- [ ] **Review 12 / 13 / 14, paths and routing:** Save Project As across a symlinked parent
  (including `/tmp` and `/private/tmp`) and reopen: each relative root resolves to the
  same directory. Folder-name includes exclude unmatched ancestors as documented; an
  included descendant does not bypass its excluded parent. Launch with `--project X`
  or X's directory when a recovered window already contains it: reuse that window once;
  `--new-window` still creates another. Send a late Finder event just before relay quiet
  expiry: wait for the last request/ack plus the quiet interval. Force a relay failure:
  its error names all paths in the failed batch.
- [ ] **4.3 contexts (Lane A):** review new editor key-binding contexts with Lane A's changes;
  Lane B did not edit editor/view context providers.

# Round 15: find panels, overlays, status and editing details

Native checks are **pending human review**. Run an ordinary release build on an unlocked macOS
desktop with a disposable profile and project; no GUI, Metal or packaging script was run in the
sandbox. Keep the round-14 Finder/Dock/window-open and native context-menu checklist below.
Record OS, input source, physical display scale, `ui_scale`, and observed results.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --release --locked -- -D warnings
cargo test --workspace --locked --release
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo build --release --locked -p velocity
./target/release/velocity --data-dir /tmp/velocity-r15-review /path/to/disposable/project
```

- [ ] **1.1 / 1.9 Replace and buttons:** Enter / Shift+Enter in both Replace fields find
  forward/backward without editing. Replace uses Cmd+Alt+E or its button; Replace All uses its
  button / Ctrl+Alt+Enter. Find shows Find / Find Prev / Find All, Replace shows Find / Replace /
  Replace All, and Find in Files shows Find / Replace. Alt+Enter in Files never selects buffer
  matches. Confirm the reviewer's focused-panel key binding contexts still apply.
- [ ] **1.2 / 1.3 Incremental and typing:** start partway down a long document, Cmd+I, type a
  query with an offscreen next match. The first match at/after the original caret is selected and
  revealed, with wrapping. Enter advances and closes. Reopen, type, then Escape: original
  selections (including the primary region) and horizontal/vertical scroll return. Repeat with
  `hide_panel {"cancel":true}`, and Escape before the background search completes. Ordinary Find
  also selects/reveals while typing. After three Find Next presses, Left/Right, Cmd+A and Cmd+C
  in the field must keep the current document match. With In Selection enabled, typing and
  option changes must retain every original selection and reopening must retain the full scope.
  Seed Find with a forward/reversed selection: preview starts at its beginning. Test
  `find_selected_text` and `auto_find_in_selection`.
  Incremental Enter intentionally follows the [public search guide](https://docs.sublimetext.io/guide/usage/search-and-replace.html#incremental-search):
  advance to the next match and dismiss the panel; compare on the reviewer's current Sublime build.
- [ ] **1.4–1.6 Focus and history:** reopening Find selects its query; reopening Replace also
  selects its replacement. Click the editor with Find visible, Cmd+F focuses/selects, then Cmd+F
  closes. Check every find panel's close button and Escape from editor/panel focus. Up/Down in
  Find and Replace walk committed history and return to the draft. Pause during a half-typed
  query, quit/restart and verify checkpoints did not add prefixes to history.
- [ ] **1.7 / 1.8 Formats and feedback:** regex `(cat)([0-9]+)` with `\2:\u\1`, `$&`,
  `$+{name}` for a named capture, `\n`, `\t`, `\\`, `\U…\E` / `\L…\E` and the existing
  `$1`, `${1}`, `${name}`, `$$` behave as expected; Undo restores one replacement transaction.
  A missing query shows `Unable to find …`; replacement counts survive worker completion and
  expire after about five seconds. Invalid regex appears inline/in the status bar using the
  user's expression, with no internal flag wrapper or modal alert. Let the message expire, then
  retry Enter, Find Next/Prev, Find All and Replace without changing the invalid query: the error
  must reappear. Check long feedback stops before all three Find/Replace buttons at every scale.
  Also try `$1px` / `$1_old` (numeric capture followed by literal suffix). Repeat in Files.
- [ ] **2.1–2.3 Files:** try `<open folders>`, `<open files>`, `<current file>`, `<project>`,
  `*.rs`, `src/`, `//src/`, `-node_modules` and `-node_modules/`, including nested directories,
  paths containing spaces and unsaved active/open files. See `Searching N files…` followed by
  actual match/file counts in the header and status. Escape / hide the Results panel cancels
  promptly. While searching, open Find, a Vintage prompt, or build output: the search keeps
  running in the background, including when the other panel closes. Starting another search
  replaces the prior search. F4 / Shift+F4 skip context/header rows and jump search matches after
  the panel closes. A build switches those keys to diagnostics without dropping a running search;
  run a new search to switch back. In a root `/.../src/app`, `src/*.rs` must match only its own
  `src/` subtree, `-src/*.rs` must retain root-level files, and `-me/*` must not match a parent
  directory. A hidden root such as `~/.dotfiles` with `-.*` must still search visible descendants.
  Repeat relative patterns under an explicit nested root; `//` remains anchored at the project.
- [ ] **3.1–3.5 Overlays:** Cmd+R lists symbols and keeps the selected symbol through repeated
  frames and a source refresh. Filter quickly and press Enter before ranking finishes: the best
  current file/command candidate opens/runs. Changing a query returns selection to the best row;
  only refreshes of the same query preserve a manually selected row. Rank Cmd+P files, Escape,
  then Cmd+R in plain text or while indexing and immediately Enter: no stale file opens and no
  stale symbol offset is used. Switch a file query directly to `#word`, `@`, and `:line`, too.
  Single-click a result to accept; click outside to cancel and restore the preview. Close Split is absent with one group, available with two, and
  cannot run after becoming disabled; a rejected acceptance leaves the palette open. Idle palette
  frames reuse the enablement captured on open, and acceptance rechecks the chosen command.
  Footers describe each overlay and contain no find messages;
  split hints appear only on Goto. Repeat with Pinyin/Japanese composition in overlay inputs.
- [ ] **3.6 / 4.1 Scale and status:** at `ui_scale` 0.75, 1, 1.5 and 2 the overlay is centered;
  right-click sidebar rows and verify the path under the pointer. A single empty caret has no
  selection count; one/multiple regions show correct Unicode character counts and plurals.
  Encoding/EOL appear only with `show_encoding` / `show_line_endings`. Click Spaces / Tab Size:
  change tabs/spaces and width, convert mixed indentation, then Undo once.
- [ ] **4.2 Side bar / round-14 regressions:** Ctrl+0 leaves Find visible; Escape returns to the
  view without hiding Find. Retest Create File/Folder, Rename (including case-only/no-op), Trash,
  Search This Folder, Reveal in Finder and Copy Path. Rename retains deleted/external-change
  state; relative roots survive reload; symlinked sidebar paths target their canonical open tab.
  Keep the round-14 tab-close/active-tab/context-argument and Finder-open checks green.
- [ ] **5.1–5.3 Editor:** on wrapped Unicode lines, Home/End / Cmd+Left/Right reach visual
  boundaries; End on a non-final row remains just before its wrap point, on the same drawn row.
  Follow it with Home, Up and Down and verify the originating visual row; compare exact End
  placement with Sublime on the native desktop. Repeated BOL reaches indentation then column
  zero. Hard BOL/EOL remain logical. Repeat with Shift, grapheme clusters and multiple carets.
  Home on a huge single-line file must remain responsive. Find All to create 10,000 carets in a
  large viewport-highlighted buffer; arrows and quote keys must remain responsive with edit
  history retained. Type quotes rapidly after edits in code, strings and comments while
  highlighting catches up: published line scopes continue to control pairing.
  Cmd+D, Quick Find All, Cmd+E and Find Under retain caret-word and primary-selection behavior.

Earlier checklists follow.

# Round 11: Vintage and user grammar review

Run outside the sandbox on an awake, unlocked display:

```sh
cd /Users/zoro/dev/velocity-lane-a
cargo build --release -p velocity
zsh crates/velocity/scripts/review-round11-gui.zsh
```

The script freezes the binary, records its hash, creates an isolated profile with both original
TextMate fixtures and enables Vintage in command mode. It prints the profile/grammar/log paths.
`--prepare-only` creates those fixtures without starting a native application.

- [ ] See COMMAND in the status bar and a readable block caret. Try `2d3w`, `u`, `gg`, `G`,
  `f`/`;`, `ciw`, `v`, `V`, Ctrl+V, `>`, registers, `q`/`@` and `.`. Pending prefixes/counts
  and visual line/block selections should update on screen. Confirm multiple carets still work.
- [ ] Press `i`, activate Pinyin/Japanese, compose and choose candidates. Preedit and candidate
  geometry should match normal editing; Escape after committing returns to command mode.
  In command mode, `h/j/k/l` should act immediately with the input method still selected.
- [ ] Use `/one`, `?one`, `n` and `N`; Escape cancels a prompt without editing. Try `d/three`.
  Use the input panel's arrows, clipboard and IME. Open Find or another tab during a prompt.
- [ ] In `editor.rs`, reindent the second line with `==`; `u` restores indentation in one step.
  Verify clipboard `yw`/`p` using another application. Check `:w`, `:wq`, `:q`, `:q!`,
  `:e`, `:e!` and `:w <temporary path>` with dirty/save-cancel/failure paths.
- [ ] Ctrl+Alt+V disables/re-enables Vintage for the active view. Edit User preferences while
  open to test all four settings. Turning it off closes a vi prompt and restores normal typing.
  Reopen using a fresh profile to confirm Vintage is off by default.
- [ ] `notes.vnote` detects Velocity Notes; `xml-notes.vxnote` detects Velocity XML Notes.
  Check TODO/string/comment colors, syntax picker and palette entries. Edit the printed JSON
  grammar's name/rules, add another nested grammar, then remove it: open views and choices
  should reload while retaining unsaved edits. Try a malformed definition and read-only cache.
- [ ] Test split/clone views, Retina scaling, status clipping and Metal validation. Record the
  display, input source and input JSONL path with observed results. Native rows remain unverified
  until a human runs them; the headless tests initialize no window server or GPU.

Earlier checklists follow.

# Native AppKit NSTextInputClient manual checklist (P1)

Round-nine tester status: **Chinese IME confirmed working by the human tester**. Hot Exit is
currently under human test. The individual legacy rows below retain their original evidence
cells; Japanese/reconversion/display-specific cases are not marked passed by that confirmation.

Adapted from the repository’s MIT OR Apache-2.0 P0 checklist. Hardware results are pending.

Run outside the sandbox:

```sh
cd /Users/zoro/dev/velocity
cargo build --release -p velocity
./target/release/velocity --log-input crates/velocity/fixtures/ime-and-unicode.rs
```

Record machine, macOS version, active input source, keyboard layout, display/scale/refresh and the
JSONL filename. Test each relevant row with Simplified Chinese Pinyin, then the Japanese input
source where indicated. Position the caret by clicking or arrows; select with Shift+arrows or drag.
Leave the observed-result column empty until tested.

| Case and steps | Expected result | Observed result / pass/fail / log reference |
| --- | --- | --- |
| Pinyin: click after `→`, type `nihao` | Inline marked text with underline and preedit caret; document contains transient preedit (Undo/cancel restores the original); candidate window adjacent to caret | |
| Choose `你好` with Space or candidate number | Exactly one `你好` inserted, marked text cleared; confirmation Enter/Space does not insert an extra newline/space | |
| Continue typing English after commit | New characters insert once at the new caret; no duplicated IME commit | |
| Shift toggles English while composition is active | Native IME's selected commit/cancel behavior is reflected once; no stale preedit; subsequent English input works | |
| Backspace inside `nihao` preedit | Marked text updates through IME; previously committed document text is not deleted | |
| Esc during preedit | Native composition cancels, underline/candidate UI clears; document content remains as before composition | |
| Move within a preedit using native IME keys | Preedit cursor and selected conversion range update in UTF-16 code units, never splitting a surrogate | |
| Scroll a few pixels with an active visible composition | Marked text stays inline and candidate anchor follows the visible caret, with no 2× Retina offset | |
| Scroll composition offscreen and back | Candidate anchor stays inside the client area while offscreen; real caret position is restored on return | |
| Resize with an active composition | Underline, text, cursor and candidate anchor follow current geometry; no stretched/stale atlas text | |
| Move window between 1× and 2× displays | Glyphs rerasterize; caret/candidate placement keeps logical-point alignment | |
| Japanese: enter Kana, Space to convert, move conversion selection, then confirm | Kana/marked range is visible; candidate conversion commits once; caret ends after committed text | |
| Ctrl+Cmd+Space, choose an emoji in the native picker | Emoji inserts once and advances appropriately; record a missing commit separately from a missing image | |
| Option+E then E using an English layout | `é` inserted once, dead-key preedit cleared, no extra apostrophe or `e` | |
| Pinyin full-width punctuation: `，。！？（）` | Correct full-width glyphs and two-cell spacing; no duplicate ASCII punctuation | |
| Select text, then start Pinyin and confirm | Selection remains well-defined; commit replaces selected text exactly once; subsequent deletion is safe | |
| Switch focus away and back during preedit | No stale marked text; IME can be enabled and used again after focus returns | |
| Compare fixture's `你好`, Kana and punctuation | Correct fallback glyphs; no tofu, inappropriate Latin glyphs, clipping or baseline jumps | |
| Compare `😀 🦀 🚀 👩‍💻 ❤️ 🇨🇳` | Simple color emoji visible; inspect ZWJ, variation selectors and flags for correct single-cluster display | |
| Compare NFC `é` and decomposed `é`; Backspace each | Legible placement; one Backspace deletes one extended grapheme | |
| Shift+arrows and mouse drag across Chinese/emoji and newlines | Selection rectangles follow cell widths and line boundaries; Backspace/typing replaces selection | |
| PageUp/PageDown, horizontal and fractional trackpad scrolling | Smooth point-based movement, gutter fixed; caret is revealed by subsequent arrow/edit input | |
| Type quickly with `--log-input`, then close | Every delivered keyboard and IME event has a timestamp; changes have receipt→submit/present-return records | |

Native logging records key events, standard command selectors, Insert/Marked/Unmark,
explicit replacement ranges, selected/marked document UTF-16 ranges and latency to
queue submission/present-return. A commit without prior marked text must work. The
buffer contains inline transient preedit; cancellation restores its origin snapshot,
and the whole composition is one Undo step. `unmarkText` keeps the current text once.

Additional native range/geometry cases (all pending):

| Case and steps | Expected result | Observed result / pass/fail / log reference |
| --- | --- | --- |
| Japanese: select committed 日本語, invoke input source's reconversion, change and confirm | Explicit UTF-16 replacement range replaces exactly that text; one Undo restores it | |
| Place caret after a non-BMP emoji, invoke reconversion of adjacent CJK | Offsets count emoji as two UTF-16 units; no adjacent scalar is damaged | |
| English: hold E and choose an accent from the press-and-hold menu | AppKit replacementRange changes the original E once; selected/marked ranges remain valid | |
| Emoji picker with no composition and a nonempty selection | Exactly one emoji replaces the selection, with correct color and candidate position | |
| Start composition over a selected word, cancel, Undo/Redo, then paste | Original selection/text restore on cancel; subsequent history/paste is correct | |
| Reconversion requests attributedSubstring at document edges / emoji | Actual range is clipped to valid scalar boundaries; no crash or half-surrogate text | |
| Drag from the text to a window on a display left/above the primary display | Candidate rect remains in screen points with correct sign; no Retina multiplication | |
| Hide/occlude/unocclude during composition and move between 60/120 Hz displays | No idle redraw loop; candidate anchor and display rate refresh correctly | |
| About/Hide/Quit, Undo/Redo/Cut/Copy/Paste/Select All from menus and shortcuts | Native actions reach editor commands; clipboard text round-trips Unicode | |
| Toggle system light/dark, resize continuously, enter/leave fullscreen | Fresh geometry and colors, no stretched text; native fullscreen works with tabbing disabled | |

Check environment.fonts and final_render_state.fonts for resolved faces, warnings and
CoreText raster counts. Some OS font formats require the retained native face's bitmap
fallback. Check CJK, combining marks, emoji colors/alpha and ZWJ/flags visually.
The layout is a unidirectional grid, not paragraph bidi or automatic wrapping.

Candidate queries implement both firstRectForCharacterRange:actualRange: and
characterIndexForPoint:. View→window→screen conversion uses logical points, including
flipped-view conversion. Record requested/actual UTF-16 ranges and resulting screen
placement when any reconversion or picker case fails. No native input or visual pass
is claimed from headless tests alone.

## Round 4 editing smoke test

Use disposable copies of files. Record results here; these native cases were not run in the sandbox.

- [ ] Open two files with Cmd+O and via `velocity first.txt second.txt:2:3`; check the one-based Unicode position and tab labels.
- [ ] Type, select, cut/copy/paste, Enter with indentation, Tab/Shift+Tab; save with Cmd+S, Save As with Cmd+Shift+S, and reopen to compare bytes.
- [ ] Add carets with Cmd+click, Cmd+D and Ctrl+Alt+Up/Down; type/delete/paste at every caret. Split a multiline selection with Cmd+Shift+L; all carets and selections must render.
- [ ] Cmd+Z/Cmd+Shift+Z restores the full multi-caret transaction; grouped typing undoes by the buffer's rules. Cmd+U/Cmd+Shift+U restores soft selection history. Escape leaves one selection.
- [ ] Switch with Ctrl+Tab/Ctrl+Shift+Tab and Cmd+1…9; each tab retains its text, history and scroll. Closing a dirty tab/window offers Save/Cancel/Discard; cancelling leaves edits intact.
- [ ] Open/save UTF-8 with/without BOM, UTF-16LE/BE with BOM, Windows-1252, LF/CRLF/CR and mixed endings. Confirm displayed encoding and majority line endings, BOM retention and permissions. Unrepresentable saves must report an error and preserve the original file.
- [ ] Set `trim_trailing_white_space_on_save` to true then `"not_on_caret"`; test all caret lines, `trim_only_modified_white_space`, inserted lines and final-newline cleanup. Undo cleanup as one transaction.
- [ ] Change an open file externally: clean views reload, dirty views ask Keep Edits/Reload. Repeat with an atomic rename and after a previous save.
- [ ] Edit User preferences/keymap with comments/trailing commas; change tab size/font/gutter and override a shortcut. Verify the new binding wins and project/view settings keep their precedence.
- [ ] Enter `(`, `[`, `{`, quotes, and matching closing characters; confirm pairing/skip contexts. Repeat on the actual keyboard layout, including shifted shortcuts, keypad keys and an optional modifier-tap user binding.
- [ ] Repeat the IME rows above with multiple carets: marked text is a transient primary preview; commit inserts once per caret and one Undo removes the committed conversion. Explicit native replacement ranges replace only their specified region.
- [ ] Open the 100 MB fixture: a loading tab appears promptly, UI stays responsive, progress advances, and edits cannot overwrite the file before loading succeeds.
- [ ] While testing startup outside the sandbox, verify asleep/locked/occluded runs produce explicit errors in about two seconds, then unlock/uncover and rerun successfully. Inspect skipped_drawables, first_callback_main_ms and first_real_scanout separately.


## Round 5: widgets, navigation, search and side bar

Run the reviewer fixture command from RUNBOOK.md outside the sandbox. These are human acceptance
checks; use the automatic `--bench-panels`/search example/matrix for measurements. Leave results
blank until observed. Keep the entire IME section above, and repeat its core cases in both the
Goto input and Find/Replace/Where inputs (including Undo, horizontal scrolling and Retina moves).

| Case and steps | Expected result | Observed result / pass/fail / log reference |
| --- | --- | --- |
| Cmd+P; type `ime`, Up/Down/PageDown; Esc | Fuzzy glyph highlights, kind/path details and a bounded list; transient preview disappears; original tab, selection and viewport return | |
| Cmd+P `ime-and-unicode.rs:8:3`; Enter | Target uses one-based scalar coordinates and becomes a pinned tab | |
| Cmd+P `:8`; Esc, then repeat and Enter | Preview moves within current file; Esc restores selection; Enter retains it | |
| Cmd+P `#input`; select a word; Enter | Fuzzy word rows preview the correct UTF-8 position | |
| Cmd+R / `file@symbol` | Current-file syntax symbols appear with transformed names; selection previews, Escape restores, Enter commits | |
| Cmd/Ctrl+Enter on a Goto result | Two visible panes; each scrolls independently; clicking switches active target; Close Split palette action restores one pane | |
| Cmd+Shift+P `save`; inspect hint; run safe commands repeatedly | Captions, binding hints and per-invocation recent ordering; arrows and Enter work; Escape preserves document | |
| Pinyin/Japanese/emoji in Goto and find inputs | Composition stays inside the focused input; candidate rectangle follows wide graphemes; confirmation never accepts a panel prematurely; Undo changes only that input | |
| Cmd+F; toggle regex/case/word/wrap/selection; Cmd+G / Shift+G | Correct highlights and forward/backward wrap; invalid Perl lookaround/backreference shows an error without altering text | |
| Cmd+I and type incrementally; Escape | Viewport updates during typing, complete matches arrive asynchronously, no stale results after an edit/tab switch | |
| Select a region, Cmd+Alt+F; preserve case; Replace All; Undo | Only selected matches change; cat/CAT/Cat map to replacement casing; one undo restores all; repeated shrinking replacements remain in the tracked selection | |
| Regex `(cat)([0-9]+)` -> `${2}:$1`; Replace All | Captures expand correctly; zero-width regex progresses without hanging | |
| Cmd+E / Cmd+Shift+E; Alt+Enter | Find/replacement seeded from selection/word; Find All creates a multi-selection | |
| Cmd+Shift+F; Where `<project>, *.rs, -ignored*`; Enter | Streaming Find Results has file headers, numbered context, highlights and summary; Up/Down/Enter or double-click jumps | |
| Edit without saving; Files Where `<open files>` | Search sees the unsaved buffer text; IO/decoding failures are reported | |
| Fill Replace, run Files search, invoke Replace in Files; Cancel, then confirm on disposable files | Cancel writes nothing; confirmation reports counts; originals kept beside each file; changed/dirty targets rejected; backup bytes restore originals | |
| Expand fixture folders, create/rename/delete a file externally; adjust ignore rules | Lazy tree and Goto index update; .gitignore and file/folder/binary exclusions agree with their documented scopes | |
| Cmd+K, Cmd+B; Ctrl+0; navigate Open Files and folders | Visibility toggles, keyboard focus stays in tree, single-click previews and double-click/Enter pins | |
| Project: Add Folder…; choose a second directory | New root appears and indexes; canceling native picker leaves roots intact | |
| Open many files; wheel over tabs; close, dirty an input, drag tabs | Scrolling, close hit targets, dirty dots, reorder and active document identity remain correct | |
| Resize panels/window; hover/press controls; scroll output/results/list | Rows remain clipped/virtualized; caret/selection and active surface route correctly; status stays readable | |
| Click document while Find remains visible, then resume panel input | Document edits and panel edits target their own buffers; overlay always captures above both | |

Native geometry, font quality, appearance, Metal validation, refresh-rate behavior and GUI timing
remain pending reviewer evidence. Attach the run directories, completed table and observed hardware
configuration; do not replace missing scanout values with CPU timings.


## Round 6: highlighting, language commands and view features

All native observations and timing cells below remain pending reviewer runs. Use the round-6
RUNBOOK commands on an awake, unlocked display; attach actual physical dimensions and JSONL.

| Action | Expected | Observed / run directory |
| --- | --- | --- |
| Open Rust, Python, shell/shebang, compound-extension and hidden-extension files | Detected syntax appears in the status bar; first real highlighted screen is correlated to positive presentedTime | |
| Click status syntax; use Set Syntax in Cmd+Shift+P; choose another language | Filtered picker works with keyboard/IME; syntax-specific settings apply without text/history loss | |
| Set color_scheme auto; change macOS appearance, then set an explicit scheme | Original dark/light palette switches live; explicit selection stays fixed | |
| Override a scheme variable/global in Packages/User; edit an already highlighted file | Token colors and background/caret/selection/gutter/find/guides/rulers/minimap update without stale palette spans | |
| View bold/italic, solid/stippled/wavy underline; enable no_bold/no_italic | Real face variants and decorations appear; CJK/emoji fallback and baseline/grid remain correct | |
| Type quotes/brackets in code, strings and comments; repeat while scopes update | Pairing uses current scopes; closing-quote skip remains usable; pending selector contexts do not pair on unknown scopes | |
| Cmd+/ on multiple carets, overlapping selected lines, and reversed selections; Cmd+Alt+/ | Line/block delimiters follow syntax metadata; toggling and undo restore original text | |
| Enter around Rust braces, Python blocks and temporary continuation rules; Reindent | Correct indentation/tab stops; asynchronous reindent never overwrites a later edit | |
| Cmd+R, filter and move through symbols; Escape, Enter; repeat after editing | Metadata names/transforms and positions update; preview/cancel/commit preserve source selection | |
| Move carets around nested brackets and HTML/XML tags; toggle individual matching settings | String/comment brackets are ignored; tag matching can stay enabled with bracket matching disabled | |
| Toggle wrap; resize; set wrap_width/style and continuation indentation | Graphemes remain intact; Up/Down/Page, mouse hit tests, selections and reveal follow visual rows | |
| Compose Pinyin/Kana/emoji across a wrap boundary; commit, cancel, undo | Candidate rectangle covers the first visual fragment; committed text and layout use the same snapshot | |
| Toggle whitespace selection/all/none/leading/trailing, guides, rulers, relative numbers and gutter | Only requested decorations appear; scheme globals and clipping agree | |
| Set smooth/phase/blink/solid caret, extra width and line padding; move between editor and panels/windows | Animation resets on input; inactive selections and focus render correctly; no idle continuous redraw for solid caret | |
| Fold via gutter, selection, indentation/brackets/block scopes, level and tag attributes | Ellipsis markers and suffix text remain visible; hidden lines are skipped by navigation and highlighting requests | |
| Edit/undo before and inside folds; unfold, unfold all; toggle fold-button fading | Named fold regions track edits; viewport and candidate geometry stay valid | |
| Show/hide minimap, click, drag beyond its bounds and release; resize or edit/newline | Token colors match the document; viewport scrolls without changing selections; indicator and border obey settings | |
| Run automatic open/scroll/typing with default and 4K physical sizes, then Metal validation | Positive highlighted scanout, full CPU timing, isolated minimap GPU timing and clean validation; no external input in automatic runs | |


## Round 7 — workbench/session/build acceptance (native review pending)

Run the isolated-profile commands at the top of RUNBOOK.md. Fill the last column with evidence,
actual scale/size/refresh and any failure; these rows have not been executed in the sandbox.

| Action | Expected behavior | Reviewer observation |
| --- | --- | --- |
| Launch with a non-existent data directory | Empty editor opens, User tree may be created, no missing-Packages exception | |
| Launch with corrupt session / read-only profile / a file occupying the data path | Empty usable editor; corrupt original preserved; IO failures reported without data loss | |
| All seven layout presets, normalized custom cells, splitters and New/Close Pane | Cells cover content, divider hits respect spanning cells, tabs stay with their groups | |
| Cmd-click / Shift-click / Tab Selection commands | Selected sheets appear horizontally inside one group with connector styling | |
| Sidebar Cmd/Shift multi-select; Goto Cmd+Enter | Additive sheets stay visible and previews pin on acceptance | |
| Move/close multiple selected tabs | Order/selection preserved; the last dirty shared views ask once when policy requires | |
| Clone File, separate carets/scroll/folds, type/undo/redo, close current owner | One document/history, independent view state, no stale text or detached-view panic | |
| IME in lower/right group, scroll inactive selected sheet, return focus | Candidate anchor, hit testing, selection and view-local scroll use correct offsets | |
| Cmd+Shift+N; close one window; CLI -n/-a/-w and stdin | One application, independent windows; wait ends when requested sheets close | |
| Full screen; distraction-free from normal and already-full-screen; customize its settings | Chrome/settings restore correctly and sidebar visibility is remembered per mode | |
| Quit/relaunch dirty/untitled text, redo, folds, syntax, sidebar and search state | Retained state restored, hidden clean tabs lazy; round-nine native history removes the former private-history limitation | |
| Hot Exit always / only_on_quit / disabled; remember_open_files false | Close/quit/crash policies follow RUNBOOK; disabled never silently discards dirty project docs | |
| Edit a dirty document on disk while quit, then restore | Buffer text retained as dirty, restored undo history dropped | |
| Open/Save As/Close/Edit/Quick Switch Project; nested fresh destination | Folder rules, inline builds and unknown keys preserved; workspace tied to project | |
| Leave a project idle after editing | Debounced session writes complete without an autosave/settings-reload loop | |
| Cmd+B with Workbench Example; F4/Shift+F4; output double-click | Both streams appear, exit/time shown, correct diagnostic location and inline badge | |
| Build With variants, Cancel Process Tree, quiet, syntax and wrapping | Variant overrides apply, tree terminates, UI stays responsive, output follows settings | |
| Native automatic startup/typing/scroll with four groups | Positive scanout/complete JSONL; budgets from RESULTS/RUNBOOK assessed with no missing timestamps | |


## Round 8 — intelligence and Hot Exit acceptance (native review pending)

Run the exact commands in RUNBOOK.md. Do not run GUI/Metal in the sandbox.

- [ ] Cmd+Shift+R: type `render`, inspect kind badges and file:line, arrow through previews,
  Esc restores original selection/scroll and Enter pins the target. Repeat with unsaved names.
- [ ] F12/Cmd+Alt+Down on `unique_target` jumps to another already-open file. `shared_name`
  opens multiple definition rows. Shift+F12 lists references when syntax metadata publishes
  indexed references. Missing metadata yields no reference locations. Ctrl+- and Ctrl+Shift+-
  traverse cross-file jumps and remain bounded after 130 jumps.
- [ ] Hover over a symbol, click a location, then disable `show_definitions` and confirm it
  stays hidden. Open Help → Indexing Status, watch percentage/logs, close only that window,
  and verify editor focus, folders and session state are intact.
- [ ] Ctrl+Space/Auto Complete: arrows, cycling, Enter and mouse commit. At the bottom of a
  view the popup flips upward. Check annotations/kinds/details and a second window/group.
  Type rapidly, switch tabs and move the caret while queries are pending; stale results never
  replace text. Set `auto_complete_commit_on_tab`, `auto_complete_cycle`, delay, selector,
  triggers, `auto_complete_with_fields` and size limits and verify each behavior.
- [ ] Type `fn`, Tab, rename the first field, Tab to args, Shift+Tab back, then Tab to `$0`.
  Expand `${1:name} = $1; ${2:value} $0` from an inline/User snippet: field outlines and mirrors
  update live with Chinese/emoji and multiple carets. Esc or moving outside the field exits.
  Verify undo/redo treats a primary edit and all its mirrors as one transaction. Clone the file
  into another group: passive painting must preserve fields; editing the other clone must
  cancel stale fields in the source view.
- [ ] Add/change `.sublime-snippet` and `.sublime-completions` resources in Packages/User;
  snippet names appear as `Snippet: …` and tab triggers/resource suggestions update. Runtime
  folders and unsaved text must remain in the window.
- [ ] Hot Exit: edit unsaved named and untitled files with expanded/mirrored snippets, retain
  an undo/redo branch, create multiple windows/groups and open the index status window. Quit
  and relaunch the same profile. Text, selections, folders, layouts and normal undo/redo return;
  popup/field sessions and the separate status window are transient. Repeat with `hot_exit`
  always/only_on_quit/disabled, Cmd+W and a forced process exit after autosave. Keep the existing
  read-only-session failure guard: quit must not silently discard dirty text when saving fails.
- [ ] Start with fresh, read-only and corrupt profiles. Cache creation/fallback and subsequent
  session writes must not reload settings or drop folders added through Open Folder. Verify
  indexing exclusions, disabling/re-enabling indexing and closing an unsaved indexed buffer.
- [ ] Retest Chinese IME commits/candidate placement with a popup and inside a snippet field.
  IME preedit must suppress completion and never publish preview text to the index/session.
- [ ] Record five native intelligence runs, physical size/scale/refresh and memory. Use the
  separate `--intel-words 0` run for the real ten-file memory target and retain the default
  100k-word stress footprint separately. Compare
  p50/p95 typing to 2/4 ms and definition jumps to 20 ms; retain scanout and submit separately.


## Round 9 — full Hot Exit and review UI (human verification pending)

Use the exact disposable-profile commands at the top of RUNBOOK.md. Chinese IME confirmation
remains accepted; repeat one composition only to check the new Hot Exit boundary. Record native
results/log references here; headless tests do not check these boxes.

- [ ] **First priority — full history:** type two words, move/select text, soft undo/redo, text undo once, quit and relaunch. Continue Cmd+U/Cmd+Shift+U and Cmd+Z/Cmd+Shift+Z: text, selection direction/primary, groups, redo cursor and saved-state dirty indicator match the uninterrupted session.
- [ ] Quit halfway through typing a word, relaunch and finish it without moving the caret; one Undo removes the same typing group. Repeat after an actual save: undo/redo must return to the clean saved point.
- [ ] Clone a file into two groups; alternate text edits and selection-only moves, close one historical editing view, quit/relaunch, then undo/redo from the remaining view. Each edit retains its owner; clones retain independent selections/scroll and share text undo.
- [ ] Keep one clone hidden, quit/relaunch, activate it, save another session and relaunch again. No extra selection events appear in soft undo; clean hidden documents remain lazy.
- [ ] Change a dirty file externally while quit. Relaunch retains unsaved contents as dirty and drops text/soft history. For a changed clean file, load current disk contents and drop history. Missing/read-only/corrupt profiles still recover as documented.
- [ ] Commit one Chinese IME composition, soft-select elsewhere and quit/relaunch. The composition remains a single undoable edit; no transient preedit returns.
- [ ] In the fixture sidebar, inspect `? M ! A+ M+ D+ U!`; missing and staged-deleted paths are visible. Folder badge priority ends at Unmerged; ignored files/folders are dimmed. Hover badge areas for status tooltips, including the Open Files section.
- [ ] Check branch name and `? / M / S / !` counts against the fixture. Turn `show_git_status_in_status_bar` off; turn `show_git_status` off/on. Set an invalid `git_binary`: behavior continues through gix.
- [ ] With `sublime_merge_path` empty, clicking branch status does nothing. Configure a real smerge-compatible executable; branch/context actions open repository, file/folder history, line history and blame for the intended path. Index bytes remain unchanged by Velocity status/diff reads.
- [ ] Add/modify/delete lines, including a deletion at EOF without a final newline. Inspect color-scheme `line_diff_*` markers; Ctrl+. / Ctrl+, navigate with wrap. Test `mini_diff` true/false/"auto" both inside and outside the repository.
- [ ] Switch `git_diff_target` between index/head; stage/save/change the file externally. Markers and revert sources update to the selected reference. A non-repository save resets its saved-file reference.
- [ ] Cmd+K Cmd+/ and right-click Show/Hide Diff Hunk display original lines **beneath** current content, with changed scalar shading. Cmd+K Cmd+; shows only this hunk. Scroll, resize, wrap, move Up/Down across the read-only original rows and click below them; caret and IME candidate geometry stay aligned. An edit hides old expansions.
- [ ] Cmd+K Cmd+Z, Revert Modification and context Revert Diff Hunk restore exactly one hunk; Undo restores the edit. Repeat after grouped undo/redo; no stale hunk reverts another version.
- [ ] F6 toggles squiggles; with selector `comment, string`, comments/strings are checked and code identifiers are not. Change `misspelling` in the scheme. Scroll through long documents and two clone viewports: checks follow only visible exact scopes without an idle redraw loop.
- [ ] Ctrl+F6 / Ctrl+Shift+F6 select/reveal misspellings beyond the viewport and wrap. Move the caret or edit while a navigation request is pending; the old request must not jump later.
- [ ] Right-click a misspelling; accept a suggestion, Undo, Add to Dictionary and Ignore Word. Suggestions are version checked, Escape dismisses the menu, and words survive relaunch. Existing User preference comments/keys survive.
- [ ] View → Dictionary lists English and `Packages/User/review.dic`; select the custom dictionary. Test a syntax-specific `spell_check` setting, invalid dictionaries and a package-traversal path; no incorrect underlines or crash.
- [ ] Run automatic native on/off typing, 4K typing, full-history restore and Metal validation commands. Retain complete JSONL and assess ≤5% typing regression, 2/4 ms absolute typing and ≤300 ms native first-window targets separately from CPU results.

## Round 10: theme and minihtml acceptance

No GUI or Metal execution was performed in the sandbox. Leave these boxes unchecked until a
human runs them. Preserve the round-nine IME confirmation above; repeat the panel-input cases
because the UI now shares proportional font metrics with hit testing and candidate geometry.

Run `zsh crates/velocity/scripts/review-round10-gui.zsh /absolute/path/to/round9/velocity` on an
awake, unlocked display. It builds once, runs the new automatic native demo with Metal validation
at ordinary/4K logical sizes, then alternates round-nine/current four-group typing and scrolling.
It finally opens a disposable manual profile. Record its printed evidence directory, machine,
OS, scale/refresh, all completion/error records and any Metal diagnostics.

- [ ] Open the command palette and run **Theme & minihtml: Review Demo**. Inspect all three
  phantom placements, the right-hand annotation, the scrollable hover and the HTML sheet tab.
  Wrapped paragraphs, list markers and links must keep distinct baselines; emoji/CJK and
  bold/italic/code faces must align with their measured boxes.
- [ ] In the HTML sheet click Light/Dark, then switch OS appearance with `theme: "auto"`.
  Explicit `theme` stays selected; `dark_theme`/`light_theme` choose the auto pair. Check native
  title-bar appearance, then set `themed_title_bar: false` and verify system appearance returns.
- [ ] Visit Goto, palette, sidebar expanded/selected rows, transient/dirty tabs, find/replace
  buttons, context menu and status bar in both appearances. Hover/pressed/selected states,
  match colors, kind badges, file icons, close/fold glyphs and minimap tint must be legible.
  Drag a scroll puck and switch `overlay_scroll_bars` between enabled/disabled/system.
- [ ] Add a User theme fragment that changes a status/sidebar color, font size and content margin;
  verify live reload. Try `ui_scale` 0.75, 1, 1.5 and 2; in a search field click CJK/emoji text,
  extend selection and compose Pinyin. Caret, selection, hit testing and IME anchor must agree.
  Move between displays to check @2x/@3x resource selection and glyph/texture sharpness.
- [ ] Use the palette's **Theme & minihtml: Popup flags …** entries. Default popups close on
  selection changes; flag 16 keeps them; 4 closes on movement/click/scroll; 8 permits movement
  toward/inside the popup and closes on movement away; 32 closes on character input; 48 keeps
  selections while closing on typing. Escape closes the popup. Completion cooperation is
  exercised by the automatic demo's flag-18 popup beside the completion menu.
- [ ] Scroll a popup larger than its maximum height. Click `subl:` actions and the public HTTPS
  link, and hover titles. Link/tooltip hit regions must follow scroll/resize and stay clipped.
- [ ] Edit and undo text before an inline/below/block phantom. Text, carets, hit testing and
  phantom links must retain the same tracked anchor and reserved space. Annotations must not
  add editable rows or change vertical navigation.
- [ ] Open `fixtures/theme-ui.sublime-project` and `workbench-error.txt`, select **Theme annotations**
  in Build With and build. The error appears as a right-aligned HTML annotation with a colored
  left border; literal `<markup> &` is displayed as text. Toggle `show_errors_inline` off/on.
- [ ] In the HTML sheet click commands/browser links, scroll, resize, close/reopen the app and
  confirm its name/content return read-only. Text commands must not change its buffer.
- [ ] Repeat with fresh, unwritable and corrupt profiles, then a malformed/missing User theme
  and missing image. Confirm fallback/diagnostics and continued editing. Record actual native
  timings against the round-ten budgets; CPU structural goldens are not a pixel-quality result.
# Round 12: huge files, long lines and hex

Run `zsh crates/velocity/scripts/review-round12-gui.zsh target/release/velocity
/tmp/velocity-r12-fixtures` outside the sandbox for three automatic native runs. Each performs
100 scripted scroll/offset/edit actions and exits. Inspect its `large_file_native` record,
all load telemetry, Metal validation and presentation logs. No native result is claimed here.

For interactive checks, open the generated fixtures with `target/release/velocity -n --log-input
--data-dir /tmp/velocity-r12-manual /tmp/velocity-r12-fixtures/log-4gb.log
/tmp/velocity-r12-fixtures/line-50mb.js /tmp/velocity-r12-fixtures/binary-4gb.bin`.

1. On the 4 GB log, verify a visible prefix while the tab still shows a percentage, and the
   status bar says read-only. Scroll/select/copy within that prefix. Typing must do nothing;
   Save and Save As must be disabled and must not create a partial destination. Close during
   loading, open another file, and verify no late content replaces it. Reopen and let loading
   finish; the prefix restriction should clear and the file should be editable. Repeat with
   a large UTF-16 BOM file and a Windows-1252 file containing an invalid UTF-8 byte late in it;
   fallback may show an empty provisional view, but must retain progress and original encoding.
2. In the 50 MB JS line, wait for “Preparing line geometry” to clear, then scroll far right,
   move to the middle/end, select across the viewport and type/delete. Verify no full-line
   flash or stalled scrolling. Check Shift+arrows, Option+word arrows, Home/End, a second
   caret and pointer hit tests. Start Pinyin/emoji composition in the middle; candidate/marked
   rectangles and committed text must follow the caret. Cancel composition and undo/redo.
   Newline inserts LF without scanning an oversized line for automatic indentation.
3. Verify the large-document status hint and each **Large File: Enable … for This View**
   palette command. Explicit re-enabling can perform expensive work; automatic policy should
   remain active in another view. Viewport syntax can start approximate; a normal line after
   an oversized comment/string must receive repaired scopes without the scene thread parsing it.
4. In the binary, verify 16 hex bytes plus ASCII per row. Ctrl+G `0xee6b2800` goes to the
   `END` tail. Select those three bytes: Copy gives `45 4e 44`, **Hex: Copy Text** gives `END`.
   Try a partial final row, decimal offsets, malformed input and offsets beyond EOF. Editing,
   IME commits and Save must be blocked. Select All then Copy must report the clipboard size
   limit without trying a 4 GB allocation. Scroll with the scrollbar and trackpad. Modify a
   disposable source externally; a fresh snapshot should appear without mapping faults.
5. Close/restart with huge or binary tabs open, including a loading tab. Restore must use a
   disk reference and worker loading, with the same save restrictions, rather than treating
   the old prefix as a complete document. Verify ordinary small-file typing/scroll/IME still
   behave as in previous rounds.


## Round 13: release wiring and VoiceOver (human review required)

Run on an unlocked **macOS 14+** desktop, outside the sandbox. The script prepares a disposable
profile, a Unicode document, a windowed document and a local declarative package with a
Python marker. The isolated `vel-crash` tests exercise actual panic/signal hooks. The script
does not install the CLI or any package, or change the home directory. Choose a fresh fixture
directory; the script refuses to replace an earlier profile.

```sh
cargo build --release --locked -p velocity
scripts/package-macos.sh /tmp/velocity-r13-bundle
```

The reviewer retains the existing minimum-macOS-14 decision and the DMG signing fix. Inspect
`packaging-report.json`: prefer `hdiutil create` and require strict deep codesign of the mounted
app for distribution; a `makehybrid` fallback is not evidence of a distributable signature.

Release actions:

For the next-launch prompt, first create a uniquely named synthetic report in the normal
local crash directory (this does not crash the app or alter previous reports), then launch:

```sh
python3 - <<'PY'
from pathlib import Path
import uuid
folder = Path.home() / 'Library/Logs/Velocity'
folder.mkdir(parents=True, mode=0o700, exist_ok=True)
report = folder / ('reviewer-fixture-' + uuid.uuid4().hex + '.crash')
with report.open('x') as stream:
    stream.write('Synthetic Velocity reviewer report; no document contents.\n')
report.chmod(0o600)
print(report)
PY
```

Launch the prepared bundle with a fresh fixture profile:

```sh
zsh crates/velocity/scripts/review-round13-gui.zsh \
  /tmp/velocity-r13-bundle/Velocity.app /tmp/velocity-r13-review
```

After quitting, relaunch that same profile without rerunning fixture setup:

```sh
VELOCITY_GUI_PROCESS=1 /tmp/velocity-r13-bundle/Velocity.app/Contents/MacOS/velocity \
  --data-dir /tmp/velocity-r13-review/profile
```

- [ ] Launch: the native crash alert offers **Open Report Folder** and **Dismiss**. Open reveals
  `~/Library/Logs/Velocity`; dismiss opens nothing. The shown report becomes
  `.seen` and remains readable. Launch the executable again with the same profile,
  without creating another report: the alert does not recur. Help → Open Crash Reports Folder
  reveals that same local directory. No report upload occurs. The synthetic report can be
  removed manually after review.
- [ ] Velocity → Install Command Line Tool… shows exact source/destination paths and two bin
  choices. Select `~/.local/bin` (use Alternative when system bin is preferred).
  Cancel writes nothing; confirm creates the symlink; repeat reports an existing installation.
  Run `~/.local/bin/velocity --help`. For actual file forwarding,
  use `--data-dir /tmp/velocity-r13-review/profile`. Do not select
  `/usr/local/bin` for this disposable review. Unrelated existing commands must yield an error.
  If a new local symlink was installed for this review, remove that symlink after checking its target.
- [ ] Cmd+Shift+P → Package: Install from File… → `Reviewer Fixture.sublime-package` in the
  fixture root. Keep typing while the worker runs. Set Syntax: Reviewer Fixture, Snippet:
  Reviewer Fixture and the `reviewer` build system become available without reopening documents.
  Package: List reports resources and **partially supported: no plugin API**; select its row
  to inspect contribution paths. The Python marker never executes.
- [ ] Package: Disable/Enable removes/restores its resource choices live. Unsaved text, undo,
  selections, folders and open tabs survive. The preferences retain `KeepDisabled` and unrelated
  settings. Package: Remove removes its contributions and reports recoverable backup paths.
  Replacing an installed archive also reports the previous archive’s backup.
- [ ] Package: Install from URL… accepts a reviewer-controlled HTTPS archive URL, then a package
  name, including URLs with query parameters. Invalid TLS, HTTP URLs and invalid ZIPs show errors
  while the editor stays usable. No public service is configured or needed for local checks.
- [ ] Package: Check for Upgrades is absent with `package_index_url: null`. With an explicit
  reviewer-controlled HTTPS version-1 index (schema in `vel-packages/README.md`), it appears.
  Nothing fetches until invoked. Available/current versions appear before an installation
  confirmation. Cancel retains the archive; confirmation checks digest/metadata and retains
  disabled state. Restore the setting to null after review.

VoiceOver: enable with **Cmd+F5** (or the system accessibility shortcut). **VO** below means
Control+Option. Use Accessibility Inspector alongside VoiceOver to inspect the public AX values.

- [ ] Interact with the workspace/text area (VO+Shift+Down). The editor has role `AXTextArea`,
  a document label and the visible/windowed text value. Read `Hello 你好 😀 é` in order. Arrow,
  Shift+Arrow and mouse selection changes announce their current caret/selection rather than
  stale text. Typing, native IME preedit/commit and Undo update the value and selection once.
- [ ] Inspect `AXSelectedText`, `AXSelectedTextRange(s)` and `AXInsertionPointLineNumber`.
  UTF-16 offsets count 😀 as two units; combining marks retain valid scalar boundaries. Reverse
  a multiline selection: insertion line follows the actual primary caret. Add multiple carets:
  AX exposes the primary only; typing still edits all editor carets. Setting an AX range updates
  the primary editor selection, and no half surrogate is exposed or replaced.
- [ ] Query `AXLineForIndex`, `AXRangeForLine`, `AXStringForRange` and `AXBoundsForRange` in
  Inspector. Include newline, end-of-document and empty ranges. Bounds align with text at 1×/2×,
  after horizontal/vertical scrolling, wrapping, folds, window resize, UI scaling and splits.
  Screen bounds use points; a Retina display must not double offsets or sizes.
- [ ] Open `project/windowed.txt`, move near the end and query its value. Each value has at most
  **65,536 UTF-16 units**; Help reports its document offset and first line. Selected/parameterized
  ranges are relative to that value; insertion line remains document-relative. Move the caret
  beyond the old window and confirm the next value/range queries track it. Whole-value AX
  replacement is unavailable for a partial window. No full-document flattening or UI pause.
- [ ] Traverse with Cmd+F6 / Cmd+Shift+F6 and VO navigation. Tabs expose document names, selected
  state and separate Close buttons; Return/Space or VO+Space activates focused controls. Visit
  both tabs, close/cancel a dirty tab, then switch groups. The primary editor follows the selected
  group. Closing the quick panel restores the editor’s focus.
- [ ] The side bar is an outline with labelled open-file/project rows, disclosure depth and
  expansion state. Select/open files and expand/collapse directories with AX actions. Hidden
  rows do not become duplicate elements. Goto/palette rows expose captions, badges/details and
  selected state; filtering and Enter/VO+Space choose the announced row.
- [ ] Find/Replace and Find in Files fields have distinct Find/Replace/Where labels. Regex,
  case, whole-word, wrap, selection and preserve-case controls have named actions; Find Next,
  Find All and Replace All buttons work. Standard Tab/Shift+Tab retains input routing; the
  explicit Cmd+F6 traversal includes non-text controls.
- [ ] Status text announces line/column, selections, encoding and current message. The syntax
  popup opens its picker. Completion rows, context-menu choices and visible minihtml popup
  text/links have semantic names/actions. Activate a popup link and confirm its normal action.
- [ ] System Settings → Accessibility → Display → Reduce motion, toggled **while running**:
  theme tint/opacity transitions and texture sequences settle immediately; no theme animation
  resumes on resource reload. Toggle Increase contrast: chrome control boundaries strengthen.
  Toggle back and confirm ordinary animations/appearance return. Custom syntax color contrast
  remains the scheme author’s choice.

Record macOS version, display scale, VoiceOver version/verbosity, exact failing action and AX
attribute values. GUI behavior, actual speech and screen-coordinate alignment are **pending
human review**; headless tests do not establish these outcomes. Binary hex documents currently
have a read-only description rather than a navigable AX hex-text value.

## Round 14: native opens, context menus and round-12 regressions

Run on an unlocked macOS 14+ desktop, outside the sandbox, using a bundle prepared by the
reviewer. This round does **not** run the packaging script. The headless plist test only
serializes document-type declarations; it does not assemble, sign or launch an application.
Replace the app path below with the reviewer's updated bundle. Use a new fixture directory.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --release
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps
/usr/bin/python3 -B -m unittest discover -s crates/velocity/packaging -p 'test_bundle.py'
cargo build --release --locked -p velocity
python3 crates/velocity/scripts/review-round14-fixtures.py /tmp/velocity-r14-native --huge

APP=/absolute/path/to/Velocity.app
FIXTURE=/tmp/velocity-r14-native
PROFILE="$FIXTURE/profile"
DOCS="$FIXTURE/Project with spaces"
# Cold LaunchServices launch: the filename travels in the open Apple event, not argv.
/usr/bin/open -na "$APP" "$DOCS/one.rs" --args --data-dir "$PROFILE"
# With the first instance still running, route to its key window.
/usr/bin/open -a "$APP" "$DOCS/two.txt"
# CLI locations still use the existing per-profile socket.
"$APP/Contents/SharedSupport/bin/velocity" --data-dir "$PROFILE" "$DOCS/one.rs:120:7"
```

- [ ] **Cold Finder Open With:** quit Velocity, then Finder → Open With → this bundle for
  `one.rs`. Repeat for `any-file.velocity-blob`, a folder, and `literal:12:3`. Text opens in
  a tab, binary data in read-only hex, folders in the sidebar, and the native colon filename
  remains literal. Text/data are Alternate Editor handlers; folders are Alternate Viewer
  handlers. Check the receiving process remains alive rather than handing off an empty GUI.
- [ ] **Double-click:** on a disposable fixture extension, Get Info → Open With → Velocity →
  Change All…, quit, and double-click. Verify cold and running launches. Restore the previous
  association afterward. Velocity must not take document ownership merely by being installed.
- [ ] **Dock drop:** drag several files and a folder onto Velocity's Dock icon, first cold and
  then running. While startup/loading is busy, issue another open. Requests retain order and
  are applied after window initialization. Drop a duplicate, a symlink alias and an edited
  open file: focus its existing tab while preserving its text, selections and scroll.
- [ ] **Window drop:** drop files/folders over the editor, tab row and sidebar, including
  Unicode/space-containing paths. Existing text dragging/input behavior remains usable. Drop
  `huge.log` and the binary: retain background prefix/huge-file and hex behavior. Use an
  inaccessible or subsequently removed file and confirm a path-specific error, with later
  valid entries still opened.
- [ ] **Running instance/windows:** open two windows, make the second key and use Finder Open
  With and `open -a` again. Only that key window receives the files. Close all windows without
  quitting, then click the Dock icon: create a fresh window. Repeat with a Finder open and a
  CLI request while no window exists. The retained template polls IPC without rendering.
  `velocity --data-dir "$PROFILE" -w "$DOCS/two.txt"` must return when its sheet/window closes.
  Force a secondary LaunchServices process with
  `open -na "$APP" "$DOCS/literal:12:3" "$DOCS/two.txt" --args --data-dir "$PROFILE"`:
  its queued batches must reach the primary before the temporary receiver quits. Repeat while
  the primary quits; takeover must retain all recovered windows and dirty documents.

Menu resources are original Velocity files in `Packages/Default`: `Context.sublime-menu`,
`Tab Context.sublime-menu` and `Side Bar.sublime-menu`. Enabled packages and User extend their
stable IDs with the same resource precedence as the menu bar. `velocity_dynamic` inserts the
existing spelling, diff and Git choices. Commands receive resource args plus literal `paths`,
clicked `group`, group-local `index`, and the window-local `event: {x,y}` point. Internal sheet
identity keeps targets stable while workers/IPC progress during tracking.

- [ ] **Native tracking:** right-click, two-finger click and Control-click on each surface.
  Verify native menu appearance, keyboard navigation, Escape dismissal, disabled rows and
  checks. Keep a menu open while indexing/loading and IPC complete; select an action, then
  type/scroll immediately. No borrow panic, hung tracking loop or menu-bar replacement occurs.
  During the spelling-worker wait, Escape, typing and scrolling cancel the deferred popup.
  A User menu `chain` and a window command such as `toggle_setting` inherit the clicked tab's
  context and check its state even when another tab was previously active.
- [ ] **Editor:** right-click inside either of two selections and Copy: preserve all regions.
  Right-click outside all selections: move the caret first. Repeat in an inactive split.
  Cut/Copy/Paste/Select Document, cached/worker spelling replacements, dictionary actions,
  diff show/revert and configured Git actions work on the pointed document/hunk/word. Copy
  Full File Path, Locate in Project Tree and Show in File Browser use the pointed document.
- [ ] **Tabs:** right-click an inactive tab and use Close This Tab, Keep Only This Tab, Close
  Following Tabs and Close Saved Tabs. Targets follow that group's visual order. Dirty
  prompts, Save/Discard/Cancel and shared views retain their usual behavior. Create Untitled
  File opens in the clicked group; path/reveal actions use the clicked tab.
- [ ] **Sidebar:** Open Selected Entry expands directories/opens files. Create File/Folder
  and Change Entry Name use the existing input panel; Escape cancels, an existing destination
  reports an error, and separators/traversal names are rejected. Rename an open dirty file and
  a parent directory: retarget all sheets/descendants, keep undo and watch the new path.
  Move Entry to Trash first asks confirmation; Cancel changes nothing. Confirm moves to Finder
  Trash, including symlink entries, with no permanent-delete fallback. Open buffers remain
  readable, marked `[deleted]` and dirty, and can be saved. A previously clean deleted buffer
  must prompt on close and survive Hot Exit; saving either shared view clears the markers.
  Clipboard/reveal/permission errors surface.
- [ ] Copy Full Path and Copy Project-Relative Path have the expected text. Search This Folder
  opens Find in Files with that directory in Where. Detach Project Folder removes the root
  without deleting its files; configured Git choices retain the selected path. Repeat with
  User menu overrides, disabled packages and a hot resource reload.
- [ ] **Accessibility:** VoiceOver reads native menu items and their disabled/checked state;
  no second custom context list appears in the workspace AX tree. Dismissal leaves the round-13
  editor/tab/sidebar/input AX elements and focus behavior intact.

Round-12 regressions and change checks:

- [ ] CLI `one.rs:120:7` centers line 120 at startup and over IPC, even after restoring an old
  scroll position. Repeat in a resized/split/wrapped view and while a large file is loading.
- [ ] In light and dark themes at multiple UI scales, the strip above the sidebar matches its
  themed background and contains no black clear-color rectangle.
- [ ] Window title is the file name plus dirty marker and optional ` — project`; switching,
  typing, saving and external changes update it. No debug/native AppKit title or encoding/EOL
  suffix. Unchanged frames do not call the native title setter.
- [ ] UI labels/headings use system sans-serif regular/bold/italic faces at their actual sizes.
  No CoreText message about requesting private `.SFNS-*` names or substituting Times appears.
  The renderer side is `vel-render/src/native.rs` (`font_location` uses the CoreText UI font for
  system names).
- [ ] A normal launch creates no interactive telemetry file. Quit, then opt in with the direct
  command below. Confirm the exact default name `interactive-<runid>-<pid>.jsonl`, valid JSONL,
  and the JSON mode/PID; no compile-time source directory receives output. Unset
  `VELOCITY_LOG_DIR` and repeat to check `~/Library/Logs/Velocity/`. Explicit `--output` is an
  opt-in destination, creates a new file and refuses to overwrite one.

```sh
VELOCITY_GUI_PROCESS=1 VELOCITY_LOG_DIR="$FIXTURE/logs" \
  "$APP/Contents/MacOS/velocity" --data-dir "$PROFILE" --log-input "$DOCS/one.rs"
```

- [ ] **Reviewer regressions:** Cmd+F/Replace/Incremental/Find in Files toggle; Escape from
  editor focus and each find panel's close button hide the panel/match paint without losing
  Find Next. Scroll and mouse moves stay coalesced on the display link; keys/clicks remain
  immediate. The resource-stamp walk runs at most every 500 ms; an explicit package reload
  takes effect immediately.
- [ ] **Real-event profile:** while scrolling a 10 MB file using the trackpad, sample the app
  (for example `sample PID 10 -file /tmp/velocity-r14-native/native-sample.txt`). Inspect
  native menu/title work: no per-frame `setMainMenu`/menu reconstruction or unchanged title
  mutation. Record receipt-to-submit p50/p95, display refresh rate and frame gaps using lane
  B's native scroll gate after integration. Headless CPU numbers do not establish native
  AppKit, font rasterization, GPU, scanout, VoiceOver or Finder/Dock outcomes.

Paired CPU reviewer commands (use the retained clean `4b65f50` binary as BEFORE):

```sh
python3 crates/velocity/scripts/measure-typing.py \
  --before /tmp/velocity-r14-baseline/velocity-main-4b65f50 \
  --after target/release/velocity --runs 10 --output-dir /tmp/velocity-r14-typing-review
python3 crates/velocity/scripts/compare-scroll.py \
  --old /tmp/velocity-r14-baseline/velocity-main-4b65f50 \
  --new target/release/velocity --pairs 10 --output-dir /tmp/velocity-r14-scroll-review
```

Run pairs sequentially with no concurrent compilation. Retain raw JSONL, binary hashes and
summary confidence intervals. Typing and scrolling each have a **≤5% regression** budget;
report CPU budgets separately from pending native measurements.
