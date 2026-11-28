//! One bounded URL cache and worker queue per application UI thread.
use std::{
    cell::RefCell,
    collections::{HashSet, VecDeque},
    sync::{Arc, mpsc},
};
use vel_render::scene::Image;

const MAX_ACTIVE: usize = 4;
const MAX_QUEUED: usize = 32;
const MAX_ENTRIES: usize = 128;
const MAX_BYTES: usize = 24 * 1024 * 1024;

type Loader = Arc<dyn Fn(&str) -> Option<Arc<Image>> + Send + Sync>;
type Reply = (String, Option<Arc<Image>>);

thread_local! {
    static REMOTE: RefCell<Option<Remote>> = const { RefCell::new(None) };
}

/// Enables HTTPS resolution for minihtml on the current UI thread. The loader
/// runs on workers and is responsible for validating, fetching and decoding.
pub fn enable(loader: impl Fn(&str) -> Option<Arc<Image>> + Send + Sync + 'static) {
    REMOTE.with_borrow_mut(|r| {
        r.get_or_insert_with(|| Remote::new(Arc::new(loader)));
    });
}

pub(super) fn resolve(url: &str, request: bool) -> (bool, Option<Arc<Image>>) {
    REMOTE.with_borrow_mut(|r| {
        r.as_mut()
            .map_or((false, None), |r| r.resolve(url, request))
    })
}

pub(crate) fn get(url: &str) -> Option<Arc<Image>> {
    resolve(url, false).1
}

pub(crate) fn pending() -> bool {
    REMOTE.with_borrow(|r| r.as_ref().is_some_and(|r| !r.active.is_empty()))
}

pub(crate) fn poll() -> bool {
    REMOTE.with_borrow_mut(|r| r.as_mut().is_some_and(Remote::poll))
}

pub(crate) fn generation() -> u64 {
    REMOTE.with_borrow(|r| r.as_ref().map_or(0, |r| r.generation))
}

pub(crate) fn request(url: &str) -> bool {
    resolve(url, true).0
}

struct Remote {
    loader: Loader,
    active: HashSet<String>,
    queue: VecDeque<String>,
    // Failed requests also occupy entries, so a broken image does not retry on every paint.
    ready: VecDeque<Reply>,
    bytes: usize,
    tx: mpsc::Sender<Reply>,
    rx: mpsc::Receiver<Reply>,
    generation: u64,
}

impl Remote {
    fn new(loader: Loader) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            loader,
            active: HashSet::new(),
            queue: VecDeque::new(),
            ready: VecDeque::new(),
            bytes: 0,
            tx,
            rx,
            generation: 1,
        }
    }

    fn resolve(&mut self, url: &str, request: bool) -> (bool, Option<Arc<Image>>) {
        if let Some(i) = self.ready.iter().position(|(s, _)| s == url) {
            let entry = self.ready.remove(i).unwrap();
            let image = entry.1.clone();
            self.ready.push_back(entry);
            return (true, image);
        }
        if request {
            if self.active.contains(url) || self.queue.iter().any(|s| s == url) {
                return (true, None);
            }
            if self.queue.len() < MAX_QUEUED {
                self.queue.push_back(url.into());
                self.start_queued();
                return (true, None);
            }
        }
        (false, None)
    }

    fn remember(&mut self, (url, image): Reply, max_bytes: usize) {
        let image = image.filter(|i| i.rgba.len() <= max_bytes);
        self.bytes += image.as_ref().map_or(0, |i| i.rgba.len());
        self.ready.push_back((url, image));
        while self.bytes > max_bytes || self.ready.len() > MAX_ENTRIES {
            let (_, evicted) = self.ready.pop_front().unwrap();
            self.bytes -= evicted.as_ref().map_or(0, |i| i.rgba.len());
        }
        self.generation = self.generation.wrapping_add(1);
    }

    fn start_queued(&mut self) {
        while self.active.len() < MAX_ACTIVE {
            let Some(url) = self.queue.pop_front() else {
                break;
            };
            let worker_url = url.clone();
            let loader = self.loader.clone();
            let tx = self.tx.clone();
            let worker = std::thread::Builder::new()
                .name("minihtml-https".into())
                .spawn(move || {
                    let image = loader(&worker_url);
                    let _ = tx.send((worker_url, image));
                });
            if worker.is_ok() {
                self.active.insert(url);
            } else {
                self.remember((url, None), MAX_BYTES);
            }
        }
    }

    fn poll(&mut self) -> bool {
        let before = self.generation;
        while let Ok(reply) = self.rx.try_recv() {
            self.active.remove(&reply.0);
            self.remember(reply, MAX_BYTES);
        }
        self.start_queued();
        before != self.generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    /// Hang guard: stops a wait that would otherwise never end. It is not a latency
    /// bound; the test checks the condition it waits for.
    const HANG_GUARD: std::time::Duration = std::time::Duration::from_secs(120);

    fn pixel(id: u64) -> Arc<Image> {
        Arc::new(Image {
            id,
            logical_size: [1., 1.],
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 255].into(),
        })
    }

    #[test]
    fn r35_remote_cache_enforces_weighted_lru_without_pinning_evictions() {
        let mut remote = Remote::new(Arc::new(|_| None));
        let a = pixel(1);
        let b = pixel(2);
        let weak_b = Arc::downgrade(&b);
        remote.remember(("a".into(), Some(a)), 8);
        remote.remember(("b".into(), Some(b)), 8);
        assert_eq!(remote.resolve("a", true).1.unwrap().id, 1);
        remote.remember(("c".into(), Some(pixel(3))), 8);
        assert_eq!(remote.bytes, 8);
        assert!(!remote.resolve("b", false).0);
        assert!(weak_b.upgrade().is_none());
        for n in 0..MAX_ENTRIES + 1 {
            remote.remember((format!("broken-{n}"), None), 8);
        }
        assert_eq!(remote.ready.len(), MAX_ENTRIES);
        assert_eq!(remote.bytes, 0);
        assert!(remote.resolve("broken-128", true).0);
        assert!(remote.active.is_empty());
    }

    #[test]
    fn r35_remote_requests_coalesce_and_bound_active_and_queued_jobs() {
        use std::sync::{Condvar, Mutex};
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let worker_gate = gate.clone();
        let (started, events) = mpsc::channel();
        let mut remote = Remote::new(Arc::new(move |url| {
            started.send(url.to_owned()).unwrap();
            let (released, condvar) = &*worker_gate;
            let _guard = condvar
                .wait_timeout_while(released.lock().unwrap(), Duration::from_secs(30), |go| !*go)
                .unwrap();
            None
        }));
        for i in 0..MAX_ACTIVE + MAX_QUEUED {
            let name = format!("image-{i}");
            assert!(remote.resolve(&name, true).0);
            assert!(remote.resolve(&name, true).0);
        }
        assert!(!remote.resolve("overflow", true).0);
        let mut running = HashSet::new();
        for _ in 0..MAX_ACTIVE {
            running.insert(events.recv_timeout(HANG_GUARD).unwrap());
        }
        assert_eq!(running.len(), MAX_ACTIVE);
        assert_eq!(remote.active.len(), MAX_ACTIVE);
        assert_eq!(remote.queue.len(), MAX_QUEUED);
        let (released, condvar) = &*gate;
        *released.lock().unwrap() = true;
        condvar.notify_all();
        let deadline = Instant::now() + HANG_GUARD;
        while !remote.active.is_empty() {
            remote.poll();
            assert!(remote.active.len() <= MAX_ACTIVE);
            assert!(Instant::now() < deadline, "image workers did not drain");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(remote.ready.len(), MAX_ACTIVE + MAX_QUEUED);
        assert_eq!(events.try_iter().count(), MAX_QUEUED);
    }
}
