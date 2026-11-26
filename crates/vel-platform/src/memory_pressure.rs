//! Opt-in, process-wide allocator relief for completed large document work.

/// Minimum logical size of a document or composite edit worth scheduling.
pub const LARGE_WORK_BYTES: usize = 1024 * 1024;

#[cfg(target_os = "macos")]
use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, Thread},
    time::{Duration, Instant},
};

#[cfg(target_os = "macos")]
const QUIET_MS: u64 = 750;
#[cfg(target_os = "macos")]
const COOLDOWN_MS: u64 = 5_000;
#[cfg(target_os = "macos")]
static ENABLED: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "macos")]
static WORKER: OnceLock<Option<Worker>> = OnceLock::new();

/// Enables automatic relief for an interactive process, without starting a worker.
pub fn enable() {
    #[cfg(target_os = "macos")]
    ENABLED.store(true, Ordering::Relaxed);
}

/// True in an interactive process where requests have an effect.
pub fn enabled() -> bool {
    #[cfg(target_os = "macos")]
    return ENABLED.load(Ordering::Relaxed);
    #[cfg(not(target_os = "macos"))]
    false
}

/// Queues best-effort relief after the caller's large temporary work settles.
pub fn request() {
    #[cfg(target_os = "macos")]
    if enabled()
        && let Some(worker) = WORKER.get_or_init(Worker::start)
    {
        worker
            .schedule
            .due
            .fetch_max(worker.schedule.now() + QUIET_MS, Ordering::AcqRel);
        worker.thread.unpark();
    }
}

/// Extends the quiet interval without taking a lock or creating a worker.
#[cfg(target_os = "macos")]
pub(crate) fn note_activity() {
    if let Some(Some(worker)) = WORKER.get() {
        let schedule = &worker.schedule;
        if schedule.due.load(Ordering::Relaxed) != 0 {
            let later = schedule.now() + QUIET_MS;
            let _ = schedule
                .due
                .try_update(Ordering::AcqRel, Ordering::Relaxed, |due| {
                    (due != 0 && due < later).then_some(later)
                });
        }
    }
}

/// No allocator relief outside macOS, so input activity has nothing to postpone.
#[cfg(not(target_os = "macos"))]
pub(crate) fn note_activity() {}

#[cfg(target_os = "macos")]
struct Schedule {
    epoch: Instant,
    /// Zero means nothing to do; otherwise a deadline in milliseconds from epoch.
    due: AtomicU64,
}

#[cfg(target_os = "macos")]
impl Schedule {
    fn now(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    fn run(&self) {
        let mut allowed = 0;
        loop {
            let due = self.due.load(Ordering::Acquire);
            if due == 0 {
                thread::park();
                continue;
            }
            let wait = due.max(allowed).saturating_sub(self.now());
            if wait != 0 {
                thread::park_timeout(Duration::from_millis(wait));
            } else if self
                .due
                .compare_exchange(due, 0, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                release_free_pages();
                allowed = self.now() + COOLDOWN_MS;
            }
        }
    }
}

#[cfg(target_os = "macos")]
struct Worker {
    schedule: Arc<Schedule>,
    thread: Thread,
}

#[cfg(target_os = "macos")]
impl Worker {
    fn start() -> Option<Self> {
        let schedule = Arc::new(Schedule {
            epoch: Instant::now(),
            due: AtomicU64::new(0),
        });
        let shared = schedule.clone();
        let task = thread::Builder::new()
            .name("velocity-malloc-relief".into())
            .spawn(move || {
                // SAFETY: the Darwin selector changes only the current thread's priority.
                unsafe { libc::setpriority(libc::PRIO_DARWIN_THREAD, 0, libc::PRIO_DARWIN_BG) };
                shared.run();
            })
            .ok()?;
        Some(Self {
            schedule,
            thread: task.thread().clone(),
        })
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn release_free_pages() -> usize {
    unsafe extern "C" {
        fn malloc_zone_pressure_relief(zone: *mut libc::c_void, goal: usize) -> usize;
    }
    // SAFETY: NULL visits all malloc zones; a zero goal asks for all eligible
    // free pages. libmalloc synchronizes this with concurrent allocations.
    unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) }
}

/// Runs one allocator pass on a worker and returns Darwin's reported byte count.
/// For offline memory measurement only, never for interactive callers.
pub fn measure_relief() -> std::io::Result<usize> {
    #[cfg(target_os = "macos")]
    {
        thread::Builder::new()
            .name("velocity-measure-relief".into())
            .spawn(release_free_pages)?
            .join()
            .map_err(|_| std::io::Error::other("allocator relief worker panicked"))
    }
    #[cfg(not(target_os = "macos"))]
    Ok(0)
}
