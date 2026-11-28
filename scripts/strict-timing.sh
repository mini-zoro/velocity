#!/usr/bin/env bash
# Runs the performance contracts: tests whose wall-clock budgets are enforced only when
# VELOCITY_STRICT_TIMING=1. Every other run of these tests checks their behaviour and
# only prints the time, because the parallel suite on a shared machine cannot hold
# millisecond budgets. Here each test runs alone (--test-threads=1) on a build the
# caller already made, so the budget measures the code rather than the rest of the
# suite.
#
# CI runs this as an informational step (ci.yml); locally:
#   scripts/strict-timing.sh            # release build, like CI
#
# A renamed or deleted contract fails the run (the count of tests that ran must match
# the list), so update CONTRACTS together with the test.
set -u

CONTRACTS=(
    # velocity (bin unit tests)
    app::lane_b_round18_tests::verification::final_verification::round18_final_continuous_daemon_finishes_and_cancels_promptly
    app::lane_b_round18_tests::verification::final_verification::third_verification::round18_third_burst_is_bounded_and_publishes_tail_after_exit
    app::lane_b_round18_tests::verification::final_verification::third_verification::round18_third_cancel_after_producer_exit_discards_retained_tail
    app::lane_b_round18_tests::verification::final_verification::third_verification::round18_third_cancel_discards_burst_backlog_immediately
    app::lane_b_round19_tests::r19_app_sustained_build_output_has_steady_progress_and_lossless_tail
    app::lane_b_round19_tests::recovery::special::r19_special_restore_watchdog_backs_up_and_keeps_dirty_text
    app::round18_tests::find_files_tests::find_results_tab_streams_while_typing_within_frame_budget
    app::round18_tests::long_wrap_tests::wrapped_four_mib_repaints_row_slices_and_matches_fresh_layout_after_typing
    app::round18_tests::round20_review::r20_pure_insert_minimap_width_is_refreshed_by_next_publication
    app::round22::hot_exit_quits_without_loading_twenty_hidden_documents
    panels::round19_tests::r19_hover_stale_embedded_pointer_moves_never_parse
    # vel-build (unit and integration tests)
    environment::tests::round18_final_login_cache_shared_across_build_overrides
    environment::tests::round18_login_detached_pipe_obeys_deadline_and_retries
    environment::tests::round18_login_failure_and_timeout_are_bounded_and_do_not_block_the_caller
    environment::tests::round18_verify_login_failure_backoff_slow_success_and_profile_edit
    cancel_reaps_process_tree_and_unblocks_inherited_pipes
    dropping_a_build_with_full_output_queue_reaps_its_tree_without_a_consumer
    round18_detached_output_pipe_finishes_cancels_and_restarts_without_waiting
    round18_restart_kills_child_and_grandchild_before_new_process_starts
    round19_sustained_output_keeps_ui_polling_progress_with_regex
    # vel-fileio, vel-session
    regular::tests::checked_document_opens_reject_a_special_file_swapped_after_stat
    generations::tests::session_writer_makes_progress_during_continuous_changes
)

cd "$(dirname "$0")/.." || exit 2
log=$(mktemp "${TMPDIR:-/tmp}/strict-timing.XXXXXX")
status=0
# Same profile and features as the main test step, so cargo reuses its build. Library,
# binary and integration-test harnesses only: doctests hold no contracts.
VELOCITY_STRICT_TIMING=1 cargo test --workspace --locked --release --no-fail-fast \
    --lib --bins --tests -- --exact --test-threads=1 --nocapture "${CONTRACTS[@]}" >"$log" 2>&1 ||
    status=$?

ran=$(awk '/^test result:/ { n += $4 + $6 } END { print n + 0 }' "$log")
failed=$(awk '/^test result:/ { n += $6 } END { print n + 0 }' "$log")
summary="strict timing: ${ran} of ${#CONTRACTS[@]} contracts ran, ${failed} failed"

# --nocapture interleaves test output with the result lines, so failures are read
# from libtest's closing "failures:" list.
failures=$(awk '/^failures:$/ { list = 1; next } /^test result:/ { list = 0 }
    list && /^    [A-Za-z0-9_:]+$/ { print $1 }' "$log" | sort -u)
grep -oE 'timing: .*' "$log" || true
grep -E -A3 'panicked at' "$log" || true
echo "$summary"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    {
        echo "### Strict timing contracts (informational)"
        echo
        echo "$summary"
        echo
        echo '```'
        grep -oE 'timing: .*|.*FAILED.*|.*panicked at.*' "$log" || true
        echo '```'
    } >>"$GITHUB_STEP_SUMMARY"
fi
if [ "$ran" -ne "${#CONTRACTS[@]}" ]; then
    echo "::warning::strict timing: expected ${#CONTRACTS[@]} contracts, ${ran} ran (renamed or removed test?)"
    status=1
fi
if [ "$failed" -ne 0 ]; then
    while read -r name; do
        [ -n "$name" ] && echo "::warning::strict timing contract failed: $name"
    done <<<"$failures"
    status=1
fi
rm -f "$log"
exit "$status"
