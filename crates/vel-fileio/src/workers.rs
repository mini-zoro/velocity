use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, Weak, mpsc},
    thread,
};

type Job = Box<dyn FnOnce() + Send>;
const MAX_PENDING_READS: usize = 32;
const DEFAULT_WORKERS: usize = 4;
/// Transient admission failure; an automatic reload can defer without closing its tab.
pub const IO_QUEUE_FULL: &str =
    "file I/O queue is full; try again after pending files load or close";

#[derive(Debug)]
pub(crate) struct PendingRead(Weak<Mutex<State>>, u64);

impl PendingRead {
    pub(crate) fn cancel(&self) {
        let Some(queue) = self.0.upgrade() else {
            return;
        };
        let removed = {
            let mut state = queue.lock().expect("file worker queue");
            let index = state.pending.iter().position(|(id, _)| *id == self.1);
            index.and_then(|i| state.pending.remove(i))
        };
        drop(removed);
    }
}

#[derive(Default)]
struct State {
    limit: usize,
    active: usize,
    next: u64,
    pending: VecDeque<(u64, Job)>,
    write: Option<Job>,
}

/// A window's bounded executor for background file reads and explicit writes.
#[derive(Clone, Default)]
pub struct IoWorkers(Arc<Mutex<State>>);

impl std::fmt::Debug for IoWorkers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IoWorkers").finish_non_exhaustive()
    }
}

impl IoWorkers {
    /// Queues a cancellable read without blocking the caller.
    pub(crate) fn submit(
        &self,
        limit: usize,
        name: &str,
        job: impl FnOnce() + Send + 'static,
    ) -> Result<PendingRead, String> {
        let id = self.schedule(limit, name, Box::new(job), false)?;
        Ok(PendingRead(Arc::downgrade(&self.0), id))
    }

    /// Waits for an explicit write; it takes priority over background jobs not yet started.
    pub fn write<T: Send + 'static>(
        &self,
        limit: usize,
        job: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, String> {
        let (send, receive) = mpsc::channel();
        self.schedule(
            limit,
            "velocity-file-write",
            Box::new(move || {
                let _ = send.send(job());
            }),
            true,
        )?;
        receive.recv().map_err(|e| e.to_string())
    }

    fn schedule(&self, limit: usize, name: &str, job: Job, priority: bool) -> Result<u64, String> {
        let builder = thread::Builder::new().name(name.into());
        let limit = if limit == 0 {
            DEFAULT_WORKERS
        } else {
            limit.min(64)
        };
        let shared = self.0.clone();
        let mut state = self.0.lock().expect("file worker queue");
        if (priority && state.write.is_some())
            || (!priority && state.pending.len() >= MAX_PENDING_READS)
        {
            return Err(IO_QUEUE_FULL.into());
        }
        state.limit = limit;
        if state.active < limit {
            builder
                .spawn(move || work(shared))
                .map_err(|e| e.to_string())?;
            state.active += 1;
        }
        let id = state.next;
        state.next += 1;
        if priority {
            state.write = Some(job);
        } else {
            state.pending.push_back((id, job));
        }
        Ok(id)
    }
}

fn work(queue: Arc<Mutex<State>>) {
    loop {
        let job = {
            let mut state = queue.lock().expect("file worker queue");
            let next = if state.active <= state.limit {
                state
                    .write
                    .take()
                    .or_else(|| state.pending.pop_front().map(|(_, job)| job))
            } else {
                None
            };
            let Some(job) = next else {
                state.active -= 1;
                return;
            };
            job
        };
        // Other queued requests must survive an unwinding I/O job.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn r35_worker_limit_controls_real_job_admission() {
        let pool = IoWorkers::default();
        let (done, finished) = mpsc::channel();
        let (stop, release) = mpsc::channel();
        let first = done.clone();
        pool.submit(1, "first", move || {
            first.send(1).unwrap();
            release.recv().unwrap();
        })
        .unwrap();
        // Hang guard only: the jobs run as soon as a slot is free.
        let timeout = Duration::from_secs(120);
        assert_eq!(finished.recv_timeout(timeout).unwrap(), 1);
        let second = done.clone();
        pool.submit(1, "second", move || second.send(2).unwrap())
            .unwrap();
        assert_eq!(pool.0.lock().unwrap().pending.len(), 1);
        assert!(finished.try_recv().is_err());
        pool.submit(2, "third", move || done.send(3).unwrap())
            .unwrap();
        // Both queued jobs can now take the second slot before the first releases.
        assert_eq!(finished.recv_timeout(timeout).unwrap(), 2);
        assert_eq!(finished.recv_timeout(timeout).unwrap(), 3);
        stop.send(()).unwrap();
        assert_eq!(pool.write(2, || 42).unwrap(), 42);
    }

    #[test]
    fn full_read_queue_recovers_capacity_when_a_tab_closes() {
        let pool = IoWorkers::default();
        let (started, running) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        pool.submit(1, "blocked-read", move || {
            started.send(()).unwrap();
            let _ = blocked.recv();
        })
        .unwrap();
        // Hang guards only: the jobs run as soon as a slot is free.
        running.recv_timeout(Duration::from_secs(120)).unwrap();
        let (did_run, cancelled) = mpsc::channel();
        let first = pool
            .submit(1, "closed-tab", move || did_run.send(()).unwrap())
            .unwrap();
        for _ in 1..MAX_PENDING_READS {
            pool.submit(1, "queued-read", || {}).unwrap();
        }
        assert!(pool.submit(1, "overflow", || {}).is_err());
        first.cancel();
        assert!(matches!(
            cancelled.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        let (finished, done) = mpsc::channel();
        pool.submit(1, "replacement", move || finished.send(()).unwrap())
            .unwrap();
        release.send(()).unwrap();
        done.recv_timeout(Duration::from_secs(120)).unwrap();
    }
}
