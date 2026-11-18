//! Status text formatting, independent of document and native window services.
/// Incremental tab-expanded status columns for one recently queried physical line.
/// Checkpoints are relative to the line, so edits to earlier rows only move its origin.
#[derive(Debug, Default)]
pub struct ColumnCache {
    identity: Option<(vel_buffer::BufferId, vel_buffer::ChangeId)>,
    start: usize,
    tab: usize,
    checkpoints: Vec<(usize, usize)>,
}
impl ColumnCache {
    /// Rebases cached prefix checkpoints without reading document contents. Edits
    /// invalidate only the suffix they touch, including a possible joining scalar.
    pub fn change(
        &mut self,
        before: (vel_buffer::BufferId, vel_buffer::ChangeId),
        after: (vel_buffer::BufferId, vel_buffer::ChangeId),
        changes: &vel_buffer::ChangeSet,
    ) {
        if self.identity != Some(before) {
            self.identity = None;
            return;
        }
        let edits = changes.changes();
        let at = edits.partition_point(|edit| edit.old_range.start < self.start);
        if at > 0 && edits[at - 1].old_range.end > self.start {
            self.identity = None;
            return;
        }
        if let Some(edit) = edits.get(at) {
            let relative = edit.old_range.start - self.start;
            let keep = self
                .checkpoints
                .partition_point(|&(byte, _)| byte < relative);
            self.checkpoints.truncate(keep.max(1));
        }
        self.start = changes.transform_point(self.start, vel_buffer::Bias::Left);
        self.identity = Some(after);
    }
    /// Queries a scalar column (CJK counts once); tabs always reach the next stop.
    /// At most one checkpoint interval is rescanned on an unchanged line/revision.
    pub fn column(
        &mut self,
        text: &vel_text::Text,
        identity: (vel_buffer::BufferId, vel_buffer::ChangeId),
        caret: usize,
        tab_size: usize,
        interval: usize,
    ) -> usize {
        // Build 4152 fixes status columns after changing tab width:
        // https://www.sublimetext.com/download
        let caret = text.floor_char_boundary(caret.min(text.len_bytes()));
        let start = text.line_to_byte(text.byte_to_line(caret));
        let tab = tab_size.max(1);
        if self.identity != Some(identity) || self.start != start || self.tab != tab {
            self.identity = Some(identity);
            self.start = start;
            self.tab = tab;
            self.checkpoints.clear();
            self.checkpoints.push((0, 0));
        }
        let relative = caret - start;
        let at = self
            .checkpoints
            .partition_point(|&(byte, _)| byte <= relative)
            .saturating_sub(1);
        let (mut byte, mut column) = self.checkpoints[at];
        let mut next = byte + interval.clamp(64, 4096);
        let append = at + 1 == self.checkpoints.len();
        for chunk in text.slice(start + byte..caret).chunks() {
            for ch in chunk.chars() {
                column += if ch == '\t' { tab - column % tab } else { 1 };
                byte += ch.len_utf8();
                if append && byte >= next {
                    self.checkpoints.push((byte, column));
                    next = byte + interval.clamp(64, 4096);
                }
            }
        }
        // The last caret is a useful checkpoint for repeated typing/movement.
        if append
            && self
                .checkpoints
                .last()
                .is_some_and(|&(last, _)| last < relative)
        {
            self.checkpoints.push((relative, column));
        }
        column + 1
    }
}
/// Status-only tab-expanded scalar column, including beyond the long-line threshold.
pub fn column(text: &vel_text::Text, caret: usize, tab_size: usize, prefix_limit: usize) -> usize {
    ColumnCache::default().column(
        text,
        (vel_buffer::BufferId(0), vel_buffer::ChangeId(0)),
        caret,
        tab_size,
        prefix_limit,
    )
}

/// Menu requested by one of the document-format status items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatItem {
    /// Reopen and save encodings.
    Encoding,
    /// The three line-ending conventions.
    LineEndings,
}
/// A native-UI-independent status-menu row, ready for Lane B's click adapter.
#[derive(Clone, Debug)]
pub struct FormatMenuItem {
    /// Independently authored caption.
    pub caption: String,
    /// Named command; absent for a submenu.
    pub command: Option<&'static str>,
    /// Public command arguments.
    pub args: serde_json::Value,
    /// Whether the row can run for this document.
    pub enabled: bool,
    /// Current format choice.
    pub checked: bool,
    /// Nested choices.
    pub children: Vec<FormatMenuItem>,
}
/// Builds the encoding popup without IO or parsing; captions come from the codec table.
pub fn encoding_menu(
    names: &[(&str, &str)],
    current: &str,
    reopen: bool,
    save: bool,
) -> Vec<FormatMenuItem> {
    // Status sections open format menus:
    // https://docs.sublimetext.io/guide/customization/menus.html#available-menus
    [
        ("Reopen with Encoding", "reopen", reopen),
        ("Save with Encoding", "save", save),
    ]
    .into_iter()
    .map(|(caption, command, enabled)| FormatMenuItem {
        caption: caption.into(),
        command: None,
        args: serde_json::json!({}),
        enabled,
        checked: false,
        children: names
            .iter()
            .map(|(name, _)| FormatMenuItem {
                caption: (*name).into(),
                command: Some(command),
                args: serde_json::json!({"encoding":name}),
                enabled,
                checked: *name == current,
                children: vec![],
            })
            .collect(),
    })
    .collect()
}
/// Builds the line-ending popup with public alias arguments and a single check.
pub fn line_ending_menu(current: &str) -> Vec<FormatMenuItem> {
    [("Unix", "unix"), ("Windows", "windows"), ("Mac OS 9", "cr")]
        .into_iter()
        .map(|(caption, value)| FormatMenuItem {
            caption: caption.into(),
            command: Some("set_line_ending"),
            args: serde_json::json!({"type":value}),
            enabled: true,
            checked: current == value,
            children: vec![],
        })
        .collect()
}
/// Describes selected Unicode scalar characters, omitting a single empty caret.
pub fn selection(regions: usize, characters: usize) -> String {
    let chars = format!(
        "{characters} character{} selected",
        if characters == 1 { "" } else { "s" }
    );
    match (regions, characters) {
        (0 | 1, 0) => String::new(),
        (1, _) => chars,
        (_, 0) => format!("{regions} selection regions"),
        _ => format!("{regions} selection regions · {chars}"),
    }
}
/// Formats the clickable indentation item.
pub fn indentation(spaces: bool, width: usize) -> String {
    format!("{}: {width}", if spaces { "Spaces" } else { "Tab Size" })
}
/// Completed Find in Files feedback, counting occurrences and files with matches.
pub fn search(matches: usize, files: usize) -> String {
    format!(
        "{matches} {} in {files} {}",
        if matches == 1 { "match" } else { "matches" },
        if files == 1 { "file" } else { "files" }
    )
}
/// Position, optional selection and optional format fields in their displayed order.
pub fn position(
    line: usize,
    column: usize,
    regions: usize,
    characters: usize,
    encoding: Option<&str>,
    endings: Option<&str>,
) -> String {
    position_mode(
        "compact", line, column, regions, characters, encoding, endings,
    )
}
/// Formats the documented enabled/compact/disabled position field without hiding other fields.
#[allow(clippy::too_many_arguments)]
pub fn position_mode(
    mode: &str,
    line: usize,
    column: usize,
    regions: usize,
    characters: usize,
    encoding: Option<&str>,
    endings: Option<&str>,
) -> String {
    // Name added in ST4: https://www.sublimetext.com/download (build 4121).
    // The public changelog gives no default; Velocity retains its compact formatting.
    let mut fields = match mode {
        "disabled" => vec![],
        "enabled" => vec![format!("Line {line}, Column {column}")],
        _ => vec![format!("Ln {line}, Col {column}")],
    };
    let selected = selection(regions, characters);
    if !selected.is_empty() {
        fields.push(selected);
    }
    fields.extend(encoding.into_iter().chain(endings).map(str::to_owned));
    fields.join(" · ")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_and_format_status_table() {
        for (regions, characters, expected) in [
            (1, 0, ""),
            (1, 1, "1 character selected"),
            (1, 2, "2 characters selected"),
            (2, 0, "2 selection regions"),
            (2, 1, "2 selection regions · 1 character selected"),
            (3, 8, "3 selection regions · 8 characters selected"),
        ] {
            assert_eq!(selection(regions, characters), expected);
        }
        assert_eq!(position(2, 3, 1, 0, None, None), "Ln 2, Col 3");
        assert_eq!(
            position(2, 3, 1, 1, Some("UTF-8"), Some("Unix")),
            "Ln 2, Col 3 · 1 character selected · UTF-8 · Unix"
        );
        assert_eq!(indentation(true, 4), "Spaces: 4");
        assert_eq!(indentation(false, 2), "Tab Size: 2");
        assert_eq!(search(1, 1), "1 match in 1 file");
        assert_eq!(search(2, 1), "2 matches in 1 file");
        assert_eq!(search(0, 0), "0 matches in 0 files");
    }
}
