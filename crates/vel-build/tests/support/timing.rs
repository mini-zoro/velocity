//! Wall-clock rules for the build tests (shared by the unit and integration tests).
//!
//! These tests run in parallel beside the whole workspace, so a wall clock in a test is
//! a hang guard ([`HANG_GUARD`]): it stops a test that would otherwise wait forever. It
//! is not a latency claim; the test checks the condition it waits for.
#![allow(dead_code)]

use std::time::Duration;

/// Hang guard for waiting on a condition that normally holds within milliseconds.
pub const HANG_GUARD: Duration = Duration::from_secs(120);
