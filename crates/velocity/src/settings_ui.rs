//! The graphical User-preferences sheet. Nothing here is initialized by editor windows
//! until the sheet is opened.
#[cfg(target_os = "macos")]
#[path = "settings_ui_view.rs"]
mod view;

#[cfg(target_os = "macos")]
mod model {
    use crate::{workbench, workspace::Workspace};
    use serde::Deserialize;
    use std::{
        collections::BTreeMap,
        path::PathBuf,
        sync::LazyLock,
        time::{Duration, Instant},
    };
    use vel_config::{Value, json::Document, settings::Layer};
    use vel_i18n::Translator;
    use vel_platform::{Command, Rect, TextInput};
    use vel_ui::{Scroll, input::Input};

    const METADATA: &str = include_str!("../resources/Packages/Default/settings-ui.json");
    const REFERENCE: &str = include_str!("settings_reference.json");
    const DELAY: Duration = Duration::from_millis(350);

    #[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub(super) enum Control {
        Bool,
        Number,
        Text,
        Enum,
        Array,
        Color,
        JsonOnly,
    }

    #[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
    #[serde(rename_all = "snake_case")]
    pub(super) enum Group {
        #[default]
        Editing,
        Appearance,
        Fonts,
        Files,
        Find,
        Completion,
        Sidebar,
        Build,
        Packages,
        Language,
        Developer,
    }

    impl Group {
        pub(super) fn name(self, tr: &Translator) -> std::borrow::Cow<'static, str> {
            match self {
                Self::Editing => tr.tr("Editing"),
                Self::Appearance => tr.tr("Appearance"),
                Self::Fonts => tr.tr("Fonts and Lines"),
                Self::Files => tr.tr("Files and Encoding"),
                Self::Find => tr.tr("Find"),
                Self::Completion => tr.tr("Completions"),
                Self::Sidebar => tr.tr("Sidebar and Index"),
                Self::Build => tr.tr("Build"),
                Self::Packages => tr.tr("Packages and Themes"),
                Self::Language => tr.tr("Language"),
                Self::Developer => tr.tr("Developer"),
            }
        }
    }

    #[derive(Debug, Deserialize)]
    struct Meta {
        group: Group,
        control: Control,
        range: Option<[f64; 2]>,
        step: Option<f64>,
        #[serde(default)]
        decimal: bool,
        #[serde(default)]
        aliases: Vec<String>,
        name: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum Effect {
        Immediate,
        NextWindow,
        NextDocument,
        NextOpen,
        NextSave,
        NextBuild,
        NextSession,
        NextFullScreen,
        NextVintageView,
    }

    impl Effect {
        fn label(&self, tr: &Translator) -> std::borrow::Cow<'static, str> {
            match self {
                Self::Immediate => tr.tr("Immediately"),
                Self::NextWindow => tr.tr("Next window"),
                Self::NextDocument => tr.tr("Next new document"),
                Self::NextOpen => tr.tr("Next file open"),
                Self::NextSave => tr.tr("Next save"),
                Self::NextBuild => tr.tr("Next build"),
                Self::NextSession => tr.tr("Next close or quit"),
                Self::NextFullScreen => tr.tr("Next full-screen transition"),
                Self::NextVintageView => tr.tr("When a view enters Vintage"),
            }
        }
    }

    #[derive(Debug, Deserialize)]
    struct Reference {
        description: String,
        effect: Effect,
    }

    #[derive(Debug)]
    struct Definition {
        key: String,
        name: String,
        description: String,
        effect: Effect,
        meta: Meta,
    }

    static DEFINITIONS: LazyLock<Vec<Definition>> = LazyLock::new(|| {
        let metadata = Document::parse(METADATA).expect("checked Settings UI metadata");
        let meta: BTreeMap<String, Meta> =
            serde_json::from_value(metadata.value).expect("checked Settings UI schema");
        let mut reference: BTreeMap<String, Reference> =
            serde_json::from_str(REFERENCE).expect("generated settings reference");
        meta.into_iter()
            .map(|(key, mut meta)| {
                let Reference {
                    description,
                    effect,
                } = reference.remove(&key).expect("checked reference coverage");
                let name = meta.name.take().unwrap_or_else(|| human_name(&key));
                Definition {
                    key,
                    name,
                    description,
                    effect,
                    meta,
                }
            })
            .collect()
    });

    fn human_name(key: &str) -> String {
        key.split(['_', '.'])
            .map(|word| match word {
                "ui" => "UI".into(),
                "git" => "Git".into(),
                "eof" => "EOF".into(),
                _ => word[..1].to_ascii_uppercase() + &word[1..],
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[derive(Debug)]
    pub(super) struct Row {
        def: &'static Definition,
        pub(super) value: Value,
        pub(super) modified: bool,
        pub(super) name: String,
        pub(super) description: String,
        pub(super) effect: String,
        search: String,
        pub(super) compatibility: bool,
    }

    impl Row {
        pub(super) fn key(&self) -> &'static str {
            &self.def.key
        }

        pub(super) fn control(&self) -> Control {
            self.def.meta.control
        }

        pub(super) fn editable(&self) -> bool {
            !self.compatibility
                && match self.control() {
                    Control::Bool => self.value.is_boolean(),
                    Control::Number => self.value.is_number(),
                    Control::Text => self.value.is_string(),
                    _ => false,
                }
        }

        pub(super) fn display(&self, tr: &Translator) -> String {
            match &self.value {
                Value::Bool(true) => tr.tr("On").into_owned(),
                Value::Bool(false) => tr.tr("Off").into_owned(),
                Value::String(s) if s.is_empty() => tr.tr("Empty").into_owned(),
                Value::String(s) => s.clone(),
                Value::Null => tr.tr("Default").into_owned(),
                v => v.to_string(),
            }
        }

        fn edit_value(&self) -> String {
            self.value
                .as_str()
                .map_or_else(|| self.value.to_string(), str::to_owned)
        }

        fn parse(&self, source: &str, tr: &Translator) -> Result<Value, String> {
            let metadata = &self.def.meta;
            if self.control() == Control::Number {
                let [min, max] = metadata.range.expect("checked number range");
                let source = source.trim();
                let value = if metadata.decimal {
                    source
                        .parse::<f64>()
                        .ok()
                        .filter(|n| n.is_finite())
                        .map(Value::from)
                } else if min >= 0. {
                    source.parse::<u64>().ok().map(Value::from)
                } else {
                    source.parse::<i64>().ok().map(Value::from)
                };
                let args = [("min", min.to_string()), ("max", max.to_string())];
                return value
                    .filter(|value| value.as_f64().is_some_and(|n| (min..=max).contains(&n)))
                    .ok_or_else(|| {
                        if metadata.decimal {
                            tr.tr_fmt("Enter a number from {min} to {max}.", &args)
                        } else {
                            tr.tr_fmt("Enter a whole number from {min} to {max}.", &args)
                        }
                    });
            }
            if source.chars().any(char::is_control) {
                return Err(tr
                    .tr("Enter single-line text without control characters.")
                    .into_owned());
            }
            match self.key() {
                "default_encoding"
                    if vel_fileio::encoding_format(source, vel_fileio::LineEnding::Lf).is_err() =>
                {
                    return Err(tr.tr("Enter a supported encoding name.").into_owned());
                }
                "fallback_encoding" if vel_fileio::fallback_encoding(source).1.is_some() => {
                    return Err(tr.tr("Enter a supported encoding name.").into_owned());
                }
                "auto_complete_selector" | "spelling_selector"
                    if !source.is_empty() && vel_scope::Selector::parse(source).is_err() =>
                {
                    return Err(tr.tr("Enter a valid scope selector.").into_owned());
                }
                "result_file_regex" | "result_line_regex"
                    if !source.is_empty()
                        && vel_build::ResultParser::new(Some(source), None, PathBuf::new())
                            .is_err() =>
                {
                    return Err(tr.tr("Enter a valid regular expression.").into_owned());
                }
                _ => {}
            }
            Ok(Value::String(source.to_owned()))
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum Focus {
        Search,
        Sidebar,
        List,
        Edit,
    }

    #[derive(Clone, Copy, Debug)]
    pub(super) enum Target {
        Search,
        Modified,
        Json,
        Repair,
        Category(Option<Group>),
        #[allow(dead_code)] // Also used by the native application.
        Row(usize),
        Control(usize),
        Step(usize, bool),
        Reset(usize),
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub(crate) enum Navigation {
        #[default]
        Stay,
        Close,
        Json,
        Repair,
    }

    #[derive(Debug)]
    pub(super) struct Edit {
        pub(super) row: usize,
        pub(super) input: Input,
        pub(super) error: Option<String>,
        // Latest validated draft only. Preedit never becomes a publication.
        pending: Option<(Instant, Value)>,
    }

    #[derive(Debug)]
    pub(crate) struct Page {
        pub(super) path: Option<PathBuf>,
        pub(super) rows: Vec<Row>,
        pub(super) filtered: Vec<usize>,
        pub(super) groups: BTreeMap<Group, usize>,
        pub(super) modified_count: usize,
        pub(super) modified_only: bool,
        pub(super) category: Option<Group>,
        pub(super) focus: Focus,
        pub(super) selected: usize,
        pub(super) search: Input,
        pub(super) edit: Option<Edit>,
        pub(super) error: Option<String>,
        pub(super) scroll: Scroll,
        pub(super) category_scroll: Scroll,
        pub(super) hits: Vec<(Rect, Target)>,
        pub(super) list_bounds: Rect,
        pub(super) sidebar_bounds: Rect,
        pub(super) bounds: Rect,
        value_revision: Option<(u64, u64, u64)>,
        file_revision: Option<u64>,
    }

    impl Page {
        pub(crate) fn new(workspace: &Workspace) -> Self {
            let mut page = Self {
                path: workspace.data_path().map(workbench::preferences_path),
                rows: Vec::new(),
                filtered: Vec::new(),
                groups: BTreeMap::new(),
                modified_count: 0,
                modified_only: false,
                category: Some(Group::Editing),
                focus: Focus::Search,
                selected: 0,
                search: Input::default(),
                edit: None,
                error: None,
                scroll: Scroll::default(),
                category_scroll: Scroll::default(),
                hits: Vec::new(),
                list_bounds: Rect::default(),
                sidebar_bounds: Rect::default(),
                bounds: Rect::default(),
                value_revision: None,
                file_revision: None,
            };
            page.refresh(workspace);
            page
        }

        fn check_file(&mut self, tr: &Translator) {
            self.error = match &self.path {
                Some(path) => workbench::user_preferences_source(path)
                    .and_then(|text| Document::parse(&text))
                    .and_then(|doc| {
                        doc.value.is_object().then_some(()).ok_or_else(|| {
                            tr.tr("Expected a JSON object at line 1, column 1.")
                                .into_owned()
                        })
                    })
                    .err(),
                None => Some(tr.tr("No User preferences file is available.").into_owned()),
            };
            if self.error.is_some() {
                if self.focus == Focus::Edit {
                    self.focus = Focus::List;
                }
                self.edit = None;
            }
        }

        pub(crate) fn refresh(&mut self, workspace: &Workspace) {
            let tr = &workspace.translations;
            if self.file_revision != Some(workspace.resource_revision) {
                self.file_revision = Some(workspace.resource_revision);
                self.check_file(tr);
            }
            let settings = &workspace.settings;
            let revision = (
                settings.cache_identity(),
                tr.revision(),
                workspace.resource_revision,
            );
            if self.value_revision == Some(revision) {
                return;
            }
            self.value_revision = Some(revision);
            let selected = self.selected_row();
            self.groups.clear();
            self.modified_count = 0;
            self.rows.clear();
            let user = settings.layer(Layer::User);
            let platform = settings.layer(Layer::Platform);
            let defaults = settings.layer(Layer::Default);
            for def in DEFINITIONS.iter() {
                let key = def.key.as_str();
                let modified = user.contains_key(key);
                *self.groups.entry(def.meta.group).or_default() += usize::from(modified);
                self.modified_count += usize::from(modified);
                let name = tr.tr(&def.name).into_owned();
                let description = tr.tr(&def.description).into_owned();
                let mut search = format!(
                    "{key}\n{}\n{name}\n{}\n{description}",
                    def.name, def.description
                );
                for alias in &def.meta.aliases {
                    search.push('\n');
                    search.push_str(alias);
                }
                self.rows.push(Row {
                    def,
                    value: user
                        .get(key)
                        .or_else(|| platform.get(key))
                        .or_else(|| defaults.get(key))
                        .cloned()
                        .unwrap_or_default(),
                    name,
                    description,
                    effect: tr.tr_fmt(
                        "Takes effect: {time}",
                        &[("time", def.effect.label(tr).into_owned())],
                    ),
                    search: search.to_lowercase(),
                    modified,
                    compatibility: def
                        .description
                        .contains("accepted for compatibility, no effect in Velocity"),
                });
            }
            self.filter(selected);
        }

        fn filter(&mut self, selected: Option<usize>) {
            let query = self.search.value().to_lowercase();
            self.filtered.clear();
            self.filtered
                .extend(self.rows.iter().enumerate().filter_map(|(i, row)| {
                    ((!self.modified_only || row.modified)
                        && if query.trim().is_empty() {
                            self.category
                                .is_none_or(|group| group == row.def.meta.group)
                        } else {
                            query
                                .split_whitespace()
                                .all(|part| row.search.contains(part))
                        })
                    .then_some(i)
                }));
            if let Some(exact) = self
                .filtered
                .iter()
                .position(|&i| self.rows[i].key() == query.trim())
            {
                self.filtered[..=exact].rotate_right(1);
            }
            self.selected = selected
                .and_then(|row| self.filtered.iter().position(|&i| i == row))
                .unwrap_or_else(|| {
                    if selected.is_some() {
                        self.selected.min(self.filtered.len().saturating_sub(1))
                    } else {
                        0
                    }
                });
            self.scroll.rows = self.filtered.len();
            self.scroll.scroll(0.);
        }

        fn selected_row(&self) -> Option<usize> {
            self.filtered.get(self.selected).copied()
        }

        pub(crate) fn focus_search(&mut self, workspace: &mut Workspace) {
            if !self.finish_edit(workspace) {
                self.edit = None;
            }
            self.focus = Focus::Search;
            self.search.command(Command::SelectAll);
        }

        pub(crate) fn input(&self) -> Option<&Input> {
            match self.focus {
                Focus::Search => Some(&self.search),
                Focus::Edit => self.edit.as_ref().map(|edit| &edit.input),
                _ => None,
            }
        }

        pub(crate) fn input_mut(&mut self) -> Option<&mut Input> {
            match self.focus {
                Focus::Search => Some(&mut self.search),
                Focus::Edit => self.edit.as_mut().map(|edit| &mut edit.input),
                _ => None,
            }
        }

        pub(crate) fn text_input(&mut self, tr: &Translator, event: TextInput, now: Instant) {
            let Some(input) = self.input_mut() else {
                return;
            };
            let before = input.value();
            input.text_input(event);
            self.input_changed(tr, before, now);
        }

        fn input_changed(&mut self, tr: &Translator, before: String, now: Instant) {
            if self.focus == Focus::Search {
                if before != self.search.value() {
                    self.scroll.offset = 0.;
                    self.filter(None);
                }
            } else if let Some(edit) = &mut self.edit {
                let after = edit.input.value();
                if edit.input.composing() || after == before {
                    return;
                }
                match self.rows[edit.row].parse(&after, tr) {
                    Ok(value) => {
                        edit.error = None;
                        edit.pending = Some((now + DELAY, value));
                    }
                    Err(error) => {
                        edit.error = Some(error);
                        edit.pending = None;
                    }
                }
            }
        }

        pub(crate) fn deadline(&self) -> Option<Instant> {
            let edit = self.edit.as_ref().filter(|edit| !edit.input.composing())?;
            edit.pending.as_ref().map(|(when, _)| *when)
        }

        pub(crate) fn flush_due(&mut self, workspace: &mut Workspace, now: Instant) -> bool {
            self.refresh(workspace);
            if !self.deadline().is_some_and(|due| due <= now) {
                return false;
            }
            if let Some((row, value)) = self
                .edit
                .as_mut()
                .and_then(|edit| edit.pending.take().map(|(_, value)| (edit.row, value)))
            {
                self.write(workspace, row, Some(value));
            }
            true
        }

        fn write(&mut self, workspace: &mut Workspace, row: usize, value: Option<Value>) {
            let key = self.rows[row].key();
            let previous = workspace.settings.layer(Layer::User).get(key);
            if previous == value.as_ref() || self.error.is_some() {
                return;
            }
            self.check_file(&workspace.translations);
            if self.error.is_some() {
                return;
            }
            if let Err(error) = workspace.set_user_preference(key, value) {
                self.error = Some(error);
                self.edit = None;
                self.focus = Focus::List;
            } else {
                workspace.sync_ui_language();
                self.refresh(workspace);
            }
        }

        pub(crate) fn finish_edit(&mut self, workspace: &mut Workspace) -> bool {
            let Some(edit) = &mut self.edit else {
                return true;
            };
            if edit.error.is_some() || edit.input.composing() {
                return false;
            }
            let edit = self.edit.take().unwrap();
            self.focus = Focus::List;
            if let Some((_, value)) = edit.pending {
                self.write(workspace, edit.row, Some(value));
            }
            true
        }

        fn select(&mut self, id: usize) {
            if let Some(index) = self.filtered.iter().position(|&r| r == id) {
                self.selected = index;
                self.scroll.reveal(index);
            }
        }

        fn begin_edit(&mut self, row: usize) {
            self.focus = Focus::Edit;
            let mut input = Input::default();
            input.set(&self.rows[row].edit_value());
            self.edit = Some(Edit {
                row,
                input,
                error: None,
                pending: None,
            });
        }

        fn activate(&mut self, workspace: &mut Workspace, row: usize, toggle_only: bool) {
            if self.error.is_some() || !self.rows[row].editable() {
                return;
            }
            match self.rows[row].control() {
                Control::Bool => {
                    let next = !self.rows[row].value.as_bool().unwrap();
                    self.write(workspace, row, Some(Value::Bool(next)));
                }
                Control::Number | Control::Text if !toggle_only => self.begin_edit(row),
                _ => {}
            }
        }

        fn step(&mut self, workspace: &mut Workspace, row: usize, up: bool, now: Instant) {
            if self.error.is_some() || !self.rows[row].editable() {
                return;
            }
            let edit = self.edit.as_ref();
            let candidate = edit
                .filter(|e| e.row == row)
                .and_then(|e| e.input.value().trim().parse::<f64>().ok())
                .filter(|n| n.is_finite())
                .or_else(|| self.rows[row].value.as_f64());
            let Some(current) = candidate else { return };
            let meta = &self.rows[row].def.meta;
            let [min, max] = meta.range.unwrap();
            let step = meta.step.unwrap();
            let next = (current + if up { step } else { -step }).clamp(min, max);
            let text = if meta.decimal {
                format!("{next:.4}")
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_owned()
            } else {
                format!("{next:.0}")
            };
            if self.edit.as_ref().is_none_or(|edit| edit.row != row) {
                self.begin_edit(row);
            }
            if let Some(edit) = &mut self.edit {
                let before = edit.input.value();
                edit.input.set(&text);
                self.input_changed(&workspace.translations, before, now);
            }
        }

        fn category(&mut self, group: Option<Group>) {
            self.category = group;
            self.search.set("");
            self.scroll.offset = 0.;
            self.filter(None);
        }

        pub(crate) fn pointer(
            &mut self,
            workspace: &mut Workspace,
            point: vel_platform::Point,
            extend: bool,
            now: Instant,
        ) -> Navigation {
            self.refresh(workspace);
            use Target::*;
            let Some(target) =
                self.hits.iter().rev().find_map(|(bounds, target)| {
                    vel_ui::contains(*bounds, point).then_some(*target)
                })
            else {
                return Navigation::Stay;
            };
            match target {
                Repair => return Navigation::Repair,
                Json => {
                    if !self.finish_edit(workspace) {
                        self.edit = None;
                        self.focus = Focus::List;
                    }
                    return Navigation::Json;
                }
                Control(row) if self.edit.as_ref().is_some_and(|edit| edit.row == row) => {
                    self.edit.as_mut().unwrap().input.click_at(point, extend);
                    return Navigation::Stay;
                }
                Step(row, up) if self.edit.as_ref().is_some_and(|edit| edit.row == row) => {
                    self.step(workspace, row, up, now);
                    return Navigation::Stay;
                }
                Reset(row) if self.edit.as_ref().is_some_and(|edit| edit.row == row) => {
                    self.edit = None;
                    self.focus = Focus::List;
                }
                _ => {
                    if !self.finish_edit(workspace) {
                        return Navigation::Stay;
                    }
                }
            }
            match target {
                Search => {
                    self.focus = Focus::Search;
                    self.search.click_at(point, extend);
                }
                Modified => {
                    self.show_modified();
                }
                Category(group) => {
                    self.focus = Focus::Sidebar;
                    self.category(group);
                }
                Row(row) => {
                    self.focus = Focus::List;
                    self.select(row);
                }
                Control(row) => {
                    self.focus = Focus::List;
                    self.select(row);
                    self.activate(workspace, row, false);
                }
                Step(row, up) => {
                    self.select(row);
                    self.step(workspace, row, up, now);
                }
                Reset(row) => {
                    self.focus = Focus::List;
                    self.select(row);
                    self.write(workspace, row, None);
                }
                Json | Repair => unreachable!(),
            }
            Navigation::Stay
        }

        pub(crate) fn toggle(&mut self, workspace: &mut Workspace) {
            if self.focus == Focus::List
                && let Some(row) = self.selected_row()
            {
                self.activate(workspace, row, true);
            }
        }

        pub(crate) fn show_modified(&mut self) {
            self.modified_only = !self.modified_only;
            self.filter(self.selected_row());
        }

        pub(crate) fn command(
            &mut self,
            workspace: &mut Workspace,
            cmd: Command,
            now: Instant,
        ) -> Navigation {
            self.refresh(workspace);
            if self.input().is_some_and(Input::composing) {
                if cmd == Command::Cancel {
                    self.input_mut().unwrap().cancel_composition();
                }
                return Navigation::Stay;
            }
            use Command::*;
            match cmd {
                Cancel => match self.focus {
                    Focus::Edit => {
                        self.edit = None;
                        self.focus = Focus::List;
                    }
                    Focus::List => self.focus = Focus::Sidebar,
                    Focus::Search | Focus::Sidebar => return Navigation::Close,
                },
                Tab | Backtab => {
                    if self.finish_edit(workspace) {
                        self.focus = match (self.focus, cmd) {
                            (Focus::Search, Tab) | (Focus::List, Backtab) => Focus::Sidebar,
                            (Focus::Sidebar, Tab) | (Focus::Search, Backtab) => Focus::List,
                            _ => Focus::Search,
                        };
                    }
                }
                Up(false) | Down(false) if self.focus == Focus::Sidebar => {
                    let categories: Vec<_> = std::iter::once(None)
                        .chain(self.groups.keys().copied().map(Some))
                        .collect();
                    let current = categories
                        .iter()
                        .position(|g| *g == self.category)
                        .unwrap_or(0);
                    let index = current
                        .saturating_add_signed(if cmd == Down(false) { 1 } else { -1 })
                        .min(categories.len() - 1);
                    self.category(categories[index]);
                    self.category_scroll.reveal(index);
                }
                Up(false) | Down(false) | PageUp | PageDown
                    if self.focus == Focus::List || self.focus == Focus::Search =>
                {
                    let delta = match cmd {
                        Up(false) => -1,
                        PageUp => -((self.scroll.viewport / self.scroll.row_height.max(1.))
                            as isize)
                            .max(1),
                        PageDown => ((self.scroll.viewport / self.scroll.row_height.max(1.))
                            as isize)
                            .max(1),
                        _ => 1,
                    };
                    if self.focus == Focus::Search {
                        self.focus = Focus::List;
                    } else {
                        self.selected = self
                            .selected
                            .saturating_add_signed(delta)
                            .min(self.filtered.len().saturating_sub(1));
                    }
                    self.scroll.reveal(self.selected);
                }
                Up(false) | Down(false) if self.focus == Focus::Edit => {
                    if let Some(edit) = &self.edit
                        && self.rows[edit.row].control() == Control::Number
                    {
                        self.step(workspace, edit.row, cmd == Up(false), now);
                    }
                }
                Newline | LiteralNewline => match self.focus {
                    Focus::Edit => {
                        self.finish_edit(workspace);
                    }
                    Focus::Search | Focus::Sidebar => {
                        self.focus = Focus::List;
                        self.scroll.reveal(self.selected);
                    }
                    Focus::List => {
                        if let Some(row) = self.selected_row() {
                            self.activate(workspace, row, false);
                        }
                    }
                },
                DeleteBackward | DeleteForward if self.focus == Focus::List => {
                    if let Some(row) = self.selected_row() {
                        self.write(workspace, row, None);
                    }
                }
                _ => {
                    if let Some(input) = self.input_mut() {
                        let before = input.value();
                        input.command(cmd);
                        self.input_changed(&workspace.translations, before, now);
                    }
                }
            }
            Navigation::Stay
        }

        pub(crate) fn scroll_at(&mut self, point: vel_platform::Point, delta: f64) {
            if vel_ui::contains(self.sidebar_bounds, point) {
                self.category_scroll.scroll(delta);
            } else if self.focus != Focus::Edit {
                self.scroll.scroll(delta);
            }
        }

        pub(crate) fn search_for(&mut self, query: &str) {
            self.search.set(query);
            self.scroll.offset = 0.;
            self.filter(None);
        }

        #[cfg(test)]
        pub(crate) fn results(&self) -> impl Iterator<Item = (&str, &str)> {
            self.filtered
                .iter()
                .map(|&i| (self.rows[i].key(), self.rows[i].def.name.as_str()))
        }

        pub(crate) fn read_only(&self) -> Option<&str> {
            self.error.as_deref()
        }

        /// The text audit excludes setting data, but still checks every piece of chrome.
        pub(crate) fn is_row_data(&self, drawn: &str, tr: &Translator) -> bool {
            let prefix = drawn.strip_suffix('…');
            let matches = |s: &str| {
                let s = s.replace(['\n', '\r', '\t'], " ");
                s == drawn || prefix.is_some_and(|p| !p.is_empty() && s.starts_with(p))
            };
            matches(&self.search.value())
                || self.filtered.iter().any(|&id| {
                    let row = &self.rows[id];
                    matches(row.key())
                        || matches(&row.name)
                        || matches(&row.description)
                        || matches(&row.display(tr))
                })
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use vel_config::{json, settings::validate_layer};

        #[test]
        fn settings_ui_metadata_matches_defaults_and_allowed_types() {
            // The reference generator validates duplicate keys, orphans and coverage without
            // losing duplicates during deserialization. Test the runtime schema and validator too.
            let meta = Document::parse(METADATA).unwrap().value;
            let source = include_str!("../resources/Packages/Default/Preferences.sublime-settings");
            let defaults = Document::parse(source).unwrap().value;
            let defaults = defaults.as_object().unwrap();
            let definitions = &*DEFINITIONS;
            assert_eq!(definitions.len(), defaults.len());
            let reference: BTreeMap<String, Reference> = serde_json::from_str(REFERENCE).unwrap();
            assert_eq!(
                reference.keys().collect::<Vec<_>>(),
                defaults.keys().collect::<Vec<_>>()
            );
            let mut editable_count = 0;
            for definition in definitions {
                let key = &definition.key;
                let default = &defaults[key];
                let compatible = |value: &Value| {
                    let mut layer = json!({key: value}).as_object().unwrap().clone();
                    assert!(
                        validate_layer(&mut layer, defaults).is_empty(),
                        "{key}: {value} is not accepted by the settings reader type table",
                    );
                };
                compatible(default);
                let row = Row {
                    def: definition,
                    value: default.clone(),
                    modified: false,
                    name: String::new(),
                    description: String::new(),
                    effect: String::new(),
                    search: String::new(),
                    compatibility: false,
                };
                match row.control() {
                    Control::Bool => {
                        compatible(&Value::Bool(!default.as_bool().unwrap()));
                    }
                    Control::Text => {
                        let edited = row
                            .parse(default.as_str().unwrap(), &Translator::default())
                            .unwrap_or_else(|e| panic!("{key}: {e}"));
                        compatible(&edited);
                    }
                    Control::Number => {
                        let [min, max] = definition.meta.range.unwrap();
                        let step = definition.meta.step.unwrap();
                        assert!(
                            step > 0. && (min..=max).contains(&default.as_f64().unwrap()),
                            "{key}"
                        );
                        let edited = row
                            .parse(&(min + step).to_string(), &Translator::default())
                            .unwrap_or_else(|e| panic!("{key}: {e}"));
                        compatible(&edited);
                    }
                    Control::Array => assert!(default.is_array(), "{key}"),
                    Control::Color => assert!(default.is_string(), "{key}"),
                    Control::JsonOnly => assert!(default.is_null() || default.is_object(), "{key}"),
                    Control::Enum => {
                        let choices = meta[key]["choices"].as_array().unwrap();
                        assert!(!choices.is_empty(), "{key}");
                        for (i, value) in choices.iter().enumerate() {
                            compatible(value);
                            assert!(!choices[..i].contains(value), "{key}: duplicate {value}");
                        }
                        // draw_white_space also accepts a combination of these modes as an array.
                        if let Some(values) = default.as_array() {
                            assert!(values.iter().all(|v| choices.contains(v)), "{key}");
                        } else {
                            assert!(
                                choices.contains(default),
                                "{key}: missing default {default}"
                            );
                        }
                    }
                }
                editable_count += usize::from(row.editable());
                if let Some(conditions) = meta[key]["show_when"].as_object() {
                    for condition in conditions.keys() {
                        assert!(
                            defaults.contains_key(condition),
                            "{key}: {condition} not shipped"
                        );
                    }
                }
            }
            assert!(editable_count > 100);
        }

        #[test]
        fn settings_ui_enum_choices_match_reader_modes() {
            let meta = Document::parse(METADATA).unwrap().value;
            // These are the explicit cases in the indicated reader; arbitrary strings
            // accepted by type validation are not necessarily actionable choices.
            let contracts: &[(&str, Value)] = &[
                // view.rs: whitespace, caret_alpha, bar overlay, layout
                ("word_wrap", json!(["auto", true, false])),
                ("wrap_width_style", json!(["auto", "fixed"])),
                (
                    "draw_white_space",
                    json!(["none", "selection", "leading", "trailing", "all", null]),
                ),
                (
                    "draw_unicode_white_space",
                    json!(["none", "punctuation", "all"]),
                ),
                (
                    "caret_style",
                    json!(["solid", "blink", "smooth", "phase", "block", "underline"]),
                ),
                (
                    "overlay_scroll_bars",
                    json!(["system", "enabled", "disabled"]),
                ),
                // vel-config settings.rs and shipped Velocity theme guards
                ("ruler_style", json!(["solid", "dotted", "stippled"])),
                ("file_tab_style", json!(["angled", "square", "rounded"])),
                // panels.rs: status, selection, and file preview
                (
                    "show_line_column",
                    json!(["enabled", "compact", "disabled"]),
                ),
                ("preview_on_click", json!([true, false, "only_left"])),
                (
                    "auto_find_in_selection",
                    json!([false, true, "find_only", "replace_only"]),
                ),
                // vel-fileio, vel-session, and open_requests.rs
                ("default_line_ending", json!(["system", "unix", "windows"])),
                (
                    "trim_trailing_white_space_on_save",
                    json!([false, true, "all", "not_on_caret"]),
                ),
                (
                    "hot_exit",
                    json!(["always", "only_on_quit", "disabled", true, false]),
                ),
                (
                    "open_files_in_new_window",
                    json!(["never", "finder_only", "always", true, false]),
                ),
                // completion.rs, vintage.rs, and the annotation baseline reader
                (
                    "auto_complete_preserve_order",
                    json!(["none", "some", "strict", true, false]),
                ),
                ("vintage_ctrl_keys", json!([null, true, false])),
                ("git_diff_target", json!(["index", "head"])),
            ];
            for (key, values) in contracts {
                assert_eq!(&meta[key]["choices"], values, "{key}");
            }
            let mut locales = vec![json!("en")];
            locales
                .extend(crate::ui_language::shipped_catalogs().map(|(tag, _)| json!(tag.as_str())));
            locales.push(json!("auto"));
            assert_eq!(meta["ui_language"]["choices"], Value::Array(locales));
            assert_eq!(
                DEFINITIONS
                    .iter()
                    .filter(|def| def.meta.control == Control::Enum)
                    .count(),
                contracts.len() + 1,
            );
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) use model::{Navigation, Page};
#[cfg(not(target_os = "macos"))]
#[derive(Debug)]
pub(crate) struct Page;
