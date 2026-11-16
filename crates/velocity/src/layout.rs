//! Incremental grapheme wrapping and fold projection with logarithmic row lookup.
use std::{ops::Range, sync::Arc};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
use vel_buffer::{BufferId, ChangeId, ChangeSet, Region, VerticalLayout};
use vel_text::Text;

pub const FOLDS: &str = "velocity.folds";
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Zero disables wrapping; otherwise this is the visual width in cells.
    pub width: usize,
    pub tab: usize,
    pub indent: bool,
}
#[derive(Clone, Debug)]
pub struct Segment {
    pub bytes: Range<usize>,
    pub indent: usize,
}
#[derive(Debug)]
pub struct Unit<'a> {
    pub bytes: Range<usize>,
    pub text: &'a str,
    pub width: usize,
}
#[derive(Clone, Debug, Default)]
struct Line {
    segments: Arc<[Segment]>,
    hidden: Vec<Range<usize>>,
}
#[derive(Debug, Default)]
struct Heights {
    tree: Vec<usize>,
}
impl Heights {
    fn build(lines: &[Line]) -> Self {
        let mut tree = vec![0; lines.len() + 1];
        for (row, line) in lines.iter().enumerate() {
            let i = row + 1;
            tree[i] += line.segments.len();
            let parent = i + i.isolate_lowest_one();
            if parent < tree.len() {
                tree[parent] += tree[i];
            }
        }
        Self { tree }
    }
    fn before(&self, row: usize) -> usize {
        let mut i = row;
        let mut n = 0;
        while i > 0 {
            n += self.tree[i];
            i &= i - 1;
        }
        n
    }
    fn update(&mut self, row: usize, old: usize, new: usize) {
        let mut i = row + 1;
        while i < self.tree.len() {
            self.tree[i] = self.tree[i] - old + new;
            i += i.isolate_lowest_one();
        }
    }
    fn locate(&self, visual: usize) -> (usize, usize) {
        let mut at = 0;
        let mut sum = 0;
        let mut bit = self.tree.len().next_power_of_two() >> 1;
        while bit > 0 {
            let next = at + bit;
            if next < self.tree.len() && sum + self.tree[next] <= visual {
                at = next;
                sum += self.tree[next];
            }
            bit >>= 1;
        }
        (at.min(self.tree.len().saturating_sub(2)), visual - sum)
    }
}
#[derive(Debug, Default)]
pub struct Layout {
    identity: Option<(BufferId, ChangeId)>,
    pub long: crate::large::LongLines,
    options: Options,
    lines: Vec<Line>,
    heights: Heights,
    folds: Vec<Region>,
    count: usize,
    inline: Vec<(usize, usize)>,
    phantom_inline: Vec<(usize, usize)>,
    pub rewrapped: usize,
    pub page: usize,
}
impl Layout {
    pub fn prepare_change(&mut self, identity: (BufferId, ChangeId)) {
        if self.identity != Some(identity) {
            self.invalidate();
        }
    }
    pub fn configure(
        &mut self,
        text: &Text,
        identity: (BufferId, ChangeId),
        options: Options,
        folds: &[Region],
    ) {
        if self.identity == Some(identity) && self.options == options && self.folds == folds {
            return;
        }
        if self.identity != Some(identity) {
            self.long.reset();
        }
        self.identity = Some(identity);
        self.options = options;
        self.folds = folds.to_vec();
        self.count = text.len_lines();
        self.rewrapped = 0;
        if options.width == 0 && folds.is_empty() {
            self.lines.clear();
            self.heights.tree.clear();
            return;
        }
        if options.width == 0 {
            let single: Arc<[Segment]> = Arc::from([Segment {
                bytes: 0..usize::MAX,
                indent: 0,
            }]);
            self.lines = vec![
                Line {
                    segments: single,
                    hidden: vec![]
                };
                self.count
            ];
            for fold in folds {
                let first = text.byte_to_line(fold.begin());
                let last = text.byte_to_line(fold.end());
                let start = text.line_to_byte(first);
                let len = text.line_len_bytes(first);
                self.lines[first] = Line {
                    segments: Arc::from([Segment {
                        bytes: 0..len,
                        indent: 0,
                    }]),
                    hidden: std::iter::once(
                        fold.begin() - start..fold.end().min(start + len) - start,
                    )
                    .collect(),
                };
                for row in first + 1..last {
                    self.lines[row] = Line::default();
                }
                if last > first {
                    let end = fold.end() - text.line_to_byte(last);
                    if end > 0 {
                        self.lines[last] = if end >= text.line_len_bytes(last) {
                            Line::default()
                        } else {
                            Line {
                                segments: Arc::from([Segment {
                                    bytes: end..usize::MAX,
                                    indent: 0,
                                }]),
                                hidden: vec![],
                            }
                        };
                    }
                }
            }
            self.heights = Heights::build(&self.lines);
            return;
        }
        self.lines = (0..self.count)
            .map(|row| self.make_line(text, row))
            .collect();
        self.rewrapped = self.count;
        self.heights = Heights::build(&self.lines);
    }
    pub fn invalidate(&mut self) {
        self.identity = None;
    }
    pub fn change(
        &mut self,
        old: &Text,
        after: &Text,
        identity: (BufferId, ChangeId),
        changes: &ChangeSet,
    ) {
        if self.identity.is_none() {
            return;
        }
        if let Some(before) = self.identity {
            self.long.change(old, after, before, identity, changes);
        }
        self.identity = Some(identity);
        self.inline.clear();
        self.folds = self
            .folds
            .iter()
            .map(|r| changes.transform_region(*r))
            .filter(|r| !r.empty())
            .collect();
        self.count = after.len_lines();
        self.rewrapped = 0;
        if self.lines.is_empty() {
            return;
        }
        let Some(first) = changes.changes().first() else {
            return;
        };
        let last = changes.changes().last().expect("first");
        let start = old.byte_to_line(first.old_range.start);
        let end = old.byte_to_line(last.old_range.end) + 1;
        let new_end = after.len_lines() - (self.lines.len() - end);
        // Each disjoint same-line edit rewraps just its own physical line.
        if old.len_lines() == after.len_lines()
            && changes.changes().iter().all(|c| {
                let at = changes.transform_point(c.old_range.start, vel_buffer::Bias::Left);
                old.byte_to_line(c.old_range.start) == old.byte_to_line(c.old_range.end)
                    && after.byte_to_line(at) == after.byte_to_line(at + c.new_len)
            })
        {
            let mut previous = None;
            for c in changes.changes() {
                let row = after.byte_to_line(
                    changes.transform_point(c.old_range.start, vel_buffer::Bias::Left),
                );
                if previous == Some(row) {
                    continue;
                }
                previous = Some(row);
                let line = self.make_line(after, row);
                self.heights
                    .update(row, self.lines[row].segments.len(), line.segments.len());
                self.lines[row] = line;
                self.rewrapped += 1;
            }
        } else {
            let replacement: Vec<_> = (start..new_end)
                .map(|row| self.make_line(after, row))
                .collect();
            self.rewrapped = replacement.len();
            self.lines.splice(start..end, replacement);
            self.heights = Heights::build(&self.lines);
        }
    }
    fn make_line(&self, text: &Text, row: usize) -> Line {
        let start = text.line_to_byte(row);
        let content = text.line(row).to_string();
        let len = content.trim_end_matches('\n').len();
        let mut hidden = Vec::new();
        let mut visible_start = 0;
        // Fold sets are sorted and disjoint; binary search keeps this O(log folds).
        let first = self.folds.partition_point(|f| f.end() <= start);
        for fold in self.folds[first..]
            .iter()
            .take_while(|f| f.begin() <= start + len)
        {
            if fold.begin() < start && fold.end() > start {
                if fold.end() >= start + len {
                    return Line::default();
                }
                visible_start = fold.end() - start;
                continue;
            }
            if fold.begin() >= start && fold.begin() <= start + len {
                hidden.push(fold.begin() - start..fold.end().min(start + len) - start);
            }
        }
        let units: Vec<_> = units(&content[..len], &hidden, self.options.tab)
            .into_iter()
            .filter(|u| u.bytes.start >= visible_start)
            .collect();
        let indent = if self.options.indent && self.options.width > 1 {
            crate::editor::cells_with_tab(
                &content[..content.len() - content.trim_start_matches([' ', '\t']).len()],
                self.options.tab,
            )
            .min(self.options.width / 2)
        } else {
            0
        };
        let mut segments = Vec::new();
        let mut from = 0;
        if units.is_empty() {
            segments.push(Segment {
                bytes: visible_start..len,
                indent: 0,
            });
        }
        while from < units.len() {
            let left = if from == 0 { 0 } else { indent };
            let mut col = left;
            let mut end = from;
            let mut word = None;
            while end < units.len() {
                if self.options.width > 0
                    && col + units[end].width + self.phantom_gap(start + units[end].bytes.start)
                        > self.options.width
                    && end > from
                {
                    break;
                }
                col += units[end].width + self.phantom_gap(start + units[end].bytes.start);
                end += 1;
                if units[end - 1].text.chars().all(char::is_whitespace) {
                    word = Some(end);
                }
            }
            if end < units.len()
                && let Some(boundary) = word
                && boundary > from
            {
                end = boundary;
            }
            segments.push(Segment {
                bytes: units[from].bytes.start..units[end - 1].bytes.end,
                indent: left,
            });
            from = end;
        }
        Line {
            segments: segments.into(),
            hidden,
        }
    }
    pub fn set_phantom_inline(&mut self, text: &Text, inline: Vec<(usize, usize)>) {
        if self.phantom_inline == inline {
            return;
        }
        self.phantom_inline = inline;
        if !self.lines.is_empty() {
            self.lines = (0..text.len_lines())
                .map(|row| self.make_line(text, row))
                .collect();
            self.heights = Heights::build(&self.lines);
        }
    }
    #[inline]
    pub fn has_phantom_inline(&self) -> bool {
        !self.phantom_inline.is_empty()
    }
    #[inline]
    pub fn phantom_gap(&self, at: usize) -> usize {
        self.phantom_inline
            .iter()
            .filter(|(p, _)| *p == at)
            .map(|(_, n)| n)
            .sum()
    }
    /// Inserts read-only original lines before physical row boundaries.
    pub fn set_inline(&mut self, mut blocks: Vec<(usize, usize)>) {
        blocks.sort_unstable();
        self.inline = blocks;
    }
    fn raw_before(&self, row: usize) -> usize {
        if self.lines.is_empty() {
            row.min(self.count)
        } else {
            self.heights.before(row.min(self.count))
        }
    }
    pub fn inline_at(&self, visual: usize) -> Option<(usize, usize)> {
        let mut shift = 0;
        for &(row, count) in &self.inline {
            let start = self.raw_before(row) + shift;
            if visual < start {
                break;
            }
            if visual < start + count {
                return Some((row, visual - start));
            }
            shift += count;
        }
        None
    }
    /// Skips read-only original rows during vertical caret movement.
    pub fn editable_visual(&self, visual: usize, forward: bool) -> usize {
        let mut target = visual.min(self.total().saturating_sub(1));
        for _ in 0..=self.inline.len() {
            let mut shift = 0;
            let mut found = None;
            for &(row, count) in &self.inline {
                let start = self.raw_before(row) + shift;
                if (start..start + count).contains(&target) {
                    found = Some(start..start + count);
                    break;
                }
                shift += count;
            }
            let Some(block) = found else {
                return target;
            };
            target = if forward {
                if block.end < self.total() {
                    block.end
                } else {
                    block.start.saturating_sub(1)
                }
            } else if block.start > 0 {
                block.start - 1
            } else {
                block.end.min(self.total().saturating_sub(1))
            };
        }
        target
    }
    pub fn total(&self) -> usize {
        self.raw_total() + self.inline.iter().map(|(_, n)| n).sum::<usize>()
    }
    fn raw_total(&self) -> usize {
        if self.lines.is_empty() {
            self.count
        } else {
            self.heights.before(self.count)
        }
    }
    pub fn locate(&self, visual: usize) -> (usize, usize) {
        let visual = visual.min(self.total().saturating_sub(1));
        let mut shift = 0;
        let mut raw = visual;
        for &(row, count) in &self.inline {
            let start = self.raw_before(row) + shift;
            if visual < start {
                break;
            }
            if visual < start + count {
                raw = self.raw_before(row).saturating_sub(1);
                break;
            }
            shift += count;
            raw = visual - shift;
        }
        let visual = raw.min(self.raw_total().saturating_sub(1));
        if self.lines.is_empty() {
            (visual, 0)
        } else {
            self.heights.locate(visual)
        }
    }
    pub fn segment(&self, text: &Text, row: usize, segment: usize) -> Segment {
        if self.lines.is_empty() {
            Segment {
                bytes: 0..body_len(text, row),
                indent: 0,
            }
        } else {
            let mut segment = self.lines[row].segments[segment].clone();
            if segment.bytes.end == usize::MAX {
                segment.bytes.end = text.line_len_bytes(row);
            }
            segment
        }
    }
    pub fn hidden(&self, row: usize) -> &[Range<usize>] {
        self.lines.get(row).map_or(&[], |l| &l.hidden)
    }
    pub fn visual_at(&self, text: &Text, byte: usize) -> (usize, Segment) {
        let mut row = text.byte_to_line(byte);
        let mut col = byte - text.line_to_byte(row);
        if !self.lines.is_empty()
            && self.lines[row].segments.is_empty()
            && let Some(fold) = self
                .folds
                .get(self.folds.partition_point(|f| f.end() <= byte))
                .filter(|f| f.begin() <= byte && byte < f.end())
        {
            row = text.byte_to_line(fold.begin());
            col = fold.begin() - text.line_to_byte(row);
        }
        let before = if self.lines.is_empty() {
            row
        } else {
            self.heights.before(row)
        };
        let segment = if self.lines.is_empty() {
            0
        } else {
            self.lines[row]
                .segments
                .partition_point(|s| s.bytes.end <= col)
                .min(self.lines[row].segments.len().saturating_sub(1))
        };
        let shift: usize = self
            .inline
            .iter()
            .filter(|(boundary, _)| *boundary <= row)
            .map(|(_, n)| n)
            .sum();
        (before + segment + shift, self.segment(text, row, segment))
    }
    pub fn column(&self, text: &Text, byte: usize) -> usize {
        let byte = self
            .folds
            .get(self.folds.partition_point(|f| f.end() <= byte))
            .filter(|f| f.begin() < byte && byte < f.end())
            .map_or(byte, |f| f.begin());
        let row = text.byte_to_line(byte);
        let col = byte - text.line_to_byte(row);
        if self.options.width == 0 && text.line_len_bytes(row) >= crate::large::LONG_LINE {
            return self.long_query(text, row, col, |i, a, c| {
                c + i.byte_to_column(col.saturating_sub(a))
            });
        }
        let line = text.line(row).to_string();
        let (_, segment) = self.visual_at(text, byte);
        let start = text.line_to_byte(row);
        let gaps: usize = self
            .phantom_inline
            .iter()
            .filter(|(at, _)| *at >= start + segment.bytes.start && *at < byte)
            .map(|(_, n)| n)
            .sum();
        gaps + segment.indent
            + units(&line, self.hidden(row), self.options.tab)
                .iter()
                .filter(|u| {
                    u.bytes.start >= segment.bytes.start
                        && u.bytes.end <= col.min(segment.bytes.end)
                })
                .map(|u| u.width)
                .sum::<usize>()
    }
    pub fn offset(&self, text: &Text, visual: usize, x: f64) -> usize {
        let (row, n) = self.locate(visual);
        let segment = self.segment(text, row, n);
        if self.options.width == 0 && text.line_len_bytes(row) >= crate::large::LONG_LINE {
            let x = if x.is_finite() { x.max(0.) } else { 0. };
            return text.line_to_byte(row)
                + self.long_query(text, row, x as usize, |i, a, c| {
                    a + i.column_to_byte(((x + 0.5) as usize).saturating_sub(c))
                });
        }
        let line = text.line(row).to_string();
        let mut col = segment.indent as f64;
        let x = if x.is_finite() { x.max(0.) } else { 0. };
        for unit in units(&line, self.hidden(row), self.options.tab)
            .iter()
            .filter(|u| u.bytes.start >= segment.bytes.start && u.bytes.end <= segment.bytes.end)
        {
            let gap = self.phantom_gap(text.line_to_byte(row) + unit.bytes.start) as f64;
            if x <= col + gap {
                return text.line_to_byte(row) + unit.bytes.start;
            }
            col += gap;
            if x <= col + unit.width as f64 / 2. {
                return text.line_to_byte(row) + unit.bytes.start;
            }
            col += unit.width as f64;
        }
        let end = text.line_to_byte(row) + segment.bytes.end;
        if segment.bytes.end < body_len(text, row) {
            text.prev_grapheme_boundary(end)
        } else {
            end
        }
    }
    fn long_query<T>(
        &self,
        text: &Text,
        row: usize,
        byte: usize,
        f: impl FnOnce(&vel_largefile::LongLineIndex, usize, usize) -> T,
    ) -> T {
        self.long.query(
            text,
            self.identity.expect("configured layout"),
            row,
            self.options.tab,
            byte,
            f,
        )
    }
    pub fn wrapped(&self) -> bool {
        self.options.width > 0
    }
    pub fn long_window(
        &self,
        text: &Text,
        row: usize,
        first: usize,
        columns: usize,
    ) -> crate::large::Window {
        self.long_query(text, row, first, |index, a, c| {
            let range = index.visible_window(first.saturating_sub(c), columns);
            // A giant combining cluster may occupy a single cell. Never flatten it on UI.
            let end = index.text().floor_char_boundary(
                (index.line_start() + range.end).min(index.line_start() + range.start + 64 * 1024),
            );
            crate::large::Window {
                text: index
                    .text()
                    .slice(index.line_start() + range.start..end)
                    .to_string(),
                bytes: a + range.start..a + end - index.line_start(),
                column: c + index.byte_to_column(range.start),
            }
        })
    }
    pub fn long_motion(&self, text: &Text, byte: usize, forward: bool, word: bool) -> usize {
        let row = text.byte_to_line(byte);
        let start = text.line_to_byte(row);
        if forward && byte == start + body_len(text, row) && byte < text.len_bytes() {
            return byte + 1;
        }
        if !forward && byte == start && start > 0 {
            return start - 1;
        }
        self.long_query(text, row, byte - start, |i, a, _| {
            let local = (byte - start).saturating_sub(a);
            start
                + a
                + match (forward, word) {
                    (true, true) => i.next_word_boundary(local),
                    (false, true) => i.prev_word_boundary(local),
                    (true, false) => i.next_grapheme_boundary(local),
                    (false, false) => i.prev_grapheme_boundary(local),
                }
        })
    }
    pub fn visible_bytes(&self, text: &Text, first: usize, count: usize) -> Range<usize> {
        let (a, _) = self.locate(first);
        let (b, _) = self.locate(first + count);
        text.line_to_byte(a)..if b + 1 < text.len_lines() {
            text.line_to_byte(b + 1)
        } else {
            text.len_bytes()
        }
    }
}
impl VerticalLayout for Layout {
    fn x_of(&self, text: &Text, offset: usize) -> f64 {
        self.column(text, offset) as f64
    }
    fn offset_at(&self, text: &Text, line: usize, x: f64) -> usize {
        self.offset(text, line, x)
    }
    fn lines_per_page(&self) -> usize {
        self.page.max(1)
    }
}
/// Graphemes are indivisible; folded ranges occupy one ellipsis cell.
pub fn units<'a>(line: &'a str, hidden: &[Range<usize>], tab: usize) -> Vec<Unit<'a>> {
    let mut units = Vec::new();
    let mut column = 0;
    let tab = tab.max(1);
    let mut fold_index = 0;
    let mut at = 0;
    while at < line.len() {
        if let Some(fold) = hidden.get(fold_index)
            && at >= fold.start
        {
            units.push(Unit {
                bytes: fold.clone(),
                text: "…",
                width: 1,
            });
            column += 1;
            at = fold.end;
            fold_index += 1;
            if fold.is_empty() { /* A fold starting at EOL still draws its marker below. */ }
            continue;
        }
        let g = line[at..].graphemes(true).next().expect("grapheme");
        if g == "\n" {
            break;
        }
        let width = if g == "\t" {
            tab - column % tab
        } else {
            g.width().max(1)
        };
        units.push(Unit {
            bytes: at..at + g.len(),
            text: g,
            width,
        });
        column += width;
        at += g.len();
    }
    if let Some(fold) = hidden.get(fold_index)
        && fold.start == line.trim_end_matches('\n').len()
    {
        units.push(Unit {
            bytes: fold.clone(),
            text: "…",
            width: 1,
        });
    }
    units
}

/// Physical line body length without allocating or scanning its contents.
pub fn body_len(text: &Text, row: usize) -> usize {
    let start = text.line_to_byte(row);
    let len = text.line_len_bytes(row);
    len - usize::from(len > 0 && text.byte_at(start + len - 1) == b'\n')
}

#[cfg(test)]
mod tests {
    use super::*;
    use vel_buffer::{Buffer, Change, Edit, EditKind, Selection};
    fn configure(layout: &mut Layout, b: &Buffer, width: usize, folds: &[Region]) {
        layout.configure(
            b.text(),
            (b.id(), b.change_id()),
            Options {
                width,
                tab: 4,
                indent: true,
            },
            folds,
        );
    }
    #[test]
    fn unicode_wrap_hit_testing_and_vertical_layout_share_boundaries() {
        let b = Buffer::new("  你好 e\u{301} 👩‍💻 abcdef\nnext");
        let mut l = Layout::default();
        configure(&mut l, &b, 8, &[]);
        assert!(l.total() > 2);
        for visual in 0..l.total() {
            let at = l.offset_at(b.text(), visual, 2.);
            assert!(b.text().is_char_boundary(at));
            assert_eq!(l.visual_at(b.text(), at).0, visual);
        }
        let first = l.segment(b.text(), 0, 0);
        assert!(b.text().is_char_boundary(first.bytes.end));
        assert!(b.text().slice(first.bytes).to_string().contains("你好"));
    }
    #[test]
    fn one_character_rewraps_one_line_and_matches_full_layout() {
        let mut b = Buffer::new("long words wrap here 你好\n".repeat(100_000));
        let id = b.new_view();
        let mut l = Layout::default();
        configure(&mut l, &b, 12, &[]);
        let old = b.text().clone();
        let byte = old.line_to_byte(50_000) + 2;
        b.transact(
            id,
            EditKind::Other,
            &[Edit {
                range: byte..byte,
                text: "x",
            }],
            Selection::from_regions([Region::caret(byte + 1)]),
        );
        l.change(
            &old,
            b.text(),
            (b.id(), b.change_id()),
            &ChangeSet::new(vec![Change {
                old_range: byte..byte,
                new_len: 1,
            }]),
        );
        assert_eq!(l.rewrapped, 1);
        let mut fresh = Layout::default();
        configure(&mut fresh, &b, 12, &[]);
        assert_eq!(l.total(), fresh.total());
        for row in [0, 50_000, 99_999] {
            assert_eq!(
                l.visual_at(b.text(), b.text().line_to_byte(row)).0,
                fresh.visual_at(b.text(), b.text().line_to_byte(row)).0
            );
        }
    }
    #[test]
    fn folds_project_rows_and_inline_attributes_into_ellipsis() {
        let b = Buffer::new("fn a() {\n    hello();\n}\n<tag a='b'>text</tag>");
        let mut l = Layout::default();
        configure(&mut l, &b, 0, &[Region::new(8, 22)]);
        assert_eq!(l.total(), 3);
        assert_eq!(l.locate(1).0, 2);
        let hidden = 4..10;
        let units = units("<tag a='b'>", std::slice::from_ref(&hidden), 4);
        assert_eq!(units.iter().map(|u| u.text).collect::<String>(), "<tag…>");
    }
}
