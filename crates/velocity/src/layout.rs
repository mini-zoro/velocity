//! Incremental grapheme wrapping and fold projection with logarithmic row lookup.
use std::{ops::Range, sync::Arc};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
use vel_buffer::{BufferId, ChangeId, ChangeSet, Region, VerticalLayout};
use vel_render::invisibles::Invisible;
use vel_text::Text;

pub const FOLDS: &str = "velocity.folds";
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Zero disables wrapping; otherwise this is the visual width in cells.
    pub width: usize,
    pub tab: usize,
    pub indent: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub bytes: Range<usize>,
    pub indent: usize,
    /// Physical display column before this segment; computed during wrapping.
    pub source_column: usize,
    /// Projected cells in the segment, excluding continuation indentation.
    pub columns: usize,
}
#[derive(Debug)]
pub struct Unit<'a> {
    pub bytes: Range<usize>,
    pub text: &'a str,
    pub width: usize,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Line {
    segments: Arc<[Segment]>,
    hidden: Vec<Range<usize>>,
}
// Editing-only metadata must not enlarge the geometry returned to display code.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Dependency {
    end: usize,
    inspected_until: usize,
}
// Most physical rows have one segment. Keep their dependency inline so the
// geometry allocation retains the same locality as the ordinary Line cache.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Dependencies {
    #[default]
    Empty,
    One(Dependency),
    Many(Arc<[Dependency]>),
}
impl From<Vec<Dependency>> for Dependencies {
    fn from(value: Vec<Dependency>) -> Self {
        match value.len() {
            0 => Self::Empty,
            1 => Self::One(value[0]),
            _ => Self::Many(value.into()),
        }
    }
}
impl std::ops::Deref for Dependencies {
    type Target = [Dependency];
    fn deref(&self) -> &[Dependency] {
        match self {
            Self::Empty => &[],
            Self::One(value) => std::slice::from_ref(value),
            Self::Many(values) => values,
        }
    }
}
impl Dependencies {
    fn as_mut_slice(&mut self) -> &mut [Dependency] {
        match self {
            Self::Empty => &mut [],
            Self::One(value) => std::slice::from_mut(value),
            Self::Many(values) => Arc::make_mut(values),
        }
    }
}
#[derive(Clone, Debug)]
struct WrapCache {
    tail: (usize, usize),
    dependencies: Dependencies,
    invisible: Option<Arc<Vec<(usize, usize)>>>,
}
impl WrapCache {
    /// Replace gaps in the rewrapped section and rebase the untouched suffix.
    fn edit_gaps(
        &mut self,
        begin: usize,
        old_end: usize,
        new_end: usize,
        new: Vec<(usize, usize)>,
    ) {
        let Some(gaps) = &mut self.invisible else {
            self.invisible = (!new.is_empty()).then(|| Arc::new(new));
            return;
        };
        let first = gaps.partition_point(|&(at, _)| at < begin);
        let last = gaps.partition_point(|&(at, _)| at < old_end);
        if first == last && new.is_empty() && (old_end == new_end || last == gaps.len()) {
            return;
        }
        let gaps = Arc::make_mut(gaps);
        let suffix = first + new.len();
        gaps.splice(first..last, new);
        if old_end != new_end {
            for (at, _) in &mut gaps[suffix..] {
                *at = *at - old_end + new_end;
            }
        }
        if gaps.is_empty() {
            self.invisible = None;
        }
    }
}
impl Default for WrapCache {
    fn default() -> Self {
        Self {
            tail: (usize::MAX, 0),
            dependencies: Dependencies::Empty,
            invisible: None,
        }
    }
}
#[derive(Debug, Default)]
struct Heights {
    tree: Vec<usize>,
}
impl Heights {
    fn build(lines: &[Line]) -> Self {
        Self::from_sizes(lines.iter().map(|line| line.segments.len()))
    }
    fn from_sizes(sizes: impl ExactSizeIterator<Item = usize>) -> Self {
        let mut tree = vec![0; sizes.len() + 1];
        for (row, size) in sizes.enumerate() {
            let i = row + 1;
            tree[i] += size;
            let parent = i + i.isolate_lowest_one();
            if parent < tree.len() {
                tree[parent] += tree[i];
            }
        }
        Self { tree }
    }
    fn before(&self, row: usize) -> usize {
        let mut i = row.min(self.tree.len().saturating_sub(1));
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
    pub geometry_revision: u64,
    identity: Option<(BufferId, ChangeId)>,
    pub long: crate::large::LongLines,
    options: Options,
    lines: Vec<Line>,
    // Allocated only for wrapped rows; unwrapped geometry has its original size.
    wrap: Vec<WrapCache>,
    heights: Heights,
    // Physical byte weights (including a virtual final LF), independent of wraps.
    byte_rows: Heights,
    folds: Vec<Region>,
    requested_folds: Vec<Region>,
    count: usize,
    inline: Vec<(usize, usize)>,
    phantom_inline: Vec<(usize, usize)>,
    invisible: Option<Invisible>,
    pub rewrapped: usize,
    pub page: usize,
    /// Last viewport width in cells, also used for commands between frames.
    pub viewport_width: usize,
}
impl Layout {
    pub fn current(&self, identity: (BufferId, ChangeId), options: Options) -> bool {
        self.identity == Some(identity) && self.options == options
    }
    pub fn has_folds(&self) -> bool {
        !self.folds.is_empty()
    }
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
        if self.identity == Some(identity)
            && self.options == options
            && self.requested_folds == folds
        {
            return;
        }
        self.geometry_revision = self.geometry_revision.wrapping_add(1);
        if self.identity != Some(identity) {
            self.long.reset();
        }
        self.identity = Some(identity);
        self.options = options;
        self.requested_folds = folds.to_vec();
        self.project_folds(text);
        self.count = text.len_lines();
        self.rewrapped = 0;
        self.byte_rows.tree.clear();
        self.wrap.clear();
        if options.width == 0 && folds.is_empty() {
            self.lines.clear();
            self.heights.tree.clear();
            return;
        }
        if options.width == 0 {
            self.lines = vec![Self::unwrapped_line(); self.count];
            // https://www.sublimetext.com/download (rendering large folded regions).
            // Measure only exposed edges, using the same geometry as an edit.
            let empty = Line::default();
            let mut measured = None;
            for fold in &self.folds {
                let first = text.byte_to_line(fold.begin());
                let last = text.byte_to_line(fold.end());
                if measured != Some(first) {
                    self.lines[first] = self.make_unwrapped_line(text, first);
                }
                if first < last {
                    self.lines[first + 1..last].fill(empty.clone());
                    self.lines[last] = self.make_unwrapped_line(text, last);
                }
                measured = Some(last);
            }
            self.heights = Heights::build(&self.lines);
            return;
        }
        (self.lines, self.wrap) = (0..self.count).map(|row| self.make_line(text, row)).unzip();
        self.rewrapped = self.count;
        self.heights = Heights::build(&self.lines);
        self.rebuild_byte_rows();
    }
    pub fn invalidate(&mut self) {
        if self.identity.take().is_some() {
            self.geometry_revision = self.geometry_revision.wrapping_add(1);
        }
    }
    fn project_folds(&mut self, text: &Text) {
        fn plain_boundary(text: &Text, at: usize) -> bool {
            if at == 0 || at == text.len_bytes() {
                return true;
            }
            let (chunk, start) = text.chunk_at_byte(at);
            let bytes = chunk.as_bytes();
            let next = bytes[at - start];
            if !next.is_ascii() {
                return false;
            }
            let before = if at == start {
                text.byte_at(at - 1)
            } else {
                bytes[at - start - 1]
            };
            before.is_ascii() && (before != b'\r' || next != b'\n')
        }
        self.folds = vel_render::fold::outermost(self.requested_folds.iter().filter_map(|r| {
            let a = r.begin().min(text.len_bytes());
            let b = r.end().min(text.len_bytes());
            (a < b).then(|| {
                // Fold markers must not leave dangling combining marks or split emoji.
                let a = if plain_boundary(text, a) {
                    a
                } else {
                    text.prev_grapheme_boundary(
                        text.next_grapheme_boundary(text.floor_char_boundary(a)),
                    )
                };
                let b = if plain_boundary(text, b) {
                    b
                } else {
                    text.next_grapheme_boundary(
                        text.prev_grapheme_boundary(text.ceil_char_boundary(b)),
                    )
                };
                a..b
            })
        }))
        .into_iter()
        .map(Region::from)
        .collect();
    }
    fn unwrapped_line() -> Line {
        static SEGMENT: std::sync::LazyLock<Arc<[Segment]>> = std::sync::LazyLock::new(|| {
            Arc::from([Segment {
                bytes: 0..usize::MAX,
                indent: 0,
                source_column: 0,
                columns: usize::MAX,
            }])
        });
        Line {
            segments: SEGMENT.clone(),
            hidden: Vec::new(),
        }
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
        self.requested_folds = self
            .requested_folds
            .iter()
            .map(|r| changes.transform_region(*r))
            .filter(|r| !r.empty())
            .collect();
        if !self.requested_folds.is_empty() || !self.folds.is_empty() {
            self.project_folds(after);
        }
        self.count = after.len_lines();
        self.rewrapped = 0;
        if self.options.width == 0 && self.folds.is_empty() {
            self.lines.clear();
            self.heights.tree.clear();
            return;
        }
        if changes.changes().windows(2).any(|pair| {
            pair[0].old_range.end > pair[1].old_range.start
                || pair[0].old_range.start == pair[1].old_range.start
        }) {
            // Such externally supplied maps are valid to represent, but their
            // left-biased point transform is not a row-edit oracle.
            if self.options.width != 0 || !self.folds.is_empty() {
                (self.lines, self.wrap) = (0..self.count)
                    .map(|row| self.make_line(after, row))
                    .unzip();
                if self.options.width == 0 {
                    self.wrap.clear();
                }
                self.heights = Heights::build(&self.lines);
                self.rebuild_byte_rows();
                self.rewrapped = self.count;
            }
            return;
        }
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
        // Pure insertions cannot cancel a newline. Equal line counts establish
        // the common typing case without four rope lookups for every caret.
        if old.len_lines() == after.len_lines()
            && (changes.changes().iter().all(|c| c.old_range.is_empty())
                || changes.changes().iter().all(|c| {
                    let at = changes.transform_point(c.old_range.start, vel_buffer::Bias::Left);
                    old.byte_to_line(c.old_range.start) == old.byte_to_line(c.old_range.end)
                        && after.byte_to_line(at) == after.byte_to_line(at + c.new_len)
                }))
        {
            // Descend in old row order: changes to later byte weights cannot
            // move the old coordinates of rows still to be visited.
            let cached_rows = !self.byte_rows.tree.is_empty();
            let reader = ByteReader::new(
                after,
                if cached_rows {
                    changes.changes().len()
                } else {
                    0
                },
            );
            let mut end_index = changes.changes().len();
            while end_index > 0 {
                let at = changes.changes()[end_index - 1].old_range.start;
                let row = if cached_rows {
                    self.byte_rows.locate(at).0
                } else {
                    old.byte_to_line(at)
                };
                let old_start = if cached_rows {
                    self.byte_rows.before(row)
                } else {
                    old.line_to_byte(row)
                };
                let mut index = end_index - 1;
                while index > 0 && changes.changes()[index - 1].old_range.start >= old_start {
                    index -= 1;
                }
                let old_len = self.lines[row].segments.last().map_or(0, |s| s.bytes.end);
                if !self.rewrap_suffix(
                    &reader,
                    row,
                    old_start,
                    &changes.changes()[index..end_index],
                    changes,
                ) {
                    let (line, cache) = self.make_line(after, row);
                    self.heights
                        .update(row, self.lines[row].segments.len(), line.segments.len());
                    self.lines[row] = line;
                    if let Some(slot) = self.wrap.get_mut(row) {
                        *slot = cache;
                    }
                }
                if cached_rows {
                    let len = self.lines[row]
                        .segments
                        .last()
                        .expect("unfolded row")
                        .bytes
                        .end;
                    self.byte_rows.update(row, old_len + 1, len + 1);
                }
                self.rewrapped += 1;
                end_index = index;
            }
        } else {
            let (replacement, caches): (Vec<_>, Vec<_>) = (start..new_end)
                .map(|row| self.make_line(after, row))
                .unzip();
            self.rewrapped = replacement.len();
            self.lines.splice(start..end, replacement);
            if self.options.width > 0 {
                self.wrap.splice(start..end, caches);
            }
            self.heights = Heights::build(&self.lines);
            self.rebuild_byte_rows();
        }
    }
    fn rebuild_byte_rows(&mut self) {
        self.byte_rows = if self.options.width > 0 && self.folds.is_empty() {
            Heights::from_sizes(
                self.lines
                    .iter()
                    .map(|line| line.segments.last().expect("unfolded row").bytes.end + 1),
            )
        } else {
            Heights::default()
        };
    }
    /// Retain the unaffected prefix and stop at a stable old break. Only the
    /// previous segment needs reconsidering when a word joins across a break.
    /// Fold/phantom projection and cross-segment deletions use the full row oracle.
    fn rewrap_suffix(
        &mut self,
        reader: &ByteReader<'_>,
        row: usize,
        old_start: usize,
        edits: &[vel_buffer::Change],
        map: &ChangeSet,
    ) -> bool {
        // Keep the text-only loop free of placeholder work, including its unit
        // storage and cache updates, when none of the drawing options is on.
        if self.invisible.is_some() {
            self.rewrap_suffix_impl::<true>(reader, row, old_start, edits, map)
        } else {
            self.rewrap_suffix_impl::<false>(reader, row, old_start, edits, map)
        }
    }
    fn rewrap_suffix_impl<const GAPS: bool>(
        &mut self,
        reader: &ByteReader<'_>,
        row: usize,
        old_start: usize,
        edits: &[vel_buffer::Change],
        map: &ChangeSet,
    ) -> bool {
        let line = &self.lines[row];
        if self.options.width == 0
            || !self.folds.is_empty()
            || !self.phantom_inline.is_empty()
            || !line.hidden.is_empty()
            || line.segments.is_empty()
            || edits
                .windows(2)
                .any(|p| p[0].old_range.start == p[1].old_range.start)
        {
            return false;
        }
        let first = edits[0].old_range.start - old_start;
        let containing = |at| {
            line.segments
                .partition_point(|s| s.bytes.end <= at)
                .min(line.segments.len() - 1)
        };
        if edits.iter().any(|c| {
            !c.old_range.is_empty()
                && c.old_range.end - old_start
                    > line.segments[containing(c.old_range.start - old_start)]
                        .bytes
                        .end
        }) {
            return false;
        }
        let containing = containing(first);
        // A word rollback can inspect beyond intervening short segments. A
        // grapheme can also be wider than the entire wrap width. Prefix maxima
        // capture both dependencies without scanning a growing row on each key.
        let restart = self.wrap[row].dependencies[..containing]
            .partition_point(|dependency| dependency.inspected_until < first);
        let begin = line.segments[restart].bytes.start;
        let mut column = line.segments[restart].source_column;
        let start = map.transform_point(old_start, vel_buffer::Bias::Left);
        // The cached final break is the exact body length; asking the rope for
        // every growing line's end would itself add a per-key prefix scan.
        let old_len = line.segments.last().expect("wrapped line").bytes.end;
        let removed = edits.iter().map(|c| c.old_range.len()).sum::<usize>();
        let inserted = edits.iter().map(|c| c.new_len).sum::<usize>();
        let Some(len) = old_len
            .checked_sub(removed)
            .and_then(|n| n.checked_add(inserted))
        else {
            return false;
        };
        let short = len < crate::large::LONG_LINE;
        if GAPS && short != (old_len < crate::large::LONG_LINE) {
            // Crossing the drawing cutoff changes gaps throughout the row.
            return false;
        }
        let invisible = if GAPS && short { self.invisible } else { None };
        let gap = |unit: &Unit<'_>| {
            invisible
                .and_then(|opt| opt.cell(unit.text, unit.width))
                .map_or(0, |(_, gap)| gap)
        };
        let Some(end) = start
            .checked_add(len)
            .filter(|&end| end <= reader.text.len_bytes())
        else {
            return false;
        };
        // A stale or projected segment is not a physical line-length oracle.
        // Validate the cached window endpoints before taking any string slices.
        if begin > len
            || !reader.is_char_boundary(start + begin)
            || !reader.is_char_boundary(end)
            || (start > 0 && reader.byte_at(start - 1) != b'\n')
            || (end < reader.text.len_bytes() && reader.byte_at(end) != b'\n')
        {
            return false;
        }
        let continuation_indent = line.segments.get(1).map_or_else(
            || self.line_indent(reader, start, len),
            |segment| segment.indent,
        );
        // A stable previous break and an append that still fits cannot change
        // earlier units. This applies to spaces, tabs and Unicode alike; include
        // the entire old final grapheme so a joining mark can change its width.
        // Overflow, uncertain indentation and projected content use rewrapping.
        let last_segment = line.segments.last().expect("wrapped line");
        let cached_tail = self.wrap[row].tail;
        let line_tail = if cached_tail.0 == usize::MAX
            && first == old_len
            && edits.len() == 1
            && edits[0].old_range.is_empty()
        {
            // First edit since full layout: measure only the final old segment,
            // then subsequent appends reuse the final grapheme directly.
            let content = reader.copy_bytes(start + last_segment.bytes.start..start + old_len);
            units_from(&content, &[], self.options.tab, last_segment.source_column)
                .last()
                .map_or((0, 0), |unit| {
                    (last_segment.bytes.start + unit.bytes.start, unit.width)
                })
        } else {
            cached_tail
        };

        if edits.len() == 1
            && edits[0].old_range.is_empty()
            && first == old_len
            && (line.segments.len() == 1 || restart == line.segments.len() - 1)
            && (continuation_indent == 0 || line.segments.len() > 1)
            && let (tail_start, tail_width) = line_tail
            && tail_start != usize::MAX
            && tail_start >= last_segment.bytes.start
            && len - tail_start <= self.options.width.saturating_add(1).saturating_mul(8)
        {
            let content = reader.copy_bytes(start + tail_start..start + len);
            let source_column = last_segment.source_column + last_segment.columns - tail_width;
            let units = units_from(&content, &[], self.options.tab, source_column);
            let columns =
                last_segment.columns - tail_width + units.iter().map(|u| u.width).sum::<usize>();
            let mut new_gaps = Vec::new();
            let mut gap_width = 0;
            if GAPS && let Some(gaps) = &self.wrap[row].invisible {
                let first = gaps.partition_point(|&(at, _)| at < last_segment.bytes.start);
                gap_width += gaps[first..]
                    .iter()
                    .take_while(|&&(at, _)| at < tail_start)
                    .map(|&(_, width)| width)
                    .sum::<usize>();
            }
            if invisible.is_some() {
                for unit in &units {
                    let width = gap(unit);
                    if width != 0 {
                        gap_width += width;
                        new_gaps.push((tail_start + unit.bytes.start, width));
                    }
                }
            }
            if !units.is_empty() && columns + gap_width + last_segment.indent <= self.options.width
            {
                let tail = units
                    .last()
                    .map_or((0, 0), |u| (tail_start + u.bytes.start, u.width));
                let line = &mut self.lines[row];
                let segments = Arc::make_mut(&mut line.segments);
                let segment = segments.last_mut().expect("wrapped line");
                segment.bytes.end = len;
                segment.columns = columns;
                *self.wrap[row]
                    .dependencies
                    .as_mut_slice()
                    .last_mut()
                    .expect("wrapped dependency") = Dependency {
                    end: len,
                    inspected_until: len,
                };
                self.wrap[row].tail = tail;
                if invisible.is_some() {
                    self.wrap[row].edit_gaps(tail_start, old_len, len, new_gaps);
                }
                return true;
            }
        }
        let last = edits.last().expect("edited row").old_range.end - old_start;
        let last_after = last + inserted - removed;
        let indent = if restart == 0 && (first == 0 || continuation_indent != 0) {
            self.line_indent(reader, start, len)
        } else {
            continuation_indent
        };
        // An edit in a later segment cannot change leading indentation: the
        // previous segment starts after an already capped/non-whitespace prefix.
        // If that is not established (e.g. a huge leading grapheme), use make_line.
        if restart > 0 && first == begin {
            return false;
        }
        let mut tail = Vec::new();
        let mut dependencies = Vec::new();
        let mut new_gaps = Vec::new();
        let mut at = begin;
        let mut reuse = line.segments.len();
        let mut final_unit = None;
        let tab = self.options.tab.max(1);
        while at < len {
            // Read only a bounded window, including its last complete grapheme.
            // In particular an interior edit does not copy the untouched tail.
            let mut window = self
                .options
                .width
                .saturating_add(1)
                .saturating_mul(4)
                .max(32);
            let (content_len, units, gaps) = loop {
                let limit = start + at.saturating_add(window).min(len);
                let mut end = if limit == start + len {
                    limit
                } else {
                    reader.floor_char_boundary(limit)
                };
                if end < start + len {
                    end = reader.text.next_grapheme_boundary(end).min(start + len);
                }
                let content = reader.copy_bytes(start + at..end);
                // Retain owned content outside this loop; unit ranges suffice
                // once grapheme widths and whitespace breaks have been read.
                let parts = units_from(&content, &[], tab, column);
                let gaps = if invisible.is_some() {
                    parts
                        .iter()
                        .filter_map(|unit| {
                            let width = gap(unit);
                            (width != 0).then_some((unit.bytes.start, width))
                        })
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                };
                let (end_unit, _) = wrap_end(
                    &parts,
                    0,
                    if at == 0 { 0 } else { indent },
                    self.options.width,
                    |at| {
                        if GAPS {
                            Self::invisible_gap(&gaps, at)
                        } else {
                            0
                        }
                    },
                );
                if end == start + len || end_unit < parts.len() {
                    let parts: Vec<_> = parts
                        .into_iter()
                        .map(|u| (u.bytes, u.width, u.text.chars().all(char::is_whitespace)))
                        .collect();
                    break (content.len(), parts, gaps);
                }
                window = window.saturating_mul(2);
            };
            let eof = at + content_len == len;
            if eof {
                final_unit = units.last().map(|u| (at + u.0.start, u.1));
            }
            let base = at;
            let mut from = 0;
            while from < units.len() {
                let left = if at == 0 { 0 } else { indent };
                let mut col = left;
                let mut end = from;
                let mut word = None;
                while end < units.len() {
                    let width = units[end].1
                        + if GAPS {
                            Self::invisible_gap(&gaps, units[end].0.start)
                        } else {
                            0
                        };
                    if col + width > self.options.width && end > from {
                        break;
                    }
                    col += width;
                    end += 1;
                    if units[end - 1].2 {
                        word = Some(end);
                    }
                }
                let lookahead = base + units[end.min(units.len() - 1)].0.end;
                if end == units.len() && !eof {
                    break;
                }
                if end < units.len()
                    && let Some(boundary) = word
                {
                    end = boundary;
                }
                let columns = units[from..end].iter().map(|u| u.1).sum::<usize>();
                if GAPS && !gaps.is_empty() {
                    let begin = units[from].0.start;
                    let finish = units[end - 1].0.end;
                    let first_gap = gaps.partition_point(|&(at, _)| at < begin);
                    new_gaps.extend(
                        gaps[first_gap..]
                            .iter()
                            .take_while(|&&(at, _)| at < finish)
                            .map(|&(at, width)| (base + at, width)),
                    );
                }
                at = base + units[end - 1].0.end;
                tail.push(Segment {
                    bytes: base + units[from].0.start..at,
                    indent: left,
                    source_column: column,
                    columns,
                });
                dependencies.push(Dependency {
                    end: lookahead,
                    inspected_until: 0,
                });
                column += columns;
                from = end;
                // Matching byte breaks alone are insufficient: a changed source
                // column can change all subsequent tab stops. Require that phase
                // and continuation indentation to agree before reusing a suffix.
                if at > last_after && at < len && indent == continuation_indent {
                    let old_at = at + removed - inserted;
                    let next = line.segments.partition_point(|s| s.bytes.end < old_at);
                    if let Some(segment) = line.segments.get(next)
                        && segment.bytes.end == old_at
                        && column % tab == (segment.source_column + segment.columns) % tab
                    {
                        reuse = next + 1;
                        break;
                    }
                }
            }
            if reuse < line.segments.len() {
                break;
            }
        }
        if tail.is_empty() {
            tail.push(Segment {
                bytes: 0..0,
                indent: 0,
                source_column: 0,
                columns: 0,
            });
            dependencies.push(Dependency::default());
        }
        let old_count = line.segments.len();
        let column_at_reuse = line.segments.get(reuse).map(|s| s.source_column);
        if invisible.is_some() {
            let old_end = line.segments.get(reuse).map_or(old_len, |s| s.bytes.start);
            self.wrap[row].edit_gaps(begin, old_end, at, new_gaps);
        }
        let line = &mut self.lines[row];
        self.wrap[row].tail = if reuse < old_count {
            if cached_tail.0 == usize::MAX {
                cached_tail
            } else {
                (cached_tail.0 + inserted - removed, cached_tail.1)
            }
        } else {
            final_unit.unwrap_or((0, 0))
        };
        // A changed row normally keeps its segment count. Mutate its unique
        // slices then; a moved/added break rebuilds only this row's two slices.
        let mut rebuilt_segments = if tail.len() == reuse - restart {
            None
        } else {
            let mut segments = line.segments.to_vec();
            segments.splice(restart..reuse, tail.iter().cloned());
            Some(segments)
        };
        let segments = if let Some(segments) = &mut rebuilt_segments {
            segments.as_mut_slice()
        } else {
            let segments = Arc::make_mut(&mut line.segments);
            segments[restart..reuse].clone_from_slice(&tail);
            segments
        };
        let cache = &mut self.wrap[row];
        let mut rebuilt_dependencies = if dependencies.len() == reuse - restart {
            None
        } else {
            let mut result = cache.dependencies.to_vec();
            result.splice(restart..reuse, dependencies.iter().copied());
            Some(result)
        };
        let metadata = if let Some(metadata) = &mut rebuilt_dependencies {
            metadata.as_mut_slice()
        } else {
            let metadata = cache.dependencies.as_mut_slice();
            metadata[restart..reuse].copy_from_slice(&dependencies);
            metadata
        };
        let suffix = segments.len() - (old_count - reuse);
        if let Some(old_column) = column_at_reuse {
            for (segment, dependency) in segments[suffix..].iter_mut().zip(&mut metadata[suffix..])
            {
                segment.bytes = segment.bytes.start + inserted - removed
                    ..segment.bytes.end + inserted - removed;
                segment.source_column = segment.source_column + column - old_column;
                dependency.end = dependency.end + inserted - removed;
            }
        }
        let mut inspected = if restart == 0 {
            0
        } else {
            metadata[restart - 1].inspected_until
        };
        for dependency in &mut metadata[restart..] {
            inspected = inspected.max(dependency.end);
            dependency.inspected_until = inspected;
        }
        self.heights.update(row, old_count, segments.len());
        if let Some(segments) = rebuilt_segments {
            line.segments = segments.into();
        }
        if let Some(dependencies) = rebuilt_dependencies {
            cache.dependencies = dependencies.into();
        }
        true
    }
    fn line_indent(&self, reader: &ByteReader<'_>, start: usize, len: usize) -> usize {
        if !self.options.indent || self.options.width <= 1 {
            return 0;
        }
        let mut column = 0;
        let tab = self.options.tab.max(1);
        let mut at = start;
        while at < start + len {
            let (chunk, base) = reader.chunk_at_byte(at);
            for byte in chunk.as_bytes()[at - base..].iter().take(start + len - at) {
                match byte {
                    b' ' => column += 1,
                    b'\t' => column += tab - column % tab,
                    _ => return column,
                }
                if column >= self.options.width / 2 {
                    return self.options.width / 2;
                }
                at += 1;
            }
        }
        column
    }
    fn make_line(&self, text: &Text, row: usize) -> (Line, WrapCache) {
        if self.options.width == 0 {
            (self.make_unwrapped_line(text, row), WrapCache::default())
        } else if self.invisible.is_some() {
            self.make_line_impl::<true>(text, row)
        } else {
            self.make_line_impl::<false>(text, row)
        }
    }
    fn fold_projection(
        &self,
        row: usize,
        start: usize,
        len: usize,
    ) -> Option<(usize, Vec<Range<usize>>)> {
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
                    return None;
                }
                visible_start = fold.end() - start;
                continue;
            }
            if fold.begin() >= start && fold.begin() <= start + len {
                hidden.push(
                    fold.begin() - start
                        ..fold
                            .end()
                            .min(start + len + usize::from(row + 1 < self.count))
                            - start,
                );
            }
        }
        Some((visible_start, hidden))
    }
    fn make_unwrapped_line(&self, text: &Text, row: usize) -> Line {
        if self.folds.is_empty() {
            return Self::unwrapped_line();
        }
        let start = text.line_to_byte(row);
        let (chunk, base) = text.chunk_at_byte(start);
        let tail = &chunk[start - base..];
        let owned;
        let content = if let Some((line, _)) = tail.split_once('\n') {
            line
        } else if base + chunk.len() == text.len_bytes() {
            tail
        } else {
            owned = text.line(row).to_string();
            &owned
        };
        let Some((visible_start, hidden)) = self.fold_projection(row, start, content.len()) else {
            return Line::default();
        };
        if visible_start == 0 && hidden.is_empty() {
            return Self::unwrapped_line();
        }
        let tab = self.options.tab.max(1);
        let width = |span: &str, initial: usize| {
            if !span.is_ascii() {
                return crate::editor::cells_with_tab_at(span, tab, initial);
            }
            let mut col = initial;
            for byte in span.bytes() {
                col += if byte == b'\t' { tab - col % tab } else { 1 };
            }
            col - initial
        };
        let mut columns = 0;
        let mut at = visible_start;
        for fold in &hidden {
            columns += width(&content[at..fold.start], columns) + 1;
            at = fold.end.min(content.len());
        }
        columns += width(&content[at..], columns);
        let end = hidden
            .last()
            .map_or(content.len(), |f| content.len().max(f.end));
        Line {
            segments: Arc::from([Segment {
                bytes: visible_start..end,
                indent: 0,
                source_column: 0,
                columns,
            }]),
            hidden,
        }
    }
    fn make_line_impl<const GAPS: bool>(&self, text: &Text, row: usize) -> (Line, WrapCache) {
        let start = text.line_to_byte(row);
        let content = text.line(row).to_string();
        let len = content.len();
        let Some((visible_start, hidden)) = self.fold_projection(row, start, len) else {
            return (Line::default(), WrapCache::default());
        };
        let units = units_in(
            &content[visible_start..len],
            visible_start,
            &hidden,
            self.options.tab,
            0,
        );
        let mut gaps = Vec::new();
        if GAPS
            && let Some(options) = self
                .invisible
                .filter(|_| len < crate::large::LONG_LINE)
                .filter(|options| options.contains(&content[..len]))
        {
            gaps.extend(units.iter().filter_map(|u| {
                let (_, gap) = options.cell(u.text, u.width)?;
                (gap != 0).then_some((u.bytes.start, gap))
            }));
        }
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
        let mut dependencies = Vec::new();
        let mut from = 0;
        if units.is_empty() {
            segments.push(Segment {
                bytes: visible_start..len,
                indent: 0,
                source_column: 0,
                columns: 0,
            });
            dependencies.push(Dependency::default());
        }
        let mut source_column = 0;
        while from < units.len() {
            let left = if from == 0 { 0 } else { indent };
            let (end, lookahead) = wrap_end(&units, from, left, self.options.width, |at| {
                self.phantom_gap(start + at)
                    + if GAPS {
                        Self::invisible_gap(&gaps, at)
                    } else {
                        0
                    }
            });
            let columns = units[from..end].iter().map(|u| u.width).sum::<usize>();
            segments.push(Segment {
                bytes: units[from].bytes.start..units[end - 1].bytes.end,
                indent: left,
                source_column,
                columns,
            });
            dependencies.push(Dependency {
                end: lookahead,
                inspected_until: 0,
            });
            source_column += columns;
            from = end;
        }
        let mut inspected = 0;
        for dependency in &mut dependencies {
            inspected = inspected.max(dependency.end);
            dependency.inspected_until = inspected;
        }
        (
            Line {
                segments: segments.into(),
                hidden,
            },
            WrapCache {
                tail: (usize::MAX, 0),
                dependencies: dependencies.into(),
                invisible: (!gaps.is_empty()).then(|| Arc::new(gaps)),
            },
        )
    }
    pub fn set_phantom_inline(&mut self, text: &Text, mut inline: Vec<(usize, usize)>) {
        inline.sort_unstable();
        if self.phantom_inline == inline {
            return;
        }
        self.geometry_revision = self.geometry_revision.wrapping_add(1);
        self.phantom_inline = inline;
        if !self.lines.is_empty() {
            (self.lines, self.wrap) = (0..text.len_lines())
                .map(|row| self.make_line(text, row))
                .unzip();
            if self.options.width == 0 {
                self.wrap.clear();
            }
            self.heights = Heights::build(&self.lines);
        }
    }
    #[inline]
    pub fn has_phantom_inline(&self) -> bool {
        !self.phantom_inline.is_empty()
    }
    #[inline]
    pub fn phantom_gap(&self, at: usize) -> usize {
        self.phantom_inline[self.phantom_inline.partition_point(|(p, _)| *p < at)..]
            .iter()
            .take_while(|(p, _)| *p == at)
            .map(|(_, n)| n)
            .sum()
    }
    pub fn set_invisibles(&mut self, options: Invisible) {
        let options = (options != Invisible::default()).then_some(options);
        if self.invisible != options {
            self.invisible = options;
            self.invalidate();
        }
    }

    // Wrapped rows keep their ordered, line-relative gaps with the wrap cache.
    // Unwrapped rows only need gaps for a visible row or a caret geometry query.
    pub fn invisible_gaps(
        &self,
        row: usize,
        text: &str,
        base: usize,
        source_column: usize,
    ) -> Option<Arc<Vec<(usize, usize)>>> {
        let options = self.invisible?;
        if let Some(cache) = self.wrap.get(row) {
            return cache.invisible.clone();
        }
        if text.len() >= crate::large::LONG_LINE || !options.contains(text) {
            return None;
        }
        let gaps = units_in(
            text,
            base,
            self.hidden(row),
            self.options.tab,
            source_column,
        )
        .into_iter()
        .filter_map(|u| {
            let (_, gap) = options.cell(u.text, u.width)?;
            (gap != 0).then_some((u.bytes.start, gap))
        })
        .collect::<Vec<_>>();
        (!gaps.is_empty()).then(|| Arc::new(gaps))
    }

    #[inline]
    pub fn invisible_gap(gaps: &[(usize, usize)], at: usize) -> usize {
        gaps.binary_search_by_key(&at, |(p, _)| *p)
            .map_or(0, |i| gaps[i].1)
    }
    /// Inserts read-only original lines before physical row boundaries.
    pub fn set_inline(&mut self, mut blocks: Vec<(usize, usize)>) {
        blocks.sort_unstable();
        if self.inline != blocks {
            self.geometry_revision = self.geometry_revision.wrapping_add(1);
            self.inline = blocks;
        }
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
        let row = row.min(text.len_lines().saturating_sub(1));
        if let Some(line) = self.lines.get(row)
            && let Some(segment) = line
                .segments
                .get(segment.min(line.segments.len().saturating_sub(1)))
        {
            let mut segment = segment.clone();
            if segment.bytes.end == usize::MAX {
                segment.bytes.end = text.line_len_bytes(row);
            } else if self.identity.is_none() {
                // Normal configured segments already contain valid byte ranges.
                // Only an invalidated cache needs a per-row text lookup here;
                // offset() also clamps against its actual line before returning.
                let len = text.line_len_bytes(row);
                // An unwrapped multiline marker can extend one byte past the
                // header's LF. Keep that projection span for folded_segment_end.
                segment.bytes.end = segment
                    .bytes
                    .end
                    .min(len.saturating_add(usize::from(!line.hidden.is_empty())));
            }
            segment.bytes.start = segment.bytes.start.min(segment.bytes.end);
            segment
        } else {
            Segment {
                bytes: 0..body_len(text, row),
                indent: 0,
                source_column: 0,
                columns: usize::MAX,
            }
        }
    }
    pub fn hidden(&self, row: usize) -> &[Range<usize>] {
        self.lines.get(row).map_or(&[], |line| &line.hidden)
    }
    pub fn visual_at(&self, text: &Text, byte: usize) -> (usize, Segment) {
        let byte = text.floor_char_boundary(byte.min(text.len_bytes()));
        let mut row = text.byte_to_line(byte);
        let mut col = byte - text.line_to_byte(row);
        if !self.lines.is_empty()
            && self.lines.get(row).is_some_and(|l| l.segments.is_empty())
            && let Some(fold) = self
                .folds
                .get(self.folds.partition_point(|f| f.end() < byte))
                .filter(|f| f.begin() <= byte && byte <= f.end())
        {
            let begin = text.floor_char_boundary(fold.begin().min(text.len_bytes()));
            row = text.byte_to_line(begin);
            col = begin - text.line_to_byte(row);
        }
        let before = if self.lines.is_empty() {
            row
        } else {
            self.heights.before(row)
        };
        let segment = self.lines.get(row).map_or(0, |line| {
            line.segments
                .partition_point(|s| s.bytes.end <= col)
                .min(line.segments.len().saturating_sub(1))
        });
        let shift: usize = self
            .inline
            .iter()
            .filter(|(boundary, _)| *boundary <= row)
            .map(|(_, n)| n)
            .sum();
        (before + segment + shift, self.segment(text, row, segment))
    }
    pub fn column(&self, text: &Text, byte: usize) -> usize {
        self.column_in(text, byte, &mut None)
    }
    /// [`Layout::column`] for many positions: consecutive queries on one row (and, for wrapped
    /// or folded rows, one segment) share its display units and gaps through `row_cache`,
    /// instead of rebuilding the whole line each time.
    pub fn column_in(&self, text: &Text, byte: usize, row_cache: &mut Option<RowColumns>) -> usize {
        let byte = text.floor_char_boundary(byte.min(text.len_bytes()));
        let fold = self
            .folds
            .get(self.folds.partition_point(|f| f.end() < byte))
            .filter(|f| {
                f.begin() < byte
                    && (byte < f.end()
                        || (byte == f.end()
                            && !self.lines.is_empty()
                            && self
                                .lines
                                .get(text.byte_to_line(byte))
                                .is_some_and(|l| l.segments.is_empty())))
            });
        let after_marker = usize::from(fold.is_some_and(|f| byte == f.end()));
        let byte = fold.map_or(byte, |f| {
            text.floor_char_boundary(f.begin().min(text.len_bytes()))
        });
        let row = text.byte_to_line(byte);
        let col = byte - text.line_to_byte(row);
        if self.options.width == 0 && text.line_len_bytes(row) >= crate::large::LONG_LINE {
            return after_marker
                + self.long_query(text, row, col, |i, a, c| {
                    c + i.byte_to_column(col.saturating_sub(a))
                });
        }
        let (_, segment) = self.visual_at(text, byte);
        let start = text.line_to_byte(row);
        // [`Layout::unit_window`] measures a window for long wrapped rows and for folded rows
        // whose first segment starts inside a fold or that reach a segment behind a hidden
        // range; the cache then also depends on the segment's start and source column.
        let window = self.long_wrapped(row)
            || !self.folds.is_empty()
                && self.lines.get(row).is_some_and(|line| {
                    line.segments
                        .first()
                        .is_some_and(|first| first.bytes.start != 0)
                        || segment.bytes.start > 0
                            && line.hidden.iter().any(|r| r.start < segment.bytes.start)
                });
        let stale = row_cache.as_ref().is_none_or(|cache| {
            cache.row != row
                || window
                    && (cache.base != segment.bytes.start
                        || cache.source_column != segment.source_column)
        });
        if stale {
            let (line, base, source_column) = self.unit_window(text, row, start, &segment);
            let units = units_in(
                &line,
                base,
                self.hidden(row),
                self.options.tab,
                source_column,
            );
            let mut widths = Vec::with_capacity(units.len() + 1);
            widths.push(0);
            let mut total = 0;
            for unit in &units {
                total += unit.width;
                widths.push(total);
            }
            *row_cache = Some(RowColumns {
                row,
                base,
                source_column,
                bounds: units.iter().map(|u| (u.bytes.start, u.bytes.end)).collect(),
                widths,
                gaps: self.invisible_gaps(row, &line, base, source_column),
            });
        }
        let cache = row_cache.as_ref().expect("row columns");
        let inline: usize = self.phantom_inline[self
            .phantom_inline
            .partition_point(|(at, _)| *at < start + segment.bytes.start)..]
            .iter()
            .take_while(|(at, _)| *at < byte)
            .map(|(_, n)| n)
            .sum();
        let gaps = cache.gaps.as_ref().map_or(0, |gaps| {
            gaps[gaps.partition_point(|(at, _)| *at < segment.bytes.start)..]
                .iter()
                .take_while(|(at, _)| *at < col)
                .map(|(_, n)| n)
                .sum()
        });
        // Units are ordered and contiguous: those starting in the segment and ending by the
        // column form one run, summed through the prefix widths.
        let first = cache
            .bounds
            .partition_point(|(a, _)| *a < segment.bytes.start);
        let past = cache
            .bounds
            .partition_point(|(_, b)| *b <= col.min(segment.bytes.end));
        after_marker + inline + gaps + segment.indent + cache.widths[past.max(first)]
            - cache.widths[first]
    }
    pub fn offset(&self, text: &Text, visual: usize, x: f64) -> usize {
        let visual = visual.min(self.total().saturating_sub(1));
        let (row, n) = self.locate(visual);
        let row = row.min(text.len_lines().saturating_sub(1));
        let segment = self.segment(text, row, n);
        if self.options.width == 0 && text.line_len_bytes(row) >= crate::large::LONG_LINE {
            let x = if x.is_finite() { x.max(0.) } else { 0. };
            return text.line_to_byte(row)
                + self.long_query(text, row, x as usize, |i, a, c| {
                    a + i.column_to_byte(((x + 0.5) as usize).saturating_sub(c))
                });
        }
        let start = text.line_to_byte(row);
        let (line, base, source_column) = self.unit_window(text, row, start, &segment);
        let gaps = self.invisible_gaps(row, &line, base, source_column);
        let mut col = segment.indent as f64;
        let x = if x.is_finite() { x.max(0.) } else { 0. };
        for unit in units_in(
            &line,
            base,
            self.hidden(row),
            self.options.tab,
            source_column,
        )
        .iter()
        .filter(|u| u.bytes.start >= segment.bytes.start && u.bytes.end <= segment.bytes.end)
        {
            let gap = self.phantom_gap(start + unit.bytes.start) as f64;
            if x <= col + gap {
                return text.floor_char_boundary(
                    (text.line_to_byte(row) + unit.bytes.start).min(text.len_bytes()),
                );
            }
            col += gap;
            let extra = gaps
                .as_ref()
                .map_or(0, |g| Self::invisible_gap(g, unit.bytes.start));
            let width = (unit.width + extra) as f64;
            if x <= col + width / 2. {
                return text.floor_char_boundary(
                    (text.line_to_byte(row) + unit.bytes.start).min(text.len_bytes()),
                );
            }
            if extra != 0 && x < col + width {
                return (start + unit.bytes.end).min(text.len_bytes());
            }
            col += width;
        }
        if let Some(end) = self.folded_segment_end(text, row, visual, &segment) {
            return end;
        }
        let end = text.floor_char_boundary(start + segment.bytes.end.min(text.line_len_bytes(row)));
        if segment.bytes.end < body_len(text, row) {
            text.prev_grapheme_boundary(end)
        } else {
            end
        }
    }
    /// Caret endpoint of a marker ending this segment, shared by hit testing and
    /// visual EOL. A closing line on a separate row must not capture its header.
    pub(crate) fn folded_segment_end(
        &self,
        text: &Text,
        row: usize,
        visual: usize,
        segment: &Segment,
    ) -> Option<usize> {
        let row = row.min(text.len_lines().saturating_sub(1));
        let hidden = self
            .hidden(row)
            .iter()
            .find(|r| r.end == segment.bytes.end)?;
        let start = text.line_to_byte(row);
        let fold = self
            .folds
            .get(
                self.folds
                    .partition_point(|f| f.begin() < start + hidden.start),
            )
            .filter(|f| f.begin() == start + hidden.start)?;
        Some(if self.visual_at(text, fold.end()).0 == visual {
            text.floor_char_boundary(fold.end().min(text.len_bytes()))
        } else {
            text.floor_char_boundary((start + hidden.start).min(text.len_bytes()))
        })
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
            self.identity.unwrap_or((BufferId(0), ChangeId(0))),
            row.min(text.len_lines().saturating_sub(1)),
            self.options.tab,
            byte,
            f,
        )
    }
    pub fn wrapped(&self) -> bool {
        self.options.width > 0
    }
    pub(crate) fn long_wrapped(&self, row: usize) -> bool {
        self.wrapped()
            && self
                .lines
                .get(row)
                .is_some_and(|line| line.segments.last().is_some_and(|s| s.bytes.end > 8192))
    }
    pub(crate) fn segment_text(
        text: &Text,
        start: usize,
        segment: &Segment,
    ) -> (String, usize, usize) {
        let base = segment.bytes.start;
        (
            text.slice(start + base..(start + segment.bytes.end).min(text.len_bytes()))
                .to_string(),
            base,
            segment.source_column,
        )
    }
    pub(crate) fn unit_window(
        &self,
        text: &Text,
        row: usize,
        start: usize,
        segment: &Segment,
    ) -> (String, usize, usize) {
        if self.long_wrapped(row)
            || !self.folds.is_empty()
                && self.lines.get(row).is_some_and(|line| {
                    line.segments
                        .first()
                        .is_some_and(|first| first.bytes.start != 0)
                        || segment.bytes.start > 0
                            && line.hidden.iter().any(|r| r.start < segment.bytes.start)
                })
        {
            Self::segment_text(text, start, segment)
        } else {
            (text.line(row).to_string(), 0, 0)
        }
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
        let byte = text.floor_char_boundary(byte.min(text.len_bytes()));
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
        let a = a.min(text.len_lines().saturating_sub(1));
        let (b, _) = self.locate(first.saturating_add(count));
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
/// Revalidates geometry at each internal vi key, including edit/motion macro
/// sequences that execute entirely between two frames. Unchanged keys are O(1).
pub(crate) struct MotionLayout<'a> {
    pub layout: &'a std::cell::RefCell<Layout>,
    pub annotations: &'a std::cell::RefCell<crate::annotations::Annotations>,
    pub options: Options,
}
impl VerticalLayout for MotionLayout<'_> {
    fn synchronize(&self, buffer: &vel_buffer::Buffer, view: vel_buffer::ViewId) {
        let identity = (buffer.id(), buffer.change_id());
        if self.layout.borrow().current(identity, self.options) {
            return;
        }
        let folds = self.annotations.borrow_mut().folded(buffer, view);
        self.layout
            .borrow_mut()
            .configure(buffer.text(), identity, self.options, &folds);
    }
    fn x_of(&self, text: &Text, offset: usize) -> f64 {
        self.layout.borrow().x_of(text, offset)
    }
    fn offset_at(&self, text: &Text, line: usize, x: f64) -> usize {
        self.layout.borrow().offset_at(text, line, x)
    }
    fn lines_per_page(&self) -> usize {
        self.layout.borrow().lines_per_page()
    }
}
/// Dense caret edits get an index of borrowed chunks, never their contents.
/// A rope's generic slice/seek computes scalar summaries of patched leaf prefixes;
/// repeating those scans for every caret makes Unicode typing grow with the run.
/// Limit indexing to dense edits so sparse edits do not traverse a large document.
struct ByteReader<'a> {
    text: &'a Text,
    chunks: Vec<(usize, &'a str)>,
}
impl<'a> ByteReader<'a> {
    fn new(text: &'a Text, edits: usize) -> Self {
        let chunks = if edits >= 128 && text.len_bytes() <= edits.saturating_mul(4096) {
            let mut at = 0;
            text.chunks()
                .map(|chunk| {
                    let start = at;
                    at += chunk.len();
                    (start, chunk)
                })
                .collect()
        } else {
            Vec::new()
        };
        Self { text, chunks }
    }
    fn chunk_at_byte(&self, at: usize) -> (&'a str, usize) {
        if self.chunks.is_empty() {
            return self.text.chunk_at_byte(at);
        }
        let index = self
            .chunks
            .partition_point(|(start, _)| *start <= at)
            .saturating_sub(1);
        let (start, chunk) = self.chunks[index];
        (chunk, start)
    }
    fn is_char_boundary(&self, at: usize) -> bool {
        if at == self.text.len_bytes() {
            return true;
        }
        let (chunk, start) = self.chunk_at_byte(at);
        chunk.is_char_boundary(at - start)
    }
    fn byte_at(&self, at: usize) -> u8 {
        let (chunk, start) = self.chunk_at_byte(at);
        chunk.as_bytes()[at - start]
    }
    fn floor_char_boundary(&self, at: usize) -> usize {
        let (chunk, start) = self.chunk_at_byte(at);
        start + chunk.floor_char_boundary(at - start)
    }
    fn copy_bytes(&self, range: Range<usize>) -> String {
        let mut result = String::with_capacity(range.len());
        let mut at = range.start;
        while at < range.end {
            let (chunk, base) = self.chunk_at_byte(at);
            let end = range.end.min(base + chunk.len());
            result.push_str(&chunk[at - base..end - base]);
            at = end;
        }
        result
    }
}

// The full and incremental paths use the same whitespace/fallback rule.
// https://docs.sublimetext.io/reference/settings.html#automatic-behavior
// The reference excludes word wrapping from word_separators; no current source
// confirms changing today's CJK breaks, so preserve them.
fn wrap_end(
    units: &[Unit<'_>],
    from: usize,
    left: usize,
    width: usize,
    gap: impl Fn(usize) -> usize,
) -> (usize, usize) {
    let mut col = left;
    let mut end = from;
    let mut word = None;
    while end < units.len() {
        let cells = units[end].width + gap(units[end].bytes.start);
        if width > 0 && col + cells > width && end > from {
            break;
        }
        col += cells;
        end += 1;
        if units[end - 1].text.chars().all(char::is_whitespace) {
            word = Some(end);
        }
    }
    let lookahead = units[end.min(units.len().saturating_sub(1))].bytes.end;
    if end < units.len()
        && let Some(boundary) = word
    {
        end = boundary;
    }
    (end, lookahead)
}

/// One row's (or wrapped segment's) display units, for repeated [`Layout::column_in`]
/// queries.
#[derive(Debug)]
pub struct RowColumns {
    row: usize,
    /// Start of the measured window within the line, and its first source column.
    base: usize,
    source_column: usize,
    /// Window-relative byte range of each unit, in order.
    bounds: Vec<(usize, usize)>,
    /// Cells before each unit (and after the last).
    widths: Vec<usize>,
    gaps: Option<Arc<Vec<(usize, usize)>>>,
}
/// Graphemes are indivisible; folded ranges occupy one ellipsis cell.
pub fn units<'a>(line: &'a str, hidden: &[Range<usize>], tab: usize) -> Vec<Unit<'a>> {
    units_from(line, hidden, tab, 0)
}
/// Measures a visual row using its physical tab phase, returning line-relative bytes.
pub(crate) fn units_in<'a>(
    line: &'a str,
    base: usize,
    hidden: &[Range<usize>],
    tab: usize,
    column: usize,
) -> Vec<Unit<'a>> {
    if base == 0 {
        return units_from(line, hidden, tab, column);
    }
    let folds = hidden[hidden.partition_point(|r| r.start < base)..]
        .iter()
        .take_while(|r| r.start <= base + line.len())
        .map(|r| r.start - base..r.end - base)
        .collect::<Vec<_>>();
    let mut units = units_from(line, &folds, tab, column);
    for unit in &mut units {
        unit.bytes.start += base;
        unit.bytes.end += base;
    }
    units
}
fn units_from<'a>(
    line: &'a str,
    hidden: &[Range<usize>],
    tab: usize,
    mut column: usize,
) -> Vec<Unit<'a>> {
    let mut units = Vec::new();
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
            // A stale cache must not turn the next string slice into a split scalar.
            at = line.ceil_char_boundary(fold.end.min(line.len()));
            fold_index += 1;
            if fold.is_empty() { /* A fold starting at EOL still draws its marker below. */ }
            continue;
        }
        // ASCII followed by ASCII is already a complete grapheme (except CRLF).
        // Spaces/tabs take this path too; non-ASCII neighbors still get Unicode
        // segmentation so a following combining mark stays with its base.
        let byte = line.as_bytes()[at];
        let next = line.as_bytes().get(at + 1);
        let g = if byte.is_ascii()
            && next.is_none_or(u8::is_ascii)
            && !(byte == b'\r' && next == Some(&b'\n'))
        {
            &line[at..at + 1]
        } else {
            line[at..].graphemes(true).next().expect("grapheme")
        };
        if g == "\n" {
            break;
        }
        let width = if g == "\t" {
            tab - column % tab
        } else if g.len() == 1 && byte.is_ascii() {
            1
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
    let row = row.min(text.len_lines().saturating_sub(1));
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

#[cfg(test)]
mod round20_tests {
    use super::*;
    use vel_buffer::Change;
    fn random(seed: &mut u64, bound: usize) -> usize {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (*seed >> 32) as usize % bound
    }
    fn equal(actual: &Layout, model: &Layout, seed: u64, step: usize, text: &Text) {
        assert_eq!(
            actual.lines, model.lines,
            "seed {seed}, step {step}, options {:?}, text {text}",
            actual.options
        );
        assert_eq!(
            actual
                .wrap
                .iter()
                .map(|cache| &cache.dependencies)
                .collect::<Vec<_>>(),
            model
                .wrap
                .iter()
                .map(|cache| &cache.dependencies)
                .collect::<Vec<_>>(),
            "dependency cache seed {seed}, step {step}, text {text}"
        );
        assert_eq!(actual.total(), model.total());
        assert!(
            actual
                .wrap
                .iter()
                .map(|cache| &cache.invisible)
                .eq(model.wrap.iter().map(|cache| &cache.invisible)),
            "invisible gaps seed {seed}, step {step}, text {text}"
        );
        if !actual.byte_rows.tree.is_empty() {
            for row in 0..text.len_lines() {
                assert_eq!(
                    actual.byte_rows.before(row),
                    text.line_to_byte(row),
                    "cached physical row {row}"
                );
            }
        }
        for visual in 0..actual.total() {
            assert_eq!(actual.locate(visual), model.locate(visual));
        }
    }
    #[test]
    fn dense_unicode_caret_edits_match_full_layout_and_chunk_reads() {
        for width in [1, 2, 7, 13, 80] {
            for indent in [false, true] {
                let mut source = Text::from("  let x = 1; 你好 e\u{301}\tend\n".repeat(160));
                let id = BufferId(922);
                let options = Options {
                    width,
                    tab: 4,
                    indent,
                };
                let mut actual = Layout::default();
                actual.configure(&source, (id, ChangeId(0)), options, &[]);
                let mut rng = 222;
                for step in 1..=40 {
                    let mut edits = Vec::new();
                    for row in 0..160 {
                        let line = source.line(row).to_string();
                        let positions: Vec<_> = line
                            .char_indices()
                            .map(|(at, _)| at)
                            .chain([line.len()])
                            .collect();
                        let from = random(&mut rng, positions.len());
                        let to = (from + random(&mut rng, 2)).min(positions.len() - 1);
                        let start = source.line_to_byte(row);
                        let text = [
                            "let x = 1; ",
                            " ",
                            "\t",
                            "界",
                            "e\u{301}",
                            "\u{301}",
                            "",
                            "\n",
                        ][random(&mut rng, 8)];
                        edits.push(vel_text::Edit {
                            range: start + positions[from]..start + positions[to],
                            text,
                        });
                    }
                    let before = source.clone();
                    source.apply_edits(&edits);
                    let changes = ChangeSet::new(
                        edits
                            .iter()
                            .map(|e| Change {
                                old_range: e.range.clone(),
                                new_len: e.text.len(),
                            })
                            .collect(),
                    );
                    actual.change(&before, &source, (id, ChangeId(step)), &changes);
                    let mut model = Layout::default();
                    model.configure(&source, (id, ChangeId(step)), options, &[]);
                    equal(&actual, &model, 222, step as usize, &source);
                    let reader = ByteReader::new(&source, edits.len());
                    assert!(!reader.chunks.is_empty());
                    for row in [0, 79, 159] {
                        let start = source.line_to_byte(row);
                        let end = start + body_len(&source, row);
                        assert_eq!(reader.copy_bytes(start..end), source.line(row).to_string());
                    }
                }
            }
        }
    }
    #[test]
    fn incremental_unicode_wrap_matches_full_layout_over_2048_seeds() {
        wrap_edit_sweep(false, 2048);
    }
    #[test]
    fn incremental_invisible_wrap_matches_full_layout() {
        wrap_edit_sweep(true, 256);
    }
    fn wrap_edit_sweep(invisible: bool, seeds: u64) {
        const TOKENS: &[&str] = &[
            "a",
            "let x = 1; ",
            " ",
            "\t",
            "你好",
            "界",
            "e\u{301}",
            "\u{301}",
            "👩‍💻",
            "🏽",
            "🇨🇳",
            "\n",
            "",
            "  \t  ",
            "\u{200b}",
            "\u{a0}",
            "\u{202e}",
            "\u{1}",
            "\u{a0}\u{301}",
        ];
        let id = BufferId(920);
        for seed in (0..seeds).chain([1000493, 23028]) {
            for width in [0, 1, 2, 7, 13, 80] {
                for indent in [false, true] {
                    let mut rng = seed + 1;
                    let mut source = Text::from("  let x = 1; 你好 e\u{301}\tmore words\n\t世界\n");
                    let mut folds = if width == 0 {
                        vec![Region::new(5, source.line_to_byte(1) + 2)]
                    } else {
                        Vec::new()
                    };
                    let options = Options {
                        width,
                        tab: 1 + random(&mut rng, 8),
                        indent,
                    };
                    let mut actual = Layout::default();
                    let marks = if invisible {
                        Invisible {
                            whitespace: vel_render::invisibles::WhiteSpace::All,
                            bidi: true,
                            control: true,
                        }
                    } else {
                        Invisible::default()
                    };
                    let tokens = if invisible {
                        TOKENS
                    } else {
                        &TOKENS[..TOKENS.len() - 5]
                    };
                    actual.set_invisibles(marks);
                    actual.configure(&source, (id, ChangeId(0)), options, &folds);
                    for step in 1..=24 {
                        let content = source.to_string();
                        let boundaries: Vec<_> = content
                            .char_indices()
                            .map(|(at, _)| at)
                            .chain([content.len()])
                            .collect();
                        let count = 1 + random(&mut rng, 4);
                        let mut edits = Vec::new();
                        for _ in 0..count {
                            let from = random(&mut rng, boundaries.len());
                            let to = if random(&mut rng, 3) == 0 {
                                (from + random(&mut rng, 5)).min(boundaries.len() - 1)
                            } else {
                                from
                            };
                            let range = boundaries[from]..boundaries[to];
                            if edits.iter().any(|(r, _): &(Range<usize>, &str)| {
                                r.start <= range.end && range.start <= r.end
                            }) {
                                continue;
                            }
                            edits.push((range, tokens[random(&mut rng, tokens.len())]));
                        }
                        edits.sort_by_key(|(r, _)| r.start);
                        let before = source.clone();
                        for (range, text) in edits.iter().rev() {
                            source.replace(range.clone(), text);
                        }
                        let changes = ChangeSet::new(
                            edits
                                .iter()
                                .map(|(range, text)| Change {
                                    old_range: range.clone(),
                                    new_len: text.len(),
                                })
                                .collect(),
                        );
                        actual.change(&before, &source, (id, ChangeId(step as u64)), &changes);
                        folds = folds
                            .into_iter()
                            .map(|r| changes.transform_region(r))
                            .filter(|r| !r.empty())
                            .collect();
                        let mut model = Layout::default();
                        model.set_invisibles(marks);
                        model.configure(&source, (id, ChangeId(step as u64)), options, &folds);
                        if actual.lines != model.lines {
                            eprintln!("before: {before}\nedits: {edits:?}");
                        }
                        equal(&actual, &model, seed, step, &source);
                    }
                }
            }
        }
    }
    #[test]
    fn unwrapped_fold_ends_on_grapheme_boundary_and_measures_visible_columns() {
        let mut source = Text::from("fn alpha() {\n    hello();\n    world();\ne\u{301}\tend\n");
        let before = source.clone();
        let options = Options {
            width: 0,
            tab: 4,
            indent: false,
        };
        let id = BufferId(923);
        let fold = Region::new(12, source.line_to_byte(3) + 1);
        let mut changed = Layout::default();
        changed.configure(&source, (id, ChangeId(0)), options, &[fold]);
        source.insert(3, "Z");
        let changes = ChangeSet::new(vec![Change {
            old_range: 3..3,
            new_len: 1,
        }]);
        changed.change(&before, &source, (id, ChangeId(1)), &changes);
        let mut fresh = Layout::default();
        fresh.configure(
            &source,
            (id, ChangeId(1)),
            options,
            &[changes.transform_region(fold)],
        );
        equal(&changed, &fresh, 923, 1, &source);
        assert_eq!(changed.segment(&source, 0, 0).columns, 14);
        assert_eq!(changed.segment(&source, 3, 0).bytes.start, "e\u{301}".len());
        assert_eq!(changed.segment(&source, 3, 0).columns, 7);
    }
    #[test]
    fn incremental_wrap_folds_and_inline_phantoms_use_full_geometry() {
        for width in [0, 1, 2, 7, 13, 80] {
            for indent in [false, true] {
                let mut source = Text::from("  let x = 1; 你好\nfold me\n\tend e\u{301}\n");
                let id = BufferId(921);
                let options = Options {
                    width,
                    tab: 4,
                    indent,
                };
                let mut folds = vec![Region::new(4, source.line_to_byte(2) + 1)];
                let mut actual = Layout::default();
                actual.configure(&source, (id, ChangeId(0)), options, &folds);
                actual.set_phantom_inline(&source, vec![(2, 3)]);
                for step in 1..=20 {
                    let before = source.clone();
                    let at = if step % 2 == 0 {
                        1
                    } else {
                        source.len_bytes() - 1
                    };
                    source.insert(at, "界 ");
                    let changes = ChangeSet::new(vec![Change {
                        old_range: at..at,
                        new_len: "界 ".len(),
                    }]);
                    folds = folds
                        .into_iter()
                        .map(|r| changes.transform_region(r))
                        .collect();
                    actual.change(&before, &source, (id, ChangeId(step)), &changes);
                    let mut model = Layout::default();
                    model.configure(&source, (id, ChangeId(step)), options, &folds);
                    model.set_phantom_inline(&source, vec![(2, 3)]);
                    equal(&actual, &model, 0, step as usize, &source);
                }
            }
        }
    }
}

#[cfg(test)]
mod review20_tests {
    use super::*;
    use vel_buffer::Change;
    #[test]
    fn nonadjacent_grapheme_lookahead_matches_full_layout() {
        let old = Text::from("  \u{308}abcdefghijklmnopqrstuvwxyz");
        let mut after = old.clone();
        after.insert(16, "🏽");
        let options = Options {
            width: 13,
            tab: 4,
            indent: true,
        };
        let id = BufferId(201);
        let mut layout = Layout::default();
        layout.configure(&old, (id, ChangeId(0)), options, &[]);
        layout.change(
            &old,
            &after,
            (id, ChangeId(1)),
            &ChangeSet::new(vec![Change {
                old_range: 16..16,
                new_len: 4,
            }]),
        );
        let mut model = Layout::default();
        model.configure(&after, (id, ChangeId(1)), options, &[]);
        assert_eq!(layout.lines, model.lines);
        assert_eq!(layout.wrap[0].dependencies[0].end, 20);
        assert_eq!(layout.wrap[0].dependencies, model.wrap[0].dependencies);
    }
    #[test]
    fn equal_start_insert_and_replace_map_uses_full_geometry() {
        for width in [0, 1, 2, 7, 13, 80] {
            let old = Text::from("abc\n  hello world\n");
            let after = Text::from("xQbc\n  hello world\n");
            let options = Options {
                width,
                tab: 4,
                indent: true,
            };
            let id = BufferId(202);
            let mut layout = Layout::default();
            layout.configure(&old, (id, ChangeId(0)), options, &[]);
            layout.change(
                &old,
                &after,
                (id, ChangeId(1)),
                &ChangeSet::new(vec![
                    Change {
                        old_range: 0..0,
                        new_len: 1,
                    },
                    Change {
                        old_range: 0..1,
                        new_len: 1,
                    },
                ]),
            );
            let mut model = Layout::default();
            model.configure(&after, (id, ChangeId(1)), options, &[]);
            assert_eq!(layout.lines, model.lines);
            assert_eq!(layout.total(), model.total());
        }
    }
    #[test]
    fn ordinary_line_metadata_remains_forty_bytes() {
        assert_eq!(std::mem::size_of::<Line>(), 40);
        assert_eq!(std::mem::size_of::<Segment>(), 40);
    }
}

#[cfg(test)]
#[test]
fn oversized_grapheme_lookahead_seed_570() {
    let old = Text::from("re界a \tr👩‍💻🏽🇨🇳世");
    let mut after = old.clone();
    after.insert(24, "🏽");
    let options = Options {
        width: 1,
        tab: 1,
        indent: false,
    };
    let id = BufferId(570);
    let mut actual = Layout::default();
    actual.configure(&old, (id, ChangeId(0)), options, &[]);
    actual.change(
        &old,
        &after,
        (id, ChangeId(1)),
        &ChangeSet::new(vec![vel_buffer::Change {
            old_range: 24..24,
            new_len: 4,
        }]),
    );
    let mut model = Layout::default();
    model.configure(&after, (id, ChangeId(1)), options, &[]);
    assert_eq!(actual.lines, model.lines);
    assert_eq!(actual.wrap[0].dependencies, model.wrap[0].dependencies);
}
