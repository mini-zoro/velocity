//! Process-wide memory maintenance on the native UI thread's existing poll ticks.
use crate::{AppHandler, MemoryCacheBytes, MemoryPressure};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
#[path = "memory_guard_macos.rs"]
pub(crate) mod macos;
#[cfg(windows)]
#[path = "memory_guard_windows.rs"]
pub(crate) mod windows;

const GIB: u64 = 1 << 30;
const QUIET: Duration = Duration::from_millis(750);
const BACKGROUND: Duration = Duration::from_secs(60);
const SAMPLE: Duration = Duration::from_secs(30);
const COOLDOWN: Duration = Duration::from_secs(5 * 60);

fn ceiling(documents: u64) -> u64 {
    GIB.saturating_add(documents.saturating_mul(2))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Request {
    Idle,
    Pressure(MemoryPressure),
    Watchdog { before: u64, documents: u64 },
}

impl Request {
    pub(crate) fn deliver(self, app: &mut dyn AppHandler) -> Option<MemoryCacheBytes> {
        match self {
            Self::Idle => app.on_background_memory_idle(),
            Self::Pressure(level) => app.on_memory_pressure(level),
            Self::Watchdog { .. } => app.on_memory_pressure(MemoryPressure::Warning),
        }
    }
}

#[derive(Default)]
struct Watchdog {
    pending: Option<MemoryPressure>,
    input: Option<Instant>,
    next_tick: Option<Instant>,
    next_sample: Option<Instant>,
    last_trim: Option<Instant>,
    background: Option<Instant>,
    background_trimmed: bool,
}

impl Watchdog {
    fn pressure(&mut self, level: MemoryPressure) {
        self.pending = self.pending.max(Some(level));
    }

    fn foreground(&mut self) {
        self.background = None;
        self.background_trimmed = false;
    }

    fn due(&self, now: Instant) -> bool {
        self.pending.is_some() || self.next_tick.is_none_or(|next| now >= next)
    }

    fn tick(
        &mut self,
        now: Instant,
        foreground: bool,
        footprint: impl FnOnce() -> Option<u64>,
        loaded_bytes: impl FnOnce() -> Option<u64>,
    ) -> Option<Request> {
        // A busy editor may already poll every 8 ms. No extra wakes, and cheap
        // housekeeping only once a second unless a system event is queued.
        self.next_tick = Some(now + Duration::from_secs(1));
        if foreground {
            self.foreground();
        } else {
            self.background.get_or_insert(now);
        }
        let idle_trim =
            !self.background_trimmed && self.background.is_some_and(|at| now - at >= BACKGROUND);
        // System events are queued until this tick, but do not require a quiet key window.
        if let Some(level) = self.pending.take() {
            self.background_trimmed |= idle_trim;
            self.next_sample = Some(now + SAMPLE);
            return Some(Request::Pressure(level));
        }
        if self.input.is_some_and(|at| now - at < QUIET) {
            return None;
        }
        if self.next_sample.is_none_or(|next| now >= next)
            && self.last_trim.is_none_or(|at| now - at >= COOLDOWN)
        {
            self.next_sample = Some(now + SAMPLE);
            if let Some(before) = footprint().filter(|&bytes| bytes > GIB)
                && let Some(documents) = loaded_bytes()
                && before > ceiling(documents)
            {
                self.last_trim = Some(now);
                self.background_trimmed |= idle_trim;
                return Some(Request::Watchdog { before, documents });
            }
        }
        if idle_trim {
            self.background_trimmed = true;
            return Some(Request::Idle);
        }
        None
    }
}

#[cfg(any(target_os = "macos", windows))]
thread_local! {
    static WATCHDOG: std::cell::RefCell<Watchdog> = std::cell::RefCell::default();
}

#[cfg(any(target_os = "macos", windows))]
pub(crate) fn reset() {
    WATCHDOG.with_borrow_mut(|guard| *guard = Watchdog::default());
}

#[cfg(any(target_os = "macos", windows))]
pub(crate) fn became_foreground() {
    WATCHDOG.with_borrow_mut(Watchdog::foreground);
}

#[cfg(any(target_os = "macos", windows))]
pub(crate) fn note_input() {
    WATCHDOG.with_borrow_mut(|wd| wd.input = Some(Instant::now()));
}

#[cfg(any(target_os = "macos", windows))]
pub(crate) fn queue_pressure(level: MemoryPressure) {
    WATCHDOG.with_borrow_mut(|wd| wd.pressure(level));
}

#[cfg(any(target_os = "macos", windows))]
pub(crate) fn sum_documents(bytes: impl Iterator<Item = u64>) -> Option<u64> {
    bytes.reduce(u64::saturating_add)
}

#[cfg(any(target_os = "macos", windows))]
pub(crate) fn service(
    now: Instant,
    can_dispatch: impl FnOnce() -> bool,
    foreground_and_dragging: impl FnOnce() -> (bool, bool),
    os_pressure: impl FnOnce() -> Option<MemoryPressure>,
    loaded_bytes: impl FnOnce() -> Option<u64>,
    deliver: impl FnOnce(Request) -> Option<MemoryCacheBytes>,
) {
    use crate::Platform;

    if !WATCHDOG.with_borrow(|wd| wd.due(now)) || !can_dispatch() {
        return;
    }
    if let Some(pressure) = os_pressure() {
        // Windows observes the signal here; leave the cache pass to the next
        // event-loop tick, just like the macOS dispatch source does.
        queue_pressure(pressure);
        return;
    }
    let (foreground, dragging) = foreground_and_dragging();
    let request = WATCHDOG.with_borrow_mut(|wd| {
        if dragging {
            wd.input = Some(now);
        }
        wd.tick(now, foreground, Platform::memory_footprint, loaded_bytes)
    });
    if let Some(request) = request
        && let Some(sizes) = deliver(request)
    {
        Platform::release_free_heap_pages();
        if let Request::Watchdog { before, documents } = request {
            // This diagnostic read is paired with the triggering sample; it
            // doesn't schedule or trigger another watchdog decision.
            let after = Platform::memory_footprint()
                .map_or_else(|| "unavailable".into(), |n| n.to_string());
            use std::io::Write;
            let _ = writeln!(
                std::io::stderr().lock(),
                "velocity memory guard: before={before} after={after} document_bytes={documents} ceiling={} atlas_bytes={} instance_bytes={} font_bytes={} image_bytes={} find_bytes={}",
                ceiling(documents),
                sizes.atlas,
                sizes.instances,
                sizes.fonts,
                sizes.images,
                sizes.find,
            );
        }
    }
}

#[cfg(any(target_os = "macos", windows))]
pub(crate) fn sum_caches(
    bytes: impl Iterator<Item = MemoryCacheBytes>,
) -> Option<MemoryCacheBytes> {
    bytes.reduce(|a, b| MemoryCacheBytes {
        atlas: a.atlas.saturating_add(b.atlas),
        instances: a.instances.saturating_add(b.instances),
        fonts: a.fonts.saturating_add(b.fonts),
        images: a.images.saturating_add(b.images),
        find: a.find.saturating_add(b.find),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watchdog_ceiling_and_rate_limit() {
        assert_eq!(ceiling(0), GIB);
        assert_eq!(ceiling(123), GIB + 246);
        assert_eq!(ceiling(u64::MAX), u64::MAX);

        let start = Instant::now();
        let mut guard = Watchdog::default();
        let documents = 123;
        let before = ceiling(documents) + 1;
        let check = |guard: &mut Watchdog, secs, bytes: Option<u64>| {
            guard.tick(
                start + Duration::from_secs(secs),
                true,
                || bytes,
                || Some(documents),
            )
        };
        assert_eq!(check(&mut guard, 0, Some(before - 1)), None);
        assert_eq!(check(&mut guard, 29, Some(before)), None);
        let trim = Some(Request::Watchdog { before, documents });
        assert_eq!(check(&mut guard, 30, Some(before)), trim);
        assert_eq!(check(&mut guard, 329, Some(before)), None);
        assert_eq!(check(&mut guard, 330, Some(before)), trim);
    }

    #[test]
    fn watchdog_waits_for_quiet_and_trims_once_per_background_period() {
        let start = Instant::now();
        let secs = |n| start + Duration::from_secs(n);
        let mut guard = Watchdog {
            input: Some(start),
            ..Default::default()
        };
        assert_eq!(
            guard.tick(start, true, || panic!("while typing"), || Some(0)),
            None
        );
        assert!(guard.due(secs(1)));
        assert_eq!(guard.tick(secs(1), false, || None, || Some(0)), None);
        assert_eq!(guard.tick(secs(60), false, || None, || Some(0)), None);
        assert_eq!(
            guard.tick(secs(61), false, || None, || Some(0)),
            Some(Request::Idle)
        );
        assert_eq!(guard.tick(secs(121), false, || None, || Some(0)), None);
        guard.foreground(); // including a brief activation between polls
        assert_eq!(guard.tick(secs(122), false, || None, || Some(0)), None);
        assert_eq!(
            guard.tick(secs(182), false, || None, || Some(0)),
            Some(Request::Idle)
        );
    }

    #[test]
    fn queued_pressure_reaches_each_handler_on_the_tick_even_during_input() {
        #[derive(Default)]
        struct Handler(Option<MemoryPressure>);
        impl AppHandler for Handler {
            fn on_frame(&mut self, _: &mut crate::FrameContext<'_>) -> Result<(), String> {
                Ok(())
            }
            fn on_memory_pressure(&mut self, level: MemoryPressure) -> Option<MemoryCacheBytes> {
                assert!(self.0.replace(level).is_none());
                Some(MemoryCacheBytes::default())
            }
        }
        let now = Instant::now();
        let mut apps = [Handler::default(), Handler::default()];
        let mut guard = Watchdog {
            input: Some(now),
            ..Default::default()
        };
        guard.pressure(MemoryPressure::Warning);
        guard.pressure(MemoryPressure::Critical);
        assert!(apps.iter().all(|app| app.0.is_none()));
        let request = guard
            .tick(now, true, || panic!("not the watchdog"), || None)
            .unwrap();
        for app in &mut apps {
            request.deliver(app).unwrap();
            assert_eq!(app.0, Some(MemoryPressure::Critical));
        }
        assert!(!guard.due(now));
    }
}
