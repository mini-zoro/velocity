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
    /// The next/previous LF-delimited paragraph boundary (Cocoa).
    Paragraphs,
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
fn sub_boundary(left: char, right: char, s: &Settings) -> bool {
    // Build 4142 documents configurable separators, not acronym or digit
    // transitions. Retain the existing lowercase-to-uppercase stop.
    // https://www.sublimetext.com/download
    s.sub_word_separators.contains(left)
        || s.sub_word_separators.contains(right)
        || (left.is_lowercase() && right.is_uppercase())
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
            || (t.subword && sub_boundary(a, b, s));
    }
    ac != bc || (sub && sub_boundary(a, b, s)) || a == '\n' || b == '\n'
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
        if !forward && !word && s.translate_tabs_to_spaces && s.use_tab_stops {
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

/// Deletes a selection or the last canonically decomposed component of a grapheme.
pub fn delete_backward_component(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    commit_owned(buffer, id, decomposing_deletions(buffer, id))
}

/// Plans decomposing Backspace in old coordinates for hosts that manage edits.
pub fn decomposing_deletions(buffer: &Buffer, id: ViewId) -> Vec<(Range<usize>, String)> {
    use unicode_normalization::UnicodeNormalization;
    // https://developer.apple.com/documentation/appkit/nsstandardkeybindingresponding/deletebackwardbydecomposingpreviouscharacter(_:)
    let text = buffer.text();
    let selection = buffer.view(id).sel();
    let mut deletion: Vec<(Range<usize>, bool)> = Vec::new();
    for r in selection.iter() {
        let mut range = if r.empty() {
            text.prev_grapheme_boundary(r.b)..r.b
        } else {
            r.to_range()
        };
        if range.is_empty() {
            continue;
        }
        let mut decompose = r.empty();
        while deletion
            .last()
            .is_some_and(|(last, _)| last.end > range.start)
        {
            let (last, was_component) = deletion.pop().unwrap();
            range.start = range.start.min(last.start);
            decompose &= was_component;
        }
        deletion.push((range, decompose));
    }
    deletion
        .into_iter()
        .map(|(range, decompose)| {
            let mut result = String::new();
            if decompose {
                result.extend(text.slice(range.clone()).chars().nfd());
                result.pop();
            }
            (range, result)
        })
        .collect()
}

/// Removes both delimiters around empty carets in one transaction.
pub fn delete_left_right(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    let text = buffer.text();
    let deletion = Selection::from_regions(buffer.view(id).sel().iter().map(|r| {
        Region::new(
            if r.empty() {
                text.prev_grapheme_boundary(r.b)
            } else {
                r.begin()
            },
            text.next_grapheme_boundary(r.end()),
        )
    }));
    commit_owned(
        buffer,
        id,
        deletion
            .iter()
            .filter(|r| !r.empty())
            .map(|r| (r.to_range(), String::new()))
            .collect(),
    )
}

/// Kills to the hard line end, including the LF if already at that boundary.
pub fn delete_to_hard_eol(buffer: &mut Buffer, id: ViewId) -> Vec<String> {
    let text = buffer.text();
    let deletion = Selection::from_regions(buffer.view(id).sel().iter().map(|r| {
        if !r.empty() {
            return r;
        }
        let end = line_end(text, text.byte_to_line(r.b));
        let end = if r.b == end && end < text.len_bytes() {
            end + 1
        } else {
            end
        };
        Region::new(r.b, end)
    }));
    kill_regions(buffer, id, deletion)
}
/// Kills backwards to each supplied visual line start, or to hard BOL without a layout.
pub fn delete_to_bol(buffer: &mut Buffer, id: ViewId, starts: Option<&[usize]>) -> Vec<String> {
    let text = buffer.text();
    let deletion =
        Selection::from_regions(buffer.view(id).sel().iter().enumerate().map(|(i, r)| {
            if !r.empty() {
                return r;
            }
            let start = starts.map_or_else(|| text.line_to_byte(text.byte_to_line(r.b)), |v| v[i]);
            Region::new(start.min(r.b), r.b)
        }));
    kill_regions(buffer, id, deletion)
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
        } else if unit == MoveUnit::Paragraphs {
            let row = text.byte_to_line(r.b);
            if forward {
                let row =
                    row + usize::from(r.b == line_end(text, row) && row + 1 < text.len_lines());
                line_end(text, row)
            } else {
                let row = row.saturating_sub(usize::from(r.b == text.line_to_byte(row)));
                text.line_to_byte(row)
            }
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
/// Word for a caret: like [`word_at`], but a caret right after a word and not before another
/// takes the word on its left (`foo|`, `foo| bar`, `foo|\n`), as ⌘D and the case commands do.
fn caret_word(text: &Text, at: usize, s: &Settings) -> Region {
    let after_word = prev(text, at).is_some_and(|c| class(c, s) == Class::Word);
    let before_word = ch(text, at).is_some_and(|c| class(c, s) == Class::Word);
    if after_word && !before_word {
        word_at(text, text.prev_grapheme_boundary(at), s)
    } else {
        word_at(text, at, s)
    }
}
/// The word, or run of whitespace or punctuation, that starts at or contains byte `at` — the
/// character under the pointer for a double click.
pub fn word_region(buffer: &Buffer, id: ViewId, at: usize) -> Region {
    word_at(
        buffer.text(),
        at.min(buffer.text().len_bytes()),
        &buffer.view(id).settings,
    )
}
/// Moves to an absolute destination, optionally extending from the anchor.
pub fn move_to(buffer: &mut Buffer, id: ViewId, target: MoveTo, extend: bool) {
    // Bol is the smart line start (first non-blank, then column zero, like ⌘← and Home);
    // HardBol is column zero. Eol/HardEol coincide without layout wrapping.
    let text = buffer.text();
    let v = buffer.view(id);
    let selection = v.sel().map(|r| {
        let row = text.byte_to_line(r.b);
        let b = match target {
            MoveTo::Bol => {
                let first = first_nonblank(text, row);
                if r.b == first {
                    text.line_to_byte(row)
                } else {
                    first
                }
            }
            MoveTo::HardBol => text.line_to_byte(row),
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
        ExpandTo::Word if r.empty() => {
            let word = caret_word(text, r.b, &v.settings);
            r.directed(word.begin(), word.end())
        }
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
            let start = text.line_to_byte(*lines.first().expect("line"));
            let mut end = full_line_end(text, *lines.last().expect("line"));
            // Already whole lines: take the next one too, so repeated ⌘L keeps going down.
            if !r.empty() && r.begin() == start && r.end() == end && end < text.len_bytes() {
                end = full_line_end(text, text.byte_to_line(end));
            }
            r.directed(start, end)
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
/// Splits each selection at LF boundaries, or into words if it fits on one line.
pub fn split_selection_into_lines(buffer: &mut Buffer, id: ViewId) {
    // ST-BEHAVIOR: unverified [L3]: keep empty lines; a selection ending at BOL adds no extra caret; preserve direction.
    let text = buffer.text();
    let v = buffer.view(id);
    let mut regions = Vec::new();
    for r in v.sel().iter() {
        // https://www.sublimetext.com/download (Split into Lines falls back to words).
        if !r.empty() && text.byte_to_line(r.begin()) == text.byte_to_line(r.end()) {
            let mut start = None;
            let mut at = r.begin();
            let previous_len = regions.len();
            for c in text.slice(r.to_range()).chars() {
                if c.is_whitespace() {
                    if let Some(a) = start.take() {
                        regions.push(r.directed(a, at));
                    }
                } else {
                    start.get_or_insert(at);
                }
                at += c.len_utf8();
            }
            if let Some(a) = start {
                regions.push(r.directed(a, at));
            }
            if regions.len() == previous_len {
                regions.push(r);
            }
            continue;
        }
        for line in touched_lines(text, &Selection::from(r)) {
            let a = r.begin().max(text.line_to_byte(line));
            let b = r.end().min(line_end(text, line));
            regions.push(r.directed(a, b.max(a)));
        }
    }
    buffer.set_selection(id, Selection::from_regions(regions));
}
/// Adds a neighboring column region, mapping both ends at their visual goal columns.
pub fn select_lines(buffer: &mut Buffer, id: ViewId, forward: bool, layout: &dyn VerticalLayout) {
    let text = buffer.text();
    let mut selection = buffer.view(id).sel().clone();
    let mut primary = selection.primary().unwrap_or_default();
    let source: Vec<_> = selection.iter().collect();
    for r in source {
        let adjacent = |at, x| {
            let row = text.byte_to_line(at);
            let next = if forward {
                (row + 1).min(text.len_lines() - 1)
            } else {
                row.saturating_sub(1)
            };
            if next == row {
                at
            } else {
                layout.offset_at(text, next, x)
            }
        };
        let x = r.xpos.unwrap_or_else(|| layout.x_of(text, r.b));
        let a = adjacent(r.a, if r.empty() { x } else { layout.x_of(text, r.a) });
        let b = adjacent(r.b, x);
        if a != r.a || b != r.b {
            let next = Region {
                a,
                b,
                xpos: Some(x),
            };
            if Some(r) == buffer.view(id).sel().primary() {
                primary = next;
            }
            selection.add(next);
        }
    }
    buffer.set_selection(id, selection.with_primary_region(primary));
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
        let r = caret_word(buffer.text(), r.b, &v.settings);
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
        // Every caret expands to its own word; selected regions stay.
        let words: Vec<_> = v
            .sel()
            .iter()
            .map(|r| {
                if r.empty() {
                    caret_word(buffer.text(), r.b, &v.settings)
                } else {
                    r
                }
            })
            .collect();
        let next = words.iter().map(|r| r.end()).max().unwrap_or(seed.end());
        let selection = if words.len() > 1 {
            Selection::from_regions(words)
        } else {
            Selection::from(seed)
        };
        buffer.set_selection(id, selection);
        buffer.view_mut(id).find = Some(FindState {
            needle,
            whole_word: whole,
            next,
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
fn commit_owned(buffer: &mut Buffer, id: ViewId, owned: Vec<(Range<usize>, String)>) -> ChangeId {
    commit_owned_with_moves(buffer, id, owned, None)
}
fn commit_owned_with_moves(
    buffer: &mut Buffer,
    id: ViewId,
    mut owned: Vec<(Range<usize>, String)>,
    moves: Option<crate::change::RegionMoves>,
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
    buffer.transact_with_region_moves(id, EditKind::Other, &edits, selection, moves)
}
fn line_edits(
    buffer: &mut Buffer,
    id: ViewId,
    moves: Option<crate::change::RegionMoves>,
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
    commit_owned_with_moves(buffer, id, edits, moves)
}
/// Indents every touched line once, using a tab or `tab_size` spaces.
pub fn indent(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    // Verification: https://www.sublimetext.com/download, build 4166 mentions an
    // improvement on empty lines without specifying the rule. Retain today's behavior.
    line_edits(buffer, id, None, |t, l, s| {
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
    unindent_with_moves(buffer, id, None)
}
/// Unindents lines without excluding restored leading indentation from folds on undo.
pub fn unindent_preserving_folds(
    buffer: &mut Buffer,
    id: ViewId,
    fold_key: &'static str,
) -> ChangeId {
    unindent_with_moves(
        buffer,
        id,
        Some(crate::change::RegionMoves::unindent(fold_key)),
    )
}
fn unindent_with_moves(
    buffer: &mut Buffer,
    id: ViewId,
    moves: Option<crate::change::RegionMoves>,
) -> ChangeId {
    line_edits(buffer, id, moves, |t, l, s| {
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
    line_edits(buffer, id, None, |t, l, _| {
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
/// How [`reorder`] permutes a block of lines.
#[derive(Clone, Copy)]
enum Order {
    Sort { case_sensitive: bool },
    Reverse,
}
fn reorder(buffer: &mut Buffer, id: ViewId, order: Order) -> ChangeId {
    // Contiguous touched blocks are permuted independently and keep their terminal LF. Without a
    // non-empty selection the whole buffer is permuted, leaving out the empty line after a final
    // LF.
    let text = buffer.text();
    let selection = buffer.view(id).sel();
    let groups = if selection.iter().all(Region::empty) {
        let mut last = text.len_lines();
        if last > 1 && text.line(last - 1).is_empty() {
            last -= 1;
        }
        std::iter::once(0..last).collect()
    } else {
        blocks(touched_lines(text, selection))
    };
    let edits = groups
        .into_iter()
        .map(|g| {
            let mut lines: Vec<_> = g.clone().map(|l| text.line(l).to_string()).collect();
            match order {
                Order::Sort {
                    case_sensitive: true,
                } => lines.sort(),
                Order::Sort {
                    case_sensitive: false,
                } => lines.sort_by_cached_key(|l| l.to_lowercase()),
                Order::Reverse => lines.reverse(),
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
/// Sorts each contiguous block of touched lines (the whole buffer without a selection) by
/// Unicode scalar order.
pub fn sort_lines(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    reorder(
        buffer,
        id,
        Order::Sort {
            case_sensitive: true,
        },
    )
}
/// Like [`sort_lines`], ignoring case.
pub fn sort_lines_folded(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    reorder(
        buffer,
        id,
        Order::Sort {
            case_sensitive: false,
        },
    )
}
/// Reverses each contiguous block of touched lines (the whole buffer without a selection).
pub fn reverse_lines(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    reorder(buffer, id, Order::Reverse)
}
/// Joins the lines each selection spans (a caret joins its line with the next), trimming the
/// whitespace around every seam.
///
/// Rules observed in Sublime Text 4215 as a black box (2026-11-22):
/// - a selection's last line counts even when the selection ends at its first column, so
///   Select All also joins the final empty line and drops the trailing newline;
/// - a seam gets one space, except after `(`, `[` or `{`, before `)`, `]` or `}`, and
///   where either side is blank (blank lines vanish without doubling spaces).
pub fn join_lines(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    let text = buffer.text();
    let selection = buffer.view(id).sel();
    let mut joins = BTreeSet::new();
    for r in selection.iter() {
        let a = text.byte_to_line(r.begin());
        let b = text.byte_to_line(r.end());
        if a == b {
            if a + 1 < text.len_lines() {
                joins.insert(a);
            }
        } else {
            joins.extend(a..b);
        }
    }
    // https://www.sublimetext.com/download, build 4213: joining maintains the
    // selection. Replace only each seam so endpoints inside the text remain attached.
    let content = |row: usize| {
        let line = text.line(row).to_string();
        let line = line.strip_suffix('\n').unwrap_or(&line).to_owned();
        line.strip_suffix('\r').unwrap_or(&line).to_owned()
    };
    let edits = joins
        .iter()
        .copied()
        .map(|row| {
            let left = text.line(row).to_string();
            let right = text.line(row + 1).to_string();
            let a = if row > 0 && joins.contains(&(row - 1)) && left.trim().is_empty() {
                line_end(text, row)
            } else {
                text.line_to_byte(row) + left.trim_end_matches([' ', '\t']).len()
            };
            let b = text.line_to_byte(row + 1) + right.len()
                - right.trim_start_matches([' ', '\t']).len();
            // The text this seam continues: the nearest non-blank line of the same chain.
            let previous = (0..=row)
                .rev()
                .take_while(|&r| r == row || joins.contains(&r))
                .map(&content)
                .find(|line| !line.trim().is_empty());
            let next = content(row + 1);
            let next = next.trim_start_matches([' ', '\t']);
            let space = match previous.as_deref().map(str::trim_end) {
                None => false,
                Some(left) => {
                    !next.is_empty()
                        && !left.ends_with(['(', '[', '{'])
                        && !next.starts_with([')', ']', '}'])
                }
            };
            (a..b, if space { " " } else { "" }.into())
        })
        .collect();
    commit_owned(buffer, id, edits)
}
/// Opens an empty line above each caret's line, indented like that line, and moves the carets
/// onto it (⇧⌘↩).
pub fn add_line_before(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    let text = buffer.text();
    let rows: BTreeSet<usize> = buffer
        .view(id)
        .sel()
        .iter()
        .map(|r| text.byte_to_line(r.b))
        .collect();
    let mut edits = Vec::new();
    let mut carets = Vec::new();
    let mut shift = 0;
    for row in rows {
        let start = text.line_to_byte(row);
        let indent = text.slice(start..first_nonblank(text, row)).to_string();
        carets.push(Region::caret(start + shift + indent.len()));
        shift += indent.len() + 1;
        edits.push((start..start, format!("{indent}\n")));
    }
    let edit_list: Vec<_> = edits
        .iter()
        .map(|(range, text)| Edit {
            range: range.clone(),
            text,
        })
        .collect();
    buffer.transact(
        id,
        EditKind::Other,
        &edit_list,
        Selection::from_regions(carets),
    )
}
/// Duplicates selected text in place, or each caret's line once (including its LF), leaving the
/// carets and selections on the second copy.
pub fn duplicate_line(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    let text = buffer.text();
    let selection = buffer.view(id).sel().clone();
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let mut lines = BTreeSet::new();
    for r in selection.iter() {
        if r.empty() {
            lines.insert(text.byte_to_line(r.b));
        } else {
            edits.push((r.begin()..r.begin(), text.slice(r.to_range()).to_string()));
        }
    }
    for l in lines {
        let start = text.line_to_byte(l);
        let end = full_line_end(text, l);
        let mut s = text.slice(start..end).to_string();
        if end == line_end(text, l) {
            s.push('\n');
        }
        edits.push((start..start, s));
    }
    edits.sort_by_key(|(r, _)| r.start);
    let map = ChangeSet::new(
        edits
            .iter()
            .map(|(r, s)| Change {
                old_range: r.clone(),
                new_len: s.len(),
            })
            .collect(),
    );
    // Insertions sit at or before each region, so everything moves onto the second copy.
    let after = selection.map(|r| {
        Region::new(
            map.transform_point(r.a, Bias::Right),
            map.transform_point(r.b, Bias::Right),
        )
    });
    let edit_list: Vec<_> = edits
        .iter()
        .map(|(range, text)| Edit {
            range: range.clone(),
            text,
        })
        .collect();
    buffer.transact(id, EditKind::Other, &edit_list, after)
}
fn swap_lines(
    buffer: &mut Buffer,
    id: ViewId,
    forward: bool,
    folds: &[Region],
    fold_key: Option<&'static str>,
) -> ChangeId {
    let text = buffer.text();
    let selection = buffer.view(id).sel().clone();
    let mut units: Vec<Range<usize>> = Vec::new();
    for fold in folds {
        let start = text.byte_to_line(fold.begin());
        let end_row = text.byte_to_line(fold.end());
        let end = end_row + usize::from(text.line_to_byte(end_row) != fold.end());
        let end = end.max(start + 1);
        if let Some(last) = units.last_mut().filter(|last| last.end > start) {
            last.end = last.end.max(end);
        } else {
            units.push(start..end);
        }
    }
    let unit = |row| {
        units
            .get(units.partition_point(|unit| unit.end <= row))
            .filter(|unit| unit.contains(&row))
            .cloned()
            .unwrap_or(row..row + 1)
    };
    let mut groups: Vec<Range<usize>> = Vec::new();
    for g in blocks(touched_lines(text, &selection)) {
        let rows = unit(g.start).start..unit(g.end - 1).end;
        if let Some(last) = groups.last_mut().filter(|last| last.end >= rows.start) {
            last.end = last.end.max(rows.end);
        } else {
            groups.push(rows);
        }
    }
    let mut edits = Vec::new();
    let mut shifts = Vec::new();
    let mut rotated = Vec::new();
    for g in groups {
        if (!forward && g.start == 0) || (forward && g.end == text.len_lines()) {
            continue;
        }
        let a = if forward {
            g.start
        } else {
            unit(g.start - 1).start
        };
        let b = if forward { unit(g.end).end } else { g.end };
        let middle = if forward { g.end } else { g.start };
        let mut lines: Vec<_> = (a..b).map(|l| text.line(l).to_string()).collect();
        lines.rotate_left(middle - a);
        let end = full_line_end(text, b - 1);
        let mut value = lines.join("\n");
        if b < text.len_lines() {
            value.push('\n');
        }
        if fold_key.is_some() && text.slice(text.line_to_byte(a)..end) != value.as_str() {
            rotated.push((a, middle, b, end));
        }
        shifts.push((
            g,
            if forward {
                (b - middle) as isize
            } else {
                -((middle - a) as isize)
            },
        ));
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
    let relocations = fold_key.map(|name| {
        crate::change::RegionMoves::lines(
            name,
            rotated
                .into_iter()
                .flat_map(|(a, mid, b, end)| {
                    let start = text.line_to_byte(a);
                    let pivot = text.line_to_byte(mid);
                    let new_pivot = after.line_to_byte(a + b - mid);
                    [
                        (start..pivot, new_pivot..end),
                        (pivot..end, start..new_pivot),
                    ]
                })
                .collect(),
        )
    });
    buffer.transact_with_region_moves(id, EditKind::Other, &edits, new, relocations)
}
/// Moves each touched block one line upward.
pub fn swap_line_up(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    swap_lines(buffer, id, false, &[], None)
}
/// Moves each touched block one line downward.
pub fn swap_line_down(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    swap_lines(buffer, id, true, &[], None)
}
/// Swaps projected line units, keeping the named fold regions on their content.
pub fn swap_folded_lines(
    buffer: &mut Buffer,
    id: ViewId,
    forward: bool,
    folds: &[Region],
    fold_key: &'static str,
) -> ChangeId {
    swap_lines(buffer, id, forward, folds, Some(fold_key))
}
fn case(buffer: &mut Buffer, id: ViewId, mode: u8) -> ChangeId {
    // ST-BEHAVIOR: unverified [C1]: empty carets transform the word run; title case uses word classes; swap case uses Unicode scalar mappings, without locale.
    let v = buffer.view(id);
    let selection = Selection::from_regions(v.sel().iter().map(|r| {
        if r.empty() {
            caret_word(buffer.text(), r.b, &v.settings)
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
/// Circulates selected strings, or swaps adjacent graphemes and advances each caret.
pub fn transpose(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    use unicode_segmentation::UnicodeSegmentation;
    let text = buffer.text();
    let old = buffer.view(id).sel().clone();
    // https://docs.sublimetext.io/reference/commands.html#transpose: selections
    // circulate; without selections swap adjacent characters and advance one character.
    if old.iter().any(|r| !r.empty()) {
        let values: Vec<_> = old
            .iter()
            .map(|r| text.slice(r.to_range()).to_string())
            .collect();
        let rotated: Vec<_> = (0..values.len())
            .map(|i| values[(i + values.len() - 1) % values.len()].clone())
            .collect();
        return commit_owned(
            buffer,
            id,
            old.iter()
                .zip(rotated)
                .map(|(r, s)| (r.to_range(), s))
                .collect(),
        );
    }
    let mut ranges = Vec::new();
    let mut targets = Vec::new();
    for r in old.iter() {
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
        ranges.push(Region::new(a, b));
        // At BOL the first pair is swapped, then the caret advances over the new first
        // grapheme. Elsewhere the end of the swapped pair is one grapheme ahead.
        targets.push(if r.b == start { a + b - middle } else { b });
    }
    let selection = Selection::from_regions(ranges);
    let owned: Vec<_> = selection
        .iter()
        .map(|r| {
            let s = text.slice(r.to_range()).to_string();
            (r.to_range(), s.graphemes(true).rev().collect::<String>())
        })
        .collect();
    let edits: Vec<_> = owned
        .iter()
        .map(|(range, text)| Edit {
            range: range.clone(),
            text,
        })
        .collect();
    let mut targets = targets.into_iter();
    let after = old.map(|r| {
        let b = targets.next().expect("one target per selection");
        if r.empty() { Region::caret(b) } else { r }
    });
    buffer.transact(id, EditKind::Other, &edits, after)
}

/// Permutations of lines or selected strings; shuffle accepts a reproducible seed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permutation {
    /// Reverse the input order.
    Reverse,
    /// Keep the first occurrence of each distinct string.
    Unique,
    /// Fisher–Yates shuffle, driven by a deterministic 64-bit PRNG.
    Shuffle(u64),
}
fn permute(values: &mut Vec<String>, operation: Permutation) {
    match operation {
        Permutation::Reverse => values.reverse(),
        Permutation::Unique => {
            let mut seen = BTreeSet::new();
            values.retain(|s| seen.insert(s.clone()));
        }
        Permutation::Shuffle(mut seed) => {
            // SplitMix64 permits every seed, including zero. Rejection sampling
            // avoids modulo bias for non-power-of-two selection counts.
            for i in (1..values.len()).rev() {
                let count = i as u64 + 1;
                let threshold = count.wrapping_neg() % count;
                let chosen = loop {
                    seed = seed.wrapping_add(0x9e3779b97f4a7c15);
                    let mut n = seed;
                    n = (n ^ (n >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
                    n = (n ^ (n >> 27)).wrapping_mul(0x94d049bb133111eb);
                    n ^= n >> 31;
                    if n >= threshold {
                        break (n % count) as usize;
                    }
                };
                values.swap(i, chosen);
            }
        }
    }
}
/// Permutes each contiguous touched line block, preserving its terminal LF.
pub fn permute_lines(buffer: &mut Buffer, id: ViewId, operation: Permutation) -> ChangeId {
    let text = buffer.text();
    let selection = buffer.view(id).sel();
    let groups = if selection.iter().all(Region::empty) {
        let mut end = text.len_lines();
        if end > 1 && text.line(end - 1).is_empty() {
            end -= 1;
        }
        std::iter::once(0..end).collect()
    } else {
        blocks(touched_lines(text, selection))
    };
    let edits = groups
        .into_iter()
        .map(|g| {
            let mut lines = g.clone().map(|l| text.line(l).to_string()).collect();
            permute(&mut lines, operation);
            let mut value = lines.join("\n");
            if g.end < text.len_lines() {
                value.push('\n');
            }
            (
                text.line_to_byte(g.start)..full_line_end(text, g.end - 1),
                value,
            )
        })
        .collect();
    commit_owned(buffer, id, edits)
}
/// Reorders selected strings across their original slots, ignoring empty carets.
pub fn permute_selection(buffer: &mut Buffer, id: ViewId, operation: Permutation) -> ChangeId {
    // ST-BEHAVIOR: unverified [T2]: unique empties redundant slots, retaining
    // intervening text. Empty carets do not consume a permutation slot.
    let regions: Vec<_> = buffer
        .view(id)
        .sel()
        .iter()
        .filter(|r| !r.empty())
        .collect();
    let mut values = regions
        .iter()
        .map(|r| buffer.text().slice(r.to_range()).to_string())
        .collect();
    permute(&mut values, operation);
    values.resize(regions.len(), String::new());
    commit_owned(
        buffer,
        id,
        regions
            .into_iter()
            .zip(values)
            .map(|(r, s)| (r.to_range(), s))
            .collect(),
    )
}
/// Sorts selected strings across slots, optionally folding case for comparisons.
pub fn sort_selection(
    buffer: &mut Buffer,
    id: ViewId,
    case_sensitive: bool,
    reverse: bool,
) -> ChangeId {
    // ST-BEHAVIOR: unverified [T2]: stable Unicode lowercase keys when folded;
    // reverse reverses the resulting slot order, including equal keys.
    let regions: Vec<_> = buffer
        .view(id)
        .sel()
        .iter()
        .filter(|r| !r.empty())
        .collect();
    let mut values: Vec<_> = regions
        .iter()
        .map(|r| buffer.text().slice(r.to_range()).to_string())
        .collect();
    if case_sensitive {
        values.sort();
    } else {
        values.sort_by_cached_key(|s| s.to_lowercase());
    }
    if reverse {
        values.reverse();
    }
    commit_owned(
        buffer,
        id,
        regions
            .into_iter()
            .zip(values)
            .map(|(r, s)| (r.to_range(), s))
            .collect(),
    )
}

fn paragraph(text: &Text, row: usize) -> Range<usize> {
    let blank = |l| text.line(l).chars().all(char::is_whitespace);
    if blank(row) {
        return row..row + 1;
    }
    let mut start = row;
    let mut end = row + 1;
    while start > 0 && !blank(start - 1) {
        start -= 1;
    }
    while end < text.len_lines() && !blank(end) {
        end += 1;
    }
    start..end
}
/// Expands each selection to complete paragraphs separated by whitespace-only lines.
pub fn expand_selection_to_paragraph(buffer: &mut Buffer, id: ViewId) {
    // ST-BEHAVIOR: unverified [T3]: blank rows are their own paragraph; include
    // paragraph terminal LF, retain direction and exclude an end-at-BOL row.
    let text = buffer.text();
    let selection = buffer.view(id).sel().map(|r| {
        let rows = touched_lines(text, &Selection::from(r));
        let first = paragraph(text, *rows.first().expect("one touched row"));
        let last = paragraph(text, *rows.last().expect("one touched row"));
        r.directed(
            text.line_to_byte(first.start),
            full_line_end(text, last.end - 1),
        )
    });
    buffer.set_selection(id, selection);
}
fn wrap_prefix<'a>(line: &'a str, prefixes: &[String]) -> &'a str {
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let body = &line[indent..];
    let marker = prefixes
        .iter()
        .map(String::as_str)
        .filter(|prefix| !prefix.is_empty() && body.starts_with(prefix))
        .max_by_key(|prefix| prefix.len());
    let end = marker.map_or(indent, |marker| {
        let tail = &body[marker.len()..];
        indent + marker.len() + tail.len() - tail.trim_start_matches([' ', '\t']).len()
    });
    &line[..end]
}
/// Reflows text paragraphs, preserving indentation. Syntax-free text has no comment prefixes.
pub fn wrap_lines(buffer: &mut Buffer, id: ViewId, width: usize) -> ChangeId {
    wrap_lines_with_prefixes(buffer, id, width, &[])
}
/// Reflows paragraphs sharing indentation and a language-provided comment prefix. Every output
/// line retains that prefix; adjacent code and differently prefixed comments stay separate.
pub fn wrap_lines_with_prefixes(
    buffer: &mut Buffer,
    id: ViewId,
    width: usize,
    prefixes: &[String],
) -> ChangeId {
    use unicode_width::UnicodeWidthStr;
    let text = buffer.text();
    let settings = &buffer.view(id).settings;
    let mut paragraphs = BTreeSet::new();
    for row in touched_lines(text, buffer.view(id).sel()) {
        let first = text.line(row).to_string();
        if first.trim().is_empty() {
            continue;
        }
        let prefix = wrap_prefix(&first, prefixes);
        let same = |row| {
            let line = text.line(row).to_string();
            !line.trim().is_empty() && wrap_prefix(&line, prefixes) == prefix
        };
        let mut start = row;
        let mut end = row + 1;
        while start > 0 && same(start - 1) {
            start -= 1;
        }
        while end < text.len_lines() && same(end) {
            end += 1;
        }
        paragraphs.insert((start, end));
    }
    let edits = paragraphs
        .into_iter()
        .map(|(start, end)| {
            let first = text.line(start).to_string();
            let prefix = wrap_prefix(&first, prefixes);
            let indent = prefix.chars().fold(0, |col, c| {
                if c == '\t' {
                    col + settings.tab_size - col % settings.tab_size
                } else {
                    col + unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)
                }
            });
            let mut value = prefix.to_owned();
            let mut column = indent;
            let mut has_word = false;
            for row in start..end {
                let line = text.line(row).to_string();
                for word in line[prefix.len()..].split_whitespace() {
                    let len = UnicodeWidthStr::width(word);
                    if has_word && column + 1 + len > width.max(1) {
                        value.push('\n');
                        value.push_str(prefix);
                        column = indent;
                        has_word = false;
                    }
                    if has_word {
                        value.push(' ');
                        column += 1;
                    }
                    value.push_str(word);
                    column += len;
                    has_word = true;
                }
            }
            if end < text.len_lines() {
                value.push('\n');
            }
            (
                text.line_to_byte(start)..full_line_end(text, end - 1),
                value,
            )
        })
        .collect();
    commit_owned(buffer, id, edits)
}

/// Common identifier styles; Unicode casing may change the byte length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentCase {
    /// `snake_case`.
    Snake,
    /// `camelCase`.
    Camel,
    /// `PascalCase`.
    Pascal,
    /// `kebab-case`.
    Kebab,
    /// `UPPER_SNAKE`.
    UpperSnake,
}
fn identifier_case(value: &str, style: IdentCase) -> String {
    let chars: Vec<_> = value.chars().collect();
    let mut words = Vec::<String>::new();
    let mut word = String::new();
    for (i, &c) in chars.iter().enumerate() {
        // Combining marks stay with their preceding letter. Separators and
        // acronym/lowercase transitions split words, including HTTPServer.
        if c == '_' || c == '-' || c.is_whitespace() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            continue;
        }
        let previous = i.checked_sub(1).map(|n| chars[n]);
        let boundary = c.is_uppercase()
            && previous.is_some_and(|p| {
                p.is_lowercase()
                    || p.is_numeric()
                    || (p.is_uppercase() && chars.get(i + 1).is_some_and(|n| n.is_lowercase()))
            });
        if boundary && !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
        word.push(c);
    }
    if !word.is_empty() {
        words.push(word);
    }
    let mut result = String::new();
    for (i, word) in words.iter().enumerate() {
        match style {
            IdentCase::Snake | IdentCase::Kebab | IdentCase::UpperSnake => {
                if i > 0 {
                    result.push(if style == IdentCase::Kebab { '-' } else { '_' });
                }
                if style == IdentCase::UpperSnake {
                    result.push_str(&word.to_uppercase());
                } else {
                    result.push_str(&word.to_lowercase());
                }
            }
            IdentCase::Camel if i == 0 => result.push_str(&word.to_lowercase()),
            IdentCase::Camel | IdentCase::Pascal => {
                let mut chars = word.chars();
                if let Some(c) = chars.next() {
                    result.extend(c.to_uppercase());
                }
                result.push_str(&chars.as_str().to_lowercase());
            }
        }
    }
    result
}
/// Converts selected identifier text (or the word under a caret) in one edit batch.
pub fn convert_ident_case(buffer: &mut Buffer, id: ViewId, style: IdentCase) -> ChangeId {
    // ST-BEHAVIOR: unverified [T5]: acronym boundaries, separator collapse,
    // caret word expansion and Unicode case mappings are clean-room choices.
    let view = buffer.view(id);
    let regions = Selection::from_regions(view.sel().iter().map(|r| {
        if r.empty() {
            let word = caret_word(buffer.text(), r.b, &view.settings);
            if buffer
                .text()
                .slice(word.to_range())
                .chars()
                .any(char::is_alphanumeric)
            {
                word
            } else {
                r
            }
        } else {
            r
        }
    }));
    let edits = regions
        .iter()
        .map(|r| {
            (
                r.to_range(),
                identifier_case(&buffer.text().slice(r.to_range()).to_string(), style),
            )
        })
        .collect();
    commit_owned(buffer, id, edits)
}

/// Named region set used by the mark commands and clear_bookmarks {"name":"mark"}.
pub const MARK: &str = "mark";
const MARK_PRIMARY: &str = "velocity.mark.primary";
/// Mapped bookmark regions, also consumed by the view's gutter accessor.
pub const BOOKMARKS: &str = "velocity.bookmarks";
/// Saves a view's directed selection as editing-mapped named regions.
pub fn set_mark(buffer: &mut Buffer, id: ViewId) {
    // ST-BEHAVIOR: unverified [T6]: marks are per-view, persisted named regions;
    // they map through edits but are not independently text-undoable.
    buffer.erase_regions(id, "velocity.mark");
    buffer.add_regions(id, MARK, buffer.view(id).sel().iter().collect(), false);
    let primary = buffer.view(id).sel().primary().unwrap_or_default();
    buffer.add_regions(id, MARK_PRIMARY, vec![primary], false);
}
/// Extends each caret to its corresponding saved mark's caret.
pub fn select_to_mark(buffer: &mut Buffer, id: ViewId) -> bool {
    // ST-BEHAVIOR: unverified [T6]: unequal counts use the first saved mark.
    let Some(mark) = buffer
        .view(id)
        .get_regions(MARK)
        .or_else(|| buffer.view(id).get_regions("velocity.mark"))
    else {
        return false;
    };
    if mark.regions.is_empty() {
        return false;
    }
    let count = buffer.view(id).sel().len();
    let mut i = 0;
    let selection = buffer.view(id).sel().map(|r| {
        let region = Region::new(
            r.b,
            mark.regions[if count == mark.regions.len() { i } else { 0 }].b,
        );
        i += 1;
        region
    });
    buffer.set_selection(id, selection);
    true
}
/// Exchanges the saved mark regions with the current selection.
pub fn swap_with_mark(buffer: &mut Buffer, id: ViewId) -> bool {
    // ST-BEHAVIOR: unverified [T6]: the complete directed regions are exchanged.
    let Some(mark) = buffer
        .view(id)
        .get_regions(MARK)
        .or_else(|| buffer.view(id).get_regions("velocity.mark"))
    else {
        return false;
    };
    if mark.regions.is_empty() {
        return false;
    }
    let mut selection = Selection::from_regions(mark.regions.clone());
    if let Some(primary) = buffer
        .view(id)
        .get_regions(MARK_PRIMARY)
        .and_then(|set| set.regions.first())
    {
        selection = selection.with_primary_region(*primary);
    }
    set_mark(buffer, id);
    buffer.set_selection(id, selection);
    true
}
/// Removes all bookmarks for this view.
pub fn clear_bookmarks(buffer: &mut Buffer, id: ViewId) {
    clear_named_regions(buffer, id, BOOKMARKS);
}
/// Clears a named region set, including the mark's primary-region metadata.
pub fn clear_named_regions(buffer: &mut Buffer, id: ViewId, name: &str) {
    buffer.erase_regions(id, name);
    if name == MARK {
        buffer.erase_regions(id, MARK_PRIMARY);
        buffer.erase_regions(id, "velocity.mark");
    }
}
// Inclusive interval keys distinguish an empty bookmark at p from a region ending at p.
fn bookmark_end(region: Region) -> (usize, bool) {
    (region.end(), region.empty())
}
fn bookmark_prefix_ends(regions: &[Region]) -> Vec<(usize, bool)> {
    let mut end = (0, false);
    regions
        .iter()
        .map(|r| {
            end = end.max(bookmark_end(*r));
            end
        })
        .collect()
}
fn overlaps_bookmarks(regions: &[Region], ends: &[(usize, bool)], query: Region) -> bool {
    let limit = regions.partition_point(|r| (r.begin(), true) <= bookmark_end(query));
    limit > 0 && ends[limit - 1] >= (query.begin(), true)
}
/// Toggles directed bookmark regions: carets remove marks touching their line, selections
/// remove overlapping marks; selections with no matching bookmark add a complete region.
pub fn toggle_bookmark(buffer: &mut Buffer, id: ViewId) {
    // https://docs.sublimetext.io/reference/commands.html#toggle-bookmark only specifies
    // active regions; https://www.sublimetext.com/download (4166) names improved toggling
    // without its rule. Use the round-16 reviewer fallback: line touch for an empty caret,
    // overlap for a selection, otherwise add. Evaluate all carets against the original set.
    let old = buffer
        .view(id)
        .get_regions(BOOKMARKS)
        .map_or_else(Vec::new, |set| set.regions.clone());
    let old_ends = bookmark_prefix_ends(&old);
    let selections: Vec<_> = buffer.view(id).sel().iter().collect();
    let targets: Vec<_> = selections
        .iter()
        .map(|r| {
            if r.empty() {
                let row = buffer.text().byte_to_line(r.b);
                Region::new(
                    buffer.text().line_to_byte(row),
                    full_line_end(buffer.text(), row),
                )
            } else {
                *r
            }
        })
        .collect();
    let mut queries = targets.clone();
    queries.sort_by_key(|r| r.begin());
    queries.dedup_by_key(|r| (r.begin(), r.end()));
    let query_ends = bookmark_prefix_ends(&queries);
    let mut regions: std::collections::BTreeMap<_, _> = old
        .iter()
        .filter(|r| !overlaps_bookmarks(&queries, &query_ends, **r))
        .map(|r| ((r.begin(), r.end()), *r))
        .collect();
    let mut added_lines = BTreeSet::new();
    for (region, target) in selections.into_iter().zip(targets) {
        if !overlaps_bookmarks(&old, &old_ends, target)
            && (!region.empty() || added_lines.insert(target.begin()))
        {
            regions.insert((region.begin(), region.end()), region);
        }
    }
    buffer.add_regions(id, BOOKMARKS, regions.into_values().collect(), false);
}
fn navigate_bookmark(buffer: &mut Buffer, id: ViewId, forward: bool, extend: bool) -> bool {
    let Some(set) = buffer
        .view(id)
        .get_regions(BOOKMARKS)
        .filter(|s| !s.regions.is_empty())
    else {
        return false;
    };
    let selection = buffer.view(id).sel().map(|r| {
        let marks = &set.regions;
        let next = if forward {
            let i = marks.partition_point(|b| b.begin() <= r.end());
            marks[i % marks.len()]
        } else {
            let i = marks.partition_point(|b| b.begin() < r.begin());
            marks[if i == 0 { marks.len() - 1 } else { i - 1 }]
        };
        if extend {
            Region::new(r.a, next.b)
        } else {
            next
        }
    });
    buffer.set_selection(id, selection);
    true
}
/// Selects the next complete bookmark region, wrapping at EOF.
pub fn next_bookmark(buffer: &mut Buffer, id: ViewId, extend: bool) -> bool {
    navigate_bookmark(buffer, id, true, extend)
}
/// Selects the previous complete bookmark region, wrapping at BOF.
pub fn prev_bookmark(buffer: &mut Buffer, id: ViewId, extend: bool) -> bool {
    navigate_bookmark(buffer, id, false, extend)
}
/// Selects every stored bookmark region, preserving its direction.
pub fn select_all_bookmarks(buffer: &mut Buffer, id: ViewId) -> bool {
    let Some(set) = buffer
        .view(id)
        .get_regions(BOOKMARKS)
        .filter(|s| !s.regions.is_empty())
    else {
        return false;
    };
    let selection = Selection::from_regions(set.regions.clone());
    buffer.set_selection(id, selection);
    true
}
/// Compatibility alias for select_all_bookmarks.
pub fn select_bookmarks(buffer: &mut Buffer, id: ViewId) -> bool {
    select_all_bookmarks(buffer, id)
}
/// Deletes to the saved mark and returns the removed strings for the application's kill ring.
pub fn delete_to_mark(buffer: &mut Buffer, id: ViewId) -> Vec<String> {
    if !select_to_mark(buffer, id) {
        return vec![];
    }
    kill_regions(buffer, id, buffer.view(id).sel().clone())
}

fn kill_regions(buffer: &mut Buffer, id: ViewId, regions: Selection) -> Vec<String> {
    let edits: Vec<_> = regions
        .iter()
        .filter(|r| !r.empty())
        .map(|r| (r.to_range(), String::new()))
        .collect();
    if edits.is_empty() {
        return vec![];
    }
    let values = regions
        .iter()
        .map(|r| buffer.text().slice(r.to_range()).to_string())
        .collect();
    commit_owned(buffer, id, edits);
    values
}
/// Inserts kill-ring strings, distributing when the selection counts agree.
pub fn yank(buffer: &mut Buffer, id: ViewId, values: &[String]) -> ChangeId {
    if values.is_empty() {
        return buffer.change_id();
    }
    let count = buffer.view(id).sel().len();
    let joined = if values.len() == count {
        String::new()
    } else {
        values.join("\n")
    };
    let texts = (0..count)
        .map(|i| {
            if values.len() == count {
                values[i].clone()
            } else {
                joined.clone()
            }
        })
        .collect();
    replacements(buffer, id, texts, EditKind::Other)
}
/// Sublime-spelled alias for the touched-line whitespace operation.
pub fn trim_trailing_white_space(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    trim_trailing_whitespace(buffer, id)
}

/// Inserts text at EOF in one transaction, mapping existing selections through it.
pub fn append(buffer: &mut Buffer, id: ViewId, text: &str) -> ChangeId {
    // ST-BEHAVIOR: unverified [T8]: EOF carets stick to appended text; other
    // selections retain their positions. Force/scroll arguments belong to hosts.
    let end = buffer.text().len_bytes();
    commit_owned(buffer, id, vec![(end..end, text.into())])
}
/// Replaces nonempty selections by their two endpoints; keeps existing carets.
pub fn split_selection(buffer: &mut Buffer, id: ViewId) {
    // ST-BEHAVIOR: unverified [T9]: normalized endpoint carets, final endpoint primary.
    let regions = buffer
        .view(id)
        .sel()
        .iter()
        .flat_map(|r| [Region::caret(r.a), Region::caret(r.b)])
        .collect::<Vec<_>>();
    buffer.set_selection(id, Selection::from_regions(regions));
}
/// Selects the complement of nonempty selections over the entire document.
pub fn invert_selection(buffer: &mut Buffer, id: ViewId) {
    // ST-BEHAVIOR: unverified [T9]: carets do not cut a gap; an empty complement
    // becomes an EOF caret so the view still owns at least one selection.
    let mut at = 0;
    let mut regions = Vec::new();
    for r in buffer.view(id).sel().iter().filter(|r| !r.empty()) {
        if at < r.begin() {
            regions.push(Region::new(at, r.begin()));
        }
        at = r.end();
    }
    if at < buffer.text().len_bytes() {
        regions.push(Region::new(at, buffer.text().len_bytes()));
    }
    if regions.is_empty() {
        regions.push(Region::caret(at));
    }
    buffer.set_selection(id, Selection::from_regions(regions));
}
/// Applies ASCII ROT13 to selected text; non-ASCII scalars remain intact.
pub fn rot13(buffer: &mut Buffer, id: ViewId) -> ChangeId {
    // ST-BEHAVIOR: unverified [T10]: carets expand to their word, as case commands do.
    let view = buffer.view(id);
    let regions = Selection::from_regions(view.sel().iter().map(|r| {
        if r.empty() {
            caret_word(buffer.text(), r.b, &view.settings)
        } else {
            r
        }
    }));
    let edits = regions
        .iter()
        .map(|r| {
            let s = buffer
                .text()
                .slice(r.to_range())
                .chars()
                .map(|c| match c {
                    'a'..='m' | 'A'..='M' => char::from_u32(c as u32 + 13).unwrap(),
                    'n'..='z' | 'N'..='Z' => char::from_u32(c as u32 - 13).unwrap(),
                    _ => c,
                })
                .collect();
            (r.to_range(), s)
        })
        .collect();
    commit_owned(buffer, id, edits)
}

/// Matches plain-text brackets for hosts without syntax services. The view replaces this
/// with its language's code/tag pairs and string/comment scope boundaries.
pub fn plain_bracket_command(buffer: &mut Buffer, id: ViewId, expand: bool, extend: bool) {
    let source = buffer.text().to_string();
    let mut stack = Vec::new();
    let mut pairs = Vec::new();
    for (at, c) in source.char_indices() {
        if matches!(c, '(' | '[' | '{') {
            stack.push((c, at));
        } else if let Some(open) = match c {
            ')' => Some('('),
            ']' => Some('['),
            '}' => Some('{'),
            _ => None,
        } && let Some((_, a)) = stack.pop_if(|(c, _)| *c == open)
        {
            pairs.push((a, at));
        }
    }
    let selection = buffer.view(id).sel().map(|r| {
        if expand {
            pairs
                .iter()
                .flat_map(|&(a, b)| [a + 1..b, a..b + 1])
                .filter(|p| {
                    p.start <= r.begin()
                        && r.end() <= p.end
                        && (p.start < r.begin() || r.end() < p.end)
                })
                .min_by_key(|p| p.len())
                .map_or(r, |p| r.directed(p.start, p.end))
        } else {
            pairs
                .iter()
                .filter(|&&(a, b)| a <= r.b && r.b <= b + 1)
                .min_by_key(|&&(a, b)| {
                    let priority = if r.b == a || r.b == b {
                        0
                    } else if r.b == a + 1 || r.b == b + 1 {
                        1
                    } else {
                        2
                    };
                    (priority, b - a)
                })
                .map_or(r, |&(a, b)| {
                    let to = if r.b >= b { a } else { b };
                    Region::new(if extend { r.a } else { to }, to)
                })
        }
    });
    buffer.set_selection(id, selection);
}
