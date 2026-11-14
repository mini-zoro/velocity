//! Original subsequence matcher: filename, adjacency, word boundaries and recency.
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

/// Prepared candidate; normalization and character masks are paid once at indexing time.
#[derive(Clone, Debug)]
pub struct Candidate {
    /// User-facing caption/path.
    pub label: String,
    lower: String,
    name: usize,
    mask: u64,
    boundaries: Vec<bool>,
    /// Monotonic use counter. Zero means never opened.
    pub recent: u64,
}
fn mask(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0, |m, b| m | (1 << (b % 64)))
}
impl Candidate {
    /// Prepares a path or command caption.
    pub fn new(label: impl Into<String>) -> Self {
        let label = label.into();
        let lower = label.to_lowercase();
        let name = lower.rfind(['/', '\\']).map_or(0, |i| i + 1);
        let mask = mask(lower.as_bytes());
        let mut boundaries = vec![false; lower.len()];
        let mut offset = 0;
        let mut previous = None::<char>;
        for c in label.chars() {
            boundaries[offset] = previous
                .is_none_or(|p| !p.is_alphanumeric() || (p.is_lowercase() && c.is_uppercase()));
            offset += c.to_lowercase().map(char::len_utf8).sum::<usize>();
            previous = Some(c);
        }
        Self {
            label,
            lower,
            name,
            mask,
            boundaries,
            recent: 0,
        }
    }
}
/// Ranked hit with original UTF-8 byte positions for highlighted Unicode scalars.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// Index into the candidate snapshot.
    pub index: usize,
    /// Higher is better; ties retain index order.
    pub score: i64,
    /// Matched scalar positions in the original label.
    pub positions: Vec<usize>,
}
fn score_bytes(hay: &[u8], needle: &[u8], start: usize, boundaries: &[bool]) -> Option<i64> {
    let mut at = start;
    let mut score = 0;
    let mut previous = usize::MAX;
    for (qi, c) in needle.iter().enumerate() {
        let pos = hay[at..].iter().position(|b| b == c)? + at;
        score += 20;
        if previous.checked_add(1) == Some(pos) {
            score += 38;
        }
        if pos == start || boundaries[pos] {
            score += 32;
        }
        if qi == 0 {
            score -= (pos - start) as i64 * 2;
        } else {
            score -= (pos - at) as i64;
        }
        previous = pos;
        at = pos + 1;
    }
    Some(score)
}
fn positions(label: &str, query: &str) -> Vec<usize> {
    let mut query = query.chars().flat_map(char::to_lowercase).peekable();
    let mut out = Vec::new();
    for (i, c) in label.char_indices() {
        for lower in c.to_lowercase() {
            if query.peek() == Some(&lower) {
                out.push(i);
                query.next();
                if query.peek().is_none() {
                    return out;
                }
            }
        }
    }
    out
}
/// Reusable incremental matcher. Backspace/changed queries rescore the complete snapshot.
#[derive(Debug, Default)]
pub struct Matcher {
    previous: String,
    candidates: Vec<usize>,
}
impl Matcher {
    /// Clears narrowing after a snapshot/recency change.
    pub fn reset(&mut self) {
        self.previous.clear();
        self.candidates.clear();
    }
    /// Ranks at most `limit` items using partial selection, retaining *all* matches for narrowing.
    pub fn rank(&mut self, items: &[Candidate], query: &str, limit: usize) -> Vec<Hit> {
        self.rank_cancel(items, query, limit, || false)
            .unwrap_or_default()
    }
    fn rank_cancel(
        &mut self,
        items: &[Candidate],
        query: &str,
        limit: usize,
        cancel: impl Fn() -> bool,
    ) -> Option<Vec<Hit>> {
        let query = query.to_lowercase();
        let incremental = !self.previous.is_empty() && query.starts_with(&self.previous);
        let needle = query.as_bytes();
        let required = mask(needle);
        let newest = items.iter().map(|item| item.recent).max().unwrap_or(0);
        let mut matches = Vec::new();
        let count = if incremental {
            self.candidates.len()
        } else {
            items.len()
        };
        for n in 0..count {
            if n % 256 == 0 && cancel() {
                return None;
            }
            let i = if incremental { self.candidates[n] } else { n };
            let item = &items[i];
            if item.mask & required != required {
                continue;
            }
            let bytes = item.lower.as_bytes();
            let base = if needle.is_empty() {
                Some(0)
            } else {
                let stem_end = item.lower[item.name..]
                    .rfind('.')
                    .map_or(bytes.len(), |p| item.name + p);
                score_bytes(&bytes[..stem_end], needle, item.name, &item.boundaries)
                    .map(|s| s + 700)
                    .or_else(|| {
                        score_bytes(bytes, needle, item.name, &item.boundaries).map(|s| s + 500)
                    })
                    .or_else(|| score_bytes(bytes, needle, 0, &item.boundaries))
            };
            if let Some(mut score) = base {
                if !needle.is_ascii()
                    && positions(&item.label, &query).len() < query.chars().count()
                {
                    continue;
                }
                if !needle.is_empty() && item.lower[item.name..] == query {
                    score += 1000;
                }
                if !needle.is_empty() && item.lower[item.name..].starts_with(&query) {
                    score += 150;
                }
                score -= bytes.len() as i64;
                if item.recent != 0 {
                    score += 200 - newest.saturating_sub(item.recent).min(200) as i64;
                }
                matches.push((i, score));
            }
        }
        self.previous = query.clone();
        self.candidates = matches.iter().map(|(i, _)| *i).collect();
        let compare = |a: &(usize, i64), b: &(usize, i64)| b.1.cmp(&a.1).then(a.0.cmp(&b.0));
        if matches.len() > limit {
            matches.select_nth_unstable_by(limit, compare);
            matches.truncate(limit);
        }
        matches.sort_unstable_by(compare);
        Some(
            matches
                .into_iter()
                .map(|(index, score)| {
                    let item = &items[index];
                    let label = if !query.is_empty()
                        && score_bytes(item.lower.as_bytes(), needle, item.name, &item.boundaries)
                            .is_some()
                    {
                        // Normalized name offsets can differ from original offsets for Unicode case folding.
                        item.label.rfind(['/', '\\']).map_or(0, |i| i + 1)
                    } else {
                        0
                    };
                    Hit {
                        index,
                        score,
                        positions: positions(&item.label[label..], &query)
                            .into_iter()
                            .map(|p| p + label)
                            .collect(),
                    }
                })
                .collect(),
        )
    }
}
/// Completed worker result, tagged to prevent stale-query publication.
#[derive(Debug)]
pub struct Ranked {
    /// Submitted generation.
    pub generation: u64,
    /// End-to-end worker queue/scoring time.
    pub elapsed: Duration,
    /// Current top K.
    pub hits: Vec<Hit>,
}
#[derive(Debug)]
enum Work {
    Items(Arc<Vec<Candidate>>),
    Query(u64, String, usize, Instant),
    Stop,
}
/// One scoring worker; new queries cancel stale work and queued queries are coalesced.
#[derive(Debug)]
pub struct Worker {
    tx: mpsc::Sender<Work>,
    rx: mpsc::Receiver<Ranked>,
    generation: Arc<AtomicU64>,
    join: Option<thread::JoinHandle<()>>,
}
impl Default for Worker {
    fn default() -> Self {
        Self::new()
    }
}
impl Worker {
    /// Starts an idle device-free worker.
    pub fn new() -> Self {
        let (tx, requests) = mpsc::channel();
        let (results, rx) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let current = generation.clone();
        let join = thread::spawn(move || {
            let mut items = Arc::new(Vec::new());
            let mut matcher = Matcher::default();
            while let Ok(work) = requests.recv() {
                match work {
                    Work::Stop => break,
                    Work::Items(next) => {
                        items = next;
                        matcher.reset();
                    }
                    Work::Query(id, query, limit, at) => {
                        if current.load(Ordering::Acquire) != id {
                            continue;
                        }
                        if let Some(hits) = matcher.rank_cancel(&items, &query, limit, || {
                            current.load(Ordering::Relaxed) != id
                        }) && current.load(Ordering::Acquire) == id
                        {
                            let _ = results.send(Ranked {
                                generation: id,
                                elapsed: at.elapsed(),
                                hits,
                            });
                        }
                    }
                }
            }
        });
        Self {
            tx,
            rx,
            generation,
            join: Some(join),
        }
    }
    /// Atomically invalidates old queries and supplies a new index.
    pub fn set_items(&self, items: Arc<Vec<Candidate>>) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let _ = self.tx.send(Work::Items(items));
    }
    /// Returns a monotonically increasing query token.
    pub fn query(&self, query: impl Into<String>, limit: usize) -> u64 {
        let id = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let _ = self
            .tx
            .send(Work::Query(id, query.into(), limit, Instant::now()));
        id
    }
    /// Invalidates in-flight results while retaining the last completed narrowing set.
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
    /// Drains old results; only the newest generation can escape.
    pub fn poll(&self) -> Option<Ranked> {
        self.rx
            .try_iter()
            .filter(|r| r.generation == self.generation.load(Ordering::Acquire))
            .last()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let _ = self.tx.send(Work::Stop);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranking_goldens_narrowing_backspace_and_unicode() {
        let items: Vec<_> = [
            "src/main.rs",
            "tests/main_test.rs",
            "docs/main/guide.md",
            "src/market.rs",
            "src/MapReader.rs",
            "src/你好.rs",
            "main.rs",
        ]
        .into_iter()
        .map(Candidate::new)
        .collect();
        let mut matcher = Matcher::default();
        for (q, top) in [
            ("main", [6, 0, 1]),
            ("mr", [4, 3, 6]),
            ("你好", [5, usize::MAX, usize::MAX]),
        ] {
            let actual: Vec<_> = matcher.rank(&items, q, 3).iter().map(|h| h.index).collect();
            let expected: Vec<_> = top.into_iter().filter(|i| *i != usize::MAX).collect();
            assert_eq!(actual, expected, "{q}");
        }
        for q in ["m", "ma", "mai", "main", "ma", "rs", ""] {
            assert_eq!(
                matcher.rank(&items, q, 3),
                Matcher::default().rank(&items, q, 3)
            );
        }
        let mut recent = vec![Candidate::new("aa.rs"), Candidate::new("ab.rs")];
        recent[1].recent = 10;
        assert_eq!(matcher.rank(&recent, "", 1)[0].index, 1);
    }
    #[test]
    fn stale_worker_queries_never_publish() {
        let worker = Worker::new();
        worker.set_items(Arc::new(
            (0..100_000)
                .map(|i| Candidate::new(format!("src/{i}/main.rs")))
                .collect(),
        ));
        worker.query("m", 20);
        let last = worker.query("unmatchable", 20);
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(result) = worker.poll() {
                assert_eq!(result.generation, last);
                assert!(result.hits.is_empty());
                break;
            }
            assert!(Instant::now() < until);
            thread::yield_now();
        }
    }
}
