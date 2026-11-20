//! Text projection of the ordered worker results. Display text is editable;
//! jump targets are attached to mapped byte regions, not parsed from that text.
use super::*;
use crate::{editor::Editor, workspace::Tab};
use vel_buffer::{ChangeId, ChangeSet, EditKind, Region};

pub(crate) const SYNTAX: &str = "Packages/Default/Find Results.sublime-syntax";
pub(crate) const MATCH_SCOPE: &str = "markup.inserted.match.find-in-files";

#[derive(Clone, Debug)]
pub(super) struct Target {
    pub path: Arc<Path>,
    pub sheet: Option<u64>,
    pub line: usize,
    pub column: usize,
}

#[derive(Debug)]
struct Marker {
    row: Region,
    target: Option<Target>,
    matches: Vec<Region>,
}

#[derive(Debug)]
pub(super) struct ResultsTab {
    pub sheet: u64,
    version: ChangeId,
    rows: Vec<Marker>,
    // The latest search's rows are a suffix of this vector. Older rows retain
    // their targets even though F4 now walks the new search's model.
    start: usize,
    heading: Option<(Region, String)>,
}

#[derive(Default)]
pub(super) struct Batch {
    rows: Vec<(usize, String, Marker)>,
    bytes: usize,
}

impl Batch {
    pub(super) fn full(&self) -> bool {
        self.bytes >= 128 * 1024
    }

    pub(super) fn insert(
        &mut self,
        view: &files::Results,
        at: usize,
        count: usize,
        untitled: &BTreeMap<PathBuf, u64>,
    ) {
        for (rank, _, _) in &mut self.rows {
            if *rank >= at {
                *rank += count;
            }
        }
        for rank in at..at + count {
            let (text, target, matches) = match view.rows.get(rank).expect("inserted result") {
                files::Row::Header(path) => (
                    format!("\n{}:\n", path.to_string_lossy().escape_debug()),
                    None,
                    Vec::new(),
                ),
                files::Row::Gap => ("        …\n".into(), None, Vec::new()),
                files::Row::Line(line) => {
                    let text = line.formatted();
                    let prefix = text.len() - line.text.len() - 1;
                    let matches = line
                        .highlights
                        .iter()
                        .filter(|range| !range.is_empty())
                        .map(|range| Region::new(prefix + range.start, prefix + range.end))
                        .collect();
                    (text, Some(target(line, untitled)), matches)
                }
            };
            let len = text.len();
            self.bytes += len;
            self.rows.push((
                rank,
                text,
                Marker {
                    row: Region::new(0, len),
                    target,
                    matches,
                },
            ));
        }
    }
}

pub(super) fn target(line: &files::Line, untitled: &BTreeMap<PathBuf, u64>) -> Target {
    Target {
        path: line.path.clone(),
        sheet: untitled.get(line.path.as_ref()).copied(),
        line: line.number,
        column: line.column(),
    }
}

impl ResultsTab {
    pub(super) fn new(tab: &Tab) -> Self {
        Self {
            sheet: tab.sheet,
            version: tab.editor.buffer.change_id(),
            rows: Vec::new(),
            start: 0,
            heading: None,
        }
    }

    fn map(&mut self, changes: &ChangeSet) {
        if let Some((region, _)) = &mut self.heading {
            *region = changes.transform_region(*region);
        }
        let Some(first) = changes.changes().first() else {
            return;
        };
        let first = self
            .rows
            .partition_point(|r| r.row.end() < first.old_range.start);
        let mut end = 0;
        for marker in &mut self.rows[first..] {
            let row = changes.transform_region(marker.row);
            // A deletion/replacement spanning rows can map swallowed regions
            // to opposite sides. Keep their rank, but don't jump from them.
            marker.row = if row.begin() < end {
                Region::caret(end)
            } else {
                row
            };
            end = marker.row.end();
            for hit in &mut marker.matches {
                *hit = changes.transform_region(*hit);
            }
            marker.matches.retain(|hit| {
                !hit.empty() && hit.begin() >= marker.row.begin() && hit.end() <= end
            });
        }
    }

    fn sync(&mut self, editor: &Editor) {
        if self.version == editor.buffer.change_id() {
            return;
        }
        if let Some(changes) = editor.buffer.changes_since(self.version) {
            for (_, map) in &changes {
                self.map(map);
            }
        } else {
            // Do not guess by parsing user-edited text after the edit log expired.
            let end = Region::caret(editor.text.len_bytes());
            for marker in &mut self.rows {
                marker.row = end;
                marker.matches.clear();
            }
            self.heading = None;
        }
        self.version = editor.buffer.change_id();
    }

    fn edit(&mut self, editor: &mut Editor, edits: Vec<(Range<usize>, String)>) {
        editor.unmark();
        editor.apply_edits(edits, None, EditKind::Other);
        editor.sync();
        self.sync(editor);
    }

    pub(super) fn begin(&mut self, tab: &mut Tab, heading: String) {
        self.sync(&tab.editor);
        self.start = self.rows.len();
        let end = tab.editor.text.len_bytes();
        let separator = if end == 0 { "" } else { "\n\n" };
        let at = end + separator.len();
        self.edit(
            &mut tab.editor,
            vec![(end..end, format!("{separator}{heading}\n"))],
        );
        self.heading = Some((Region::new(at, at + heading.len()), heading));
        tab.editor.set_caret(at, false);
        tab.pending_center = true;
    }

    pub(super) fn write(&mut self, editor: &mut Editor, batch: Batch) {
        let mut fresh = batch.rows;
        if fresh.is_empty() {
            return;
        }
        self.sync(editor);
        fresh.sort_unstable_by_key(|(rank, ..)| *rank);
        let end = editor.text.len_bytes();
        let mut edits: Vec<(Range<usize>, String)> = Vec::new();
        let mut added = 0;
        for (i, (rank, text, marker)) in fresh.iter_mut().enumerate() {
            let at = self
                .rows
                .get(self.start + *rank - i)
                .map_or(end, |r| r.row.begin());
            if edits.last().is_none_or(|(range, _)| range.start != at) {
                let separator = if at > 0 && editor.text.byte_at(at - 1) != b'\n' {
                    "\n"
                } else {
                    ""
                };
                edits.push((at..at, separator.into()));
                added += separator.len();
            }
            let base = at + added;
            let offset = |r: Region| Region::new(base + r.a, base + r.b);
            marker.row = offset(marker.row);
            marker.matches.iter_mut().for_each(|r| *r = offset(*r));
            edits.last_mut().expect("result edit").1.push_str(text);
            added += text.len();
        }
        self.edit(editor, edits);

        // Reassemble just the insertion suffix in one pass. In the common
        // one-file stream this doesn't walk or copy any previous results.
        let first = self.start + fresh[0].0;
        if first == self.rows.len() {
            self.rows
                .extend(fresh.into_iter().map(|(_, _, marker)| marker));
        } else {
            let mut old = self.rows.split_off(first).into_iter();
            for (rank, _, marker) in fresh {
                let count = self.start + rank - self.rows.len();
                self.rows.extend(old.by_ref().take(count));
                self.rows.push(marker);
            }
            self.rows.extend(old);
        }
    }

    pub(super) fn finish(&mut self, editor: &mut Editor, heading: String, footer: &str) {
        self.sync(editor);
        let mut edits = Vec::new();
        if let Some((region, original)) = self.heading.take()
            && !region.empty()
            && editor.text.slice(region.to_range()) == original.as_str()
            && original != heading
        {
            // Normally only the file count changed. Keep the caret and any
            // selection on the rest of the heading in their original text.
            let shared = |old: &str, new: &str, reverse: bool| {
                let old = old.chars();
                let new = new.chars();
                if reverse {
                    old.rev()
                        .zip(new.rev())
                        .take_while(|(a, b)| a == b)
                        .map(|(c, _)| c.len_utf8())
                        .sum::<usize>()
                } else {
                    old.zip(new)
                        .take_while(|(a, b)| a == b)
                        .map(|(c, _)| c.len_utf8())
                        .sum::<usize>()
                }
            };
            let prefix = shared(&original, &heading, false);
            let suffix = shared(&original[prefix..], &heading[prefix..], true);
            edits.push((
                region.begin() + prefix..region.end() - suffix,
                heading[prefix..heading.len() - suffix].to_owned(),
            ));
        }
        let end = editor.text.len_bytes();
        edits.push((end..end, format!("\n{footer}\n")));
        self.edit(editor, edits);
    }

    pub(super) fn report(&mut self, editor: &mut Editor, report: &str) {
        let end = editor.text.len_bytes();
        self.edit(editor, vec![(end..end, format!("\n{report}\n"))]);
    }

    pub(super) fn at(&mut self, editor: &Editor, point: usize) -> Option<(Option<usize>, Target)> {
        self.sync(editor);
        let line = editor.text.byte_to_line(point);
        let start = editor.text.line_to_byte(line);
        let end = start + editor.text.line_len_bytes(line);
        let i = self.rows.partition_point(|r| r.row.end() <= start);
        // An edit can widen a region to more than one physical line; only its
        // tracked first line is a result. For merged lines prefer the hit under
        // the caret, otherwise use the first surviving result on that line.
        self.rows[i..]
            .iter()
            .enumerate()
            .take_while(|(_, marker)| marker.row.begin() < end)
            .filter(|(_, marker)| !marker.row.empty() && marker.row.begin() >= start)
            .filter_map(|(offset, marker)| Some((offset, marker.row, marker.target.as_ref()?)))
            .min_by_key(|(_, row, _)| !row.to_range().contains(&point))
            .map(|(offset, _, target)| ((i + offset).checked_sub(self.start), target.clone()))
    }

    pub(super) fn matches(&mut self, editor: &Editor, bytes: Range<usize>) -> Vec<Range<usize>> {
        self.sync(editor);
        let first = self.rows.partition_point(|r| r.row.end() <= bytes.start);
        self.rows[first..]
            .iter()
            .take_while(|r| r.row.begin() < bytes.end)
            .flat_map(|marker| &marker.matches)
            .map(|hit| hit.to_range())
            .collect()
    }
}
