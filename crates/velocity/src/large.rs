//! Per-view feature decisions and versioned worker-built horizontal indexes.
use std::{
    cell::RefCell,
    collections::BTreeMap,
    ops::Range,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
};
use vel_buffer::{BufferId, ChangeId, ChangeSet};
use vel_config::settings::Settings;
use vel_largefile::{DocumentMetrics, FeaturePolicy, LargeDocumentPolicy, LongLineIndex};
use vel_text::Text;

pub const LONG_LINE: usize = 16 * 1024;
// Dropping a stale receiver cannot interrupt LongLineIndex construction. Bound actual
// builders across generations/views, so typing before readiness cannot spawn a thread per key.
static INDEX_BUILDERS: AtomicUsize = AtomicUsize::new(0);
#[derive(Debug)]
struct IndexPermit;
impl IndexPermit {
    fn acquire() -> Option<Self> {
        INDEX_BUILDERS
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < 2).then_some(n + 1)
            })
            .ok()
            .map(|_| Self)
    }
}
impl Drop for IndexPermit {
    fn drop(&mut self) {
        INDEX_BUILDERS.fetch_sub(1, Ordering::Release);
    }
}

pub fn rules(settings: &Settings) -> LargeDocumentPolicy {
    let d = LargeDocumentPolicy::default();
    let n = |key, value| {
        settings
            .get(key)
            .and_then(vel_config::Value::as_u64)
            .unwrap_or(value)
    };
    LargeDocumentPolicy {
        viewport_bytes: n("large_file.viewport_bytes", d.viewport_bytes),
        viewport_lines: n("large_file.viewport_lines", d.viewport_lines),
        long_line_bytes: n("large_file.long_line_bytes", d.long_line_bytes),
        indexing_bytes: n("large_file.indexing_bytes", d.indexing_bytes),
        wrap_bytes: n("large_file.wrap_bytes", d.wrap_bytes),
        spell_bytes: n("large_file.spell_bytes", d.spell_bytes),
        diff_bytes: n("large_file.diff_bytes", d.diff_bytes),
        minimap_lines: n("large_file.minimap_lines", d.minimap_lines),
    }
}
/// Parsed once per settings identity; buffer transactions only compare numeric thresholds.
#[derive(Debug)]
pub struct FeatureConfig {
    rules: LargeDocumentPolicy,
    full: bool,
    indexing: bool,
    wrap: Option<bool>,
    enable_wrap: bool,
    spell: bool,
    diff: bool,
    minimap: bool,
}
impl FeatureConfig {
    pub fn new(settings: &Settings) -> Self {
        Self {
            rules: rules(settings),
            full: settings.bool("large_file.enable_highlighting", false),
            indexing: settings.bool("large_file.enable_indexing", false),
            wrap: settings
                .get("word_wrap")
                .and_then(vel_config::Value::as_bool),
            enable_wrap: settings.bool("large_file.enable_word_wrap", false),
            spell: settings.bool("large_file.enable_spell_check", false),
            diff: settings.bool("large_file.enable_diff", false),
            minimap: settings.bool("large_file.enable_minimap", false),
        }
    }
    pub fn evaluate(&self, metrics: DocumentMetrics) -> FeaturePolicy {
        let mut p = self.rules.evaluate(metrics);
        if self.full || self.indexing {
            p.highlighting = vel_largefile::HighlightMode::Full;
        }
        p.indexing |= self.indexing;
        // The thresholds govern automatic wrapping. An explicit word_wrap:true
        // (including View > Word Wrap) opts in even for multi-MiB lines. Layout
        // and painting use independent visual-row ranges once breaks are built.
        p.word_wrap = self.wrap.unwrap_or(p.word_wrap) || self.enable_wrap;
        p.spell_check |= self.spell;
        p.diff_against_disk |= self.diff;
        if self.minimap {
            p.minimap_stride = 1;
        }
        p
    }
}

pub fn metrics(text: &Text, caret: usize) -> DocumentMetrics {
    DocumentMetrics {
        bytes: text.len_bytes() as u64,
        lines: text.len_lines() as u64,
        longest_line_bytes: crate::layout::body_len(text, 0).max(crate::layout::body_len(
            text,
            text.byte_to_line(text.floor_char_boundary(caret.min(text.len_bytes()))),
        )) as u64,
    }
}
pub fn hint(p: FeaturePolicy) -> String {
    let mut disabled = Vec::new();
    if p.highlighting == vel_largefile::HighlightMode::Viewport {
        disabled.push("viewport syntax");
    }
    if !p.indexing {
        disabled.push("indexing off");
    }
    if !p.word_wrap {
        disabled.push("wrap off");
    }
    if !p.spell_check {
        disabled.push("spelling off");
    }
    if !p.diff_against_disk {
        disabled.push("disk diff off");
    }
    if p.minimap_stride > 1 {
        disabled.push("sampled minimap");
    }
    disabled.join(" · ")
}

type Identity = (BufferId, ChangeId);
#[derive(Debug)]
struct Pending {
    identity: Identity,
    row: usize,
    tab: usize,
    rx: mpsc::Receiver<LongLineIndex>,
}
#[derive(Debug, Default)]
struct Indexes {
    identity: Option<Identity>,
    tab: usize,
    ready: BTreeMap<usize, LongLineIndex>,
    pending: BTreeMap<usize, Pending>,
    waiting: bool,
}
/// Interior mutability permits nonblocking native geometry queries to collect finished indexes.
#[derive(Debug, Default)]
pub struct LongLines(RefCell<Indexes>);
impl LongLines {
    pub fn poll(&self) -> bool {
        let mut cache = self.0.borrow_mut();
        let mut changed = cache.waiting && INDEX_BUILDERS.load(Ordering::Acquire) < 2;
        if changed {
            cache.waiting = false;
        }
        let rows: Vec<_> = cache.pending.keys().copied().collect();
        for row in rows {
            let p = cache.pending.remove(&row).expect("pending index");
            match p.rx.try_recv() {
                Ok(index) if Some(p.identity) == cache.identity && p.tab == cache.tab => {
                    if cache.ready.len() >= 8
                        && let Some((_, old)) = cache.ready.pop_first()
                    {
                        crate::language::retire(old);
                    }
                    cache.ready.insert(row, index);
                    changed = true;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    cache.pending.insert(row, p);
                }
                _ => {}
            }
        }
        changed
    }
    pub fn pending(&self) -> bool {
        let cache = self.0.borrow();
        cache.waiting || !cache.pending.is_empty()
    }
    pub fn reset(&self) {
        *self.0.borrow_mut() = Indexes::default();
    }
    pub fn change(
        &self,
        old: &Text,
        after: &Text,
        before: Identity,
        identity: Identity,
        changes: &ChangeSet,
    ) {
        let mut cache = self.0.borrow_mut();
        if cache.ready.is_empty() && cache.pending.is_empty() {
            cache.identity = Some(identity);
            return;
        }
        if cache.identity != Some(before) {
            cache.ready.clear();
        }
        cache.pending.clear();
        cache.identity = Some(identity);
        let [change] = changes.changes() else {
            cache.ready.clear();
            return;
        };
        let row = old.byte_to_line(change.old_range.start);
        let start = old.line_to_byte(row);
        let local = change.old_range.start - start..change.old_range.end - start;
        let at = changes.transform_point(change.old_range.start, vel_buffer::Bias::Left);
        if old.byte_to_line(change.old_range.end) != row
            || after.byte_to_line(at) != row
            || after.byte_to_line(at + change.new_len) != row
            || old.len_lines() != after.len_lines()
        {
            cache.ready.clear();
            return;
        }
        cache.ready.retain(|&r, _| r == row);
        if let Some(index) = cache.ready.get_mut(&row) {
            index.update_after_edit(after.clone(), local, change.new_len);
        }
    }
    pub fn query<T>(
        &self,
        text: &Text,
        identity: Identity,
        row: usize,
        tab: usize,
        fallback_byte: usize,
        f: impl FnOnce(&LongLineIndex, usize, usize) -> T,
    ) -> T {
        let mut cache = self.0.borrow_mut();
        if cache.identity != Some(identity) || cache.tab != tab {
            cache.ready.clear();
            cache.pending.clear();
            cache.identity = Some(identity);
            cache.tab = tab;
        }
        if let Some(p) = cache.pending.remove(&row) {
            match p.rx.try_recv() {
                Ok(index) if p.identity == identity && p.tab == tab && p.row == row => {
                    if cache.ready.len() >= 8
                        && let Some((_, old)) = cache.ready.pop_first()
                    {
                        crate::language::retire(old);
                    }
                    cache.ready.insert(row, index);
                }
                Err(mpsc::TryRecvError::Empty) => {
                    cache.pending.insert(row, p);
                }
                _ => {}
            }
        }
        if let Some(index) = cache.ready.get(&row) {
            return f(index, 0, 0);
        }
        if !cache.pending.contains_key(&row) && cache.pending.len() < 4 {
            if let Some(permit) = IndexPermit::acquire() {
                cache.waiting = false;
                let (tx, rx) = mpsc::channel();
                let snapshot = text.clone();
                if std::thread::Builder::new()
                    .name("velocity-long-line".into())
                    .spawn(move || {
                        let _permit = permit;
                        let _ = tx.send(LongLineIndex::new(snapshot, row, tab));
                    })
                    .is_ok()
                {
                    cache.pending.insert(
                        row,
                        Pending {
                            identity,
                            row,
                            tab,
                            rx,
                        },
                    );
                }
            } else {
                cache.waiting = true;
            }
        }
        // Bounded provisional geometry while the full index is being built. Exact columns
        // replace this estimate when the version-matching worker result is collected.
        let start = text.line_to_byte(row);
        let body = crate::layout::body_len(text, row);
        let local = fallback_byte.min(body);
        let a = text.floor_char_boundary(start + local.saturating_sub(4096)) - start;
        let b = text.floor_char_boundary(start + local.saturating_add(8192).min(body)) - start;
        let preview = text.slice(start + a..start + b).to_string();
        let index = LongLineIndex::new(Text::from(preview.as_str()), 0, tab);
        f(&index, a, a)
    }
    pub fn ready(&self, row: usize) -> bool {
        self.0.borrow().ready.contains_key(&row)
    }
}

/// A bounded visible slice. Byte and column origins refer to the complete physical line.
#[derive(Debug)]
pub struct Window {
    pub text: String,
    pub bytes: Range<usize>,
    pub column: usize,
}

impl crate::editor::Editor {
    pub fn large_command(&mut self, name: &str, args: &vel_config::Value) -> Result<bool, String> {
        use vel_buffer::{EditKind, Region};
        if name == "large_file_enable_feature" {
            let feature = args["feature"].as_str().ok_or("feature required")?;
            if ![
                "highlighting",
                "indexing",
                "word_wrap",
                "spell_check",
                "diff",
                "minimap",
            ]
            .contains(&feature)
            {
                return Err("Unknown large-file feature".into());
            }
            self.settings.set(
                vel_config::settings::Layer::View,
                &format!("large_file.enable_{feature}"),
                vel_config::json!(true),
            );
            if feature == "word_wrap" {
                self.settings.set(
                    vel_config::settings::Layer::View,
                    "word_wrap",
                    vel_config::json!(true),
                );
            }
            if feature == "minimap" {
                self.settings.set(
                    vel_config::settings::Layer::View,
                    "show_minimap",
                    vel_config::json!(true),
                );
            }
            if feature == "spell_check" {
                self.settings.set(
                    vel_config::settings::Layer::View,
                    "spell_check",
                    vel_config::json!(true),
                );
            }
            self.sync();
            return Ok(true);
        }
        if let Some(snapshot) = self.binary.clone() {
            let len = snapshot.view.len_bytes() as usize;
            let mut to = self.caret;
            match name {
                "goto_hex_offset" => {
                    to = crate::binary::offset(
                        args["text"]
                            .as_str()
                            .or(args["offset"].as_str())
                            .unwrap_or("0"),
                    )?;
                }
                "copy" | "copy_hex" | "copy_hex_text" => {
                    let range = if self.anchor == self.caret {
                        self.caret..(self.caret + 1).min(len)
                    } else {
                        self.selection()
                    };
                    if range.len()
                        > self
                            .settings
                            .usize("large_file.copy_bytes", 16 * 1024 * 1024)
                    {
                        return Err("Byte selection exceeds the large_file.copy_bytes limit; select a smaller range".into());
                    }
                    self.clipboard = crate::binary::copy(&snapshot, range, name != "copy_hex_text");
                    return Ok(true);
                }
                "select_all" => {
                    self.anchor = 0;
                    self.caret = len;
                    return Ok(true);
                }
                "move" => {
                    let step = match args["by"].as_str().unwrap_or("characters") {
                        "lines" => 16,
                        "pages" => 16 * self.layout.borrow().page.max(1),
                        _ => 1,
                    };
                    to = if args["forward"].as_bool().unwrap_or(true) {
                        to.saturating_add(step)
                    } else {
                        to.saturating_sub(step)
                    };
                }
                "move_to" => {
                    to = match args["to"].as_str().unwrap_or("bof") {
                        "eof" => len,
                        "eol" => (to / 16 * 16 + 16).min(len),
                        "bol" => to / 16 * 16,
                        _ => 0,
                    };
                }
                _ => return Ok(self.registry.kind(name) == Some(vel_commands::Kind::TextCommand)),
            }
            self.set_caret(to, args["extend"].as_bool().unwrap_or(false));
            return Ok(true);
        }
        if self.layout.borrow().total() == 0 {
            self.layout.borrow_mut().configure(
                &self.text,
                (self.buffer.id(), self.buffer.change_id()),
                crate::layout::Options {
                    width: 0,
                    tab: self.preferences.tab_size,
                    indent: false,
                },
                &[],
            );
        }
        let row = self.text.byte_to_line(self.caret);
        if self.text.line_len_bytes(row) < LONG_LINE {
            return Ok(false);
        }
        let forward = args["forward"].as_bool().unwrap_or(true);
        let extend = args["extend"].as_bool().unwrap_or(false);
        if name == "move"
            && matches!(
                args["by"].as_str(),
                Some("characters" | "words" | "word_ends" | "subwords" | "subword_ends")
            )
        {
            let word = args["by"] != "characters";
            let old = self.buffer.view(self.id).sel();
            let after = crate::editing::selection_primary(
                old.iter()
                    .map(|r| {
                        let to = if !extend && !r.empty() {
                            if forward { r.end() } else { r.begin() }
                        } else {
                            self.layout
                                .borrow()
                                .long_motion(&self.text, r.b, forward, word)
                        };
                        Region::new(if extend { r.a } else { to }, to)
                    })
                    .collect(),
                old.primary_index(),
            );
            self.set_selection(after);
            return Ok(true);
        }
        if name == "move_to" && matches!(args["to"].as_str(), Some("eol" | "bol" | "eof" | "bof")) {
            let old = self.buffer.view(self.id).sel();
            let after = crate::editing::selection_primary(
                old.iter()
                    .map(|r| {
                        let row = self.text.byte_to_line(r.b);
                        let to = match args["to"].as_str() {
                            Some("eol") => {
                                self.text.line_to_byte(row)
                                    + crate::layout::body_len(&self.text, row)
                            }
                            Some("bol") => self.text.line_to_byte(row),
                            Some("eof") => self.text.len_bytes(),
                            _ => 0,
                        };
                        Region::new(if extend { r.a } else { to }, to)
                    })
                    .collect(),
                old.primary_index(),
            );
            self.set_selection(after);
            return Ok(true);
        }
        if name == "insert_best_completion" && !self.read_only && !self.features.indexing {
            let tab = self.preferences.tab_size.max(1);
            let edits = self
                .buffer
                .view(self.id)
                .sel()
                .iter()
                .map(|r| {
                    let column = self.layout.borrow().column(&self.text, r.begin());
                    let text = if self.preferences.translate_tabs_to_spaces {
                        " ".repeat(tab - column % tab)
                    } else {
                        "\t".into()
                    };
                    (r.to_range(), text)
                })
                .collect();
            self.apply_edits(edits, None, EditKind::InsertChars);
            self.sync();
            return Ok(true);
        }
        if matches!(name, "left_delete" | "right_delete") && !self.read_only {
            let edits = self
                .buffer
                .view(self.id)
                .sel()
                .iter()
                .map(|r| {
                    let range = if !r.empty() {
                        r.to_range()
                    } else {
                        let to = self.layout.borrow().long_motion(
                            &self.text,
                            r.b,
                            name == "right_delete",
                            false,
                        );
                        r.b.min(to)..r.b.max(to)
                    };
                    (range, String::new())
                })
                .collect();
            self.apply_edits(edits, None, EditKind::Delete);
            self.sync();
            return Ok(true);
        }
        Ok(false)
    }
}

pub fn first_line(text: &Text) -> String {
    let end = text.floor_char_boundary(text.line_len_bytes(0).min(8192));
    text.slice(0..end).to_string()
}
