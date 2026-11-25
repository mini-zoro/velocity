use super::*;
use std::{
    collections::HashMap,
    fmt,
    mem::size_of,
    sync::{Mutex, MutexGuard, OnceLock},
};

pub(super) const DEFAULT_BUFFER_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_PROCESS_BYTES: usize = 256 * 1024 * 1024;

/// A shared byte budget for live buffer histories. By default every buffer
/// uses the same process-wide budget.
#[derive(Clone)]
pub struct HistoryBudget(Arc<Mutex<Budget>>);

impl fmt::Debug for HistoryBudget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HistoryBudget").finish_non_exhaustive()
    }
}

impl HistoryBudget {
    /// Creates an independent budget for a group of buffers.
    pub fn new(bytes: usize) -> Self {
        Self(Arc::new(Mutex::new(Budget {
            limit: bytes,
            ..Budget::default()
        })))
    }

    /// The budget used by `Buffer::new` and session restore.
    pub fn process() -> Self {
        static PROCESS: OnceLock<HistoryBudget> = OnceLock::new();
        PROCESS
            .get_or_init(|| {
                Self::new(
                    configured_bytes("VELOCITY_HISTORY_PROCESS_BYTES")
                        .unwrap_or(DEFAULT_PROCESS_BYTES),
                )
            })
            .clone()
    }

    /// The configured aggregate limit, in bytes.
    pub fn limit_bytes(&self) -> usize {
        self.lock().limit
    }

    /// Accounted history bytes currently owned by the buffers in this budget.
    pub fn retained_bytes(&self) -> usize {
        self.lock().retained
    }

    pub(super) fn lock(&self) -> MutexGuard<'_, Budget> {
        self.0.lock().expect("history budget")
    }
}

pub(super) fn default_buffer_bytes() -> usize {
    buffer_bytes_override().unwrap_or(DEFAULT_BUFFER_BYTES)
}

pub(super) fn buffer_bytes_override() -> Option<usize> {
    static BYTES: OnceLock<Option<usize>> = OnceLock::new();
    *BYTES.get_or_init(|| configured_bytes("VELOCITY_HISTORY_BUFFER_BYTES"))
}

fn configured_bytes(key: &str) -> Option<usize> {
    std::env::var(key).ok()?.parse().ok()
}

// The budget owns the queues (not the buffers or their live views), so it can
// free another buffer's oldest history immediately, even if that buffer is idle.
#[derive(Default, Debug)]
pub(super) struct Budget {
    histories: BTreeMap<BufferId, History>,
    limit: usize,
    retained: usize,
    age: u64,
}

impl Budget {
    pub(super) fn get(&self, id: BufferId) -> &History {
        &self.histories[&id]
    }

    pub(super) fn limit(&self) -> usize {
        self.limit
    }

    pub(super) fn insert(&mut self, id: BufferId, history: History) {
        self.remove(id);
        self.retained += history.bytes();
        self.histories.insert(id, history);
        self.update(id, |_, _| ());
    }

    pub(super) fn remove(&mut self, id: BufferId) {
        if let Some(history) = self.histories.remove(&id) {
            self.retained -= history.bytes();
        }
    }

    pub(super) fn next_age(&mut self) -> u64 {
        self.age += 1;
        self.age
    }

    pub(super) fn update<R>(&mut self, id: BufferId, f: impl FnOnce(&mut History, u64) -> R) -> R {
        let age = self.next_age();
        let history = self.histories.get_mut(&id).expect("registered buffer");
        let before = history.bytes();
        let result = f(history, age);
        history.prune(self.limit);
        self.retained = self.retained - before + history.bytes();
        while self.retained > self.limit {
            let (id, kind) = self
                .histories
                .iter()
                .filter_map(|(&id, h)| h.oldest().map(|(age, kind)| (age, id, kind)))
                .min()
                .map(|(_, id, kind)| (id, kind))
                .expect("nonempty budget");
            let history = self.histories.get_mut(&id).expect("registered buffer");
            let before = history.bytes();
            history.evict(kind);
            self.retained -= before - history.bytes();
        }
        result
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Oldest {
    // Prefer to discard the map before its own edit when they have the same age.
    Map,
    Undo,
    Redo,
    Soft(ViewId, bool),
}

#[derive(Debug, Default)]
pub(super) struct History {
    pub(super) undo: VecDeque<Group>,
    pub(super) redo: VecDeque<Group>,
    pub(super) log: VecDeque<LoggedChange>,
    pub(super) soft: BTreeMap<ViewId, SoftHistory>,
    pub(super) limits: HistoryLimits,
    pub(super) entries: usize,
    usage: Usage,
    live: BTreeSet<u64>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct SoftHistory {
    pub(super) undo: VecDeque<SoftRecord>,
    pub(super) redo: VecDeque<SoftRecord>,
}

#[derive(Clone, Debug)]
pub(super) struct SoftRecord {
    pub(super) age: u64,
    pub(super) event: SoftEvent,
}

// Selection arrays and transaction maps can be referenced by several entries.
// Charge their allocated capacity once per buffer, until its last history
// reference goes away. Counting the references themselves is separate.
#[derive(Debug, Default)]
struct Usage {
    bytes: usize,
    shared: HashMap<usize, (usize, usize)>,
}

const SHARED_METADATA: usize = 4 * size_of::<usize>();

impl Usage {
    fn shared(&mut self, (key, size): (usize, usize), add: bool) {
        match self.shared.entry(key) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                debug_assert!(add);
                // Allow for the reference-count table entry as well.
                let size = size + SHARED_METADATA;
                entry.insert((1, size));
                self.bytes += size;
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let (count, bytes) = entry.get_mut();
                if add {
                    *count += 1;
                } else if *count > 1 {
                    *count -= 1;
                } else {
                    self.bytes -= *bytes;
                    entry.remove();
                }
            }
        }
    }

    fn fixed(&mut self, bytes: usize, add: bool) {
        if add {
            self.bytes += bytes;
        } else {
            self.bytes -= bytes;
        }
    }

    fn soft(&mut self, record: &SoftRecord, add: bool) {
        self.fixed(size_of::<SoftRecord>(), add);
        if let SoftEvent::Selection { before, after, .. } = &record.event {
            self.shared(before.history_allocation(), add);
            self.shared(after.history_allocation(), add);
        }
    }

    fn step(&mut self, step: &Step, add: bool) {
        self.fixed(step.bytes, add);
        self.shared(step.before_selection.history_allocation(), add);
        self.shared(step.after_selection.history_allocation(), add);
        self.shared(map_allocation(&step.changes), add);
    }
}

fn map_allocation(map: &Arc<ChangeSet>) -> (usize, usize) {
    (
        Arc::as_ptr(map) as usize,
        map.storage_bytes() + size_of::<ChangeSet>() + 2 * size_of::<usize>(),
    )
}

impl History {
    pub(super) fn bytes(&self) -> usize {
        self.usage.bytes
    }

    pub(super) fn maps_from(
        &self,
        from: ChangeId,
        current: ChangeId,
    ) -> Option<impl DoubleEndedIterator<Item = &LoggedChange> + ExactSizeIterator> {
        (from <= current
            && self
                .log
                .front()
                .map_or(from == current, |c| from.0 >= c.id.0 - 1))
        .then(|| self.log.range(self.log.partition_point(|c| c.id <= from)..))
    }

    pub(super) fn record_step(&mut self, step: &Step) {
        self.usage.step(step, true);
        self.live.insert(step.after_state);
        self.entries += 1;
    }

    pub(super) fn record_map(&mut self, item: LoggedChange) {
        self.usage.fixed(size_of::<LoggedChange>(), true);
        self.usage.shared(map_allocation(&item.map), true);
        self.log.push_back(item);
    }

    fn forget_map(&mut self) {
        let item = self.log.pop_front().expect("oldest map");
        self.usage.fixed(size_of::<LoggedChange>(), false);
        self.usage.shared(map_allocation(&item.map), false);
    }

    pub(super) fn record_soft(&mut self, id: ViewId, record: SoftRecord, redo: bool) {
        self.usage.soft(&record, true);
        let view = self.soft.entry(id).or_default();
        let queue = if redo { &mut view.redo } else { &mut view.undo };
        queue.push_back(record);
        while queue.len() > self.limits.selection_entries {
            self.usage
                .soft(&queue.pop_front().expect("excess soft record"), false);
        }
    }

    pub(super) fn record_selection(&mut self, id: ViewId, record: SoftRecord, limit: usize) {
        let SoftEvent::Selection { before, after, .. } = &record.event else {
            unreachable!("selection event");
        };
        let (before_key, before_bytes) = before.history_allocation();
        let (after_key, after_bytes) = after.history_allocation();
        let bytes = size_of::<SoftRecord>()
            + before_bytes
            + SHARED_METADATA
            + if before_key != after_key {
                after_bytes + SHARED_METADATA
            } else {
                0
            };
        // A selection does not change the text revision chain. It can be
        // skipped without evicting unrelated history to make room for it.
        if bytes <= limit.min(self.limits.bytes) {
            self.record_soft(id, record, false);
        }
    }

    pub(super) fn pop_soft(&mut self, id: ViewId, redo: bool) -> Option<SoftRecord> {
        let view = self.soft.get_mut(&id)?;
        let event = if redo { &mut view.redo } else { &mut view.undo }.pop_back()?;
        self.usage.soft(&event, false);
        Some(event)
    }

    pub(super) fn close_view(&mut self, id: ViewId) {
        if let Some(view) = self.soft.remove(&id) {
            for event in view.undo.into_iter().chain(view.redo) {
                self.usage.soft(&event, false);
            }
        }
    }

    pub(super) fn clear_soft_redo(&mut self, id: ViewId) {
        if let Some(view) = self.soft.get_mut(&id) {
            for event in view.redo.drain(..) {
                self.usage.soft(&event, false);
            }
        }
    }

    pub(super) fn set_limits(&mut self, limits: HistoryLimits) {
        self.limits = limits;
        for view in self.soft.values_mut() {
            for queue in [&mut view.undo, &mut view.redo] {
                while queue.len() > limits.selection_entries {
                    self.usage
                        .soft(&queue.pop_front().expect("excess soft record"), false);
                }
            }
        }
    }

    fn forget_group(&mut self, group: Group) {
        for step in group.steps {
            self.usage.step(&step, false);
            self.live.remove(&step.after_state);
            self.entries -= 1;
        }
    }

    fn forget_stale_edits(&mut self) {
        for view in self.soft.values_mut() {
            for queue in [&mut view.undo, &mut view.redo] {
                queue.retain(|record| {
                    if matches!(record.event, SoftEvent::Edit(state) if !self.live.contains(&state))
                    {
                        self.usage.soft(record, false);
                        false
                    } else {
                        true
                    }
                });
            }
        }
    }

    pub(super) fn clear_redo(&mut self) {
        let invalidated = !self.redo.is_empty();
        while let Some(group) = self.redo.pop_front() {
            self.forget_group(group);
        }
        for view in self.soft.values_mut() {
            for event in view.redo.drain(..) {
                self.usage.soft(&event, false);
            }
        }
        if invalidated {
            self.forget_stale_edits();
        }
    }

    fn prune(&mut self, process_limit: usize) {
        let mut evicted = false;
        while self.entries > self.limits.entries {
            let group = self
                .undo
                .pop_front()
                .or_else(|| self.redo.pop_front())
                .expect("excess edit entries");
            self.forget_group(group);
            evicted = true;
        }
        if evicted {
            self.forget_stale_edits();
        }
        while self.log.len() > self.limits.changes {
            self.forget_map();
        }
        while self.bytes() > self.limits.bytes.min(process_limit) {
            self.evict(self.oldest().expect("nonempty history").1);
        }
    }

    fn oldest(&self) -> Option<(u64, Oldest)> {
        let log = self.log.front().map(|item| (item.age, Oldest::Map));
        let undo = self.undo.front().map(|g| (g.steps[0].age, Oldest::Undo));
        // Redo stacks run backwards: evict from the far end so no remaining
        // text step depends on an evicted step.
        let redo = self.redo.front().map(|g| (g.steps[0].age, Oldest::Redo));
        log.into_iter()
            .chain(undo)
            .chain(redo)
            .chain(self.soft.iter().flat_map(|(&id, view)| {
                view.undo
                    .front()
                    .map(|r| (r.age, Oldest::Soft(id, false)))
                    .into_iter()
                    .chain(view.redo.front().map(|r| (r.age, Oldest::Soft(id, true))))
            }))
            .min()
    }

    fn evict(&mut self, kind: Oldest) {
        match kind {
            Oldest::Map => self.forget_map(),
            Oldest::Undo | Oldest::Redo => {
                let group = if kind == Oldest::Undo {
                    &mut self.undo
                } else {
                    &mut self.redo
                }
                .pop_front()
                .expect("oldest group");
                self.forget_group(group);
                self.forget_stale_edits();
            }
            Oldest::Soft(id, redo) => {
                let view = self.soft.get_mut(&id).expect("view with soft history");
                let record = if redo { &mut view.redo } else { &mut view.undo }
                    .pop_front()
                    .expect("oldest soft record");
                self.usage.soft(&record, false);
            }
        }
    }
}
