//! Owned history capture and compact, checked persistence. No changes to transaction storage.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::io::{Read, Write};

const VERSION: u32 = 1;
const MAX_PAYLOAD: usize = 512 * 1024 * 1024;

/// Invalid, incompatible or mismatched persisted history. Import never mutates a live buffer.
#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    /// Snapshot format is not supported by this build.
    #[error("unsupported history version {0}")]
    Version(u32),
    /// The supplied text differs from the snapshot's current text.
    #[error("history text length or BLAKE3 digest mismatch")]
    TextMismatch,
    /// A view mapping is missing, duplicated or invalid.
    #[error("invalid history view mapping")]
    ViewMap,
    /// The payload contains invalid coordinates, identities, references or encoding.
    #[error("invalid history: {0}")]
    Invalid(String),
}
fn invalid(s: impl ToString) -> HistoryError {
    HistoryError::Invalid(s.to_string())
}

/// Owned, versioned history. Capture shares rope roots, steps and selection arrays.
/// Serialize on a worker: it extracts only edit deltas, packs varints and compresses them.
#[derive(Debug, Clone)]
pub struct HistorySnapshot {
    repr: Repr,
}
#[derive(Debug, Clone)]
enum Repr {
    Live(Arc<Capture>),
    Packed(Wire),
}
// An owned export is independent of the live buffer and its budget registration.
#[derive(Debug)]
struct Capture {
    text: Text,
    views: BTreeMap<ViewId, ViewState>,
    undo: VecDeque<Group>,
    redo: VecDeque<Group>,
    soft: BTreeMap<ViewId, super::history_budget::SoftHistory>,
    log: VecDeque<LoggedChange>,
    change: ChangeId,
    limits: HistoryLimits,
    state: u64,
    next_state: u64,
    saved: u64,
    group_open: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Wire {
    version: u32,
    text_len: usize,
    text_hash: String,
    views: Vec<ViewId>,
    // Base64 avoids JSON's per-byte array overhead. Binary serde formats also work.
    data: String,
}
#[derive(Serialize, Deserialize)]
struct Body {
    limits: HistoryLimits,
    state: u64,
    next_state: u64,
    saved: u64,
    group_open: bool,
    steps: Vec<SavedStep>,
    undo: Vec<SavedGroup>,
    redo: Vec<SavedGroup>,
    views: Vec<SavedView>,
    // Ages relative to export's current ChangeId, so process-local IDs never escape.
    log: Vec<(u64, usize, Vec<Change>)>,
}
#[derive(Serialize, Deserialize)]
struct SavedStep {
    owner: ViewId,
    before_state: u64,
    after_state: u64,
    before_selection: Selection,
    after_selection: Selection,
    changes: Vec<Change>,
    removed: Vec<String>,
    inserted: Vec<String>,
}
#[derive(Serialize, Deserialize)]
struct SavedGroup {
    steps: Vec<usize>,
    run: Option<Run>,
    has_word: bool,
}
#[derive(Serialize, Deserialize)]
enum SavedEvent {
    Selection(Selection, Selection, u64, u64),
    Edit(usize),
}
#[derive(Serialize, Deserialize)]
struct SavedView {
    id: ViewId,
    selection: Selection,
    settings: Settings,
    regions: BTreeMap<String, RegionSet>,
    soft: Vec<SavedEvent>,
    soft_redo: Vec<SavedEvent>,
}
impl HistorySnapshot {
    /// Schema version; currently 1.
    pub fn version(&self) -> u32 {
        match &self.repr {
            Repr::Live(_) => VERSION,
            Repr::Packed(w) => w.version,
        }
    }
    /// Attached old view IDs to map to fresh process-local identities on restore.
    pub fn view_ids(&self) -> Vec<ViewId> {
        match &self.repr {
            Repr::Live(b) => b.views.keys().copied().collect(),
            Repr::Packed(w) => w.views.clone(),
        }
    }
    fn wire(&self) -> Result<Wire, HistoryError> {
        let Repr::Live(b) = &self.repr else {
            let Repr::Packed(w) = &self.repr else {
                unreachable!()
            };
            return Ok(w.clone());
        };
        let steps: Vec<_> = b
            .undo
            .iter()
            .chain(b.redo.iter().rev())
            .flat_map(|g| g.steps.iter())
            .collect();
        let indices: BTreeMap<_, _> = steps
            .iter()
            .enumerate()
            .map(|(i, s)| (s.after_state, i))
            .collect();
        let event = |e: &SoftEvent| -> SavedEvent {
            match e {
                SoftEvent::Selection {
                    before,
                    after,
                    at,
                    state,
                } => {
                    SavedEvent::Selection(before.clone(), after.clone(), b.change.0 - at.0, *state)
                }
                SoftEvent::Edit(state) => SavedEvent::Edit(indices[state]),
            }
        };
        let group = |g: &Group| SavedGroup {
            steps: g.steps.iter().map(|s| indices[&s.after_state]).collect(),
            run: g.run,
            has_word: g.has_word,
        };
        let mut body = Body {
            limits: b.limits,
            state: b.state,
            next_state: b.next_state,
            saved: b.saved,
            group_open: b.group_open,
            steps: steps
                .iter()
                .map(|s| {
                    let inverse = s.changes.inverse();
                    SavedStep {
                        owner: s.owner,
                        before_state: s.before_state,
                        after_state: s.after_state,
                        before_selection: s.before_selection.clone(),
                        after_selection: s.after_selection.clone(),
                        changes: s.changes.changes().to_vec(),
                        removed: s
                            .changes
                            .changes()
                            .iter()
                            .map(|c| {
                                s.before.as_ref().map_or_else(String::new, |t| {
                                    t.slice(c.old_range.clone()).to_string()
                                })
                            })
                            .collect(),
                        inserted: inverse
                            .changes()
                            .iter()
                            .map(|c| {
                                s.inserted.as_ref().map_or_else(
                                    || {
                                        s.after
                                            .as_ref()
                                            .expect("snapshot")
                                            .slice(c.old_range.clone())
                                            .to_string()
                                    },
                                    |t| t.to_string(),
                                )
                            })
                            .collect(),
                    }
                })
                .collect(),
            undo: b.undo.iter().map(group).collect(),
            redo: b.redo.iter().map(group).collect(),
            views: b
                .views
                .values()
                .map(|v| {
                    let soft = b.soft.get(&v.id);
                    SavedView {
                        id: v.id,
                        selection: v.selection.clone(),
                        settings: v.settings.clone(),
                        regions: v.regions.clone(),
                        soft: soft
                            .into_iter()
                            .flat_map(|s| &s.undo)
                            .map(|r| event(&r.event))
                            .collect(),
                        soft_redo: soft
                            .into_iter()
                            .flat_map(|s| &s.redo)
                            .map(|r| event(&r.event))
                            .collect(),
                    }
                })
                .collect(),
            log: b
                .log
                .iter()
                .map(|c| (b.change.0 - c.id.0, c.old_len, c.map.changes().to_vec()))
                .collect(),
        };
        body.delta_code(false);
        let raw = postcard::to_allocvec(&body).map_err(invalid)?;
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&raw).map_err(invalid)?;
        Ok(Wire {
            version: VERSION,
            text_len: b.text.len_bytes(),
            text_hash: digest(&b.text),
            views: b.views.keys().copied().collect(),
            data: STANDARD.encode(encoder.finish().map_err(invalid)?),
        })
    }
}
impl Serialize for HistorySnapshot {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.wire()
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for HistorySnapshot {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self {
            repr: Repr::Packed(Wire::deserialize(deserializer)?),
        })
    }
}
fn digest(text: &Text) -> String {
    let mut hash = blake3::Hasher::new();
    for chunk in text.chunks() {
        hash.update(chunk.as_bytes());
    }
    hash.finalize().to_hex().to_string()
}
impl ViewId {
    /// Reserves a fresh process-local view identity for a history import map.
    pub fn fresh() -> Self {
        Self(NEXT_VIEW.fetch_add(1, Ordering::Relaxed))
    }
}
impl Buffer {
    /// Captures owned history without copying document text or extracting edit strings.
    /// Serialization is deliberately deferred, allowing Hot Exit to encode on its writer.
    pub fn export_history(&self) -> HistorySnapshot {
        let budget = self.budget.lock();
        let history = budget.get(self.id);
        HistorySnapshot {
            repr: Repr::Live(Arc::new(Capture {
                text: self.text.clone(),
                views: self.views.clone(),
                undo: history.undo.clone(),
                redo: history.redo.clone(),
                soft: history.soft.clone(),
                log: history.log.clone(),
                change: self.change,
                limits: history.limits,
                state: self.state,
                next_state: self.next_state,
                saved: self.saved,
                group_open: self.group_open,
            })),
        }
    }
    /// Restores into a fresh buffer and attaches mapped views. The map must contain exactly
    /// the snapshot's attached views, with unique nonzero targets reserved with `ViewId::fresh`.
    /// Closed historical owners receive fresh hidden identities, preserving text undo.
    /// Change IDs are rebased; old async results must not be reused across process restarts.
    pub fn restore_history(
        text: impl Into<Text>,
        snapshot: HistorySnapshot,
        view_id_map: &BTreeMap<ViewId, ViewId>,
    ) -> Result<Self, HistoryError> {
        let wire = snapshot.wire()?;
        if wire.version != VERSION {
            return Err(HistoryError::Version(wire.version));
        }
        let text = text.into();
        if wire.text_len != text.len_bytes() || wire.text_hash != digest(&text) {
            return Err(HistoryError::TextMismatch);
        }
        let ids: BTreeSet<_> = wire.views.iter().copied().collect();
        let targets: BTreeSet<_> = view_id_map.values().copied().collect();
        if ids.len() != wire.views.len()
            || ids != view_id_map.keys().copied().collect()
            || targets.len() != ids.len()
            || targets.contains(&ViewId(0))
            || targets.contains(&ViewId(u64::MAX))
        {
            return Err(HistoryError::ViewMap);
        }
        let packed = STANDARD.decode(&wire.data).map_err(invalid)?;
        let mut raw = Vec::new();
        ZlibDecoder::new(packed.as_slice())
            .take(MAX_PAYLOAD as u64 + 1)
            .read_to_end(&mut raw)
            .map_err(invalid)?;
        if raw.len() > MAX_PAYLOAD {
            return Err(invalid("payload limit"));
        }
        let mut body: Body = postcard::from_bytes(&raw).map_err(invalid)?;
        body.delta_code(true);
        let Body {
            mut limits,
            state,
            next_state,
            saved,
            group_open,
            steps,
            undo,
            redo,
            views,
            log,
        } = body;
        if limits.selection_entries < 256
            || state >= next_state
            || saved >= next_state
            || steps.len() > limits.entries
            || views.len() != ids.len()
            || log.len() > limits.changes
        {
            return Err(invalid("limits or revision identities"));
        }
        // Keep the version-1 payload compatible. A launch override takes
        // precedence; an older session cannot raise the default cap.
        limits.bytes = history_budget::buffer_bytes_override()
            .unwrap_or_else(|| limits.bytes.min(default_buffer_bytes()));
        let budget = HistoryBudget::process();
        let stamp = budget.lock().next_age();
        let mut used = BTreeSet::new();
        let mut order = Vec::new();
        let cursor = undo.iter().map(|g| g.steps.len()).sum::<usize>();
        for g in undo.iter().chain(redo.iter().rev()) {
            if g.steps.is_empty() {
                return Err(invalid("empty undo group"));
            }
            for &i in &g.steps {
                if i >= steps.len() || !used.insert(i) {
                    return Err(invalid("duplicate or missing step"));
                }
                order.push(i);
            }
        }
        if used.len() != steps.len() {
            return Err(invalid("unreferenced step"));
        }
        if let Some(maximum) = targets.last() {
            NEXT_VIEW.fetch_max(maximum.0 + 1, Ordering::Relaxed);
        }
        let wanted_states: BTreeSet<_> = views
            .iter()
            .flat_map(|v| v.soft.iter().chain(&v.soft_redo))
            .filter_map(|e| {
                if let SavedEvent::Selection(_, _, _, state) = e {
                    Some(*state)
                } else {
                    None
                }
            })
            .collect();
        let mut known_texts = BTreeMap::new();
        known_texts.insert(state, text.clone());
        let mut owners = view_id_map.clone();
        let mut changes = Vec::with_capacity(steps.len());
        let mut states = BTreeSet::new();
        for s in &steps {
            if s.owner == ViewId(0)
                || s.before_state >= s.after_state
                || s.after_state >= next_state
                || !states.insert(s.after_state)
                || s.changes.len() != s.removed.len()
                || s.changes.len() != s.inserted.len()
            {
                return Err(invalid("step identities or edits"));
            }
            owners.entry(s.owner).or_insert_with(ViewId::fresh);
            changes.push(checked_map(&s.changes, None)?);
        }
        let mut root = text.clone();
        for &i in order[..cursor].iter().rev() {
            apply_checked(
                &mut root,
                &changes[i].inverse(),
                &steps[i].inserted,
                &steps[i].removed,
            )?;
        }
        let mut built = Vec::with_capacity(steps.len());
        built.resize_with(steps.len(), || None);
        let mut revision = order.first().map_or(state, |&i| steps[i].before_state);
        for (position, &i) in order.iter().enumerate() {
            let s = &steps[i];
            if s.before_state != revision {
                return Err(invalid("disconnected revision chain"));
            }
            if position == cursor && (revision != state || root != text) {
                return Err(invalid("undo cursor"));
            }
            if wanted_states.contains(&s.before_state) {
                known_texts.insert(s.before_state, root.clone());
            }
            selection_ok(&s.before_selection, &root)?;
            let before = root.clone();
            apply_checked(&mut root, &changes[i], &s.removed, &s.inserted)?;
            selection_ok(&s.after_selection, &root)?;
            if wanted_states.contains(&s.after_state) {
                known_texts.insert(s.after_state, root.clone());
            }
            revision = s.after_state;
            let inserted = s
                .inserted
                .first()
                .filter(|first| {
                    s.changes.iter().all(|c| c.old_range.is_empty())
                        && s.inserted.iter().all(|v| v == *first)
                })
                .map(|v| Arc::<str>::from(v.as_str()));
            let retained_text = inserted.as_ref().map_or_else(
                || {
                    before
                        .estimated_retained_bytes(&root)
                        .max(root.estimated_retained_bytes(&before))
                },
                |v| v.len() + 2 * std::mem::size_of::<usize>(),
            );
            built[i] = Some(Arc::new(Step {
                before: inserted.is_none().then_some(before),
                after: inserted.is_none().then(|| root.clone()),
                inserted,
                before_selection: s.before_selection.clone(),
                after_selection: s.after_selection.clone(),
                owner: owners[&s.owner],
                changes: Arc::new(changes[i].clone()),
                before_state: s.before_state,
                after_state: s.after_state,
                age: stamp,
                bytes: Step::fixed_bytes(retained_text),
            }));
        }
        if cursor == order.len() && (revision != state || root != text) {
            return Err(invalid("current revision"));
        }
        let built: Vec<_> = built.into_iter().map(Option::unwrap).collect();
        let group = |g: SavedGroup| Group {
            steps: g.steps.into_iter().map(|i| built[i].clone()).collect(),
            run: g.run,
            has_word: g.has_word,
        };
        let mut b = Buffer::with_history_budget(text, budget);
        let mut h = History::default();
        h.limits = limits;
        b.state = state;
        b.next_state = next_state;
        b.saved = saved;
        b.group_open = group_open;
        h.undo = undo.into_iter().map(group).collect();
        h.redo = redo.into_iter().map(group).collect();
        for step in &built {
            h.record_step(step);
        }
        // Keep all stored ages (including stale soft selections) without underflow.
        let age = log
            .iter()
            .map(|l| l.0)
            .chain(
                views
                    .iter()
                    .flat_map(|v| v.soft.iter().chain(&v.soft_redo))
                    .filter_map(|e| {
                        if let SavedEvent::Selection(_, _, age, _) = e {
                            Some(*age)
                        } else {
                            None
                        }
                    }),
            )
            .max()
            .unwrap_or(0);
        b.change = ChangeId(
            age.checked_add(1)
                .ok_or_else(|| invalid("change age overflow"))?,
        );
        let mut previous = None;
        for (age, old_len, cs) in log {
            let id = ChangeId(
                b.change
                    .0
                    .checked_sub(age)
                    .ok_or_else(|| invalid("change age"))?,
            );
            if previous.is_some_and(|p: u64| p.checked_add(1) != Some(id.0)) {
                return Err(invalid("noncontiguous change log"));
            }
            let map = Arc::new(checked_map(&cs, Some(old_len))?);
            h.record_map(LoggedChange {
                id,
                age: stamp,
                map,
                old_len,
            });
            previous = Some(id.0);
        }
        if previous.is_some_and(|id| id != b.change.0) {
            return Err(invalid("change log cursor"));
        }
        for (i, c) in h.log.iter().enumerate() {
            let next_len = h.log.get(i + 1).map_or(b.text.len_bytes(), |c| c.old_len);
            if mapped_len(c.old_len, &c.map)? != next_len {
                return Err(invalid("change log lengths"));
            }
        }
        for v in views {
            if !ids.contains(&v.id)
                || b.views.contains_key(&owners[&v.id])
                || v.settings.tab_size == 0
            {
                return Err(invalid("view identity or settings"));
            }
            selection_ok(&v.selection, &b.text)?;
            for set in v.regions.values() {
                let mut last = 0;
                for r in &set.regions {
                    if r.begin() < last || (set.drop_empty && r.empty()) {
                        return Err(invalid("named region order"));
                    }
                    region_ok(*r, &b.text)?;
                    last = r.begin();
                }
            }
            let event = |e: SavedEvent| -> Result<SoftEvent, HistoryError> {
                Ok(match e {
                    SavedEvent::Edit(i) => {
                        let step = built.get(i).ok_or_else(|| invalid("soft step"))?;
                        if step.owner != owners[&v.id] {
                            return Err(invalid("soft event owner"));
                        }
                        SoftEvent::Edit(step.after_state)
                    }
                    SavedEvent::Selection(before, after, age, state) => {
                        // These coordinates belong to an older revision, possibly an expired branch.
                        // Validate structure now, bounds when mapping to a known retained revision.
                        if !before.snapshot_valid(None)
                            || !after.snapshot_valid(None)
                            || state >= next_state
                        {
                            return Err(invalid("soft selection"));
                        }
                        let at = ChangeId(
                            b.change
                                .0
                                .checked_sub(age)
                                .ok_or_else(|| invalid("soft age"))?,
                        );
                        for selection in [&before, &after] {
                            if let Some(text) = known_texts.get(&state) {
                                selection_ok(selection, text)?;
                            }
                            if h.maps_from(at, b.change).is_some() {
                                let mapped = b
                                    .map_selection_from(&h, selection, at)
                                    .ok_or_else(|| invalid("soft event bounds"))?;
                                selection_ok(&mapped, &b.text)?;
                            }
                        }
                        SoftEvent::Selection {
                            before,
                            after,
                            at,
                            state,
                        }
                    }
                })
            };
            if v.soft.len() > limits.selection_entries
                || v.soft_redo.len() > limits.selection_entries
            {
                return Err(invalid("soft history limit"));
            }
            let soft: Vec<_> = v.soft.into_iter().map(event).collect::<Result<_, _>>()?;
            let soft_redo: Vec<_> = v
                .soft_redo
                .into_iter()
                .map(event)
                .collect::<Result<_, _>>()?;
            for (queue, redo) in [(soft, false), (soft_redo, true)] {
                for event in queue {
                    h.record_soft(owners[&v.id], SoftRecord { age: stamp, event }, redo);
                }
            }
            b.views.insert(
                owners[&v.id],
                ViewState {
                    id: owners[&v.id],
                    selection: v.selection,
                    settings: v.settings,
                    regions: v.regions,
                    find: None,
                },
            );
        }
        b.budget.lock().insert(b.id, h);
        Ok(b)
    }
    /// Gets shared retained maps after a revision, without copying their edit arrays.
    /// Returns `None` if that revision has already been evicted.
    pub fn retained_changes_since(
        &self,
        from: ChangeId,
    ) -> Option<impl DoubleEndedIterator<Item = (ChangeId, Arc<ChangeSet>)> + ExactSizeIterator>
    {
        self.retained_changes_up_to(from, usize::MAX)
    }
    /// As `retained_changes_since`, reading at most the first `limit` maps.
    pub fn retained_changes_up_to(
        &self,
        from: ChangeId,
        limit: usize,
    ) -> Option<impl DoubleEndedIterator<Item = (ChangeId, Arc<ChangeSet>)> + ExactSizeIterator>
    {
        let budget = self.budget.lock();
        let maps = budget.get(self.id).maps_from(from, self.change)?;
        let entries: Vec<_> = maps.take(limit).map(|c| (c.id, c.map.clone())).collect();
        Some(entries.into_iter())
    }
    /// Whether retained change maps are approaching either configured eviction limit.
    /// Incremental clients can advance old anchors before the oldest maps are lost.
    pub fn change_log_pressure(&self) -> bool {
        let budget = self.budget.lock();
        let h = budget.get(self.id);
        h.log.len() >= h.limits.changes / 2 || h.bytes() >= h.limits.bytes.min(budget.limit()) / 2
    }
    /// Retained maps after an ID, in publication order, including individual undo/redo steps.
    /// `None` means the caller must reset its incremental service from the current snapshot.
    pub fn changes_since(&self, from: ChangeId) -> Option<Vec<(ChangeId, ChangeSet)>> {
        self.retained_changes_since(from)
            .map(|maps| maps.map(|(id, map)| (id, map.as_ref().clone())).collect())
    }
}
fn region_ok(r: Region, text: &Text) -> Result<(), HistoryError> {
    if !text.is_char_boundary(r.a)
        || !text.is_char_boundary(r.b)
        || r.xpos.is_some_and(|x| !x.is_finite())
    {
        Err(invalid("region boundary"))
    } else {
        Ok(())
    }
}
fn selection_ok(s: &Selection, text: &Text) -> Result<(), HistoryError> {
    if s.snapshot_valid(Some(text)) {
        Ok(())
    } else {
        Err(invalid("selection"))
    }
}
fn checked_map(cs: &[Change], old_len: Option<usize>) -> Result<ChangeSet, HistoryError> {
    let mut old = 0;
    let mut new = 0usize;
    for c in cs {
        if c.old_range.start < old
            || c.old_range.end < c.old_range.start
            || old_len.is_some_and(|n| c.old_range.end > n)
        {
            return Err(invalid("edit order or range"));
        }
        new = new
            .checked_add(c.old_range.start - old)
            .and_then(|n| n.checked_add(c.new_len))
            .ok_or_else(|| invalid("edit overflow"))?;
        old = c.old_range.end;
    }
    Ok(ChangeSet::new(cs.to_vec()))
}
fn mapped_len(mut len: usize, cs: &ChangeSet) -> Result<usize, HistoryError> {
    for c in cs.changes() {
        len = len
            .checked_sub(c.old_range.len())
            .and_then(|n| n.checked_add(c.new_len))
            .ok_or_else(|| invalid("edit length overflow"))?;
    }
    Ok(len)
}
fn apply_checked(
    text: &mut Text,
    cs: &ChangeSet,
    expected: &[String],
    insert: &[String],
) -> Result<(), HistoryError> {
    checked_map(cs.changes(), Some(text.len_bytes()))?;
    mapped_len(text.len_bytes(), cs)?;
    if cs.changes().len() != insert.len() || insert.len() != expected.len() {
        return Err(invalid("edit cardinality"));
    }
    for ((c, old), new) in cs.changes().iter().zip(expected).zip(insert) {
        if new.len() != c.new_len
            || !text.is_char_boundary(c.old_range.start)
            || !text.is_char_boundary(c.old_range.end)
            || text.slice(c.old_range.clone()).to_string() != *old
        {
            return Err(invalid("edit text or UTF-8 boundary"));
        }
    }
    text.apply_edits(
        &cs.changes()
            .iter()
            .zip(insert)
            .map(|(c, s)| Edit {
                range: c.old_range.clone(),
                text: s,
            })
            .collect::<Vec<_>>(),
    );
    Ok(())
}

// Delta-code repeated coordinates before varint packing. Compression alone is ineffective
// on 10,000 monotonically increasing selection/map coordinates for one-byte typing.
fn relative(value: usize, base: usize, decode: bool) -> usize {
    if decode {
        base.wrapping_add((value >> 1) ^ 0usize.wrapping_sub(value & 1))
    } else {
        let d = value.wrapping_sub(base);
        (d << 1) ^ ((d as isize >> (usize::BITS - 1)) as usize)
    }
}
fn coordinate(value: &mut usize, base: &mut usize, decode: bool) {
    let actual = if decode {
        relative(*value, *base, true)
    } else {
        *value
    };
    *value = if decode {
        actual
    } else {
        relative(actual, *base, false)
    };
    *base = actual;
}
impl Body {
    fn delta_code(&mut self, decode: bool) {
        let mut base = 0;
        let mut state = 0;
        for step in &mut self.steps {
            let mut actual_base = base;
            if let Some(c) = step.changes.first_mut() {
                coordinate(&mut c.old_range.start, &mut actual_base, decode);
            } else {
                actual_base = 0;
            }
            let original_before = if decode {
                relative(step.before_state as usize, state, true)
            } else {
                step.before_state as usize
            };
            step.before_state = if decode {
                original_before as u64
            } else {
                relative(original_before, state, false) as u64
            };
            step.after_state = relative(step.after_state as usize, original_before, decode) as u64;
            state = original_before;
            step.before_selection.snapshot_relative(actual_base, decode);
            step.after_selection.snapshot_relative(actual_base, decode);
            let mut previous_end = actual_base;
            for (i, c) in step.changes.iter_mut().enumerate() {
                let start = if i == 0 {
                    actual_base
                } else if decode {
                    relative(c.old_range.start, previous_end, true)
                } else {
                    c.old_range.start
                };
                let end = if decode {
                    start.wrapping_add(c.old_range.end)
                } else {
                    c.old_range.end
                };
                if i > 0 {
                    c.old_range.start = if decode {
                        start
                    } else {
                        relative(start, previous_end, false)
                    };
                }
                c.old_range.end = if decode { end } else { end.wrapping_sub(start) };
                previous_end = end;
            }
            base = actual_base;
        }
        let mut age = 0;
        let mut len = 0;
        let mut base = 0;
        for (saved_age, old_len, changes) in &mut self.log {
            let mut v = *saved_age as usize;
            coordinate(&mut v, &mut age, decode);
            *saved_age = v as u64;
            coordinate(old_len, &mut len, decode);
            let mut previous_end = 0;
            for (i, c) in changes.iter_mut().enumerate() {
                let coordinate_base = if i == 0 { base } else { previous_end };
                let start = if decode {
                    relative(c.old_range.start, coordinate_base, true)
                } else {
                    c.old_range.start
                };
                let end = if decode {
                    start.wrapping_add(c.old_range.end)
                } else {
                    c.old_range.end
                };
                c.old_range.start = if decode {
                    start
                } else {
                    relative(start, coordinate_base, false)
                };
                c.old_range.end = if decode { end } else { end.wrapping_sub(start) };
                if i == 0 {
                    base = start;
                }
                previous_end = end;
            }
        }
        let mut index = 0;
        for group in self.undo.iter_mut().chain(&mut self.redo) {
            for i in &mut group.steps {
                coordinate(i, &mut index, decode);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn corrupt(mutate: impl FnOnce(&mut Body)) -> Result<Buffer, HistoryError> {
        let mut b = Buffer::new("世abc");
        let v = b.new_view();
        b.set_selection(v, Selection::from(Region::caret(3)));
        crate::ops::insert(&mut b, v, "x");
        let snapshot = b.export_history();
        let mut wire = snapshot.wire().unwrap();
        let compressed = STANDARD.decode(&wire.data).unwrap();
        let mut raw = Vec::new();
        ZlibDecoder::new(compressed.as_slice())
            .read_to_end(&mut raw)
            .unwrap();
        let mut body: Body = postcard::from_bytes(&raw).unwrap();
        body.delta_code(true);
        mutate(&mut body);
        body.delta_code(false);
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder
            .write_all(&postcard::to_allocvec(&body).unwrap())
            .unwrap();
        wire.data = STANDARD.encode(encoder.finish().unwrap());
        Buffer::restore_history(
            b.text.clone(),
            HistorySnapshot {
                repr: Repr::Packed(wire),
            },
            &BTreeMap::from([(v, ViewId::fresh())]),
        )
    }
    #[test]
    fn malformed_internal_payloads_are_errors_not_panics() {
        assert!(corrupt(|b| b.steps[0].changes[0].old_range = 1..2).is_err()); // inside 世
        assert!(corrupt(|b| b.steps[0].inserted[0] = "corrupt".into()).is_err());
        assert!(corrupt(|b| b.undo[0].steps[0] = usize::MAX).is_err());
        assert!(corrupt(|b| b.log[0].2[0].new_len = usize::MAX).is_err());
        assert!(
            corrupt(|b| {
                if let SavedEvent::Selection(before, _, _, _) = &mut b.views[0].soft[0] {
                    *before = Selection::from(Region::caret(1));
                }
            })
            .is_err()
        );
        assert!(
            corrupt(|b| b.views[0].selection = Selection::from(Region::caret(usize::MAX))).is_err()
        );
    }
}
