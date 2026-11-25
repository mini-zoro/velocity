use crate::{Bias, Change, ChangeId, ChangeSet, Region, Selection};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use vel_text::{Edit, Text};

#[path = "history_budget.rs"]
mod history_budget;
pub use history_budget::HistoryBudget;
use history_budget::{History, SoftRecord, default_buffer_bytes};

#[path = "history.rs"]
mod history;
pub use history::{HistoryError, HistorySnapshot};

static NEXT_BUFFER: AtomicU64 = AtomicU64::new(1);
static NEXT_VIEW: AtomicU64 = AtomicU64::new(1);
/// Process-unique document identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BufferId(pub u64);
/// Process-unique view identity.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ViewId(pub u64);
/// Intent used to control undo grouping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    /// Continuous typing; adjacent same-class runs can group.
    InsertChars,
    /// Paste; always its own undo step.
    Paste,
    /// Deletion; consecutive single deletions next to the carets in one direction group.
    Delete,
    /// All other editing commands.
    Other,
}
/// Configurable bounds; bytes include metadata and retained rope allocations.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct HistoryLimits {
    /// Maximum retained committed transactions, including redo.
    pub entries: usize,
    /// Maximum accounted retained bytes for this buffer, including soft history and change maps.
    pub bytes: usize,
    /// Per-view selection-event limit (minimum 256).
    pub selection_entries: usize,
    /// Maximum transaction maps, including undo/redo maps.
    pub changes: usize,
}
impl Default for HistoryLimits {
    fn default() -> Self {
        Self {
            entries: 10_000,
            bytes: default_buffer_bytes(),
            selection_entries: 256,
            changes: 10_000,
        }
    }
}
/// Text semantics passed explicitly to operations; no global configuration dependency.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Characters classified as punctuation.
    pub word_separators: String,
    /// Extra delimiters for sub-word boundaries.
    // Editor configuration reapplies this value on restore. Do not change the
    // positional Settings payload in the version-1 postcard undo history.
    #[serde(skip, default = "default_sub_word_separators")]
    pub sub_word_separators: String,
    /// Visual tab width, at least one.
    pub tab_size: usize,
    /// Indent with spaces and erase leading spaces to a previous tab stop.
    pub translate_tabs_to_spaces: bool,
    /// With spaces, Backspace in leading white space erases back to the previous tab stop
    /// (`use_tab_stops`); otherwise one character.
    pub use_tab_stops: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            word_separators: "./\\()\"'-:,.;<>~!@#$%^&*|+=[]{}`~?".into(),
            sub_word_separators: default_sub_word_separators(),
            tab_size: 4,
            translate_tabs_to_spaces: true,
            use_tab_stops: true,
        }
    }
}
fn default_sub_word_separators() -> String {
    "_".into()
}
/// One named annotation set. Overlapping regions are allowed.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct RegionSet {
    /// Regions in ascending begin order.
    pub regions: Vec<Region>,
    /// Whether regions collapsed by editing are discarded.
    pub drop_empty: bool,
    // A fold can include a line separator even when the line is temporarily at
    // an unterminated EOF. Keep that affinity separate from physical ranges.
    #[serde(skip)]
    pub(crate) missing_lf: Vec<Region>,
}
impl RegionSet {
    /// Whether there are any regions, including folds with no physical byte at EOF.
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty() && self.missing_lf.is_empty()
    }
}
#[derive(Clone, Debug)]
enum SoftEvent {
    Selection {
        before: Selection,
        after: Selection,
        at: ChangeId,
        state: u64,
    },
    Edit(u64),
}
/// Per-view state. Change selections through `Buffer::set_selection` to record soft undo.
#[derive(Clone, Debug)]
pub struct ViewState {
    id: ViewId,
    selection: Selection,
    regions: BTreeMap<String, RegionSet>,
    /// Word and indentation preferences for this view.
    pub settings: Settings,
    pub(crate) find: Option<FindState>,
}
#[derive(Clone, Debug)]
pub(crate) struct FindState {
    pub needle: String,
    pub whole_word: bool,
    pub next: usize,
    pub at: ChangeId,
}
impl ViewState {
    /// Unique identity.
    pub fn id(&self) -> ViewId {
        self.id
    }
    /// Current normalized selection.
    pub fn sel(&self) -> &Selection {
        &self.selection
    }
    /// Named regions, or `None` if the key is absent.
    pub fn get_regions(&self, key: &str) -> Option<&RegionSet> {
        self.regions.get(key)
    }
}
#[derive(Debug)]
struct Step {
    // Pure uniform insertions need only their compact inverse ranges and one shared string.
    // Replacements/deletions retain rope snapshots, never copied deleted strings.
    before: Option<Text>,
    after: Option<Text>,
    inserted: Option<Arc<str>>,
    before_selection: Selection,
    after_selection: Selection,
    owner: ViewId,
    changes: Arc<ChangeSet>,
    before_state: u64,
    after_state: u64,
    age: u64,
    // Rope and transaction bookkeeping; shared selections and maps are charged separately.
    bytes: usize,
}
impl Step {
    fn fixed_bytes(retained_text: usize) -> usize {
        // Include the Arc, group slots, and revision index entry.
        retained_text + std::mem::size_of::<Self>() + 128
    }
}
#[derive(Clone, Debug)]
struct Group {
    steps: Vec<Arc<Step>>,
    run: Option<Run>,
    has_word: bool,
}
/// A buffer-local undo boundary, valid while its revision remains in undo history.
#[derive(Clone, Copy, Debug)]
pub struct UndoMarker {
    buffer: BufferId,
    state: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Run {
    Word,
    Whitespace,
    Punctuation,
    Newline,
    /// Deletions ending at every caret (backspace).
    DeleteBack,
    /// Deletions starting at every caret (forward delete).
    DeleteForward,
}
#[derive(Clone, Debug)]
struct LoggedChange {
    id: ChangeId,
    age: u64,
    map: Arc<ChangeSet>,
    old_len: usize,
}
/// Shared document, transaction history and all attached views.
#[derive(Debug)]
pub struct Buffer {
    id: BufferId,
    text: Text,
    views: BTreeMap<ViewId, ViewState>,
    budget: HistoryBudget,
    change: ChangeId,
    state: u64,
    next_state: u64,
    saved: u64,
    group_open: bool,
}
impl Drop for Buffer {
    fn drop(&mut self) {
        self.budget.lock().remove(self.id);
    }
}
impl Buffer {
    /// Creates a saved buffer; attach one or more views with `new_view()`.
    pub fn new(text: impl Into<Text>) -> Self {
        Self::with_history_budget(text, HistoryBudget::process())
    }
    /// Creates a buffer that shares the supplied budget instead of the process default.
    pub fn with_history_budget(text: impl Into<Text>, budget: HistoryBudget) -> Self {
        let buffer = Self {
            id: BufferId(NEXT_BUFFER.fetch_add(1, Ordering::Relaxed)),
            text: text.into(),
            views: BTreeMap::new(),
            budget,
            change: ChangeId(0),
            state: 0,
            next_state: 1,
            saved: 0,
            group_open: false,
        };
        buffer.budget.lock().insert(buffer.id, History::default());
        buffer
    }
    fn update_history<R>(&mut self, f: impl FnOnce(&mut Self, &mut History, u64) -> R) -> R {
        let budget = self.budget.clone();
        budget
            .lock()
            .update(self.id, |history, age| f(self, history, age))
    }
    /// Unique document identity.
    pub fn id(&self) -> BufferId {
        self.id
    }
    /// Immutable text, clonable in O(1) for background readers.
    pub fn text(&self) -> &Text {
        &self.text
    }
    /// Creates a view with one caret at zero.
    pub fn new_view(&mut self) -> ViewId {
        let id = ViewId(NEXT_VIEW.fetch_add(1, Ordering::Relaxed));
        self.views.insert(
            id,
            ViewState {
                id,
                selection: Selection::default(),
                regions: BTreeMap::new(),
                settings: Settings::default(),
                find: None,
            },
        );
        id
    }
    /// Detaches a view; undo can still restore text from its transactions.
    pub fn close_view(&mut self, id: ViewId) -> bool {
        self.break_undo_group();
        let removed = self.views.remove(&id).is_some();
        if removed {
            self.update_history(|_, h, _| h.close_view(id));
        }
        removed
    }
    /// Attached view identities.
    pub fn views(&self) -> impl Iterator<Item = ViewId> + '_ {
        self.views.keys().copied()
    }
    /// Per-view state; an unknown identity panics before mutation.
    pub fn view(&self, id: ViewId) -> &ViewState {
        self.views
            .get(&id)
            .expect("view is not attached to this buffer")
    }
    /// Changes settings, ending a typing group.
    pub fn set_settings(&mut self, id: ViewId, settings: Settings) {
        assert!(settings.tab_size > 0, "tab size must be positive");
        self.break_undo_group();
        self.views.get_mut(&id).expect("attached view").settings = settings;
    }
    /// Records a selection-only change in the view's bounded soft history.
    pub fn set_selection(&mut self, id: ViewId, mut selection: Selection) {
        selection.normalize();
        validate_selection(&self.text, &selection);
        self.break_undo_group();
        let v = self.views.get_mut(&id).expect("attached view");
        if v.selection == selection {
            return;
        }
        let before = std::mem::replace(&mut v.selection, selection.clone());
        v.find = None;
        let limit = self.budget.limit_bytes();
        self.update_history(|buffer, h, age| {
            h.clear_soft_redo(id);
            h.record_selection(
                id,
                SoftRecord {
                    age,
                    event: SoftEvent::Selection {
                        before,
                        after: selection,
                        at: buffer.change,
                        state: buffer.state,
                    },
                },
                limit,
            );
        });
    }
    /// Adds or replaces a named annotation set.
    pub fn add_regions(
        &mut self,
        id: ViewId,
        key: impl Into<String>,
        mut regions: Vec<Region>,
        drop_empty: bool,
    ) {
        for r in &regions {
            validate_region(&self.text, *r);
        }
        regions.sort_by_key(|r| r.begin());
        if drop_empty {
            regions.retain(|r| !r.empty());
        }
        self.views
            .get_mut(&id)
            .expect("attached view")
            .regions
            .insert(
                key.into(),
                RegionSet {
                    regions,
                    drop_empty,
                    missing_lf: Vec::new(),
                },
            );
    }
    /// Removes a named annotation set.
    pub fn erase_regions(&mut self, id: ViewId, key: &str) -> bool {
        self.views
            .get_mut(&id)
            .expect("attached view")
            .regions
            .remove(key)
            .is_some()
    }
    /// Last committed transaction, including undo/redo.
    pub fn change_id(&self) -> ChangeId {
        self.change
    }
    /// Maps an old point, or returns `None` for an expired/future ID or out-of-bounds point.
    pub fn transform_point_from(&self, point: usize, from: ChangeId, bias: Bias) -> Option<usize> {
        let budget = self.budget.lock();
        self.map_point(budget.get(self.id), point, from, bias)
    }
    fn map_point(&self, h: &History, point: usize, from: ChangeId, bias: Bias) -> Option<usize> {
        let mut maps = h.maps_from(from, self.change)?.peekable();
        (point <= maps.peek().map_or(self.text.len_bytes(), |c| c.old_len))
            .then(|| maps.fold(point, |p, c| c.map.transform_point(p, bias)))
    }
    /// Projects sorted current byte points back through retained edits. For each
    /// point, picks the largest old byte with a left-biased image at or below it.
    /// Returns false without changing the input if the version is unavailable or
    /// the points are out of bounds. Results need not be UTF-8 boundaries.
    pub fn unmap_left_points_to(&self, points: &mut [usize], to: ChangeId) -> bool {
        debug_assert!(points.is_sorted());
        let budget = self.budget.lock();
        let Some(maps) = budget.get(self.id).maps_from(to, self.change) else {
            return false;
        };
        if points.last().is_some_and(|&p| p > self.text.len_bytes()) {
            return false;
        }
        for change in maps.rev() {
            change.map.unmap_left_points(points);
        }
        true
    }
    /// Maps a historic named region with exclusion bias at its boundaries.
    pub fn transform_region_from(&self, region: Region, from: ChangeId) -> Option<Region> {
        let budget = self.budget.lock();
        let mut maps = budget.get(self.id).maps_from(from, self.change)?.peekable();
        (region.end() <= maps.peek().map_or(self.text.len_bytes(), |c| c.old_len))
            .then(|| maps.fold(region, |r, c| c.map.transform_region(r)))
    }
    /// Ends continuous typing explicitly.
    pub fn break_undo_group(&mut self) {
        self.group_open = false;
    }
    /// Starts a boundary that can later glue completed native typing groups.
    /// Recording a marker does not change undo/redo behavior until gluing.
    pub fn mark_undo_group(&mut self) -> UndoMarker {
        self.break_undo_group();
        UndoMarker {
            buffer: self.id,
            state: self.state,
        }
    }
    /// Glues the undo groups after `marker` into one, preserving all transactions
    /// and soft-history events. Returns false if the boundary was undone, pruned,
    /// or belongs to another buffer; unrelated history is never absorbed.
    pub fn glue_undo_groups(&mut self, marker: UndoMarker) -> bool {
        self.break_undo_group();
        if marker.buffer != self.id || marker.state == self.state {
            return false;
        }
        self.update_history(|_, h, _| {
            let Some(start) = h
                .undo
                .iter()
                .rposition(|g| g.steps[0].before_state == marker.state)
            else {
                return false;
            };
            let groups = h.undo.split_off(start);
            let mut group = Group {
                steps: Vec::new(),
                run: None,
                has_word: false,
            };
            for mut next in groups {
                group.steps.append(&mut next.steps);
            }
            h.undo.push_back(group);
            true
        })
    }
    /// Replaces all history limits and immediately enforces them.
    pub fn set_history_limits(&mut self, mut limits: HistoryLimits) {
        limits.selection_entries = limits.selection_entries.max(256);
        self.update_history(|_, h, _| h.set_limits(limits));
    }
    /// Current history limits.
    pub fn history_limits(&self) -> HistoryLimits {
        self.budget.lock().get(self.id).limits
    }
    /// Number of transactions retained across undo and redo.
    pub fn history_entries(&self) -> usize {
        self.budget.lock().get(self.id).entries
    }
    /// Accounted bytes retained by text, selection, and change-map history.
    pub fn history_retained_bytes(&self) -> usize {
        self.budget.lock().get(self.id).bytes()
    }
    /// Metadata owned by the last undo transaction, including both selection arrays.
    /// Shared document storage is accounted separately by `history_retained_bytes()`.
    pub fn last_undo_metadata_bytes(&self) -> usize {
        self.budget
            .lock()
            .get(self.id)
            .undo
            .back()
            .and_then(|g| g.steps.last())
            .map_or(0, |s| {
                std::mem::size_of::<Step>()
                    + s.changes.storage_bytes()
                    + s.before_selection.storage_bytes()
                    + s.after_selection.storage_bytes()
                    + 2 * Selection::header_bytes()
                    + std::mem::size_of::<ChangeSet>()
                    + std::mem::size_of::<Group>()
                    + std::mem::size_of::<SoftEvent>()
                    + std::mem::size_of::<LoggedChange>()
                    + 7 * std::mem::size_of::<usize>()
                    + s.inserted.as_ref().map_or(0, |text| text.len())
            })
    }
    /// Marks the exact current revision saved, breaking the typing group.
    pub fn mark_saved(&mut self) {
        self.saved = self.state;
        self.break_undo_group();
    }
    /// Whether the current revision differs from the saved revision.
    pub fn is_dirty(&self) -> bool {
        self.state != self.saved
    }
    /// Marks restored or externally changed contents unsaved without manufacturing an edit.
    pub fn mark_unsaved(&mut self) {
        if !self.is_dirty() {
            self.invalidate_saved_revision();
        }
    }
    /// Invalidates the previous saved point after accepting a different disk revision.
    /// No existing undo/redo state can become clean until a successful save.
    pub fn invalidate_saved_revision(&mut self) {
        self.saved = self.next_state;
        self.next_state = self.next_state.checked_add(1).expect("revision overflow");
    }
    /// Whether a replacement leaves its old bytes intact. Insertions/deletions with different
    /// lengths take constant time; equal-length replacements compare only the affected slice.
    pub fn edit_is_noop(&self, edit: &Edit<'_>) -> bool {
        edit.range.len() == edit.text.len()
            && (if edit.range.is_empty() {
                assert!(
                    self.text.is_char_boundary(edit.range.start),
                    "edit must be on a scalar boundary"
                );
                true
            } else {
                self.text.slice(edit.range.clone()) == edit.text
            })
    }
    /// Applies validated edits once in pre-transaction coordinates and records undo.
    /// Invalid edits or selections panic without changing the buffer or its history.
    pub fn transact(
        &mut self,
        id: ViewId,
        kind: EditKind,
        edits: &[Edit<'_>],
        new_selection: Selection,
    ) -> ChangeId {
        self.transact_with_region_moves(id, kind, edits, new_selection, None)
    }
    pub(crate) fn transact_with_region_moves(
        &mut self,
        id: ViewId,
        kind: EditKind,
        edits: &[Edit<'_>],
        new_selection: Selection,
        moves: Option<crate::change::RegionMoves>,
    ) -> ChangeId {
        self.transact_impl(id, kind, edits, new_selection, moves, None)
    }
    /// Records projected edits together, using the original caret edits for typing-group decisions.
    /// For example, snippet mirrors need not have a caret of their own.
    pub fn transact_projected(
        &mut self,
        id: ViewId,
        kind: EditKind,
        edits: &[Edit<'_>],
        new_selection: Selection,
        caret_edits: &[Edit<'_>],
    ) -> ChangeId {
        self.transact_impl(id, kind, edits, new_selection, None, Some(caret_edits))
    }
    fn transact_impl(
        &mut self,
        id: ViewId,
        kind: EditKind,
        edits: &[Edit<'_>],
        mut new_selection: Selection,
        moves: Option<crate::change::RegionMoves>,
        caret_edits: Option<&[Edit<'_>]>,
    ) -> ChangeId {
        // Validate even an all-no-op request, before changing selections or history.
        let mut previous_end = 0;
        for edit in edits {
            assert!(
                edit.range.start >= previous_end
                    && edit.range.start <= edit.range.end
                    && edit.range.end <= self.text.len_bytes(),
                "invalid edit range/order"
            );
            previous_end = edit.range.end;
        }
        let filtered;
        let edits = if edits.iter().any(|e| self.edit_is_noop(e)) {
            filtered = edits
                .iter()
                .filter(|e| !self.edit_is_noop(e))
                .map(|e| Edit {
                    range: e.range.clone(),
                    text: e.text,
                })
                .collect::<Vec<_>>();
            filtered.as_slice()
        } else {
            edits
        };
        if edits.is_empty() {
            new_selection.normalize();
            validate_selection(&self.text, &new_selection);
            if self.view(id).sel() != &new_selection {
                self.set_selection(id, new_selection);
            }
            return self.change;
        }
        let before_selection = self.view(id).selection.clone();
        let changes = Arc::new(
            ChangeSet::new(
                edits
                    .iter()
                    .map(|e| Change {
                        old_range: e.range.clone(),
                        new_len: e.text.len(),
                    })
                    .collect(),
            )
            .with_region_moves(moves),
        );
        let before = self.text.clone();
        let mut after = before.clone();
        after.apply_edits(edits);
        new_selection.normalize();
        new_selection.reset_xpos();
        // apply_edits has already validated every old endpoint. The end of each
        // replacement is therefore a proven scalar boundary in the result. Most
        // Unicode typing places exactly one caret there per edit; checking those
        // coordinates avoids traversing a growing number of rope fragments.
        if after.len_bytes() == after.len_chars() || !selection_at_edit_ends(&new_selection, edits)
        {
            validate_selection(&after, &new_selection);
        }
        let caret_edits = caret_edits.unwrap_or(edits);
        let run = typing_run(
            kind,
            caret_edits,
            &self.view(id).settings,
            &before_selection,
        );
        let state = self.next_state;
        self.next_state = self.next_state.checked_add(1).expect("revision overflow");
        // Account both possible current roots; unchanged shared allocations cost zero.
        // Pure insertion avoids snapshot retention and this diagnostic tree walk.
        let inserted = edits
            .first()
            .filter(|first| {
                edits
                    .iter()
                    .all(|e| e.range.is_empty() && e.text == first.text)
            })
            .map(|e| Arc::<str>::from(e.text));
        let retained_text = if let Some(text) = &inserted {
            text.len() + 2 * std::mem::size_of::<usize>()
        } else {
            before
                .estimated_retained_bytes(&after)
                .max(after.estimated_retained_bytes(&before))
        };
        let step = Step {
            before: if inserted.is_some() {
                None
            } else {
                Some(before)
            },
            after: if inserted.is_some() {
                None
            } else {
                Some(after.clone())
            },
            inserted,
            before_selection,
            after_selection: new_selection.clone(),
            owner: id,
            changes: changes.clone(),
            before_state: self.state,
            after_state: state,
            age: 0,
            bytes: Step::fixed_bytes(retained_text),
        };
        self.text = after;
        self.state = state;
        let old_len = self.publish(&changes, Some(id));
        self.views.get_mut(&id).expect("attached view").selection = new_selection;
        self.update_history(|buffer, h, age| {
            let merge = buffer.group_open
                && h.undo
                    .back()
                    .is_some_and(|g| can_group(g, id, &step.before_selection, run, caret_edits));
            h.clear_redo();
            let step = Arc::new(Step { age, ..step });
            h.record_step(&step);
            if merge {
                let g = h.undo.back_mut().expect("merge group");
                g.steps.push(step);
                g.run = run;
                g.has_word |= run == Some(Run::Word);
            } else {
                h.undo.push_back(Group {
                    steps: vec![step],
                    run,
                    has_word: run == Some(Run::Word),
                });
            }
            buffer.group_open = run.is_some();
            h.record_map(LoggedChange {
                id: buffer.change,
                age,
                map: changes,
                old_len,
            });
            h.record_soft(
                id,
                SoftRecord {
                    age,
                    event: SoftEvent::Edit(state),
                },
                false,
            );
        });
        self.change
    }
    fn publish(&mut self, map: &ChangeSet, owner: Option<ViewId>) -> usize {
        // Recover the pre-edit length from the net delta, avoiding a second text snapshot.
        let mut old_len = self.text.len_bytes();
        for c in map.changes() {
            old_len = old_len + c.old_range.len() - c.new_len;
        }
        self.change.0 = self.change.0.checked_add(1).expect("change id overflow");
        for v in self.views.values_mut() {
            if Some(v.id) != owner {
                v.selection = map.transform_selection(&v.selection);
            }
            v.find = None;
            // ST-BEHAVIOR: unverified [M2]: annotations map through undo/redo; dropped regions are not resurrected. Equal starts keep input order.
            for (key, set) in &mut v.regions {
                if map.transform_separator_regions(key, set, self.text.len_bytes()) {
                    continue;
                }
                for r in &mut set.regions {
                    *r = map.transform_named_region(key, *r);
                }
                if set.drop_empty {
                    set.regions.retain(|r| !r.empty());
                }
                // Replacement starts map Right to the new end, while interior points map
                // to its start. Restore stable order for navigation and mark index pairing.
                // This is linear on the already-sorted insertion/deletion path.
                set.regions.sort_by_key(|r| r.begin());
            }
        }
        old_len
    }
    fn apply_step(&mut self, s: &Step, forward: bool, age: u64) -> LoggedChange {
        let (selection, state, map) = if forward {
            (&s.after_selection, s.after_state, s.changes.clone())
        } else {
            (
                &s.before_selection,
                s.before_state,
                Arc::new(s.changes.inverse()),
            )
        };
        if let Some(inserted) = &s.inserted {
            let edits: Vec<_> = map
                .changes()
                .iter()
                .map(|c| Edit {
                    range: c.old_range.clone(),
                    text: if forward { inserted.as_ref() } else { "" },
                })
                .collect();
            self.text.apply_edits(&edits);
        } else {
            self.text = if forward {
                s.after.as_ref()
            } else {
                s.before.as_ref()
            }
            .expect("replacement snapshot")
            .clone();
        }
        self.state = state;
        let old_len = self.publish(&map, Some(s.owner));
        if let Some(v) = self.views.get_mut(&s.owner) {
            v.selection = selection.clone();
        }
        LoggedChange {
            id: self.change,
            age,
            map,
            old_len,
        }
    }
    /// Undoes the latest global edit group, restoring the originating view's selection.
    pub fn undo(&mut self, id: ViewId) -> bool {
        // ST-BEHAVIOR: unverified [H3]: undo/redo are buffer-global and restore the originating view; soft edit undo only proceeds at the current revision.
        self.view(id);
        self.break_undo_group();
        self.update_history(|buffer, h, age| {
            let Some(group) = h.undo.pop_back() else {
                return false;
            };
            for step in group.steps.iter().rev() {
                h.record_map(buffer.apply_step(step, false, age));
            }
            h.redo.push_back(group);
            true
        })
    }
    /// Reapplies the most recently undone group.
    pub fn redo(&mut self, id: ViewId) -> bool {
        self.view(id);
        self.break_undo_group();
        self.update_history(|buffer, h, age| {
            let Some(group) = h.redo.pop_back() else {
                return false;
            };
            for step in &group.steps {
                h.record_map(buffer.apply_step(step, true, age));
            }
            h.undo.push_back(group);
            true
        })
    }
    /// Undoes the latest live selection event or individual transaction of this view.
    pub fn soft_undo(&mut self, id: ViewId) -> bool {
        self.view(id);
        self.break_undo_group();
        self.update_history(|buffer, h, age| {
            while let Some(record) = h.pop_soft(id, false) {
                match &record.event {
                    SoftEvent::Selection {
                        before, at, state, ..
                    } => {
                        let selection = if *state == buffer.state {
                            Some(before.clone())
                        } else {
                            buffer.map_selection_from(h, before, *at)
                        };
                        let Some(selection) = selection else {
                            continue;
                        };
                        buffer.views.get_mut(&id).expect("view").selection = selection;
                    }
                    SoftEvent::Edit(state) => {
                        if buffer.state != *state {
                            continue;
                        }
                        let Some(group) = h.undo.back_mut() else {
                            continue;
                        };
                        if !group.steps.last().is_some_and(|s| s.after_state == *state) {
                            continue;
                        }
                        let step = group.steps.pop().expect("last step");
                        if group.steps.is_empty() {
                            h.undo.pop_back();
                        }
                        h.record_map(buffer.apply_step(&step, false, age));
                        h.redo.push_back(Group {
                            steps: vec![step],
                            run: None,
                            has_word: false,
                        });
                    }
                }
                h.record_soft(id, record, true);
                return true;
            }
            false
        })
    }
    /// Reapplies the most recently soft-undone event, if still applicable.
    pub fn soft_redo(&mut self, id: ViewId) -> bool {
        self.view(id);
        self.break_undo_group();
        self.update_history(|buffer, h, age| {
            let Some(record) = h.pop_soft(id, true) else {
                return false;
            };
            match &record.event {
                SoftEvent::Selection {
                    after, at, state, ..
                } => {
                    let s = if *state == buffer.state {
                        Some(after.clone())
                    } else {
                        buffer.map_selection_from(h, after, *at)
                    };
                    let Some(s) = s else {
                        return false;
                    };
                    buffer.views.get_mut(&id).expect("view").selection = s;
                }
                SoftEvent::Edit(state) => {
                    let Some(group) = h.redo.back().filter(|g| {
                        g.steps.len() == 1
                            && g.steps[0].after_state == *state
                            && g.steps[0].before_state == buffer.state
                    }) else {
                        return false;
                    };
                    h.record_map(buffer.apply_step(&group.steps[0], true, age));
                    let group = h.redo.pop_back().expect("matched redo group");
                    h.undo.push_back(group);
                }
            }
            h.record_soft(id, record, false);
            true
        })
    }
    fn map_selection_from(
        &self,
        history: &History,
        s: &Selection,
        at: ChangeId,
    ) -> Option<Selection> {
        let r: Option<Vec<_>> = s
            .iter()
            .map(|r| {
                Some(Region::new(
                    self.map_point(history, r.a, at, Bias::Right)?,
                    self.map_point(history, r.b, at, Bias::Right)?,
                ))
            })
            .collect();
        Some(Selection::from_regions(r?).with_primary(s.primary_index()))
    }
    pub(crate) fn view_mut(&mut self, id: ViewId) -> &mut ViewState {
        self.views.get_mut(&id).expect("attached view")
    }
}
fn validate_region(text: &Text, r: Region) {
    assert!(r.end() <= text.len_bytes(), "selection exceeds text");
    assert!(
        text.floor_char_boundary(r.a) == r.a && text.floor_char_boundary(r.b) == r.b,
        "selection must be on scalar boundaries"
    );
}
fn selection_at_edit_ends(selection: &Selection, edits: &[Edit<'_>]) -> bool {
    if selection.len() != edits.len() {
        return false;
    }
    let mut old_end = 0;
    let mut new_end = 0;
    selection.iter().zip(edits).all(|(r, edit)| {
        new_end += edit.range.start - old_end + edit.text.len();
        old_end = edit.range.end;
        r.empty() && r.b == new_end
    })
}

fn validate_selection(text: &Text, s: &Selection) {
    if text.len_bytes() == text.len_chars() {
        for r in s.iter() {
            assert!(r.end() <= text.len_bytes(), "selection exceeds text");
        }
    } else if s.len() >= 128 && text.len_bytes() <= s.len().saturating_mul(4096) {
        // Normalized selections have sorted, disjoint ranges. Walk borrowed
        // chunks once for dense carets instead of two Unicode metric lookups per
        // caret. Sparse selections still search the tree, even in huge files.
        let mut chunks = text.chunks();
        let mut chunk = chunks.next().unwrap_or("");
        let mut start = 0;
        for r in s.iter() {
            assert!(r.end() <= text.len_bytes(), "selection exceeds text");
            for byte in [r.begin(), r.end()] {
                while byte > start + chunk.len() {
                    start += chunk.len();
                    chunk = chunks.next().expect("selection within text");
                }
                assert!(
                    chunk.is_char_boundary(byte - start),
                    "selection must be on scalar boundaries"
                );
            }
        }
    } else {
        for r in s.iter() {
            validate_region(text, r);
        }
    }
}
fn typing_run(
    kind: EditKind,
    edits: &[Edit<'_>],
    settings: &Settings,
    selection: &Selection,
) -> Option<Run> {
    if kind == EditKind::Delete {
        if edits.is_empty() || edits.len() != selection.len() {
            return None;
        }
        let deletes = |at: fn(&Edit<'_>) -> usize| {
            selection
                .iter()
                .zip(edits)
                .all(|(r, e)| r.empty() && e.text.is_empty() && !e.range.is_empty() && at(e) == r.b)
        };
        return if deletes(|e| e.range.end) {
            Some(Run::DeleteBack)
        } else if deletes(|e| e.range.start) {
            Some(Run::DeleteForward)
        } else {
            None
        };
    }
    if kind != EditKind::InsertChars || edits.is_empty() {
        return None;
    }
    let mut run = None;
    for e in edits {
        if e.text.is_empty() {
            return None;
        }
        for ch in e.text.chars() {
            let c = if ch == '\n' {
                Run::Newline
            } else if ch.is_whitespace() {
                Run::Whitespace
            } else if settings.word_separators.contains(ch) {
                Run::Punctuation
            } else {
                Run::Word
            };
            if run.is_some_and(|old| old != c) {
                return None;
            }
            run = Some(c);
        }
    }
    run
}
fn can_group(
    group: &Group,
    id: ViewId,
    selection: &Selection,
    run: Option<Run>,
    edits: &[Edit<'_>],
) -> bool {
    // ST-BEHAVIOR: unverified [H2]: word + following spaces form a group; next word starts another.
    // Punctuation/newline transitions break groups. Selection changes always break groups.
    let Some(last) = group.steps.last() else {
        return false;
    };
    last.owner == id
        && last.after_selection == *selection
        && selection.len() == edits.len()
        && (matches!(run, Some(Run::DeleteBack | Run::DeleteForward))
            || selection
                .iter()
                .zip(edits)
                .all(|(r, e)| r.empty() && r.b == e.range.start))
        && (group.run == run
            || (group.run == Some(Run::Word) && run == Some(Run::Whitespace))
            || (!group.has_word && group.run == Some(Run::Whitespace) && run == Some(Run::Word)))
}
