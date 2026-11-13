//! UTF-8/grapheme-safe toy buffer and marked-text state. No platform or GPU calls.
use crate::lexer::{Token, lex};
use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cursor {
    pub line: usize,
    pub byte: usize,
}
#[derive(Clone, Copy, Debug)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
}
#[derive(Clone, Debug, Default)]
pub struct ImeState {
    pub enabled: bool,
    pub preedit: String,
    pub selection: Option<(usize, usize)>,
}
impl ImeState {
    pub fn composing(&self) -> bool {
        !self.preedit.is_empty()
    }
    pub fn clear(&mut self) {
        self.preedit.clear();
        self.selection = None;
    }
    pub fn update(&mut self, text: String, selection: Option<(usize, usize)>) {
        // winit offsets are UTF-8 byte offsets, not UTF-16 indices.
        self.selection =
            selection.map(|(a, b)| (floor_boundary(&text, a), floor_boundary(&text, b)));
        self.preedit = text;
    }
}
fn floor_boundary(s: &str, n: usize) -> usize {
    let mut n = n.min(s.len());
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    n
}
pub fn previous_grapheme(s: &str, byte: usize) -> usize {
    s[..byte]
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(i, _)| i)
}
pub fn next_grapheme(s: &str, byte: usize) -> usize {
    s[byte..]
        .graphemes(true)
        .next()
        .map_or(byte, |g| byte + g.len())
}
pub fn cells(s: &str) -> usize {
    let mut col = 0;
    for g in s.graphemes(true) {
        col += if g == "\t" {
            4 - col % 4
        } else {
            UnicodeWidthStr::width(g)
        };
    }
    col
}
pub fn byte_at_cell(s: &str, target: usize) -> usize {
    let mut col = 0;
    for (i, g) in s.grapheme_indices(true) {
        let w = if g == "\t" {
            4 - col % 4
        } else {
            UnicodeWidthStr::width(g)
        };
        if col + w > target {
            return i;
        }
        col += w;
    }
    s.len()
}

#[derive(Debug)]
pub struct Editor {
    pub lines: Vec<String>,
    pub cursor: Cursor,
    pub anchor: Option<Cursor>,
    pub ime: ImeState,
    pub source: String,
    pub bytes_loaded: usize,
    pub truncated: bool,
    tokens: HashMap<usize, Vec<Token>>,
    token_order: VecDeque<usize>,
    preferred_column: Option<usize>,
}
impl Editor {
    pub fn from_text(s: &str) -> Self {
        let lines = s
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l).to_owned())
            .collect::<Vec<_>>();
        let tokens = HashMap::new();
        Self {
            lines,
            cursor: Cursor::default(),
            anchor: None,
            ime: ImeState::default(),
            source: "in-memory".into(),
            bytes_loaded: s.len(),
            truncated: false,
            tokens,
            token_order: VecDeque::new(),
            preferred_column: None,
        }
    }
    pub fn load(file: Option<&Path>, limit: usize) -> Result<Self, String> {
        let default = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../spikes/data/rust-10mb.rs"
        ));
        let path = file.unwrap_or(default);
        let f = match File::open(path) {
            Ok(f) => f,
            Err(e) if file.is_none() && e.kind() == std::io::ErrorKind::NotFound => {
                let n = if limit == 0 { 5000 } else { limit };
                let mut editor=Self::from_text(&(0..n).map(|i|format!("pub fn worker_{i:05}(input: &str) -> usize {{ let count = 42; count + input.len() }} // generated row {i}")).collect::<Vec<_>>().join("\n"));
                editor.source = format!("generated:Rust:{n}lines");
                return Ok(editor);
            }
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let mut reader = BufReader::new(f);
        let mut lines = Vec::new();
        let mut bytes = 0;
        let mut ended_in_newline = false;
        while limit == 0 || lines.len() < limit {
            let mut s = String::new();
            let n = reader
                .read_line(&mut s)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            if n == 0 {
                if ended_in_newline {
                    lines.push(String::new());
                }
                break;
            }
            bytes += n;
            ended_in_newline = s.ends_with('\n');
            if ended_in_newline {
                s.pop();
                if s.ends_with('\r') {
                    s.pop();
                }
            }
            lines.push(s);
        }
        // fill_buf checks the cap without decoding or materializing the remaining file.
        let truncated = limit != 0
            && lines.len() == limit
            && !reader.fill_buf().map_err(|e| e.to_string())?.is_empty();
        if lines.is_empty() {
            lines.push(String::new());
        }
        let tokens = HashMap::new();
        Ok(Self {
            lines,
            cursor: Cursor::default(),
            anchor: None,
            ime: ImeState::default(),
            source: path.display().to_string(),
            bytes_loaded: bytes,
            truncated,
            tokens,
            token_order: VecDeque::new(),
            preferred_column: None,
        })
    }
    pub fn line_tokens(&mut self, row: usize) -> &[Token] {
        if !self.tokens.contains_key(&row) {
            if self.token_order.len() == 1024
                && let Some(old) = self.token_order.pop_front()
            {
                self.tokens.remove(&old);
            }
            self.tokens.insert(row, lex(&self.lines[row]));
            self.token_order.push_back(row);
        }
        &self.tokens[&row]
    }
    pub fn selection(&self) -> Option<(Cursor, Cursor)> {
        self.anchor.filter(|a| *a != self.cursor).map(|a| {
            if a < self.cursor {
                (a, self.cursor)
            } else {
                (self.cursor, a)
            }
        })
    }
    fn invalidate(&mut self, start: usize) {
        self.tokens.retain(|row, _| *row < start);
        self.token_order.retain(|row| *row < start);
        self.preferred_column = None;
    }
    pub fn delete_selection(&mut self) -> bool {
        let Some((a, b)) = self.selection() else {
            self.anchor = None;
            return false;
        };
        let suffix = self.lines[b.line][b.byte..].to_owned();
        self.lines[a.line].truncate(a.byte);
        self.lines[a.line].push_str(&suffix);
        if a.line != b.line {
            self.lines.drain(a.line + 1..=b.line);
        }
        self.cursor = a;
        self.anchor = None;
        self.invalidate(a.line);
        true
    }
    /// Shared editing path for real KeyboardInput, IME commit, and synthetic input.
    pub fn insert(&mut self, text: &str) {
        self.delete_selection();
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let row = self.cursor.line;
        let byte = self.cursor.byte;
        let tail = self.lines[row].split_off(byte);
        let mut parts = text.split('\n');
        self.lines[row].push_str(parts.next().unwrap());
        let mut inserted = 0;
        for part in parts {
            inserted += 1;
            self.lines.insert(row + inserted, part.to_owned());
        }
        self.cursor.line = row + inserted;
        self.cursor.byte = self.lines[self.cursor.line].len();
        self.lines[self.cursor.line].push_str(&tail);
        self.invalidate(row);
    }
    pub fn commit_ime(&mut self, text: &str) {
        self.ime.clear();
        self.insert(text);
    }
    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        let row = self.cursor.line;
        let byte = self.cursor.byte;
        if byte > 0 {
            let prev = previous_grapheme(&self.lines[row], byte);
            self.lines[row].replace_range(prev..byte, "");
            self.cursor.byte = prev;
            self.invalidate(row);
        } else if row > 0 {
            let tail = self.lines.remove(row);
            self.cursor.line -= 1;
            self.cursor.byte = self.lines[row - 1].len();
            self.lines[row - 1].push_str(&tail);
            self.invalidate(row - 1);
        }
    }
    pub fn move_cursor(&mut self, motion: Motion, select: bool) {
        if select {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else if let Some((a, b)) = self.selection() {
            self.anchor = None;
            self.cursor = if matches!(motion, Motion::Left | Motion::Up | Motion::Home) {
                a
            } else {
                b
            };
            self.preferred_column = None;
            return;
        } else {
            self.anchor = None;
        }
        let row = self.cursor.line;
        let byte = self.cursor.byte;
        let vertical = matches!(motion, Motion::Up | Motion::Down);
        let preferred = self
            .preferred_column
            .unwrap_or_else(|| cells(&self.lines[row][..byte]));
        match motion {
            Motion::Left if byte > 0 => {
                self.cursor.byte = previous_grapheme(&self.lines[row], byte)
            }
            Motion::Left if row > 0 => {
                self.cursor.line -= 1;
                self.cursor.byte = self.lines[row - 1].len();
            }
            Motion::Right if byte < self.lines[row].len() => {
                self.cursor.byte = next_grapheme(&self.lines[row], byte)
            }
            Motion::Right if row + 1 < self.lines.len() => {
                self.cursor.line += 1;
                self.cursor.byte = 0;
            }
            Motion::Up if row > 0 => {
                self.cursor.line -= 1;
                self.cursor.byte = byte_at_cell(&self.lines[row - 1], preferred);
            }
            Motion::Down if row + 1 < self.lines.len() => {
                self.cursor.line += 1;
                self.cursor.byte = byte_at_cell(&self.lines[row + 1], preferred);
            }
            Motion::Home => self.cursor.byte = 0,
            Motion::End => self.cursor.byte = self.lines[row].len(),
            _ => {}
        }
        self.preferred_column = if vertical { Some(preferred) } else { None };
    }
    pub fn place_cursor(&mut self, cursor: Cursor, select: bool) {
        let cursor = Cursor {
            line: cursor.line.min(self.lines.len() - 1),
            byte: floor_boundary(
                &self.lines[cursor.line.min(self.lines.len() - 1)],
                cursor.byte,
            ),
        };
        if select {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = cursor;
        self.preferred_column = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_edits_and_join_split() {
        let mut e = Editor::from_text("");
        e.insert("你e\u{301}🦀");
        e.backspace();
        assert_eq!(e.lines[0], "你e\u{301}");
        e.backspace();
        assert_eq!(e.lines[0], "你");
        e.insert("\n好");
        assert_eq!(e.cursor, Cursor { line: 1, byte: 3 });
        e.move_cursor(Motion::Home, false);
        e.backspace();
        assert_eq!(e.lines, vec!["你好"]);
        assert_eq!(e.cursor.byte, 3);
    }
    #[test]
    fn selection_across_lines_replaced_once() {
        let mut e = Editor::from_text("ab\n中文\nef");
        e.cursor = Cursor { line: 2, byte: 1 };
        e.anchor = Some(Cursor { line: 0, byte: 1 });
        e.insert("X\nY");
        assert_eq!(e.lines, vec!["aX", "Yf"]);
        assert_eq!(e.cursor, Cursor { line: 1, byte: 1 });
        assert!(e.anchor.is_none());
    }
    #[test]
    fn vertical_movement_retains_column_and_cjk_width() {
        let mut e = Editor::from_text("abcde\n你\n123456");
        e.move_cursor(Motion::End, false);
        e.move_cursor(Motion::Down, false);
        assert_eq!(e.cursor.byte, 3);
        e.move_cursor(Motion::Down, false);
        assert_eq!(e.cursor.byte, 5);
        assert_eq!(cells("a\t你🦀"), 8);
    }
    #[test]
    fn composition_is_transient_and_commit_clears_it() {
        let mut e = Editor::from_text("let ");
        e.move_cursor(Motion::End, false);
        e.ime.enabled = true;
        e.ime.update("nihao".into(), Some((5, 5)));
        e.ime.update("你好".into(), Some((3, 6)));
        assert_eq!(e.lines[0], "let ");
        e.commit_ime("你好");
        assert_eq!(e.lines[0], "let 你好");
        assert!(!e.ime.composing());
        e.ime.update("shi".into(), Some((3, 3)));
        e.ime.clear();
        assert_eq!(e.lines[0], "let 你好");
    }
    #[test]
    fn ime_ranges_are_bytes_and_are_safely_clamped() {
        let mut i = ImeState::default();
        i.update("你🦀".into(), Some((2, 99)));
        assert_eq!(i.selection, Some((0, 7)));
        i.update("".into(), None);
        assert!(!i.composing());
        assert_eq!(i.selection, None);
    }
    #[test]
    fn token_cache_stays_bounded_during_a_whole_file_sweep() {
        let mut e = Editor::from_text(&"let x = 42; // cached\n".repeat(3000));
        for row in 0..e.lines.len() {
            let _ = e.line_tokens(row);
            assert!(e.tokens.len() <= 1024);
            assert_eq!(e.tokens.len(), e.token_order.len());
        }
        e.insert("new\n");
        assert!(e.tokens.is_empty());
        assert!(e.token_order.is_empty());
    }
    #[test]
    fn token_cache_invalidation_after_multiline_edit() {
        let mut e = Editor::from_text("let a\n// b");
        assert_eq!(e.line_tokens(1)[0].kind, crate::lexer::Kind::Comment);
        e.insert("//\n");
        assert_eq!(e.line_tokens(0)[0].kind, crate::lexer::Kind::Comment);
        assert_eq!(e.lines.len(), 3);
        assert_eq!(e.line_tokens(2)[0].kind, crate::lexer::Kind::Comment);
    }
}
