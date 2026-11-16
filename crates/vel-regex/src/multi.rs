//! Lazy, bounded multi-pattern candidate machines. Onig remains the exact oracle.
use crate::{Match, MatcherBackend, OnigSet, PatternMatcher, PatternSet, PatternSpec, SearchCache};
use regex_automata::{
    Input, MatchKind,
    hybrid::{
        dfa::DFA,
        regex::{Cache, Regex as DfaRegex},
    },
    nfa::thompson::{NFA, WhichCaptures},
};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, TryLockError, Weak,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Instant,
};
static NEXT_SET: AtomicUsize = AtomicUsize::new(1);

/// Limits shared by all contexts in one backend (normally one syntax set).
#[derive(Debug, Clone)]
pub struct MultiDfaConfig {
    /// Compare initial warm queries and retain Onig for contexts without a clear gain.
    pub adaptive: bool,
    /// Assert full match/capture equality against a fresh Onig query (diagnostic mode).
    pub verify_matches: bool,
    /// Hard bound on resident candidate machines, including their search caches.
    pub memory_limit: usize,
    /// Maximum lazy transition cache per context. Oversized contexts use Onig.
    pub context_cache_limit: usize,
    /// Contexts below this rule count use Onig directly.
    pub minimum_patterns: usize,
}
impl Default for MultiDfaConfig {
    fn default() -> Self {
        Self {
            adaptive: false,
            verify_matches: false,
            memory_limit: 32 * 1024 * 1024,
            // Larger combined rule sets otherwise exhaust the NFA/transition
            // allowance and fall back to one native search per pattern. The
            // shared 32 MiB resident budget still bounds all contexts together.
            context_cache_limit: 1024 * 1024,
            minimum_patterns: 4,
        }
    }
}
/// Diagnostics for lazy compilation and the actual resident machine/cache budget.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct MultiDfaStats {
    /// Number of distinct pattern lists registered, including contexts using Onig.
    pub pattern_sets: usize,
    /// Number of first-use/recompilation attempts.
    pub compilations: usize,
    /// Actual resident machine and cache bytes (excluding Onig and source HIR).
    pub resident_bytes: usize,
    /// Largest observed resident byte count after budget enforcement.
    pub peak_bytes: usize,
    /// Compiled machines removed in LRU order.
    pub evictions: usize,
    /// ASCII line/context classifications performed.
    pub classifications: u64,
    /// Contexts where compilation or a DFA resource limit selected Onig.
    pub fallbacks: usize,
    /// Contexts selected for Onig after comparing 64 warm queries.
    pub adaptive_onig_sets: usize,
    /// First-use compile durations in milliseconds; at most 8192 observations.
    pub compile_ms: Vec<f64>,
}
/// Multi-pattern DFA backend with shared ordered sets and a global LRU budget.
///
/// A machine nominates all possible rules once per context/line. Only nominated
/// rules and unsupported patterns run in Oniguruma, in original priority order.
/// Unicode haystacks use Onig, preserving its exact word class and full folding.
/// No automaton/cache is held by parser checkpoints or per-line workspaces.
#[derive(Debug, Clone)]
pub struct MultiDfa(Arc<Shared>);
#[derive(Debug)]
struct Shared {
    config: MultiDfaConfig,
    pool: Mutex<Pool>,
}
type Key = Vec<(String, bool, usize)>;
#[derive(Debug, Default)]
struct Pool {
    sets: HashMap<Key, Weak<MultiSet>>,
    machines: HashMap<usize, Resident>,
    clock: u64,
    stats: MultiDfaStats,
}
impl Pool {
    fn evict_to(&mut self, target: usize) {
        while self.stats.resident_bytes > target {
            let Some((&victim, _)) = self
                .machines
                .iter()
                .filter(|(_, r)| r.machine.is_some())
                .min_by_key(|(_, r)| r.last_used)
            else {
                break;
            };
            if let Some(resident) = self.machines.remove(&victim) {
                self.stats.resident_bytes -= resident.machine.as_ref().map_or(0, Machine::bytes);
                self.stats.evictions += 1;
            }
        }
    }
}
#[derive(Debug)]
struct Resident {
    machine: Option<Machine>,
    last_used: u64,
    tuning: Tuning,
}
#[derive(Debug, Default)]
struct Tuning {
    queries: usize,
    multi_ns: u128,
    onig_ns: u128,
    line: Option<(usize, u64)>,
    cache: SearchCache,
}
#[derive(Debug)]
struct Machine {
    dfa: DfaRegex,
    cache: Cache,

    // DFA-local pattern IDs map back to the original context order.
    map: Vec<usize>,
    exact: Vec<bool>,
    literals: Option<(aho_corasick::AhoCorasick, Vec<usize>)>,
    fallback: Vec<usize>,
}
impl Machine {
    fn bytes(&self) -> usize {
        self.dfa.forward().memory_usage()
            + self.dfa.reverse().memory_usage()
            + self.cache.memory_usage()
            + (self.map.capacity() + self.fallback.capacity()) * size_of::<usize>()
            + self.exact.capacity()
            + self.literals.as_ref().map_or(0, |(ac, map)| {
                ac.memory_usage() + map.capacity() * size_of::<usize>()
            })
    }
    fn compile(patterns: &[PatternSpec], config: &MultiDfaConfig) -> Option<Self> {
        if patterns.len() < config.minimum_patterns || config.memory_limit == 0 {
            return None;
        }
        let mut hirs = Vec::new();
        let mut map = Vec::new();
        let mut fallback = Vec::new();
        let mut exact = Vec::new();
        for (i, pattern) in patterns.iter().enumerate() {
            if !pattern.regex.0.onig_semantics {
                fallback.push(i);
                continue;
            }
            match pattern
                .regex
                .0
                .candidate
                .get_or_init(|| crate::frontend::candidate(pattern.regex.regex_str()))
            {
                Some(candidate) => {
                    if !candidate.exact && candidate.hir.properties().maximum_len() == Some(0) {
                        fallback.push(i);
                        continue;
                    }
                    hirs.push(&candidate.hir);
                    map.push(i);
                    exact.push(candidate.exact);
                }
                None => fallback.push(i),
            }
        }
        if hirs.len() < config.minimum_patterns {
            return None;
        }
        let literals = hirs
            .iter()
            .enumerate()
            .map(|(i, hir)| crate::frontend::literals(hir).map(|ls| (i, ls)))
            .collect::<Option<Vec<_>>>()
            .and_then(|sets| {
                let mut map = Vec::new();
                let mut needles = Vec::new();
                for (i, literals) in sets {
                    for literal in literals {
                        needles.push(literal);
                        map.push(i);
                    }
                }
                aho_corasick::AhoCorasick::builder()
                    .match_kind(aho_corasick::MatchKind::LeftmostFirst)
                    .build(&needles)
                    .ok()
                    .map(|ac| (ac, map))
            });
        let nfa = NFA::compiler()
            .configure(
                NFA::config()
                    .which_captures(WhichCaptures::None)
                    .utf8(false)
                    .nfa_size_limit(Some(config.context_cache_limit)),
            )
            .build_many_from_hir(&hirs)
            .ok()?;
        let forward = DFA::builder()
            .configure(
                DFA::config()
                    .match_kind(MatchKind::LeftmostFirst)
                    .cache_capacity(config.context_cache_limit)
                    .minimum_cache_clear_count(Some(1))
                    .minimum_bytes_per_state(Some(10)),
            )
            .build_from_nfa(nfa)
            .ok()?;
        let reverse_nfa = NFA::compiler()
            .configure(
                NFA::config()
                    .which_captures(WhichCaptures::None)
                    .utf8(false)
                    .reverse(true)
                    .nfa_size_limit(Some(config.context_cache_limit)),
            )
            .build_many_from_hir(&hirs)
            .ok()?;
        let reverse = DFA::builder()
            .configure(
                DFA::config()
                    .match_kind(MatchKind::All)
                    .cache_capacity(config.context_cache_limit)
                    .minimum_cache_clear_count(Some(1))
                    .minimum_bytes_per_state(Some(10)),
            )
            .build_from_nfa(reverse_nfa)
            .ok()?;
        let dfa = DfaRegex::builder().build_from_dfas(forward, reverse);
        let cache = dfa.create_cache();
        Some(Self {
            dfa,
            cache,
            map,
            exact,
            literals,
            fallback,
        })
    }
}
impl Default for MultiDfa {
    fn default() -> Self {
        Self::new(MultiDfaConfig::default())
    }
}
impl MultiDfa {
    /// Creates an independent bounded pool; clones share its machines and limits.
    pub fn new(config: MultiDfaConfig) -> Self {
        Self(Arc::new(Shared {
            config,
            pool: Mutex::new(Pool::default()),
        }))
    }
    /// Returns a consistent snapshot of compilation and resident-memory counters.
    pub fn stats(&self) -> MultiDfaStats {
        let pool = self.0.pool.lock().unwrap_or_else(|e| e.into_inner());
        let mut stats = pool.stats.clone();
        stats.pattern_sets = pool.sets.len();
        stats
    }
}
impl MatcherBackend for MultiDfa {
    fn build(&self, patterns: Vec<PatternSpec>) -> PatternSet {
        if patterns.len() < self.0.config.minimum_patterns {
            return PatternSet::from_specs(patterns);
        }
        let key: Key = patterns
            .iter()
            .map(|p| {
                (
                    p.regex.regex_str().to_owned(),
                    p.allow_empty,
                    if p.regex.0.onig_semantics {
                        0
                    } else {
                        Arc::as_ptr(&p.regex.0) as usize
                    },
                )
            })
            .collect();
        let mut pool = match self.0.pool.try_lock() {
            Ok(pool) => pool,
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
            Err(TryLockError::WouldBlock) => {
                // Context registration must not wait behind another document's search.
                return PatternSet::with_matcher(Arc::new(MultiSet {
                    disabled: AtomicBool::new(patterns.len() < self.0.config.minimum_patterns),
                    id: NEXT_SET.fetch_add(1, Ordering::Relaxed),
                    patterns: OnigSet(patterns),
                    shared: self.0.clone(),
                }));
            }
        };
        if let Some(set) = pool.sets.get(&key).and_then(Weak::upgrade) {
            return PatternSet::with_matcher(set);
        }
        if pool.sets.len() >= 8192 {
            pool.sets.retain(|_, set| set.strong_count() > 0);
        }
        let disabled = patterns.len() < self.0.config.minimum_patterns;
        let set = Arc::new(MultiSet {
            disabled: AtomicBool::new(disabled),
            id: NEXT_SET.fetch_add(1, Ordering::Relaxed),
            patterns: OnigSet(patterns),
            shared: self.0.clone(),
        });
        pool.sets.insert(key, Arc::downgrade(&set));
        PatternSet::with_matcher(set)
    }
}
#[derive(Debug)]
struct MultiSet {
    id: usize,
    patterns: OnigSet,
    shared: Arc<Shared>,
    disabled: AtomicBool,
}
#[derive(Default)]
struct Candidate {
    generation: u64,
    from: usize,
    hit: Option<(usize, usize, usize)>,
}
type LineCandidates = HashMap<usize, Candidate, fnv::FnvBuildHasher>;
impl MultiSet {
    fn query(&self, line: &str, start: usize, cache: &mut SearchCache) -> Option<Match> {
        let id = self.id;
        let mut pool = match self.shared.pool.try_lock() {
            Ok(pool) => pool,
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
            Err(TryLockError::WouldBlock) => {
                return self.patterns.find_earliest(line, start, cache, &[]);
            }
        };
        pool.clock += 1;
        let clock = pool.clock;
        if !pool.machines.contains_key(&id) {
            let reservation = self
                .shared
                .config
                .context_cache_limit
                .saturating_mul(4)
                .saturating_add(self.patterns.0.len().saturating_mul(64))
                .saturating_add(4096);
            pool.evict_to(self.shared.config.memory_limit.saturating_sub(reservation));
            let start = Instant::now();
            let machine = Machine::compile(&self.patterns.0, &self.shared.config);
            let elapsed = start.elapsed().as_secs_f64() * 1000.;
            pool.stats.compilations += 1;
            if pool.stats.compile_ms.len() < 8192 {
                pool.stats.compile_ms.push(elapsed);
            }
            if machine.is_none() {
                pool.stats.fallbacks += 1;
                self.disabled.store(true, Ordering::Relaxed);
            }
            pool.stats.resident_bytes += machine.as_ref().map_or(0, Machine::bytes);
            pool.machines.insert(
                id,
                Resident {
                    machine,
                    last_used: clock,
                    tuning: Tuning::default(),
                },
            );
        }
        let resident = pool.machines.get_mut(&id).expect("inserted resident");
        resident.last_used = clock;
        let tuning = self.shared.config.adaptive
            && resident.machine.is_some()
            && resident.tuning.queries < 68;
        let measured = tuning.then(Instant::now);
        let before = resident.machine.as_ref().map_or(0, Machine::bytes);
        let result = if let Some(machine) = &mut resident.machine {
            let generation = cache.generation;
            let candidates = cache.backend_state(LineCandidates::default);
            if candidates.len() >= 4096 && !candidates.contains_key(&id) {
                candidates.clear();
            }
            let candidate = candidates.entry(id).or_default();
            let hit = if candidate.generation == generation
                && start >= candidate.from
                && candidate.hit.is_none_or(|(_, at, _)| at >= start)
            {
                Ok(candidate.hit)
            } else {
                if let Some((ac, map)) = &machine.literals {
                    Ok(ac
                        .find(aho_corasick::Input::new(line).span(start..line.len()))
                        .map(|hit| (map[hit.pattern().as_usize()], hit.start(), hit.end())))
                } else {
                    machine
                        .dfa
                        .try_search(
                            &mut machine.cache,
                            &Input::new(line).span(start..line.len()),
                        )
                        .map(|hit| {
                            hit.map(|hit| (hit.pattern().as_usize(), hit.start(), hit.end()))
                        })
                }
            };
            match hit {
                Ok(hit) => {
                    candidate.generation = generation;
                    candidate.from = start;
                    candidate.hit = hit;
                    let fast = hit.and_then(|(local, at, end)| {
                        let index = machine.map[local];
                        let candidate = self.patterns.0[index]
                            .regex
                            .0
                            .candidate
                            .get()
                            .and_then(Option::as_ref)
                            .expect("compiled candidate");
                        if machine.exact[local]
                            && (self.patterns.0[index].allow_empty
                                || (!candidate.zero_width && at != end))
                            && !(at == line.len() && line.ends_with('\n'))
                            && !(candidate.start_lf && end == line.len() && line.ends_with('\n'))
                        {
                            (self.patterns.0[index]
                                .regex
                                .compiled(self.patterns.0[index].allow_empty)
                                .is_ok()
                                && candidate.guards_match(line, at, end))
                            .then_some((
                                index,
                                at,
                                if candidate.zero_width { at } else { end },
                                at,
                                candidate.captures.as_ref().map(|_| candidate),
                            ))
                        } else {
                            self.patterns
                                .find_candidate_spans(line, at, cache, &[], std::iter::once(index))
                                .filter(|(_, span)| span.0 == at)
                                .map(|(index, (begin, end))| (index, begin, end, at, None))
                        }
                    });
                    if hit.is_some() && fast.is_none() {
                        // A guard or empty-match policy rejected the superset's winner.
                        // Resolve the query in original priority order in one pass.
                        self.patterns.find_earliest(line, start, cache, &[])
                    } else {
                        let fallback = self
                            .patterns
                            .find_candidate_spans(
                                line,
                                start,
                                cache,
                                &[],
                                machine.fallback.iter().copied().take_while(|index| {
                                    fast.as_ref()
                                        .is_none_or(|hit| hit.1 != start || *index < hit.0)
                                }),
                            )
                            .map(|(index, (begin, end))| (index, begin, end, start, None));
                        let winner = match (fast, fallback) {
                            (Some(a), Some(b)) => {
                                Some(if (a.1, a.0) <= (b.1, b.0) { a } else { b })
                            }
                            (a, b) => a.or(b),
                        };
                        winner.and_then(|(index, begin, end, from, layout)| {
                            if let Some(candidate) = layout {
                                let mut spans = smallvec::smallvec![Some((begin, end))];
                                for &(group, first, last) in
                                    candidate.captures.as_ref().expect("checked layout")
                                {
                                    spans.resize(spans.len().max(group as usize + 1), None);
                                    spans[group as usize] =
                                        Some((first.resolve(begin, end), last.resolve(begin, end)));
                                }
                                let slots = self.patterns.0[index]
                                    .regex
                                    .compiled(self.patterns.0[index].allow_empty)
                                    .as_ref()
                                    .ok()?
                                    .capture_slots()?;
                                // HIR may erase dead captures such as (a){0}. Native
                                // group metadata preserves their unmatched slots.
                                spans.resize(slots, None);
                                Some(Match {
                                    pattern: index,
                                    start: begin,
                                    end,
                                    captures: crate::Region { spans },
                                })
                            } else {
                                self.patterns.capture_match(line, from, index, (begin, end))
                            }
                        })
                    }
                }
                Err(_) => self.patterns.find_earliest(line, start, cache, &[]),
            }
        } else {
            self.patterns.find_earliest(line, start, cache, &[])
        };
        let after = resident.machine.as_ref().map_or(0, Machine::bytes);
        let mut selected_onig = false;
        if let Some(measured) = measured {
            let elapsed = measured.elapsed().as_nanos();
            let line_key = (cache as *const SearchCache as usize, cache.generation);
            if resident.tuning.line != Some(line_key) {
                resident.tuning.cache.reset_line();
                resident.tuning.line = Some(line_key);
            }
            let start_reference = Instant::now();
            std::hint::black_box(self.patterns.find_earliest(
                line,
                start,
                &mut resident.tuning.cache,
                &[],
            ));
            let reference_ns = start_reference.elapsed().as_nanos();
            if resident.tuning.queries >= 4 {
                resident.tuning.multi_ns += elapsed;
                resident.tuning.onig_ns += reference_ns;
            }
            resident.tuning.queries += 1;
            if resident.tuning.queries == 68
                && resident.tuning.multi_ns * 115 >= resident.tuning.onig_ns * 100
            {
                self.disabled.store(true, Ordering::Relaxed);
                selected_onig = true;
            }
        }
        if selected_onig {
            pool.stats.adaptive_onig_sets += 1;
        }
        pool.stats.classifications += 1;
        let mut bytes = pool.stats.resident_bytes - before + after;
        while bytes > self.shared.config.memory_limit {
            let Some((&victim, _)) = pool
                .machines
                .iter()
                .filter(|(_, r)| r.machine.is_some())
                .min_by_key(|(_, r)| r.last_used)
            else {
                break;
            };
            if let Some(resident) = pool.machines.remove(&victim) {
                bytes -= resident.machine.as_ref().map_or(0, Machine::bytes);
                pool.stats.evictions += 1;
            }
        }
        pool.stats.resident_bytes = bytes;
        pool.stats.peak_bytes = pool.stats.peak_bytes.max(bytes);
        result
    }
}
impl Drop for MultiSet {
    fn drop(&mut self) {
        let mut pool = match self.shared.pool.try_lock() {
            Ok(pool) => pool,
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
            // The unreachable machine remains inside the same LRU byte budget.
            Err(TryLockError::WouldBlock) => return,
        };
        if let Some(resident) = pool.machines.remove(&self.id) {
            pool.stats.resident_bytes -= resident.machine.as_ref().map_or(0, Machine::bytes);
        }
    }
}

impl PatternMatcher for MultiSet {
    fn find_earliest(
        &self,
        line: &str,
        start: usize,
        cache: &mut SearchCache,
        excluded: &[usize],
    ) -> Option<Match> {
        if start > line.len() || !line.is_char_boundary(start) {
            return None;
        }
        cache.bind(line);
        if self.disabled.load(Ordering::Relaxed) || !cache.ascii || !excluded.is_empty() {
            return self.patterns.find_earliest(line, start, cache, excluded);
        }
        let result = self.query(line, start, cache);
        if self.shared.config.verify_matches {
            let expected =
                self.patterns
                    .find_earliest(line, start, &mut SearchCache::default(), excluded);
            assert_eq!(
                result, expected,
                "candidate mismatch at {start} in {line:?}; patterns {:?}",
                self.patterns
            );
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn contention_falls_back_without_waiting_for_another_document() {
        let backend = MultiDfa::new(MultiDfaConfig {
            minimum_patterns: 1,
            ..MultiDfaConfig::default()
        });
        let patterns = || {
            vec![PatternSpec {
                regex: crate::Regex::new("abc|def".into()),
                allow_empty: false,
            }]
        };
        let existing = backend.build(patterns());
        let lock = backend.0.pool.lock().unwrap();
        let other = backend.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let registered = other.build(patterns());
            let mut cache = SearchCache::default();
            sender
                .send((
                    existing.find_earliest("abc", 0, &mut cache),
                    registered.find_earliest("def", 0, &mut cache),
                ))
                .unwrap();
        });
        let result = receiver.recv_timeout(std::time::Duration::from_secs(1));
        drop(lock);
        thread.join().unwrap();
        let (existing, registered) =
            result.expect("context registration/search waited for the busy pool");
        assert!(existing.is_some() && registered.is_some());
    }
}
