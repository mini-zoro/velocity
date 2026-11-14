//! Per-context timings are a separate instrumented run, never throughput samples.
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Instant,
};
use vel_regex::{Match, MatcherBackend, PatternMatcher, PatternSet, PatternSpec, SearchCache};

#[derive(Debug, Default)]
struct Counter {
    nanos: [AtomicU64; 5],
    calls: [AtomicU64; 5],
}
#[derive(Debug)]
pub struct Backend {
    inner: Arc<dyn MatcherBackend>,
    pub run: Arc<AtomicUsize>,
    counters: Mutex<BTreeMap<String, Arc<Counter>>>,
}
impl Backend {
    pub fn new(inner: Arc<dyn MatcherBackend>) -> Self {
        Self {
            inner,
            run: Arc::new(AtomicUsize::new(usize::MAX)),
            counters: Mutex::new(BTreeMap::new()),
        }
    }
    pub fn results(&self) -> serde_json::Value {
        let counters = self.counters.lock().unwrap();
        let rows: BTreeMap<_, _> = counters.iter().map(|(name, counter)| (name,
            serde_json::json!({"nanos":counter.nanos.each_ref().map(|n| n.load(Ordering::Relaxed)),
                "queries":counter.calls.each_ref().map(|n| n.load(Ordering::Relaxed))}))).collect();
        serde_json::json!(rows)
    }
}
impl MatcherBackend for Backend {
    fn build(&self, patterns: Vec<PatternSpec>) -> PatternSet {
        self.build_named(patterns, "<anonymous>")
    }
    fn build_named(&self, patterns: Vec<PatternSpec>, name: &str) -> PatternSet {
        let counter = self
            .counters
            .lock()
            .unwrap()
            .entry(name.into())
            .or_default()
            .clone();
        PatternSet::with_matcher(Arc::new(Matcher {
            inner: self.inner.build_named(patterns, name),
            counter,
            run: self.run.clone(),
        }))
    }
}
#[derive(Debug)]
struct Matcher {
    inner: PatternSet,
    counter: Arc<Counter>,
    run: Arc<AtomicUsize>,
}
impl PatternMatcher for Matcher {
    fn find_earliest(
        &self,
        line: &str,
        start: usize,
        cache: &mut SearchCache,
        excluded: &[usize],
    ) -> Option<Match> {
        let run = self.run.load(Ordering::Relaxed);
        if run >= 5 {
            return self
                .inner
                .find_earliest_excluding(line, start, cache, excluded);
        }
        let time = Instant::now();
        let result = self
            .inner
            .find_earliest_excluding(line, start, cache, excluded);
        self.counter.nanos[run].fetch_add(time.elapsed().as_nanos() as u64, Ordering::Relaxed);
        self.counter.calls[run].fetch_add(1, Ordering::Relaxed);
        result
    }
}
