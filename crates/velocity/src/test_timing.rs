//! Wall-clock rules for this binary's tests.
//!
//! The tests run in parallel, beside other test binaries, on loaded CI runners and in
//! the local gate, so the time any one step takes says little about the code. A wall
//! clock in a test is a hang guard ([`HANG_GUARD`]): it stops a test that would
//! otherwise wait forever for a condition. It is not a latency claim: the condition is
//! what the test checks, and the guard is generous so that a busy machine never trips
//! it.

use std::time::Duration;

/// Hang guard for waiting on a condition that normally holds within milliseconds.
pub(crate) const HANG_GUARD: Duration = Duration::from_secs(120);
