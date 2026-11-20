//! The graphical User-preferences sheet. Nothing here is initialized by editor windows
//! until the sheet is opened.
#[cfg(target_os = "macos")]
#[path = "settings_ui_file.rs"]
mod file;
#[cfg(target_os = "macos")]
#[path = "settings_ui_view.rs"]
mod view;

#[cfg(target_os = "macos")]
mod model {
    use super::file::{FileSession, Issue, Mutation, Snapshot};
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
    use vel_platform::{Command, Point, Rect, TextInput};
    use vel_ui::{Item, QuickPanel, Scroll, input::Input};

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
        #[serde(default)]
        choices: Vec<Value>,
        #[serde(default)]
        show_when: BTreeMap<String, Condition>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(untagged)]
    enum Condition {
        Not { not: Value },
        Any(Vec<Value>),
        Equals(Value),
    }

    impl Condition {
        fn matches(&self, value: &Value) -> bool {
            match self {
                Self::Not { not } => not != value,
                Self::Any(values) => values.contains(value),
                Self::Equals(expected) => expected == value,
            }
        }
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
        pub(super) effect: Option<String>,
        search: String,
        pub(super) compatibility: bool,
        visible: bool,
    }

    impl Row {
        pub(super) fn key(&self) -> &'static str {
            &self.def.key
        }

        pub(super) fn control(&self) -> Control {
            self.def.meta.control
        }

        pub(super) fn next_window(&self) -> bool {
            matches!(self.def.effect, Effect::NextWindow)
        }

        pub(super) fn editable(&self) -> bool {
            !self.compatibility
                && match self.control() {
                    Control::Bool => self.value.is_boolean(),
                    Control::Number => self.value.is_number(),
                    Control::Text => self.value.is_string(),
                    Control::Enum => !self.choices().is_empty(),
                    Control::Array => self.value.is_array() || self.key() == "rulers",
                    _ => false,
                }
        }

        pub(super) fn display(&self, tr: &Translator) -> String {
            Self::label(&self.value, tr)
        }

        pub(super) fn label(value: &Value, tr: &Translator) -> String {
            match value {
                Value::String(s) if s == "auto" => tr.tr("Automatic").into_owned(),
                Value::Bool(true) => tr.tr("On").into_owned(),
                Value::Bool(false) => tr.tr("Off").into_owned(),
                Value::String(s) if s.is_empty() => tr.tr("Empty").into_owned(),
                Value::String(s) => s.clone(),
                Value::Null => tr.tr("Default").into_owned(),
                v => v.to_string(),
            }
        }

        pub(super) fn choices(&self) -> &[Value] {
            &self.def.meta.choices
        }

        pub(super) fn tags(&self) -> &[Value] {
            self.value.as_array().map_or_else(
                || {
                    if self.key() == "rulers" && self.value.is_number() {
                        std::slice::from_ref(&self.value)
                    } else {
                        &[]
                    }
                },
                Vec::as_slice,
            )
        }

        pub(super) fn tag_label(value: &Value) -> String {
            value
                .as_str()
                .map_or_else(|| value.to_string(), str::to_owned)
        }

        fn parse_tag(&self, source: &str, tr: &Translator) -> Result<Value, String> {
            let source = source.trim();
            if source.is_empty() || source.chars().any(char::is_control) {
                return Err(tr.tr("Enter a nonempty, single-line value.").into_owned());
            }
            if self.key() == "rulers" {
                return source
                    .parse::<usize>()
                    .ok()
                    .filter(|&n| n > 0)
                    .map(Value::from)
                    .ok_or_else(|| tr.tr("Enter a positive column number.").into_owned());
            }
            let value = Value::String(source.to_owned());
            if !self.choices().is_empty() && !self.choices().contains(&value) {
                return Err(tr.tr_fmt(
                    "Choose one of: {values}.",
                    &[(
                        "values",
                        self.choices()
                            .iter()
                            .map(Self::tag_label)
                            .collect::<Vec<_>>()
                            .join(", "),
                    )],
                ));
            }
            Ok(value)
        }

        pub(super) fn edit_value(&self) -> String {
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
        Action(Target),
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum Target {
        Search,
        Modified,
        Json,
        Repair,
        ResetAll,
        Reload,
        Category(Option<Group>),
        #[allow(dead_code)] // Also used by the native application.
        Row(usize),
        Control(usize),
        Step(usize, bool),
        Reset(usize),
        RowJson(usize),
        NewWindow(usize),
        RemoveTag(usize, usize),
        MoreTags(usize),
        PopupItem(usize),
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub(crate) enum Navigation {
        #[default]
        Stay,
        Close,
        Json,
        Repair,
        NewWindow,
    }

    #[derive(Debug)]
    pub(super) struct Edit {
        pub(super) row: usize,
        pub(super) input: Input,
        pub(super) error: Option<String>,
        // Latest validated draft only. Preedit never becomes a publication.
        pending: Option<(Instant, Value)>,
        base: (Option<Value>, Value),
        dirty: bool,
        pub(super) conflict: bool,
    }

    #[derive(Debug)]
    pub(super) struct Popup {
        pub(super) row: usize,
        pub(super) tags: bool,
        pub(super) panel: QuickPanel,
        pub(super) bounds: Rect,
        value: Value,
    }

    #[derive(Debug)]
    struct Change {
        id: u64,
        row: usize,
        previous: Option<Value>,
        applied: Option<Value>,
    }

    #[derive(Debug)]
    pub(crate) struct Page {
        pub(super) path: Option<PathBuf>,
        pub(super) home: Option<PathBuf>,
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
        pub(super) notice: Option<String>,
        pub(super) popup: Option<Popup>,
        pub(super) hovered: Option<Point>,
        pub(super) hover_since: Option<Instant>,
        pub(super) tooltips: Vec<(Rect, String)>,
        pub(super) heading_bounds: Rect,
        pub(super) heading: String,
        pub(super) row_list: Rect,
        pub(super) announcement: Option<String>,
        pub(super) conflicts: BTreeMap<usize, String>,
        pub(super) scroll: Scroll,
        pub(super) category_scroll: Scroll,
        pub(super) hits: Vec<(Rect, Target)>,
        pub(super) list_bounds: Rect,
        pub(super) sidebar_bounds: Rect,
        pub(super) bounds: Rect,
        value_revision: Option<(u64, u64, u64)>,
        file_revision: Option<u64>,
        file: FileSession,
        history: Vec<Change>,
        next_change: u64,
        restoring: bool,
        reloading: bool,
    }

    impl Page {
        pub(crate) fn new(workspace: &Workspace) -> Self {
            let mut page = Self {
                path: workspace.data_path().map(workbench::preferences_path),
                home: std::env::var_os("HOME").map(PathBuf::from),
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
                notice: None,
                popup: None,
                hovered: None,
                hover_since: None,
                tooltips: Vec::new(),
                heading_bounds: Rect::default(),
                heading: String::new(),
                row_list: Rect::default(),
                announcement: None,
                conflicts: BTreeMap::new(),
                scroll: Scroll::default(),
                category_scroll: Scroll::default(),
                hits: Vec::new(),
                list_bounds: Rect::default(),
                sidebar_bounds: Rect::default(),
                bounds: Rect::default(),
                value_revision: None,
                file_revision: None,
                file: FileSession::new(None),
                history: Vec::new(),
                next_change: 0,
                restoring: false,
                reloading: false,
            };
            page.file = FileSession::new(page.read_file(&workspace.translations));
            page.file_revision = Some(workspace.resource_revision);
            page.refresh(workspace);
            page
        }

        fn read_file(&mut self, tr: &Translator) -> Option<Snapshot> {
            let snapshot = self
                .path
                .as_deref()
                .ok_or_else(|| tr.tr("No User preferences file is available.").into_owned())
                .and_then(Snapshot::read);
            self.error = snapshot
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|file| file.object())
                .err()
                .map(|error| {
                    if error == "Expected a JSON object at line 1, column 1." {
                        tr.tr("Expected a JSON object at line 1, column 1.")
                            .into_owned()
                    } else {
                        error
                    }
                });
            snapshot.ok()
        }

        fn sync_writes(&mut self, tr: &Translator) -> bool {
            let (pending, failures) = self.file.results();
            let restored = self.restoring && !pending;
            if restored {
                self.restoring = false;
                if failures.is_empty() {
                    self.history.clear();
                    self.notice = None;
                    self.announcement = Some(tr.tr("Settings restored").into_owned());
                }
            }
            let changed = restored || !failures.is_empty();
            for failure in failures {
                self.history.retain(|change| Some(change.id) != failure.id);
                self.notice = Some(match failure.issue {
                    Issue::Outside => tr.tr("The file was changed outside Settings").into_owned(),
                    Issue::Io(error) => {
                        tr.tr_fmt("Could not save Settings: {detail}", &[("detail", error)])
                    }
                });
                if let Some((row, draft)) = failure.draft {
                    if let Some(edit) = self.edit.as_mut().filter(|edit| edit.row == row) {
                        if !edit.dirty && self.rows[row].control() == Control::Array {
                            edit.input.set(&draft);
                        }
                        edit.conflict = true;
                        edit.pending = None;
                    } else {
                        self.conflicts.insert(row, draft);
                    }
                }
            }
            changed
        }

        pub(crate) fn refresh(&mut self, workspace: &Workspace) {
            let tr = &workspace.translations;
            self.sync_writes(tr);
            if self.file_revision != Some(workspace.resource_revision) {
                self.file_revision = Some(workspace.resource_revision);
                self.reloading = false;
                let snapshot = self.read_file(tr);
                self.file.observe(snapshot.as_ref());
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
            let effective = |key| {
                user.get(key)
                    .or_else(|| platform.get(key))
                    .or_else(|| defaults.get(key))
            };
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
                    value: effective(key).cloned().unwrap_or_default(),
                    name,
                    description,
                    effect: (!matches!(def.effect, Effect::Immediate)).then(|| {
                        tr.tr_fmt(
                            "Takes effect: {time}",
                            &[("time", def.effect.label(tr).into_owned())],
                        )
                    }),
                    search: search.to_lowercase(),
                    modified,
                    compatibility: def
                        .description
                        .contains("accepted for compatibility, no effect in Velocity"),
                    visible: def.meta.show_when.iter().all(|(key, condition)| {
                        effective(key.as_str()).is_some_and(|value| condition.matches(value))
                    }),
                });
            }
            if let Some(edit) = &mut self.edit {
                let row = &self.rows[edit.row];
                let base = (user.get(row.key()).cloned(), row.value.clone());
                if self.error.is_some() || edit.base != base {
                    if edit.dirty || edit.input.composing() || edit.conflict {
                        edit.conflict = true;
                        edit.pending = None;
                    } else {
                        edit.base = base;
                        if row.control() != Control::Array {
                            edit.input.set(&row.edit_value());
                        }
                    }
                }
            }
            self.filter(selected);
            if let Some(edit) = &self.edit
                && !self.filtered.contains(&edit.row)
            {
                self.stash_edit();
            }
            if self.popup.as_ref().is_some_and(|popup| {
                self.error.is_some()
                    || !self.filtered.contains(&popup.row)
                    || popup.value != self.rows[popup.row].value
            }) {
                self.popup = None;
            }
        }

        fn filter(&mut self, selected: Option<usize>) {
            let query = self.search.value().to_lowercase();
            self.filtered.clear();
            self.filtered
                .extend(self.rows.iter().enumerate().filter_map(|(i, row)| {
                    (row.visible
                        && (!self.modified_only || row.modified)
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
            self.popup = None;
            if !self.finish_edit(workspace) {
                self.stash_edit();
            }
            self.focus = Focus::Search;
            self.search.command(Command::SelectAll);
        }

        pub(crate) fn input(&self) -> Option<&Input> {
            if self.popup.is_some() {
                return None;
            }
            match self.focus {
                Focus::Search => Some(&self.search),
                Focus::Edit if self.error.is_none() => self.edit.as_ref().map(|edit| &edit.input),
                _ => None,
            }
        }

        pub(crate) fn input_mut(&mut self) -> Option<&mut Input> {
            if self.popup.is_some() {
                return None;
            }
            match self.focus {
                Focus::Search => Some(&mut self.search),
                Focus::Edit if self.error.is_none() => {
                    self.edit.as_mut().map(|edit| &mut edit.input)
                }
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
                edit.dirty = true;
                if edit.conflict || self.rows[edit.row].control() == Control::Array {
                    edit.error = None;
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
            let changed = self.sync_writes(&workspace.translations);
            self.refresh(workspace);
            if !self.deadline().is_some_and(|due| due <= now) {
                return changed;
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

        pub(super) fn writable(&self) -> bool {
            self.error.is_none() && !self.restoring && !self.reloading
        }

        pub(super) fn can_reload(&self) -> bool {
            self.path.is_some() && !self.file.pending() && !self.reloading
        }

        fn base(&self, workspace: &Workspace, row: usize) -> (Option<Value>, Value) {
            let row = &self.rows[row];
            (
                workspace
                    .settings
                    .layer(Layer::User)
                    .get(row.key())
                    .cloned(),
                row.value.clone(),
            )
        }

        fn write(&mut self, workspace: &mut Workspace, row: usize, value: Option<Value>) -> bool {
            let previous = self.base(workspace, row).0;
            if previous == value {
                return true;
            }
            let id = self.next_change;
            self.next_change += 1;
            self.history.push(Change {
                id,
                row,
                previous: previous.clone(),
                applied: value.clone(),
            });
            if self.write_change(workspace, row, previous, value, id) {
                true
            } else {
                self.history.retain(|change| change.id != id);
                false
            }
        }

        fn write_change(
            &mut self,
            workspace: &mut Workspace,
            row: usize,
            previous: Option<Value>,
            value: Option<Value>,
            id: u64,
        ) -> bool {
            let Some(path) = self.path.clone().filter(|_| self.writable()) else {
                return false;
            };
            let file = self.read_file(&workspace.translations);
            if file.is_none() || self.error.is_some() {
                if let Some(edit) = &mut self.edit {
                    edit.conflict = true;
                }
                return false;
            }
            let key = self.rows[row].key();
            let draft = self
                .edit
                .as_ref()
                .filter(|edit| edit.row == row && edit.dirty)
                .map(|edit| (row, edit.input.value()));
            let mutation = Mutation::Key {
                key,
                previous,
                value: value.clone(),
            };
            if let Err(error) = self.file.queue(workspace, path, mutation, Some(id), draft) {
                self.notice = Some(
                    workspace
                        .translations
                        .tr_fmt("Could not save Settings: {detail}", &[("detail", error)]),
                );
                return false;
            }
            let value_revision = workspace.settings.cache_identity();
            workspace.apply_user_preference(key, value.clone());
            let settings = &workspace.settings;
            if let Some(edit) = self.edit.as_mut().filter(|edit| edit.row == row) {
                let effective = value
                    .as_ref()
                    .or_else(|| settings.layer(Layer::Platform).get(key))
                    .or_else(|| settings.layer(Layer::Default).get(key))
                    .cloned()
                    .unwrap_or_default();
                edit.base = (value, effective);
                edit.dirty = false;
            }
            workspace.sync_ui_language();
            if value_revision != workspace.settings.cache_identity() {
                self.refresh(workspace);
            }
            let row = &self.rows[row];
            let tr = &workspace.translations;
            let value = match row.value.as_bool() {
                Some(true) => tr.tr("on").into_owned(),
                Some(false) => tr.tr("off").into_owned(),
                _ => row.display(tr),
            };
            self.announcement = Some(tr.tr_fmt(
                "{name}, {value}",
                &[("name", row.name.clone()), ("value", value)],
            ));
            true
        }

        fn undo(&mut self, workspace: &mut Workspace) {
            if !self.writable() {
                return;
            }
            let Some(change) = self.history.pop() else {
                return;
            };
            if self.base(workspace, change.row).0 != change.applied {
                self.notice = Some(
                    workspace
                        .translations
                        .tr("The file was changed outside Settings")
                        .into_owned(),
                );
                self.history.push(change);
            } else {
                if self
                    .edit
                    .as_ref()
                    .is_some_and(|edit| edit.row == change.row)
                {
                    self.stash_edit();
                }
                self.write_change(
                    workspace,
                    change.row,
                    change.applied,
                    change.previous,
                    change.id,
                );
            }
        }

        fn reset_all(&mut self, workspace: &mut Workspace) {
            let Some(path) = self.path.clone().filter(|_| self.writable()) else {
                return;
            };
            self.popup = None;
            if let Err(error) = self
                .file
                .queue(workspace, path, Mutation::Restore, None, None)
            {
                self.notice = Some(
                    workspace
                        .translations
                        .tr_fmt("Could not save Settings: {detail}", &[("detail", error)]),
                );
                return;
            }
            self.restoring = true;
        }

        fn reload(&mut self, workspace: &mut Workspace) {
            if self.can_reload() {
                self.file = FileSession::new(self.read_file(&workspace.translations));
                self.notice = None;
                self.history.clear();
                self.focus = Focus::List;
                self.reloading = true;
                self.stash_edit();
                workspace.request_resource_reload();
            }
        }

        fn stash_edit(&mut self) {
            if let Some(edit) = self.edit.take() {
                if edit.conflict {
                    self.conflicts.insert(edit.row, edit.input.value());
                }
                if self.focus == Focus::Edit {
                    self.focus = Focus::List;
                }
            }
        }

        pub(crate) fn finish_edit(&mut self, workspace: &mut Workspace) -> bool {
            let Some(edit) = &mut self.edit else {
                return true;
            };
            if edit.error.is_some() || edit.conflict || edit.input.composing() {
                return false;
            }
            if let Some((_, value)) = edit.pending.take()
                && !self.write(workspace, self.edit.as_ref().unwrap().row, Some(value))
            {
                return false;
            }
            self.edit = None;
            self.focus = Focus::List;
            true
        }

        fn select(&mut self, id: usize) {
            if let Some(index) = self.filtered.iter().position(|&r| r == id) {
                self.selected = index;
                self.scroll.reveal(index);
            }
        }

        fn begin_edit(&mut self, workspace: &Workspace, row: usize) {
            self.focus = Focus::Edit;
            let mut input = Input::default();
            let saved = self.conflicts.remove(&row);
            if let Some(draft) = &saved {
                input.set(draft);
            } else if self.rows[row].control() != Control::Array {
                input.set(&self.rows[row].edit_value());
            }
            self.edit = Some(Edit {
                row,
                input,
                error: None,
                pending: None,
                base: self.base(workspace, row),
                dirty: saved.is_some(),
                conflict: saved.is_some(),
            });
        }

        fn activate(&mut self, workspace: &mut Workspace, row: usize, toggle_only: bool) {
            if !self.writable() || !self.rows[row].editable() {
                return;
            }
            match self.rows[row].control() {
                Control::Bool => {
                    let next = !self.rows[row].value.as_bool().unwrap();
                    self.write(workspace, row, Some(Value::Bool(next)));
                }
                Control::Number | Control::Text | Control::Array if !toggle_only => {
                    self.begin_edit(workspace, row)
                }
                Control::Enum => self.open_popup(&workspace.translations, row, false),
                _ => {}
            }
        }

        fn step(&mut self, workspace: &mut Workspace, row: usize, up: bool, now: Instant) {
            if !self.writable()
                || !self.rows[row].editable()
                || self.edit.as_ref().is_some_and(|edit| edit.conflict)
            {
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
                self.begin_edit(workspace, row);
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

        fn open_popup(&mut self, tr: &Translator, row: usize, tags: bool) {
            if !self.writable() || !self.rows[row].editable() {
                return;
            }
            let value = &self.rows[row];
            let entries = if tags { value.tags() } else { value.choices() };
            let mut panel = QuickPanel::default();
            panel.set_items(
                entries
                    .iter()
                    .enumerate()
                    .map(|(i, entry)| Item {
                        caption: if tags {
                            format!("×  {}", Row::tag_label(entry))
                        } else {
                            format!(
                                "{}{}",
                                if entry == &value.value { "✓  " } else { "" },
                                Row::label(entry, tr)
                            )
                        },
                        value: i,
                        ..Item::default()
                    })
                    .collect(),
            );
            if entries.is_empty() {
                return;
            }
            if !tags {
                panel.selected = entries
                    .iter()
                    .position(|entry| entry == &value.value)
                    .unwrap_or(0);
            }
            self.popup = Some(Popup {
                row,
                tags,
                panel,
                bounds: Rect::default(),
                value: value.value.clone(),
            });
        }

        fn choose(&mut self, workspace: &mut Workspace, item: usize) {
            let Some(popup) = self.popup.take() else {
                return;
            };
            if popup.tags {
                self.remove_tag(workspace, popup.row, item);
            } else if let Some(choice) = self.rows[popup.row].choices().get(item).cloned()
                && choice != self.rows[popup.row].value
            {
                self.write(workspace, popup.row, Some(choice));
            }
        }

        fn add_tag(&mut self, workspace: &mut Workspace) {
            let Some(edit) = self.edit.as_mut().filter(|edit| !edit.conflict) else {
                return;
            };
            let source = edit.input.value();
            if source.trim().is_empty() {
                self.finish_edit(workspace);
                return;
            }
            let row = edit.row;
            match self.rows[row].parse_tag(&source, &workspace.translations) {
                Ok(tag) => {
                    let mut tags = self.rows[row].tags().to_vec();
                    tags.push(tag);
                    if self.write(workspace, row, Some(Value::Array(tags)))
                        && let Some(edit) = &mut self.edit
                        && !edit.conflict
                    {
                        edit.input.set("");
                        edit.error = None;
                    }
                }
                Err(error) => edit.error = Some(error),
            }
        }

        fn remove_tag(&mut self, workspace: &mut Workspace, row: usize, index: usize) {
            if !self.writable()
                || self
                    .edit
                    .as_ref()
                    .is_some_and(|edit| edit.row == row && edit.conflict)
            {
                return;
            }
            let mut tags = self.rows[row].tags().to_vec();
            if index < tags.len() {
                tags.remove(index);
                self.write(workspace, row, Some(Value::Array(tags)));
            }
        }

        fn target_at(&self, point: Point) -> Option<Target> {
            self.hits
                .iter()
                .rev()
                .find_map(|(bounds, target)| vel_ui::contains(*bounds, point).then_some(*target))
        }

        pub(crate) fn ax_focus(&mut self, workspace: &mut Workspace, point: Point) {
            let Some(target) = self.target_at(point) else {
                return;
            };
            if let Target::PopupItem(index) = target
                && let Some(popup) = &mut self.popup
            {
                popup.panel.selected = index;
                popup.panel.scroll.reveal(index);
                return;
            }
            self.popup = None;
            if matches!(target, Target::Control(row) if self.edit.as_ref().is_some_and(|edit| edit.row == row))
            {
                return;
            }
            if !self.finish_edit(workspace) {
                self.stash_edit();
            }
            match target {
                Target::Search => self.focus = Focus::Search,
                Target::Row(row) => {
                    self.select(row);
                    self.focus = Focus::List;
                }
                Target::Control(row) => {
                    self.select(row);
                    if self.writable()
                        && matches!(
                            self.rows[row].control(),
                            Control::Text | Control::Number | Control::Array
                        )
                    {
                        self.begin_edit(workspace, row);
                    } else {
                        self.focus = Focus::Action(target);
                    }
                }
                target => self.focus = Focus::Action(target),
            }
        }

        fn use_target(
            &mut self,
            workspace: &mut Workspace,
            target: Target,
            now: Instant,
        ) -> Navigation {
            use Target::*;
            match target {
                Search => self.focus = Focus::Search,
                Modified => self.show_modified(),
                Category(group) => {
                    self.focus = Focus::Sidebar;
                    self.category(group);
                }
                Row(row) | Control(row) => {
                    self.focus = Focus::List;
                    self.select(row);
                    if target == Control(row) {
                        self.activate(workspace, row, false);
                    }
                }
                Step(row, up) => {
                    self.select(row);
                    self.step(workspace, row, up, now);
                }
                Reset(row) => {
                    if !self.writable() {
                        return Navigation::Stay;
                    }
                    self.focus = Focus::List;
                    self.select(row);
                    self.conflicts.remove(&row);
                    self.write(workspace, row, None);
                }
                RemoveTag(row, index) => self.remove_tag(workspace, row, index),
                MoreTags(row) => {
                    self.select(row);
                    self.open_popup(&workspace.translations, row, true);
                }
                PopupItem(item) => self.choose(workspace, item),
                ResetAll => self.reset_all(workspace),
                Reload => self.reload(workspace),
                Json | RowJson(_) => return Navigation::Json,
                Repair => return Navigation::Repair,
                NewWindow(_) => return Navigation::NewWindow,
            }
            Navigation::Stay
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
            let target = self.target_at(point);
            if let Some(popup) = &self.popup {
                if let Some(PopupItem(item)) = target {
                    self.choose(workspace, item);
                } else if !vel_ui::contains(popup.bounds, point) {
                    self.popup = None;
                }
                return Navigation::Stay;
            }
            let Some(target) = target else {
                return Navigation::Stay;
            };
            match target {
                Repair => return Navigation::Repair,
                Reload => {
                    self.reload(workspace);
                    return Navigation::Stay;
                }
                Json | RowJson(_) => {
                    if !self.finish_edit(workspace) {
                        self.stash_edit();
                    }
                    return Navigation::Json;
                }
                Control(row) if self.edit.as_ref().is_some_and(|edit| edit.row == row) => {
                    if let Some(input) = self.input_mut() {
                        input.click_at(point, extend);
                    }
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
                RemoveTag(row, _) | MoreTags(row)
                    if self.edit.as_ref().is_some_and(|edit| edit.row == row) => {}
                _ => {
                    if !self.finish_edit(workspace) {
                        if self.edit.as_ref().is_some_and(|edit| edit.conflict) {
                            self.stash_edit();
                        } else {
                            return Navigation::Stay;
                        }
                    }
                }
            }
            if target == Search {
                self.search.click_at(point, extend);
            }
            self.use_target(workspace, target, now)
        }

        pub(crate) fn space(&mut self, workspace: &mut Workspace, now: Instant) -> Navigation {
            self.refresh(workspace);
            if let Some(popup) = &self.popup {
                if let Some(item) = popup.panel.current() {
                    self.choose(workspace, item);
                }
            } else if let Focus::Action(target) = self.focus {
                return self.use_target(workspace, target, now);
            } else if self.focus == Focus::List
                && let Some(row) = self.selected_row()
            {
                self.activate(workspace, row, true);
            }
            Navigation::Stay
        }

        pub(crate) fn show_modified(&mut self) {
            self.modified_only = !self.modified_only;
            self.filter(self.selected_row());
        }

        fn tab(&mut self, backwards: bool) {
            let mut order = vec![
                Focus::Search,
                Focus::Action(Target::Modified),
                Focus::Action(Target::Json),
            ];
            if self.writable() {
                order.push(Focus::Action(Target::ResetAll));
            }
            order.push(Focus::Sidebar);
            order.push(Focus::List);
            if let Some(row) = self.selected_row() {
                order.push(Focus::Action(Target::RowJson(row)));
                if self.writable() {
                    if self.rows[row].next_window() {
                        order.push(Focus::Action(Target::NewWindow(row)));
                    }
                    for &(_, target) in &self.hits {
                        if matches!(target, Target::RemoveTag(owner, _) | Target::MoreTags(owner) if owner == row)
                        {
                            order.push(Focus::Action(target));
                        }
                    }
                    if self.rows[row].modified {
                        order.push(Focus::Action(Target::Reset(row)));
                    }
                }
            }
            if self.error.is_some() && self.path.is_some() {
                order.push(Focus::Action(Target::Repair));
            }
            if self.notice.is_some() && self.can_reload() {
                order.push(Focus::Action(Target::Reload));
            }
            let index = order
                .iter()
                .position(|&focus| self.focus == focus)
                .unwrap_or(0);
            self.focus = if backwards {
                order[(index + order.len() - 1) % order.len()]
            } else {
                order[(index + 1) % order.len()]
            };
            if self.focus == Focus::List {
                self.scroll.reveal(self.selected);
            }
        }

        pub(crate) fn command(
            &mut self,
            workspace: &mut Workspace,
            cmd: Command,
            now: Instant,
        ) -> Navigation {
            self.refresh(workspace);
            use Command::*;
            if let Some(popup) = &mut self.popup {
                match cmd {
                    Up(false) | Backtab => {
                        popup.panel.navigate(-1);
                    }
                    Down(false) | Tab => {
                        popup.panel.navigate(1);
                    }
                    PageUp => {
                        popup.panel.navigate(-5);
                    }
                    PageDown => {
                        popup.panel.navigate(5);
                    }
                    Cancel => self.popup = None,
                    Newline | LiteralNewline => {
                        if let Some(item) = popup.panel.current() {
                            self.choose(workspace, item);
                        }
                    }
                    Undo => {
                        self.popup = None;
                        self.undo(workspace);
                    }
                    _ => {}
                }
                return Navigation::Stay;
            }
            if self.input().is_some_and(Input::composing) {
                if cmd == Command::Cancel {
                    self.input_mut().unwrap().cancel_composition();
                }
                return Navigation::Stay;
            }
            match cmd {
                Cancel => match self.focus {
                    Focus::Edit => {
                        self.edit = None;
                        self.focus = Focus::List;
                    }
                    Focus::Action(_) => self.focus = Focus::List,
                    Focus::List => self.focus = Focus::Sidebar,
                    Focus::Search | Focus::Sidebar => return Navigation::Close,
                },
                Tab | Backtab => {
                    if self.edit.as_ref().is_some_and(|edit| edit.conflict) {
                        self.stash_edit();
                    }
                    if self.finish_edit(workspace) {
                        self.tab(cmd == Backtab);
                    }
                }
                Undo => self.undo(workspace),
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
                        if self
                            .edit
                            .as_ref()
                            .is_some_and(|edit| self.rows[edit.row].control() == Control::Array)
                        {
                            self.add_tag(workspace);
                        } else {
                            self.finish_edit(workspace);
                        }
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
                    Focus::Action(target) => return self.use_target(workspace, target, now),
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
            if let Some(popup) = &mut self.popup {
                popup.panel.scroll.scroll(delta);
            } else if vel_ui::contains(self.sidebar_bounds, point) {
                self.category_scroll.scroll(delta);
            } else if self.focus != Focus::Edit {
                self.scroll.scroll(delta);
            }
        }

        pub(crate) fn hover(&mut self, point: Point, now: Instant) -> bool {
            let previous = self.hovered;
            let at = |point| {
                self.tooltips
                    .iter()
                    .rposition(|(rect, _)| vel_ui::contains(*rect, point))
            };
            let changed = previous.is_none_or(|previous| at(previous) != at(point));
            self.hovered = Some(point);
            if changed {
                self.hover_since = Some(now);
            }
            changed
        }

        pub(crate) fn hover_deadline(&self) -> Option<Instant> {
            self.hover_since
                .map(|since| since + Duration::from_millis(650))
        }

        pub(crate) fn search_for(&mut self, query: &str) {
            self.search.set(query);
            self.scroll.offset = 0.;
            self.filter(None);
        }

        pub(crate) fn use_fixture_home(&mut self, home: PathBuf) {
            self.home = Some(home);
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
            let drawn = drawn
                .strip_prefix("✓  ")
                .or_else(|| drawn.strip_prefix("×  "))
                .unwrap_or(drawn);
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
                        || row.tags().iter().any(|tag| matches(&Row::tag_label(tag)))
                        || row
                            .choices()
                            .iter()
                            .any(|choice| matches(&Row::label(choice, tr)))
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
                    effect: None,
                    search: String::new(),
                    compatibility: false,
                    visible: true,
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
                for (parent, condition) in &definition.meta.show_when {
                    assert!(
                        defaults.contains_key(parent),
                        "{key}: unshipped dependency {parent}"
                    );
                    let values = match condition {
                        Condition::Not { not } => std::slice::from_ref(not),
                        Condition::Any(values) => values,
                        Condition::Equals(value) => std::slice::from_ref(value),
                    };
                    assert!(!values.is_empty(), "{key}: empty dependency {parent}");
                    for value in values {
                        assert!(!value.is_object(), "{key}: invalid dependency {parent}");
                        let mut layer =
                            serde_json::Map::from_iter([(parent.clone(), value.clone())]);
                        assert!(
                            validate_layer(&mut layer, defaults).is_empty(),
                            "{key}: bad operand for {parent}: {value}"
                        );
                    }
                }
            }
            assert!(editable_count > 100);
            fn acyclic<'a>(definition: &'a Definition, path: &mut Vec<&'a str>) {
                assert!(
                    !path.contains(&definition.key.as_str()),
                    "show_when cycle: {path:?}"
                );
                path.push(&definition.key);
                for key in definition.meta.show_when.keys() {
                    acyclic(
                        DEFINITIONS.iter().find(|def| &def.key == key).unwrap(),
                        path,
                    );
                }
                path.pop();
            }
            for definition in DEFINITIONS.iter() {
                acyclic(definition, &mut Vec::new());
            }
            for key in ["wrap_width", "wrap_width_style"] {
                let definition = DEFINITIONS.iter().find(|def| def.key == key).unwrap();
                let wrap = &definition.meta.show_when["word_wrap"];
                assert!(!wrap.matches(&json!(false)));
                assert!(wrap.matches(&json!(true)) && wrap.matches(&json!("auto")));
            }
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
