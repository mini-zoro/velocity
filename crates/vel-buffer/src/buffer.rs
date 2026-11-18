use crate::{Bias, Change, ChangeId, ChangeSet, Region, Selection};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use vel_text::{Edit, Text};

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
    /// Maximum accounted retained bytes, including redo.
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
            bytes: 256 * 1024 * 1024,
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
            tab_size: 4,
            translate_tabs_to_spaces: true,
            use_tab_stops: true,
        }
    }
}
/// One named annotation set. Overlapping regions are allowed.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct RegionSet {
    /// Regions in ascending begin order.
    pub regions: Vec<Region>,
    /// Whether regions collapsed by editing are discarded.
    pub drop_empty: bool,
}
#[derive(Clone, Debug)]
enum SoftEvent {
    Selection {
        before: Selection,
        after: Selection,
        at: ChangeId,
        state: u64,
    },
    Edit(Arc<Step>),
}
/// Per-view state. Change selections through `Buffer::set_selection` to record soft undo.
#[derive(Clone, Debug)]
pub struct ViewState {
    id: ViewId,
    selection: Selection,
    regions: BTreeMap<String, RegionSet>,
    /// Word and indentation preferences for this view.
    pub settings: Settings,
    soft: VecDeque<SoftEvent>,
    soft_redo: Vec<SoftEvent>,
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
    bytes: usize,
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
    map: Arc<ChangeSet>,
    old_len: usize,
}
/// Shared document, transaction history and all attached views.
#[derive(Debug)]
pub struct Buffer {
    id: BufferId,
    text: Text,
    views: BTreeMap<ViewId, ViewState>,
    undo: VecDeque<Group>,
    redo: Vec<Group>,
    log: VecDeque<LoggedChange>,
    log_bytes: usize,
    change: ChangeId,
    limits: HistoryLimits,
    retained: usize,
    entries: usize,
    state: u64,
    next_state: u64,
    saved: u64,
    group_open: bool,
    live: BTreeSet<u64>,
}
impl Buffer {
    /// Creates a saved buffer; attach one or more views with `new_view()`.
    pub fn new(text: impl Into<Text>) -> Self {
        Self {
            id: BufferId(NEXT_BUFFER.fetch_add(1, Ordering::Relaxed)),
            text: text.into(),
            views: BTreeMap::new(),
            undo: VecDeque::new(),
            redo: Vec::new(),
            log: VecDeque::new(),
            log_bytes: 0,
            change: ChangeId(0),
            limits: HistoryLimits::default(),
            retained: 0,
            entries: 0,
            state: 0,
            next_state: 1,
            saved: 0,
            group_open: false,
            live: BTreeSet::new(),
        }
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
                soft: VecDeque::new(),
                soft_redo: Vec::new(),
                find: None,
            },
        );
        id
    }
    /// Detaches a view; undo can still restore text from its transactions.
    pub fn close_view(&mut self, id: ViewId) -> bool {
        self.break_undo_group();
        self.views.remove(&id).is_some()
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
        v.soft.push_back(SoftEvent::Selection {
            before,
            after: selection,
            at: self.change,
            state: self.state,
        });
        v.soft_redo.clear();
        v.find = None;
        while v.soft.len() > self.limits.selection_entries {
            v.soft.pop_front();
        }
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
    pub fn transform_point_from(
        &self,
        mut point: usize,
        from: ChangeId,
        bias: Bias,
    ) -> Option<usize> {
        if !self.retains(from) {
            return None;
        }
        let first = self.log.partition_point(|c| c.id <= from);
        let mut maps = self.log.range(first..);
        if let Some(first) = maps.next() {
            if point > first.old_len {
                return None;
            }
            point = first.map.transform_point(point, bias);
            for c in maps {
                point = c.map.transform_point(point, bias);
            }
        } else if point > self.text.len_bytes() {
            return None;
        }
        Some(point)
    }
    /// Maps a historic named region with exclusion bias at its boundaries.
    pub fn transform_region_from(&self, mut region: Region, from: ChangeId) -> Option<Region> {
        if !self.retains(from) {
            return None;
        }
        let mut maps = self.log.iter().filter(|c| c.id > from);
        if let Some(first) = maps.next() {
            if region.end() > first.old_len {
                return None;
            }
            region = first.map.transform_region(region);
            for c in maps {
                region = c.map.transform_region(region);
            }
        } else if region.end() > self.text.len_bytes() {
            return None;
        }
        Some(region)
    }
    fn retains(&self, id: ChangeId) -> bool {
        id <= self.change
            && self
                .log
                .front()
                .map_or(id == self.change, |c| id.0 >= c.id.0 - 1)
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
        let Some(start) = self.undo.iter().rposition(|g| {
            g.steps
                .first()
                .is_some_and(|s| s.before_state == marker.state)
        }) else {
            return false;
        };
        let groups = self.undo.split_off(start);
        let mut group = Group {
            steps: Vec::new(),
            run: None,
            has_word: false,
        };
        for mut next in groups {
            group.steps.append(&mut next.steps);
        }
        self.undo.push_back(group);
        true
    }
    /// Replaces all history limits and immediately enforces them.
    pub fn set_history_limits(&mut self, mut limits: HistoryLimits) {
        limits.selection_entries = limits.selection_entries.max(256);
        self.limits = limits;
        // Limit changes are rare; bound selection redo here without walking it
        // on every typing transaction.
        for view in self.views.values_mut() {
            let excess = view
                .soft_redo
                .len()
                .saturating_sub(limits.selection_entries);
            view.soft_redo.drain(..excess);
        }
        self.prune();
    }
    /// Current history limits.
    pub fn history_limits(&self) -> HistoryLimits {
        self.limits
    }
    /// Number of transactions retained across undo and redo.
    pub fn history_entries(&self) -> usize {
        self.entries
    }
    /// Accounted retained history bytes (metadata and exclusive snapshot allocations).
    pub fn history_retained_bytes(&self) -> usize {
        self.retained
    }
    /// Metadata owned by the last undo transaction, including both selection arrays.
    /// Shared document storage is accounted separately by `history_retained_bytes()`.
    pub fn last_undo_metadata_bytes(&self) -> usize {
        self.undo
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
        mut new_selection: Selection,
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
        let changes = Arc::new(ChangeSet::new(
            edits
                .iter()
                .map(|e| Change {
                    old_range: e.range.clone(),
                    new_len: e.text.len(),
                })
                .collect(),
        ));
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
        let run = typing_run(kind, edits, &self.view(id).settings, &before_selection);
        let merge = self.group_open
            && self
                .undo
                .back()
                .is_some_and(|g| can_group(g, id, &before_selection, run, edits));
        self.clear_redo();
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
        let bytes = retained_text
            + changes.storage_bytes()
            + before_selection.storage_bytes()
            + new_selection.storage_bytes()
            + std::mem::size_of::<Step>()
            + 256;
        let step = Arc::new(Step {
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
            bytes,
        });
        self.text = after;
        self.state = state;
        self.publish(changes, Some(id));
        self.views.get_mut(&id).expect("attached view").selection = new_selection;
        if merge {
            let g = self.undo.back_mut().expect("merge group");
            g.steps.push(step.clone());
            g.run = run;
            g.has_word |= run == Some(Run::Word);
        } else {
            self.undo.push_back(Group {
                steps: vec![step.clone()],
                run,
                has_word: run == Some(Run::Word),
            });
        }
        self.group_open = run.is_some();
        self.entries += 1;
        self.retained += bytes;
        self.live.insert(state);
        let v = self.views.get_mut(&id).expect("attached view");
        v.soft.push_back(SoftEvent::Edit(step));
        v.soft_redo.clear();
        while v.soft.len() > self.limits.selection_entries {
            v.soft.pop_front();
        }
        self.prune();
        self.change
    }
    fn publish(&mut self, map: Arc<ChangeSet>, owner: Option<ViewId>) {
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
            for set in v.regions.values_mut() {
                for r in &mut set.regions {
                    *r = map.transform_region(*r);
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
        self.log_bytes += map.storage_bytes() + std::mem::size_of::<LoggedChange>();
        self.log.push_back(LoggedChange {
            id: self.change,
            map,
            old_len,
        });
        while self.log.len() > self.limits.changes || self.log_bytes > self.limits.bytes {
            if let Some(old) = self.log.pop_front() {
                self.log_bytes -= old.map.storage_bytes() + std::mem::size_of::<LoggedChange>();
            } else {
                break;
            }
        }
    }
    fn apply_step(&mut self, s: &Step, forward: bool) {
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
        self.publish(map, Some(s.owner));
        if let Some(v) = self.views.get_mut(&s.owner) {
            v.selection = selection.clone();
        }
    }
    /// Undoes the latest global edit group, restoring the originating view's selection.
    pub fn undo(&mut self, id: ViewId) -> bool {
        // ST-BEHAVIOR: unverified [H3]: undo/redo are buffer-global and restore the originating view; soft edit undo only proceeds at the current revision.
        self.view(id);
        self.break_undo_group();
        let Some(group) = self.undo.pop_back() else {
            return false;
        };
        for step in group.steps.iter().rev() {
            self.apply_step(step, false);
        }
        self.redo.push(group);
        true
    }
    /// Reapplies the most recently undone group.
    pub fn redo(&mut self, id: ViewId) -> bool {
        self.view(id);
        self.break_undo_group();
        let Some(group) = self.redo.pop() else {
            return false;
        };
        for step in &group.steps {
            self.apply_step(step, true);
        }
        self.undo.push_back(group);
        true
    }
    /// Undoes the latest live selection event or individual transaction of this view.
    pub fn soft_undo(&mut self, id: ViewId) -> bool {
        self.view(id);
        self.break_undo_group();
        loop {
            let Some(event) = self
                .views
                .get_mut(&id)
                .expect("attached view")
                .soft
                .pop_back()
            else {
                return false;
            };
            match &event {
                SoftEvent::Selection {
                    before, at, state, ..
                } => {
                    let selection = if *state == self.state {
                        Some(before.clone())
                    } else {
                        self.map_selection_from(before, *at)
                    };
                    let Some(selection) = selection else {
                        continue;
                    };
                    self.views.get_mut(&id).expect("view").selection = selection;
                }
                SoftEvent::Edit(step) => {
                    if self.state != step.after_state {
                        continue;
                    }
                    let Some(group) = self.undo.back_mut() else {
                        continue;
                    };
                    if !group.steps.last().is_some_and(|s| Arc::ptr_eq(s, step)) {
                        continue;
                    }
                    let step = group.steps.pop().expect("last step");
                    if group.steps.is_empty() {
                        self.undo.pop_back();
                    }
                    self.apply_step(&step, false);
                    self.redo.push(Group {
                        steps: vec![step],
                        run: None,
                        has_word: false,
                    });
                }
            }
            self.views.get_mut(&id).expect("view").soft_redo.push(event);
            return true;
        }
    }
    /// Reapplies the most recently soft-undone event, if still applicable.
    pub fn soft_redo(&mut self, id: ViewId) -> bool {
        self.view(id);
        self.break_undo_group();
        let Some(event) = self.views.get_mut(&id).expect("view").soft_redo.pop() else {
            return false;
        };
        match &event {
            SoftEvent::Selection {
                after, at, state, ..
            } => {
                let s = if *state == self.state {
                    Some(after.clone())
                } else {
                    self.map_selection_from(after, *at)
                };
                let Some(s) = s else {
                    return false;
                };
                self.views.get_mut(&id).expect("view").selection = s;
            }
            SoftEvent::Edit(step) => {
                if self.state != step.before_state
                    || !self
                        .redo
                        .last()
                        .is_some_and(|g| g.steps.len() == 1 && Arc::ptr_eq(&g.steps[0], step))
                {
                    return false;
                }
                self.redo.pop();
                self.apply_step(step, true);
                self.undo.push_back(Group {
                    steps: vec![step.clone()],
                    run: None,
                    has_word: false,
                });
            }
        }
        self.views.get_mut(&id).expect("view").soft.push_back(event);
        true
    }
    fn map_selection_from(&self, s: &Selection, at: ChangeId) -> Option<Selection> {
        let r: Option<Vec<_>> = s
            .iter()
            .map(|r| {
                Some(Region::new(
                    self.transform_point_from(r.a, at, Bias::Right)?,
                    self.transform_point_from(r.b, at, Bias::Right)?,
                ))
            })
            .collect();
        Some(Selection::from_regions(r?).with_primary(s.primary_index()))
    }
    fn clear_redo(&mut self) {
        let invalidated = !self.redo.is_empty();
        for g in self.redo.drain(..) {
            for s in g.steps {
                self.retained -= s.bytes;
                self.entries -= 1;
                self.live.remove(&s.after_state);
            }
        }
        for v in self.views.values_mut() {
            v.soft_redo.clear();
            if invalidated {
                v.soft.retain(|e| match e {
                    SoftEvent::Edit(s) => self.live.contains(&s.after_state),
                    _ => true,
                });
            }
        }
    }
    fn prune(&mut self) {
        // ST-BEHAVIOR: unverified [H4]: count transactions, evict complete oldest groups; mark_saved breaks groups; stale soft events are skipped.
        let mut evicted = false;
        while self.entries > self.limits.entries || self.retained > self.limits.bytes {
            let group = self.undo.pop_front().or_else(|| {
                if self.redo.is_empty() {
                    None
                } else {
                    Some(self.redo.remove(0))
                }
            });
            let Some(g) = group else {
                break;
            };
            evicted = true;
            for s in g.steps {
                self.retained -= s.bytes;
                self.entries -= 1;
                self.live.remove(&s.after_state);
            }
        }
        for v in self.views.values_mut() {
            // Live edit events cannot become stale without eviction or redo
            // invalidation. Avoid visiting the entire soft history on each key;
            // selection-only redo is still cleared by every new transaction.
            if evicted {
                v.soft.retain(|e| match e {
                    SoftEvent::Edit(s) => self.live.contains(&s.after_state),
                    _ => true,
                });
                v.soft_redo.retain(|e| match e {
                    SoftEvent::Edit(s) => self
                        .redo
                        .iter()
                        .any(|g| g.steps.iter().any(|x| Arc::ptr_eq(s, x))),
                    _ => true,
                });
            }
            while v.soft.len() > self.limits.selection_entries {
                v.soft.pop_front();
            }
        }
        while self.log.len() > self.limits.changes || self.log_bytes > self.limits.bytes {
            if let Some(old) = self.log.pop_front() {
                self.log_bytes -= old.map.storage_bytes() + std::mem::size_of::<LoggedChange>();
            } else {
                break;
            }
        }
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
