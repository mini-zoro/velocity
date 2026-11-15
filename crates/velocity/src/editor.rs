//! Buffer-backed editing, command routing and transient IME preview.
use std::{
    cell::RefCell,
    ops::Range,
    sync::{Arc, mpsc},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
use vel_buffer::{Buffer, Change, ChangeSet, Region, Selection, ViewId, ops};
use vel_commands::{Host, Registry};
use vel_config::{
    Value, json,
    keymap::{ContextProvider, Key, Keymap, PlainScope, Resolution, ScopeProvider},
    settings::{EditorSettings, Layer, Settings},
};
use vel_platform::{Command, TextInput, Utf16Range};
use vel_text::Text;

pub type ReindentResult = (
    vel_buffer::BufferId,
    vel_buffer::ChangeId,
    Result<Vec<(Range<usize>, String)>, String>,
);
#[derive(Debug)]
struct Composition {
    original: Range<usize>,
    contents: String,
    selection: Selection,
    explicit: bool,
}
#[derive(Clone, Debug)]
pub struct SharedLayoutEdit {
    pub buffer: vel_buffer::BufferId,
    pub from: vel_buffer::ChangeId,
    pub to: vel_buffer::ChangeId,
    pub before: Text,
    pub after: Text,
    pub changes: ChangeSet,
}
#[derive(Debug)]
pub struct Editor {
    pub theme: std::rc::Rc<RefCell<vel_ui::theme::ThemeUi>>,
    pub html: RefCell<crate::html_ui::ViewHtml>,
    pub annotation_view: ViewId,
    pub annotations: std::rc::Rc<RefCell<crate::annotations::Annotations>>,
    pub completion: crate::completion::Completion,
    pub fields: Option<vel_snippet::Session>,
    pub fields_version: Option<(vel_buffer::BufferId, vel_buffer::ChangeId)>,
    pub snippets: Arc<std::collections::BTreeMap<String, vel_snippet::SnippetFile>>,
    pub snippet_path: Option<std::path::PathBuf>,
    pub language: crate::language::LanguageCell,
    pub layout: RefCell<crate::layout::Layout>,
    language_buffer: Option<vel_buffer::BufferId>,
    pub reindent_job: Option<mpsc::Receiver<ReindentResult>>,
    pub read_only: bool,
    pub buffer: Buffer,
    pub shared_shadow: bool,
    pub layout_edits: std::rc::Rc<RefCell<std::collections::VecDeque<SharedLayoutEdit>>>,
    pub layout_version: vel_buffer::ChangeId,
    pub id: ViewId,
    // O(1) snapshot used by layout and native queries, including preedit preview.
    pub text: Text,
    pub anchor: usize,
    pub caret: usize,
    pub marked: Option<Range<usize>>,
    composition: Option<Composition>,
    pub settings: Settings,
    pub preferences: EditorSettings,
    settings_revision: u64,
    pub registry: Registry,
    pub keymap: Keymap,
    pub clipboard: String,
    pub actions: Vec<(String, Value)>,
    pub revision: u64,
}
impl Editor {
    /// Namespaces paint state by view, including cloned views of a shared buffer.
    pub fn ui_id(&self, local: u32) -> u64 {
        (self.annotation_view.0 << 32) | u64::from(local)
    }

    pub fn new(text: Text) -> Self {
        let mut buffer = Buffer::new(text.clone());
        let id = buffer.new_view();
        let mut settings = Settings::default();
        settings
            .replace(
                Layer::Default,
                serde_json::to_value(EditorSettings::default()).expect("settings"),
            )
            .expect("settings");
        static DEFAULT_KEYMAP: std::sync::OnceLock<Keymap> = std::sync::OnceLock::new();
        let keymap = DEFAULT_KEYMAP
            .get_or_init(|| {
                let mut keymap = Keymap::default();
                keymap
                    .append(include_str!(
                        "../resources/Packages/Default/Velocity.sublime-keymap"
                    ))
                    .expect("our keymap");
                keymap
            })
            .clone();
        let preferences = settings.editor();
        let settings_revision = settings.revision();
        let mut registry = Registry::standard();
        for name in crate::html_ui::COMMANDS {
            registry.register(name, vel_commands::Kind::TextCommand, None);
        }
        for name in [
            "new_html_sheet",
            "open_url",
            "minihtml_navigate",
            "theme_demo",
            "set_theme_appearance",
        ] {
            registry.register(name, vel_commands::Kind::WindowCommand, None);
        }
        Self {
            theme: crate::theme_ui::default_ui(),
            html: Default::default(),
            annotation_view: id,
            annotations: std::rc::Rc::new(RefCell::new(crate::annotations::Annotations::new(
                text.clone(),
            ))),
            completion: Default::default(),
            fields: None,
            fields_version: None,
            snippets: Default::default(),
            snippet_path: None,
            shared_shadow: false,
            layout_edits: std::rc::Rc::new(RefCell::new(std::collections::VecDeque::new())),
            layout_version: vel_buffer::ChangeId(0),
            language: std::rc::Rc::new(RefCell::new(None)),
            layout: RefCell::new(crate::layout::Layout::default()),
            language_buffer: None,
            reindent_job: None,
            read_only: false,
            buffer,
            id,
            text,
            preferences,
            settings_revision,
            anchor: 0,
            caret: 0,
            marked: None,
            composition: None,
            settings,
            registry,
            keymap,
            clipboard: String::new(),
            actions: Vec::new(),
            revision: 0,
        }
    }
    pub fn adopt_document(&mut self) {
        self.language_buffer = Some(self.buffer.id());
        let current = &self.buffer.view(self.id).settings;
        if current.tab_size != self.preferences.tab_size
            || current.translate_tabs_to_spaces != self.preferences.translate_tabs_to_spaces
            || current.word_separators != self.preferences.word_separators
        {
            self.buffer.set_settings(
                self.id,
                vel_buffer::Settings {
                    tab_size: self.preferences.tab_size,
                    translate_tabs_to_spaces: self.preferences.translate_tabs_to_spaces,
                    word_separators: self.preferences.word_separators.clone(),
                },
            );
            self.buffer.break_undo_group();
        }
    }
    pub fn sync(&mut self) {
        if self.fields.is_some()
            && !self.shared_shadow
            && self.fields_version != Some((self.buffer.id(), self.buffer.change_id()))
        {
            self.fields = None;
            self.fields_version = None;
        }
        if self.settings.get("velocity_html_sheet").is_some() {
            self.read_only = true;
        }
        if self.settings_revision != self.settings.revision() {
            self.preferences = self.settings.editor();
            self.settings_revision = self.settings.revision();
            let current = &self.buffer.view(self.id).settings;
            if current.tab_size != self.preferences.tab_size
                || current.translate_tabs_to_spaces != self.preferences.translate_tabs_to_spaces
                || current.word_separators != self.preferences.word_separators
            {
                self.buffer.set_settings(
                    self.id,
                    vel_buffer::Settings {
                        tab_size: self.preferences.tab_size,
                        translate_tabs_to_spaces: self.preferences.translate_tabs_to_spaces,
                        word_separators: self.preferences.word_separators.clone(),
                    },
                );
            }
        }
        if !self.shared_shadow && self.layout_version != self.buffer.change_id() {
            let edits = self.layout_edits.borrow();
            for edit in edits.iter().filter(|e| e.buffer == self.buffer.id()) {
                if edit.from == self.layout_version {
                    self.layout
                        .borrow_mut()
                        .prepare_change((edit.buffer, edit.from));
                    self.layout.borrow_mut().change(
                        &edit.before,
                        &edit.after,
                        (edit.buffer, edit.to),
                        &edit.changes,
                    );
                    self.layout_version = edit.to;
                }
            }
            if self.layout_version != self.buffer.change_id() {
                self.layout.borrow_mut().invalidate();
                self.layout_version = self.buffer.change_id();
            }
        }
        let reset = !self.shared_shadow
            && self.language.borrow().as_ref().is_some_and(|l| {
                l.version != self.buffer.change_id()
                    || self.language_buffer != Some(self.buffer.id())
            });
        if reset {
            self.fields = None;
            self.completion.hide();
            let previous = self.language.borrow_mut().take().expect("language");
            let result = crate::language::Language::new(
                previous.assets.clone(),
                &previous.syntax,
                self.buffer.text().clone(),
                self.buffer.change_id(),
                previous.scheme.clone(),
                self.preferences.tab_size,
            );
            match result {
                Ok(language) => *self.language.borrow_mut() = Some(language),
                Err(e) => self
                    .actions
                    .push(("language_error".into(), json!({"message": e}))),
            }
            self.language_buffer = Some(self.buffer.id());
            self.layout.borrow_mut().invalidate();
        }
        if !self.shared_shadow {
            self.annotations
                .borrow_mut()
                .committed(&self.buffer, &self.settings);
        }
        self.text = self.buffer.text().clone();
        let primary = self
            .buffer
            .view(self.id)
            .sel()
            .primary()
            .expect("selection");
        self.anchor = primary.a;
        self.caret = primary.b;
        self.revision = self.buffer.change_id().0;
        if self.fields.as_ref().is_some_and(|s| {
            if self.shared_shadow {
                return false;
            }
            let fields = s.selections();
            self.buffer.view(self.id).sel().iter().any(|r| {
                !fields
                    .iter()
                    .any(|f| f.start <= r.begin() && r.end() <= f.end)
            })
        }) {
            self.fields = None;
        }
    }
    pub fn attach_language(
        &mut self,
        assets: Arc<crate::language::Assets>,
        syntax: &str,
        scheme: Arc<vel_color_scheme::ColorScheme>,
    ) -> Result<(), String> {
        self.completion.hide();
        let language = crate::language::Language::new(
            assets,
            syntax,
            self.buffer.text().clone(),
            self.buffer.change_id(),
            scheme,
            self.preferences.tab_size,
        )?;
        *self.language.borrow_mut() = Some(language);
        self.language_buffer = Some(self.buffer.id());
        Ok(())
    }
    pub fn poll_language(&mut self) -> bool {
        let mut changed = self
            .language
            .borrow_mut()
            .as_mut()
            .is_some_and(|l| l.poll());
        if let Some((buffer, version, edits)) =
            self.reindent_job.as_ref().and_then(|rx| rx.try_recv().ok())
        {
            self.reindent_job = None;
            if buffer == self.buffer.id()
                && version == self.buffer.change_id()
                && !self.read_only
                && self.marked.is_none()
            {
                match edits {
                    Ok(edits) => {
                        self.apply_edits(edits, None, vel_buffer::EditKind::Other);
                        self.sync();
                        changed = true;
                    }
                    Err(error) => {
                        if let Some(language) = self.language.borrow_mut().as_mut() {
                            language.errors.push(error);
                        }
                    }
                }
            }
        }
        changed
    }
    pub fn regions(&self) -> Vec<Region> {
        if let Some(composition) = &self.composition {
            let changes = ChangeSet::new(vec![Change {
                old_range: composition.original.clone(),
                new_len: composition.contents.len(),
            }]);
            self.buffer
                .view(self.id)
                .sel()
                .iter()
                .map(|r| {
                    if r == composition.selection.primary().expect("primary") {
                        Region::new(self.anchor, self.caret)
                    } else {
                        changes.transform_region(r)
                    }
                })
                .collect()
        } else {
            self.buffer.view(self.id).sel().iter().collect()
        }
    }
    pub fn set_selection(&mut self, selection: Selection) {
        self.unmark();
        if self.buffer.view(self.id).sel() != &selection {
            self.completion.hide();
            self.html.borrow_mut().selection_modified();
        }
        if !self.shared_shadow && self.buffer.view(self.id).sel() != &selection {
            self.buffer.break_undo_group();
        }
        self.buffer.set_selection(self.id, selection);
        self.sync();
    }
    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }
    pub fn selected_utf16(&self) -> Utf16Range {
        Utf16Range::from_bytes(&self.text, self.selection())
    }
    pub fn marked_utf16(&self) -> Option<Utf16Range> {
        self.marked
            .clone()
            .map(|r| Utf16Range::from_bytes(&self.text, r))
    }
    pub fn run(&mut self, name: &str, args: &Value) -> Result<(), String> {
        if self.read_only
            && self.registry.kind(name) == Some(vel_commands::Kind::TextCommand)
            && !crate::html_ui::COMMANDS.contains(&name)
        {
            return Ok(());
        }
        self.unmark();
        self.sync();
        let before_version = self.buffer.change_id();
        let before_selection = (
            self.anchor,
            self.caret,
            self.buffer.view(self.id).sel().len(),
        );
        if matches!(
            name,
            "undo" | "redo" | "redo_or_repeat" | "soft_undo" | "soft_redo"
        ) {
            self.fields = None;
            self.completion.hide();
        }
        let mut registry = std::mem::take(&mut self.registry);
        let result = registry.run(name, args, self);
        self.registry = registry;
        self.sync();
        if self.buffer.change_id() == before_version
            && before_selection
                != (
                    self.anchor,
                    self.caret,
                    self.buffer.view(self.id).sel().len(),
                )
        {
            self.completion.hide();
            self.html.borrow_mut().selection_modified();
        }
        if before_selection
            != (
                self.anchor,
                self.caret,
                self.buffer.view(self.id).sel().len(),
            )
        {
            self.html.borrow_mut().selection_modified();
        }
        if self.buffer.change_id() != before_version
            && !matches!(
                name,
                "commit_completion"
                    | "replace_completion_with_next_completion"
                    | "insert_best_completion"
                    | "insert_snippet"
            )
        {
            let character = if name == "insert" {
                args["characters"].as_str().and_then(|s| s.chars().last())
            } else {
                None
            };
            self.html.borrow_mut().character_event();
            self.completion.edited(character);
        }
        result
    }
    pub fn context_matches(&self, context: &[vel_config::keymap::Context]) -> bool {
        context.iter().all(|c| c.matches(&EditorContext(self)))
    }
    pub fn resolve_key(&mut self, key: &Key) -> Resolution {
        if self.marked.is_some() {
            return Resolution::Unbound;
        }
        let mut keymap = std::mem::take(&mut self.keymap);
        let resolution = keymap.resolve(key, &EditorContext(self));
        self.keymap = keymap;
        resolution
    }
    pub fn key(&mut self, key: &Key) -> Result<bool, String> {
        match self.resolve_key(key) {
            Resolution::Unbound => Ok(false),
            Resolution::Pending => Ok(true),
            Resolution::Command(invocation) => {
                self.run(&invocation.command, &invocation.args)?;
                Ok(true)
            }
        }
    }
    pub fn text_input(&mut self, input: TextInput) {
        match input {
            TextInput::Insert { text, replacement } => {
                if let Some(composition) = self.composition.take() {
                    let range = replacement.map(|r| r.bytes(&self.text));
                    let mapped = range.map(|r| self.map_preview_range(r, &composition));
                    self.marked = None;
                    self.sync();
                    if let Some(range) = mapped {
                        self.buffer
                            .set_selection(self.id, Selection::from_regions([Region::from(range)]));
                    } else if composition.explicit {
                        self.buffer.set_selection(
                            self.id,
                            Selection::from_regions([Region::from(composition.original)]),
                        );
                    } else {
                        self.buffer.set_selection(self.id, composition.selection);
                    }
                } else if let Some(replacement) = replacement {
                    let range = replacement.bytes(self.buffer.text());
                    self.buffer
                        .set_selection(self.id, Selection::from_regions([Region::from(range)]));
                }
                let _ = self.run("insert", &json!({"characters":text}));
            }
            TextInput::Marked {
                text,
                selected,
                replacement,
            } => {
                self.completion.hide();
                if text.is_empty() {
                    if self.composition.is_some() {
                        self.cancel_composition();
                    } else if replacement.is_some() {
                        self.text_input(TextInput::Insert { text, replacement });
                    }
                    return;
                }
                let original = if let Some(composition) = &self.composition {
                    replacement
                        .map(|r| self.map_preview_range(r.bytes(&self.text), composition))
                        .unwrap_or_else(|| composition.original.clone())
                } else {
                    replacement
                        .map(|r| r.bytes(&self.text))
                        .unwrap_or_else(|| self.selection())
                };
                let selection = self
                    .composition
                    .as_ref()
                    .map(|c| c.selection.clone())
                    .unwrap_or_else(|| self.buffer.view(self.id).sel().clone());
                let explicit =
                    replacement.is_some() || self.composition.as_ref().is_some_and(|c| c.explicit);
                self.text = self.buffer.text().clone();
                self.text.replace(original.clone(), &text);
                let relative = selected.bytes(&Text::from(text.as_str()));
                self.anchor = original.start + relative.start;
                self.caret = original.start + relative.end;
                self.marked = Some(original.start..original.start + text.len());
                self.composition = Some(Composition {
                    original,
                    contents: text,
                    selection,
                    explicit,
                });
                self.revision += 1;
            }
            TextInput::Unmark => self.unmark(),
        }
    }
    fn map_preview_range(&self, range: Range<usize>, composition: &Composition) -> Range<usize> {
        let start = composition.original.start;
        let end = start + composition.contents.len();
        let delta = composition.original.len() as isize - composition.contents.len() as isize;
        let begin = if range.start < start {
            range.start
        } else if range.start < end {
            start
        } else {
            range.start.wrapping_add_signed(delta)
        };
        let finish = if range.end <= start {
            range.end
        } else if range.end <= end {
            composition.original.end
        } else {
            range.end.wrapping_add_signed(delta)
        };
        begin..finish
    }
    pub fn unmark(&mut self) {
        if let Some(composition) = self.composition.take() {
            self.marked = None;
            self.sync();
            if composition.explicit {
                self.buffer.set_selection(
                    self.id,
                    Selection::from_regions([Region::from(composition.original)]),
                );
            } else {
                self.buffer.set_selection(self.id, composition.selection);
            }
            let _ = self.run("insert", &json!({"characters":composition.contents}));
        }
    }
    pub fn cancel_composition(&mut self) {
        self.composition = None;
        self.marked = None;
        self.sync();
    }
    pub fn set_caret(&mut self, at: usize, extend: bool) {
        self.unmark();
        let at = self.text.floor_char_boundary(at);
        self.set_selection(Selection::from_regions([Region::new(
            if extend { self.anchor } else { at },
            at,
        )]));
    }
    pub fn add_caret(&mut self, at: usize) {
        self.unmark();
        let mut selection = self.buffer.view(self.id).sel().clone();
        selection.add(Region::caret(self.text.floor_char_boundary(at)));
        self.set_selection(selection);
    }
    pub fn select_word(&mut self, at: usize) {
        self.set_caret(at, false);
        ops::expand_selection(&mut self.buffer, self.id, ops::ExpandTo::Word);
        self.sync();
    }
    pub fn select_line(&mut self, at: usize) {
        self.set_caret(at, false);
        ops::expand_selection(&mut self.buffer, self.id, ops::ExpandTo::Line);
        self.sync();
    }
    pub fn command(&mut self, command: Command) {
        if command == Command::Cancel {
            if self.html.borrow().popup.as_ref().is_some_and(|p| p.visible) {
                self.html.borrow_mut().popup = None;
                return;
            }
            if self.completion.visible() {
                self.completion.hide();
                return;
            }
            self.fields = None;
            self.cancel_composition();
            self.keymap.cancel();
            return;
        }
        let (name, args) = selector_command(command);
        let _ = self.run(name, &args);
    }
}
impl Host for Editor {
    fn intercept_text_command(&mut self, name: &str, args: &Value) -> Result<bool, String> {
        if self.html_command(name, args)? {
            return Ok(true);
        }
        if self.annotation_command(name, args)? {
            return Ok(true);
        }
        if self.intelligence_command(name, args)? {
            return Ok(true);
        }
        self.language_command(name, args)
    }
    fn command_finished(&mut self) {
        self.sync();
    }
    fn buffer(&mut self) -> (&mut Buffer, ViewId) {
        (&mut self.buffer, self.id)
    }
    fn settings(&self) -> EditorSettings {
        self.preferences.clone()
    }
    fn clipboard(&mut self) -> String {
        self.clipboard.clone()
    }
    fn set_clipboard(&mut self, text: &str) {
        self.clipboard = text.into();
    }
    fn action(&mut self, name: &str, args: &Value) -> Result<(), String> {
        self.actions.push((name.into(), args.clone()));
        Ok(())
    }
}
struct EditorContext<'a>(&'a Editor);
impl ContextProvider for EditorContext<'_> {
    fn scopes_ready(&self) -> bool {
        if self.0.marked.is_some() {
            return false;
        }
        self.0.language.borrow().as_ref().is_none_or(|l| {
            self.0
                .buffer
                .view(self.0.id)
                .sel()
                .iter()
                .all(|r| l.scopes_ready_at(&self.0.text, r.b))
        })
    }
    fn values(&self, key: &str) -> Vec<Value> {
        let e = self.0;
        let selections = e.buffer.view(e.id).sel();
        if let Some(key) = key.strip_prefix("setting.") {
            return vec![e.settings.get(key).cloned().unwrap_or(Value::Null)];
        }
        match key {
            "selection_empty" => selections.iter().map(|r| json!(r.empty())).collect(),
            "num_selections" => vec![json!(selections.len())],
            "preceding_text" | "following_text" | "text" => selections
                .iter()
                .map(|r| {
                    let before_row = e.text.byte_to_line(r.begin());
                    let after_row = e.text.byte_to_line(r.end());
                    let start = e.text.line_to_byte(before_row);
                    let end = e.text.line_to_byte(after_row) + e.text.line(after_row).len_bytes();
                    json!(
                        e.text
                            .slice(match key {
                                "preceding_text" => start..r.begin(),
                                "following_text" => r.end()..end,
                                _ => r.to_range(),
                            })
                            .to_string()
                    )
                })
                .collect(),
            "has_prev_field" => vec![json!(e.field_previous())],
            "has_next_field" => vec![json!(e.fields.as_ref().is_some_and(|s| s.has_next_field()))],
            "auto_complete_visible" => vec![json!(e.completion.visible())],
            "overlay_visible" | "panel_visible" => vec![json!(false)],
            "is_recording_macro" => vec![json!(e.registry.is_recording())],
            _ => vec![],
        }
    }
    fn selector_matches(&self, selector: &str) -> Vec<bool> {
        self.0
            .buffer
            .view(self.0.id)
            .sel()
            .iter()
            .map(|r| {
                self.0.language.borrow().as_ref().map_or_else(
                    || PlainScope.matches(selector, r.b),
                    |l| l.selector(&self.0.text, selector, r.b),
                )
            })
            .collect()
    }
}
pub fn selector_command(command: Command) -> (&'static str, Value) {
    use Command::*;
    match command {
        DeleteBackward => ("left_delete", json!({})),
        DeleteForward => ("right_delete", json!({})),
        Newline => ("insert", json!({"characters":"\n"})),
        Tab => ("insert_best_completion", json!({})),
        Left(extend) | Right(extend) => (
            "move",
            json!({"by":"characters","forward":matches!(command,Right(_)),"extend":extend}),
        ),
        Up(extend) | Down(extend) => (
            "move",
            json!({"by":"lines","forward":matches!(command,Down(_)),"extend":extend}),
        ),
        WordLeft(extend) | WordRight(extend) => (
            "move",
            json!({"by":"word_ends","forward":matches!(command,WordRight(_)),"extend":extend}),
        ),
        Home(extend) => ("move_to", json!({"to":"bol","extend":extend})),
        End(extend) => ("move_to", json!({"to":"eol","extend":extend})),
        DocumentStart(extend) => ("move_to", json!({"to":"bof","extend":extend})),
        DocumentEnd(extend) => ("move_to", json!({"to":"eof","extend":extend})),
        PageUp | PageDown => ("move", json!({"by":"pages","forward":command==PageDown})),
        Undo => ("undo", json!({})),
        Redo => ("redo", json!({})),
        Copy => ("copy", json!({})),
        Cut => ("cut", json!({})),
        Paste => ("paste", json!({})),
        SelectAll => ("select_all", json!({})),
        Cancel => ("single_selection", json!({})),
    }
}
/// Visual monospace columns with configurable tab stops.
pub fn cells_with_tab(text: &str, tab: usize) -> usize {
    cells_with_tab_at(text, tab, 0)
}
pub fn cells_with_tab_at(text: &str, tab: usize, initial: usize) -> usize {
    let tab = tab.max(1);
    let mut col = initial;
    for g in text.graphemes(true) {
        col += if g == "\t" {
            tab - col % tab
        } else {
            g.width().max(1)
        };
    }
    col - initial
}
#[cfg(test)]
pub fn cells(text: &str) -> usize {
    cells_with_tab(text, 4)
}
#[cfg(test)]
pub fn byte_at_cell_with_tab(text: &str, target: usize, tab: usize) -> usize {
    let mut col = 0;
    for (i, g) in text.grapheme_indices(true) {
        let width = if g == "\t" {
            tab - col % tab
        } else {
            g.width().max(1)
        };
        if target < col + width {
            return if target - col < width.div_ceil(2) {
                i
            } else {
                i + g.len()
            };
        }
        col += width;
    }
    text.len()
}
#[cfg(test)]
mod tests {
    use super::*;
    fn insert(e: &mut Editor, s: &str, r: Option<Utf16Range>) {
        e.text_input(TextInput::Insert {
            text: s.into(),
            replacement: r,
        });
    }
    fn mark(e: &mut Editor, s: &str, selection: Utf16Range, r: Option<Utf16Range>) {
        e.text_input(TextInput::Marked {
            text: s.into(),
            selected: selection,
            replacement: r,
        });
    }
    #[test]
    fn pinyin_commit_replaces_selection_once_and_undo_is_atomic() {
        let mut e = Editor::new(Text::from("a🦀z"));
        e.set_selection(Selection::from_regions([Region::new(1, 5)]));
        mark(&mut e, "nihao", Utf16Range::new(5, 0), None);
        assert_eq!(e.text.to_string(), "anihaoz");
        assert_eq!(e.marked_utf16(), Some(Utf16Range::new(1, 5)));
        mark(&mut e, "你好", Utf16Range::new(2, 0), None);
        insert(&mut e, "你好", None);
        assert_eq!(e.text.to_string(), "a你好z");
        assert!(e.marked.is_none());
        e.command(Command::Undo);
        assert_eq!(e.text.to_string(), "a🦀z");
        e.command(Command::Redo);
        assert_eq!(e.text.to_string(), "a你好z");
    }
    #[test]
    fn replacement_ranges_reconversion_picker_and_split_surrogates() {
        let mut e = Editor::new(Text::from("a🦀日本z"));
        insert(&mut e, "😀", Some(Utf16Range::new(2, 1)));
        assert_eq!(e.text.to_string(), "a😀日本z");
        mark(
            &mut e,
            "にほん",
            Utf16Range::new(0, 3),
            Some(Utf16Range::new(3, 2)),
        );
        assert_eq!(e.selected_utf16(), Utf16Range::new(3, 3));
        insert(&mut e, "日本語", Some(Utf16Range::new(3, 3)));
        assert_eq!(e.text.to_string(), "a😀日本語z");
    }
    #[test]
    fn cancel_empty_preedit_unmark_and_commit_without_preedit() {
        let mut e = Editor::new(Text::from("old"));
        e.command(Command::SelectAll);
        mark(&mut e, "abc", Utf16Range::new(3, 0), None);
        e.command(Command::Cancel);
        assert_eq!(e.text.to_string(), "old");
        mark(&mut e, "é", Utf16Range::new(1, 0), None);
        e.text_input(TextInput::Unmark);
        assert_eq!(e.text.to_string(), "é");
        e.command(Command::Undo);
        assert_eq!(e.text.to_string(), "old");
        mark(&mut e, "x", Utf16Range::new(1, 0), None);
        mark(&mut e, "", Utf16Range::default(), None);
        assert_eq!(e.text.to_string(), "old");
        insert(&mut e, "🦀", None);
        assert_eq!(e.text.to_string(), "🦀");
    }
    #[test]
    fn extended_grapheme_deletion_and_column_movement() {
        let mut e = Editor::new(Text::from("a👩‍💻é\n你\nabc"));
        e.command(Command::End(false));
        e.command(Command::DeleteBackward);
        assert_eq!(e.text.line(0).to_string(), "a👩‍💻");
        e.command(Command::DeleteBackward);
        assert_eq!(e.text.line(0).to_string(), "a");
        e.command(Command::Down(false));
        // vel-buffer chooses the left boundary on an exact CJK midpoint.
        assert_eq!(e.caret, e.text.line_to_byte(1));
        e.command(Command::Down(false));
        assert_eq!(e.caret, e.text.line_to_byte(2) + 1);
    }
    #[test]
    fn tabs_and_double_width_hit_testing() {
        assert_eq!(cells("a\t你👩‍💻é"), 9);
        assert_eq!(byte_at_cell_with_tab("你x", 0, 4), 0);
        assert_eq!(byte_at_cell_with_tab("你x", 1, 4), 3);
        assert_eq!(byte_at_cell_with_tab("éx", 1, 4), 3);
    }
}

#[cfg(test)]
mod transaction_tests {
    use super::*;
    #[test]
    fn unicode_edits_match_flat_reference_over_repeated_transactions() {
        let mut editor = Editor::new(Text::new());
        let mut model = String::new();
        let mut caret = 0;
        let mut state = 0x76656c6f63697479u64;
        for _ in 0..2000 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            match state % 7 {
                0..=3 => {
                    let choices = ["x", "你", "🦀", "é", "\n", "👩‍💻"];
                    let text = choices[(state >> 32) as usize % choices.len()];
                    editor.text_input(TextInput::Insert {
                        text: text.into(),
                        replacement: None,
                    });
                    model.insert_str(caret, text);
                    caret += text.len();
                }
                4 => {
                    editor.command(Command::DeleteBackward);
                    if caret > 0 {
                        let previous = model[..caret].grapheme_indices(true).next_back().unwrap().0;
                        model.replace_range(previous..caret, "");
                        caret = previous;
                    }
                }
                5 => {
                    editor.command(Command::Left(false));
                    if caret > 0 {
                        caret = model[..caret].grapheme_indices(true).next_back().unwrap().0;
                    }
                }
                _ => {
                    editor.command(Command::Right(false));
                    if caret < model.len() {
                        caret += model[caret..].graphemes(true).next().unwrap().len();
                    }
                }
            }
            assert_eq!(editor.text.to_string(), model);
            assert_eq!(editor.caret, caret);
            assert!(model.is_char_boundary(caret));
        }
    }
    #[test]
    fn empty_marked_text_with_an_explicit_range_can_delete_without_preedit() {
        let mut editor = Editor::new(Text::from("a🦀z"));
        editor.text_input(TextInput::Marked {
            text: String::new(),
            selected: Utf16Range::default(),
            replacement: Some(Utf16Range::new(1, 2)),
        });
        assert_eq!(editor.text.to_string(), "az");
        editor.command(Command::Undo);
        assert_eq!(editor.text.to_string(), "a🦀z");
    }
}

/// Converts native events to the public keymap vocabulary. Composition bypasses this.
pub fn binding_key(event: &vel_platform::KeyEvent) -> Option<Key> {
    use vel_platform::KeyState;
    if event.state == KeyState::Up {
        return None;
    }
    if event.state == KeyState::FlagsChanged {
        let (name, pressed) = match event.physical_code {
            56 | 60 => ("shift", event.modifiers.shift()),
            59 | 62 => ("ctrl", event.modifiers.control()),
            58 | 61 => ("alt", event.modifiers.option()),
            54 | 55 => ("super", event.modifiers.command()),
            _ => return None,
        };
        return pressed.then(|| Key {
            name: name.into(),
            modifiers: 0,
        });
    }
    let modifiers = u8::from(event.modifiers.control())
        | (u8::from(event.modifiers.option()) * 2)
        | (u8::from(event.modifiers.shift()) * 4)
        | (u8::from(event.modifiers.command()) * 8);
    let name = match event.physical_code {
        36 => "enter".into(),
        76 => "keypad_enter".into(),
        49 => "space".into(),
        122 => "f1".into(),
        120 => "f2".into(),
        99 => "f3".into(),
        118 => "f4".into(),
        96 => "f5".into(),
        97 => "f6".into(),
        98 => "f7".into(),
        100 => "f8".into(),
        101 => "f9".into(),
        109 => "f10".into(),
        103 => "f11".into(),
        111 => "f12".into(),
        105 => "f13".into(),
        107 => "f14".into(),
        113 => "f15".into(),
        48 => "tab".into(),
        51 => "backspace".into(),
        117 => "delete".into(),
        53 => "escape".into(),
        123 => "left".into(),
        124 => "right".into(),
        125 => "down".into(),
        126 => "up".into(),
        115 => "home".into(),
        119 => "end".into(),
        116 => "pageup".into(),
        121 => "pagedown".into(),
        65 => "keypad_period".into(),
        67 => "keypad_multiply".into(),
        69 => "keypad_plus".into(),
        75 => "keypad_divide".into(),
        78 => "keypad_minus".into(),
        81 => "keypad_equals".into(),
        82 => "keypad0".into(),
        83 => "keypad1".into(),
        84 => "keypad2".into(),
        85 => "keypad3".into(),
        86 => "keypad4".into(),
        87 => "keypad5".into(),
        88 => "keypad6".into(),
        89 => "keypad7".into(),
        91 => "keypad8".into(),
        92 => "keypad9".into(),
        _ => {
            let text = event.characters_ignoring_modifiers.to_lowercase();
            if text.is_empty() || text.chars().any(char::is_control) {
                return None;
            }
            text
        }
    };
    Some(Key::from_parts(&name, modifiers))
}

#[cfg(test)]
mod keymap_tests {
    use super::*;
    #[test]
    fn default_pairing_skip_and_multiple_carets_use_contexts() {
        let mut editor = Editor::new(Text::from("\n"));
        editor.set_selection(Selection::from_regions([
            Region::caret(0),
            Region::caret(1),
        ]));
        assert!(editor.key(&Key::parse("(").unwrap()).unwrap());
        assert_eq!(editor.text.to_string(), "()\n()");
        assert_eq!(editor.regions(), [Region::caret(1), Region::caret(4)]);
        assert!(editor.key(&Key::parse(")").unwrap()).unwrap());
        assert_eq!(editor.regions(), [Region::caret(2), Region::caret(5)]);
        assert_eq!(editor.text.to_string(), "()\n()");
        editor
            .settings
            .set(Layer::View, "auto_match_enabled", json!(false));
        editor.sync();
        assert!(!editor.key(&Key::parse("(").unwrap()).unwrap());
    }
    #[test]
    fn composition_preview_does_not_change_buffer_and_commit_targets_every_caret() {
        let mut editor = Editor::new(Text::from("a\nb"));
        editor.set_selection(Selection::from_regions([
            Region::caret(1),
            Region::caret(3),
        ]));
        editor.text_input(TextInput::Marked {
            text: "ni".into(),
            selected: Utf16Range::new(2, 0),
            replacement: None,
        });
        assert_eq!(editor.buffer.text().to_string(), "a\nb");
        assert_eq!(editor.text.to_string(), "a\nbni");
        assert!(!editor.key(&Key::parse("super+d").unwrap()).unwrap());
        editor.text_input(TextInput::Insert {
            text: "你".into(),
            replacement: None,
        });
        assert_eq!(editor.text.to_string(), "a你\nb你");
        editor.command(Command::Undo);
        assert_eq!(editor.text.to_string(), "a\nb");
    }
}

#[cfg(test)]
mod context_tests {
    use super::*;
    #[test]
    fn text_contexts_use_selection_edges_in_both_directions() {
        let mut editor = Editor::new(Text::from("beforeSELECTafter"));
        for region in [Region::new(6, 12), Region::new(12, 6)] {
            editor.set_selection(Selection::from_regions([region]));
            let context = EditorContext(&editor);
            assert_eq!(context.values("preceding_text"), vec![json!("before")]);
            assert_eq!(context.values("following_text"), vec![json!("after")]);
            assert_eq!(context.values("text"), vec![json!("SELECT")]);
        }
    }
}
