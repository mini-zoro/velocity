//! The graphical User-preferences sheet. Nothing here is initialized by editor windows
//! until the sheet is opened.
#[cfg(any(target_os = "macos", windows))]
#[path = "settings_ui_file.rs"]
mod file;
#[cfg(any(target_os = "macos", windows))]
#[path = "settings_ui_pages.rs"]
mod pages;
#[cfg(any(target_os = "macos", windows))]
#[path = "settings_ui_view.rs"]
mod view;

#[cfg(any(target_os = "macos", windows))]
mod model {
    use super::file::{FileSession, Issue, Mutation, Snapshot};
    pub(super) use super::pages::{self, Condition};
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
        General,
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
                Self::General => tr.tr("General"),
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
        name: String,
        description: String,
        #[serde(default)]
        choices: Vec<Value>,
        /// Captions for `choices`, in order; `null` keeps the generic caption.
        #[serde(default)]
        labels: Vec<Option<String>>,
        #[serde(default)]
        show_when: BTreeMap<String, Condition>,
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
        effect: Effect,
        #[serde(default)]
        compatibility: bool,
    }

    #[derive(Debug)]
    struct Definition {
        key: String,
        reference: Reference,
        meta: Meta,
    }

    static DEFINITIONS: LazyLock<Vec<Definition>> = LazyLock::new(|| {
        let metadata = Document::parse(METADATA).expect("checked Settings UI metadata");
        let meta: BTreeMap<String, Meta> =
            serde_json::from_value(metadata.value).expect("checked Settings UI schema");
        let mut reference: BTreeMap<String, Reference> =
            serde_json::from_str(REFERENCE).expect("generated settings reference");
        meta.into_iter()
            .map(|(key, meta)| Definition {
                reference: reference.remove(&key).expect("checked reference coverage"),
                key,
                meta,
            })
            .collect()
    });

    #[cfg(test)]
    pub(crate) fn source_text() -> impl Iterator<Item = &'static str> {
        DEFINITIONS
            .iter()
            .flat_map(|def| {
                [def.meta.name.as_str(), def.meta.description.as_str()]
                    .into_iter()
                    .chain(def.meta.labels.iter().flatten().map(String::as_str))
            })
            .chain(pages::source_text())
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
            matches!(self.def.reference.effect, Effect::NextWindow)
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
            if self.control() == Control::Enum {
                self.choice_label(&self.value, tr)
            } else {
                Self::label(&self.value, tr)
            }
        }

        /// A choice's caption: its authored label, a language's own name, or the generic one.
        pub(super) fn choice_label(&self, value: &Value, tr: &Translator) -> String {
            if self.key() == "ui_language"
                && let Some(tag) = value.as_str().filter(|tag| *tag != "auto")
            {
                return if tag == "en" {
                    "English".to_owned()
                } else {
                    tr.catalog(tag)
                        .map_or_else(|| tag.to_owned(), |c| c.language_name.clone())
                };
            }
            if let Some(index) = self.choices().iter().position(|choice| choice == value)
                && let Some(Some(label)) = self.def.meta.labels.get(index)
            {
                return pages::expand_label(label, tr);
            }
            if pages::catalog(self.key()).is_some()
                && let Some(name) = value.as_str().filter(|name| *name != "auto")
            {
                return pages::resource_label(name);
            }
            Self::label(value, tr)
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

    /// The sidebar entry shown on the right: a curated page or the full list.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Pane {
        Page(usize),
        Advanced,
    }

    impl Pane {
        /// A curated page by its id in settings-pages.json.
        pub(crate) fn page(id: &str) -> Self {
            pages::pages()
                .iter()
                .position(|page| page.id == id)
                .map_or(Self::Advanced, Self::Page)
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum Target {
        Search,
        Modified,
        Json,
        Repair,
        ResetAll,
        Reload,
        Pane(Pane),
        /// The Advanced list's group filter popup.
        Groups,
        /// A curated push button, by its id in `pages::buttons()`.
        Button(usize),
        /// One option of a radio group.
        Choice(usize, usize),
        /// The "?" beside a curated control: hover shows the description.
        Help(usize),
        /// A search result's "Page › Section" caption: shows it on its page.
        Locate(usize),
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
        /// A curated button's command, run by the window.
        Command(&'static str, &'static Value),
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

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum PopupKind {
        Choices(usize),
        Tags(usize),
        Groups,
    }

    #[derive(Debug)]
    pub(super) struct Popup {
        pub(super) kind: PopupKind,
        pub(super) panel: QuickPanel,
        pub(super) bounds: Rect,
        value: Value,
        /// The values behind the rows: a key's choices, or the Advanced groups.
        choices: Vec<Value>,
    }

    impl Popup {
        pub(super) fn row(&self) -> Option<usize> {
            match self.kind {
                PopupKind::Choices(row) | PopupKind::Tags(row) => Some(row),
                PopupKind::Groups => None,
            }
        }
    }

    /// How a curated row edits its value.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum Kind {
        Check,
        Popup,
        Radio,
        Field,
        Stepper,
        Tags,
    }

    /// One drawn element of the right side, in order: a curated page, search results, or
    /// the Advanced list.
    #[derive(Clone, Debug)]
    pub(super) enum Entry {
        Heading {
            title: String,
            hint: Option<String>,
        },
        Setting {
            row: usize,
            label: String,
            kind: Kind,
            indent: bool,
            enabled: bool,
            /// Effect note when the section does not already carry it.
            hint: Option<String>,
            choices: Vec<Value>,
            /// Lists installed font families, fetched from the system when first opened.
            font: bool,
            /// In search results: the page and "Page › Section" caption it lives on.
            location: Option<(usize, String)>,
        },
        /// A titled group of search results.
        Group {
            title: String,
        },
        /// A full row with its key, as on Advanced.
        Row(usize),
        Button {
            id: usize,
            caption: String,
        },
        Preview(pages::Preview),
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
        /// The Advanced list's group filter.
        pub(super) category: Option<Group>,
        pub(super) pane: Pane,
        /// Each curated page resolved against the current values.
        pub(super) entries: Vec<Vec<Entry>>,
        /// What the right side draws now; `filtered` lists its rows in the same order.
        pub(super) display: Vec<Entry>,
        /// Localized page names and the search group title, kept for filtering.
        page_names: Vec<String>,
        more_title: String,
        /// Section headings as drawn, for accessibility.
        pub(super) headings: Vec<(Rect, String)>,
        /// Curated page scroll offset and content extent.
        pub(super) page_offset: f64,
        pub(super) page_extent: f64,
        pub(super) page_bounds: Rect,
        /// Installed theme and color scheme values for the curated popups.
        catalogs: [Vec<Value>; 2],
        /// Scroll the selected curated control into view on the next paint.
        pub(super) reveal: bool,
        /// Installed font families, once the window has asked the system for them.
        fonts: Vec<Value>,
        /// A font popup waiting for the window to list the installed families.
        font_request: Option<usize>,
        /// Type-to-select in an open popup: the typed prefix and when it was last extended.
        type_select: (String, Option<Instant>),
        /// Values written since the window last broadcast them to other windows.
        published: Vec<(String, Option<Value>)>,
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
                category: None,
                pane: Pane::Page(0),
                entries: Vec::new(),
                display: Vec::new(),
                page_names: Vec::new(),
                more_title: String::new(),
                headings: Vec::new(),
                page_offset: 0.,
                page_extent: 0.,
                page_bounds: Rect::default(),
                catalogs: Default::default(),
                reveal: false,
                fonts: Vec::new(),
                font_request: None,
                type_select: (String::new(), None),
                published: Vec::new(),
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
                let name = tr.tr(&def.meta.name).into_owned();
                let description = pages::platform_text(&tr.tr(&def.meta.description));
                let mut search = format!(
                    "{key}\n{}\n{}",
                    def.meta.name,
                    pages::platform_text(&def.meta.description)
                );
                for catalog in tr.installed_catalogs() {
                    for source in [&def.meta.name, &def.meta.description] {
                        if let Some(localized) = catalog.strings.get(source) {
                            search.push('\n');
                            search.push_str(&pages::platform_text(localized));
                        }
                    }
                }
                for alias in &def.meta.aliases {
                    search.push('\n');
                    search.push_str(alias);
                }
                self.rows.push(Row {
                    def,
                    value: effective(key).cloned().unwrap_or_default(),
                    name,
                    description,
                    effect: (!matches!(def.reference.effect, Effect::Immediate)).then(|| {
                        tr.tr_fmt(
                            "Takes effect: {time}",
                            &[("time", def.reference.effect.label(tr).into_owned())],
                        )
                    }),
                    search: search.to_lowercase(),
                    modified,
                    compatibility: def.reference.compatibility,
                    visible: def.meta.show_when.iter().all(|(key, condition)| {
                        effective(key.as_str()).is_some_and(|value| condition.matches(value))
                    }),
                });
            }
            let catalog = |items: &[crate::workspace::appearance::Resource]| {
                std::iter::once(Value::from("auto"))
                    .chain(items.iter().map(|item| Value::from(item.value.as_str())))
                    .collect::<Vec<_>>()
            };
            self.catalogs = [
                catalog(&workspace.appearance.themes),
                catalog(&workspace.appearance.schemes),
            ];
            self.entries = pages::pages()
                .iter()
                .enumerate()
                .map(|(index, page)| self.page_entries_for(index, page, tr))
                .collect();
            self.page_names = pages::pages()
                .iter()
                .map(|page| tr.tr(&page.name).into_owned())
                .collect();
            self.more_title = tr.tr("More in Advanced").into_owned();
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
                    || popup.row().is_some_and(|row| {
                        !self.filtered.contains(&row) || popup.value != self.rows[row].value
                    })
            }) {
                self.popup = None;
            }
        }

        pub(super) fn row_of(&self, key: &str) -> Option<usize> {
            self.rows.binary_search_by(|row| row.key().cmp(key)).ok()
        }

        /// Resolves one curated page against the current values: hidden dependencies and
        /// other platforms' options drop out, `enabled_when` dims, popups get their choices.
        fn page_entries_for(
            &self,
            page_index: usize,
            page: &'static pages::Spec,
            tr: &Translator,
        ) -> Vec<Entry> {
            use pages::Item;
            let mut out = Vec::new();
            let mut ordinal = 0;
            for section in &page.sections {
                let start = out.len();
                out.push(Entry::Heading {
                    title: tr.tr(&section.title).into_owned(),
                    hint: None,
                });
                for item in &section.items {
                    match item {
                        Item::Setting {
                            key,
                            label,
                            control,
                            choices,
                            enabled_when,
                            platform,
                            indent,
                        } => {
                            if platform.is_some_and(|platform| !platform.current()) {
                                continue;
                            }
                            let Some(row) = self.row_of(key) else {
                                continue;
                            };
                            let entry = &self.rows[row];
                            if !entry.visible {
                                continue;
                            }
                            let catalog = pages::catalog(key);
                            let kind = match (control, entry.control()) {
                                (Some(pages::Style::Radio), Control::Enum) => Kind::Radio,
                                (Some(pages::Style::Font), Control::Text) => Kind::Popup,
                                (_, Control::Bool) => Kind::Check,
                                (_, Control::Enum) => Kind::Popup,
                                (_, Control::Number) => Kind::Stepper,
                                (_, Control::Array) => Kind::Tags,
                                (_, Control::Text) if catalog.is_some() => Kind::Popup,
                                (_, Control::Text) => Kind::Field,
                                _ => continue,
                            };
                            let font = *control == Some(pages::Style::Font);
                            let mut values = match catalog {
                                Some(schemes) => self.catalogs[usize::from(schemes)].clone(),
                                None if font => self.fonts.clone(),
                                None => choices.clone().unwrap_or_else(|| entry.choices().to_vec()),
                            };
                            if matches!(kind, Kind::Popup | Kind::Radio)
                                && !values.contains(&entry.value)
                            {
                                values.insert(0, entry.value.clone());
                            }
                            out.push(Entry::Setting {
                                row,
                                label: tr.tr(label).into_owned(),
                                kind,
                                indent: *indent,
                                enabled: enabled_when.iter().all(|(key, condition)| {
                                    self.row_of(key)
                                        .is_some_and(|row| condition.matches(&self.rows[row].value))
                                }),
                                hint: entry.effect.clone(),
                                choices: values,
                                font,
                                location: None,
                            });
                        }
                        Item::Button { button, .. } => {
                            out.push(Entry::Button {
                                id: pages::button_id(page_index, ordinal),
                                caption: tr.tr(button).into_owned(),
                            });
                            ordinal += 1;
                        }
                        Item::Preview { preview } => out.push(Entry::Preview(*preview)),
                    }
                }
                // A note shared by every option in a section sits beside its heading.
                let hints = out[start + 1..].iter().filter_map(|entry| match entry {
                    Entry::Setting { hint, .. } => Some(hint),
                    _ => None,
                });
                let mut shared = hints.clone();
                if let Some(Some(first)) = shared.next()
                    && hints.clone().all(|hint| hint.as_ref() == Some(first))
                {
                    let first = first.clone();
                    for entry in &mut out[start + 1..] {
                        if let Entry::Setting { hint, .. } = entry {
                            *hint = None;
                        }
                    }
                    if let Entry::Heading { hint, .. } = &mut out[start] {
                        *hint = Some(first);
                    }
                }
                if out.len() == start + 1 {
                    out.pop();
                }
            }
            out
        }

        /// Whether the right side shows a curated page (no search).
        pub(super) fn curated_pane(&self) -> bool {
            matches!(self.pane, Pane::Page(_)) && self.search.value().trim().is_empty()
        }

        /// The curated presentation of a row on screen (a page or a search result).
        pub(super) fn curated(&self, row: usize) -> Option<(Kind, bool, &[Value])> {
            self.display.iter().find_map(|entry| match entry {
                Entry::Setting {
                    row: id,
                    kind,
                    enabled,
                    choices,
                    ..
                } if *id == row => Some((*kind, *enabled, choices.as_slice())),
                _ => None,
            })
        }

        fn is_font_popup(&self, row: usize) -> bool {
            self.display.iter().any(
                |entry| matches!(entry, Entry::Setting { row: id, font: true, .. } if *id == row),
            )
        }

        /// The curated page that shows a row, if any.
        fn page_of(&self, row: usize) -> Option<usize> {
            self.entries.iter().position(|entries| {
                entries
                    .iter()
                    .any(|entry| matches!(entry, Entry::Setting { row: id, .. } if *id == row))
            })
        }

        /// The font popup the window should list families for, once.
        pub(crate) fn take_font_request(&mut self) -> bool {
            self.font_request.is_some() && self.fonts.is_empty()
        }

        /// Installs the system's font families and opens the popup that asked for them.
        pub(crate) fn provide_fonts(&mut self, workspace: &Workspace, mut families: Vec<String>) {
            families.sort_by_key(|family| family.to_lowercase());
            families.dedup();
            self.fonts = families.into_iter().map(Value::from).collect();
            self.value_revision = None;
            self.refresh(workspace);
            if let Some(row) = self.font_request.take() {
                self.open_popup(&workspace.translations, row, false);
            }
        }

        /// Curated choices first, then the metadata's; captions included.
        fn choice_list(&self, row: usize) -> Vec<Value> {
            self.curated(row)
                .map(|(_, _, choices)| choices.to_vec())
                .filter(|choices| !choices.is_empty())
                .unwrap_or_else(|| self.rows[row].choices().to_vec())
        }

        pub(super) fn choice_caption(&self, row: usize, value: &Value, tr: &Translator) -> String {
            self.rows[row].choice_label(value, tr)
        }

        fn filter(&mut self, selected: Option<usize>) {
            let query = self.search.value().to_lowercase();
            let query = query.trim();
            self.display.clear();
            if query.is_empty() {
                match self.pane {
                    Pane::Page(page) => {
                        self.display = self.entries.get(page).cloned().unwrap_or_default();
                    }
                    Pane::Advanced => {
                        let rows = self.rows.iter().enumerate().filter(|(_, row)| {
                            row.visible
                                && (!self.modified_only || row.modified)
                                && self
                                    .category
                                    .is_none_or(|group| group == row.def.meta.group)
                        });
                        self.display = rows.map(|(i, _)| Entry::Row(i)).collect();
                    }
                }
            } else {
                // Search covers every setting: curated controls first, with where they live,
                // then the rest as Advanced rows. The group and modified filters are Advanced's.
                let matches = |row: &Row| {
                    row.visible
                        && query
                            .split_whitespace()
                            .all(|part| row.search.contains(part))
                };
                let exact = |entry: &Entry| match entry {
                    Entry::Setting { row, .. } | Entry::Row(row) => self.rows[*row].key() == query,
                    _ => false,
                };
                let mut curated = Vec::new();
                for (page, entries) in self.entries.iter().enumerate() {
                    let mut section = "";
                    for entry in entries {
                        match entry {
                            Entry::Heading { title, .. } => section = title,
                            Entry::Setting { row, .. } if matches(&self.rows[*row]) => {
                                let mut entry = entry.clone();
                                if let Entry::Setting { location, hint, .. } = &mut entry {
                                    let name = self.page_names.get(page).map_or("", String::as_str);
                                    *location = Some((page, format!("{name} › {section}")));
                                    hint.clone_from(&self.rows[*row].effect);
                                }
                                curated.push(entry);
                            }
                            _ => {}
                        }
                    }
                }
                let shown: std::collections::BTreeSet<_> = curated
                    .iter()
                    .filter_map(|entry| match entry {
                        Entry::Setting { row, .. } => Some(*row),
                        _ => None,
                    })
                    .collect();
                let mut rest: Vec<_> = (0..self.rows.len())
                    .filter(|i| !shown.contains(i) && matches(&self.rows[*i]))
                    .map(Entry::Row)
                    .collect();
                for group in [&mut curated, &mut rest] {
                    if let Some(at) = group.iter().position(exact) {
                        group[..=at].rotate_right(1);
                    }
                }
                self.display = curated;
                if !rest.is_empty() {
                    self.display.push(Entry::Group {
                        title: self.more_title.clone(),
                    });
                    self.display.append(&mut rest);
                }
            }
            self.filtered = self
                .display
                .iter()
                .filter_map(|entry| match entry {
                    Entry::Setting { row, .. } | Entry::Row(row) => Some(*row),
                    _ => None,
                })
                .collect();
            self.selected = selected
                .and_then(|row| self.filtered.iter().position(|&i| i == row))
                .unwrap_or_else(|| {
                    if selected.is_some() {
                        self.selected.min(self.filtered.len().saturating_sub(1))
                    } else {
                        0
                    }
                });
        }

        pub(super) fn selected_row(&self) -> Option<usize> {
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

        pub(crate) fn discard_field_composition(&mut self, id: vel_buffer::BufferId) {
            if self.search.buffer.id() == id {
                self.search.cancel_composition();
            } else if let Some(edit) = &mut self.edit
                && edit.input.buffer.id() == id
            {
                edit.input.cancel_composition();
            }
        }

        pub(crate) fn text_input(&mut self, tr: &Translator, event: TextInput, now: Instant) {
            if let Some(popup) = &mut self.popup {
                // Type to select, as in a native popup menu.
                if let TextInput::Insert { text, .. } = &event {
                    let (prefix, last) = &mut self.type_select;
                    if last.is_none_or(|last| now.duration_since(last) > Duration::from_secs(1)) {
                        prefix.clear();
                    }
                    *last = Some(now);
                    prefix.push_str(&text.to_lowercase());
                    let wanted = prefix.trim_start().to_owned();
                    if let Some(index) = popup.panel.items.iter().position(|item| {
                        item.caption
                            .trim_start_matches("✓  ")
                            .to_lowercase()
                            .starts_with(&wanted)
                    }) {
                        popup.panel.selected = index;
                        popup.panel.scroll.reveal(index);
                    }
                }
                return;
            }
            // Typing while the sidebar or a control has focus searches, as in a list.
            if self.input().is_none()
                && self.popup.is_none()
                && matches!(self.focus, Focus::Sidebar | Focus::List | Focus::Action(_))
                && match &event {
                    TextInput::Insert { text, .. } | TextInput::Marked { text, .. } => {
                        !text.trim().is_empty()
                    }
                    _ => false,
                }
            {
                self.focus = Focus::Search;
            }
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
                    self.page_offset = 0.;
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
            self.published.push((key.to_owned(), value.clone()));
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
                self.reveal = true;
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
            match self.curated(row) {
                Some((_, false, _)) => return,
                Some((Kind::Popup, ..)) => {
                    if self.fonts.is_empty() && self.is_font_popup(row) {
                        self.font_request = Some(row);
                    } else {
                        self.open_popup(&workspace.translations, row, false);
                    }
                    return;
                }
                Some((Kind::Radio, ..)) => {
                    self.step_choice(workspace, row, 1);
                    return;
                }
                _ => {}
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

        /// Moves a radio group to its previous or next option, wrapping around.
        fn step_choice(&mut self, workspace: &mut Workspace, row: usize, delta: isize) {
            if !self.writable() || self.curated(row).is_some_and(|(_, enabled, _)| !enabled) {
                return;
            }
            let choices = self.choice_list(row);
            if choices.is_empty() {
                return;
            }
            let current = choices
                .iter()
                .position(|choice| *choice == self.rows[row].value)
                .unwrap_or(0);
            let next = (current as isize + delta).rem_euclid(choices.len() as isize) as usize;
            self.write(workspace, row, Some(choices[next].clone()));
        }

        fn choose_value(&mut self, workspace: &mut Workspace, row: usize, index: usize) {
            if !self.writable()
                || !self.rows[row].editable()
                || self.curated(row).is_some_and(|(_, enabled, _)| !enabled)
            {
                return;
            }
            if let Some(choice) = self.choice_list(row).get(index).cloned()
                && choice != self.rows[row].value
            {
                self.write(workspace, row, Some(choice));
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

        /// Advanced's group filter.
        fn category(&mut self, group: Option<Group>) {
            self.category = group;
            self.search.set("");
            self.page_offset = 0.;
            self.filter(None);
        }

        /// Shows a sidebar entry, leaving any search.
        pub(crate) fn show_pane(&mut self, workspace: &mut Workspace, pane: Pane) {
            if !self.finish_edit(workspace) {
                self.stash_edit();
            }
            self.popup = None;
            self.pane = pane;
            self.search.set("");
            self.page_offset = 0.;
            self.page_offset = 0.;
            self.filter(None);
        }

        pub(super) fn panes() -> impl Iterator<Item = Pane> {
            (0..pages::pages().len())
                .map(Pane::Page)
                .chain([Pane::Advanced])
        }

        /// The groups behind the Advanced filter popup, "All settings" first.
        pub(super) fn group_list(&self) -> Vec<Option<Group>> {
            std::iter::once(None)
                .chain(self.groups.keys().copied().map(Some))
                .collect()
        }

        fn open_groups(&mut self, tr: &Translator) {
            let groups = self.group_list();
            let mut panel = QuickPanel::default();
            panel.set_items(
                groups
                    .iter()
                    .enumerate()
                    .map(|(i, group)| Item {
                        caption: format!(
                            "{}{}",
                            if *group == self.category { "✓  " } else { "" },
                            group.map_or_else(|| tr.tr("All settings"), |g| g.name(tr))
                        ),
                        value: i,
                        ..Item::default()
                    })
                    .collect(),
            );
            panel.selected = groups
                .iter()
                .position(|group| *group == self.category)
                .unwrap_or(0);
            self.popup = Some(Popup {
                kind: PopupKind::Groups,
                panel,
                bounds: Rect::default(),
                value: Value::Null,
                choices: (0..groups.len()).map(Value::from).collect(),
            });
        }

        fn open_popup(&mut self, tr: &Translator, row: usize, tags: bool) {
            if !self.writable() || !self.rows[row].editable() {
                return;
            }
            let entries = if tags {
                self.rows[row].tags().to_vec()
            } else {
                self.choice_list(row)
            };
            let value = &self.rows[row];
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
                                value.choice_label(entry, tr)
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
                kind: if tags {
                    PopupKind::Tags(row)
                } else {
                    PopupKind::Choices(row)
                },
                panel,
                bounds: Rect::default(),
                value: value.value.clone(),
                choices: entries,
            });
        }

        fn choose(&mut self, workspace: &mut Workspace, item: usize) {
            let Some(popup) = self.popup.take() else {
                return;
            };
            match popup.kind {
                PopupKind::Tags(row) => self.remove_tag(workspace, row, item),
                PopupKind::Choices(row) => {
                    if let Some(choice) = popup.choices.get(item).cloned()
                        && choice != self.rows[row].value
                    {
                        self.write(workspace, row, Some(choice));
                    }
                }
                PopupKind::Groups => {
                    if let Some(&group) = self.group_list().get(item) {
                        self.focus = Focus::Action(Target::Groups);
                        self.category(group);
                    }
                }
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
        pub(crate) fn text_field_at(&self, point: Point) -> bool {
            if self.popup.is_some() {
                return false;
            }
            match self.target_at(point) {
                Some(Target::Search) => true,
                Some(Target::Control(row)) if self.writable() && self.rows[row].editable() => {
                    matches!(
                        self.rows[row].control(),
                        Control::Text | Control::Number | Control::Array
                    )
                }
                _ => false,
            }
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
                Target::Pane(_) => self.focus = Focus::Sidebar,
                Target::Row(row)
                | Target::Help(row)
                | Target::Choice(row, _)
                | Target::Locate(row) => {
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
                        && !self
                            .curated(row)
                            .is_some_and(|(kind, enabled, _)| kind == Kind::Popup || !enabled)
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
                Modified => {
                    self.focus = Focus::Action(Modified);
                    self.show_modified();
                }
                Pane(pane) => {
                    self.show_pane(workspace, pane);
                    self.focus = Focus::Sidebar;
                }
                Groups => {
                    self.focus = Focus::Action(Groups);
                    self.open_groups(&workspace.translations);
                }
                Button(id) => {
                    self.focus = Focus::Action(Button(id));
                    let button = &pages::buttons()[id];
                    return match (button.action, button.command) {
                        (Some(pages::Action::Json), _) => Navigation::Json,
                        (None, Some(command)) => Navigation::Command(command, button.args),
                        (None, None) => Navigation::Stay,
                    };
                }
                Choice(row, index) => {
                    self.focus = Focus::List;
                    self.select(row);
                    self.choose_value(workspace, row, index);
                }
                Help(row) => {
                    self.focus = Focus::List;
                    self.select(row);
                }
                Locate(row) => {
                    if let Some(page) = self.page_of(row) {
                        self.show_pane(workspace, self::Pane::Page(page));
                        self.select(row);
                        self.focus = Focus::List;
                        self.reveal = true;
                    }
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

        /// Preferences written since the last call, for the other windows.
        pub(crate) fn take_published(&mut self) -> Vec<(String, Option<Value>)> {
            std::mem::take(&mut self.published)
        }

        pub(crate) fn show_modified(&mut self) {
            self.modified_only = !self.modified_only;
            self.filter(self.selected_row());
        }

        /// Keyboard focus stops in drawing order: search, the sidebar, the header buttons,
        /// then every curated control and button. A run of Advanced rows is one stop (arrows
        /// move inside it), followed by the selected row's own buttons.
        fn focus_order(&self) -> Vec<(Focus, Option<usize>)> {
            let mut order = vec![(Focus::Search, None), (Focus::Sidebar, None)];
            if !self.curated_pane() {
                if self.search.value().trim().is_empty() {
                    order.push((Focus::Action(Target::Groups), None));
                    order.push((Focus::Action(Target::Modified), None));
                    if self.writable() {
                        order.push((Focus::Action(Target::ResetAll), None));
                    }
                }
                order.push((Focus::Action(Target::Json), None));
            }
            let selected = self.selected_row();
            let chips = |row: usize, order: &mut Vec<_>| {
                for &(_, target) in &self.hits {
                    if matches!(target, Target::RemoveTag(owner, _) | Target::MoreTags(owner) if owner == row)
                    {
                        order.push((Focus::Action(target), None));
                    }
                }
            };
            let first_row = self
                .display
                .iter()
                .filter(|entry| matches!(entry, Entry::Setting { .. } | Entry::Row(_)))
                .position(|entry| matches!(entry, Entry::Row(_)));
            let row_stop = match selected {
                Some(row) if self.curated(row).is_none() => Some(self.selected),
                _ => first_row,
            };
            let mut index = 0;
            for entry in &self.display {
                match entry {
                    Entry::Setting {
                        row,
                        enabled,
                        location,
                        ..
                    } => {
                        if *enabled || selected == Some(*row) {
                            order.push((Focus::List, Some(index)));
                        }
                        if selected == Some(*row) {
                            if self.writable() {
                                chips(*row, &mut order);
                            }
                            if location.is_some() {
                                order.push((Focus::Action(Target::Locate(*row)), None));
                            }
                        }
                        index += 1;
                    }
                    Entry::Row(row) => {
                        if row_stop == Some(index) {
                            order.push((Focus::List, Some(index)));
                            order.push((Focus::Action(Target::RowJson(*row)), None));
                            if self.writable() {
                                if self.rows[*row].next_window() {
                                    order.push((Focus::Action(Target::NewWindow(*row)), None));
                                }
                                chips(*row, &mut order);
                                if self.rows[*row].modified {
                                    order.push((Focus::Action(Target::Reset(*row)), None));
                                }
                            }
                        }
                        index += 1;
                    }
                    Entry::Button { id, .. } => {
                        order.push((Focus::Action(Target::Button(*id)), None));
                    }
                    _ => {}
                }
            }
            if self.error.is_some() && self.path.is_some() {
                order.push((Focus::Action(Target::Repair), None));
            }
            if self.notice.is_some() && self.can_reload() {
                order.push((Focus::Action(Target::Reload), None));
            }
            order
        }

        fn tab(&mut self, backwards: bool) {
            let order = self.focus_order();
            let row_run = self
                .selected_row()
                .is_some_and(|row| self.curated(row).is_none());
            let index = order
                .iter()
                .position(|&(focus, selected)| {
                    self.focus == focus
                        && (selected.is_none() || selected == Some(self.selected) || row_run)
                })
                .unwrap_or(0);
            let (focus, selected) = if backwards {
                order[(index + order.len() - 1) % order.len()]
            } else {
                order[(index + 1) % order.len()]
            };
            self.focus = focus;
            if let Some(selected) = selected {
                self.selected = selected;
            }
            if self.focus == Focus::List {
                self.reveal = true;
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
                let input = self.input_mut().unwrap();
                let before = input.value();
                input.command(cmd);
                self.input_changed(&workspace.translations, before, now);
                return Navigation::Stay;
            }
            match cmd {
                // Escape cancels an edit, then clears the search, then closes the window.
                Cancel => {
                    if self.focus == Focus::Edit {
                        self.edit = None;
                        self.focus = Focus::List;
                    } else if !self.search.value().is_empty() {
                        self.search.set("");
                        self.focus = Focus::Search;
                        self.page_offset = 0.;
                        self.page_offset = 0.;
                        self.filter(None);
                    } else {
                        return Navigation::Close;
                    }
                }
                Tab | Backtab => {
                    if self.edit.as_ref().is_some_and(|edit| edit.conflict) {
                        self.stash_edit();
                    }
                    if self.finish_edit(workspace) {
                        self.tab(cmd == Backtab);
                    }
                }
                Undo if self.input().is_some_and(Input::ime_commit_is_latest)
                    && (self.focus == Focus::Search
                        || self.edit.as_ref().is_some_and(|edit| edit.dirty)) =>
                {
                    let input = self.input_mut().unwrap();
                    let before = input.value();
                    input.command(Undo);
                    self.input_changed(&workspace.translations, before, now);
                }
                Undo => self.undo(workspace),
                Up(false) | Down(false) if self.focus == Focus::Sidebar => {
                    let panes: Vec<_> = Self::panes().collect();
                    let current = panes
                        .iter()
                        .position(|pane| *pane == self.pane)
                        .unwrap_or(0);
                    let index = current
                        .saturating_add_signed(if cmd == Down(false) { 1 } else { -1 })
                        .min(panes.len() - 1);
                    self.show_pane(workspace, panes[index]);
                    self.focus = Focus::Sidebar;
                    self.category_scroll.reveal(index);
                }
                Left(false) | Right(false)
                    if self.focus == Focus::List
                        && self
                            .selected_row()
                            .and_then(|row| self.curated(row))
                            .is_some_and(|(kind, ..)| kind == Kind::Radio) =>
                {
                    let row = self.selected_row().unwrap();
                    self.step_choice(workspace, row, if cmd == Right(false) { 1 } else { -1 });
                }
                Up(false) | Down(false) | PageUp | PageDown
                    if self.focus == Focus::List || self.focus == Focus::Search =>
                {
                    let page = ((self.page_bounds.height / 52.) as isize).max(1);
                    let delta = match cmd {
                        Up(false) => -1,
                        PageUp => -page,
                        PageDown => page,
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
                    self.reveal = true;
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
                        self.reveal = true;
                    }
                    Focus::List => {
                        if let Some(row) = self.selected_row() {
                            self.activate(workspace, row, false);
                        }
                    }
                    Focus::Action(target) => return self.use_target(workspace, target, now),
                },
                DeleteBackward | DeleteForward
                    if self.focus == Focus::List
                        && self
                            .selected_row()
                            .is_some_and(|row| self.curated(row).is_none()) =>
                {
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
            } else {
                self.reveal = false;
                self.page_offset = (self.page_offset + delta)
                    .clamp(0., (self.page_extent - self.page_bounds.height).max(0.));
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
            self.page_offset = 0.;
            self.filter(None);
        }

        /// The curated pages and Advanced, for tests and the scene renderer.
        #[cfg(test)]
        pub(crate) fn pane(&self) -> Pane {
            self.pane
        }

        /// Filters Advanced to one group by its English name (tests).
        #[cfg(test)]
        pub(crate) fn show_group_named(&mut self, name: &str) {
            let tr = Translator::default();
            let group = self.groups.keys().copied().find(|g| g.name(&tr) == name);
            self.category(group);
        }

        /// Gives a control keyboard focus and scrolls it into view (scenes and tests).
        pub(crate) fn focus_setting(&mut self, key: &str) {
            if let Some(row) = self.row_of(key) {
                self.select(row);
                self.focus = Focus::List;
                self.reveal = true;
            }
        }

        /// Hovers the first "?" as if the pointer had rested there (scene renderer).
        pub(crate) fn hover_first_help(&mut self) {
            if let Some(&(frame, _)) = self
                .hits
                .iter()
                .find(|(_, target)| matches!(target, Target::Help(_)))
            {
                let point = Point {
                    x: frame.x + frame.width / 2.,
                    y: frame.y + frame.height / 2.,
                };
                let now = Instant::now();
                self.hover(point, now);
                self.hover_since = now.checked_sub(Duration::from_secs(2));
            }
        }

        pub(crate) fn use_fixture_home(&mut self, home: PathBuf) {
            self.home = Some(home);
        }

        #[cfg(test)]
        pub(crate) fn results(&self) -> impl Iterator<Item = (&str, &str)> {
            self.filtered
                .iter()
                .map(|&i| (self.rows[i].key(), self.rows[i].name.as_str()))
        }

        #[cfg(test)]
        pub(crate) fn localized_copy(&self) -> impl Iterator<Item = (&str, &str, &str)> {
            self.rows
                .iter()
                .map(|row| (row.key(), row.name.as_str(), row.description.as_str()))
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
                || super::view::sample_text(drawn)
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
                        || self
                            .choice_list(id)
                            .iter()
                            .any(|choice| matches(&row.choice_label(choice, tr)))
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
                ("show_side_bar", json!(["auto", true, false])),
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
                (
                    "selection_description_column_type",
                    json!(["display", "character", "utf16", "byte"]),
                ),
                ("control_character_style", json!(["hex", "name", "caret"])),
                ("fold_style", json!(["auto", "indentation"])),
                ("remember_layout", json!(["auto", true, false])),
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

#[cfg(any(target_os = "macos", all(windows, not(test))))]
pub(crate) use model::Navigation;
#[cfg(any(target_os = "macos", windows))]
pub(crate) use model::Page;
#[cfg(target_os = "macos")]
pub(crate) use model::Pane;
#[cfg(all(target_os = "macos", test))]
pub(crate) use model::source_text;
#[cfg(any(target_os = "macos", all(windows, not(test))))]
pub(crate) use pages::platform_args;
#[cfg(all(target_os = "macos", test))]
pub(crate) use pages::{pages as pages_for_tests, platform_text_for};
#[cfg(not(any(target_os = "macos", windows)))]
#[derive(Debug)]
pub(crate) struct Page;
