//! View-local fold reveals observed at the existing committed-state publication hook.
use std::collections::{BTreeMap, HashSet};
use vel_buffer::{Buffer, BufferId, ChangeId, Region, Selection, ViewId};
#[derive(Debug, Default)]
struct State {
    identity: Option<(BufferId, ChangeId)>,
    selection: Option<Selection>,
    revealed: HashSet<(usize, usize)>,
    visible: Vec<Region>,
}
#[derive(Debug, Default)]
pub(super) struct FoldStates(BTreeMap<ViewId, State>);
impl FoldStates {
    pub fn forget(&mut self, view: ViewId) {
        self.0.remove(&view);
    }
    pub fn observe(&mut self, buffer: &Buffer) {
        self.0.retain(|id, state| {
            state.identity.is_some_and(|(b, _)| b != buffer.id())
                || buffer.views().any(|v| v == *id)
        });
        for id in buffer.views() {
            self.update(buffer, id);
        }
    }
    fn update(&mut self, buffer: &Buffer, id: ViewId) {
        let Some(set) = buffer.view(id).get_regions(crate::layout::FOLDS) else {
            self.0.remove(&id);
            return;
        };
        if set.regions.is_empty() {
            self.0.remove(&id);
            return;
        }
        let state = self.0.entry(id).or_default();
        let identity = (buffer.id(), buffer.change_id());
        let selection = buffer.view(id).sel();
        // Fold installation and session restore explicitly call reset_folds,
        // including same-version replacements. Unchanged frames stay O(1).
        if state.identity == Some(identity) && state.selection.as_ref() == Some(selection) {
            return;
        }
        if let Some((old, version)) = state.identity {
            if old != identity.0 {
                state.revealed.clear();
            } else if version != identity.1 {
                if let Some(changes) = buffer.changes_since(version) {
                    for (_, changes) in changes {
                        state.revealed = state
                            .revealed
                            .iter()
                            .map(|&(a, b)| {
                                let r = changes.transform_named_region(
                                    crate::layout::FOLDS,
                                    Region::new(a, b),
                                );
                                (r.begin(), r.end())
                            })
                            .collect();
                    }
                } else {
                    state.revealed.clear();
                }
            }
        }
        state.identity = Some(identity);
        state.selection = Some(selection.clone());
        // Selection regions are sorted/disjoint. Their endpoints and fold starts
        // advance together, including nested folds: O(folds + selections).
        let mut endpoints = selection
            .iter()
            .flat_map(|r| [r.begin(), r.end()])
            .peekable();
        let mut revealed = HashSet::with_capacity(state.revealed.len());
        state.visible.clear();
        for fold in &set.regions {
            while endpoints.peek().is_some_and(|p| *p <= fold.begin()) {
                endpoints.next();
            }
            let key = (fold.begin(), fold.end());
            if state.revealed.contains(&key) || endpoints.peek().is_some_and(|p| *p < fold.end()) {
                revealed.insert(key);
            } else {
                state.visible.push(*fold);
            }
        }
        state.revealed = revealed;
    }
    pub fn visible(&mut self, buffer: &Buffer, id: ViewId) -> Vec<Region> {
        self.update(buffer, id);
        self.0
            .get(&id)
            .map_or_else(Vec::new, |state| state.visible.clone())
    }
}
/// The opt-in indentation-only mode must compute its own candidates. Normal
/// analysis suppresses an indent fold if a syntax fold claims the same header.
pub(crate) fn indentation_folds(
    editor: &crate::editor::Editor,
) -> Option<Vec<crate::language::Fold>> {
    if editor.settings.string("fold_style", "auto") != "indentation" {
        return None;
    }
    let text = &editor.text;
    let tab = editor.preferences.tab_size;
    let mut folds = Vec::new();
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut previous = None;
    let mut add = |header, last, level| {
        let start = text.line_to_byte(header + 1) - 1;
        let end = text.line_to_byte(last) + text.line_len_bytes(last);
        if start < end {
            folds.push(crate::language::Fold {
                range: start..end,
                level,
                attributes: false,
            });
        }
    };
    for row in 0..text.len_lines() {
        let line = text.line(row).to_string();
        if line.trim().is_empty() {
            continue;
        }
        let indent = vel_syntax::indent::indentation(&line, tab);
        while stack.last().is_some_and(|(level, _)| *level >= indent) {
            let level = stack.len();
            let (_, header) = stack.pop().expect("indentation stack");
            if let Some((_, last)) = previous {
                add(header, last, level);
            }
        }
        if let Some((prior, header)) = previous
            && indent > prior
        {
            stack.push((prior, header));
        }
        previous = Some((indent, row));
    }
    if let Some((_, last)) = previous {
        for (level, (_, header)) in stack.into_iter().enumerate() {
            add(header, last, level + 1);
        }
    }
    folds.sort_unstable_by_key(|fold| fold.range.start);
    Some(folds)
}

/// D4145 excludes the closing newline by default. The start newline stays in
/// the fold, and explicit user selections and tag attributes are untouched.
pub(crate) fn code_fold_range(
    text: &vel_text::Text,
    fold: &crate::language::Fold,
    include_newline: bool,
) -> Option<std::ops::Range<usize>> {
    let mut range = fold.range.clone();
    if range.end > text.len_bytes() {
        return None;
    }
    if !include_newline
        && !fold.attributes
        && range.len() > 1
        && text.byte_at(range.end - 1) == b'\n'
    {
        range.end -= 1;
    }
    (range.start < range.end).then_some(range)
}
