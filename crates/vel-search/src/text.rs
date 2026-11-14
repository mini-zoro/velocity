//! Rope regex search and atomic buffer replacements, without copying the document.
use regex_cursor::{Input, engines::meta::Regex};
use std::{
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
};
use vel_buffer::{Buffer, Edit, EditKind, Region, Selection, ViewId};
use vel_text::{ChunkCursor, Text};

/// Find/replace options shared by the panel and file search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Interpret regex syntax rather than literal text.
    pub regex: bool,
    /// Match case exactly.
    pub case_sensitive: bool,
    /// Require Unicode word boundaries.
    pub whole_word: bool,
    /// Continue from the opposite end at EOF/BOF.
    pub wrap: bool,
    /// Adapt replacement casing to each matched spelling.
    pub preserve_case: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            regex: false,
            case_sensitive: false,
            whole_word: false,
            wrap: true,
            preserve_case: false,
        }
    }
}
/// Compiled query. Supports regex-crate syntax: no lookaround, backreferences or recursion.
#[derive(Clone, Debug)]
pub struct Search {
    /// Original panel query.
    pub query: String,
    /// Effective panel flags.
    pub options: Options,
    re: Regex,
}
impl Search {
    /// Compiles once per query. Empty input clears search instead of matching every boundary.
    pub fn new(query: &str, options: Options) -> Result<Self, String> {
        let source = if options.regex {
            query.into()
        } else {
            regex::escape(query)
        };
        let source = if options.whole_word {
            format!("\\b(?:{source})\\b")
        } else {
            source
        };
        let re = Regex::new(&format!(
            "(?m){}(?:{source})",
            if options.case_sensitive { "" } else { "(?i)" }
        ))
        .map_err(|e| e.to_string())?;
        Ok(Self {
            query: query.into(),
            options,
            re,
        })
    }
    /// Returns matches within an optional set of selection/viewport ranges, retaining rope context.
    pub fn find(&self, text: &Text, ranges: &[Range<usize>]) -> Vec<Range<usize>> {
        self.find_cancel(text, ranges, || false).unwrap_or_default()
    }
    fn find_cancel(
        &self,
        text: &Text,
        ranges: &[Range<usize>],
        cancelled: impl Fn() -> bool,
    ) -> Option<Vec<Range<usize>>> {
        if self.query.is_empty() {
            return Some(Vec::new());
        }
        let all = 0..text.len_bytes();
        let ranges = if ranges.is_empty() {
            std::slice::from_ref(&all)
        } else {
            ranges
        };
        let mut matches = Vec::new();
        for range in ranges {
            let input = Input::new(ChunkCursor::from(text)).range(range.clone());
            for found in self.re.find_iter(input) {
                if cancelled() {
                    return None;
                }
                matches.push(found.start()..found.end());
            }
            if cancelled() {
                return None;
            }
        }
        matches.sort_by_key(|r| (r.start, r.end));
        matches.dedup();
        Some(matches)
    }
    /// Chooses the next/previous match. `current` is skipped, including a zero-width match.
    pub fn next(
        &self,
        matches: &[Range<usize>],
        caret: usize,
        current: Option<&Range<usize>>,
        backwards: bool,
    ) -> Option<Range<usize>> {
        let valid = |r: &&Range<usize>| current != Some(*r);
        if backwards {
            matches
                .iter()
                .rev()
                .filter(valid)
                .find(|r| r.end <= caret)
                .or_else(|| {
                    self.options
                        .wrap
                        .then(|| matches.iter().rev().find(valid))
                        .flatten()
                })
                .cloned()
        } else {
            matches
                .iter()
                .filter(valid)
                .find(|r| r.start >= caret)
                .or_else(|| {
                    self.options
                        .wrap
                        .then(|| matches.iter().find(valid))
                        .flatten()
                })
                .cloned()
        }
        .or_else(|| self.options.wrap.then(|| current.cloned()).flatten())
    }
    /// Expands numbered/named `$group`/`${group}` and `$$`, copying only captured substrings.
    pub fn replacements(
        &self,
        text: &Text,
        template: &str,
        ranges: &[Range<usize>],
    ) -> Vec<Replacement> {
        if self.query.is_empty() {
            return Vec::new();
        }
        let all = 0..text.len_bytes();
        let ranges = if ranges.is_empty() {
            std::slice::from_ref(&all)
        } else {
            ranges
        };
        let mut result = Vec::new();
        for range in ranges {
            for capture in self
                .re
                .captures_iter(Input::new(ChunkCursor::from(text)).range(range.clone()))
            {
                let found = capture.get_match().expect("matched capture");
                let range = found.start()..found.end();
                let mut replacement = String::new();
                if self.options.regex {
                    let mut chars = template.chars().peekable();
                    while let Some(c) = chars.next() {
                        if c != '$' {
                            replacement.push(c);
                            continue;
                        }
                        if chars.peek() == Some(&'$') {
                            chars.next();
                            replacement.push('$');
                            continue;
                        }
                        let braced = chars.peek() == Some(&'{');
                        if braced {
                            chars.next();
                        }
                        let mut name = String::new();
                        while chars
                            .peek()
                            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
                        {
                            name.push(chars.next().expect("peeked"));
                        }
                        if braced && chars.peek() == Some(&'}') {
                            chars.next();
                        }
                        if name.is_empty() {
                            replacement.push('$');
                            continue;
                        }
                        let group = name
                            .parse::<usize>()
                            .ok()
                            .and_then(|n| capture.get_group(n))
                            .or_else(|| capture.get_group_by_name(&name));
                        if let Some(group) = group {
                            replacement.push_str(&text.slice(group.start..group.end).to_string());
                        }
                    }
                } else {
                    replacement.push_str(template);
                }
                if self.options.preserve_case {
                    replacement = preserve(&text.slice(range.clone()).to_string(), &replacement);
                }
                result.push(Replacement {
                    range,
                    text: replacement,
                });
            }
        }
        result.sort_by_key(|r| (r.range.start, r.range.end));
        result.dedup_by(|a, b| a.range == b.range);
        result
    }
}
/// A prepared replacement in the original snapshot's byte coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replacement {
    /// Original match.
    pub range: Range<usize>,
    /// Expanded replacement.
    pub text: String,
}
/// Commits all replacements as one undoable transaction, mapping every caret.
pub fn apply(buffer: &mut Buffer, view: ViewId, replacements: &[Replacement]) {
    if replacements.is_empty() {
        return;
    }
    let changes = vel_buffer::ChangeSet::new(
        replacements
            .iter()
            .map(|r| vel_buffer::Change {
                old_range: r.range.clone(),
                new_len: r.text.len(),
            })
            .collect(),
    );
    let selection = Selection::from_regions(
        buffer
            .view(view)
            .sel()
            .iter()
            .map(|r| changes.transform_region(r)),
    );
    let edits: Vec<_> = replacements
        .iter()
        .map(|r| Edit {
            range: r.range.clone(),
            text: r.text.as_str(),
        })
        .collect();
    buffer.transact(view, EditKind::Other, &edits, selection);
}
fn preserve(source: &str, replacement: &str) -> String {
    let letters: Vec<_> = source.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.is_empty() {
        return replacement.into();
    }
    if letters.iter().all(|c| c.is_uppercase()) {
        return replacement.to_uppercase();
    }
    if letters.iter().all(|c| c.is_lowercase()) {
        return replacement.to_lowercase();
    }
    if letters[0].is_uppercase() && letters[1..].iter().all(|c| c.is_lowercase()) {
        let lower = replacement.to_lowercase();
        let mut chars = lower.chars();
        return chars.next().map_or_else(String::new, |first| {
            first.to_uppercase().chain(chars).collect()
        });
    }
    replacement.into()
}
/// Snapshot + query tagged asynchronous match result.
#[derive(Debug)]
pub struct Matches {
    /// Cancellation generation.
    pub generation: u64,
    /// Buffer revision supplied by caller.
    pub revision: u64,
    /// Sorted UTF-8 matches.
    pub ranges: Vec<Range<usize>>,
}
#[derive(Debug)]
enum Work {
    Search(u64, Search, Text, u64, Vec<Range<usize>>),
    Stop,
}
/// Background all-document highlighting on one reusable worker. Queued stale queries are skipped.
#[derive(Debug)]
pub struct Worker {
    generation: Arc<AtomicU64>,
    tx: mpsc::Sender<Work>,
    rx: mpsc::Receiver<Matches>,
}
impl Default for Worker {
    fn default() -> Self {
        let generation = Arc::new(AtomicU64::new(0));
        let current = generation.clone();
        let (tx, requests) = mpsc::channel();
        let (results, rx) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(work) = requests.recv() {
                match work {
                    Work::Stop => break,
                    Work::Search(id, search, text, revision, ranges) => {
                        if current.load(Ordering::Acquire) != id {
                            continue;
                        }
                        if let Some(ranges) = search
                            .find_cancel(&text, &ranges, || current.load(Ordering::Relaxed) != id)
                            && current.load(Ordering::Acquire) == id
                        {
                            let _ = results.send(Matches {
                                generation: id,
                                revision,
                                ranges,
                            });
                        }
                    }
                }
            }
        });
        Self { generation, tx, rx }
    }
}
impl Worker {
    /// Searches an O(1) rope snapshot off the UI thread. At most one query is scored at a time.
    pub fn search(&mut self, search: Search, text: Text, revision: u64, ranges: Vec<Range<usize>>) {
        let id = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let _ = self
            .tx
            .send(Work::Search(id, search, text, revision, ranges));
    }
    /// Invalidates pending work when input closes or buffer identity changes.
    pub fn cancel(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
    /// Returns only the current generation.
    pub fn poll(&self) -> Option<Matches> {
        self.rx
            .try_iter()
            .filter(|r| r.generation == self.generation.load(Ordering::Acquire))
            .last()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
        let _ = self.tx.send(Work::Stop);
    }
}

/// Converts sorted matches to a multi-selection.
pub fn select_all(buffer: &mut Buffer, view: ViewId, matches: &[Range<usize>]) {
    if !matches.is_empty() {
        buffer.set_selection(
            view,
            Selection::from_regions(matches.iter().cloned().map(Region::from)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn regex_groups_case_selection_wrap_and_undo() {
        let mut buffer = Buffer::new("cat CAT Cat cAt cats\n你好42");
        let view = buffer.new_view();
        let search = Search::new(
            "cat",
            Options {
                whole_word: true,
                preserve_case: true,
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(search.find(buffer.text(), &[]), [0..3, 4..7, 8..11, 12..15]);
        let replace = search.replacements(buffer.text(), "dog", std::slice::from_ref(&(0..11)));
        apply(&mut buffer, view, &replace);
        assert_eq!(buffer.text().to_string(), "dog DOG Dog cAt cats\n你好42");
        assert!(buffer.undo(view));
        let re = Search::new(
            r"(?P<word>你好)(\d+)",
            Options {
                regex: true,
                ..Options::default()
            },
        )
        .unwrap();
        let edits = re.replacements(buffer.text(), "${2}:$word:$$", &[]);
        apply(&mut buffer, view, &edits);
        assert!(buffer.text().to_string().ends_with("42:你好:$"));
        let matches = search.find(buffer.text(), &[]);
        assert_eq!(search.next(&matches, 100, None, false), Some(0..3));
        assert_eq!(search.next(&matches, 4, Some(&(4..7)), true), Some(0..3));
        assert!(
            Search::new(
                "(?<=x)y",
                Options {
                    regex: true,
                    ..Options::default()
                }
            )
            .is_err()
        );
    }
    #[test]
    fn rope_seams_unicode_empty_and_zero_width() {
        let text = Text::from(format!("{}needle你好\nend", "x".repeat(4093)).as_str());
        assert_eq!(
            Search::new("needle你好", Options::default())
                .unwrap()
                .find(&text, &[]),
            std::slice::from_ref(&(4093..4105))
        );
        assert!(
            Search::new("", Options::default())
                .unwrap()
                .find(&text, &[])
                .is_empty()
        );
        let re = Search::new(
            "^",
            Options {
                regex: true,
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(re.find(&Text::from("a\nb"), &[]), [0..0, 2..2]);
    }
}
