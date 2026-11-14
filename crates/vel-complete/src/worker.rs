use crate::{CompletionEngine, CompletionList, ViewSnapshot};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
};
use vel_highlight::ScopeProvider;
struct Request {
    generation: u64,
    view: ViewSnapshot,
    carets: Vec<usize>,
    scopes: Arc<dyn ScopeProvider>,
}
/// Latest-query-wins completion worker, with cancellation before/inside source scans and at poll.
/// Dropping/cancelling does not join a potentially active background query.
#[derive(Debug)]
pub struct CompletionService {
    tx: mpsc::Sender<Request>,
    rx: mpsc::Receiver<CompletionList>,
    generation: Arc<AtomicU64>,
}
impl CompletionService {
    /// Starts one reusable worker over an immutable set of prepared sources.
    pub fn new(engine: Arc<CompletionEngine>) -> std::io::Result<Self> {
        let (tx, requests) = mpsc::channel::<Request>();
        let (results, rx) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let current = generation.clone();
        thread::Builder::new()
            .name("velocity-complete".into())
            .spawn(move || {
                while let Ok(request) = requests.recv() {
                    if current.load(Ordering::Acquire) != request.generation {
                        continue;
                    }
                    let mut list = engine.query_cancel(
                        &request.view,
                        &request.carets,
                        &*request.scopes,
                        || current.load(Ordering::Relaxed) != request.generation,
                    );
                    list.generation = request.generation;
                    if !list.cancelled
                        && current.load(Ordering::Acquire) == request.generation
                        && results.send(list).is_err()
                    {
                        break;
                    }
                }
            })?;
        Ok(Self { tx, rx, generation })
    }
    /// Submits immutable view/scopes; source collection never occurs on the caller thread.
    pub fn query(
        &self,
        view: ViewSnapshot,
        carets: Vec<usize>,
        scopes: Arc<dyn ScopeProvider>,
    ) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let _ = self.tx.send(Request {
            generation,
            view,
            carets,
            scopes,
        });
        generation
    }
    /// Invalidates queued/in-flight and already-published results.
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
    /// Returns only the newest generation; UI must additionally compare view/buffer versions.
    pub fn poll(&self) -> Option<CompletionList> {
        self.rx
            .try_iter()
            .filter(|result| result.generation == self.generation.load(Ordering::Acquire))
            .last()
    }
}
impl Drop for CompletionService {
    fn drop(&mut self) {
        self.cancel();
    }
}
