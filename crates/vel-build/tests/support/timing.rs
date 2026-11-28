//! Wall-clock rules for the build tests (shared by the unit and integration tests).
//!
//! These tests run in parallel beside the whole workspace, so a wall clock appears in
//! two roles only. A hang guard ([`HANG_GUARD`]) stops a test that would otherwise wait
//! forever; it is not a latency claim. A performance contract ("returns within N ms")
//! is enforced only in an explicit timing run, `VELOCITY_STRICT_TIMING=1`; other runs
//! use a relaxed bound that still tells the behaviour apart from the failure.
#![allow(dead_code)]

use std::time::Duration;

/// Hang guard for waiting on a condition that normally holds within milliseconds.
pub const HANG_GUARD: Duration = Duration::from_secs(120);

/// Whether this is an explicit timing run (`VELOCITY_STRICT_TIMING` set, and not `0`).
pub fn strict_timing() -> bool {
    std::env::var_os("VELOCITY_STRICT_TIMING")
        .is_some_and(|value| !value.is_empty() && value != "0")
}

/// `strict` in an explicit timing run, `relaxed` otherwise.
pub fn budget(strict: Duration, relaxed: Duration) -> Duration {
    if strict_timing() { strict } else { relaxed }
}

/// A performance contract: `elapsed` stays under `limit` in an explicit timing run.
/// Other runs only print the measurement.
#[track_caller]
pub fn within(elapsed: Duration, limit: Duration, what: &str) {
    eprintln!("timing: {what}: {elapsed:?} (contract {limit:?})");
    if strict_timing() {
        assert!(elapsed < limit, "{what}: {elapsed:?}, contract {limit:?}");
    }
}
