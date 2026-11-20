//! Opt-in attribution for synchronous parsing on the current thread.
use std::{cell::RefCell, collections::BTreeMap, time::Instant};

#[derive(Debug, Default, serde::Serialize)]
struct Timing {
    calls: u64,
    nanos: u128,
}

/// Counts and accumulated wall time from an instrumented parse, not throughput samples.
#[derive(Debug, Default, serde::Serialize)]
pub struct Profile {
    contexts: BTreeMap<String, Timing>,
    native_searches: BTreeMap<String, Timing>,
    native_capture_searches: BTreeMap<String, Timing>,
    events: BTreeMap<&'static str, u64>,
}

thread_local! {
    static CURRENT: RefCell<Option<Profile>> = const { RefCell::new(None) };
}

/// Starts a fresh profile for the current thread.
pub fn start() {
    CURRENT.with_borrow_mut(|current| *current = Some(Profile::default()));
}

/// Finishes the profile and disables collection on the current thread.
pub fn finish() -> Profile {
    CURRENT.with_borrow_mut(Option::take).unwrap_or_default()
}

/// Counts a route or parser event, when profiling is active.
pub fn count(event: &'static str) {
    CURRENT.with_borrow_mut(|profile| {
        if let Some(profile) = profile {
            *profile.events.entry(event).or_default() += 1;
        }
    });
}

fn timed<T>(
    key: &str,
    run: impl FnOnce() -> T,
    select: impl FnOnce(&mut Profile) -> &mut BTreeMap<String, Timing>,
) -> T {
    let active = CURRENT.with_borrow(Option::is_some);
    if !active {
        return run();
    }
    let at = Instant::now();
    let value = run();
    let nanos = at.elapsed().as_nanos();
    CURRENT.with_borrow_mut(|profile| {
        if let Some(profile) = profile {
            let map = select(profile);
            let row = if let Some(row) = map.get_mut(key) {
                row
            } else {
                map.entry(key.to_owned()).or_default()
            };
            row.calls += 1;
            row.nanos += nanos;
        }
    });
    value
}

/// Times one ordered matcher call for a named context.
pub fn with_context<T>(name: &str, run: impl FnOnce() -> T) -> T {
    timed(name, run, |profile| &mut profile.contexts)
}

pub(crate) fn with_rule<T>(source: &str, captures: bool, run: impl FnOnce() -> T) -> T {
    timed(source, run, |profile| {
        if captures {
            &mut profile.native_capture_searches
        } else {
            &mut profile.native_searches
        }
    })
}
