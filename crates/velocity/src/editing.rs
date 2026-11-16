//! Language-aware commands over exact, old-coordinate buffer transactions.
use crate::{
    editor::{Editor, cells_with_tab},
    layout::FOLDS,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    sync::mpsc,
};
use vel_buffer::{Bias, Change, ChangeSet, Edit, EditKind, Region, Selection, VerticalLayout};
use vel_config::Value;

impl Editor {
    pub fn apply_edits(
        &mut self,
        mut owned: Vec<(Range<usize>, String)>,
        mut after: Option<Selection>,
        kind: EditKind,
    ) {
        if self.read_only || owned.is_empty() {
            return;
        }
        owned.sort_by_key(|(r, _)| (r.start, r.end));
        owned.dedup_by(|a, b| a.0 == b.0);
        if let Some(mut fields) = self.fields.take() {
            let input: Vec<_> = owned
                .iter()
                .map(|(range, text)| vel_snippet::SnippetEdit {
                    range: range.clone(),
                    text: text.clone(),
                })
                .collect();
            let old_fields = fields.selections();
            let input_changes = ChangeSet::new(
                input
                    .iter()
                    .map(|e| Change {
                        old_range: e.range.clone(),
                        new_len: e.text.len(),
                    })
                    .collect(),
            );
            match fields.edit_fields(self.buffer.text(), &input) {
                Ok(edits) => {
                    let new_fields = fields.selections();
                    let requested = after.as_ref();
                    after = Some(Selection::from_regions(
                        self.buffer
                            .view(self.id)
                            .sel()
                            .iter()
                            .enumerate()
                            .filter_map(|(selection_index, r)| {
                                let i = old_fields
                                    .iter()
                                    .position(|f| f.start <= r.begin() && r.end() <= f.end)?;
                                let start =
                                    input_changes.transform_point(old_fields[i].start, Bias::Left);
                                let desired = requested
                                    .and_then(|selection| selection.iter().nth(selection_index))
                                    .unwrap_or_else(|| {
                                        Region::caret(
                                            input_changes.transform_point(r.end(), Bias::Right),
                                        )
                                    });
                                let map = |point: usize| {
                                    (new_fields[i].start + point.saturating_sub(start))
                                        .min(new_fields[i].end)
                                };
                                Some(Region::new(map(desired.a), map(desired.b)))
                            }),
                    ));
                    owned = edits.into_iter().map(|e| (e.range, e.text)).collect();
                    self.fields = Some(fields);
                }
                Err(_) => {
                    if fields.rebase(&input_changes) {
                        self.fields = Some(fields);
                    }
                }
            }
        }
        let changes = ChangeSet::new(
            owned
                .iter()
                .map(|(r, s)| Change {
                    old_range: r.clone(),
                    new_len: s.len(),
                })
                .collect(),
        );
        let selection = after.unwrap_or_else(|| {
            let old = self.buffer.view(self.id).sel();
            selection_primary(
                old.iter()
                    .map(|r| {
                        Region::new(
                            changes.transform_point(r.a, Bias::Right),
                            changes.transform_point(r.b, Bias::Right),
                        )
                    })
                    .collect(),
                old.primary_index(),
            )
        });
        let old = self.buffer.text().clone();
        let before_version = self.buffer.change_id();
        self.layout
            .borrow_mut()
            .prepare_change((self.buffer.id(), self.buffer.change_id()));
        let edits: Vec<_> = owned
            .iter()
            .map(|(range, text)| Edit {
                range: range.clone(),
                text,
            })
            .collect();
        self.buffer.transact(self.id, kind, &edits, selection);
        self.publish_direct_change(old, before_version, changes);
    }
    /// Publishes one completed transaction made by the host or the vi engine.
    pub fn publish_direct_change(
        &mut self,
        old: vel_text::Text,
        before_version: vel_buffer::ChangeId,
        changes: ChangeSet,
    ) {
        self.annotations.borrow_mut().change(
            &self.buffer,
            &self.settings,
            before_version,
            &changes,
        );
        self.fields_version = self
            .fields
            .as_ref()
            .map(|_| (self.buffer.id(), self.buffer.change_id()));
        if let Some(language) = self.language.borrow_mut().as_mut() {
            language.change(
                self.buffer.text().clone(),
                self.buffer.change_id(),
                changes.clone(),
                self.preferences.tab_size,
            );
        }
        self.layout.borrow_mut().change(
            &old,
            self.buffer.text(),
            (self.buffer.id(), self.buffer.change_id()),
            &changes,
        );
        let mut shared = self.layout_edits.borrow_mut();
        shared.push_back(crate::editor::SharedLayoutEdit {
            buffer: self.buffer.id(),
            from: before_version,
            to: self.buffer.change_id(),
            before: old,
            after: self.buffer.text().clone(),
            changes,
        });
        while shared.len() > 64 {
            shared.pop_front();
        }
        self.layout_version = self.buffer.change_id();
    }
    fn replace_selections(&mut self, values: Vec<(String, Range<usize>)>, kind: EditKind) {
        let selection = self.buffer.view(self.id).sel();
        let mut delta = 0isize;
        let mut regions = Vec::new();
        let owned: Vec<_> = selection
            .iter()
            .zip(values)
            .map(|(r, (s, relative))| {
                let begin = r.begin().wrapping_add_signed(delta);
                regions.push(Region::new(begin + relative.start, begin + relative.end));
                delta += s.len() as isize - r.size() as isize;
                (r.to_range(), s)
            })
            .collect();
        let after = selection_primary(regions, selection.primary_index());
        self.apply_edits(owned, Some(after), kind);
    }
    pub fn language_command(&mut self, name: &str, args: &Value) -> Result<bool, String> {
        match name {
            "insert" => {
                let s = args["characters"]
                    .as_str()
                    .ok_or("insert needs characters")?;
                if s == "\n" && self.settings.bool("trim_automatic_white_space", true) {
                    self.newline_trimming_blank_lines();
                    return Ok(true);
                }
                let values = self
                    .buffer
                    .view(self.id)
                    .sel()
                    .iter()
                    .map(|r| {
                        if s == "\n" {
                            self.newline(r)
                        } else {
                            (s.to_owned(), s.len()..s.len())
                        }
                    })
                    .collect();
                self.replace_selections(values, EditKind::InsertChars);
            }
            "paste_and_indent" => {
                let clipboard = normalize_line_breaks(&self.clipboard);
                let whole_lines = last_copy(&clipboard).is_some_and(|c| c.whole_lines);
                let selection = self.buffer.view(self.id).sel().clone();
                let mut edits = Vec::new();
                let mut seen = std::collections::BTreeSet::new();
                for r in selection.iter() {
                    let row = self.text.byte_to_line(r.begin());
                    let start = self.text.line_to_byte(row);
                    let line = self.text.line(row).to_string();
                    let indent = &line[..line.len() - line.trim_start_matches([' ', '\t']).len()];
                    if whole_lines && r.empty() {
                        // Above the caret's line, at that line's indentation.
                        if seen.insert(start) {
                            edits.push((start..start, reindent_block(&clipboard, indent, false)));
                        }
                        continue;
                    }
                    // At the caret: inside the indentation the destination indentation is what
                    // precedes the caret; the first pasted line keeps its place on the line.
                    let before = &line[..(r.begin() - start).min(line.len())];
                    let at_indent = before.trim_start_matches([' ', '\t']).is_empty();
                    let dest = if at_indent { before } else { indent };
                    edits.push((r.to_range(), reindent_block(&clipboard, dest, true)));
                }
                self.apply_edits(edits, None, EditKind::Paste);
            }
            "paste" => {
                // Pasted text uses the buffer's LF line breaks whatever the source used.
                let clipboard = normalize_line_breaks(&self.clipboard);
                let selection = self.buffer.view(self.id).sel().clone();
                let count = selection.len();
                let copy = last_copy(&clipboard);
                let parts = copy
                    .as_ref()
                    .map(|c| c.parts.clone())
                    .filter(|parts| parts.len() == count);
                if copy.as_ref().is_some_and(|c| c.whole_lines)
                    && selection.iter().all(vel_buffer::Region::empty)
                {
                    // Whole lines copied without a selection go above each caret's line, and the
                    // carets stay with their text.
                    let mut edits = Vec::new();
                    for (i, r) in selection.iter().enumerate() {
                        let line = self.text.byte_to_line(r.b);
                        let start = self.text.line_to_byte(line);
                        let value = parts.as_ref().map_or(clipboard.as_str(), |p| &p[i]);
                        if edits
                            .last()
                            .is_some_and(|(at, _): &(usize, String)| *at == start)
                        {
                            continue;
                        }
                        edits.push((start, value.to_owned()));
                    }
                    let mut shift = 0;
                    let mut regions = Vec::new();
                    let mut k = 0;
                    for r in selection.iter() {
                        while k < edits.len() && edits[k].0 <= r.b {
                            shift += edits[k].1.len();
                            k += 1;
                        }
                        regions.push(vel_buffer::Region::caret(r.b + shift));
                    }
                    let owned = edits.into_iter().map(|(at, v)| (at..at, v)).collect();
                    self.apply_edits(
                        owned,
                        Some(Selection::from_regions(regions)),
                        EditKind::Paste,
                    );
                    return Ok(true);
                }
                if let Some(parts) = parts.filter(|_| count > 1) {
                    let values = parts
                        .into_iter()
                        .map(|s| {
                            let n = s.len();
                            (s, n..n)
                        })
                        .collect();
                    self.replace_selections(values, EditKind::Paste);
                    return Ok(true);
                }
                let lines: Vec<_> = clipboard.split('\n').collect();
                let values: Vec<_> = if count > 1 && lines.len() == count {
                    lines
                        .iter()
                        .map(|s| ((*s).to_owned(), s.len()..s.len()))
                        .collect()
                } else {
                    vec![(clipboard.clone(), clipboard.len()..clipboard.len()); count]
                };
                self.replace_selections(values, EditKind::Paste);
            }
            "left_delete" | "right_delete" => {
                let mut ranges = Vec::new();
                for r in self.buffer.view(self.id).sel().iter() {
                    if !r.empty() {
                        ranges.push(r);
                        continue;
                    }
                    let mut at = if name == "right_delete" {
                        self.text.next_grapheme_boundary(r.b)
                    } else {
                        self.text.prev_grapheme_boundary(r.b)
                    };
                    // Preserve vel-buffer's tab-stop deletion while forwarding exact edits.
                    if name == "left_delete"
                        && self.preferences.translate_tabs_to_spaces
                        && self.preferences.use_tab_stops
                    {
                        let begin = self.text.line_to_byte(self.text.byte_to_line(r.b));
                        let prefix = self.text.slice(begin..r.b).to_string();
                        if !prefix.is_empty() && prefix.chars().all(|c| c == ' ' || c == '\t') {
                            let size = self.preferences.tab_size;
                            let target = (cells_with_tab(&prefix, size) - 1) / size * size;
                            let mut column = 0;
                            at = begin;
                            for c in prefix.chars() {
                                let next =
                                    column + if c == '\t' { size - column % size } else { 1 };
                                if next > target {
                                    break;
                                }
                                at += c.len_utf8();
                                column = next;
                            }
                        }
                    }
                    ranges.push(Region::new(r.b.min(at), r.b.max(at)));
                }
                let selection = Selection::from_regions(ranges);
                self.apply_edits(
                    selection
                        .iter()
                        .map(|r| (r.to_range(), String::new()))
                        .collect(),
                    None,
                    EditKind::Delete,
                );
            }
            "toggle_comment" => self.toggle_comment(args["block"].as_bool().unwrap_or(false))?,
            "reindent" => self.start_reindent()?,
            "fold" | "unfold" | "fold_by_level" | "unfold_all" | "fold_tag_attributes" => {
                self.fold_command(name, args)?
            }
            "move" if matches!(args["by"].as_str(), Some("lines" | "pages")) => {
                if self.layout.borrow().total() == 0 {
                    self.layout.borrow_mut().configure(
                        &self.text,
                        (self.buffer.id(), self.buffer.change_id()),
                        crate::layout::Options {
                            width: 0,
                            tab: self.preferences.tab_size,
                            indent: true,
                        },
                        &self.folded(),
                    );
                }
                let forward = args["forward"].as_bool().unwrap_or(false);
                let extend = args["extend"].as_bool().unwrap_or(false);
                let layout = self.layout.borrow();
                // The buffer's v1 VerticalLayout assumes LF line numbers. The view supplies
                // visual row numbers here while using the same x/offset/page contract.
                let old = self.buffer.view(self.id).sel();
                let after = selection_primary(
                    old.iter()
                        .map(|r| {
                            let x = r.xpos.unwrap_or_else(|| layout.x_of(&self.text, r.b));
                            let row = layout.visual_at(&self.text, r.b).0;
                            let delta = if args["by"] == "pages" {
                                layout.lines_per_page()
                            } else {
                                1
                            };
                            let visual = if forward {
                                row.saturating_add(delta)
                                    .min(layout.total().saturating_sub(1))
                            } else {
                                row.saturating_sub(delta)
                            };
                            let visual = layout.editable_visual(visual, forward);
                            let b = layout.offset_at(&self.text, visual, x);
                            Region {
                                a: if extend { r.a } else { b },
                                b,
                                xpos: Some(x),
                            }
                        })
                        .collect(),
                    old.primary_index(),
                );
                drop(layout);
                self.buffer.set_selection(self.id, after);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
    /// Breaks the line at every selection like `insert "\n"`, but a caret alone on a line that
    /// holds only indentation (typically left by the previous auto-indent) leaves that line
    /// empty instead of keeping trailing white space (`trim_automatic_white_space`).
    fn newline_trimming_blank_lines(&mut self) {
        let selection = self.buffer.view(self.id).sel().clone();
        let mut delta = 0isize;
        let mut regions = Vec::new();
        let mut owned = Vec::new();
        for (i, r) in selection.iter().enumerate() {
            let (text, relative, blank) = self.newline_with_blank(r);
            let mut range = r.to_range();
            // Selections are sorted and disjoint, so only the neighbours can share the row.
            if let Some(line) = blank.filter(|_| r.empty()) {
                let row = self.text.byte_to_line(r.b);
                let on_row = |o: Option<Region>, end: bool| {
                    o.is_some_and(|o| {
                        self.text
                            .byte_to_line(if end { o.end() } else { o.begin() })
                            == row
                    })
                };
                if !on_row(i.checked_sub(1).and_then(|p| selection.get(p)), true)
                    && !on_row(selection.get(i + 1), false)
                {
                    range = line;
                }
            }
            let begin = (range.start as isize + delta) as usize;
            regions.push(Region::new(begin + relative.start, begin + relative.end));
            delta += text.len() as isize - range.len() as isize;
            owned.push((range, text));
        }
        let after = selection_primary(regions, selection.primary_index());
        self.apply_edits(owned, Some(after), EditKind::InsertChars);
    }
    fn newline(&self, r: Region) -> (String, Range<usize>) {
        let (text, caret, _) = self.newline_with_blank(r);
        (text, caret)
    }
    /// [`Self::newline`] plus the byte range of the line holding `r.begin()` when that line is
    /// non-empty and holds only spaces and tabs.
    fn newline_with_blank(&self, r: Region) -> (String, Range<usize>, Option<Range<usize>>) {
        let row = self.text.byte_to_line(r.begin());
        if self.text.line_len_bytes(row) >= crate::large::LONG_LINE {
            return ("\n".into(), 1..1, None);
        }
        if !self.settings.bool("auto_indent", true) {
            return ("\n".into(), 1..1, None);
        }
        let start = self.text.line_to_byte(row);
        let line = self.text.line(row).to_string();
        let blank = (!line.is_empty() && line.bytes().all(|b| matches!(b, b' ' | b'\t')))
            .then(|| start..start + line.len());
        let prefix = &line[..r.begin() - start];
        let leading: String = prefix
            .chars()
            .take_while(|c| matches!(c, ' ' | '\t'))
            .collect();
        let mut indent = cells_with_tab(&leading, self.preferences.tab_size);
        let after_row = self.text.byte_to_line(r.end());
        let after = self
            .text
            .slice(
                r.end()..self.text.line_to_byte(after_row) + self.text.line(after_row).len_bytes(),
            )
            .to_string();
        let language = self.language.borrow();
        let mut increase = false;
        let mut decrease = false;
        let mut temporary_previous = false;
        let mut excluded = false;
        let smart = self.settings.bool("smart_indent", true);
        if let Some(l) = language.as_ref().filter(|_| smart) {
            let scopes = l.scopes(&self.text, r.begin().saturating_sub(1));
            let m = l.assets.metadata.metadata_for(&scopes);
            excluded = l
                .assets
                .metadata
                .matches_pattern(&m, "unIndentedLinePattern", prefix);
            let disable =
                l.assets
                    .metadata
                    .matches_pattern(&m, "disableIndentNextLinePattern", prefix);
            if !excluded && !disable {
                increase = l
                    .assets
                    .metadata
                    .matches_pattern(&m, "increaseIndentPattern", prefix)
                    || l.assets.metadata.matches_pattern(
                        &m,
                        "bracketIndentNextLinePattern",
                        prefix,
                    );
                // An unmatched opening delimiter supplements metadata for code syntaxes.
                let code = !l.selector(&self.text, "string, comment", r.begin().saturating_sub(1));
                if code && prefix.trim_end().ends_with(['{', '[', '(']) {
                    increase = true;
                }
                decrease = l
                    .assets
                    .metadata
                    .matches_pattern(&m, "decreaseIndentPattern", &after)
                    || (code && after.trim_start().starts_with(['}', ']', ')']));
                if row > 0 {
                    let previous = self.text.line(row - 1).to_string();
                    temporary_previous = l.assets.metadata.matches_pattern(
                        &m,
                        "bracketIndentNextLinePattern",
                        previous.trim_end(),
                    ) && !l.assets.metadata.matches_pattern(
                        &m,
                        "increaseIndentPattern",
                        previous.trim_end(),
                    );
                }
            }
        }
        if excluded {
            indent = 0;
        } else if increase {
            indent += self.preferences.tab_size;
        } else if temporary_previous {
            indent = indent.saturating_sub(self.preferences.tab_size);
        }
        let whitespace = |n: usize| {
            if self.preferences.translate_tabs_to_spaces {
                " ".repeat(n)
            } else {
                format!(
                    "{}{}",
                    "\t".repeat(n / self.preferences.tab_size),
                    " ".repeat(n % self.preferences.tab_size)
                )
            }
        };
        let mut text = format!("\n{}", whitespace(indent));
        let caret = text.len();
        if increase && decrease {
            text.push('\n');
            text.push_str(&whitespace(
                indent.saturating_sub(self.preferences.tab_size),
            ));
        }
        (text, caret..caret, blank)
    }
    fn toggle_comment(&mut self, block: bool) -> Result<(), String> {
        let language = self.language.borrow();
        let l = language
            .as_ref()
            .ok_or("No syntax metadata for this document")?;
        let mut edits = BTreeMap::new();
        let mut enclosed = Vec::new();
        let mut rows = BTreeSet::new();
        for region in self.buffer.view(self.id).sel().iter() {
            let m = l
                .assets
                .metadata
                .metadata_for(&l.scopes(&self.text, region.begin()));
            let mut delimiters = Vec::new();
            for suffix in ["", "_2", "_3", "_4"] {
                if let Some(start) = m.shell_variables.get(&format!("TM_COMMENT_START{suffix}")) {
                    let end = m
                        .shell_variables
                        .get(&format!("TM_COMMENT_END{suffix}"))
                        .cloned()
                        .unwrap_or_default();
                    if !start.is_empty() {
                        delimiters.push((start.clone(), end));
                    }
                }
            }
            if let Some((start, end)) = delimiters
                .iter()
                .find(|(_, end)| !end.is_empty())
                .filter(|_| block || !delimiters.iter().any(|(_, end)| end.is_empty()))
            {
                let a = region.begin();
                let b = region.end();
                let selected = self.text.slice(a..b).to_string();
                if selected.starts_with(start)
                    && selected.ends_with(end)
                    && selected.len() >= start.len() + end.len()
                {
                    edits.insert((a, a + start.len()), String::new());
                    edits.insert((b - end.len(), b), String::new());
                } else if a >= start.len()
                    && b + end.len() <= self.text.len_bytes()
                    && self.text.is_char_boundary(a - start.len())
                    && self.text.is_char_boundary(b + end.len())
                    && self.text.slice(a - start.len()..a).to_string() == *start
                    && self.text.slice(b..b + end.len()).to_string() == *end
                {
                    edits.insert((a - start.len(), a), String::new());
                    edits.insert((b, b + end.len()), String::new());
                } else {
                    edits.insert((a, b), format!("{start}{selected}{end}"));
                    enclosed.push((region, start.len()));
                }
            } else if let Some((start, _)) = delimiters.iter().find(|(_, end)| end.is_empty()) {
                let first = self.text.byte_to_line(region.begin());
                let last = self
                    .text
                    .byte_to_line(region.end().saturating_sub(usize::from(!region.empty())));
                for row in first..=last {
                    rows.insert((row, start.clone()));
                }
            } else {
                return Err(format!(
                    "{} has no {}comment delimiter",
                    l.name,
                    if block { "block " } else { "" }
                ));
            }
        }
        // A mixed selection is commented uniformly; a fully commented one is uncommented. Blank
        // lines are never commented, so they do not count either way.
        let remove = rows.iter().all(|(row, prefix)| {
            let line = self.text.line(*row).to_string();
            line.trim().is_empty() || line.trim_start().starts_with(prefix.trim_end())
        });
        for (row, prefix) in rows {
            let line = self.text.line(row).to_string();
            let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
            if line.trim().is_empty() {
                continue;
            }
            let byte = self.text.line_to_byte(row) + indent;
            if remove {
                let count = if line[indent..].starts_with(&prefix) {
                    prefix.len()
                } else {
                    prefix.trim_end().len()
                };
                edits.insert((byte, byte + count), String::new());
            } else {
                edits.insert((byte, byte), prefix);
            }
        }
        drop(language);
        let after = if enclosed.is_empty() {
            None
        } else {
            let changes = ChangeSet::new(
                edits
                    .iter()
                    .map(|((a, b), s)| Change {
                        old_range: *a..*b,
                        new_len: s.len(),
                    })
                    .collect(),
            );
            let old = self.buffer.view(self.id).sel();
            Some(selection_primary(
                old.iter()
                    .map(|r| {
                        if let Some((_, prefix)) = enclosed.iter().find(|(old, _)| *old == r) {
                            let a = changes.transform_point(r.begin(), Bias::Left) + prefix;
                            let b = a + r.size();
                            if r.a <= r.b {
                                Region::new(a, b)
                            } else {
                                Region::new(b, a)
                            }
                        } else {
                            Region::new(
                                changes.transform_point(r.a, Bias::Right),
                                changes.transform_point(r.b, Bias::Right),
                            )
                        }
                    })
                    .collect(),
                old.primary_index(),
            ))
        };
        self.apply_edits(
            edits.into_iter().map(|((a, b), s)| (a..b, s)).collect(),
            after,
            EditKind::Other,
        );
        Ok(())
    }
    fn start_reindent(&mut self) -> Result<(), String> {
        let language = self.language.borrow();
        let l = language
            .as_ref()
            .ok_or("No syntax metadata for this document")?;
        let metadata = l.assets.metadata.clone();
        let set = l.assets.syntaxes.clone();
        let syntax = l.syntax.clone();
        let text = self.text.clone();
        let version = self.buffer.change_id();
        let buffer = self.buffer.id();
        let options = vel_syntax::indent::IndentOptions {
            tab_size: self.preferences.tab_size,
            spaces: self.preferences.translate_tabs_to_spaces,
        };
        let regions: Vec<_> = self.buffer.view(self.id).sel().iter().collect();
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("velocity-reindent".into())
            .spawn(move || {
                let edits = (|| {
                    let source = text.to_string();
                    let reference = set.find_syntax_by_path(&syntax).expect("bound syntax");
                    let spans = vel_syntax::indent::scoped_spans(&set, reference, &source)?;
                    let result = vel_syntax::indent::reindent(&source, &spans, &metadata, options);
                    let all = regions.iter().all(|r| r.empty());
                    let mut offset = 0;
                    let mut edits = Vec::new();
                    for (old, new) in source
                        .split_inclusive('\n')
                        .zip(result.split_inclusive('\n'))
                    {
                        let row = text.byte_to_line(offset);
                        let selected = all
                            || regions.iter().any(|r| {
                                text.byte_to_line(r.begin()) <= row
                                    && row
                                        <= text.byte_to_line(
                                            r.end().saturating_sub(usize::from(!r.empty())),
                                        )
                            });
                        let a = old.len() - old.trim_start_matches([' ', '\t']).len();
                        let b = new.len() - new.trim_start_matches([' ', '\t']).len();
                        if selected && old[..a] != new[..b] {
                            edits.push((offset..offset + a, new[..b].to_owned()));
                        }
                        offset += old.len();
                    }
                    Ok(edits)
                })();
                let _ = tx.send((buffer, version, edits));
            })
            .map_err(|e| e.to_string())?;
        self.reindent_job = Some(rx);
        Ok(())
    }
    pub fn fold_command(&mut self, name: &str, args: &Value) -> Result<(), String> {
        if name == "unfold_all" {
            self.buffer.erase_regions(self.id, FOLDS);
            self.layout.borrow_mut().invalidate();
            return Ok(());
        }
        let selections: Vec<_> = self.buffer.view(self.id).sel().iter().collect();
        let mut current = self.folded();
        if name == "unfold" {
            current.retain(|fold| {
                !selections.iter().any(|r| {
                    let row = self.text.byte_to_line(r.b);
                    (fold.begin() <= r.end() && r.begin() <= fold.end())
                        || self.text.byte_to_line(fold.begin()) == row
                })
            });
        } else {
            let language = self.language.borrow();
            let analysis = language
                .as_ref()
                .and_then(|l| l.analysis.as_ref())
                .ok_or("Syntax analysis is still running; retry folding shortly")?;
            if name == "fold_by_level" || name == "fold_tag_attributes" {
                let level = args["level"].as_u64().unwrap_or(1) as usize;
                current.extend(
                    analysis
                        .folds
                        .iter()
                        .filter(|f| {
                            if name == "fold_tag_attributes" {
                                f.attributes
                            } else {
                                !f.attributes && f.level == level
                            }
                        })
                        .map(|f| Region::from(f.range.clone())),
                );
            } else {
                for selection in selections {
                    if !selection.empty() {
                        current.push(selection);
                        continue;
                    }
                    if let Some(fold) = analysis
                        .folds
                        .iter()
                        .filter(|f| {
                            !f.attributes
                                && self.text.byte_to_line(f.range.start)
                                    <= self.text.byte_to_line(selection.b)
                                && selection.b <= f.range.end
                                && !current.iter().any(|r| r.to_range() == f.range)
                        })
                        .min_by_key(|f| f.range.len())
                    {
                        current.push(Region::from(fold.range.clone()));
                    }
                }
            }
        }
        current.sort_by_key(|r| (r.begin(), std::cmp::Reverse(r.end())));
        let mut disjoint: Vec<Region> = Vec::new();
        for region in current {
            if disjoint
                .last()
                .is_none_or(|last| last.end() <= region.begin())
            {
                disjoint.push(region);
            }
        }
        self.buffer.add_regions(self.id, FOLDS, disjoint, true);
        self.layout.borrow_mut().invalidate();
        Ok(())
    }
    pub fn folded(&self) -> Vec<Region> {
        self.buffer
            .view(self.id)
            .get_regions(FOLDS)
            .map(|set| set.regions.clone())
            .unwrap_or_default()
    }
}
pub(crate) fn selection_primary(mut regions: Vec<Region>, primary: usize) -> Selection {
    if primary < regions.len() {
        let r = regions.remove(primary);
        regions.push(r);
    }
    Selection::from_regions(regions)
}

/// The regions of the last copy or cut made in this process, kept while the clipboard still
/// holds the text it produced.
#[derive(Clone, Debug)]
pub struct LastCopy {
    text: String,
    /// Text of each copied region.
    pub parts: Vec<String>,
    /// The regions were whole lines copied from empty selections.
    pub whole_lines: bool,
}
thread_local! {
    static LAST_COPY: std::cell::RefCell<Option<LastCopy>> = const { std::cell::RefCell::new(None) };
}
/// Records a copy or cut that put `text` on the clipboard.
pub fn remember_copy(text: &str, parts: Vec<String>, whole_lines: bool) {
    LAST_COPY.with(|c| {
        *c.borrow_mut() = Some(LastCopy {
            text: normalize_line_breaks(text),
            parts,
            whole_lines,
        })
    });
}
/// The last copy, if the (normalized) clipboard still holds its text.
pub fn last_copy(clipboard: &str) -> Option<LastCopy> {
    LAST_COPY.with(|c| c.borrow().clone().filter(|c| c.text == clipboard))
}
/// Converts CRLF and lone CR line breaks to LF.
pub fn normalize_line_breaks(text: &str) -> String {
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text.to_owned()
    }
}

/// Re-bases the indentation of pasted `text` onto `dest`: the whitespace prefix shared by its
/// non-blank lines is replaced by `dest`, and blank lines become empty. With `inline` the first
/// line continues the destination line at the caret, so it gets no prefix, and it only counts
/// towards the shared prefix when it starts with whitespace (it was copied from a line start).
pub fn reindent_block(text: &str, dest: &str, inline: bool) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let first_counts = !inline || lines[0].starts_with([' ', '\t']);
    let mut base: Option<&str> = None;
    for (i, line) in lines.iter().enumerate() {
        if (i == 0 && !first_counts) || line.trim().is_empty() {
            continue;
        }
        let prefix = &line[..line.len() - line.trim_start_matches([' ', '\t']).len()];
        base = Some(match base {
            None => prefix,
            Some(b) => {
                let common = b
                    .bytes()
                    .zip(prefix.bytes())
                    .take_while(|(x, y)| x == y)
                    .count();
                &b[..common]
            }
        });
    }
    let base = base.unwrap_or("");
    let strip = |line: &str| -> String {
        line.strip_prefix(base)
            .unwrap_or_else(|| line.trim_start_matches([' ', '\t']))
            .to_owned()
    };
    lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            if inline && i == 0 {
                if first_counts {
                    strip(line)
                } else {
                    (*line).to_owned()
                }
            } else if line.trim().is_empty() {
                String::new()
            } else {
                format!("{dest}{}", strip(line))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
