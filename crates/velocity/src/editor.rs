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

#[cfg(test)]
#[path = "editor/round21_tests.rs"]
mod round21_tests;

#[path = "navigation.rs"]
pub(crate) mod navigation;

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
#[derive(Debug, Default)]
struct SelectorPoints {
    version: Option<(vel_buffer::BufferId, vel_buffer::ChangeId)>,
    values: std::collections::HashMap<usize, Option<vel_scope::ScopeStack>>,
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
    pub(crate) navigation: Option<navigation::Observer>,
    pub binary: Option<Arc<crate::binary::Snapshot>>,
    pub features: vel_largefile::FeaturePolicy,
    pub document_metrics: vel_largefile::DocumentMetrics,
    pub source_bytes: u64,
    policy_config: crate::large::FeatureConfig,
    metric_job: Option<
        mpsc::Receiver<(
            vel_buffer::BufferId,
            vel_buffer::ChangeId,
            vel_largefile::DocumentMetrics,
        )>,
    >,
    policy_identity: Option<(vel_buffer::BufferId, vel_buffer::ChangeId, u64, u64)>,
    pub vintage: Option<crate::vintage::State>,
    pub vintage_requests: Vec<vel_config::keymap::Invocation>,
    pub theme: std::rc::Rc<RefCell<vel_ui::theme::ThemeUi>>,
    pub(crate) translations: vel_i18n::Translator,
    pub html: RefCell<crate::html_ui::ViewHtml>,
    pub annotation_view: ViewId,
    pub annotations: std::rc::Rc<RefCell<crate::annotations::Annotations>>,
    pub completion: crate::completion::Completion,
    pub fields: Option<vel_snippet::Session>,
    pub fields_version: Option<(vel_buffer::BufferId, vel_buffer::ChangeId)>,
    pub snippets: Arc<std::collections::BTreeMap<String, crate::completion::SnippetEntry>>,
    pub snippet_path: Option<std::path::PathBuf>,
    pub language: crate::language::LanguageCell,
    pub layout: RefCell<crate::layout::Layout>,
    // Keep this status-only cache out of the fields touched by scene painting.
    pub(crate) status_column: Box<RefCell<vel_ui::status::ColumnCache>>,
    selector_snapshots: RefCell<
        Vec<(
            vel_buffer::BufferId,
            vel_scope::Scope,
            Arc<vel_highlight::ScopeSnapshot>,
        )>,
    >,
    selector_points: RefCell<SelectorPoints>,
    pub(crate) exact_scope_cache: RefCell<crate::editing::ExactScopeCache>,
    pub(crate) automatic_indents: Option<crate::editing::AutomaticIndents>,
    pub(crate) pending_closing_tag: Option<crate::editing::PendingClosingTag>,
    #[cfg(test)]
    pub(crate) exact_scope_parses: std::cell::Cell<usize>,
    last_visual_boundary: Option<(bool, vel_buffer::ChangeId, Selection)>,
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
    pub(crate) detected_tab_size: Option<crate::language::DetectedTabSize>,
    pub preferences: EditorSettings,
    settings_revision: u64,
    pub registry: Registry,
    pub keymap: Keymap,
    pub clipboard: String,
    /// Window UI state mirrored for keymap contexts (`panel_visible`, `overlay_visible`).
    pub ui_panel_visible: bool,
    pub ui_overlay_visible: bool,
    /// Name of the visible panel (`find`, `replace`, `incremental_find`, `find_in_files`,
    /// `output.find_results`, `output.exec`, `input`), for the `panel` key binding context.
    pub ui_panel: &'static str,
    /// Whether keyboard focus is in the panel, for the `panel_has_focus` context.
    pub ui_panel_focused: bool,
    pub actions: Vec<(String, Value)>,
    pub revision: u64,
}
thread_local! {
    // Every tab/window on the application UI thread shares this one macro and kill ring.
    static MACRO_STATE: std::rc::Rc<RefCell<vel_commands::MacroState>> = Default::default();
}
pub(crate) const DISPATCHED_WINDOW_COMMANDS: &[&str] =
    include!(concat!(env!("OUT_DIR"), "/dispatched_commands.rs"));

impl Editor {
    pub(crate) fn command_registry() -> Registry {
        let mut registry = Registry::standard();
        for name in crate::context_menu::COMMANDS {
            registry.register(name, vel_commands::Kind::WindowCommand, None);
        }
        for name in crate::html_ui::COMMANDS {
            registry.register(name, vel_commands::Kind::TextCommand, None);
        }
        for name in [
            "set_clipboard",
            "set_indentation",
            "copy_as_html",
            "print",
            "prompt_open_folder_in_new_window",
            "save_and_close",
            "new_html_sheet",
            "open_url",
            "minihtml_navigate",
            "theme_demo",
            "set_theme_appearance",
        ] {
            registry.register(name, vel_commands::Kind::WindowCommand, None);
        }
        for name in [
            "set_clipboard",
            "exit_insert_mode",
            "vintage_submit_prompt",
            "large_file_enable_feature",
            "goto_hex_offset",
            "copy_hex",
            "copy_hex_text",
        ] {
            registry.register(name, vel_commands::Kind::TextCommand, None);
        }
        crate::workbench::register_commands(&mut registry);
        for name in DISPATCHED_WINDOW_COMMANDS {
            if registry.kind(name).is_none() {
                registry.register(name, vel_commands::Kind::WindowCommand, None);
            }
        }
        registry
    }
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
                    .append(include_str!(
                        "../resources/Packages/Default/Velocity Vintage.sublime-keymap"
                    ))
                    .expect("our Vintage keymap");
                keymap
            })
            .clone();
        let preferences = settings.editor();
        let settings_revision = settings.revision();
        let mut registry = Self::command_registry();
        MACRO_STATE.with(|state| registry.share_macro_state(state.clone()));
        let document_metrics = crate::large::metrics(&text, 0);
        let policy_config = crate::large::FeatureConfig::new(&settings);
        let features = policy_config.evaluate(document_metrics);
        Self {
            navigation: None,
            binary: None,
            features,
            policy_config,
            document_metrics,
            source_bytes: 0,
            policy_identity: None,
            metric_job: None,
            vintage: None,
            vintage_requests: Vec::new(),
            theme: crate::theme_ui::default_ui(),
            translations: Default::default(),
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
            status_column: Box::default(),
            selector_snapshots: RefCell::new(Vec::new()),
            selector_points: RefCell::default(),
            exact_scope_cache: RefCell::default(),
            automatic_indents: None,
            pending_closing_tag: None,
            #[cfg(test)]
            exact_scope_parses: std::cell::Cell::new(0),
            last_visual_boundary: None,
            language_buffer: None,
            reindent_job: None,
            read_only: false,
            buffer,
            id,
            text,
            preferences,
            settings_revision,
            detected_tab_size: None,
            anchor: 0,
            caret: 0,
            marked: None,
            composition: None,
            settings,
            registry,
            keymap,
            clipboard: String::new(),
            ui_panel_visible: false,
            ui_overlay_visible: false,
            ui_panel: "",
            ui_panel_focused: false,
            actions: Vec::new(),
            revision: 0,
        }
    }
    pub fn adopt_document(&mut self) {
        self.language_buffer = Some(self.buffer.id());
        if vel_commands::sync_buffer_settings(&mut self.buffer, self.id, &self.preferences) {
            self.buffer.break_undo_group();
        }
    }
    pub fn measure_document(&mut self) {
        if self.buffer.text().len_bytes() < crate::large::LONG_LINE {
            return;
        }
        let text = self.buffer.text().clone();
        let identity = (self.buffer.id(), self.buffer.change_id());
        let (tx, rx) = mpsc::channel();
        if std::thread::Builder::new()
            .name("velocity-document-metrics".into())
            .spawn(move || {
                let mut longest = 0u64;
                let mut current = 0u64;
                for chunk in text.chunks() {
                    for byte in chunk.bytes() {
                        if byte == b'\n' {
                            longest = longest.max(current);
                            current = 0;
                        } else {
                            current += 1;
                        }
                    }
                }
                let _ = tx.send((
                    identity.0,
                    identity.1,
                    vel_largefile::DocumentMetrics {
                        bytes: text.len_bytes() as u64,
                        lines: text.len_lines() as u64,
                        longest_line_bytes: longest.max(current),
                    },
                ));
            })
            .is_ok()
        {
            self.metric_job = Some(rx);
        }
    }
    pub fn refresh_policy(&mut self) {
        let identity = (
            self.buffer.id(),
            self.buffer.change_id(),
            self.settings.cache_identity(),
            self.source_bytes,
        );
        if self.policy_identity == Some(identity) {
            return;
        }
        if self.policy_identity.is_none_or(|old| old.2 != identity.2) {
            self.policy_config = crate::large::FeatureConfig::new(&self.settings);
        }
        self.policy_identity = Some(identity);
        let caret = self.buffer.view(self.id).sel().primary().map_or(0, |r| r.b);
        let text = self.buffer.text();
        let row = text.byte_to_line(caret);
        let metrics = vel_largefile::DocumentMetrics {
            bytes: (text.len_bytes() as u64).max(self.source_bytes),
            lines: text.len_lines() as u64,
            longest_line_bytes: (crate::layout::body_len(text, row) as u64).max(
                self.document_metrics
                    .longest_line_bytes
                    .min(text.len_bytes() as u64),
            ),
        };
        self.document_metrics = metrics;
        self.features = self.policy_config.evaluate(metrics);
        if self.source_bytes > 0 {
            self.features.highlighting = vel_largefile::HighlightMode::Viewport;
            self.features.indexing = false;
            self.features.word_wrap = false;
            self.features.spell_check = false;
            self.features.diff_against_disk = false;
        }
        self.annotations
            .borrow_mut()
            .set_policy(self.features.diff_against_disk, self.features.spell_check);
    }
    pub fn sync(&mut self) {
        self.rebase_navigation(false);
        if self.binary.is_some() {
            return;
        }
        self.refresh_policy();
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
            let old_tab_size = self.preferences.tab_size;
            self.preferences = self.settings.editor();
            self.settings_revision = self.settings.revision();
            if old_tab_size != self.preferences.tab_size {
                self.layout.borrow_mut().invalidate();
            }
            self.configure_vintage();
            vel_commands::sync_buffer_settings(&mut self.buffer, self.id, &self.preferences);
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
                l.policy != self.features
                    || (l.policy.highlighting == vel_largefile::HighlightMode::Viewport
                        && l.viewport_settings != self.settings.cache_identity())
                    || l.version != self.buffer.change_id()
                    || self.language_buffer != Some(self.buffer.id())
            });
        if reset {
            self.fields = None;
            self.completion.hide();
            let mut previous = self.language.borrow_mut().take().expect("language");
            if let Some(snapshot) = &previous.scopes {
                let mut cache = self.selector_snapshots.borrow_mut();
                crate::language::retire(std::mem::take(&mut *cache));
                cache.push((
                    self.language_buffer.unwrap_or(self.buffer.id()),
                    previous.root,
                    snapshot.clone(),
                ));
            }
            let result = crate::language::Language::with_policy(
                previous.assets.clone(),
                &previous.syntax,
                self.buffer.text().clone(),
                self.buffer.change_id(),
                previous.scheme.clone(),
                self.preferences.tab_size,
                self.features,
                &self.settings,
            );
            match result {
                Ok(mut language) => {
                    if self.language_buffer == Some(self.buffer.id()) {
                        let changes = self.buffer.changes_since(previous.version);
                        language.inherit_guides(&mut previous, changes.as_deref());
                    }
                    *self.language.borrow_mut() = Some(language);
                }
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

    pub(crate) fn set_view_setting(&mut self, key: &str, value: vel_config::Value) {
        if key == "tab_size" {
            self.detected_tab_size = None;
        }
        self.settings.set(Layer::View, key, value);
    }
    pub fn attach_language(
        &mut self,
        assets: Arc<crate::language::Assets>,
        syntax: &str,
        scheme: Arc<vel_color_scheme::ColorScheme>,
    ) -> Result<(), String> {
        self.completion.hide();
        self.refresh_policy();
        let language = crate::language::Language::with_policy(
            assets,
            syntax,
            self.buffer.text().clone(),
            self.buffer.change_id(),
            scheme,
            self.preferences.tab_size,
            self.features,
            &self.settings,
        )?;
        *self.language.borrow_mut() = Some(language);
        self.automatic_indents = None;
        self.language_buffer = Some(self.buffer.id());
        Ok(())
    }
    pub fn poll_language(&mut self) -> bool {
        let mut changed = self.layout.borrow().long.poll();
        if let Some((buffer, version, metrics)) =
            self.metric_job.as_ref().and_then(|rx| rx.try_recv().ok())
        {
            self.metric_job = None;
            if buffer == self.buffer.id() && version == self.buffer.change_id() {
                self.document_metrics = metrics;
                self.policy_identity = None;
                self.sync();
                changed = true;
            }
        }
        changed |= self
            .language
            .borrow_mut()
            .as_mut()
            .is_some_and(|l| l.poll());
        changed |= self.poll_closing_tag();
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
                self.complete_vintage_host();
            } else if let Some(vi) = &mut self.vintage {
                vi.cancel_host();
            }
        }
        changed
    }
    /// Complete mapped bookmark regions for gutter decoration; no allocation on paint.
    #[cfg_attr(not(test), allow(dead_code))] // Lane B consumes this accessor when drawing the gutter.
    pub fn bookmarks(&self) -> &[Region] {
        self.buffer
            .view(self.id)
            .get_regions(ops::BOOKMARKS)
            .map_or(&[], |set| set.regions.as_slice())
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
        self.observe_navigation("selection");
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
        self.observe_navigation("selection");
    }
    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }
    pub fn selected_utf16(&self) -> Utf16Range {
        if self.binary.is_some() {
            return Utf16Range::default();
        }
        Utf16Range::from_bytes(&self.text, self.selection())
    }
    pub fn marked_utf16(&self) -> Option<Utf16Range> {
        self.marked
            .clone()
            .map(|r| Utf16Range::from_bytes(&self.text, r))
    }
    pub fn run(&mut self, name: &str, args: &Value) -> Result<(), String> {
        self.observe_navigation("selection");
        let _navigation = self
            .navigation
            .as_ref()
            .map(|observer| navigation::Guard::command_args(&observer.history, name, args));
        let result = self.run_inner(name, args);
        self.observe_navigation(name);
        result
    }
    fn run_inner(&mut self, name: &str, args: &Value) -> Result<(), String> {
        if (self.binary.is_some()
            || self.document_metrics.longest_line_bytes >= crate::large::LONG_LINE as u64
            || name == "large_file_enable_feature")
            && self.large_command(name, args)?
        {
            return Ok(());
        }
        if self.read_only
            && self.registry.kind(name) == Some(vel_commands::Kind::TextCommand)
            && !crate::html_ui::COMMANDS.contains(&name)
            && !matches!(
                name,
                "move"
                    | "move_to"
                    | "select_all"
                    | "copy"
                    | "single_selection"
                    | "find"
                    | "find_next"
                    | "find_prev"
            )
        {
            return Ok(());
        }
        self.unmark();
        self.sync();
        if !matches!(
            name,
            "move" | "move_to" | "select_all" | "copy" | "single_selection"
        ) && self
            .language
            .borrow()
            .as_ref()
            .is_some_and(|l| l.policy.highlighting == vel_largefile::HighlightMode::Viewport)
        {
            let language = self.language.borrow();
            let language = language.as_ref().expect("viewport language");
            let mut seen = std::collections::HashSet::new();
            // Preserve only snapshots that are currently published at a caret. Never invoke
            // historic scope lookup just to prepare a command that may not edit anything.
            for region in self.regions() {
                if let Some(snapshot) = language.scope_snapshot_at(region.b)
                    && seen.insert(Arc::as_ptr(&snapshot))
                {
                    self.retain_selector_snapshot(language.root, snapshot);
                }
            }
        }
        let before_version = self.buffer.change_id();
        let line_folds = if matches!(name, "duplicate_line" | "join_lines") {
            self.line_command_folds()
        } else {
            None
        };
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
        let mut registry = self.registry.take_for_dispatch();
        let mut result = registry.run(name, args, self);
        self.registry = registry;
        if let Some(folds) = line_folds {
            self.reveal_line_edits(before_version, folds);
        }
        for command in std::mem::take(&mut self.vintage_requests) {
            if let Err(error) = self.run(&command.command, &command.args) {
                result = Err(error);
            }
        }
        if name == "reindent" && self.reindent_job.is_none() {
            self.complete_vintage_host();
        }
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
    /// Uses published scopes while re-highlighting; the buffer's retained edit maps relocate
    /// the old snapshot rather than parsing a line on the input thread.
    fn retain_selector_snapshot(
        &self,
        root: vel_scope::Scope,
        snapshot: Arc<vel_highlight::ScopeSnapshot>,
    ) {
        let mut cache = self.selector_snapshots.borrow_mut();
        if !cache.iter().any(|(_, _, old)| Arc::ptr_eq(old, &snapshot)) {
            if snapshot.range == (0..snapshot.text.len_lines()) {
                crate::language::retire(std::mem::take(&mut *cache));
            } else if cache.len() == 32 {
                crate::language::retire(cache.remove(0));
            }
            cache.push((self.buffer.id(), root, snapshot));
            self.selector_points.borrow_mut().values.clear();
        }
    }
    fn selector_scopes(&self, byte: usize) -> Option<vel_scope::ScopeStack> {
        self.published_scopes(byte, false)
    }
    fn check_selector_snapshot_identity(&self, root: vel_scope::Scope) {
        if self
            .selector_snapshots
            .borrow()
            .iter()
            .any(|(id, scope, _)| *id != self.buffer.id() || *scope != root)
        {
            crate::language::retire(std::mem::take(&mut *self.selector_snapshots.borrow_mut()));
            self.selector_points.borrow_mut().values.clear();
        }
    }
    fn selector_snapshot_bounds(
        &self,
        snapshot: &vel_highlight::ScopeSnapshot,
    ) -> Option<(usize, Range<usize>)> {
        let lo = snapshot.text.line_to_byte(snapshot.range.start);
        let hi = if snapshot.range.end < snapshot.text.len_lines() {
            snapshot.text.line_to_byte(snapshot.range.end)
        } else {
            snapshot.text.len_bytes()
        };
        let start =
            self.buffer
                .transform_point_from(lo, snapshot.version, vel_buffer::Bias::Left)?;
        let end =
            self.buffer
                .transform_point_from(hi, snapshot.version, vel_buffer::Bias::Right)?;
        // Both byte endpoints are inclusive in the snapshot query.
        Some((hi, start..end))
    }
    /// Resolve exact scopes first, then sweep the sorted unresolved carets
    /// through each usable snapshot's edit journal together.
    fn batch_selector_scopes(&self) -> Vec<Option<vel_scope::ScopeStack>> {
        use vel_highlight::ScopeProvider;
        let language = self.language.borrow();
        let Some(language) = language.as_ref() else {
            return Vec::new();
        };
        self.check_selector_snapshot_identity(language.root);
        let selection = self.buffer.view(self.id).sel();
        let mut missing = Vec::new();
        let mut results: Vec<_> = selection
            .iter()
            .enumerate()
            .map(|(i, r)| {
                if let Some(snapshot) = language.scope_snapshot_at(r.b) {
                    self.retain_selector_snapshot(language.root, snapshot.clone());
                    snapshot
                        .scopes_at(r.b)
                        .map(|s| vel_scope::ScopeStack::from_vec(s.to_vec()))
                } else {
                    missing.push((i, r.b));
                    None
                }
            })
            .collect();
        let cache = self.selector_snapshots.borrow();
        let mut projected = Vec::new();
        for snapshot in language
            .scopes
            .iter()
            .chain(cache.iter().rev().map(|(_, _, s)| s))
        {
            if missing.is_empty() {
                break;
            }
            let Some((limit, bounds)) = self.selector_snapshot_bounds(snapshot) else {
                continue;
            };
            let first = missing.partition_point(|&(_, byte)| byte < bounds.start);
            let end = missing.partition_point(|&(_, byte)| byte <= bounds.end);
            if first >= end {
                continue;
            }
            projected.clear();
            projected.extend(missing[first..end].iter().map(|&(_, byte)| byte));
            if !self
                .buffer
                .unmap_left_points_to(&mut projected, snapshot.version)
            {
                continue;
            }
            for (&(i, _), &byte) in missing[first..end].iter().zip(&projected) {
                let at = snapshot.text.floor_char_boundary(byte.min(limit));
                results[i] = snapshot
                    .scopes_at(at)
                    .map(|s| vel_scope::ScopeStack::from_vec(s.to_vec()));
            }
            missing.retain(|&(i, _)| results[i].is_none());
        }
        results
    }
    /// Published scopes only: display may map edits; edits require an untouched caret line.
    pub(crate) fn published_scopes(
        &self,
        byte: usize,
        unchanged_line: bool,
    ) -> Option<vel_scope::ScopeStack> {
        use vel_highlight::ScopeProvider;
        let language = self.language.borrow();
        let language = language.as_ref()?;
        self.check_selector_snapshot_identity(language.root);
        if let Some(snapshot) = language.scope_snapshot_at(byte) {
            self.retain_selector_snapshot(language.root, snapshot.clone());
            return snapshot.scopes_at(byte).map(|scopes| {
                let mut stack = vel_scope::ScopeStack::new();
                for scope in scopes {
                    stack.push(*scope);
                }
                stack
            });
        }
        let mut points = self.selector_points.borrow_mut();
        let version = (self.buffer.id(), self.buffer.change_id());
        if points.version != Some(version) {
            points.version = Some(version);
            points.values.clear();
        }
        if !unchanged_line && let Some(scopes) = points.values.get(&byte) {
            return scopes.clone();
        }
        let cache = self.selector_snapshots.borrow();
        let scopes = language
            .scopes
            .iter()
            .chain(cache.iter().rev().map(|(_, _, snapshot)| snapshot));
        'snapshots: for snapshot in scopes {
            let Some((limit, bounds)) = self.selector_snapshot_bounds(snapshot) else {
                continue;
            };
            if byte < bounds.start || byte > bounds.end {
                continue;
            }
            let mut projected = [byte];
            if !self
                .buffer
                .unmap_left_points_to(&mut projected, snapshot.version)
            {
                continue;
            }
            let old_byte = snapshot.text.floor_char_boundary(projected[0].min(limit));
            if let Some(scopes) = snapshot.scopes_at(old_byte) {
                if unchanged_line {
                    let row = snapshot.text.byte_to_line(old_byte);
                    let mut line = snapshot.text.line_to_byte(row)
                        ..(snapshot.text.line_to_byte(row) + snapshot.text.line_len_bytes(row));
                    let Some(changes) = self.buffer.changes_since(snapshot.version) else {
                        continue;
                    };
                    for (_, changes) in changes {
                        if changes.changes().iter().any(|change| {
                            change.old_range.start <= line.end && change.old_range.end >= line.start
                        }) {
                            continue 'snapshots;
                        }
                        line = changes.transform_point(line.start, vel_buffer::Bias::Right)
                            ..changes.transform_point(line.end, vel_buffer::Bias::Left);
                    }
                }
                let mut stack = vel_scope::ScopeStack::new();
                for scope in scopes {
                    stack.push(*scope);
                }
                if !unchanged_line {
                    points.values.insert(byte, Some(stack.clone()));
                }
                return Some(stack);
            }
        }
        if !unchanged_line {
            points.values.insert(byte, None);
        }
        None
    }
    /// Budgeted exact scopes, then published scopes only when the caret line was untouched.
    pub(crate) fn snippet_scopes(&self, points: &[usize]) -> Option<Vec<Vec<vel_scope::Scope>>> {
        self.exact_scopes(points).or_else(|| {
            points
                .iter()
                .map(|&point| {
                    self.published_scopes(point, true)
                        .map(|s| s.as_slice().to_vec())
                })
                .collect()
        })
    }
    pub fn context_matches(&self, context: &[vel_config::keymap::Context]) -> bool {
        let provider = EditorContext::new(self);
        context.iter().all(|c| c.matches(&provider))
    }
    pub fn resolve_key(&mut self, key: &Key) -> Resolution {
        if let Some(resolution) = self.vintage_key(key)
            && !matches!(resolution, Resolution::Unbound)
        {
            return resolution;
        }
        if self.marked.is_some() {
            return Resolution::Unbound;
        }
        let mut keymap = std::mem::take(&mut self.keymap);
        let resolution = keymap.resolve(key, &EditorContext::new(self));
        self.keymap = keymap;
        if matches!(resolution, Resolution::Unbound)
            && key.modifiers == 1
            && key.name == "o"
            && self.command_mode()
        {
            // AppKit expands this key into two selectors, the second a movement.
            // Vintage and user bindings got first refusal; do not run half the sequence.
            return Resolution::Pending;
        }
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
        if self.read_only || self.command_mode() {
            return;
        }
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
                if self.vintage.is_some() {
                    if let Some(command) = self.vintage_text(&text) {
                        let _ = self.run(&command.command, &command.args);
                    }
                } else {
                    let _ = self.run("insert", &json!({"characters":text}));
                }
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
                let compatible = self
                    .composition
                    .as_ref()
                    .is_none_or(|c| c.original == original);
                let old_preview = self.text.clone();
                let old_range = self
                    .composition
                    .as_ref()
                    .filter(|c| c.original == original)
                    .map_or_else(
                        || original.clone(),
                        |c| original.start..original.start + c.contents.len(),
                    );
                self.layout
                    .borrow_mut()
                    .prepare_change((self.buffer.id(), vel_buffer::ChangeId(self.revision)));
                self.text = self.buffer.text().clone();
                self.text.replace(original.clone(), &text);
                if !compatible {
                    self.layout.borrow_mut().invalidate();
                }
                self.layout.borrow_mut().change(
                    &old_preview,
                    &self.text,
                    (self.buffer.id(), vel_buffer::ChangeId(self.revision + 1)),
                    &ChangeSet::new(vec![Change {
                        old_range,
                        new_len: text.len(),
                    }]),
                );
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
        if let Some(binary) = &self.binary {
            self.caret = at.min(binary.view.len_bytes() as usize);
            if !extend {
                self.anchor = self.caret;
            }
            return;
        }
        self.unmark();
        let at = self.text.floor_char_boundary(at);
        self.set_selection(Selection::from_regions([Region::new(
            if extend { self.anchor } else { at },
            at,
        )]));
    }
    /// Selects the word at caret position `at` (a caret right after a word takes that word).
    pub fn select_word(&mut self, at: usize) {
        self.select_word_impl(at, false);
    }
    /// Selects the word, or run of spaces or punctuation, under the pointer: the character that
    /// starts at `at`, for double clicks.
    pub fn select_word_under(&mut self, at: usize) {
        self.select_word_impl(at, true);
    }
    fn select_word_impl(&mut self, at: usize, under: bool) {
        self.set_caret(at, false);
        if self.text.line_len_bytes(self.text.byte_to_line(self.caret)) >= crate::large::LONG_LINE {
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
            let a = self
                .layout
                .borrow()
                .long_motion(&self.text, self.caret, false, true);
            let b = self
                .layout
                .borrow()
                .long_motion(&self.text, self.caret, true, true);
            self.set_selection(Selection::from_regions([Region::new(a, b)]));
            return;
        }
        if under {
            let word = ops::word_region(&self.buffer, self.id, self.caret);
            self.set_selection(Selection::from_regions([word]));
            return;
        }
        ops::expand_selection(&mut self.buffer, self.id, ops::ExpandTo::Word);
        self.sync();
    }
    pub fn select_line(&mut self, at: usize) {
        self.set_caret(at, false);
        ops::expand_selection(&mut self.buffer, self.id, ops::ExpandTo::Line);
        self.sync();
    }
    pub fn command(&mut self, command: Command) {
        let vi_key = match command {
            Command::Cancel => Some("escape"),
            Command::Newline => Some("enter"),
            Command::DeleteBackward => Some("backspace"),
            _ => None,
        };
        if let Some(key) = vi_key
            && self.marked.is_none()
            && self.vintage.is_some()
            && self
                .key(&Key::parse(key).expect("named key"))
                .unwrap_or(false)
        {
            return;
        }
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
        if self.accepts_native_selector(command) {
            let (name, args) = selector_command(command);
            let _ = self.run(name, &args);
        }
    }
    /// Key resolution gives Vintage first refusal; AppKit fallback must not edit normal mode.
    pub(crate) fn accepts_native_selector(&self, command: Command) -> bool {
        !matches!(
            command,
            Command::DeleteBackward
                | Command::DeleteForward
                | Command::DeleteWord(_)
                | Command::DeleteDecomposingBackward
                | Command::DeleteToLineStart
                | Command::DeleteToParagraphEnd
                | Command::Newline
                | Command::LiteralNewline
                | Command::Tab
                | Command::Backtab
                | Command::Yank
                | Command::Transpose
                | Command::DeleteToMark
                | Command::UppercaseWord
                | Command::LowercaseWord
                | Command::CapitalizeWord
                | Command::Cut
                | Command::Paste
                | Command::Undo
                | Command::Redo
        ) || !self.command_mode()
    }
}
impl Host for Editor {
    fn intercept_text_command(&mut self, name: &str, args: &Value) -> Result<bool, String> {
        let previous_boundary = self.last_visual_boundary.take();
        if name == "move_to" && matches!(args["to"].as_str(), Some("bol" | "eol")) {
            self.synchronize_layout();
            if self.layout.borrow().wrapped() || self.layout.borrow().has_folds() {
                let end = args["to"] == "eol";
                let current = self.buffer.view(self.id).sel();
                let repeated = previous_boundary.is_some_and(|(was_end, version, selection)| {
                    was_end == end && version == self.buffer.change_id() && &selection == current
                });
                let extend = args["extend"].as_bool().unwrap_or(false);
                let mut regions: Vec<_> = current
                    .iter()
                    .map(|r| {
                        let at = crate::view::visual_line_boundary(self, r.b, end, repeated);
                        Region::new(if extend { r.a } else { at }, at)
                    })
                    .collect();
                let primary = regions.remove(current.primary_index());
                regions.push(primary);
                let selection = Selection::from_regions(regions);
                self.set_selection(selection.clone());
                self.last_visual_boundary = Some((end, self.buffer.change_id(), selection));
                return Ok(true);
            }
        }
        if self.vintage_command(name, args)? {
            return Ok(true);
        }
        if matches!(name, "swap_line_up" | "swap_line_down")
            && let Some(folds) = self.line_command_folds()
        {
            // Build 4107 notes improved Swap Lines, but doesn't specify fold semantics.
            // https://www.sublimetext.com/download
            ops::swap_folded_lines(
                &mut self.buffer,
                self.id,
                name == "swap_line_down",
                &folds,
                crate::layout::FOLDS,
            );
            return Ok(true);
        }
        if name == "unindent"
            && self.buffer.views().any(|id| {
                self.buffer
                    .view(id)
                    .get_regions(crate::layout::FOLDS)
                    .is_some_and(|set| !set.is_empty())
            })
        {
            ops::unindent_preserving_folds(&mut self.buffer, self.id, crate::layout::FOLDS);
            return Ok(true);
        }
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
    fn defer_macro_playback(&self) -> bool {
        true
    }
    fn wrap_width(&self) -> Option<usize> {
        Some(self.settings.usize("wrap_width", 0))
    }
    fn kill_to_bol(&mut self) -> Vec<String> {
        self.synchronize_layout();
        let layout = self.layout.borrow();
        let starts: Vec<_> = self
            .buffer
            .view(self.id)
            .sel()
            .iter()
            .map(|r| {
                let (visual, segment) = layout.visual_at(&self.text, r.b);
                self.text.line_to_byte(layout.locate(visual).0) + segment.bytes.start
            })
            .collect();
        drop(layout);
        ops::delete_to_bol(&mut self.buffer, self.id, Some(&starts))
    }
    fn kill_to_hard_eol(&mut self) -> Vec<String> {
        let values = ops::delete_to_hard_eol(&mut self.buffer, self.id);
        if values.iter().any(|s| !s.is_empty()) {
            self.finish_vintage_visual_kill();
        }
        values
    }
    fn wrap_comment_prefixes(&self) -> Vec<String> {
        let language = self.language.borrow();
        let Some(language) = language.as_ref() else {
            return vec![];
        };
        let mut prefixes = std::collections::BTreeSet::new();
        for region in self.buffer.view(self.id).sel().iter() {
            let scopes = language.scopes(&self.text, region.begin());
            let metadata = language.assets.metadata.metadata_for(&scopes);
            for suffix in ["", "_2", "_3", "_4"] {
                if metadata
                    .shell_variables
                    .get(&format!("TM_COMMENT_END{suffix}"))
                    .is_some_and(|end| !end.is_empty())
                {
                    continue;
                }
                if let Some(start) = metadata
                    .shell_variables
                    .get(&format!("TM_COMMENT_START{suffix}"))
                {
                    let start = start.trim_end();
                    if !start.is_empty() {
                        prefixes.insert(start.to_owned());
                        if start == "//" {
                            prefixes.extend(["///".into(), "//!".into()]);
                        }
                    }
                }
            }
            if scopes
                .scopes
                .iter()
                .any(|scope| scope.build_string().starts_with("comment.block"))
            {
                prefixes.insert("*".into());
            }
        }
        prefixes.into_iter().collect()
    }
    fn clipboard(&mut self) -> String {
        self.clipboard.clone()
    }
    fn set_clipboard(&mut self, text: &str) {
        self.clipboard = text.into();
    }
    fn set_clipboard_regions(&mut self, parts: Vec<String>, whole_lines: bool) {
        crate::editing::remember_copy(&self.clipboard, parts, whole_lines);
    }
    fn action(&mut self, name: &str, args: &Value) -> Result<(), String> {
        self.actions.push((name.into(), args.clone()));
        Ok(())
    }
}
struct EditorContext<'a>(
    &'a Editor,
    std::cell::OnceCell<Vec<Option<vel_scope::ScopeStack>>>,
);
impl<'a> EditorContext<'a> {
    fn new(editor: &'a Editor) -> Self {
        Self(editor, std::cell::OnceCell::new())
    }
    fn batch_scopes(&self) -> Option<&[Option<vel_scope::ScopeStack>]> {
        (self.0.buffer.view(self.0.id).sel().len() >= 32).then(|| {
            self.1
                .get_or_init(|| self.0.batch_selector_scopes())
                .as_slice()
        })
    }
}
impl ContextProvider for EditorContext<'_> {
    fn scopes_ready(&self) -> bool {
        if self.0.marked.is_some() {
            return false;
        }
        self.0.language.borrow().is_none()
            || self.batch_scopes().map_or_else(
                || {
                    self.0
                        .buffer
                        .view(self.0.id)
                        .sel()
                        .iter()
                        .all(|r| self.0.selector_scopes(r.b).is_some())
                },
                |scopes| scopes.iter().all(Option::is_some),
            )
    }
    fn values(&self, key: &str) -> Vec<Value> {
        let e = self.0;
        let selections = e.buffer.view(e.id).sel();
        if key == "setting.command_mode" || key.starts_with("vi_") {
            return vec![json!(
                e.vintage.as_ref().is_some_and(|vi| vi.engine.context(key))
            )];
        }
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
                    let start = e
                        .text
                        .line_to_byte(before_row)
                        .max(e.text.floor_char_boundary(r.begin().saturating_sub(8192)));
                    let end = (e.text.line_to_byte(after_row) + e.text.line_len_bytes(after_row))
                        .min(e.text.floor_char_boundary(
                            r.end().saturating_add(8192).min(e.text.len_bytes()),
                        ));
                    json!(
                        e.text
                            .slice(match key {
                                "preceding_text" => start..r.begin(),
                                "following_text" => r.end()..end,
                                _ =>
                                    r.begin()
                                        ..e.text
                                            .floor_char_boundary((r.begin() + 8192).min(r.end())),
                            })
                            .to_string()
                    )
                })
                .collect(),
            "has_prev_field" => vec![json!(e.field_previous())],
            "has_next_field" => vec![json!(e.fields.as_ref().is_some_and(|s| s.has_next_field()))],
            "auto_complete_visible" => vec![json!(e.completion.visible())],
            "overlay_visible" => vec![json!(e.ui_overlay_visible)],
            "panel_visible" => vec![json!(e.ui_panel_visible)],
            "panel" => vec![json!(e.ui_panel)],
            "panel_has_focus" => vec![json!(e.ui_panel_focused)],
            "is_recording" | "is_recording_macro" => vec![json!(e.registry.is_recording())],
            _ => vec![],
        }
    }
    fn selector_matches(&self, selector: &str) -> Vec<bool> {
        let plain = self.0.language.borrow().is_none();
        let parsed = (!plain)
            .then(|| selector.parse::<vel_scope::Selector>().ok())
            .flatten();
        if !plain && let Some(scopes) = self.batch_scopes() {
            return scopes
                .iter()
                .map(|stack| {
                    stack
                        .as_ref()
                        .zip(parsed.as_ref())
                        .is_some_and(|(stack, selector)| {
                            selector.does_match(stack.as_slice()).is_some()
                        })
                })
                .collect();
        }
        self.0
            .buffer
            .view(self.0.id)
            .sel()
            .iter()
            .map(|r| {
                if plain {
                    PlainScope.matches(selector, r.b)
                } else {
                    parsed.as_ref().is_some_and(|selector| {
                        self.0
                            .selector_scopes(r.b)
                            .is_some_and(|stack| selector.does_match(stack.as_slice()).is_some())
                    })
                }
            })
            .collect()
    }
}
pub fn selector_command(command: Command) -> (&'static str, Value) {
    use Command::*;
    match command {
        DeleteBackward => ("left_delete", json!({})),
        DeleteForward => ("right_delete", json!({})),
        DeleteWord(forward) => ("delete_word", json!({"forward":forward})),
        DeleteDecomposingBackward => ("left_delete", json!({"decompose":true})),
        DeleteToLineStart => ("delete_to_bol", json!({})),
        DeleteToParagraphEnd => ("delete_to_hard_eol", json!({})),
        Newline => ("insert", json!({"characters":"\n"})),
        LiteralNewline => ("insert", json!({"characters":"\n","literal":true})),
        Tab => ("insert_best_completion", json!({})),
        Backtab => ("unindent", json!({})),
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
        ParagraphStart(extend) => ("move_to", json!({"to":"hardbol","extend":extend})),
        ParagraphEnd(extend) => ("move_to", json!({"to":"hardeol","extend":extend})),
        SelectParagraphBoundary(forward) => (
            "move",
            json!({"by":"paragraphs","forward":forward,"extend":true}),
        ),
        DocumentStart(extend) => ("move_to", json!({"to":"bof","extend":extend})),
        DocumentEnd(extend) => ("move_to", json!({"to":"eof","extend":extend})),
        PageUp | PageDown => ("move", json!({"by":"pages","forward":command==PageDown})),
        ScrollPage(forward) => ("scroll_page", json!({"forward":forward})),
        SelectPage(forward) => (
            "move",
            json!({"by":"pages","forward":forward,"extend":true}),
        ),
        CenterSelection => ("show_at_center", json!({})),
        Yank => ("yank", json!({})),
        Transpose => ("transpose", json!({})),
        SetMark => ("set_mark", json!({})),
        SelectToMark => ("select_to_mark", json!({})),
        SwapWithMark => ("swap_with_mark", json!({})),
        DeleteToMark => ("delete_to_mark", json!({})),
        UppercaseWord => ("upper_case", json!({})),
        LowercaseWord => ("lower_case", json!({})),
        CapitalizeWord => ("title_case", json!({})),
        SelectWord => ("expand_selection", json!({"to":"word"})),
        SelectLine => ("expand_selection", json!({"to":"line"})),
        SelectParagraph => ("expand_selection", json!({"to":"paragraph"})),
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
    fn press(editor: &mut Editor, keys: &str) {
        for key in keys.split(' ') {
            editor.key(&Key::parse(key).unwrap()).unwrap();
        }
    }
    fn select(editor: &mut Editor, regions: &[(usize, usize)]) {
        editor.set_selection(Selection::from_regions(
            regions.iter().map(|&(a, b)| Region::new(a, b)),
        ));
    }
    #[test]
    fn whole_line_copies_paste_above_the_caret_line_and_line_breaks_are_normalized() {
        let mut editor = Editor::new(Text::from("one\ntwo\n"));
        select(&mut editor, &[(1, 1)]);
        editor.run("copy", &json!({})).unwrap();
        assert_eq!(editor.clipboard, "one\n");
        select(&mut editor, &[(5, 5)]);
        editor.run("paste", &json!({})).unwrap();
        assert_eq!(editor.text.to_string(), "one\none\ntwo\n");
        assert_eq!(editor.regions(), [Region::caret(9)]);
        // Copies from several carets go one per caret.
        let mut editor = Editor::new(Text::from("a\nb\nx\ny\n"));
        select(&mut editor, &[(0, 1), (2, 3)]);
        editor.run("copy", &json!({})).unwrap();
        select(&mut editor, &[(5, 5), (7, 7)]);
        editor.run("paste", &json!({})).unwrap();
        assert_eq!(editor.text.to_string(), "a\nb\nxa\nyb\n");
        // Text from elsewhere with CR or CRLF breaks is pasted with LF.
        let mut editor = Editor::new(Text::from(""));
        editor.clipboard = "a\r\nb\rc".into();
        editor.run("paste", &json!({})).unwrap();
        assert_eq!(editor.text.to_string(), "a\nb\nc");
    }
    #[test]
    fn repeated_backspace_is_undone_at_once() {
        let mut editor = Editor::new(Text::from("hello"));
        select(&mut editor, &[(5, 5)]);
        press(&mut editor, "backspace backspace backspace");
        assert_eq!(editor.text.to_string(), "he");
        editor.run("undo", &json!({})).unwrap();
        assert_eq!(editor.text.to_string(), "hello");
    }
    #[test]
    fn indentation_tab_and_copy_settings_switch_their_behaviour() {
        let set = |editor: &mut Editor, key: &str, value: serde_json::Value| {
            editor.settings.set(Layer::View, key, value);
            editor.sync();
        };
        // auto_indent: Enter keeps the indentation only when on.
        let mut editor = Editor::new(Text::from("\tfoo"));
        select(&mut editor, &[(4, 4)]);
        set(&mut editor, "auto_indent", json!(false));
        press(&mut editor, "enter");
        assert_eq!(editor.text.to_string(), "\tfoo\n");
        // use_tab_stops with spaces: Tab goes to the next stop, Backspace to the previous one;
        // without, Tab inserts tab_size spaces and Backspace deletes one.
        let mut editor = Editor::new(Text::from("ab"));
        set(&mut editor, "translate_tabs_to_spaces", json!(true));
        select(&mut editor, &[(0, 0)]);
        editor.run("insert", &json!({"characters": " "})).unwrap();
        press(&mut editor, "tab");
        assert_eq!(editor.text.to_string(), "    ab");
        press(&mut editor, "backspace");
        assert_eq!(editor.text.to_string(), "ab");
        set(&mut editor, "use_tab_stops", json!(false));
        editor.run("insert", &json!({"characters": " "})).unwrap();
        press(&mut editor, "tab");
        assert_eq!(editor.text.to_string(), "     ab");
        press(&mut editor, "backspace");
        assert_eq!(editor.text.to_string(), "    ab");
        // shift+tab unindents in leading white space, inserts a tab after text, and unindents
        // everywhere with shift_tab_unindent.
        let mut editor = Editor::new(Text::from("\tfoo bar"));
        select(&mut editor, &[(1, 1)]);
        press(&mut editor, "shift+tab");
        assert_eq!(editor.text.to_string(), "foo bar");
        select(&mut editor, &[(3, 3)]);
        press(&mut editor, "shift+tab");
        assert_eq!(editor.text.to_string(), "foo\t bar");
        set(&mut editor, "shift_tab_unindent", json!(true));
        editor.run("undo", &json!({})).unwrap();
        press(&mut editor, "tab");
        assert_eq!(editor.text.to_string(), "foo\t bar");
        select(&mut editor, &[(0, 0)]);
        press(&mut editor, "tab");
        select(&mut editor, &[(5, 5)]);
        press(&mut editor, "shift+tab");
        assert_eq!(editor.text.to_string(), "foo\t bar");
        // copy_with_empty_selection: off, an empty selection copies nothing.
        let mut editor = Editor::new(Text::from("one\ntwo\n"));
        editor.clipboard = "kept".into();
        set(&mut editor, "copy_with_empty_selection", json!(false));
        select(&mut editor, &[(1, 1)]);
        editor.run("cut", &json!({})).unwrap();
        assert_eq!(editor.text.to_string(), "one\ntwo\n");
        assert_eq!(editor.clipboard, "kept");
        set(&mut editor, "copy_with_empty_selection", json!(true));
        editor.run("cut", &json!({})).unwrap();
        assert_eq!(editor.text.to_string(), "two\n");
        assert_eq!(editor.clipboard, "one\n");
    }
    #[test]
    fn enter_on_an_indentation_only_line_leaves_it_empty() {
        let mut editor = Editor::new(Text::from("\tfoo"));
        select(&mut editor, &[(4, 4)]);
        editor.run("insert", &json!({"characters": "\n"})).unwrap();
        editor.run("insert", &json!({"characters": "\n"})).unwrap();
        assert_eq!(editor.text.to_string(), "\tfoo\n\n\t");
        assert_eq!(editor.caret, 7);
        editor
            .settings
            .set(Layer::View, "trim_automatic_white_space", json!(false));
        editor.sync();
        editor.run("insert", &json!({"characters": "\n"})).unwrap();
        assert_eq!(editor.text.to_string(), "\tfoo\n\n\t\n\t");
    }
    #[test]
    fn paste_and_indent_rebases_the_pasted_indentation() {
        let mut editor = Editor::new(Text::from("fn f() {\n    \n}\n"));
        select(&mut editor, &[(13, 13)]);
        editor.clipboard = "if a {\n    b();\n}".into();
        editor.run("paste_and_indent", &json!({})).unwrap();
        assert_eq!(
            editor.text.to_string(),
            "fn f() {\n    if a {\n        b();\n    }\n}\n"
        );
        assert_eq!(
            crate::editing::reindent_block("\t\tx\n\t\t\ty\n", "  ", false),
            "  x\n  \ty\n"
        );
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
            let context = EditorContext::new(&editor);
            assert_eq!(context.values("preceding_text"), vec![json!("before")]);
            assert_eq!(context.values("following_text"), vec![json!("after")]);
            assert_eq!(context.values("text"), vec![json!("SELECT")]);
        }
    }
}

#[cfg(test)]
mod round15_details {
    use super::*;
    use vel_platform::Size;

    #[test]
    fn viewport_many_carets_with_retained_edit_history() {
        let template = highlighted("let value = 42;\n");
        let language = template.language.borrow();
        let published = language.as_ref().unwrap().scopes.as_ref().unwrap();
        let source = format!("let value = 42; // {}\n", "padding ".repeat(24)).repeat(10_000);
        let mut e = Editor::new(Text::from(source.as_str()));
        for _ in 0..1_000 {
            let at = e.buffer.text().len_bytes();
            e.buffer.transact(
                e.id,
                vel_buffer::EditKind::Other,
                &[vel_text::Edit {
                    range: at..at,
                    text: " ",
                }],
                Selection::new(),
            );
        }
        let mut snapshot = (**published).clone();
        snapshot.version = e.buffer.change_id();
        snapshot.text = e.buffer.text().clone();
        snapshot.range = 0..snapshot.text.len_lines();
        snapshot.lines = (0..snapshot.text.len_lines())
            .map(|_| published.lines[0].clone())
            .collect();
        let root = language.as_ref().unwrap().root;
        let at = e.buffer.text().len_bytes();
        e.buffer.transact(
            e.id,
            vel_buffer::EditKind::Other,
            &[vel_text::Edit {
                range: at..at,
                text: " ",
            }],
            Selection::new(),
        );
        e.settings
            .set(Layer::View, "large_file.viewport_bytes", json!(1));
        e.sync();
        e.attach_language(
            crate::language::Assets::bundled().unwrap(),
            "Rust",
            Arc::new(vel_color_scheme::ColorScheme::velocity_dark()),
        )
        .unwrap();
        assert_eq!(
            e.features.highlighting,
            vel_largefile::HighlightMode::Viewport
        );
        e.selector_snapshots
            .borrow_mut()
            .push((e.buffer.id(), root, Arc::new(snapshot)));
        e.set_selection(Selection::from_regions(
            (0..10_000).map(|row| Region::caret(e.text.line_to_byte(row) + 6)),
        ));
        let start = std::time::Instant::now();
        e.run("move", &json!({"by":"characters","forward":true}))
            .unwrap();
        let arrow = start.elapsed();
        assert!(e.selector_points.borrow().values.is_empty());
        let start = std::time::Instant::now();
        assert!(EditorContext::new(&e).scopes_ready());
        assert_eq!(
            EditorContext::new(&e).selector_matches("string, comment"),
            vec![false; 10_000]
        );
        let _ = e.resolve_key(&Key::parse("\"").unwrap());
        let quote = start.elapsed();
        assert_eq!(e.regions().len(), 10_000);
        eprintln!("viewport 10k carets / 2 MiB / 1001 edits: arrow={arrow:?}, quote={quote:?}");
    }
    #[test]
    fn wrapped_home_end_extend_and_hard_boundaries() {
        let mut e = Editor::new(Text::from(
            "    alpha beta gamma delta epsilon zeta\nsecond",
        ));
        e.settings.set(Layer::View, "word_wrap", json!(true));
        let mut view = crate::view::View::new(
            Size {
                width: 160.,
                height: 240.,
            },
            8.,
            12.,
        );
        view.scene(&e);
        let at = 22;
        let segment = e.layout.borrow().visual_at(&e.text, at).1;
        assert!(segment.bytes.start > 4);
        e.set_caret(at, false);
        e.run("move_to", &json!({"to":"bol"})).unwrap();
        assert_eq!(e.caret, segment.bytes.start);
        e.run("move_to", &json!({"to":"bol"})).unwrap();
        assert_eq!(e.caret, 4);
        e.run("move_to", &json!({"to":"bol"})).unwrap();
        assert_eq!(e.caret, 0);
        e.set_caret(at, false);
        e.run("move_to", &json!({"to":"eol","extend":true}))
            .unwrap();
        assert_eq!(e.anchor, at);
        assert_eq!(e.caret, e.text.prev_grapheme_boundary(segment.bytes.end));
        assert_eq!(
            e.layout.borrow().visual_at(&e.text, e.caret).1.bytes,
            segment.bytes
        );
        e.run("move_to", &json!({"to":"eol","extend":true}))
            .unwrap();
        assert_eq!(e.caret, e.text.line_len_bytes(0));
        e.run("move_to", &json!({"to":"hardbol"})).unwrap();
        assert_eq!(e.caret, 0);
        e.run("move_to", &json!({"to":"hardeol"})).unwrap();
        assert_eq!(e.caret, e.text.line_len_bytes(0));
        e.settings.set(Layer::View, "word_wrap", json!(false));
        e.sync();
        view.scene(&e);
        e.set_caret(at, false);
        e.run("move_to", &json!({"to":"bol"})).unwrap();
        assert_eq!(e.caret, 4);
    }

    #[test]
    fn wrapped_end_stays_on_its_row_for_drawing_home_and_vertical_movement() {
        let mut e = Editor::new(Text::from(
            "    alpha beta gamma delta epsilon zeta\nsecond",
        ));
        e.settings.set(Layer::View, "word_wrap", json!(true));
        let mut view = crate::view::View::new(
            Size {
                width: 160.,
                height: 240.,
            },
            8.,
            12.,
        );
        view.scene(&e);
        e.set_caret(22, false);
        let (row, segment) = e.layout.borrow().visual_at(&e.text, e.caret);
        let before = view.rect_at(&e, e.caret);
        e.run("move_to", &json!({"to":"eol"})).unwrap();
        assert_eq!(view.rect_at(&e, e.caret).y, before.y);
        assert_eq!(e.layout.borrow().visual_at(&e.text, e.caret).0, row);
        e.run("move_to", &json!({"to":"bol"})).unwrap();
        assert_eq!(e.caret, segment.bytes.start);
        e.set_caret(22, false);
        e.run("move_to", &json!({"to":"eol"})).unwrap();
        e.run("move", &json!({"by":"lines","forward":true}))
            .unwrap();
        assert_eq!(e.layout.borrow().visual_at(&e.text, e.caret).0, row + 1);
        e.set_caret(22, false);
        e.run("move_to", &json!({"to":"eol"})).unwrap();
        e.run("move", &json!({"by":"lines","forward":false}))
            .unwrap();
        assert_eq!(e.layout.borrow().visual_at(&e.text, e.caret).0, row - 1);
    }
    #[test]
    fn smart_home_streams_a_huge_unicode_logical_line() {
        let e = Editor::new(Text::from(
            format!(" \t  {}", "界".repeat(400_000)).as_str(),
        ));
        assert_eq!(
            crate::view::visual_line_boundary(&e, e.text.floor_char_boundary(500), false, false),
            4
        );
        assert_eq!(crate::view::visual_line_boundary(&e, 4, false, false), 0);
    }
    #[test]
    fn viewport_preedit_retains_a_current_snapshot_once_for_many_carets() {
        let mut e = highlighted("let value = 42;\nlet another = 0;\n");
        let snapshot = e
            .language
            .borrow()
            .as_ref()
            .unwrap()
            .scopes
            .as_ref()
            .unwrap()
            .clone();
        e.settings
            .set(Layer::View, "large_file.viewport_bytes", json!(1));
        e.sync();
        let mut partial = (*snapshot).clone();
        partial.range = 0..1;
        partial.lines = partial.lines.iter().take(1).cloned().collect();
        e.language.borrow_mut().as_mut().unwrap().scopes = Some(Arc::new(partial));
        e.selector_snapshots.borrow_mut().clear();
        e.set_selection(Selection::from_regions([
            Region::caret(2),
            Region::caret(6),
            Region::caret(10),
        ]));
        e.run("insert", &json!({"characters":"x"})).unwrap();
        assert_eq!(e.selector_snapshots.borrow().len(), 1);
        assert!(e.selector_points.borrow().values.is_empty());
    }

    #[test]
    fn wrapped_boundaries_keep_unicode_multiple_carets_and_primary() {
        let mut e = Editor::new(Text::from(
            "  alpha 你好 beta gamma delta\n  alpha 你好 beta gamma delta",
        ));
        e.settings.set(Layer::View, "word_wrap", json!(true));
        crate::view::View::new(
            Size {
                width: 170.,
                height: 240.,
            },
            8.,
            12.,
        )
        .scene(&e);
        let second = e.text.line_to_byte(1);
        e.set_selection(Selection::from_regions([
            Region::caret(second + 20),
            Region::caret(20),
        ]));
        let first_target = crate::view::visual_line_boundary(&e, 20, false, false);
        let second_target = crate::view::visual_line_boundary(&e, second + 20, false, false);
        e.run("move_to", &json!({"to":"bol","extend":true}))
            .unwrap();
        assert_eq!(
            e.regions().iter().map(|r| r.b).collect::<Vec<_>>(),
            [first_target, second_target]
        );
        assert_eq!(e.caret, first_target);
        assert_eq!(e.anchor, 20);
        assert!(e.regions().iter().all(|r| e.text.is_char_boundary(r.b)));
    }
    fn highlighted(source: &str) -> Editor {
        let mut e = Editor::new(Text::from(source));
        e.attach_language(
            crate::language::Assets::bundled().unwrap(),
            "Rust",
            Arc::new(vel_color_scheme::ColorScheme::velocity_dark()),
        )
        .unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            e.poll_language();
            if e.language.borrow().as_ref().unwrap().scopes_ready() {
                return e;
            }
            assert!(std::time::Instant::now() < until);
            std::thread::park_timeout(std::time::Duration::from_millis(1));
        }
    }
    #[test]
    fn pending_scopes_use_published_strings_and_code_shifted_through_edits() {
        let source = "fn main() {\n let s = \"string\";\n    \n}\n";
        let mut e = highlighted(source);
        let inside = source.find("string").unwrap() + 2;
        e.set_caret(inside, false);
        assert!(EditorContext::new(&e).scopes_ready());
        assert_eq!(EditorContext::new(&e).selector_matches("string"), [true]);
        e.set_caret(0, false);
        e.run("insert", &json!({"characters":"// 你好\n"})).unwrap();
        let shift = "// 你好\n".len();
        e.set_caret(inside + shift, false);
        // Deliberately do not poll the new language service.
        assert!(EditorContext::new(&e).scopes_ready());
        assert_eq!(EditorContext::new(&e).selector_matches("string"), [true]);
        assert!(!e.key(&Key::parse("\"").unwrap()).unwrap());
        let code = source.find("    \n").unwrap() + shift + 2;
        e.set_caret(code, false);
        assert_eq!(
            EditorContext::new(&e).selector_matches("string, comment"),
            [false]
        );
        assert!(e.key(&Key::parse("\"").unwrap()).unwrap());
        assert_eq!(e.text.slice(code..code + 2).to_string(), "\"\"");
        e.run("undo", &json!({})).unwrap();
        e.set_caret(inside + shift, false);
        assert_eq!(EditorContext::new(&e).selector_matches("string"), [true]);
    }
    #[test]
    fn selectors_still_defer_when_no_scopes_have_ever_been_published() {
        let mut e = Editor::new(Text::from("fn main() {}"));
        e.attach_language(
            crate::language::Assets::bundled().unwrap(),
            "Rust",
            Arc::new(vel_color_scheme::ColorScheme::velocity_dark()),
        )
        .unwrap();
        assert!(!EditorContext::new(&e).scopes_ready());
        assert!(
            !e.context_matches(
                &serde_json::from_value::<Vec<vel_config::keymap::Context>>(
                    json!([{"key":"selector","operator":"not_equal","operand":"string"}])
                )
                .unwrap()
            )
        );
    }
}
