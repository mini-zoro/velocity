//! Text-level command semantics. Each edit command commits one pre-coordinate batch.
use crate::buffer::FindState;
use crate::{
    Bias, Buffer, Change, ChangeId, ChangeSet, EditKind, Region, Selection, Settings,
    VerticalLayout, ViewId,
};
use std::{collections::BTreeSet, ops::Range};
use vel_text::{Edit, Text};

/// Units accepted by character deletion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteBy {
    /// One extended grapheme cluster.
    Char,
}
/// Class-boundary stops used by configurable movement.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stops {
    /// Stop at a word run's start.
    pub word_start: bool,
    /// Stop at a word run's end.
    pub word_end: bool,
    /// Stop at punctuation boundaries.
    pub punctuation: bool,
    /// Stop before/after LF.
    pub newline: bool,
    /// Stop at camel-case and underscore boundaries.
    pub subword: bool,
}
/// Horizontal or layout-driven movement unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveUnit {
    /// Extended grapheme clusters.
    Characters,
    /// Character-class boundaries.
    Words,
    /// Ends of non-whitespace class runs.
    WordEnds,
    /// Class, lowercase-to-uppercase and underscore boundaries.
    Subwords,
    /// Subword ends, skipping leading horizontal whitespace.
    SubwordEnds,
    /// Configurable boundary set.
    Stops(Stops),
    /// One LF line through a supplied layout.
    Lines,
    /// One layout page.
    Pages,
}
/// Absolute movement destinations. Syntax-dependent brackets are intentionally absent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveTo {
    /// Column zero.
    Bol,
    /// End before LF.
    Eol,
    /// Toggle first non-horizontal-whitespace / column zero.
    HardBol,
    /// End before LF, including trailing horizontal whitespace.
    HardEol,
    /// Start of buffer.
    Bof,
    /// End of buffer.
    Eof,
}
/// Syntax-independent expansion target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpandTo {
    /// Character-class run under the caret.
    Word,
    /// Complete touched LF lines.
    Line,
    /// Contiguous indentation block.
    Indentation,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Word,
    Whitespace,
    Punctuation,
    Newline,
}
fn class(c: char, s: &Settings) -> Class {
    if c == '\n' {
        Class::Newline
    } else if c.is_whitespace() {
        Class::Whitespace
    } else if s.word_separators.contains(c) {
        Class::Punctuation
    } else {
        Class::Word
    }
}
fn ch(text: &Text, at: usize) -> Option<char> {
    (at < text.len_bytes()).then(|| text.char_at(at))
}
fn prev(text: &Text, at: usize) -> Option<char> {
    (at > 0).then(|| text.char_at(text.prev_grapheme_boundary(at)))
}
fn line_end(text: &Text, row: usize) -> usize {
    text.line_to_byte(row) + text.line_len_bytes(row)
}
fn first_nonblank(text: &Text, row: usize) -> usize {
    let start = text.line_to_byte(row);
    let mut at = start;
    for c in text.line(row).chars() {
        if c != ' ' && c != '\t' {
            break;
        }
        at += c.len_utf8();
    }
    at
}
fn sub_boundary(left: char, right: char) -> bool {
    left == '_' || right == '_' || (left.is_lowercase() && right.is_uppercase())
}
fn boundary(text: &Text, at: usize, s: &Settings, stops: Option<Stops>, sub: bool) -> bool {
    let (Some(a), Some(b)) = (prev(text, at), ch(text, at)) else {
        return true;
    };
    let ac = class(a, s);
    let bc = class(b, s);
    if let Some(t) = stops {
        return (t.word_start && bc == Class::Word && ac != bc)
            || (t.word_end && ac == Class::Word && ac != bc)
            || (t.punctuation
                && ac != bc
                && (ac == Class::Punctuation || bc == Class::Punctuation))
            || (t.newline && (a == '\n' || b == '\n'))
            || (t.subword && sub_boundary(a, b));
    }
    ac != bc || (sub && sub_boundary(a, b)) || a == '\n' || b == '\n'
}
fn walk(text: &Text, mut at: usize, unit: MoveUnit, forward: bool, s: &Settings) -> usize {
    if unit == MoveUnit::Characters {
        return if forward {
            text.next_grapheme_boundary(at)
        } else {
            text.prev_grapheme_boundary(at)
        };
    }
    // ST-BEHAVIOR: unverified [W1]: word-ends skip horizontal whitespace; LF is always a distinct stop.
    if matches!(unit, MoveUnit::WordEnds | MoveUnit::SubwordEnds) {
        while if forward {
            ch(text, at)
        } else {
            prev(text, at)
        }
        .is_some_and(|c| class(c, s) == Class::Whitespace)
        {
            let next = if forward {
                text.next_grapheme_boundary(at)
            } else {
                text.prev_grapheme_boundary(at)
            };
            if next == at {
                break;
            }
            at = next;
        }
    }
    let sub = matches!(unit, MoveUnit::Subwords | MoveUnit::SubwordEnds);
    let stops = if let MoveUnit::Stops(stops) = unit {
        Some(stops)
    } else {
        None
    };
    loop {
        let next = if forward {
            text.next_grapheme_boundary(at)
        } else {
            text.prev_grapheme_boundary(at)
        };
        if next == at {
            return at;
        }
        at = next;
        if boundary(text, at, s, stops, sub) {
            return at;
        }
    }
}
fn word_at(text: &Text, at: usize, s: &Settings) -> Region {
    if text.is_empty() {
        return Region::caret(0);
    }
    // ST-BEHAVIOR: unverified [W2]: EOF uses the preceding grapheme; punctuation/whitespace expand as class runs.
    let at = if at == text.len_bytes() {
        text.prev_grapheme_boundary(at)
    } else {
        at
    };
    let c = class(text.char_at(at), s);
    if c == Class::Newline {
        return Region::new(at, text.next_grapheme_boundary(at));
    }
    let mut a = at;
    let mut b = text.next_grapheme_boundary(at);
    while a > 0 && prev(text, a).is_some_and(|x| class(x, s) == c) {
        a = text.prev_grapheme_boundary(a);
    }
    while b < text.len_bytes() && ch(text, b).is_some_and(|x| class(x, s) == c) {
        b = text.next_grapheme_boundary(b);
    }
    Region::new(a, b)
}
fn replacements(buffer: &mut Buffer, id: ViewId, values: Vec<String>, kind: EditKind) -> ChangeId {
    let selection = buffer.view(id).sel().clone();
    assert_eq!(values.len(), selection.len());
    let edits: Vec<_> = selection
        .iter()
        .zip(&values)
        .map(|(r, s)| Edit {
            range: r.to_range(),
            text: s.as_str(),
        })
        .collect();
    let mut old = 0;
    let mut new = 0;
    let mut regions = Vec::with_capacity(edits.len());
    for e in &edits {
        new += e.range.start - old;
        new += e.text.len();
        regions.push(Region::caret(new));
        old = e.range.end;
    }
    let after = Selection::from_regions(regions).with_primary(selection.primary_index());
    buffer.transact(id, kind, &edits, after)
}
/// Replaces each selection; carets land after the inserted text.
pub fn insert(buffer: &mut Buffer, id: ViewId, text: &str) -> ChangeId {
    replacements(
        buffer,
        id,
        vec![text.to_owned(); buffer.view(id).sel().len()],
        EditKind::InsertChars,
    )
}
/// Pastes per-line when the number of LF-delimited lines equals the selection count.
pub fn paste(buffer: &mut Buffer, id: ViewId, text: &str) -> ChangeId {
    // ST-BEHAVIOR: unverified [E1]: split on LF, preserving an empty final field and literal CR.
    let count = buffer.view(id).sel().len();
    let lines: Vec<_> = text.split('\n').collect();
    let values = if count > 1 && lines.len() == count {
        lines.into_iter().map(str::to_owned).collect()
    } else {
        vec![text.to_owned(); count]
    };
    replacements(buffer, id, values, EditKind::Paste)
}
fn delete(buffer: &mut Buffer, id: ViewId, forward: bool, word: bool) -> ChangeId {
    let text = buffer.text();
    let s = &buffer.view(id).settings;
    let selection = buffer.view(id).sel().clone();
    let mut ranges = Vec::with_capacity(selection.len());
    for r in selection.iter() {
        if !r.empty() {
            ranges.push(r);
            continue;
        }
        let mut to = if word {
            walk(text, r.b, MoveUnit::Words, forward, s)
        } else if forward {
            text.next_grapheme_boundary(r.b)
        } else {
            text.prev_grapheme_boundary(r.b)
        };
        if !forward && !word && s.translate_tabs_to_spaces {
            let row = text.byte_to_line(r.b);
            let start = text.line_to_byte(row);
            let prefix = text.slice(start..r.b);
            let mut column = 0;
            let mut leading = true;
            for c in prefix.chars() {
                match c {
                    ' ' => column += 1,
                    '\t' => column += s.tab_size - column % s.tab_size,
                    _ => {
                        leading = false;
                        break;
                    }
                }
            }
            if !prefix.is_empty() && leading {
                let target = (column - 1) / s.tab_size * s.tab_size;
                let mut x = 0;
                to = start;
                for c in prefix.chars() {
                    let next = if c == '\t' {
                        x + s.tab_size - x % s.tab_size
                    } else {
                        x + 1
                    };
                    if next > target {
                        break;
                    }
                    to += c.len_utf8();
                    x = next;
                }
            }
        }
        ranges.push(Region::new(r.b, to));
    }
    // Overlapping deletion requests coalesce, so a grapheme is never removed twice.
    let deletion = Selection::from_regions(ranges);
    let edits: Vec<_> = deletion
        .iter()
        .filter(|r| !r.empty())
        .map(|r| Edit {
            range: r.to_range(),
            text: "",
        })
        .collect();
    let map = ChangeSet::new(
        edits
            .iter()
            .map(|e| Change {
                old_range: e.range.clone(),
                new_len: 0,
            })
            .collect(),
    );
    let after = map.transform_selection(&selection);
    buffer.transact(id, EditKind::Delete, &edits, after)
}
/// Deletes selections or one grapheme to the left, honoring leading-space tab stops.
pub fn delete_left(buffer: &mut Buffer, id: ViewId, _by: DeleteBy) -> ChangeId {
    delete(buffer, id, false, false)
}
/// Deletes selections or one grapheme to the right.
pub fn delete_right(buffer: &mut Buffer, id: ViewId, _by: DeleteBy) -> ChangeId {
    delete(buffer, id, true, false)
}
/// Deletes through the next/previous character-class boundary.
pub fn delete_word(buffer: &mut Buffer, id: ViewId, forward: bool) -> ChangeId {
    delete(buffer, id, forward, true)
}
/// Moves every caret; vertical units require a layout and preserve each goal column.
pub fn move_by(
    buffer: &mut Buffer,
    id: ViewId,
    unit: MoveUnit,
    forward: bool,
    extend: bool,
    layout: Option<&dyn VerticalLayout>,
) {
    let text = buffer.text();
    let v = buffer.view(id);
    let vertical = matches!(unit, MoveUnit::Lines | MoveUnit::Pages);
    let selection = v.sel().map(|r| {
        let mut goal = None;
        let b = if vertical {
            let layout = layout.expect("vertical movement requires a layout");
            let x = r.xpos.unwrap_or_else(|| layout.x_of(text, r.b));
            goal = Some(x);
            let delta = if unit == MoveUnit::Pages {
                layout.lines_per_page().max(1)
            } else {
                1
            };
            let row = text.byte_to_line(r.b);
            let row = if forward {
                row.saturating_add(delta).min(text.len_lines() - 1)
            } else {
                row.saturating_sub(delta)
            };
            layout.offset_at(text, row, x)
        } else if !extend && !r.empty() {
            if forward { r.end() } else { r.begin() }
        } else {
            walk(text, r.b, unit, forward, &v.settings)
        };
        Region {
            a: if extend { r.a } else { b },
            b,
            xpos: goal,
        }
    });
    buffer.set_selection(id, selection);
}
/// Moves to an absolute destination, optionally extending from the anchor.
pub fn move_to(buffer: &mut Buffer, id: ViewId, target: MoveTo, extend: bool) {
    // ST-BEHAVIOR: unverified [W3]: Bol is column zero; HardBol toggles indentation; Eol/HardEol coincide without layout wrapping.
    let text = buffer.text();
    let v = buffer.view(id);
    let selection = v.sel().map(|r| {
        let row = text.byte_to_line(r.b);
        let b = match target {
            MoveTo::Bol => text.line_to_byte(row),
            MoveTo::HardBol => {
                let first = first_nonblank(text, row);
                if r.b == first {
                    text.line_to_byte(row)
                } else {
                    first
                }
            }
            MoveTo::Eol | MoveTo::HardEol => line_end(text, row),
            MoveTo::Bof => 0,
            MoveTo::Eof => text.len_bytes(),
        };
        Region::new(if extend { r.a } else { b }, b)
    });
    buffer.set_selection(id, selection);
}
fn touched_lines(text: &Text, selection: &Selection) -> BTreeSet<usize> {
    // ST-BEHAVIOR: unverified [L1]: a nonempty region ending at the next BOL excludes that next line.
    let mut lines = BTreeSet::new();
    for r in selection.iter() {
        let a = text.byte_to_line(r.begin());
        let mut b = text.byte_to_line(r.end());
        if !r.empty() && b > a && text.line_to_byte(b) == r.end() {
            b -= 1;
        }
        lines.extend(a..=b);
    }
    lines
}
fn full_line_end(text: &Text, row: usize) -> usize {
    if row + 1 < text.len_lines() {
        text.line_to_byte(row + 1)
    } else {
        text.len_bytes()
    }
}
/// Expands selections to words, complete lines, or contiguous indentation blocks.
pub fn expand_selection(buffer: &mut Buffer, id: ViewId, to: ExpandTo) {
    let text = buffer.text();
    let v = buffer.view(id);
    let selection = v.sel().map(|r| match to {
        ExpandTo::Word => r.directed(
            word_at(text, r.begin(), &v.settings).begin(),
            word_at(
                text,
                if r.empty() {
                    r.b
                } else {
                    text.prev_grapheme_boundary(r.end())
                },
                &v.settings,
            )
            .end(),
        ),
        ExpandTo::Line => {
            let lines = touched_lines(text, &Selection::from(r));
            r.directed(
                text.line_to_byte(*lines.first().expect("line")),
                full_line_end(text, *lines.last().expect("line")),
            )
        }
        ExpandTo::Indentation => {
            // ST-BEHAVIOR: unverified [L2]: include adjacent nonblank lines with indentation >= the seed, plus blank lines between them.
            let row = text.byte_to_line(r.b);
            let indent = first_nonblank(text, row) - text.line_to_byte(row);
            let mut a = row;
            let mut b = row;
            while a > 0 {
                let n = a - 1;
                if !text.line(n).is_empty()
                    && first_nonblank(text, n) - text.line_to_byte(n) < indent
                {
                    break;
                }
                a = n;
            }
            while b + 1 < text.len_lines() {
                let n = b + 1;
                if !text.line(n).is_empty()
                    && first_nonblank(text, n) - text.line_to_byte(n) < indent
                {
                    break;
                }
                b = n;
            }
            r.directed(text.line_to_byte(a), full_line_end(text, b))
        }
    });
    buffer.set_selection(id, selection);
}
/// Keeps only the interactively primary selection.
pub fn single_selection(buffer: &mut Buffer, id: ViewId) {
    let r = buffer.view(id).sel().primary().unwrap_or_default();
    buffer.set_selection(id, Selection::from(r));
}
/// Splits each selection at LF boundaries, omitting the LF from the pieces.
pub fn split_selection_into_lines(buffer: &mut Buffer, id: ViewId) {
    // ST-BEHAVIOR: unverified [L3]: keep empty lines; a selection ending at BOL adds no extra caret; preserve direction.
    let text = buffer.text();
    let v = buffer.view(id);
    let mut regions = Vec::new();
    for r in v.sel().iter() {
        for line in touched_lines(text, &Selection::from(r)) {
            let a = r.begin().max(text.line_to_byte(line));
            let b = r.end().min(line_end(text, line));
            regions.push(r.directed(a, b.max(a)));
        }
    }
    buffer.set_selection(id, Selection::from_regions(regions));
}
/// Adds a caret on the neighboring LF line for each selected caret, at its goal column.
pub fn select_lines(buffer: &mut Buffer, id: ViewId, forward: bool, layout: &dyn VerticalLayout) {
    let text = buffer.text();
    let mut selection = buffer.view(id).sel().clone();
    let source: Vec<_> = selection.iter().collect();
    for r in source {
        let row = text.byte_to_line(r.b);
        let next = if forward {
            (row + 1).min(text.len_lines() - 1)
        } else {
            row.saturating_sub(1)
        };
        if next == row {
            continue;
        }
        let x = r.xpos.unwrap_or_else(|| layout.x_of(text, r.b));
        let p = layout.offset_at(text, next, x);
        selection.add(Region {
            a: p,
            b: p,
            xpos: Some(x),
        });
    }
    buffer.set_selection(id, selection);
}
fn match_at(text: &Text, range: Range<usize>, whole: bool, s: &Settings) -> bool {
    !whole
        || (prev(text, range.start).is_none_or(|c| class(c, s) != Class::Word)
            && ch(text, range.end).is_none_or(|c| class(c, s) != Class::Word))
}
fn find_next(
    text: &Text,
    needle: &str,
    start: usize,
    end: usize,
    whole: bool,
    s: &Settings,
    selected: Option<&Selection>,
) -> Option<Region> {
    if needle.is_empty() || end - start < needle.len() {
        return None;
    }
    let finder = memchr::memmem::Finder::new(needle.as_bytes());
    let mut carry = Vec::new();
    let mut offset = start;
    for chunk in text.slice(start..end).chunks() {
        if !carry.is_empty() {
            let n = (needle.len() - 1).min(chunk.len());
            let mut bridge = carry.clone();
            bridge.extend_from_slice(&chunk.as_bytes()[..n]);
            for at in finder.find_iter(&bridge) {
                let p = offset - carry.len() + at;
                if p < offset && p + needle.len() > offset {
                    let r = Region::new(p, p + needle.len());
                    if match_at(text, r.to_range(), whole, s)
                        && selected.is_none_or(|s| !s.contains(r))
                    {
                        return Some(r);
                    }
                }
            }
        }
        for at in finder.find_iter(chunk.as_bytes()) {
            let r = Region::new(offset + at, offset + at + needle.len());
            if match_at(text, r.to_range(), whole, s) && selected.is_none_or(|s| !s.contains(r)) {
                return Some(r);
            }
        }
        let keep = needle.len().saturating_sub(1);
        if chunk.len() >= keep {
            carry.clear();
            carry.extend_from_slice(&chunk.as_bytes()[chunk.len() - keep..]);
        } else {
            carry.extend_from_slice(chunk.as_bytes());
            if carry.len() > keep {
                carry.drain(..carry.len() - keep);
            }
        }
        offset += chunk.len();
    }
    None
}
fn find_seed(buffer: &Buffer, id: ViewId) -> (String, bool, Region) {
    let v = buffer.view(id);
    let r = v.sel().primary().unwrap_or_default();
    if r.empty() {
        let r = word_at(buffer.text(), r.b, &v.settings);
        (buffer.text().slice(r.to_range()).to_string(), true, r)
    } else {
        let needle = buffer.text().slice(r.to_range()).to_string();
        let whole = v
            .find
            .as_ref()
            .is_some_and(|f| f.at == buffer.change_id() && f.needle == needle && f.whole_word);
        (needle, whole, r)
    }
}
/// Selects a word at an empty caret, or adds the next matching occurrence incrementally.
/// Returns false once every occurrence is already selected.
pub fn find_under_expand(buffer: &mut Buffer, id: ViewId) -> bool {
    let (needle, whole, seed) = find_seed(buffer, id);
    if needle.is_empty() {
        return false;
    }
    let v = buffer.view(id);
    if v.sel().primary().is_some_and(Region::empty) {
        buffer.set_selection(id, Selection::from(seed));
        buffer.view_mut(id).find = Some(FindState {
            needle,
            whole_word: whole,
            next: seed.end(),
            at: buffer.change_id(),
        });
        return true;
    }
    let start = v
        .find
        .as_ref()
        .filter(|f| f.at == buffer.change_id() && f.needle == needle)
        .map_or(seed.end(), |f| f.next);
    let found = find_next(
        buffer.text(),
        &needle,
        start,
        buffer.text().len_bytes(),
        whole,
        &v.settings,
        Some(v.sel()),
    )
    .or_else(|| {
        find_next(
            buffer.text(),
            &needle,
            0,
            start,
            whole,
            &v.settings,
            Some(v.sel()),
        )
    });
    let Some(r) = found else {
        return false;
    };
    let mut selection = v.sel().clone();
    selection.add(r);
    buffer.set_selection(id, selection);
    buffer.view_mut(id).find = Some(FindState {
        needle,
        whole_word: whole,
        next: r.end(),
        at: buffer.change_id(),
    });
    true
}
/// Selects every non-overlapping literal occurrence of the primary selection/word.
pub fn find_all_under(buffer: &mut Buffer, id: ViewId) {
    // ST-BEHAVIOR: unverified [F1]: case-sensitive literal non-overlapping matches; caret seeds enable whole-word, selected seeds do not.
    let (needle, whole, _) = find_seed(buffer, id);
    if needle.is_empty() {
        return;
    }
    let mut next = 0;
    let mut regions = Vec::new();
    while let Some(r) = find_next(
        buffer.text(),
        &needle,
        next,
        buffer.text().len_bytes(),
        whole,
        &buffer.view(id).settings,
        None,
    ) {
        next = r.end();
        regions.push(r);
    }
    if !regions.is_empty() {
        buffer.set_selection(id, Selection::from_regions(regions));
        buffer.view_mut(id).find = Some(FindState {
            needle,
            whole_word: whole,
            next,
            at: buffer.change_id(),
        });
    }
}
fn commit_owned(
    buffer: &mut Buffer,
    id: ViewId,
    mut owned: Vec<(Range<usize>, String)>,
) -> ChangeId {
    // Keep unchanged columns and anchors stable when a line command only adds/removes a prefix.
    // ST-BEHAVIOR: unverified [E2]: commands reduce replacements to their changed scalar-aligned span.
    for (range, value) in &mut owned {
        let old = buffer.text().slice(range.clone()).to_string();
        let common = old
            .bytes()
            .zip(value.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        let prefix = old.floor_char_boundary(common);
        let mut suffix = old.as_bytes()[prefix..]
            .iter()
            .rev()
            .zip(value.as_bytes()[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        while !old.is_char_boundary(old.len() - suffix)
            || !value.is_char_boundary(value.len() - suffix)
        {
            suffix -= 1;
        }
        range.start += prefix;
        range.end -= suffix;
        *value = value[prefix..value.len() - suffix].to_owned();
    }
    owned.sort_by_key(|(r, _)| r.start);
    let edits: Vec<_> = owned
        .iter()
        .map(|(range, text)| Edit {
            range: range.clone(),
            text,
        })
        .collect();
    let map = ChangeSet::new(
        edits
            .iter()
            .map(|e| Change {
                old_range: e.range.clone(),
                new_len: e.text.len(),
            })
            .collect(),
    );
    let before = buffer.view(id).sel();
    // ST-BEHAVIOR: unverified [E2]: transform commands retain nonempty selections over their replacement; interior carets collapse to the replacement start.
    let selection = before.map(|r| {
        if r.empty() {
            Region::caret(map.transform_point(r.b, Bias::Right))
        } else {
            r.directed(
                map.transform_point(r.begin(), Bias::Left),
                map.transform_point(r.end(), Bias::Right),
            )
        }
    });
    buffer.transact(id, EditKind::Other, &edits, selection)
}
fn line_edits(
    buffer: &mut Buffer,
    id: ViewId,
    mut f: impl FnMut(&Text, usize, &Settings) -> String,
) -> ChangeId {
    let text = buffer.text();
    let v = buffer.view(id);
    let lines = touched_lines(text, v.sel());
    let edits = lines
        .into_iter()
        .map(|line| {
            (
                text.line_to_byte(line)..line_end(text, line),
                f(text, line, &v.settings),
            )
        })
        .collect();
    commit_owned(buffer, id, edits)
}
/// Indents every touched line once, using a tab or `tab_size` spaces.
pub fn indent(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    line_edits(buffer, id, |t, l, s| {
        let prefix = if s.translate_tabs_to_spaces {
            " ".repeat(s.tab_size)
        } else {
            "\t".into()
        };
        prefix + &t.line(l).to_string()
    })
}
/// Removes one leading tab or up to `tab_size` spaces from each touched line.
pub fn unindent(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    line_edits(buffer, id, |t, l, s| {
        let line = t.line(l).to_string();
        let n = if line.starts_with('\t') {
            1
        } else {
            line.bytes()
                .take(s.tab_size)
                .take_while(|&b| b == b' ')
                .count()
        };
        line[n..].to_owned()
    })
}
/// Removes spaces/tabs/CR and other Unicode horizontal whitespace at line ends.
pub fn trim_trailing_whitespace(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    // ST-BEHAVIOR: unverified [L4]: trim Unicode whitespace other than LF, including literal CR.
    line_edits(buffer, id, |t, l, _| {
        t.line(l)
            .to_string()
            .trim_end_matches(char::is_whitespace)
            .to_owned()
    })
}
fn blocks(lines: BTreeSet<usize>) -> Vec<Range<usize>> {
    let mut result: Vec<Range<usize>> = Vec::new();
    for l in lines {
        if let Some(last) = result.last_mut()
            && last.end == l
        {
            last.end += 1;
        } else {
            result.push(l..l + 1);
        }
    }
    result
}
fn reorder(buffer: &mut Buffer, id: ViewId, sort: bool) -> ChangeId {
    // ST-BEHAVIOR: unverified [L5]: reorder contiguous touched blocks independently, preserve terminal LF, sort by Rust Unicode scalar order.
    let text = buffer.text();
    let groups = blocks(touched_lines(text, buffer.view(id).sel()));
    let edits = groups
        .into_iter()
        .map(|g| {
            let mut lines: Vec<_> = g.clone().map(|l| text.line(l).to_string()).collect();
            if sort {
                lines.sort();
            } else {
                lines.reverse();
            }
            let end = full_line_end(text, g.end - 1);
            let mut replacement = lines.join("\n");
            if g.end < text.len_lines() {
                replacement.push('\n');
            }
            (text.line_to_byte(g.start)..end, replacement)
        })
        .collect();
    commit_owned(buffer, id, edits)
}
/// Sorts each contiguous block of touched lines.
pub fn sort_lines(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    reorder(buffer, id, true)
}
/// Reverses each contiguous block of touched lines.
pub fn reverse_lines(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    reorder(buffer, id, false)
}
/// Joins touched lines, or a caret's line with its successor, inserting one space.
pub fn join_lines(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    // ST-BEHAVIOR: unverified [L6]: trim horizontal whitespace around joins, insert one ASCII space; the last selected line is not joined onward.
    let text = buffer.text();
    let selection = buffer.view(id).sel();
    let mut joins = BTreeSet::new();
    for r in selection.iter() {
        let lines = touched_lines(text, &Selection::from(r));
        let a = *lines.first().expect("line");
        let b = *lines.last().expect("line");
        if a == b {
            if a + 1 < text.len_lines() {
                joins.insert(a);
            }
        } else {
            joins.extend(a..b);
        }
    }
    let mut edits = Vec::new();
    for group in blocks(joins) {
        let mut value = String::new();
        for line in group.start..=group.end {
            let part = text.line(line).to_string();
            let part = if line > group.start {
                part.trim_start_matches([' ', '\t'])
            } else {
                part.as_str()
            };
            let part = if line < group.end {
                part.trim_end_matches([' ', '\t'])
            } else {
                part
            };
            if line > group.start {
                value.push(' ');
            }
            value.push_str(part);
        }
        edits.push((
            text.line_to_byte(group.start)..line_end(text, group.end),
            value,
        ));
    }
    commit_owned(buffer, id, edits)
}
/// Duplicates each touched line once, including its LF when present.
pub fn duplicate_line(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    // ST-BEHAVIOR: unverified [L7]: duplicate whole touched lines; unterminated last line gets an LF between copies; carets follow the original text through insertion bias.
    let text = buffer.text();
    let lines = touched_lines(text, buffer.view(id).sel());
    let edits = lines
        .into_iter()
        .map(|l| {
            let start = text.line_to_byte(l);
            let end = full_line_end(text, l);
            let mut s = text.slice(start..end).to_string();
            if end == line_end(text, l) {
                s.insert(0, '\n');
            }
            (end..end, s)
        })
        .collect();
    commit_owned(buffer, id, edits)
}
fn swap_lines(buffer: &mut Buffer, id: ViewId, forward: bool) -> ChangeId {
    // ST-BEHAVIOR: unverified [L8]: move each touched block one LF line, preserve line termination and relative caret columns/direction; edge blocks stay put.
    let text = buffer.text();
    let selection = buffer.view(id).sel().clone();
    let groups = blocks(touched_lines(text, &selection));
    let mut edits = Vec::new();
    let mut shifts = Vec::new();
    for g in groups {
        if (!forward && g.start == 0) || (forward && g.end == text.len_lines()) {
            continue;
        }
        let a = if forward { g.start } else { g.start - 1 };
        let b = if forward { g.end + 1 } else { g.end };
        let mut lines: Vec<_> = (a..b).map(|l| text.line(l).to_string()).collect();
        if forward {
            lines.rotate_right(1);
        } else {
            lines.rotate_left(1);
        }
        let end = full_line_end(text, b - 1);
        let mut value = lines.join("\n");
        if b < text.len_lines() {
            value.push('\n');
        }
        shifts.push((g, if forward { 1isize } else { -1 }));
        edits.push((text.line_to_byte(a)..end, value));
    }
    let owned = edits;
    let edits: Vec<_> = owned
        .iter()
        .map(|(range, text)| Edit {
            range: range.clone(),
            text,
        })
        .collect();
    let mut after = text.clone();
    after.apply_edits(&edits);
    let new = selection.map(|r| {
        let map = |p| {
            let mut row = text.byte_to_line(p);
            let full_end = !r.empty() && p == r.end() && row > 0 && p == text.line_to_byte(row);
            if full_end {
                row -= 1;
            }
            let column = p - text.line_to_byte(row);
            let shift = shifts
                .iter()
                .find(|(g, _)| g.contains(&row))
                .map_or(0, |(_, d)| *d);
            let row = row.saturating_add_signed(shift);
            if full_end {
                full_line_end(&after, row)
            } else {
                after.line_to_byte(row) + column.min(after.line_len_bytes(row))
            }
        };
        Region::new(map(r.a), map(r.b))
    });
    buffer.transact(id, EditKind::Other, &edits, new)
}
/// Moves each touched block one line upward.
pub fn swap_line_up(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    swap_lines(buffer, id, false)
}
/// Moves each touched block one line downward.
pub fn swap_line_down(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    swap_lines(buffer, id, true)
}
fn case(buffer: &mut Buffer, id: ViewId, mode: u8) -> ChangeId {
    // ST-BEHAVIOR: unverified [C1]: empty carets transform the word run; title case uses word classes; swap case uses Unicode scalar mappings, without locale.
    let v = buffer.view(id);
    let selection = Selection::from_regions(v.sel().iter().map(|r| {
        if r.empty() {
            word_at(buffer.text(), r.b, &v.settings)
        } else {
            r
        }
    }));
    let mut edits = Vec::new();
    for r in selection.iter() {
        let value = buffer.text().slice(r.to_range()).to_string();
        let mut first = true;
        let mut output = String::new();
        for c in value.chars() {
            match mode {
                0 => output.extend(c.to_uppercase()),
                1 => output.extend(c.to_lowercase()),
                2 => {
                    if class(c, &v.settings) == Class::Word {
                        if first {
                            output.extend(c.to_uppercase());
                        } else {
                            output.extend(c.to_lowercase());
                        }
                        first = false;
                    } else {
                        output.push(c);
                        first = true;
                    }
                }
                _ => {
                    if c.is_lowercase() {
                        output.extend(c.to_uppercase());
                    } else if c.is_uppercase() {
                        output.extend(c.to_lowercase());
                    } else {
                        output.push(c);
                    }
                }
            }
        }
        edits.push((r.to_range(), output));
    }
    commit_owned(buffer, id, edits)
}
/// Unicode uppercase of every selection (word under empty carets).
pub fn upper_case(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    case(buffer, id, 0)
}
/// Unicode lowercase of every selection (word under empty carets).
pub fn lower_case(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    case(buffer, id, 1)
}
/// Uppercase each word's first scalar and lowercase the remainder.
pub fn title_case(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    case(buffer, id, 2)
}
/// Swaps Unicode upper/lowercase, allowing length-changing mappings.
pub fn swap_case(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    case(buffer, id, 3)
}
/// Swaps adjacent graphemes at carets; non-empty regions reverse their graphemes.
pub fn transpose(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    use unicode_segmentation::UnicodeSegmentation;
    // ST-BEHAVIOR: unverified [C2]: at BOL/EOF transpose the first/last pair; overlapping requests merge before reversal, preserving grapheme internals.
    let text = buffer.text();
    let v = buffer.view(id);
    let mut regions = Vec::new();
    for r in v.sel().iter() {
        if !r.empty() {
            regions.push(r);
            continue;
        }
        let row = text.byte_to_line(r.b);
        let start = text.line_to_byte(row);
        let end = line_end(text, row);
        let middle = if r.b == start {
            text.next_grapheme_boundary(start).min(end)
        } else if r.b >= end {
            text.prev_grapheme_boundary(end).max(start)
        } else {
            r.b
        };
        let a = text.prev_grapheme_boundary(middle).max(start);
        let b = text.next_grapheme_boundary(middle).min(end);
        regions.push(Region::new(a, b));
    }
    let selection = Selection::from_regions(regions);
    let edits = selection
        .iter()
        .map(|r| {
            let s = text.slice(r.to_range()).to_string();
            let reversed = s.graphemes(true).rev().collect::<String>();
            (r.to_range(), reversed)
        })
        .collect();
    commit_owned(buffer, id, edits)
}
