//! Diff/spelling commands use exact publications and normal undoable transactions.
use crate::editor::Editor;
use vel_buffer::{EditKind, Region, Selection};
use vel_config::{Value, json, settings::Layer};

pub fn line_byte(text: &vel_text::Text, row: usize) -> usize {
    if row >= text.len_lines() {
        text.len_bytes()
    } else {
        text.line_to_byte(row)
    }
}
impl Editor {
    pub fn annotation_command(&mut self, name: &str, args: &Value) -> Result<bool, String> {
        match name {
            "next_modification"
            | "prev_modification"
            | "toggle_inline_diff"
            | "revert_hunk"
            | "revert_modification" => {
                if self.settings.get("mini_diff") == Some(&json!(false)) {
                    return Ok(true);
                }
                self.annotations
                    .borrow_mut()
                    .sync(&self.buffer, &self.settings);
                self.annotations.borrow_mut().poll();
                let at = args["_context_byte"]
                    .as_u64()
                    .map_or(self.caret, |at| at as usize);
                let row = self.text.byte_to_line(at.min(self.text.len_bytes()));
                let mut annotations = self.annotations.borrow_mut();
                let Some(p) = annotations
                    .exact(self.buffer.id(), self.buffer.change_id())
                    .cloned()
                else {
                    self.actions.push((
                        "wait_for_diff_command".into(),
                        json!({"command":name,"args":args}),
                    ));
                    return Ok(true);
                };
                if matches!(name, "next_modification" | "prev_modification") {
                    let forward = name == "next_modification";
                    let next = if forward {
                        p.hunks
                            .iter()
                            .find(|h| h.new.start > row)
                            .or_else(|| p.hunks.first())
                    } else {
                        p.hunks
                            .iter()
                            .rev()
                            .find(|h| h.new.start < row)
                            .or_else(|| p.hunks.last())
                    };
                    if let Some(h) = next {
                        let at = line_byte(&self.text, h.new.start);
                        drop(annotations);
                        self.set_caret(at, false);
                    }
                    return Ok(true);
                }
                let hunk = p
                    .hunks
                    .iter()
                    .find(|h| {
                        let at = h.new.start.min(self.text.len_lines().saturating_sub(1));
                        at <= row && row < h.new.end.max(at + 1)
                    })
                    .cloned();
                let Some(hunk) = hunk else {
                    return Ok(true);
                };
                if name == "toggle_inline_diff" {
                    let was = annotations.inline.contains(&hunk.new.start);
                    if args["show_only"] == true || args["show_only_this"] == true {
                        annotations.inline.clear();
                    }
                    if was {
                        annotations.inline.remove(&hunk.new.start);
                    } else {
                        annotations.inline.insert(hunk.new.start);
                        annotations.load_inline(hunk.new.start);
                    }
                } else {
                    let range =
                        line_byte(&self.text, hunk.new.start)..line_byte(&self.text, hunk.new.end);
                    let text = p
                        .reference
                        .slice(
                            line_byte(&p.reference, hunk.old.start)
                                ..line_byte(&p.reference, hunk.old.end),
                        )
                        .to_string();
                    drop(annotations);
                    self.apply_edits(vec![(range, text)], None, EditKind::Other);
                }
            }
            "next_misspelling" | "prev_misspelling" => {
                // Navigation requests scopes in bounded pages, never a second parser.
                self.actions.push((
                    "navigate_misspelling".into(),
                    json!({"forward":name == "next_misspelling"}),
                ));
            }
            "add_word" | "ignore_word" => {
                let word = args["word"].as_str().ok_or("word argument required")?;
                let key = if name == "add_word" {
                    "added_words"
                } else {
                    "ignored_words"
                };
                let mut words = self
                    .settings
                    .get(key)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if !words.iter().any(|w| {
                    w.as_str()
                        .is_some_and(|w| w.to_lowercase() == word.to_lowercase())
                }) {
                    words.push(json!(word));
                }
                self.settings.set(Layer::User, key, json!(words));
                self.actions.push((
                    "persist_spelling_words".into(),
                    json!({"key":key,"words":words}),
                ));
            }
            "replace_misspelling" => {
                let start = args["start"].as_u64().ok_or("missing spelling start")? as usize;
                let end = args["end"].as_u64().ok_or("missing spelling end")? as usize;
                let word = args["word"].as_str().ok_or("missing spelling word")?;
                if args["version"].as_u64() == Some(self.buffer.change_id().0)
                    && args["buffer_id"].as_u64() == Some(self.buffer.id().0)
                    && start <= end
                    && end <= self.text.len_bytes()
                    && self.text.floor_char_boundary(start) == start
                    && self.text.floor_char_boundary(end) == end
                    && self.text.slice(start..end) == word
                {
                    let text = args["replacement"]
                        .as_str()
                        .ok_or("missing replacement")?
                        .to_owned();
                    self.apply_edits(vec![(start..end, text)], None, EditKind::Other);
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
    pub fn select_misspelling(&mut self, range: std::ops::Range<usize>) {
        self.set_selection(Selection::from_regions([Region::from(range)]));
    }
}
