//! Bounded read-only restore jobs. A stalled filesystem cannot trap the UI forever.
use std::{
    sync::{Condvar, Mutex, mpsc},
    time::{Duration, Instant},
};
pub(crate) const BUDGET: Duration = Duration::from_secs(2);
pub(crate) const TIMEOUT: &str = "Document restore timed out";
static ACTIVE: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());
struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        let mut active = ACTIVE.0.lock().unwrap_or_else(|e| e.into_inner());
        *active -= 1;
        ACTIVE.1.notify_one();
    }
}
#[cfg(test)]
thread_local! { static BLOCK_NEXT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
#[cfg(test)]
pub(crate) fn block_next() {
    BLOCK_NEXT.with(|b| b.set(true));
}
pub(crate) fn run<T: Send + 'static>(
    deadline: Instant,
    read: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    {
        let active = ACTIVE.0.lock().unwrap_or_else(|e| e.into_inner());
        let (mut active, _) = ACTIVE
            .1
            .wait_timeout_while(
                active,
                deadline.saturating_duration_since(Instant::now()),
                |n| *n >= 4,
            )
            .unwrap_or_else(|e| e.into_inner());
        if *active >= 4 || Instant::now() >= deadline {
            return Err(format!("{TIMEOUT}: restore readers are busy"));
        }
        *active += 1;
    }
    let permit = Permit;
    let cancel = vel_largefile::Cancellation::default();
    let cancelled = cancel.clone();
    #[cfg(test)]
    let block = BLOCK_NEXT.with(|b| b.replace(false));
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("velocity-restore-read".into())
        .spawn(move || {
            let _permit = permit;
            #[cfg(test)]
            if block {
                while !cancelled.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(1));
                }
                return;
            }
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                vel_fileio::with_cancellation(cancelled.clone(), read)
            }))
            .unwrap_or_else(|_| Err("document restore reader failed".into()));
            if !cancelled.is_cancelled() {
                let _ = tx.send(result);
            }
        })
        .map_err(|e| e.to_string())?;
    let result = rx
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| TIMEOUT.to_owned());
    cancel.cancel();
    result?
}
