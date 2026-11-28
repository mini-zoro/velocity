//! Wall-clock rules for this binary's tests.
//!
//! The tests run in parallel, beside other test binaries, on loaded CI runners and in
//! the local gate, so the time any one step takes says little about the code. A wall
//! clock may appear in a test in two roles only:
//!
//! - A hang guard ([`HANG_GUARD`]) stops a test that would otherwise wait forever for a
//!   condition. It is not a latency claim: the condition is what the test checks, and the
//!   guard is generous so that a busy machine never trips it.
//! - A performance contract ("X finishes within N ms") is checked only in an explicit
//!   timing run, `VELOCITY_STRICT_TIMING=1` ([`strict_timing`], [`within`]). Every other
//!   run checks the behaviour and prints the time it took.

use std::time::Duration;

/// Hang guard for waiting on a condition that normally holds within milliseconds.
pub(crate) const HANG_GUARD: Duration = Duration::from_secs(120);

/// Whether this is an explicit timing run (`VELOCITY_STRICT_TIMING` set, and not `0`).
pub(crate) fn strict_timing() -> bool {
    std::env::var_os("VELOCITY_STRICT_TIMING")
        .is_some_and(|value| !value.is_empty() && value != "0")
}

/// `strict` in an explicit timing run, `relaxed` otherwise. The relaxed bound still has
/// to separate the behaviour under test from the failure it guards against (for example
/// "returns before the 60 s fixture would let it"), never a measured latency.
pub(crate) fn budget(strict: Duration, relaxed: Duration) -> Duration {
    if strict_timing() { strict } else { relaxed }
}

/// A performance contract: `elapsed` stays under `limit` in an explicit timing run.
/// Other runs only print the measurement.
#[track_caller]
pub(crate) fn within(elapsed: Duration, limit: Duration, what: &str) {
    eprintln!("timing: {what}: {elapsed:?} (contract {limit:?})");
    if strict_timing() {
        assert!(elapsed < limit, "{what}: {elapsed:?}, contract {limit:?}");
    }
}
