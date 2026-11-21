use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, mpsc},
    thread,
};

type Job = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct State {
    limit: usize,
    active: usize,
    pending: VecDeque<Job>,
}

/// A window's optionally bounded executor for file reads and writes.
#[derive(Clone, Default)]
pub struct IoWorkers(Arc<Mutex<State>>);

impl std::fmt::Debug for IoWorkers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IoWorkers").finish_non_exhaustive()
    }
}

impl IoWorkers {
    /// Queues work without blocking the caller. Zero keeps standalone worker scheduling.
    pub fn submit(
        &self,
        limit: usize,
        name: &str,
        job: impl FnOnce() + Send + 'static,
    ) -> Result<(), String> {
        self.schedule(limit, name, Box::new(job), false)
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

    fn schedule(&self, limit: usize, name: &str, job: Job, priority: bool) -> Result<(), String> {
        let builder = thread::Builder::new().name(name.into());
        if limit == 0 {
            return builder.spawn(job).map(|_| ()).map_err(|e| e.to_string());
        }
        let limit = limit.min(64);
        let shared = self.0.clone();
        let mut state = self.0.lock().expect("file worker queue");
        state.limit = limit;
        if state.active < limit {
            builder
                .spawn(move || work(shared))
                .map_err(|e| e.to_string())?;
            state.active += 1;
        }
        if priority {
            state.pending.push_front(job);
        } else {
            state.pending.push_back(job);
        }
        Ok(())
    }
}

fn work(queue: Arc<Mutex<State>>) {
    loop {
        let job = {
            let mut state = queue.lock().expect("file worker queue");
            let next = if state.active <= state.limit {
                state.pending.pop_front()
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
        let timeout = Duration::from_secs(5);
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
}
