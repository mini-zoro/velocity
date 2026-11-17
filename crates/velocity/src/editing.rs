//! Language-aware commands over exact, old-coordinate buffer transactions.
use crate::{
    editor::{Editor, cells_with_tab},
    layout::FOLDS,
};
use serde_json::json;
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
        // Selection effects belong to the requested command, including replacements whose
        // bytes already match. Only text publication uses the filtered change set.
        let selection = after.unwrap_or_else(|| {
            let changes = ChangeSet::new(
                owned
                    .iter()
                    .map(|(r, s)| Change {
                        old_range: r.clone(),
                        new_len: s.len(),
                    })
                    .collect(),
            );
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
        owned.retain(|(range, text)| {
            !self.buffer.edit_is_noop(&Edit {
                range: range.clone(),
                text,
            })
        });
        if owned.is_empty() {
            if self.buffer.view(self.id).sel() != &selection {
                self.buffer.set_selection(self.id, selection);
            }
            return;
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
        if self.buffer.change_id() == before_version {
            return;
        }
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
            "move_to" if args["to"] == "brackets" => self.bracket_command(false, args),
            "expand_selection"
                if matches!(
                    args["to"].as_str(),
                    Some("brackets" | "scope" | "tag" | "smart")
                ) =>
            {
                self.bracket_command(true, args)
            }
            "toggle_comment" => self.toggle_comment(args["block"].as_bool().unwrap_or(false))?,
            "reindent" => self.start_reindent()?,
            "expand_tabs" | "unexpand_tabs" => {
                self.convert_leading_tabs(name == "expand_tabs", args)?
            }
            "detect_indentation" => {
                if let Some((size, spaces)) = crate::language::detect_indentation(&self.text) {
                    self.settings
                        .set(vel_config::settings::Layer::View, "tab_size", json!(size));
                    self.settings.set(
                        vel_config::settings::Layer::View,
                        "translate_tabs_to_spaces",
                        json!(spaces),
                    );
                    self.sync();
                }
            }
            "fold" | "unfold" | "fold_by_level" | "unfold_all" | "fold_tag_attributes" => {
                self.fold_command(name, args)?
            }
            "select_lines" | "move"
                if name == "select_lines"
                    || matches!(args["by"].as_str(), Some("lines" | "pages")) =>
            {
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
                let mut regions = if name == "select_lines" {
                    old.iter().collect::<Vec<_>>()
                } else {
                    vec![]
                };
                regions.extend(old.iter().map(|r| {
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
                    let a = if name == "select_lines" && !r.empty() {
                        let row = layout.visual_at(&self.text, r.a).0;
                        let next = if forward {
                            (row + 1).min(layout.total().saturating_sub(1))
                        } else {
                            row.saturating_sub(1)
                        };
                        layout.offset_at(
                            &self.text,
                            layout.editable_visual(next, forward),
                            layout.x_of(&self.text, r.a),
                        )
                    } else if extend {
                        r.a
                    } else {
                        b
                    };
                    Region {
                        a,
                        b,
                        xpos: Some(x),
                    }
                }));
                let primary =
                    old.primary_index() + if name == "select_lines" { old.len() } else { 0 };
                let after = selection_primary(regions, primary);
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
    fn bracket_command(&mut self, expand: bool, args: &Value) {
        let language = self.language.borrow();
        let old = self.buffer.view(self.id).sel();
        let regions = old
            .iter()
            .map(|r| {
                let pairs = if expand && args["to"].as_str() == Some("scope") {
                    Vec::new()
                } else {
                    crate::language::command_pairs(
                        language.as_ref(),
                        &self.text,
                        if expand { r.begin() } else { r.b },
                    )
                };
                if !expand {
                    let pair = pairs
                        .iter()
                        .filter(|p| p.kind != '<')
                        .filter(|p| p.open.start <= r.b && r.b <= p.close.end)
                        .min_by_key(|p| {
                            let priority = if p.open.contains(&r.b) || p.close.contains(&r.b) {
                                0
                            } else if p.open.end == r.b || p.close.end == r.b {
                                1
                            } else {
                                2
                            };
                            (priority, p.close.end - p.open.start)
                        });
                    return pair.map_or(r, |p| {
                        let at = if p.close.contains(&r.b) || r.b == p.close.end {
                            p.open.start
                        } else {
                            p.close.start
                        };
                        Region::new(
                            if args["extend"].as_bool().unwrap_or(false) {
                                r.a
                            } else {
                                at
                            },
                            at,
                        )
                    });
                }
                let target = args["to"].as_str().unwrap_or("brackets");
                let mut candidates = Vec::new();
                if matches!(target, "scope" | "smart")
                    && let Some(l) = language.as_ref()
                {
                    candidates.extend(crate::language::command_scope_regions(l, r.begin(), ""));
                }
                if target == "scope" && language.is_none() {
                    candidates.push(0..self.text.len_bytes());
                }
                for pair in pairs.iter().filter(|p| {
                    if target == "scope" {
                        false
                    } else if target == "tag" {
                        p.kind == 't'
                    } else {
                        p.kind != '<' && (target == "smart" || p.kind != 't')
                    }
                }) {
                    candidates.push(pair.open.end..pair.close.start);
                    candidates.push(pair.open.start..pair.close.end);
                }
                candidates
                    .into_iter()
                    .filter(|c| {
                        c.start <= r.begin()
                            && r.end() <= c.end
                            && (c.start < r.begin() || r.end() < c.end)
                    })
                    .min_by_key(|c| c.len())
                    .map_or(r, |c| {
                        if r.a <= r.b {
                            Region::new(c.start, c.end)
                        } else {
                            Region::new(c.end, c.start)
                        }
                    })
            })
            .collect();
        let after = selection_primary(regions, old.primary_index());
        drop(language);
        self.buffer.set_selection(self.id, after);
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
            let mut comments =
                crate::language::command_scope_regions(l, region.begin(), "comment.block");
            if comments.is_empty() && l.scope_snapshot_at(region.begin()).is_none() {
                let row = self.text.byte_to_line(region.begin());
                let line = self.text.line(row).to_string();
                let base = self.text.line_to_byte(row);
                let column = region.begin() - base;
                for (start, end) in delimiters.iter().filter(|(_, end)| !end.is_empty()) {
                    if let Some(a) = line[..column].rfind(start.trim_end()) {
                        let body = a + start.trim_end().len();
                        if !line[body..column.max(body)].contains(end.trim_start())
                            && let Some(b) = line[body..].find(end.trim_start())
                        {
                            comments.push(base + a..base + body + b + end.trim_start().len());
                        }
                    }
                }
            }
            if let Some(comment) = comments.into_iter().last()
                && region.end() <= comment.end
                && let Some((start, end)) = delimiters.iter().find(|(_, end)| !end.is_empty())
            {
                let source = self.text.slice(comment.clone()).to_string();
                let mut start_len = if source.starts_with(start) {
                    start.len()
                } else {
                    start.trim_end().len()
                };
                let mut end_len = if source.ends_with(end) {
                    end.len()
                } else {
                    end.trim_start().len()
                };
                // Block-only line comments add one padding space on each side. Recognize
                // that symmetric, single-line form without trimming a multiline body.
                if !delimiters.iter().any(|(_, end)| end.is_empty()) && !source.contains('\n') {
                    if start_len == start.trim_end().len()
                        && source.as_bytes().get(start_len) == Some(&b' ')
                    {
                        start_len += 1;
                    }
                    if source.len() > start_len + end_len
                        && end_len == end.trim_start().len()
                        && source
                            .as_bytes()
                            .get(source.len().saturating_sub(end_len + 1))
                            == Some(&b' ')
                    {
                        end_len += 1;
                    }
                }
                if source.starts_with(start.trim_end()) && source.ends_with(end.trim_start()) {
                    edits.insert((comment.start, comment.start + start_len), String::new());
                    edits.insert((comment.end - end_len, comment.end), String::new());
                    continue;
                }
            }
            if let Some((start, end)) = delimiters
                .iter()
                .find(|(_, end)| !end.is_empty())
                .filter(|_| block || !delimiters.iter().any(|(_, end)| end.is_empty()))
            {
                let whole_line = region.empty() && !block;
                let row = self.text.byte_to_line(region.b);
                let a = if whole_line {
                    self.text.line_to_byte(row)
                } else {
                    region.begin()
                };
                let b = if whole_line {
                    a + self.text.line(row).len_bytes()
                } else {
                    region.end()
                };
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
                    let (left, right) = if whole_line {
                        (
                            format!("{} ", start.trim_end()),
                            format!(" {}", end.trim_start()),
                        )
                    } else {
                        (start.clone(), end.clone())
                    };
                    // Insert separate delimiters to preserve carets and selection interiors.
                    if a == b {
                        edits.insert((a, b), format!("{left}{right}"));
                    } else {
                        edits.insert((a, a), left.clone());
                        edits.insert((b, b), right);
                    }
                    if !whole_line || a == b {
                        enclosed.push((region, left.len()));
                    }
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
    /// Convert only leading whitespace in one old-coordinate transaction. Tabs advance
    /// to visual tab stops; spaces after code and inside strings remain byte-identical.
    fn convert_leading_tabs(&mut self, spaces: bool, args: &Value) -> Result<(), String> {
        let size = self.preferences.tab_size.max(1);
        let language = self.language.borrow();
        if language.is_none() {
            return Err("Syntax scopes are unavailable; indentation was not changed".into());
        }
        // Commands can precede the first highlight publication or follow an edit in
        // the same event turn. Parse this exact document version, as reindent does,
        // rather than treating unavailable scopes as permission to change content.
        let parsed = language
            .as_ref()
            .filter(|l| !l.scopes_ready())
            .map(|l| {
                let syntax = l
                    .assets
                    .syntaxes
                    .find_syntax_by_path(&l.syntax)
                    .ok_or("No syntax metadata for this document")?;
                vel_syntax::indent::scoped_spans(&l.assets.syntaxes, syntax, &self.text.to_string())
            })
            .transpose()?;
        let mut edits = Vec::new();
        for row in 0..self.text.len_lines() {
            let prefix: String = self
                .text
                .line(row)
                .chars()
                .take_while(|c| matches!(c, ' ' | '\t'))
                .collect();
            let width = vel_syntax::indent::indentation(&prefix, size);
            let replacement = if spaces {
                " ".repeat(width)
            } else {
                "\t".repeat(width / size) + &" ".repeat(width % size)
            };
            if prefix != replacement {
                let start = self.text.line_to_byte(row);
                // Leading whitespace in strings/heredocs is document content (notably
                // <<-EOF's tab stripping). Unknown scopes never authorize conversion.
                let protected = if let Some(spans) = &parsed {
                    let first = spans.partition_point(|span| span.range.end <= start);
                    let leading_spans = &spans[first..];
                    if !leading_spans
                        .first()
                        .is_some_and(|span| span.range.start <= start)
                        || !leading_spans
                            .iter()
                            .take_while(|span| span.range.start < start + prefix.len())
                            .any(|span| span.range.end >= start + prefix.len())
                    {
                        true
                    } else {
                        leading_spans
                            .iter()
                            .take_while(|span| span.range.start < start + prefix.len())
                            .any(|span| vel_syntax::indent::is_string_or_heredoc(&span.scopes))
                    }
                } else {
                    language
                        .as_ref()
                        .and_then(|l| l.scope_snapshot_at(start))
                        .is_none_or(|snapshot| {
                            snapshot
                                .lines
                                .get(row - snapshot.range.start)
                                .is_none_or(|line| {
                                    line.runs
                                        .iter()
                                        .take_while(|run| (run.start as usize) < prefix.len())
                                        .any(|run| {
                                            snapshot.stacks.get(run.stack).is_none_or(
                                                vel_syntax::indent::is_string_or_heredoc,
                                            )
                                        })
                                })
                        })
                };
                if protected {
                    continue;
                }
                edits.push((start..start + prefix.len(), replacement));
            }
        }
        drop(language);
        self.apply_edits(edits, None, EditKind::Other);
        if args["set_translate_tabs"].as_bool().unwrap_or(false) {
            self.settings.set(
                vel_config::settings::Layer::View,
                "translate_tabs_to_spaces",
                json!(spaces),
            );
        }
        self.sync();
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
        if name == "unfold_all" || (name == "fold_by_level" && args["level"] == 0) {
            self.set_folded(Vec::new());
            return Ok(());
        }
        let selections: Vec<_> = self.buffer.view(self.id).sel().iter().collect();
        let mut current = self.folded();
        if name == "unfold" {
            // Remove only the visible parent; its nested children stay folded.
            let visible = vel_render::fold::outermost(current.iter().map(|r| r.to_range()));
            current.retain(|fold| {
                !visible.contains(&fold.to_range())
                    || !selections.iter().any(|r| {
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
                if name == "fold_by_level" {
                    current.clear();
                }
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
        self.set_folded(current);
        Ok(())
    }
    /// Installs a fold forest without discarding nested children.
    pub fn set_folded(&mut self, mut folds: Vec<Region>) {
        folds.retain(|r| !r.empty() && r.end() <= self.text.len_bytes());
        folds.sort_by_key(|r| (r.begin(), std::cmp::Reverse(r.end())));
        folds.dedup_by_key(|r| r.to_range());
        let old = self.buffer.view(self.id).sel();
        let primary = old.primary_index();
        let outer = vel_render::fold::outermost(folds.iter().map(|r| r.to_range()));
        let mut at = 0;
        let mut snap = |p| {
            while outer.get(at).is_some_and(|f| f.end <= p) {
                at += 1;
            }
            outer
                .get(at)
                .filter(|f| f.start < p && p < f.end)
                .map_or(p, |f| f.start)
        };
        let selection = selection_primary(
            old.iter()
                .map(|r| {
                    let a = snap(r.begin());
                    let b = snap(r.end());
                    Region {
                        a: if r.a <= r.b { a } else { b },
                        b: if r.a <= r.b { b } else { a },
                        ..r
                    }
                })
                .collect(),
            primary,
        );
        let moved = old != &selection;
        self.buffer.set_selection(self.id, selection);
        self.buffer.add_regions(self.id, FOLDS, folds, true);
        self.annotations.borrow_mut().reset_folds(self.id);
        self.layout.borrow_mut().invalidate();
        if moved {
            self.sync();
        }
    }
    /// Returns the live fold forest. Selection endpoints reveal every enclosing
    /// fold, including queries after Find/Goto/undo and before the next scene.
    pub fn folded(&self) -> Vec<Region> {
        self.annotations.borrow_mut().folded(&self.buffer, self.id)
    }

    /// Captures the visible forest, including reveals since the last text edit.
    pub(crate) fn folded_view_state(
        &self,
        id: vel_buffer::ViewId,
        scroll: (f64, f64),
    ) -> vel_session::ViewState {
        let mut state = vel_session::ViewState::capture(&self.buffer, id, scroll);
        state.folds = self
            .annotations
            .borrow_mut()
            .folded(&self.buffer, id)
            .into_iter()
            .map(Into::into)
            .collect();
        state
    }

    /// Native undo history restores raw region sets. The saved live forest is
    /// authoritative for each sheet, including already imported clone views.
    pub(crate) fn restore_folded_view_state(
        &mut self,
        state: &vel_session::ViewState,
        id: vel_buffer::ViewId,
    ) {
        let text = self.buffer.text();
        let folds = state
            .folds
            .iter()
            .map(|p| {
                Region::new(
                    text.floor_char_boundary(p.a.min(text.len_bytes())),
                    text.floor_char_boundary(p.b.min(text.len_bytes())),
                )
            })
            .filter(|r| !r.empty())
            .collect();
        self.buffer.add_regions(id, FOLDS, folds, true);
        self.annotations.borrow_mut().reset_folds(id);
        self.layout.borrow_mut().invalidate();
    }

    /// Fold-aware horizontal motions run at the host's first text-command hook.
    pub(crate) fn fold_navigation_command(&mut self, name: &str, args: &Value) -> bool {
        if self
            .buffer
            .view(self.id)
            .get_regions(FOLDS)
            .is_none_or(|set| set.regions.is_empty())
        {
            return false;
        }
        let mut folds = self.folded();
        // Expose selected/deleted bytes, and insertion at a hidden fold end. Public
        // build 4143 notes (https://www.sublimetext.com/download) specify the fold
        // range, but not insertion at its end: reveal it so the caret and edit agree.
        if matches!(
            name,
            "insert"
                | "paste"
                | "paste_and_indent"
                | "insert_snippet"
                | "left_delete"
                | "right_delete"
                | "delete_word"
                | "insert_best_completion"
                | "commit_completion"
        ) {
            let insertion = !matches!(name, "left_delete" | "right_delete" | "delete_word");
            let mut edits: Vec<_> = self
                .buffer
                .view(self.id)
                .sel()
                .iter()
                .map(|r| {
                    if !r.empty() {
                        r.to_range()
                    } else if name == "left_delete"
                        || (name == "delete_word" && args["forward"] != true)
                    {
                        self.text.prev_grapheme_boundary(r.b)..r.b
                    } else if name == "right_delete" || name == "delete_word" {
                        r.b..self.text.next_grapheme_boundary(r.b)
                    } else {
                        r.b..r.b
                    }
                })
                .collect();
            if !edits.is_sorted_by_key(|r| r.start) {
                edits.sort_unstable_by_key(|r| r.start);
            }
            let mut at = 0;
            folds.retain(|f| {
                while edits.get(at).is_some_and(|r| r.end <= f.begin()) {
                    at += 1;
                }
                !edits.get(at).is_some_and(|r| {
                    r.start < f.end()
                        || (insertion && r.is_empty() && r.start == f.end() && {
                            let row = self.text.byte_to_line(f.end());
                            let start = self.text.line_to_byte(row);
                            // Only an empty projected suffix is drawn on
                            // the header; a visible closing line starts at 0.
                            f.end() > start
                                && f.end() == start + crate::layout::body_len(&self.text, row)
                        })
                })
            });
        }
        if self
            .buffer
            .view(self.id)
            .get_regions(FOLDS)
            .is_some_and(|set| set.regions != folds)
        {
            self.buffer.add_regions(self.id, FOLDS, folds.clone(), true);
            self.annotations.borrow_mut().reset_folds(self.id);
            self.layout.borrow_mut().invalidate();
        }
        if folds.is_empty() {
            return false;
        }
        if name != "move" || args["by"].as_str().unwrap_or("characters") != "characters" {
            return false;
        }
        let forward = args["forward"].as_bool().unwrap_or(false);
        let extend = args["extend"].as_bool().unwrap_or(false);
        let old = self.buffer.view(self.id).sel();
        let primary = old.primary_index();
        let outer = vel_render::fold::outermost(folds.iter().map(|r| r.to_range()));
        let mut fold_at = 0;
        let regions = old
            .iter()
            .map(|r| {
                let mut at = if !extend && !r.empty() {
                    if forward { r.end() } else { r.begin() }
                } else {
                    let next = if forward {
                        self.text.next_grapheme_boundary(r.b)
                    } else {
                        self.text.prev_grapheme_boundary(r.b)
                    };
                    while outer.get(fold_at).is_some_and(|f| f.end < r.b) {
                        fold_at += 1;
                    }
                    outer
                        .get(fold_at)
                        .filter(|f| {
                            if forward {
                                f.start <= r.b && next < f.end
                            } else {
                                f.start < next && r.b <= f.end
                            }
                        })
                        .map_or(next, |f| if forward { f.end } else { f.start })
                };
                at = at.min(self.text.len_bytes());
                Region::new(if extend { r.a } else { at }, at)
            })
            .collect();
        self.set_selection(selection_primary(regions, primary));
        true
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

#[cfg(test)]
#[path = "editing_round16_tests.rs"]
mod round16_tests;
