//! Headless tab/document lifecycle, settings resources and asynchronous file tasks.
use crate::{cli::FileLocation, editor::Editor};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    sync::atomic::Ordering,
    time::{Instant, SystemTime},
};
use vel_buffer::{Region, Selection};
use vel_config::{
    Value,
    json::Document,
    keymap::Keymap,
    resources::Resources,
    settings::{Layer, Settings},
    ui::{self, MenuItem, MouseBinding, PaletteItem},
};
use vel_fileio::{self as fileio, ChangeTracker, Format, LoadTask, Loaded, Watcher};
use vel_text::Text;
#[derive(Debug)]
pub struct Tab {
    pub shared_dirty: bool,
    pub restored_format_dirty: bool,
    pub previous_document: Option<vel_buffer::BufferId>,
    pub restored_document: Option<u64>,
    pub disk_saved: Option<vel_session::DiskStamp>,
    pub sheet: u64,
    pub session_document: u64,
    pub document: vel_buffer::BufferId,
    pub shared_view: vel_buffer::ViewId,
    pub lazy: Option<(vel_session::Document, vel_session::ViewState)>,
    language_settings_revision: u64,
    needs_detection: bool,
    pub transient: bool,
    pub editor: Editor,
    pub path: Option<PathBuf>,
    pub format: Format,
    pub(crate) saved_format: Format,
    pub tracker: ChangeTracker,
    pub scroll: (f64, f64),
    pub loading: Option<LoadTask>,
    pub binary_loading: Option<crate::binary::Task>,
    pub pending_restore: Option<vel_session::ViewState>,
    location: Option<FileLocation>,
    external_load: bool,
    load_started: Option<Instant>,
    pub(crate) disk_stamp: Option<(u64, SystemTime)>,
    pub external: Option<Loaded>,
}
impl Tab {
    pub(crate) fn new(text: Text, settings: &Settings, keymap: &Keymap) -> Result<Self, String> {
        let mut editor = Editor::new(text);
        let tracker = ChangeTracker::new(&editor.buffer);
        editor.settings = settings.clone();
        editor.sync();
        editor.keymap = keymap.clone();
        let format = fileio::default_format(&settings.editor()).unwrap_or_default();
        Ok(Self {
            shared_dirty: false,
            restored_format_dirty: false,
            previous_document: None,
            restored_document: None,
            disk_saved: None,
            sheet: crate::workbench::next_sheet(),
            session_document: crate::workbench::next_sheet(),
            document: editor.buffer.id(),
            shared_view: editor.id,
            lazy: None,
            language_settings_revision: u64::MAX,
            needs_detection: true,
            transient: false,
            editor,
            path: None,
            saved_format: format.clone(),
            format,
            tracker,
            scroll: (0., 0.),
            loading: None,
            binary_loading: None,
            pending_restore: None,
            location: None,
            external_load: false,
            load_started: None,
            disk_stamp: None,
            external: None,
        })
    }
    pub fn install_restored(&mut self, loaded: Loaded) {
        self.install(loaded);
    }
    pub fn dirty(&self) -> bool {
        self.shared_dirty
            || self.restored_format_dirty
            || self.lazy.as_ref().is_some_and(|(d, _)| d.dirty)
            || self.editor.buffer.is_dirty()
            || self.format != self.saved_format
    }
    pub fn label(&self) -> String {
        if let Some(title) = self
            .editor
            .settings
            .get("velocity_html_sheet_title")
            .and_then(Value::as_str)
        {
            return title.into();
        }
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());
        format!(
            "{name}{}{}",
            if self.editor.binary.is_some() {
                " [hex]"
            } else {
                ""
            },
            if self.dirty() { " •" } else { "" }
        )
    }
    pub fn progress(&self) -> Option<(u64, u64)> {
        self.loading
            .as_ref()
            .map(|t| (t.read.load(Ordering::Relaxed), t.total))
            .or_else(|| self.binary_loading.as_ref().map(|t| (0, t.total)))
    }
    fn install(&mut self, loaded: Loaded) {
        self.editor.binary = None;
        self.editor.source_bytes = self.loading.as_ref().map_or(0, |t| t.total);
        self.editor.cancel_composition();
        *self.editor.html.borrow_mut() = Default::default();
        let old = self.editor.buffer.view(self.editor.id).sel().clone();
        let positions: Vec<_> = old
            .iter()
            .map(|r| {
                let p = self.editor.text.byte_to_point(r.b);
                (p.row, p.column)
            })
            .collect();
        self.editor.annotations = std::rc::Rc::new(std::cell::RefCell::new(
            crate::annotations::Annotations::new(loaded.text.clone()),
        ));
        self.editor.buffer = vel_buffer::Buffer::new(loaded.text.clone());
        self.editor.id = self.editor.buffer.new_view();
        self.editor.read_only = false;
        self.editor.sync();
        self.previous_document = Some(self.document);
        self.document = self.editor.buffer.id();
        self.shared_view = self.editor.id;
        self.needs_detection = true;
        self.saved_format = loaded.format.clone();
        self.format = loaded.format;
        self.tracker.mark_saved(&self.editor.buffer);
        let regions = positions.into_iter().map(|(row, column)| {
            Region::caret(
                loaded
                    .text
                    .point_to_byte(loaded.text.clip_point(vel_text::Point { row, column })),
            )
        });
        self.editor.set_selection(Selection::from_regions(regions));
        if self.loading.is_none() {
            if let Some(state) = self.pending_restore.take() {
                state.apply(&mut self.editor.buffer, self.editor.id);
                self.editor.sync();
            }
            self.editor.measure_document();
        }
        self.disk_stamp = self.path.as_deref().and_then(stamp);
        self.disk_saved = self.path.as_deref().and_then(vel_session::DiskStamp::read);
        if let Some(location) = self.location.take() {
            self.goto(&location);
        }
    }
    pub fn goto(&mut self, location: &FileLocation) {
        let text = self.editor.buffer.text();
        let row = location.line.saturating_sub(1).min(text.len_lines() - 1);
        let start = text.line_to_byte(row);
        let end = start + crate::layout::body_len(text, row);
        let chars = text
            .byte_to_char(start)
            .saturating_add(location.column.saturating_sub(1));
        let offset = text.char_to_byte(chars.min(text.len_chars())).min(end) - start;
        self.editor.set_caret(start + offset, false);
    }
}
#[derive(Debug)]
pub struct Workspace {
    pub themes: crate::theme_ui::Themes,
    pub git: crate::git_ui::GitUi,
    pub catalog: Arc<crate::completion::Catalog>,
    pub assets: Arc<crate::language::Assets>,
    syntax_settings: BTreeMap<String, serde_json::Map<String, Value>>,
    schemes: BTreeMap<String, Arc<vel_color_scheme::ColorScheme>>,
    pub dark: bool,
    pub workbench: vel_ui::workbench::Workbench,
    pub distraction_free: bool,
    pub fullscreen: bool,
    pub build_systems: Vec<vel_build::BuildSystem>,
    pub build_system: Option<String>,
    pub project_model: vel_session::project::Project,
    pub recent_files: Vec<PathBuf>,
    pub recent_folders: Vec<PathBuf>,
    pub recent_projects: Vec<PathBuf>,
    pub(crate) distraction_settings: serde_json::Map<String, Value>,
    pub folders: Vec<vel_search::project::Folder>,
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub menus: Vec<MenuItem>,
    pub mousemap: Vec<MouseBinding>,
    pub palette: Vec<PaletteItem>,
    pub messages: Vec<String>,
    pub telemetry: Vec<Value>,
    pub(crate) settings: Settings,
    pub(crate) keymap: Keymap,
    pub(crate) watcher: Option<Watcher>,
    pub(crate) data_dir: Option<PathBuf>,
    pub(crate) project: Option<PathBuf>,
    config_stamp: Vec<(PathBuf, Option<(u64, SystemTime)>)>,
    config_rx: Option<std::sync::mpsc::Receiver<Result<Configuration, String>>>,
    reload_requested: bool,
    settings_writes: Vec<std::sync::mpsc::Receiver<Result<(), String>>>,
    pub resource_revision: u64,
}
fn stamp(path: &Path) -> Option<(u64, SystemTime)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()?))
}
fn package_stamps(
    dir: &Path,
    out: &mut Vec<(PathBuf, Option<(u64, SystemTime)>)>,
    seen: &mut std::collections::BTreeSet<PathBuf>,
) {
    let mut pending = vec![dir.to_owned()];
    while let Some(dir) = pending.pop() {
        let Ok(canonical) = std::fs::canonicalize(&dir) else {
            continue;
        };
        if !seen.insert(canonical) {
            continue;
        }
        out.push((dir.clone(), stamp(&dir)));
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.to_str().is_some_and(|p| {
                    [
                        ".sublime-syntax",
                        ".tmLanguage",
                        ".tmLanguage.json",
                        ".tmPreferences",
                    ]
                    .iter()
                    .any(|s| p.ends_with(s))
                }) {
                    out.push((path.clone(), stamp(&path)));
                }
            }
        }
    }
}
fn absolute(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        std::fs::canonicalize(path).map_err(|e| e.to_string())
    } else if path.is_absolute() {
        Ok(path.into())
    } else {
        std::env::current_dir()
            .map(|p| p.join(path))
            .map_err(|e| e.to_string())
    }
}
#[derive(Debug)]
struct Configuration {
    themes: crate::theme_ui::Prepared,
    #[cfg(test)]
    resources: Resources,
    settings: Settings,
    keymap: Keymap,
    syntax_settings: BTreeMap<String, serde_json::Map<String, Value>>,
    schemes: BTreeMap<String, Arc<vel_color_scheme::ColorScheme>>,
    assets: Arc<crate::language::Assets>,
    catalog: Arc<crate::completion::Catalog>,
    menus: Vec<MenuItem>,
    mousemap: Vec<MouseBinding>,
    palette: Vec<PaletteItem>,
    folders: Vec<vel_search::project::Folder>,
    distraction_settings: serde_json::Map<String, Value>,
    build_systems: Vec<vel_build::BuildSystem>,
    project_model: vel_session::project::Project,
    config_stamp: Vec<(PathBuf, Option<(u64, SystemTime)>)>,
    messages: Vec<String>,
}
impl Configuration {
    fn load(data_dir: Option<PathBuf>, project: Option<PathBuf>) -> Result<Self, String> {
        let mut resources = Resources::default();
        crate::theme_ui::bundled(&mut resources);
        for (name, source) in [
            (
                "Rust Function.sublime-snippet",
                include_str!("../resources/Packages/Default/Rust Function.sublime-snippet"),
            ),
            (
                "Preferences.sublime-settings",
                include_str!("../resources/Packages/Default/Preferences.sublime-settings"),
            ),
            (
                "Preferences (OSX).sublime-settings",
                include_str!("../resources/Packages/Default/Preferences (OSX).sublime-settings"),
            ),
            (
                "Velocity.sublime-keymap",
                include_str!("../resources/Packages/Default/Velocity.sublime-keymap"),
            ),
            (
                "Velocity Vintage.sublime-keymap",
                include_str!("../resources/Packages/Default/Velocity Vintage.sublime-keymap"),
            ),
            (
                "Main.sublime-menu",
                include_str!("../resources/Packages/Default/Main.sublime-menu"),
            ),
            (
                "Velocity.sublime-mousemap",
                include_str!("../resources/Packages/Default/Velocity.sublime-mousemap"),
            ),
            (
                "Velocity.sublime-commands",
                include_str!("../resources/Packages/Default/Velocity.sublime-commands"),
            ),
        ] {
            resources.insert(
                &format!("Packages/Default/{name}"),
                source.as_bytes().to_vec(),
                PathBuf::from("<embedded>"),
            );
        }
        for (name, source) in [
            (
                "Velocity Dark.sublime-color-scheme",
                include_str!(
                    "../resources/Packages/Color Scheme - Velocity/Velocity Dark.sublime-color-scheme"
                ),
            ),
            (
                "Velocity Light.sublime-color-scheme",
                include_str!(
                    "../resources/Packages/Color Scheme - Velocity/Velocity Light.sublime-color-scheme"
                ),
            ),
        ] {
            resources.insert(
                &format!("Packages/Color Scheme - Velocity/{name}"),
                source.as_bytes().to_vec(),
                PathBuf::from("<embedded>"),
            );
        }
        let mut resource_messages = Vec::new();
        resources.load_syntaxes(&crate::language::packages_root())?;
        if let Some(bundle) = crate::release::bundle_resources()
            && let Err(error) = resources.load(&bundle)
        {
            resource_messages.push(error);
        }
        if let Some(data) = &data_dir
            && let Err(e) = resources.load(data)
        {
            resource_messages.push(format!("user resources: {e}"));
        }
        // Resolve ignored_packages before loading any resource category. Reading
        // the preferences first is necessary to allow re-enabling a package.
        let mut ignored = std::collections::BTreeSet::new();
        for resource in resources.matching(".sublime-settings") {
            if (resource.path.starts_with("Packages/Default/")
                || resource.path.starts_with("Packages/User/"))
                && (resource.path.ends_with("/Preferences.sublime-settings")
                    || resource
                        .path
                        .ends_with("/Preferences (OSX).sublime-settings"))
                && let Ok(doc) = resource.text().and_then(Document::parse)
                && let Some(values) = doc.value.get("ignored_packages").and_then(Value::as_array)
            {
                ignored = values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
            }
        }
        if let Some(project) = &project
            && let Ok(source) = std::fs::read_to_string(project)
            && let Ok(doc) = Document::parse(&source)
            && let Some(values) = doc.value["settings"]["ignored_packages"].as_array()
        {
            ignored = values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
        }
        resources.set_ignored(ignored);
        let mut settings = Settings::default();
        let mut user = serde_json::Map::new();
        let mut syntax = serde_json::Map::new();
        let mut syntax_settings: BTreeMap<String, serde_json::Map<String, Value>> = BTreeMap::new();
        let mut config_stamp = Vec::new();
        for resource in resources.matching(".sublime-settings") {
            let file = resource.path.rsplit('/').next().unwrap_or("");
            let doc = match resource.text().and_then(Document::parse) {
                Ok(doc) => doc,
                Err(e) => {
                    resource_messages.push(format!("{}: {e}", resource.path));
                    continue;
                }
            };
            if !doc.value.is_object() {
                resource_messages.push(format!("{}: settings must be an object", resource.path));
                continue;
            }
            if resource.origin.exists() {
                config_stamp.push((resource.origin.clone(), stamp(&resource.origin)));
            }
            match resource.path.as_str() {
                "Packages/Default/Preferences.sublime-settings" => {
                    settings.replace(Layer::Default, doc.value)?
                }
                "Packages/Default/Preferences (OSX).sublime-settings" => {
                    settings.replace(Layer::Platform, doc.value)?
                }
                _ => {
                    if file == "Preferences.sublime-settings"
                        || file == "Preferences (OSX).sublime-settings"
                    {
                        user.extend(doc.value.as_object().ok_or("settings object")?.clone());
                    } else if file == "Plain Text.sublime-settings" {
                        syntax.extend(doc.value.as_object().ok_or("settings object")?.clone());
                    }
                    if let Some(name) = file
                        .strip_suffix(".sublime-settings")
                        .filter(|n| !n.starts_with("Preferences"))
                    {
                        syntax_settings
                            .entry(name.to_owned())
                            .or_default()
                            .extend(doc.value.as_object().ok_or("settings object")?.clone());
                    }
                }
            }
        }
        settings.replace(Layer::User, Value::Object(user))?;
        settings.replace(Layer::Syntax, Value::Object(syntax))?;
        let mut distraction_settings = serde_json::json!({"line_numbers":false,"gutter":false,"draw_centered":true,"wrap_width":80,"word_wrap":true,"scroll_past_end":true,"show_minimap":false}).as_object().expect("object").clone();
        for resource in resources.matching("Distraction Free.sublime-settings") {
            if let Ok(doc) = resource.text().and_then(Document::parse)
                && let Some(values) = doc.value.as_object()
            {
                distraction_settings.extend(values.clone());
            }
        }
        let mut build_systems = Vec::new();
        for resource in resources.matching(".sublime-build") {
            let name = resource
                .path
                .rsplit('/')
                .next()
                .unwrap_or("")
                .trim_end_matches(".sublime-build");
            match resource
                .text()
                .and_then(|source| vel_build::BuildSystem::parse(name, source))
            {
                Ok(build) => build_systems.push(build),
                Err(e) => resource_messages.push(format!("{}: {e}", resource.path)),
            }
        }
        // Public permissively licensed build resources are loaded as-is.
        for (name, source) in [
            (
                "Cargo",
                include_str!("../../../third_party/sublime-packages/Rust/Cargo.sublime-build"),
            ),
            (
                "Python",
                include_str!("../../../third_party/sublime-packages/Python/Python.sublime-build"),
            ),
            (
                "Make",
                include_str!("../../../third_party/sublime-packages/Makefile/Make.sublime-build"),
            ),
        ] {
            if resources.package_enabled(match name {
                "Cargo" => "Rust",
                "Python" => "Python",
                _ => "Makefile",
            }) && !build_systems.iter().any(|b| b.name == name)
            {
                build_systems.push(vel_build::BuildSystem::parse(name, source)?);
            }
        }
        let project_model = project
            .as_deref()
            .map(vel_session::project::Project::load)
            .transpose()?
            .unwrap_or_default();
        for value in &project_model.build_systems {
            let name = value["name"]
                .as_str()
                .ok_or("project build system needs name")?;
            build_systems.push(vel_build::BuildSystem::from_value(name, value.clone())?);
        }
        let mut folder_values = Vec::new();
        if let Some(path) = &project {
            let doc = Document::parse(&std::fs::read_to_string(path).map_err(|e| e.to_string())?)?;
            settings.replace(
                Layer::Project,
                doc.value
                    .get("settings")
                    .cloned()
                    .unwrap_or(serde_json::json!({})),
            )?;
            config_stamp.push((path.clone(), stamp(path)));
            folder_values = doc
                .value
                .get("folders")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
        }
        let mut folders = Vec::new();
        let patterns = |value: Option<&Value>| -> Vec<String> {
            value
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        for folder in folder_values {
            let path = PathBuf::from(folder["path"].as_str().ok_or("project folder needs path")?);
            let path = if path.is_absolute() {
                path
            } else {
                project
                    .as_ref()
                    .and_then(|p| p.parent())
                    .unwrap_or(Path::new("."))
                    .join(path)
            };
            let path = absolute(&path)?;
            let mut excludes = Vec::new();
            for key in [
                "file_exclude_patterns",
                "folder_exclude_patterns",
                "binary_file_patterns",
            ] {
                let mut list = patterns(settings.get(key));
                list.extend(patterns(folder.get(key)));
                excludes.push(list);
            }
            folders.push(vel_search::project::Folder {
                name: folder["name"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        path.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned()
                    }),
                path,
                rules: vel_search::project::Rules::new(&excludes[0], &excludes[1], &excludes[2])?,
                follow_links: folder["follow_symlinks"].as_bool().unwrap_or(false),
            });
        }
        let mut keys = Vec::new();
        let mut menus = Vec::new();
        let mut mousemap = Vec::new();
        let mut palette = Vec::new();
        for resource in resources.matching(".sublime-keymap") {
            if resource.path.contains("(Windows)") || resource.path.contains("(Linux)") {
                continue;
            }
            let doc = match resource.text().and_then(Document::parse) {
                Ok(doc) => doc,
                Err(e) => {
                    resource_messages.push(format!("{}: {e}", resource.path));
                    continue;
                }
            };
            let check = resource.text().and_then(|s| Keymap::default().append(s));
            if let Err(e) = check {
                resource_messages.push(format!("{}: {e}", resource.path));
                continue;
            }
            if let Some(values) = doc.value.as_array() {
                keys.extend(values.clone());
            }
            if resource.origin.exists() {
                config_stamp.push((resource.origin.clone(), stamp(&resource.origin)));
            }
        }
        for resource in resources.matching(".sublime-menu") {
            if resource.path.ends_with("/Main.sublime-menu") {
                match resource.text().and_then(ui::parse) {
                    Ok(items) => ui::merge_menu(&mut menus, items),
                    Err(e) => resource_messages.push(format!("{}: {e}", resource.path)),
                }
            }
        }
        for resource in resources.matching(".sublime-mousemap") {
            match resource.text().and_then(ui::parse_mousemap) {
                Ok(items) => mousemap.extend(items),
                Err(e) => resource_messages.push(format!("{}: {e}", resource.path)),
            }
        }
        for resource in resources.matching(".sublime-commands") {
            match resource.text().and_then(ui::parse::<PaletteItem>) {
                Ok(items) => palette.extend(items),
                Err(e) => resource_messages.push(format!("{}: {e}", resource.path)),
            }
        }
        if let Some(tools) = menus.iter_mut().find(|m| m.id == "tools") {
            let choices=std::iter::once(("Automatic",serde_json::json!({"name":"Automatic"}))).chain(build_systems.iter().map(|b|(b.name.as_str(),serde_json::json!({"name":b.name})))).map(|(caption,args)|serde_json::json!({"caption":caption,"command":"set_build_system","args":args})).collect::<Vec<_>>();
            if let Some(item) = tools
                .children
                .iter_mut()
                .find(|m| m.command.as_deref() == Some("show_build_systems"))
            {
                item.caption = "Build System".into();
                item.command = None;
                item.children = serde_json::from_value(serde_json::json!(choices))
                    .map_err(|e| e.to_string())?;
            }
        }
        crate::release::extend_commands(&mut palette, &mut menus, &settings);
        ui::extend_palette(&mut palette, &menus);
        let bundled = crate::language::Assets::bundled()?;
        let assets =
            crate::language::Assets::with_user_packages(&bundled, data_dir.as_deref(), &resources)?;
        resource_messages.extend(assets.syntaxes.warnings().iter().cloned());
        for syntax in assets.syntaxes.syntaxes().iter().filter(|s| !s.hidden) {
            palette.push(PaletteItem {
                caption: format!("Set Syntax: {}", syntax.name),
                invocation: vel_config::keymap::Invocation {
                    command: "set_file_type".into(),
                    args: serde_json::json!({"syntax": syntax.path}),
                },
            });
        }
        for suffix in [".sublime-snippet", ".sublime-completions"] {
            for resource in resources.matching(suffix) {
                if resource.origin != Path::new("<embedded>") {
                    config_stamp.push((resource.origin.clone(), stamp(&resource.origin)));
                }
            }
        }
        let catalog = crate::completion::Catalog::load(&resources, &mut resource_messages);
        for (name, snippet) in &catalog.snippets {
            palette.push(PaletteItem {
                caption: format!(
                    "Snippet: {}",
                    if snippet.description.is_empty() {
                        name.as_str()
                    } else {
                        &snippet.description
                    }
                ),
                invocation: vel_config::keymap::Invocation {
                    command: "insert_snippet".into(),
                    args: serde_json::json!({"name":name}),
                },
            });
        }
        let mut scheme_resources = Vec::new();
        for suffix in [".sublime-color-scheme", ".hidden-color-scheme", ".tmTheme"] {
            for resource in resources.matching(suffix) {
                if resource.origin.exists() {
                    config_stamp.push((resource.origin.clone(), stamp(&resource.origin)));
                }
                match resource.text() {
                    Ok(source) => scheme_resources.push((resource.path.clone(), source.to_owned())),
                    Err(e) => resource_messages.push(e),
                }
            }
        }
        let mut schemes = BTreeMap::new();
        for (path, _) in &scheme_resources {
            let name = path.rsplit('/').next().expect("resource filename");
            if !schemes.contains_key(name) {
                let scheme = vel_color_scheme::load_resources(
                    scheme_resources
                        .iter()
                        .map(|(p, s)| (p.as_str(), s.as_str())),
                    name,
                )
                .map_err(|e| e.to_string());
                match scheme {
                    Ok(scheme) => {
                        schemes.insert(name.to_owned(), Arc::new(scheme));
                    }
                    Err(e) => resource_messages.push(format!("{name}: {e}")),
                }
            }
        }
        for (name, source) in [
            (
                "Velocity Dark.sublime-color-scheme",
                include_str!(
                    "../resources/Packages/Color Scheme - Velocity/Velocity Dark.sublime-color-scheme"
                ),
            ),
            (
                "Velocity Light.sublime-color-scheme",
                include_str!(
                    "../resources/Packages/Color Scheme - Velocity/Velocity Light.sublime-color-scheme"
                ),
            ),
        ] {
            if !schemes.contains_key(name) {
                let scheme = vel_color_scheme::load_resources([(name, source)], name)
                    .map_err(|e| e.to_string())?;
                schemes.insert(name.to_owned(), Arc::new(scheme));
            }
        }
        // Track resource files and containing directories, including absent User
        // locations, so creating a new override is observable on the next poll.
        for suffix in [
            ".sublime-syntax",
            ".tmLanguage",
            ".tmLanguage.json",
            ".tmPreferences",
            ".sublime-menu",
            ".sublime-mousemap",
            ".sublime-commands",
            ".sublime-theme",
            ".sublime-file-icons",
            ".png",
        ] {
            for resource in resources.matching(suffix) {
                if resource.origin.exists() {
                    config_stamp.push((resource.origin.clone(), stamp(&resource.origin)));
                }
            }
        }
        let parents: Vec<_> = config_stamp
            .iter()
            .filter_map(|(p, _)| {
                if project.as_ref() == Some(p) {
                    None
                } else {
                    p.parent().map(Path::to_owned)
                }
            })
            .collect();
        for parent in parents {
            config_stamp.push((parent.clone(), stamp(&parent)));
        }
        if let Some(data) = &data_dir {
            // Observe resource locations directly. Cache/Sessions children must not reload
            // project settings or discard dynamically added folders. Missing paths are
            // tracked as None, so their first creation is still detected.
            for path in [
                data.join("Packages"),
                data.join("Packages/User"),
                data.join("Installed Packages"),
            ] {
                let current = stamp(&path);
                config_stamp.push((path, current));
            }
        }
        if let Some(data) = &data_dir {
            package_stamps(
                &data.join("Packages"),
                &mut config_stamp,
                &mut std::collections::BTreeSet::new(),
            );
        }
        config_stamp.retain(|(p, _)| {
            data_dir.as_ref().is_some_and(|d| p.starts_with(d)) || project.as_ref() == Some(p)
        });
        config_stamp.sort_by(|a, b| a.0.cmp(&b.0));
        config_stamp.dedup_by(|a, b| a.0 == b.0);
        let keymap_source = serde_json::to_string(&keys).map_err(|e| e.to_string())?;
        let mut keymap = Keymap::default();
        keymap.append(&keymap_source)?;
        let themes = crate::theme_ui::Themes::prepare(
            &resources,
            &schemes,
            &settings,
            &mut resource_messages,
        );
        Ok(Self {
            themes,
            #[cfg(test)]
            resources,
            settings,
            keymap,
            syntax_settings,
            schemes,
            assets,
            catalog,
            menus,
            mousemap,
            palette,
            folders,
            distraction_settings,
            build_systems,
            project_model,
            config_stamp,
            messages: resource_messages,
        })
    }
}
impl Workspace {
    pub fn new(
        text: Text,
        data_dir: Option<PathBuf>,
        project: Option<PathBuf>,
        watch: bool,
    ) -> Result<Self, String> {
        let Configuration {
            themes,
            #[cfg(test)]
                resources: _,
            settings,
            keymap,
            syntax_settings,
            schemes,
            assets,
            catalog,
            menus,
            mousemap,
            palette,
            folders,
            distraction_settings,
            build_systems,
            project_model,
            config_stamp,
            messages: resource_messages,
        } = Configuration::load(data_dir.clone(), project.clone())?;
        let tab = Tab::new(text, &settings, &keymap)?;
        let mut messages = resource_messages;
        let watcher = if watch {
            match Watcher::new() {
                Ok(w) => Some(w),
                Err(e) => {
                    messages.push(format!("file notifications unavailable: {e}"));
                    None
                }
            }
        } else {
            None
        };
        let mut themes = crate::theme_ui::Themes::publish(themes);
        if let Err(e) = themes.select(&settings, true) {
            messages.push(e);
        }
        let mut workspace = Self {
            themes,
            git: Default::default(),
            catalog,
            assets,
            syntax_settings,
            schemes,
            dark: true,
            workbench: vel_ui::workbench::Workbench::default(),
            distraction_free: false,
            fullscreen: false,
            build_systems,
            build_system: None,
            project_model,
            distraction_settings,
            recent_files: Vec::new(),
            recent_folders: Vec::new(),
            recent_projects: Vec::new(),
            folders,
            tabs: vec![tab],
            active: 0,
            menus,
            mousemap,
            palette,
            messages,
            telemetry: Vec::new(),
            settings,
            keymap,
            watcher,
            data_dir,
            project,
            config_stamp,
            config_rx: None,
            reload_requested: false,
            settings_writes: Vec::new(),
            resource_revision: 0,
        };
        workspace.workbench.groups[0].add(workspace.tabs[0].sheet, false);
        workspace.configure_language(0, None, true)?;
        Ok(workspace)
    }
    pub fn track_settings_write(&mut self, rx: std::sync::mpsc::Receiver<Result<(), String>>) {
        self.settings_writes.push(rx);
    }
    #[cfg(test)]
    pub fn wait_resources(&mut self) {
        let deadline = Instant::now() + std::time::Duration::from_secs(120);
        loop {
            self.poll();
            if !self.resources_busy() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "resource worker did not finish: {:?}",
                self.messages
            );
            std::thread::park_timeout(std::time::Duration::from_millis(1));
        }
    }
    pub fn request_resource_reload(&mut self) {
        self.reload_requested = true;
    }
    pub fn resources_busy(&self) -> bool {
        self.config_rx.is_some() || self.reload_requested || !self.settings_writes.is_empty()
    }
    pub fn ignored_packages(&self) -> std::collections::BTreeSet<String> {
        self.settings
            .get("ignored_packages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    }
    pub fn editor(&self) -> &Editor {
        &self.tabs[self.active].editor
    }
    pub fn editor_mut(&mut self) -> &mut Editor {
        if let Err(e) = self.prepare_sheet(self.active) {
            self.messages.push(e);
        }
        &mut self.tabs[self.active].editor
    }
    pub fn language_busy(&self) -> bool {
        self.config_rx.is_some()
            || self.reload_requested
            || self.tabs.iter().any(|tab| {
                tab.loading.is_some()
                    || tab.binary_loading.is_some()
                    || tab.editor.layout.borrow().long.pending()
                    || tab.editor.reindent_job.is_some()
                    || tab
                        .editor
                        .language
                        .borrow()
                        .as_ref()
                        .is_some_and(|l| l.busy())
            })
    }
    pub fn new_file(&mut self) -> Result<(), String> {
        self.tabs
            .push(Tab::new(Text::new(), &self.settings, &self.keymap)?);
        self.workbench.groups[self.workbench.active]
            .add(self.tabs.last().expect("tab").sheet, false);
        self.active = self.tabs.len() - 1;
        self.configure_language(self.active, None, false)?;
        Ok(())
    }
    pub fn select(&mut self, index: usize) {
        if index < self.tabs.len() {
            if index != self.active {
                self.editor_mut().completion.hide();
            }
            self.editor_mut().unmark();
            self.editor_mut().keymap.cancel();
            self.activate_sheet(index);
        }
    }
    pub fn open(&mut self, location: FileLocation) -> Result<(), String> {
        self.open_mode(location, false)
    }
    pub fn preview(&mut self, location: FileLocation) -> Result<(), String> {
        let target = absolute(&location.path)?;
        if !self.tabs.iter().any(|t| t.path.as_ref() == Some(&target))
            && let Some(index) = self.tabs.iter().position(|t| {
                t.transient
                    && !t.dirty()
                    && self.workbench.group_of(t.sheet) == Some(self.workbench.active)
            })
        {
            self.close(index)?;
        }
        self.open_mode(location, true)
    }
    fn open_mode(&mut self, location: FileLocation, preview: bool) -> Result<(), String> {
        let path = absolute(&location.path)?;
        if let Some(index) = self
            .tabs
            .iter()
            .position(|t| t.path.as_ref() == Some(&path))
        {
            self.select(index);
            if !preview {
                self.tabs[index].transient = false;
            }
            self.tabs[index].goto(&location);
            return Ok(());
        }
        let started = Instant::now();
        let mut tab = Tab::new(Text::new(), &self.settings, &self.keymap)?;
        tab.transient = preview;
        tab.path = Some(path.clone());
        tab.location = Some(location);
        let fallback = tab.editor.settings.editor().fallback_encoding;
        let size = std::fs::metadata(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .len();
        if crate::binary::is_binary(&path)? {
            tab.editor.source_bytes = size;
            tab.editor.refresh_policy();
            tab.editor.read_only = true;
            tab.binary_loading = Some(crate::binary::Task::open(path.clone())?);
        } else if size
            >= tab
                .editor
                .settings
                .usize("large_file.load_bytes", 64 * 1024 * 1024) as u64
        {
            tab.load_started = Some(started);
            tab.editor.source_bytes = size;
            tab.editor.refresh_policy();
            tab.editor.read_only = true;
            tab.loading = Some(fileio::load_background(path.clone(), fallback)?);
        } else {
            tab.install(fileio::load(&path, &fallback, |_| {})?);
        }
        self.telemetry.push(serde_json::json!({"kind":"file_open","phase":if tab.loading.is_some(){"queued"}else{"fully_loaded"},"source_bytes":size,"elapsed_ms":started.elapsed().as_secs_f64()*1000.,"background":tab.loading.is_some(),"path":path}));
        if let Some(w) = self.watcher.as_mut() {
            w.watch(&path)?;
        }
        if self.tabs.len() == 1
            && !preview
            && self.tabs[0].path.is_none()
            && !self.tabs[0].editor.buffer.is_dirty()
            && self.tabs[0].editor.text.len_bytes() == 0
        {
            self.workbench
                .groups
                .iter_mut()
                .for_each(|g| g.remove(self.tabs[0].sheet));
            self.tabs[0] = tab;
            self.active = 0;
        } else {
            self.tabs.push(tab);
            self.active = self.tabs.len() - 1;
        }
        self.workbench.groups[self.workbench.active].add(self.tabs[self.active].sheet, false);
        crate::workbench::remember(&mut self.recent_files, path);
        self.configure_language(self.active, None, true)?;
        Ok(())
    }
    pub fn save(&mut self, index: usize, path: Option<PathBuf>) -> Result<(), String> {
        self.prepare_sheet(index)?;
        let tab = &mut self.tabs[index];
        tab.transient = false;
        if tab.loading.is_some() || tab.binary_loading.is_some() || tab.editor.read_only {
            return Err("file is loading or its load failed".into());
        }
        tab.editor.unmark();
        let target = path
            .or_else(|| tab.path.clone())
            .ok_or("save requires a path")?;
        let target = absolute(&target)?;
        let settings = tab.editor.settings.editor();
        fileio::prepare_save(
            &mut tab.editor.buffer,
            tab.editor.id,
            &settings,
            &tab.tracker,
        );
        tab.editor.sync();
        fileio::save(
            &target,
            tab.editor.buffer.text(),
            &tab.format,
            settings.atomic_save,
        )?;
        tab.editor.buffer.mark_saved();
        tab.editor.sync();
        tab.saved_format = tab.format.clone();
        tab.restored_format_dirty = false;
        tab.tracker.mark_saved(&tab.editor.buffer);
        tab.path = Some(target.clone());
        tab.disk_stamp = stamp(&target);
        tab.disk_saved = vel_session::DiskStamp::read(&target);
        tab.editor.annotations.borrow_mut().reference(
            tab.editor.buffer.text().clone(),
            tab.path.clone(),
            self.data_dir.as_ref().map(|d| d.join("Packages")),
        );
        if let Some(w) = self.watcher.as_mut() {
            w.watch(&target)?;
        }
        if let Err(e) = self.configure_language(index, None, true) {
            self.messages.push(e);
        }
        Ok(())
    }
    pub fn close(&mut self, index: usize) -> Result<(), String> {
        self.prepare_close_sheet(index)?;
        let sheet = self.tabs[index].sheet;
        for group in &mut self.workbench.groups {
            group.remove(sheet);
        }
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.new_file()?;
        }
        self.active = self.active.min(self.tabs.len() - 1);
        self.select(self.active);
        Ok(())
    }
    /// Polls jobs/notifications and reports whether scene content changed.
    pub fn poll(&mut self) -> bool {
        let enabled = self.settings.bool("show_git_status", true);
        let mut roots: Vec<_> = self.folders.iter().map(|f| f.path.clone()).collect();
        for tab in &self.tabs {
            if let Some(parent) = tab.path.as_deref().and_then(Path::parent)
                && !roots.iter().any(|root| parent.starts_with(root))
            {
                roots.push(parent.to_owned());
            }
        }
        let mut changed = self.git.poll(enabled, roots);
        if changed {
            for tab in &mut self.tabs {
                if !tab.editor.shared_shadow {
                    tab.editor.annotations.borrow_mut().refresh_reference();
                }
            }
        }
        for tab in &mut self.tabs {
            if tab.lazy.is_some() || tab.editor.shared_shadow {
                continue;
            }
            changed |= tab.editor.poll_language();
            changed |= tab.editor.annotations.borrow_mut().poll();
            if let Some(error) = tab
                .editor
                .annotations
                .borrow_mut()
                .current
                .as_mut()
                .and_then(|p| p.error.take())
            {
                self.messages.push(error);
            }
            changed |= tab.editor.poll_completion();
            if let Some(language) = tab.editor.language.borrow_mut().as_mut() {
                self.messages.append(&mut language.errors);
            }
            if tab.dirty() {
                tab.transient = false;
            }
            if let Some(snapshot) = tab.loading.as_ref().and_then(LoadTask::try_snapshot)
                && !tab.external_load
            {
                let source_bytes = tab.loading.as_ref().map_or(0, |t| t.total);
                let pending_location = tab.location.take();
                tab.install(Loaded {
                    text: snapshot.text,
                    format: Format::default(),
                    source_bytes,
                });
                tab.location = pending_location;
                tab.editor.source_bytes = source_bytes;
                tab.editor.document_metrics = snapshot.metrics;
                tab.editor.read_only = true;
                tab.editor.refresh_policy();
                self.telemetry.push(serde_json::json!({"kind":"file_open","phase":"prefix","elapsed_ms":tab.load_started.map(|s|s.elapsed().as_secs_f64()*1000.),"loaded_bytes":tab.editor.text.len_bytes(),"source_bytes":source_bytes}));
                changed = true;
            }
            if let Some(result) =
                tab.binary_loading
                    .as_ref()
                    .and_then(|t| match t.result.try_recv() {
                        Ok(result) => Some(result),
                        Err(std::sync::mpsc::TryRecvError::Empty) => None,
                        Err(e) => Some(Err(e.to_string())),
                    })
            {
                tab.binary_loading = None;
                match result {
                    Ok(snapshot) => {
                        *tab.editor.language.borrow_mut() = None;
                        tab.editor.binary = Some(snapshot);
                        tab.editor.read_only = true;
                        tab.editor.caret = 0;
                        tab.editor.anchor = 0;
                        tab.pending_restore = None;
                        tab.needs_detection = false;
                        tab.disk_stamp = tab.path.as_deref().and_then(stamp);
                    }
                    Err(e) => self.messages.push(e),
                }
                changed = true;
            }
            let result = tab
                .loading
                .as_ref()
                .and_then(|t| match t.result.try_recv() {
                    Ok(v) => Some(v),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(e) => Some(Err(format!("file worker disconnected: {e}"))),
                });
            if let Some(result) = result {
                self.telemetry.push(serde_json::json!({"kind":"file_open","phase":"fully_loaded","elapsed_ms":tab.load_started.map(|s|s.elapsed().as_secs_f64()*1000.),"source_bytes":tab.loading.as_ref().map(|t|t.total),"path":tab.path,"success":result.is_ok(),"background":true}));
                tab.load_started = None;
                tab.loading = None;
                match result {
                    Ok(loaded) => {
                        if tab.external_load && (tab.dirty() || tab.editor.marked.is_some()) {
                            tab.external = Some(loaded);
                            tab.editor.read_only = false;
                        } else {
                            tab.install(loaded);
                        }
                        tab.external_load = false;
                    }
                    Err(e) => {
                        if !tab.external_load {
                            tab.install(Loaded {
                                text: Text::new(),
                                format: Format::default(),
                                source_bytes: 0,
                            });
                            tab.editor.read_only = true;
                        }
                        self.messages.push(e);
                    }
                }
                changed = true;
            }
        }
        let events = self
            .watcher
            .as_ref()
            .map(Watcher::changed)
            .unwrap_or_default();
        for event in events {
            match event {
                Err(e) => self.messages.push(e),
                Ok(paths) => {
                    for path in paths {
                        for tab in &mut self.tabs {
                            if tab.loading.is_some()
                                || tab.binary_loading.is_some()
                                || tab.lazy.is_some()
                                || tab.editor.shared_shadow
                                || tab.path.as_ref() != Some(&path)
                                || tab.disk_stamp == stamp(&path)
                            {
                                continue;
                            }
                            tab.disk_stamp = stamp(&path);
                            if tab.editor.binary.is_some() {
                                match crate::binary::Task::open(path.clone()) {
                                    Ok(task) => tab.binary_loading = Some(task),
                                    Err(e) => self.messages.push(e),
                                }
                                continue;
                            }
                            if std::fs::metadata(&path).is_ok_and(|m| m.len() > 64 * 1024 * 1024) {
                                match fileio::load_background(
                                    path.clone(),
                                    tab.editor.settings.editor().fallback_encoding,
                                ) {
                                    Ok(task) => {
                                        tab.load_started = Some(Instant::now());
                                        tab.external_load = true;
                                        tab.loading = Some(task);
                                    }
                                    Err(e) => self.messages.push(e),
                                }
                                continue;
                            }
                            match fileio::load(
                                &path,
                                &tab.editor.settings.editor().fallback_encoding,
                                |_| {},
                            ) {
                                Ok(loaded) => {
                                    if tab.tracker.is_saved(&loaded.text)
                                        && tab.format == loaded.format
                                    {
                                        continue;
                                    }
                                    if tab.dirty() || tab.editor.marked.is_some() {
                                        tab.external = Some(loaded);
                                    } else {
                                        tab.install(loaded);
                                    }
                                    changed = true;
                                }
                                Err(e) => self.messages.push(e),
                            }
                        }
                    }
                }
            }
        }
        self.reconcile_installs();
        self.settings_writes.retain(|rx| match rx.try_recv() {
            Ok(Ok(())) => {
                self.reload_requested = true;
                false
            }
            Ok(Err(error)) => {
                self.messages.push(error);
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(error) => {
                self.messages.push(error.to_string());
                false
            }
        });
        if self.config_rx.is_none()
            && (self.reload_requested || self.config_stamp.iter().any(|(p, s)| stamp(p) != *s))
        {
            self.reload_requested = false;
            let (tx, rx) = std::sync::mpsc::channel();
            let data = self.data_dir.clone();
            let project = self.project.clone();
            match std::thread::Builder::new()
                .name("velocity-resources".into())
                .spawn(move || {
                    let _ = tx.send(Configuration::load(data, project));
                }) {
                Ok(_) => self.config_rx = Some(rx),
                Err(error) => self.messages.push(error.to_string()),
            }
        }
        let config_result = self.config_rx.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(error) => Some(Err(error.to_string())),
        });
        if let Some(result) = config_result {
            self.config_rx = None;
            let project_changed = self.project.as_ref().is_some_and(|project| {
                self.config_stamp
                    .iter()
                    .any(|(path, saved)| path == project && stamp(path) != *saved)
            });
            let folders = if project_changed {
                Vec::new()
            } else {
                self.folders.clone()
            };
            match result {
                Ok(mut config) => {
                    self.messages.append(&mut config.messages);
                    self.themes = crate::theme_ui::Themes::publish(config.themes);
                    if let Err(e) = self.themes.select(&config.settings, self.dark) {
                        self.messages.push(e);
                    }
                    self.resource_revision = self.resource_revision.wrapping_add(1);
                    self.assets = config.assets;
                    self.catalog = config.catalog;
                    self.syntax_settings = config.syntax_settings;
                    self.schemes = config.schemes;
                    self.folders = config.folders;
                    self.distraction_settings = config.distraction_settings;
                    self.build_systems = config.build_systems;
                    self.project_model = config.project_model;
                    self.settings = config.settings;
                    // User resource changes must preserve folders added at runtime. An
                    // actual project-file edit still replaces its configured folder list.
                    for mut folder in folders {
                        if self.folders.iter().any(|f| f.path == folder.path) {
                            continue;
                        }
                        let patterns = |key| {
                            self.settings
                                .get(key)
                                .and_then(Value::as_array)
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str)
                                .map(str::to_owned)
                                .collect::<Vec<_>>()
                        };
                        match vel_search::project::Rules::new(
                            &patterns("file_exclude_patterns"),
                            &patterns("folder_exclude_patterns"),
                            &patterns("binary_file_patterns"),
                        ) {
                            Ok(rules) => folder.rules = rules,
                            Err(error) => self.messages.push(error),
                        }
                        self.folders.push(folder);
                    }
                    self.keymap = config.keymap;
                    self.menus = config.menus;
                    self.mousemap = config.mousemap;
                    self.palette = config.palette;
                    self.config_stamp = config.config_stamp;
                    for tab in &mut self.tabs {
                        for layer in [Layer::Default, Layer::Platform, Layer::User, Layer::Project]
                        {
                            let values = self.settings.layer(layer).clone();
                            let _ = tab.editor.settings.replace(layer, Value::Object(values));
                        }
                        tab.editor.sync();
                        tab.language_settings_revision = u64::MAX;
                        tab.needs_detection = true;
                        tab.editor.keymap = self.keymap.clone();
                    }
                    changed = true;
                }
                Err(e) => {
                    self.messages.push(e);
                    for (p, s) in &mut self.config_stamp {
                        *s = stamp(p);
                    }
                }
            }
        }
        for index in 0..self.tabs.len() {
            if self.tabs[index].lazy.is_some() {
                continue;
            }
            if self.tabs[index].needs_detection
                || self.tabs[index].language_settings_revision
                    != self.tabs[index].editor.settings.revision()
            {
                if let Err(e) =
                    self.configure_language(index, None, self.tabs[index].needs_detection)
                {
                    self.messages.push(e);
                    self.tabs[index].language_settings_revision =
                        self.tabs[index].editor.settings.revision();
                }
                changed = true;
            }
            if self.tabs[index].loading.is_none()
                && self.tabs[index]
                    .editor
                    .language
                    .borrow()
                    .as_ref()
                    .is_some_and(|l| l.name == "Plain Text")
                && self.tabs[index].path.as_ref().is_some_and(|p| {
                    vel_highlight::detect_syntax(
                        &self.assets.syntaxes,
                        &p.to_string_lossy(),
                        &crate::large::first_line(&self.tabs[index].editor.text),
                        None,
                    )
                    .is_some()
                })
                && self.tabs[index].editor.settings.get("syntax").is_none()
            {
                if let Err(e) = self.configure_language(index, None, true) {
                    self.messages.push(e);
                }
                changed = true;
            }
        }
        changed
    }
    pub fn set_file_type(&mut self, syntax: &str) -> Result<(), String> {
        self.configure_language(self.active, Some(syntax), false)
    }
    pub fn set_appearance(&mut self, dark: bool) -> Result<(), String> {
        self.dark = dark;
        if let Err(e) = self.themes.select(&self.settings, dark) {
            self.messages.push(e);
        }
        for index in 0..self.tabs.len() {
            self.configure_language(index, None, false)?;
        }
        Ok(())
    }
    pub(crate) fn configure_language(
        &mut self,
        index: usize,
        explicit: Option<&str>,
        detect: bool,
    ) -> Result<(), String> {
        if self.tabs[index].lazy.is_none() {
            let tab = &self.tabs[index];
            tab.editor.annotations.borrow_mut().set_location(
                tab.path.clone(),
                self.data_dir.as_ref().map(|d| d.join("Packages")),
            );
        }
        let tab = &mut self.tabs[index];
        tab.editor.theme = self.themes.ui.clone();
        if tab.loading.is_some() || tab.binary_loading.is_some() || tab.editor.binary.is_some() {
            *tab.editor.language.borrow_mut() = None;
            tab.needs_detection = false;
            tab.language_settings_revision = tab.editor.settings.revision();
            return Ok(());
        }
        tab.editor.snippets = Arc::new(self.catalog.snippets.clone());
        tab.editor.snippet_path = tab.path.clone();
        let previous = tab
            .editor
            .language
            .borrow()
            .as_ref()
            .map(|l| l.syntax.clone());
        let setting_override = tab.editor.settings.get("syntax").and_then(Value::as_str);
        let chosen =
            explicit
                .or(setting_override)
                .or(if detect { None } else { previous.as_deref() });
        let reference = vel_highlight::detect_syntax(
            &self.assets.syntaxes,
            &crate::language::syntax_path(tab.path.as_deref()),
            &crate::large::first_line(&tab.editor.text),
            chosen,
        )
        .or_else(|| {
            explicit
                .is_none()
                .then(|| self.assets.syntaxes.find_syntax_plain_text())
        })
        .ok_or("Unknown syntax")?;
        let syntax = reference.path.clone();
        let name = reference.name.clone();
        if explicit.is_some() {
            tab.editor
                .settings
                .set(Layer::View, "syntax", serde_json::json!(syntax));
        }
        let values = self.syntax_settings.get(&name).cloned().unwrap_or_default();
        if tab.editor.settings.layer(Layer::Syntax) != &values {
            tab.editor
                .settings
                .replace(Layer::Syntax, Value::Object(values))?;
        }
        if detect
            && tab.needs_detection
            && tab.editor.settings.bool("detect_indentation", true)
            && let Some((size, spaces)) = crate::language::detect_indentation(&tab.editor.text)
        {
            tab.editor
                .settings
                .set(Layer::View, "tab_size", serde_json::json!(size));
            tab.editor.settings.set(
                Layer::View,
                "translate_tabs_to_spaces",
                serde_json::json!(spaces),
            );
        }
        tab.editor.sync();
        let settings = Value::Object(tab.editor.settings.effective());
        let selected = vel_color_scheme::selected_scheme(&settings, self.dark);
        let basename = selected.rsplit('/').next().unwrap_or(selected);
        let scheme = self
            .schemes
            .get(basename)
            .or_else(|| {
                self.schemes
                    .get(&format!("{basename}.sublime-color-scheme"))
            })
            .cloned()
            .or_else(|| {
                self.schemes
                    .get(if self.dark {
                        "Velocity Dark.sublime-color-scheme"
                    } else {
                        "Velocity Light.sublime-color-scheme"
                    })
                    .cloned()
            })
            .ok_or_else(|| format!("Unknown color scheme {selected}"))?;
        if previous.as_deref() != Some(syntax.as_str())
            || !tab
                .editor
                .language
                .borrow()
                .as_ref()
                .is_some_and(|l| Arc::ptr_eq(&l.assets, &self.assets))
        {
            tab.editor
                .attach_language(self.assets.clone(), &syntax, scheme)?;
        } else if let Some(language) = tab.editor.language.borrow_mut().as_mut() {
            language.recolor(
                scheme,
                tab.editor.text.clone(),
                tab.editor.preferences.tab_size,
            );
        }
        tab.needs_detection = false;
        tab.language_settings_revision = tab.editor.settings.revision();
        Ok(())
    }
    pub fn reload_external(&mut self, index: usize, reload: bool) {
        if let Some(loaded) = self.tabs[index].external.take()
            && reload
        {
            self.tabs[index].install(loaded);
            self.reconcile_installs();
            let _ = self.prepare_sheet(self.active);
        }
    }
    pub fn default_data_dir() -> Option<PathBuf> {
        std::env::var_os("VELOCITY_DATA_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join("Library/Application Support/Velocity"))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vel_config::{
        json,
        keymap::{Key, Resolution},
    };
    #[test]
    fn tabs_cli_positions_edit_save_and_format_dirty_state() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "a\r\n你好\r\n").unwrap();
        std::fs::write(&b, b"caf\xe9").unwrap();
        let mut workspace = Workspace::new(Text::new(), None, None, false).unwrap();
        workspace
            .open(FileLocation {
                path: a.clone(),
                line: 2,
                column: 2,
            })
            .unwrap();
        assert_eq!(workspace.tabs.len(), 1);
        assert_eq!(workspace.editor().caret, 5);
        assert_eq!(
            workspace.tabs[0].format.line_ending,
            fileio::LineEnding::CrLf
        );
        workspace
            .editor_mut()
            .run("insert", &json!({"characters":"!"}))
            .unwrap();
        assert!(workspace.tabs[0].dirty());
        workspace.save(0, None).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), "a\r\n你!好\r\n".as_bytes());
        assert!(!workspace.tabs[0].dirty());
        workspace
            .open(FileLocation {
                path: b,
                line: 1,
                column: 1,
            })
            .unwrap();
        assert_eq!(workspace.tabs.len(), 2);
        assert_eq!(workspace.editor().text.to_string(), "café");
        workspace.select(0);
        workspace.editor_mut().run("undo", &json!({})).unwrap();
        assert!(workspace.tabs[0].dirty());
        assert_eq!(workspace.editor().text.to_string(), "a\n你好\n");
        workspace.tabs[0].format.line_ending = fileio::LineEnding::Lf;
        workspace.close(1).unwrap();
        assert_eq!(workspace.active, 0);
        workspace.new_file().unwrap();
        assert!(workspace.tabs[1].path.is_none());
        workspace.close(1).unwrap();
        workspace.close(0).unwrap();
        assert_eq!(workspace.tabs.len(), 1);
    }
    #[test]
    fn settings_project_view_and_user_keymap_precedence_reload() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("Packages/User");
        std::fs::create_dir_all(&user).unwrap();
        let prefs = user.join("Preferences.sublime-settings");
        std::fs::write(&prefs, "{ // user\n\"tab_size\": 2, \"font_size\": 16,}").unwrap();
        std::fs::write(user.join("Plain Text.sublime-settings"), "{\"tab_size\":3}").unwrap();
        std::fs::write(
            user.join("Custom.sublime-keymap"),
            r#"[{"keys":["super+d"],"command":"insert","args":{"characters":"override"}}]"#,
        )
        .unwrap();
        let project = dir.path().join("test.sublime-project");
        std::fs::write(&project, "{\"settings\":{\"tab_size\":6,\"rulers\":[72]}}").unwrap();
        let mut workspace =
            Workspace::new(Text::new(), Some(dir.path().into()), Some(project), false).unwrap();
        // Project settings apply, but syntax-specific settings override them, like Sublime.
        assert_eq!(workspace.editor().preferences.rulers, vec![72]);
        assert_eq!(workspace.editor().preferences.tab_size, 3);
        assert_eq!(workspace.editor().preferences.font_size, 16.);
        assert!(
            matches!(workspace.editor_mut().resolve_key(&Key::parse("super+d").unwrap()),Resolution::Command(i) if i.command=="insert")
        );
        workspace
            .editor_mut()
            .settings
            .set(Layer::View, "tab_size", json!(8));
        workspace.editor_mut().sync();
        std::fs::write(&prefs, "{\"font_size\":20,\"tab_size\":4}").unwrap();
        workspace.wait_resources();
        assert_eq!(workspace.editor().preferences.font_size, 20.);
        assert_eq!(workspace.editor().preferences.tab_size, 8);
    }
    #[test]
    fn reload_decisions_and_failed_load_do_not_overwrite_edits() {
        let mut workspace = Workspace::new(Text::from("saved"), None, None, false).unwrap();
        workspace
            .editor_mut()
            .run("insert", &json!({"characters":"local "}))
            .unwrap();
        let loaded = || Loaded {
            text: Text::from("external"),
            format: Format::default(),
            source_bytes: 8,
        };
        workspace.tabs[0].external = Some(loaded());
        workspace.reload_external(0, false);
        assert_eq!(workspace.editor().text.to_string(), "local saved");
        workspace.tabs[0].external = Some(loaded());
        workspace.reload_external(0, true);
        assert_eq!(workspace.editor().text.to_string(), "external");
        assert!(!workspace.tabs[0].dirty());
        workspace.editor_mut().read_only = true;
        workspace
            .editor_mut()
            .run("insert", &json!({"characters":"bad"}))
            .unwrap();
        assert_eq!(workspace.editor().text.to_string(), "external");
        assert!(
            workspace
                .save(0, Some(PathBuf::from("/tmp/velocity-should-not-write")))
                .is_err()
        );
    }
}

#[cfg(test)]
#[path = "package_tests.rs"]
mod package_tests;
