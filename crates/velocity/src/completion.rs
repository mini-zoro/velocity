//! Asynchronous completion presentation and transactional snippet fields.
use crate::editor::Editor;
use std::{
    collections::BTreeMap,
    ops::Range,
    sync::Arc,
    time::{Duration, Instant},
};
use vel_buffer::{ChangeId, EditKind, Region, Selection};
use vel_complete::{
    CompletionEngine, CompletionFormat, CompletionList, CompletionService, CompletionSettings,
    CompletionTrigger, Invocation, ViewSnapshot,
};
use vel_config::{Value, resources::Resources};
use vel_highlight::ScopeProvider;
use vel_platform::{Point, Rect};
use vel_render::scene::Layer;
use vel_snippet::{Environment, ExpansionOptions, Snippet, SnippetFile};

#[derive(Debug, Default)]
pub struct Catalog {
    pub snippets: BTreeMap<String, SnippetFile>,
    completions: Vec<String>,
    pub engine: Arc<CompletionEngine>,
}
impl Catalog {
    pub fn load(resources: &Resources, messages: &mut Vec<String>) -> Arc<Self> {
        let mut catalog = Self::default();
        for resource in resources.matching(".sublime-snippet") {
            match resource
                .text()
                .and_then(|s| SnippetFile::parse(s).map_err(|e| e.to_string()))
            {
                Ok(snippet) => {
                    catalog.snippets.insert(resource.path.clone(), snippet);
                }
                Err(e) => messages.push(format!("{}: {e}", resource.path)),
            }
        }
        for resource in resources.matching(".sublime-completions") {
            match resource.text() {
                Ok(s) => catalog.completions.push(s.into()),
                Err(e) => messages.push(e),
            }
        }
        catalog.engine = catalog.engine_for(None, messages);
        Arc::new(catalog)
    }
    pub fn engine_for(
        &self,
        index: Option<Arc<std::sync::RwLock<vel_index::Index>>>,
        messages: &mut Vec<String>,
    ) -> Arc<CompletionEngine> {
        let mut engine = CompletionEngine::new();
        if let Some(index) = index {
            engine.set_index(index);
        }
        for source in &self.completions {
            if let Err(e) = engine.add_completions(source) {
                messages.push(e.to_string());
            }
        }
        for snippet in self.snippets.values() {
            if let Err(e) = engine.add_snippet(snippet.clone()) {
                messages.push(e.to_string());
            }
        }
        Arc::new(engine)
    }
}

#[derive(Clone, Copy, Debug)]
enum CommitFallback {
    Tab,
    Newline,
}
#[derive(Debug)]
struct Committed {
    version: ChangeId,
    carets: Vec<usize>,
    ranges: Vec<Range<usize>>,
    items: Vec<vel_complete::CompletionItem>,
    selected: usize,
}
#[derive(Debug)]
pub struct Completion {
    committed: Option<Committed>,
    pub engine: Arc<CompletionEngine>,
    service: Option<CompletionService>,
    pub list: Option<CompletionList>,
    pub selected: usize,
    pub bounds: std::cell::Cell<Rect>,
    requested_carets: Vec<usize>,
    pending: Option<Invocation>,
    since: Instant,
    pub waiting: bool,
    commit_pending: Option<CommitFallback>,
    last_version: Option<ChangeId>,
    prepared: Option<std::sync::mpsc::Receiver<(ChangeId, Arc<vel_complete::BufferWords>)>>,
    words: Option<(ChangeId, Arc<vel_complete::BufferWords>)>,
}
impl Default for Completion {
    fn default() -> Self {
        Self {
            committed: None,
            engine: Arc::new(CompletionEngine::new()),
            service: None,
            list: None,
            selected: 0,
            bounds: Default::default(),
            requested_carets: Vec::new(),
            pending: None,
            since: Instant::now(),
            waiting: false,
            commit_pending: None,
            last_version: None,
            prepared: None,
            words: None,
        }
    }
}
impl Completion {
    pub fn visible(&self) -> bool {
        self.list.as_ref().is_some_and(|l| !l.items.is_empty())
    }
    pub fn busy(&self) -> bool {
        self.pending.is_some() || self.waiting
    }
    pub fn hide(&mut self) {
        if let Some(service) = &self.service {
            service.cancel();
        }
        self.list = None;
        self.pending = None;
        self.waiting = false;
        self.commit_pending = None;
        self.last_version = None;
    }
    pub fn set_engine(&mut self, engine: Arc<CompletionEngine>) {
        if Arc::ptr_eq(&self.engine, &engine) {
            return;
        }
        self.hide();
        self.service = None;
        self.committed = None;
        self.engine = engine;
    }
    pub fn request(&mut self, invocation: Invocation, commit: bool) {
        if let Some(service) = &self.service {
            service.cancel();
        }
        self.pending = Some(invocation);
        self.commit_pending = commit.then_some(if matches!(invocation, Invocation::Tab) {
            CommitFallback::Tab
        } else {
            CommitFallback::Newline
        });
        self.since = Instant::now();
        self.waiting = false;
    }
    pub fn edited(&mut self, character: Option<char>) {
        self.committed = None;
        let previous = self.list.take();
        self.hide();
        self.last_version = None;
        self.prepared = None;
        self.words = None;
        self.list = previous.filter(|_| character.is_some_and(|c| c.is_alphanumeric() || c == '_'));
        if let Some(character) = character {
            self.request(
                Invocation::Typing {
                    character,
                    idle: Duration::ZERO,
                },
                false,
            );
        }
    }
    fn poll(&mut self, editor: &Editor) -> Result<bool, String> {
        if editor.marked.is_some() {
            self.hide();
            return Ok(false);
        }
        let version = editor.buffer.change_id();
        let carets: Vec<_> = editor
            .buffer
            .view(editor.id)
            .sel()
            .iter()
            .map(|r| r.b)
            .collect();
        if self.last_version.is_some_and(|v| v != version)
            || (self.pending.is_none()
                && carets != self.requested_carets
                && (self.waiting || self.visible()))
        {
            self.hide();
        }
        let mut changed = false;
        if let Some(list) = self.service.as_ref().and_then(CompletionService::poll) {
            self.waiting = false;
            if list.version == version
                && list.view_id == editor.id.0
                && carets == self.requested_carets
            {
                self.selected = 0;
                self.list = (!list.items.is_empty()).then_some(list);
                changed = true;
            }
        }
        if let Some(invocation) = self.pending {
            let settings = settings(editor);
            if matches!(invocation, Invocation::Typing { .. })
                && self.since.elapsed() < settings.auto_complete_delay
            {
                return Ok(changed);
            }
            let scopes = editor
                .language
                .borrow()
                .as_ref()
                .and_then(|l| l.scope_snapshot_at(editor.caret));
            let Some(scopes) = scopes else {
                return Ok(changed);
            };
            if carets.iter().any(|p| scopes.scopes_at(*p).is_none()) {
                return Ok(changed);
            }
            if let Some(analysis) = editor
                .language
                .borrow()
                .as_ref()
                .and_then(|l| l.analysis.as_ref())
                .filter(|a| a.version == version)
            {
                self.words = Some((version, analysis.words.clone()));
            }
            if let Some((v, words)) = self.prepared.as_ref().and_then(|rx| rx.try_recv().ok()) {
                self.prepared = None;
                if v == version {
                    self.words = Some((v, words));
                }
            }
            if !self.words.as_ref().is_some_and(|(v, _)| *v == version) {
                if self.prepared.is_none() {
                    let text = editor.text.clone();
                    let (tx, rx) = std::sync::mpsc::channel();
                    std::thread::Builder::new()
                        .name("velocity-completion-words".into())
                        .spawn(move || {
                            let _ =
                                tx.send((version, Arc::new(vel_complete::BufferWords::new(&text))));
                        })
                        .map_err(|e| e.to_string())?;
                    self.prepared = Some(rx);
                }
                return Ok(changed);
            }
            if self.service.is_none() {
                self.service =
                    Some(CompletionService::new(self.engine.clone()).map_err(|e| e.to_string())?);
            }
            let view = ViewSnapshot {
                view_id: editor.id.0,
                version,
                text: editor.text.clone(),
                words: self.words.as_ref().unwrap().1.clone(),
                settings,
                invocation: match invocation {
                    Invocation::Typing { character, .. } => Invocation::Typing {
                        character,
                        idle: self.since.elapsed(),
                    },
                    other => other,
                },
                fields_active: editor.fields.is_some(),
            };
            self.requested_carets = carets;
            self.last_version = Some(version);
            self.waiting = true;
            self.pending = None;
            self.service
                .as_ref()
                .unwrap()
                .query(view, self.requested_carets.clone(), scopes);
        }
        Ok(changed)
    }
    pub fn navigate(&mut self, forward: bool) {
        if let Some(list) = &self.list {
            let n = list.items.len();
            if n == 0 {
                return;
            }
            self.selected = if forward {
                if self.selected + 1 < n {
                    self.selected + 1
                } else if list.cycle {
                    0
                } else {
                    n - 1
                }
            } else if self.selected > 0 {
                self.selected - 1
            } else if list.cycle {
                n - 1
            } else {
                0
            };
        }
    }
    pub fn popup_rect(&self) -> Option<Rect> {
        self.visible().then(|| self.bounds.get())
    }
    pub fn paint(
        &self,
        editor: &Editor,
        view: &crate::view::View,
        scene: &mut vel_render::scene::Scene,
    ) {
        let Some(list) = self.list.as_ref().filter(|_| self.visible()) else {
            self.bounds.set(Rect::default());
            return;
        };
        use vel_theme::{Element, ElementPath};
        let mut theme = editor.theme.borrow_mut();
        let n = popup_rows(list.items.len(), view.size.height);
        let height = n as f64 * 24. + 50.;
        let rect = popup_bounds(view.rect_at(editor, editor.caret), view.size, height);
        self.bounds.set(rect);
        let popup_path = [
            Element::new("window", &[]),
            Element::new("popup_control auto_complete_popup", &[]),
        ];
        let popup = theme.style(
            editor.ui_id(0xC000),
            &ElementPath(&popup_path),
            &editor.settings,
        );
        let mut layer = Layer::default();
        theme.surface(&mut layer, &popup, rect, rect);
        let first = self.selected.saturating_sub(n.saturating_sub(1));
        for (row, item) in list.items.iter().skip(first).take(n).enumerate() {
            let attrs = if first + row == self.selected {
                &["selected"][..]
            } else {
                &[]
            };
            let path = [
                popup_path[0],
                popup_path[1],
                Element::new("auto_complete", &[]),
                Element::new("table_row", attrs),
            ];
            let row_style = theme.style(
                editor.ui_id(0xC100_0000 + row as u32),
                &ElementPath(&path),
                &editor.settings,
            );
            let r = Rect {
                y: rect.y + row as f64 * 24.,
                height: 24.,
                ..rect
            };
            theme.surface(&mut layer, &row_style, r, rect);
            let label_path = [
                path[0],
                path[1],
                path[2],
                path[3],
                Element::new("auto_complete_label", &[]),
            ];
            let text = theme.style(
                editor.ui_id(0xC200_0000 + row as u32),
                &ElementPath(&label_path),
                &editor.settings,
            );
            let badge = if item.kind.letter.is_empty() {
                item.kind.name.chars().next().unwrap_or('·').to_string()
            } else {
                item.kind.letter.clone()
            };
            theme.text(
                &mut layer,
                &text,
                &format!("{badge}  {}", item.trigger),
                r.x + 8.,
                r.y + 17.,
                r,
            );
            if !item.annotation.is_empty() {
                let hint_path = [
                    path[0],
                    path[1],
                    path[2],
                    path[3],
                    Element::new("auto_complete_label auto_complete_hint", &[]),
                ];
                let hint = theme.style(
                    editor.ui_id(0xC300_0000 + row as u32),
                    &ElementPath(&hint_path),
                    &editor.settings,
                );
                theme.text(
                    &mut layer,
                    &hint,
                    &item.annotation,
                    r.x + r.width * 0.6,
                    r.y + 17.,
                    r,
                );
            }
        }
        scene.layers.push(layer);
        if let Some(item) = list
            .items
            .get(self.selected)
            .filter(|item| !item.details.is_empty())
        {
            let path = [
                popup_path[0],
                popup_path[1],
                Element::new("auto_complete_detail_pane", &[]),
                Element::new("auto_complete_details", &[]),
            ];
            let detail = theme.style(editor.ui_id(0xC400), &ElementPath(&path), &editor.settings);
            let mut palette = theme.palette.clone();
            palette.set(
                "--foreground",
                vel_theme::Color {
                    r: (detail.foreground[0] * 255.) as u8,
                    g: (detail.foreground[1] * 255.) as u8,
                    b: (detail.foreground[2] * 255.) as u8,
                    a: 255,
                },
            );
            let options = vel_minihtml::LayoutOptions {
                max_width: (rect.width - 16.).max(1.) as f32,
                root_font_size: detail.size,
                palette,
                extra_css: format!(
                    "html {{ font-family: {}; }}",
                    detail.family.as_deref().unwrap_or("system-ui")
                ),
            };
            if let Ok(html) = editor.html.borrow_mut().cached(&item.details, true) {
                html.layout(&options, theme.revision, &theme.metrics);
                html.paint(
                    scene,
                    Rect {
                        x: rect.x + 8.,
                        y: rect.y + n as f64 * 24. + 4.,
                        width: rect.width - 16.,
                        height: 42.,
                    },
                );
            }
        }
    }
    pub fn hit(&mut self, point: Point) -> bool {
        let rect = self.bounds.get();
        if !self.visible() || !vel_ui::contains(rect, point) {
            return false;
        }
        let n = popup_rows(self.list.as_ref().unwrap().items.len(), rect.height);
        let first = self.selected.saturating_sub(n.saturating_sub(1));
        let row = ((point.y - rect.y) / 24.) as usize;
        if row < n {
            self.selected = first + row;
            true
        } else {
            false
        }
    }
}
fn popup_rows(count: usize, height: f64) -> usize {
    count
        .min(8)
        .min(((height - 30.) / 24.).floor().max(1.) as usize)
}
pub fn popup_bounds(caret: Rect, size: vel_platform::Size, height: f64) -> Rect {
    let width = 460f64.min(size.width.max(0.));
    let height = height.min(size.height.max(0.));
    Rect {
        x: caret.x.min((size.width - width).max(0.)).max(0.),
        y: (if caret.y + caret.height + height <= size.height {
            caret.y + caret.height
        } else {
            caret.y - height
        })
        .clamp(0., (size.height - height).max(0.)),
        width,
        height,
    }
}

fn settings(editor: &Editor) -> CompletionSettings {
    let s = &editor.settings;
    let d = CompletionSettings::default();
    CompletionSettings {
        auto_complete: s.bool("auto_complete", d.auto_complete),
        auto_complete_selector: s
            .string("auto_complete_selector", &d.auto_complete_selector)
            .into(),
        auto_complete_triggers: s
            .get("auto_complete_triggers")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| {
                        Some(CompletionTrigger {
                            selector: v["selector"].as_str()?.into(),
                            characters: v["characters"].as_str()?.into(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        auto_complete_delay: Duration::from_millis(s.usize("auto_complete_delay", 50) as u64),
        auto_complete_size_limit: s.usize("auto_complete_size_limit", d.auto_complete_size_limit),
        auto_complete_commit_on_tab: s.bool("auto_complete_commit_on_tab", false),
        auto_complete_with_fields: s.bool("auto_complete_with_fields", false),
        auto_complete_cycle: s.bool("auto_complete_cycle", false),
        auto_complete_when_likely: s.bool("auto_complete_when_likely", true),
        auto_complete_use_index: s.bool("auto_complete_use_index", true),
        auto_complete_use_history: s.bool("auto_complete_use_history", true),
        auto_complete_preserve_order: s.bool("auto_complete_preserve_order", false),
        auto_complete_include_snippets: s.bool("auto_complete_include_snippets", true),
        auto_complete_include_snippets_when_typing: s
            .bool("auto_complete_include_snippets_when_typing", false),
        ..d
    }
}
impl Editor {
    pub fn poll_completion(&mut self) -> bool {
        let mut completion = std::mem::take(&mut self.completion);
        let result = completion.poll(self);
        let commit = if completion.busy() {
            None
        } else {
            completion.commit_pending.take()
        };
        self.completion = completion;
        if let Some(fallback) = commit {
            if self.completion.visible() {
                let _ = self.commit_completion();
            } else {
                match fallback {
                    CommitFallback::Newline => {
                        let _ = self.run("insert", &vel_config::json!({"characters":"\n"}));
                    }
                    CommitFallback::Tab => {
                        let size = self.preferences.tab_size.max(1);
                        let edits = self
                            .buffer
                            .view(self.id)
                            .sel()
                            .iter()
                            .map(|r| {
                                let text = if self.preferences.translate_tabs_to_spaces {
                                    let start =
                                        self.text.line_to_byte(self.text.byte_to_line(r.begin()));
                                    let prefix = self.text.slice(start..r.begin()).to_string();
                                    " ".repeat(
                                        size - crate::editor::cells_with_tab(&prefix, size) % size,
                                    )
                                } else {
                                    "\t".into()
                                };
                                (r.to_range(), text)
                            })
                            .collect();
                        self.apply_edits(edits, None, EditKind::InsertChars);
                        self.sync();
                    }
                }
            }
        }
        result.unwrap_or(false) || commit.is_some()
    }
    pub fn commit_completion(&mut self) -> Result<(), String> {
        self.commit_completion_with(Invocation::Manual)
    }
    fn commit_completion_with(&mut self, invocation: Invocation) -> Result<(), String> {
        let Some(list) = self.completion.list.take() else {
            return Ok(());
        };
        if list.version != self.buffer.change_id()
            || list.view_id != self.id.0
            || self
                .buffer
                .view(self.id)
                .sel()
                .iter()
                .map(|r| r.b)
                .collect::<Vec<_>>()
                != self.completion.requested_carets
        {
            self.completion.hide();
            self.completion.request(invocation, true);
            return Ok(());
        }
        let Some(item) = list.items.get(self.completion.selected) else {
            return Ok(());
        };
        self.completion.engine.record_commit(item);
        self.completion.hide();
        let ranges = self.insert_completion_item(item, &list.replace_ranges)?;
        self.completion.committed = Some(Committed {
            version: self.buffer.change_id(),
            carets: self
                .buffer
                .view(self.id)
                .sel()
                .iter()
                .map(|r| r.b)
                .collect(),
            ranges,
            items: list.items,
            selected: self.completion.selected,
        });
        Ok(())
    }
    fn cycle_committed(&mut self) {
        if self.completion.visible() {
            self.completion.navigate(true);
            return;
        }
        let Some(mut committed) = self.completion.committed.take() else {
            return;
        };
        if committed.version != self.buffer.change_id()
            || committed.carets
                != self
                    .buffer
                    .view(self.id)
                    .sel()
                    .iter()
                    .map(|r| r.b)
                    .collect::<Vec<_>>()
        {
            return;
        }
        if committed.items.is_empty() {
            return;
        }
        let next = (committed.selected + 1) % committed.items.len();
        let Ok(ranges) = self.insert_completion_item(&committed.items[next], &committed.ranges)
        else {
            return;
        };
        committed.ranges = ranges;
        committed.version = self.buffer.change_id();
        committed.carets = self
            .buffer
            .view(self.id)
            .sel()
            .iter()
            .map(|r| r.b)
            .collect();
        committed.selected = next;
        self.completion.committed = Some(committed);
    }
    fn insert_completion_item(
        &mut self,
        item: &vel_complete::CompletionItem,
        ranges: &[Range<usize>],
    ) -> Result<Vec<Range<usize>>, String> {
        if item.completion_format == CompletionFormat::Snippet {
            return self.expand_snippet(&item.completion, ranges);
        }
        let mut delta = 0isize;
        let selection = Selection::from_regions(ranges.iter().map(|r| {
            let start = r.start.wrapping_add_signed(delta);
            delta += item.completion.len() as isize - r.len() as isize;
            Region::caret(start + item.completion.len())
        }));
        self.apply_edits(
            ranges
                .iter()
                .map(|r| (r.clone(), item.completion.clone()))
                .collect(),
            Some(selection),
            EditKind::Other,
        );
        self.sync();
        Ok(self
            .buffer
            .view(self.id)
            .sel()
            .iter()
            .map(|r| r.b.saturating_sub(item.completion.len())..r.b)
            .collect())
    }
    pub fn expand_snippet(
        &mut self,
        source: &str,
        ranges: &[Range<usize>],
    ) -> Result<Vec<Range<usize>>, String> {
        self.completion.hide();
        self.completion.committed = None;
        let mut env = Environment::default();
        if let Some(path) = &self.snippet_path {
            env = env.with_path(path);
        }
        if let Some(language) = self.language.borrow().as_ref() {
            env = env.with_metadata(
                &language
                    .assets
                    .metadata
                    .metadata_for(&language.scopes(&self.text, self.caret)),
            );
        }
        let expansion = Snippet::parse(source)
            .and_then(|s| {
                s.expand(
                    &self.text,
                    ranges,
                    &env,
                    ExpansionOptions {
                        tab_size: self.preferences.tab_size,
                        translate_tabs_to_spaces: self.preferences.translate_tabs_to_spaces,
                    },
                )
            })
            .map_err(|e| e.to_string())?;
        let mut delta = 0isize;
        let mut bounds: Vec<Range<usize>> = expansion
            .edits
            .iter()
            .map(|e| {
                let start = e.range.start.wrapping_add_signed(delta);
                delta += e.text.len() as isize - e.range.len() as isize;
                start..start + e.text.len()
            })
            .collect();
        let final_selections = expansion.session.selections();
        let preserve_parent = self.fields.is_some() && expansion.session.active_field() == Some(0);
        if !preserve_parent {
            self.fields = None;
        }
        let selection = Selection::from_regions(final_selections.iter().cloned().map(Region::from));
        self.apply_edits(
            expansion
                .edits
                .into_iter()
                .map(|e| (e.range, e.text))
                .collect(),
            Some(selection),
            EditKind::Other,
        );
        if preserve_parent {
            for ((bound, old_final), current) in bounds
                .iter_mut()
                .zip(final_selections)
                .zip(self.buffer.view(self.id).sel().iter())
            {
                let width = bound.len();
                let relative = old_final.end - bound.start;
                let start = current.b.saturating_sub(relative);
                *bound = start..start + width;
            }
        } else {
            self.fields =
                (expansion.session.active_field() != Some(0)).then_some(expansion.session);
        }
        self.fields_version = self
            .fields
            .as_ref()
            .map(|_| (self.buffer.id(), self.buffer.change_id()));
        self.sync();
        Ok(bounds)
    }
    pub fn field_previous(&self) -> bool {
        self.fields.as_ref().is_some_and(|s| {
            s.active_field()
                != s.fields()
                    .iter()
                    .filter(|f| f.number > 0)
                    .map(|f| f.number)
                    .min()
        })
    }
    pub fn intelligence_command(&mut self, name: &str, args: &Value) -> Result<bool, String> {
        match name {
            "auto_complete" => self.completion.request(Invocation::Manual, false),
            "hide_auto_complete" => self.completion.hide(),
            "commit_completion" => self.commit_completion()?,
            "replace_completion_with_next_completion" => self.cycle_committed(),
            "move" if args["by"] == "lines" && self.completion.visible() => self
                .completion
                .navigate(args["forward"].as_bool().unwrap_or(false)),
            "clear_fields" => {
                self.fields = None;
                self.completion.hide();
            }
            "next_field" | "prev_field" => {
                self.completion.hide();
                if let Some(fields) = &mut self.fields {
                    let ranges = if name == "next_field" {
                        fields.next_field()
                    } else {
                        fields.prev_field()
                    };
                    let end = fields.active_field() == Some(0);
                    self.buffer.set_selection(
                        self.id,
                        Selection::from_regions(ranges.into_iter().map(Region::from)),
                    );
                    if end {
                        self.fields = None;
                    }
                }
            }
            "insert_snippet" => {
                let source = args["contents"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| {
                        args["name"]
                            .as_str()
                            .and_then(|n| self.snippets.get(n))
                            .map(|s| s.content.clone())
                    })
                    .ok_or("snippet needs valid contents or name")?;
                let ranges = self
                    .buffer
                    .view(self.id)
                    .sel()
                    .iter()
                    .map(|r| r.to_range())
                    .collect::<Vec<_>>();
                self.expand_snippet(&source, &ranges)?;
            }
            "insert_best_completion" => {
                if self.completion.visible() {
                    self.commit_completion_with(Invocation::Tab)?;
                } else if self.fields.is_some() {
                    return self.intelligence_command("next_field", args);
                } else {
                    let mut ranges = Vec::new();
                    let mut trigger = None;
                    for r in self.buffer.view(self.id).sel().iter() {
                        if !r.empty() {
                            return Ok(false);
                        }
                        let start = self.text.line_to_byte(self.text.byte_to_line(r.b));
                        let prefix = self.text.slice(start..r.b).to_string();
                        let word = prefix
                            .rsplit(|c: char| !c.is_alphanumeric() && c != '_')
                            .next()
                            .unwrap_or("");
                        if word.is_empty() {
                            return Ok(false);
                        }
                        if trigger.as_ref().is_some_and(|s: &String| s != word) {
                            return Ok(false);
                        }
                        trigger = Some(word.to_owned());
                        ranges.push(r.b - word.len()..r.b);
                    }
                    let content = self
                        .snippets
                        .values()
                        .find(|s| {
                            Some(&s.tab_trigger) == trigger.as_ref()
                                && (s.scope.is_empty()
                                    || self.language.borrow().as_ref().is_some_and(|l| {
                                        l.scopes_ready_at(&self.text, self.caret)
                                            && l.selector(&self.text, &s.scope, self.caret)
                                    }))
                        })
                        .map(|s| s.content.clone());
                    if let Some(content) = content {
                        self.expand_snippet(&content, &ranges)?;
                    } else if self.language.borrow().is_some() {
                        self.completion.request(Invocation::Tab, true);
                    } else {
                        return Ok(false);
                    }
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vel_config::{json, keymap::Key, settings::Layer};
    use vel_text::Text;
    fn ready(e: &mut Editor, completion: bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            e.poll_language();
            e.poll_completion();
            if e.language.borrow().as_ref().is_some_and(|l| l.ready())
                && (!completion || (!e.completion.busy() && e.completion.visible()))
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "completion/language timeout: {:?}",
                e.completion
            );
            std::thread::park_timeout(Duration::from_millis(1));
        }
    }
    fn editor(source: &str) -> Editor {
        let mut e = Editor::new(Text::from(source));
        e.attach_language(
            crate::language::Assets::bundled().unwrap(),
            "Rust",
            Arc::new(vel_color_scheme::ColorScheme::velocity_dark()),
        )
        .unwrap();
        e.set_caret(source.len(), false);
        ready(&mut e, false);
        e
    }
    #[test]
    fn fields_mirrors_unicode_multiple_carets_navigation_and_undo() {
        let mut e = editor("\n");
        e.set_selection(Selection::from_regions([
            Region::caret(0),
            Region::caret(1),
        ]));
        e.run(
            "insert_snippet",
            &json!({"contents":"${1:name}: $1 ${2:value} $0"}),
        )
        .unwrap();
        assert!(e.fields.is_some());
        e.run("insert", &json!({"characters":"你好🦀"})).unwrap();
        assert_eq!(
            e.text.to_string(),
            "你好🦀: 你好🦀 value \n你好🦀: 你好🦀 value "
        );
        e.run("next_field", &json!({})).unwrap();
        assert!(e.field_previous());
        e.run("prev_field", &json!({})).unwrap();
        assert!(!e.field_previous());
        e.run("insert", &json!({"characters":"item"})).unwrap();
        assert!(e.text.to_string().contains("item: item"));
        e.run("undo", &json!({})).unwrap();
        assert!(e.fields.is_none());
        assert!(e.text.to_string().contains("你好🦀: 你好🦀"));
    }
    #[test]
    fn snippet_field_escape_caret_departure_and_external_edits_cancel_safely() {
        let mut e = editor("prefix ");
        e.run(
            "insert_snippet",
            &json!({"contents":"${1:one} $1 ${2:two} $0"}),
        )
        .unwrap();
        assert!(e.key(&Key::parse("escape").unwrap()).unwrap());
        assert!(e.fields.is_none());
        e.run("insert_snippet", &json!({"contents":"${1:a} $1 $0"}))
            .unwrap();
        e.set_caret(0, false);
        assert!(e.fields.is_none());
        e.run("insert_snippet", &json!({"contents":"${1:a} $1 $0"}))
            .unwrap();
        e.apply_edits(
            vec![(e.text.len_bytes()..e.text.len_bytes(), "!".into())],
            None,
            EditKind::Other,
        );
        e.sync();
        assert!(e.fields.is_some());
        e.run("insert", &json!({"characters":"b"})).unwrap();
        assert!(e.text.to_string().starts_with("b b "));
    }
    #[test]
    fn asynchronous_popup_context_keys_mouse_and_stale_commits() {
        let mut e = editor("al");
        let mut engine = CompletionEngine::new();
        engine.add_completions(r#"{"scope":"source.rust","completions":[{"trigger":"alpha","contents":"alpha","kind":"function","annotation":"A","details":"<b>detail</b>"},"alpine"]}"#).unwrap();
        e.completion.set_engine(Arc::new(engine));
        e.settings
            .set(Layer::View, "auto_complete_cycle", json!(true));
        e.sync();
        e.run("auto_complete", &json!({})).unwrap();
        ready(&mut e, true);
        assert!(e.completion.list.as_ref().unwrap().items.len() >= 2);
        e.key(&Key::parse("down").unwrap()).unwrap();
        assert_eq!(e.completion.selected, 1);
        e.key(&Key::parse("up").unwrap()).unwrap();
        assert_eq!(e.completion.selected, 0);
        let mut view = crate::view::View::new(
            vel_platform::Size {
                width: 400.,
                height: 160.,
            },
            8.,
            12.,
        );
        let scene = view.scene(&e);
        assert!(!scene.layers.last().unwrap().text.is_empty());
        let rect = e.completion.bounds.get();
        assert!(e.completion.hit(Point {
            x: rect.x + 10.,
            y: rect.y + 5.
        }));
        e.run("commit_completion", &json!({})).unwrap();
        assert_eq!(e.text.to_string(), "alpha");
        e.run("replace_completion_with_next_completion", &json!({}))
            .unwrap();
        assert_ne!(e.text.to_string(), "alpha");
        e.run("undo", &json!({})).unwrap();
        e.run("undo", &json!({})).unwrap();
        ready(&mut e, false);
        e.run("auto_complete", &json!({})).unwrap();
        ready(&mut e, true);
        e.run("insert", &json!({"characters":"z"})).unwrap();
        let text = e.text.to_string();
        e.run("commit_completion", &json!({})).unwrap();
        assert_eq!(e.text.to_string(), text);
        e.run("auto_complete", &json!({})).unwrap();
        e.set_caret(0, false);
        e.poll_completion();
        assert!(!e.completion.visible());
    }
    #[test]
    fn stale_popup_commit_queries_the_new_prefix_and_tab_fallback_uses_tab_stops() {
        let mut e = editor("al");
        let mut engine = CompletionEngine::new();
        engine
            .add_completions(r#"{"scope":"source.rust","completions":["alpha","alpine"]}"#)
            .unwrap();
        e.completion.set_engine(Arc::new(engine));
        e.run("auto_complete", &json!({})).unwrap();
        ready(&mut e, true);
        e.run("insert", &json!({"characters":"p"})).unwrap();
        e.run("commit_completion", &json!({})).unwrap();
        assert_eq!(e.text.to_string(), "alp");
        let until = Instant::now() + Duration::from_secs(10);
        while e.text != "alpha" {
            e.poll_language();
            e.poll_completion();
            assert!(Instant::now() < until);
            std::thread::park_timeout(Duration::from_millis(1));
        }
        let mut e = editor("abc");
        e.settings
            .set(Layer::View, "translate_tabs_to_spaces", json!(true));
        e.sync();
        e.run("insert_best_completion", &json!({})).unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        while e.completion.busy() {
            e.poll_language();
            e.poll_completion();
            assert!(Instant::now() < until);
            std::thread::park_timeout(Duration::from_millis(1));
        }
        assert_eq!(e.text.to_string(), "abc ");
    }
    #[test]
    fn completion_and_auto_pairs_inside_a_field_keep_parent_mirrors_and_caret() {
        let mut e = editor("");
        e.run("insert_snippet", &json!({"contents":"${1:al} = $1; $0"}))
            .unwrap();
        ready(&mut e, false);
        let mut engine = CompletionEngine::new();
        engine
            .add_completions(
                r#"{"scope":"source.rust","completions":[{"trigger":"alpha","contents":"alpha"}]}"#,
            )
            .unwrap();
        e.completion.set_engine(Arc::new(engine));
        e.run("auto_complete", &json!({})).unwrap();
        ready(&mut e, true);
        e.run("commit_completion", &json!({})).unwrap();
        assert_eq!(e.text, "alpha = alpha; ");
        assert!(e.fields.is_some());
        e.run("insert_snippet", &json!({"contents":"($0)"}))
            .unwrap();
        assert_eq!(e.text, "alpha() = alpha(); ");
        assert_eq!(e.caret, 6);
        assert!(e.fields.is_some());
        e.run("insert", &json!({"characters":"你"})).unwrap();
        assert_eq!(e.text, "alpha(你) = alpha(你); ");
    }
    #[test]
    fn delay_commit_on_tab_and_manual_requests_follow_live_settings() {
        let mut e = editor("a");
        let mut engine = CompletionEngine::new();
        engine
            .add_completions(r#"{"scope":"source.rust","completions":["alpha","alpine"]}"#)
            .unwrap();
        e.completion.set_engine(Arc::new(engine));
        e.settings
            .set(Layer::View, "auto_complete_delay", json!(60000));
        e.sync();
        e.run("insert", &json!({"characters":"l"})).unwrap();
        ready(&mut e, false);
        assert!(!e.completion.visible());
        assert!(e.completion.busy());
        e.completion.since = Instant::now() - Duration::from_secs(61);
        ready(&mut e, true);
        e.settings
            .set(Layer::View, "auto_complete_commit_on_tab", json!(true));
        e.sync();
        e.key(&Key::parse("enter").unwrap()).unwrap();
        assert_eq!(e.text.to_string(), "al\n");
        assert!(!e.completion.visible());
        e.run("undo", &json!({})).unwrap();
        ready(&mut e, false);
        e.run("auto_complete", &json!({})).unwrap();
        ready(&mut e, true);
        e.key(&Key::parse("tab").unwrap()).unwrap();
        assert_eq!(e.text.to_string(), "alpha");
        e.run("auto_complete", &json!({})).unwrap();
        ready(&mut e, true);
        e.key(&Key::parse("escape").unwrap()).unwrap();
        assert!(!e.completion.visible());
    }
    #[test]
    fn explicit_character_trigger_overrides_selector_without_blocking_typing() {
        let mut e = editor("a");
        let mut engine = CompletionEngine::new();
        engine
            .add_completions(r#"{"scope":"source.rust","completions":["alpha"]}"#)
            .unwrap();
        e.completion.set_engine(Arc::new(engine));
        e.settings.set(Layer::View, "auto_complete_delay", json!(0));
        e.settings
            .set(Layer::View, "auto_complete_selector", json!("comment"));
        e.sync();
        e.run("insert", &json!({"characters":"l"})).unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        while e.completion.busy() {
            e.poll_language();
            e.poll_completion();
            assert!(Instant::now() < until);
            std::thread::park_timeout(Duration::from_millis(1));
        }
        assert!(!e.completion.visible());
        e.settings.set(
            Layer::View,
            "auto_complete_triggers",
            json!([{"selector":"source.rust","characters":"."}]),
        );
        e.sync();
        e.run("insert", &json!({"characters":"."})).unwrap();
        assert_eq!(e.text.to_string(), "al.");
        ready(&mut e, true);
    }
    #[test]
    fn popup_selection_remains_visible_in_a_short_split() {
        let mut e = editor("a");
        let mut engine = CompletionEngine::new();
        let words = (0..64)
            .map(|i| format!("candidate{i:02}"))
            .collect::<Vec<_>>();
        engine
            .add_completions(&json!({"scope":"source.rust","completions":words}).to_string())
            .unwrap();
        e.completion.set_engine(Arc::new(engine));
        e.run("auto_complete", &json!({})).unwrap();
        ready(&mut e, true);
        e.completion.selected = e.completion.list.as_ref().unwrap().items.len() - 1;
        let chosen = e.completion.list.as_ref().unwrap().items[e.completion.selected]
            .trigger
            .clone();
        let mut view = crate::view::View::new(
            vel_platform::Size {
                width: 400.,
                height: 100.,
            },
            8.,
            12.,
        );
        let scene = view.scene(&e);
        let bounds = e.completion.bounds.get();
        let popup = scene.layers.last().unwrap();
        assert!(popup.text.iter().any(|run| run.text.contains(&chosen)));
        let selected = popup
            .surfaces
            .iter()
            .filter_map(|s| match s {
                vel_render::scene::Surface::Fill(r) if r.rect.height == 24. => Some(r),
                _ => None,
            })
            .next_back()
            .expect("selected themed row");
        assert!(
            selected.rect.y >= bounds.y
                && selected.rect.y + selected.rect.height <= bounds.y + bounds.height
        );
    }
    #[test]
    fn another_clone_edit_cancels_stale_fields() {
        let mut workspace =
            crate::workspace::Workspace::new(Text::new(), None, None, false).unwrap();
        workspace
            .editor_mut()
            .run("insert_snippet", &json!({"contents":"${1:name} $1 $0"}))
            .unwrap();
        let source = workspace.active;
        workspace.clone_file().unwrap();
        let cloned = workspace.active;
        workspace.select(source);
        for _ in 0..3 {
            workspace.prepare_sheet(cloned).unwrap();
            workspace.prepare_sheet(source).unwrap();
        }
        assert!(
            workspace.editor().fields.is_some(),
            "painting clone views must keep the source field session"
        );
        workspace.select(cloned);
        workspace.editor_mut().set_caret(0, false);
        workspace
            .editor_mut()
            .run("insert", &json!({"characters":"x"}))
            .unwrap();
        workspace.select(source);
        assert!(workspace.editor().fields.is_none());
    }
    #[test]
    fn popup_flips_and_clamps() {
        let rect = popup_bounds(
            Rect {
                x: 390.,
                y: 150.,
                width: 2.,
                height: 20.,
            },
            vel_platform::Size {
                width: 400.,
                height: 200.,
            },
            120.,
        );
        assert!(rect.y < 150.);
        assert!(rect.x >= 0. && rect.x + rect.width <= 400.);
    }
    #[test]
    fn creating_a_user_snippet_preserves_runtime_folders_and_unsaved_text() {
        let data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let mut workspace = crate::workspace::Workspace::new(
            Text::from("unsaved"),
            Some(data.path().into()),
            None,
            false,
        )
        .unwrap();
        let mut panels = crate::panels::Panels::new(&workspace, false);
        panels
            .add_folder(&mut workspace, folder.path().into())
            .unwrap();
        std::fs::write(
            data.path().join("Packages/User/New.sublime-snippet"),
            "<snippet><content>hello $0</content><description>New resource</description></snippet>",
        )
        .unwrap();
        workspace.poll();
        assert!(
            workspace
                .palette
                .iter()
                .any(|p| p.caption == "Snippet: New resource")
        );
        assert_eq!(workspace.folders.len(), 1);
        assert_eq!(
            workspace.folders[0].path,
            folder.path().canonicalize().unwrap()
        );
        assert_eq!(workspace.editor().text, "unsaved");
    }
    #[test]
    fn package_snippets_are_in_palette_and_expand_tab_triggers() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("Packages/User");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(user.join("Mirror.sublime-snippet"),"<snippet><content><![CDATA[${1:label} = $1; $0]]></content><tabTrigger>mirror</tabTrigger><scope>source.rust</scope><description>Mirror fixture</description></snippet>").unwrap();
        let mut workspace = crate::workspace::Workspace::new(
            Text::from("mirror"),
            Some(dir.path().into()),
            None,
            false,
        )
        .unwrap();
        workspace.set_file_type("Rust").unwrap();
        workspace.editor_mut().set_caret(6, false);
        ready(workspace.editor_mut(), false);
        assert!(
            workspace
                .palette
                .iter()
                .any(|p| p.caption == "Snippet: Mirror fixture")
        );
        workspace
            .editor_mut()
            .key(&Key::parse("tab").unwrap())
            .unwrap();
        assert_eq!(workspace.editor().text.to_string(), "label = label; ");
        workspace
            .editor_mut()
            .run("insert", &json!({"characters":"field"}))
            .unwrap();
        assert_eq!(workspace.editor().text.to_string(), "field = field; ");
        let (window, documents) = workspace.snapshot().unwrap();
        let mut restored =
            crate::workspace::Workspace::new(Text::new(), Some(dir.path().into()), None, false)
                .unwrap();
        restored.restore_window(&window, &documents).unwrap();
        assert_eq!(restored.editor().text.to_string(), "field = field; ");
        assert!(restored.editor().fields.is_none());
        restored.editor_mut().run("undo", &json!({})).unwrap();
        assert_eq!(restored.editor().text.to_string(), "label = label; ");
    }
}
