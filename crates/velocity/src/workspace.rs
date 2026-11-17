//! Headless tab/document lifecycle, settings resources and asynchronous file tasks.
use crate::{cli::FileLocation, editor::Editor};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    sync::atomic::Ordering,
    time::{Duration, Instant, SystemTime},
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
    /// The file was removed while its in-memory document remains open.
    pub deleted: bool,
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
    pub(crate) needs_detection: bool,
    pub(crate) indentation_initialized: bool,
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
    /// Center this location once the active view has its final viewport geometry.
    pub pending_center: bool,
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
        for command in [
            "reopen_last_file",
            "reopen_encoding",
            "save_encoding",
            "open_recent_file",
            "open_recent_folder",
            "clear_recent_items",
        ] {
            editor
                .registry
                .register(command, vel_commands::Kind::WindowCommand, None);
        }
        let format = fileio::default_format(&settings.editor()).unwrap_or_default();
        Ok(Self {
            shared_dirty: false,
            deleted: false,
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
            indentation_initialized: false,
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
            pending_center: false,
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
    /// Includes removed files whose retained contents have no saved disk copy.
    pub fn dirty(&self) -> bool {
        self.deleted
            || self.shared_dirty
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
        let name = self.name();
        format!(
            "{name}{}{}{}{}",
            if self.editor.binary.is_some() {
                " [hex]"
            } else {
                ""
            },
            if self.path.is_some()
                && self.disk_saved.is_none()
                && self.lazy.is_none()
                && !self.deleted
                && !self.editor.read_only
            {
                " [new]"
            } else {
                ""
            },
            if self.deleted { " [deleted]" } else { "" },
            if self.dirty() { " •" } else { "" }
        )
    }
    pub fn name(&self) -> String {
        if let Some(title) = self
            .editor
            .settings
            .get("velocity_html_sheet_title")
            .and_then(Value::as_str)
        {
            return title.into();
        }
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                if !self.editor.settings.bool("set_unsaved_view_name", true) {
                    return "Untitled".into();
                }
                let name: String = self
                    .editor
                    .text
                    .line(0)
                    .chars()
                    .skip_while(|c| c.is_whitespace())
                    .take(50)
                    .collect();
                let name = name.trim_end().to_owned();
                if name.is_empty() {
                    "Untitled".into()
                } else {
                    name
                }
            })
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
        self.needs_detection = !self.indentation_initialized;
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
        self.pending_center = true;
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
    recent_menu_state: Option<([Vec<PathBuf>; 3], u64)>,
    pub(crate) distraction_settings: serde_json::Map<String, Value>,
    pub folders: Vec<vel_search::project::Folder>,
    pub tabs: Vec<Tab>,
    pub active: usize,
    /// An empty window retains an input adapter but has no workbench sheet.
    pub(crate) empty: bool,
    pub(crate) closed_files: Vec<(PathBuf, vel_session::ViewState)>,
    /// Final stdin text is retained until its CLI waiter consumes it.
    pub(crate) closed_text: BTreeMap<u64, Text>,
    pub(crate) stdout_sheets: std::collections::BTreeSet<u64>,
    pub menus: Vec<MenuItem>,
    /// Resource-layered native context menus, keyed by their complete resource filename.
    pub context_menus: BTreeMap<String, Vec<MenuItem>>,
    pub mousemap: Vec<MouseBinding>,
    pub palette: Vec<PaletteItem>,
    /// Macro filenames from the background resource overlay; contents are loaded on demand.
    pub(crate) macro_paths: std::collections::BTreeSet<String>,
    pub messages: Vec<String>,
    /// Consolidated authoring diagnostics; consumed by the status bar, never alerts.
    pub(crate) resource_reports: Vec<String>,
    pub telemetry: Vec<Value>,
    pub(crate) settings: Settings,
    pub(crate) keymap: Keymap,
    pub(crate) watcher: Option<Watcher>,
    pub(crate) data_dir: Option<PathBuf>,
    pub(crate) project: Option<PathBuf>,
    config_stamp: Vec<(PathBuf, Option<(u64, SystemTime)>)>,
    config_rx: Option<std::sync::mpsc::Receiver<Result<Configuration, String>>>,
    last_config_check: Option<Instant>,
    reload_requested: bool,
    settings_writes: Vec<std::sync::mpsc::Receiver<Result<(), String>>>,
    pub resource_revision: u64,
}
pub(crate) fn stamp(path: &Path) -> Option<(u64, SystemTime)> {
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
    context_menus: BTreeMap<String, Vec<MenuItem>>,
    mousemap: Vec<MouseBinding>,
    palette: Vec<PaletteItem>,
    macro_paths: std::collections::BTreeSet<String>,
    folders: Vec<vel_search::project::Folder>,
    distraction_settings: serde_json::Map<String, Value>,
    build_systems: Vec<vel_build::BuildSystem>,
    project_model: vel_session::project::Project,
    config_stamp: Vec<(PathBuf, Option<(u64, SystemTime)>)>,
    messages: Vec<String>,
    resource_reports: Vec<String>,
}
// Diagnostics retain their resource paths; display names never establish provenance.
fn user_resource(resource: &vel_config::resources::Resource, data: Option<&Path>) -> bool {
    data.is_some_and(|data| {
        resource.origin.starts_with(data.join("Packages"))
            || resource.origin.starts_with(data.join("Installed Packages"))
    })
}
fn warning_from_path(warning: &str, path: &str) -> bool {
    warning
        .strip_prefix(path)
        .is_some_and(|rest| rest.starts_with(':'))
        || warning.starts_with(&format!("skipping {path:?}:"))
}
fn collect_resource_warnings<'a, S: AsRef<str>>(
    kind: &str,
    warnings: impl IntoIterator<Item = (Option<&'a str>, S)>,
    reports: &mut Vec<String>,
    log: &mut Vec<String>,
) {
    let mut count = 0;
    let mut first = None;
    for (path, warning) in warnings {
        eprintln!("{}", warning.as_ref());
        log.push(warning.as_ref().to_owned());
        if let Some(path) = path {
            first.get_or_insert(path);
            count += 1;
        }
    }
    if let Some(first) = first {
        reports.push(format!(
            "{count} {kind} warnings ({first}), see Velocity.log"
        ));
    }
}
/// Size after which `Velocity.log` is rotated to `Velocity.log.1`.
const RESOURCE_LOG_LIMIT: u64 = 1 << 20;
fn write_resource_log(data: Option<&Path>, warnings: &[String], reports: &mut Vec<String>) {
    use std::io::Write;
    if warnings.is_empty() {
        return;
    }
    if let Some(data) = data {
        let path = data.join("Velocity.log");
        let result = (|| {
            std::fs::create_dir_all(data)?;
            // The same warnings are logged on every configuration load, so keep the file bounded:
            // past the limit the current log becomes `Velocity.log.1` (replacing an older one).
            if std::fs::metadata(&path).is_ok_and(|m| m.len() > RESOURCE_LOG_LIMIT) {
                std::fs::rename(&path, data.join("Velocity.log.1"))?;
            }
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)?;
            let seconds = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            file.write_all(
                format!(
                    "[{seconds}] {} resource warning(s)\n{}\n",
                    warnings.len(),
                    warnings.join("\n")
                )
                .as_bytes(),
            )
        })();
        if let Err(error) = result {
            eprintln!("{}: {error}", path.display());
            if !reports.is_empty() {
                // Even an unwritable profile must not send these through the alert queue.
                reports.push(format!(
                    "Could not write {}: {error}; {}",
                    path.display(),
                    warnings.join("; ")
                ));
            }
        }
    }
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
                "Context.sublime-menu",
                include_str!("../resources/Packages/Default/Context.sublime-menu"),
            ),
            (
                "Tab Context.sublime-menu",
                include_str!("../resources/Packages/Default/Tab Context.sublime-menu"),
            ),
            (
                "Side Bar.sublime-menu",
                include_str!("../resources/Packages/Default/Side Bar.sublime-menu"),
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
        let mut resource_reports = Vec::new();
        let mut mouse_warnings = Vec::new();
        let mut scheme_warnings = Vec::new();
        let mut resource_log = Vec::new();
        resources.load_syntaxes(&crate::language::packages_root())?;
        if let Some(bundle) = crate::release::bundle_resources() {
            collect_resource_warnings(
                "package",
                resources
                    .load_with_warnings(&bundle)
                    .iter()
                    .map(|warning| (None, warning)),
                &mut resource_reports,
                &mut resource_log,
            );
        }
        if let Some(data) = &data_dir {
            let warnings = resources.load_with_warnings(data);
            let paths: Vec<_> = warnings
                .iter()
                .map(|w| w.split_once(": ").map_or(w.as_str(), |(p, _)| p))
                .collect();
            collect_resource_warnings(
                "package",
                warnings.iter().zip(paths).map(|(w, p)| (Some(p), w)),
                &mut resource_reports,
                &mut resource_log,
            );
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
        if let (_, Some(warning)) = fileio::fallback_encoding(&settings.editor().fallback_encoding)
        {
            resource_messages.push(warning);
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
                rules: vel_search::project::Rules::new(&excludes[0], &excludes[1], &excludes[2])?
                    .with_includes(
                    &patterns(folder.get("file_include_patterns")),
                    &patterns(folder.get("folder_include_patterns")),
                )?,
                follow_links: folder["follow_symlinks"].as_bool().unwrap_or(true),
            });
        }
        let mut keys = Vec::new();
        let mut menus = Vec::new();
        let mut context_menus: BTreeMap<String, Vec<MenuItem>> = BTreeMap::new();
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
            let name = resource.path.rsplit('/').next().unwrap_or_default();
            let target = match name {
                "Main.sublime-menu" => &mut menus,
                "Context.sublime-menu" | "Tab Context.sublime-menu" | "Side Bar.sublime-menu" => {
                    context_menus.entry(name.to_owned()).or_default()
                }
                _ => continue,
            };
            match resource.text().and_then(ui::parse) {
                Ok(items) => ui::merge_menu(target, items),
                Err(e) => resource_messages.push(format!("{}: {e}", resource.path)),
            }
        }
        for resource in resources.matching(".sublime-mousemap") {
            match resource.text().and_then(ui::parse_mousemap_with_warnings) {
                Ok((items, warnings)) => {
                    mousemap.extend(items);
                    mouse_warnings.extend(warnings.into_iter().map(|w| (resource.path.clone(), w)));
                }
                Err(e) => mouse_warnings.push((resource.path.clone(), e)),
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
        let user_syntaxes: Vec<_> = resources
            .matching("")
            .into_iter()
            .filter(|r| {
                user_resource(r, data_dir.as_deref())
                    && [".sublime-syntax", ".tmLanguage", ".tmLanguage.json"]
                        .iter()
                        .any(|suffix| r.path.ends_with(suffix))
            })
            .collect();
        let syntax_warnings = assets.syntaxes.warnings().iter().map(|warning| {
            let path = user_syntaxes
                .iter()
                .find(|r| warning_from_path(warning, &r.path))
                .map(|r| r.path.as_str());
            (path, warning.as_str())
        });
        collect_resource_warnings(
            "syntax",
            syntax_warnings,
            &mut resource_reports,
            &mut resource_log,
        );
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
        for suffix in [
            ".sublime-color-scheme",
            ".hidden-color-scheme",
            ".tmTheme",
            ".hidden-tmTheme",
        ] {
            for resource in resources.matching(suffix) {
                if resource.origin.exists() {
                    config_stamp.push((resource.origin.clone(), stamp(&resource.origin)));
                }
                match resource.text() {
                    Ok(source) => scheme_resources.push((resource.path.clone(), source.to_owned())),
                    Err(e) => scheme_warnings.push((resource.path.clone(), e)),
                }
            }
        }
        let mut schemes = BTreeMap::new();
        for (path, _) in &scheme_resources {
            let name = path.rsplit('/').next().expect("resource filename");
            if !schemes.contains_key(name) {
                // A User override can contribute warnings to a bundled scheme filename.
                let warning_path = scheme_resources
                    .iter()
                    .rev()
                    .find(|(p, _)| {
                        p.rsplit('/').next() == Some(name)
                            && resources
                                .get(p)
                                .is_some_and(|r| user_resource(r, data_dir.as_deref()))
                    })
                    .map_or(path, |(p, _)| p);
                let scheme = vel_color_scheme::load_resources(
                    scheme_resources
                        .iter()
                        .map(|(p, s)| (p.as_str(), s.as_str())),
                    name,
                )
                .map_err(|e| e.to_string());
                match scheme {
                    Ok(scheme) => {
                        scheme_warnings.extend(
                            scheme
                                .warnings()
                                .iter()
                                .map(|warning| (warning_path.clone(), warning.clone())),
                        );
                        schemes.insert(name.to_owned(), Arc::new(scheme));
                    }
                    Err(e) => scheme_warnings.push((warning_path.clone(), e)),
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
        let macro_paths = resources
            .matching(".sublime-macro")
            .into_iter()
            .map(|r| r.path.clone())
            .collect();
        for (kind, warnings) in [
            ("mousemap", &mouse_warnings),
            ("color-scheme", &scheme_warnings),
        ] {
            collect_resource_warnings(
                kind,
                warnings.iter().map(|(path, warning)| {
                    let user = resources
                        .get(path)
                        .is_some_and(|r| user_resource(r, data_dir.as_deref()));
                    (user.then_some(path.as_str()), format!("{path}: {warning}"))
                }),
                &mut resource_reports,
                &mut resource_log,
            );
        }
        write_resource_log(data_dir.as_deref(), &resource_log, &mut resource_reports);
        Ok(Self {
            resource_reports,
            themes,
            macro_paths,
            #[cfg(test)]
            resources,
            settings,
            keymap,
            syntax_settings,
            schemes,
            assets,
            catalog,
            menus,
            context_menus,
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
            context_menus,
            mousemap,
            palette,
            macro_paths,
            folders,
            distraction_settings,
            build_systems,
            project_model,
            config_stamp,
            messages: resource_messages,
            resource_reports,
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
            recent_menu_state: None,
            folders,
            tabs: vec![tab],
            active: 0,
            empty: false,
            closed_files: Vec::new(),
            closed_text: BTreeMap::new(),
            stdout_sheets: Default::default(),
            menus,
            context_menus,
            mousemap,
            palette,
            macro_paths,
            messages,
            resource_reports,
            telemetry: Vec::new(),
            settings,
            keymap,
            watcher,
            data_dir,
            project,
            config_stamp,
            config_rx: None,
            last_config_check: None,
            reload_requested: false,
            settings_writes: Vec::new(),
            resource_revision: 0,
        };
        workspace.workbench.groups[0].add(workspace.tabs[0].sheet, false);
        workspace.configure_language(0, None, true)?;
        Ok(workspace)
    }
    /// Refreshes the resource-defined recent submenu only when its source changes.
    pub fn refresh_file_menus(&mut self) -> bool {
        let recent = [
            &self.recent_files,
            &self.recent_folders,
            &self.recent_projects,
        ];
        if self
            .recent_menu_state
            .as_ref()
            .is_some_and(|(old, revision)| {
                *revision == self.resource_revision && old.iter().zip(recent).all(|(a, b)| a == b)
            })
        {
            return false;
        }
        let mut children = Vec::new();
        for (paths, command, key) in [
            (&self.recent_files, "open_recent_file", "file"),
            (&self.recent_folders, "open_recent_folder", "dir"),
            (&self.recent_projects, "open_project", "file"),
        ] {
            for path in paths.iter().take(20) {
                children.push(serde_json::json!({"caption":path.display().to_string(), "command":command, "args":{key:path}}));
            }
        }
        children.push(serde_json::json!({"caption":"Clear Items", "command":"clear_recent_items"}));
        if let Some(menu) = self
            .menus
            .iter_mut()
            .find(|menu| menu.id == "file")
            .and_then(|menu| {
                menu.children
                    .iter_mut()
                    .find(|item| item.id == "open_recent")
            })
        {
            menu.children =
                serde_json::from_value(serde_json::json!(children)).expect("authored recent menu");
        }
        self.recent_menu_state = Some((
            [
                self.recent_files.clone(),
                self.recent_folders.clone(),
                self.recent_projects.clone(),
            ],
            self.resource_revision,
        ));
        true
    }
    pub fn track_settings_write(&mut self, rx: std::sync::mpsc::Receiver<Result<(), String>>) {
        self.settings_writes.push(rx);
    }
    #[cfg(test)]
    pub fn wait_resources(&mut self) {
        // Tests explicitly await a disk observation as well as any asynchronous worker.
        // Force that observation without changing the production polling interval.
        self.last_config_check = None;
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
    /// Active document title, including its dirty marker and optional project name.
    pub fn window_title(&self) -> String {
        let tab = &self.tabs[self.active];
        let name = if self.is_empty() {
            "Velocity".into()
        } else {
            tab.name()
        };
        let title = format!(
            "{name}{}{}",
            if tab.deleted { " [deleted]" } else { "" },
            if tab.dirty() { " •" } else { "" }
        );
        match self.project.as_deref().and_then(Path::file_stem) {
            Some(project) => format!("{title} — {}", project.to_string_lossy()),
            None => title,
        }
    }
    /// Consume only the active tab's explicit location-centering request.
    pub fn take_pending_center(&mut self) -> bool {
        std::mem::take(&mut self.tabs[self.active].pending_center)
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
    pub fn is_empty(&self) -> bool {
        self.empty
    }
    /// Attach the empty window's input adapter when its first edit arrives.
    pub fn materialize_input(&mut self) {
        if self.empty && self.tabs[0].dirty() {
            self.empty = false;
            self.workbench.groups[self.workbench.active].add(self.tabs[0].sheet, false);
        }
    }
    pub fn new_file(&mut self) -> Result<(), String> {
        if self.empty {
            self.tabs.clear();
            self.empty = false;
        }
        self.tabs
            .push(Tab::new(Text::new(), &self.settings, &self.keymap)?);
        self.workbench.groups[self.workbench.active]
            .add(self.tabs.last().expect("tab").sheet, false);
        self.active = self.tabs.len() - 1;
        // An untitled buffer is already loaded; it has no disk indentation to detect.
        self.tabs[self.active].needs_detection = false;
        self.tabs[self.active].indentation_initialized = true;
        self.configure_language(self.active, None, false)?;
        Ok(())
    }
    pub fn select(&mut self, index: usize) {
        if !self.empty
            && index < self.tabs.len()
            && self.workbench.group_of(self.tabs[index].sheet).is_some()
        {
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
        self.materialize_input();
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
            // A duplicate open during background loading must retain the latest location
            // until the final text is installed, rather than clamp it to the empty prefix.
            if self.tabs[index].loading.is_some() || self.tabs[index].binary_loading.is_some() {
                self.tabs[index].location = Some(location.clone());
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
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => Some(metadata),
            Ok(_) => {
                return Err(format!(
                    "{}: only regular files can be opened",
                    path.display()
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let size = metadata.as_ref().map_or(0, |m| m.len());
        if metadata.is_none() {
            let location = tab.location.take().expect("open location");
            tab.goto(&location);
        } else if crate::binary::is_binary(&path)? {
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
        if let Some(w) = self.watcher.as_mut()
            && path.parent().is_some_and(Path::is_dir)
        {
            w.watch(&path)?;
        }
        // The empty window's input adapter is never a sheet, including during preview.
        if self.empty
            || (self.tabs.len() == 1
                && !preview
                && self.tabs[0].path.is_none()
                && !self.tabs[0].editor.buffer.is_dirty()
                && self.tabs[0].editor.text.len_bytes() == 0)
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
        self.empty = false;
        self.workbench.groups[self.workbench.active].add(self.tabs[self.active].sheet, false);
        crate::workbench::remember(&mut self.recent_files, path);
        self.configure_language(self.active, None, true)?;
        Ok(())
    }
    pub fn save(&mut self, index: usize, path: Option<PathBuf>) -> Result<(), String> {
        self.materialize_input();
        if self.empty {
            return Err("no file to save".into());
        }
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
        tab.deleted = false;
        let document = tab.session_document;
        tab.disk_stamp = stamp(&target);
        tab.disk_saved = vel_session::DiskStamp::read(&target);
        tab.editor.annotations.borrow_mut().reference(
            tab.editor.buffer.text().clone(),
            tab.path.clone(),
            self.data_dir.as_ref().map(|d| d.join("Packages")),
        );
        let saved_format = tab.saved_format.clone();
        let disk_saved = tab.disk_saved.clone();
        self.sync_shared();
        for sibling in &mut self.tabs {
            if sibling.session_document == document {
                sibling.deleted = false;
                // Hidden restored clones have not attached to the live buffer yet.
                // Their lazy metadata must reflect this save too, including Save As.
                if let Some((saved, _)) = &mut sibling.lazy {
                    sibling.path = Some(target.clone());
                    sibling.format = saved_format.clone();
                    sibling.saved_format = saved_format.clone();
                    sibling.disk_saved = disk_saved.clone();
                    sibling.restored_format_dirty = false;
                    saved.path = Some(target.clone());
                    saved.disk = disk_saved.clone();
                    saved.encoding = saved_format.encoding.clone();
                    saved.bom = saved_format.bom;
                    saved.line_ending = saved_format.line_ending.name().into();
                    saved.dirty = false;
                }
            } else if sibling.path.as_ref() == Some(&target) {
                sibling.deleted = false;
            }
        }
        if let Some(w) = self.watcher.as_mut() {
            w.watch(&target)?;
        }
        if let Err(e) = self.configure_language(index, None, true) {
            self.messages.push(e);
        }
        crate::workbench::remember(&mut self.recent_files, target);
        Ok(())
    }
    /// Saves dirty documents independently. Cancellation skips that document; errors
    /// include its path/name and do not prevent later documents from being saved.
    pub fn save_all(&mut self, mut choose: impl FnMut(&str) -> Option<PathBuf>) -> Vec<String> {
        let mut errors = Vec::new();
        for index in 0..self.tabs.len() {
            if !self.tabs[index].dirty() {
                continue;
            }
            let path = if self.tabs[index].path.is_none() {
                let Some(path) = choose(&self.tabs[index].name()) else {
                    continue;
                };
                Some(path)
            } else {
                None
            };
            if let Err(error) = self.save(index, path) {
                errors.push(format!("{}: {error}", self.tabs[index].name()));
            }
        }
        errors
    }
    pub fn close(&mut self, index: usize) -> Result<(), String> {
        if self.empty {
            return Ok(());
        }
        self.prepare_close_sheet(index)?;
        let tab = &self.tabs[index];
        let sheet = tab.sheet;
        let active = self.tabs[self.active].sheet;
        let group = self
            .workbench
            .group_of(sheet)
            .unwrap_or(self.workbench.active);
        let order = &self.workbench.groups[group].sheets;
        let at = order.iter().position(|id| *id == sheet).unwrap_or(0);
        let neighbor = order
            .get(at + 1)
            .or_else(|| at.checked_sub(1).and_then(|i| order.get(i)))
            .copied();
        if (!tab.transient || tab.dirty())
            && let Some(path) = &tab.path
        {
            self.closed_files.push((
                path.clone(),
                vel_session::ViewState::capture(&tab.editor.buffer, tab.editor.id, tab.scroll),
            ));
            if self.closed_files.len() > 100 {
                self.closed_files.remove(0);
            }
        }
        if self.stdout_sheets.contains(&sheet) {
            self.closed_text.insert(sheet, tab.editor.text.clone());
        }
        for group in &mut self.workbench.groups {
            group.remove(sheet);
        }
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.tabs
                .push(Tab::new(Text::new(), &self.settings, &self.keymap)?);
            self.active = 0;
            self.empty = true;
            self.tabs[0].needs_detection = false;
            self.tabs[0].indentation_initialized = true;
            self.configure_language(0, None, false)?;
        } else {
            let target = if sheet == active {
                neighbor
            } else {
                Some(active)
            }
            .and_then(|id| self.sheet_index(id))
            .or_else(|| {
                self.workbench
                    .groups
                    .iter()
                    .filter_map(|g| g.focused)
                    .find_map(|id| self.sheet_index(id))
            })
            .unwrap_or(0);
            self.active = target;
            self.select(target);
        }
        Ok(())
    }
    /// Reopens the most recently closed path with its selection, folds and viewport.
    pub fn reopen_last_file(&mut self) -> Result<(), String> {
        while let Some((path, state)) = self.closed_files.pop() {
            // A deleted file is skipped, rather than reopened as a new named buffer.
            if matches!(path.try_exists(), Ok(false)) {
                continue;
            }
            self.open(FileLocation {
                path,
                line: 1,
                column: 1,
            })?;
            let tab = &mut self.tabs[self.active];
            tab.pending_center = false;
            tab.scroll = state.scroll;
            if tab.loading.is_some() || tab.binary_loading.is_some() {
                tab.pending_restore = Some(state);
            } else {
                state.apply(&mut tab.editor.buffer, tab.editor.id);
                tab.editor.sync();
            }
            break;
        }
        Ok(())
    }
    /// Reopens a saved file with an explicit codec. Failed decoding leaves edits intact.
    pub fn reopen_encoding(&mut self, name: &str) -> Result<(), String> {
        let index = self.active;
        let tab = &self.tabs[index];
        let path = tab.path.as_deref().ok_or("Reopen requires a saved file")?;
        let loaded = fileio::load_with_encoding(path, name)?;
        self.tabs[index].install(loaded);
        self.tabs[index].deleted = false;
        self.tabs[index].restored_format_dirty = false;
        self.reconcile_installs();
        Ok(())
    }
    /// Automatic resource stamps are sampled at most every 500 ms. Explicit reloads,
    /// including completed package/settings writes, bypass the interval on the next poll.
    fn configuration_changed(&mut self, now: Instant) -> bool {
        if !self.reload_requested
            && self.last_config_check.is_some_and(|last| {
                now.saturating_duration_since(last) < Duration::from_millis(500)
            })
        {
            return false;
        }
        self.last_config_check = Some(now);
        self.reload_requested
            || self
                .config_stamp
                .iter()
                .any(|(path, saved)| stamp(path) != *saved)
    }
    /// Polls jobs/notifications and reports whether scene content changed.
    pub fn poll(&mut self) -> bool {
        self.materialize_input();
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
        let mut close_deleted = Vec::new();
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
                            if matches!(path.try_exists(), Ok(false)) {
                                if self.settings.bool("close_deleted_files", false) && !tab.dirty()
                                {
                                    close_deleted.push(tab.sheet);
                                }
                                tab.deleted = true;
                                tab.transient = false;
                                tab.external = None;
                                changed = true;
                                continue;
                            }
                            if path.is_file() && tab.deleted {
                                // Deletion is a close/save marker, not an edit to the buffer.
                                tab.deleted = false;
                                changed = true;
                            }
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
                                        tab.disk_saved = vel_session::DiskStamp::read(&path);
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
        for sheet in close_deleted {
            if let Some(index) = self.sheet_index(sheet)
                && let Err(error) = self.close(index)
            {
                self.messages.push(error);
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
        if self.config_rx.is_none() && self.configuration_changed(Instant::now()) {
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
                    self.resource_reports.append(&mut config.resource_reports);
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
                    self.context_menus = config.context_menus;
                    self.mousemap = config.mousemap;
                    self.palette = config.palette;
                    self.macro_paths = config.macro_paths;
                    self.config_stamp = config.config_stamp;
                    for tab in &mut self.tabs {
                        for layer in [Layer::Default, Layer::Platform, Layer::User, Layer::Project]
                        {
                            let values = self.settings.layer(layer).clone();
                            let _ = tab.editor.settings.replace(layer, Value::Object(values));
                        }
                        tab.editor.sync();
                        tab.language_settings_revision = u64::MAX;
                        // Resource reloads preserve the buffer's detected/manual indentation.
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
        if detect && tab.lazy.is_none() && tab.needs_detection {
            if tab.editor.settings.bool("detect_indentation", true)
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
            // A load with detection disabled also consumes the one load-time attempt.
            tab.needs_detection = false;
            tab.indentation_initialized = true;
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
    fn closing_an_earlier_tab_keeps_the_active_tab() {
        let mut workspace = Workspace::new(Text::from("a"), None, None, false).unwrap();
        for text in ["b", "c", "d"] {
            workspace.new_file().unwrap();
            workspace
                .editor_mut()
                .run("insert", &json!({"characters": text}))
                .unwrap();
        }
        workspace.select(2);
        assert_eq!(workspace.editor().text.to_string(), "c");
        workspace.close(0).unwrap();
        assert_eq!(workspace.active, 1);
        assert_eq!(workspace.editor().text.to_string(), "c");
        // Closing a later tab leaves the active one alone too.
        workspace.close(2).unwrap();
        assert_eq!(workspace.editor().text.to_string(), "c");
    }
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
    fn titles_follow_active_document_dirty_project_and_deleted_state() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("文書.rs");
        std::fs::write(&file, "saved").unwrap();
        let project = dir.path().join("Example.sublime-project");
        std::fs::write(&project, "{}").unwrap();
        let mut workspace = Workspace::new(Text::new(), None, None, false).unwrap();
        assert_eq!(workspace.window_title(), "Untitled");
        workspace
            .open(FileLocation {
                path: file,
                line: 1,
                column: 1,
            })
            .unwrap();
        assert_eq!(workspace.window_title(), "文書.rs");
        workspace
            .editor_mut()
            .run("insert", &json!({"characters":"changed "}))
            .unwrap();
        assert_eq!(workspace.window_title(), "文書.rs •");
        workspace.project = Some(project);
        assert_eq!(workspace.window_title(), "文書.rs • — Example");
        workspace.tabs[0].deleted = true;
        assert_eq!(workspace.tabs[0].label(), "文書.rs [deleted] •");
        assert_eq!(workspace.window_title(), "文書.rs [deleted] • — Example");
        workspace.save(0, None).unwrap();
        assert!(!workspace.tabs[0].deleted);
        assert_eq!(workspace.window_title(), "文書.rs — Example");
        workspace.new_file().unwrap();
        assert_eq!(workspace.window_title(), "Untitled — Example");
    }
    #[test]
    fn deleted_clean_files_are_dirty_and_retain_contents_across_session_restore() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let file = root.join("clean.txt");
        let contents = "clean contents\nretained after trash 你好\n";
        std::fs::write(&file, contents).unwrap();
        let mut workspace = Workspace::new(Text::new(), None, None, false).unwrap();
        workspace
            .open(FileLocation {
                path: file.clone(),
                line: 1,
                column: 1,
            })
            .unwrap();
        workspace.clone_file().unwrap();
        assert!(workspace.tabs.iter().all(|tab| !tab.dirty()));
        assert!(!workspace.editor().buffer.is_dirty());
        std::fs::remove_file(&file).unwrap();
        crate::context_menu::apply_delete(&mut workspace, std::slice::from_ref(&file));
        // Cloning after deletion must retain the same missing-file state too.
        workspace.clone_file().unwrap();
        assert!(workspace.tabs.iter().all(|tab| tab.deleted && tab.dirty()));
        assert!(!workspace.editor().buffer.is_dirty());
        assert!(
            workspace
                .tabs
                .iter()
                .all(|tab| tab.label() == "clean.txt [deleted] •")
        );
        assert_eq!(workspace.window_title(), "clean.txt [deleted] •");
        let (window, documents) = workspace.snapshot().unwrap();
        assert_eq!(documents.len(), 1);
        assert!(documents[0].dirty);
        assert!(documents[0].history.as_ref().unwrap().retains_text());
        let mut session = vel_session::Session {
            windows: vec![window],
            documents,
            ..Default::default()
        };
        session.apply_policy(vel_session::HotExit::Always, false, true);
        let session: vel_session::Session =
            serde_json::from_slice(&serde_json::to_vec(&session).unwrap()).unwrap();
        session.validate().unwrap();
        let mut restored = Workspace::new(Text::new(), None, None, false).unwrap();
        restored
            .restore_window(&session.windows[0], &session.documents)
            .unwrap();
        assert!(restored.tabs.iter().any(|tab| tab.lazy.is_some()));
        for index in 0..restored.tabs.len() {
            restored.prepare_sheet(index).unwrap();
            assert!(restored.tabs[index].deleted);
            assert!(restored.tabs[index].dirty());
            assert_eq!(restored.tabs[index].label(), "clean.txt [deleted] •");
            assert_eq!(restored.tabs[index].editor.text, contents);
        }
        assert!(!file.exists());
        let (_, retained) = restored.snapshot().unwrap();
        assert_eq!(retained.len(), 1);
        assert!(retained[0].dirty);
        assert_eq!(
            retained[0]
                .history
                .as_ref()
                .unwrap()
                .current_text()
                .unwrap(),
            contents
        );
    }
    #[test]
    fn deleted_file_save_clears_shared_and_lazy_markers_including_save_as() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let original = root.join("original.txt");
        let replacement = root.join("replacement.txt");
        let contents = "saved contents\n";
        std::fs::write(&original, contents).unwrap();
        let mut workspace = Workspace::new(Text::new(), None, None, false).unwrap();
        workspace
            .open(FileLocation {
                path: original.clone(),
                line: 1,
                column: 1,
            })
            .unwrap();
        for _ in 0..2 {
            workspace.clone_file().unwrap();
        }
        std::fs::remove_file(&original).unwrap();
        crate::context_menu::apply_delete(&mut workspace, std::slice::from_ref(&original));
        let (window, documents) = workspace.snapshot().unwrap();
        let session: vel_session::Session = serde_json::from_slice(
            &serde_json::to_vec(&vel_session::Session {
                windows: vec![window],
                documents,
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();
        let mut restored = Workspace::new(Text::new(), None, None, false).unwrap();
        restored
            .restore_window(&session.windows[0], &session.documents)
            .unwrap();
        assert_eq!(restored.tabs.len(), 3);
        assert!(restored.tabs.iter().any(|tab| tab.lazy.is_some()));
        let active = restored.active;
        assert!(restored.save(active, Some(dir.path().to_owned())).is_err());
        assert!(restored.tabs.iter().all(|tab| tab.deleted && tab.dirty()));
        restored.save(active, Some(replacement.clone())).unwrap();
        assert!(!original.exists());
        assert_eq!(std::fs::read_to_string(&replacement).unwrap(), contents);
        assert!(restored.tabs.iter().all(|tab| !tab.deleted && !tab.dirty()));
        assert!(
            restored
                .tabs
                .iter()
                .all(|tab| tab.path.as_ref() == Some(&replacement))
        );
        for index in 0..restored.tabs.len() {
            restored.select(index);
            assert_eq!(restored.editor().text, contents);
            assert!(!restored.tabs[index].deleted);
            assert!(!restored.tabs[index].dirty());
        }
        std::fs::remove_file(&replacement).unwrap();
        crate::context_menu::apply_delete(&mut restored, std::slice::from_ref(&replacement));
        assert!(restored.tabs.iter().all(|tab| tab.deleted && tab.dirty()));
        restored.save(restored.active, None).unwrap();
        assert_eq!(std::fs::read_to_string(&replacement).unwrap(), contents);
        assert!(restored.tabs.iter().all(|tab| !tab.deleted && !tab.dirty()));
        let (_, documents) = restored.snapshot().unwrap();
        assert_eq!(documents.len(), 1);
        assert!(!documents[0].dirty);
    }
    #[test]
    fn pending_center_only_tracks_locations_and_is_consumed_per_active_tab() {
        let mut workspace =
            Workspace::new(Text::from("first\n你好\nthird"), None, None, false).unwrap();
        assert!(!workspace.take_pending_center());
        workspace.editor_mut().set_caret(1, false);
        assert!(!workspace.take_pending_center());
        workspace.tabs[0].goto(&FileLocation {
            path: "unused".into(),
            line: 2,
            column: 2,
        });
        assert_eq!(workspace.editor().caret, "first\n你".len());
        workspace.new_file().unwrap();
        assert!(!workspace.take_pending_center());
        workspace.select(0);
        assert!(workspace.take_pending_center());
        assert!(!workspace.take_pending_center());
        workspace.tabs[0].install(Loaded {
            text: Text::from("replacement"),
            format: Format::default(),
            source_bytes: 11,
        });
        assert!(!workspace.take_pending_center());
        // The same install path is used when an asynchronous text load finishes.
        workspace.tabs[0].location = Some(FileLocation {
            path: "unused".into(),
            line: 2,
            column: 2,
        });
        workspace.tabs[0].install(Loaded {
            text: Text::from("first\n你好\nthird"),
            format: Format::default(),
            source_bytes: 18,
        });
        assert_eq!(workspace.editor().caret, "first\n你".len());
        assert!(workspace.take_pending_center());
        assert!(!workspace.take_pending_center());
    }
    #[test]
    fn stamp_polling_waits_500ms_but_explicit_reload_bypasses_the_interval() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("Packages/User");
        std::fs::create_dir_all(&user).unwrap();
        let prefs = user.join("Preferences.sublime-settings");
        std::fs::write(&prefs, r#"{"font_size":15}"#).unwrap();
        let mut workspace =
            Workspace::new(Text::new(), Some(dir.path().into()), None, false).unwrap();
        let now = Instant::now();
        assert!(!workspace.configuration_changed(now));
        std::fs::write(&prefs, r#"{"font_size":20,"tab_size":4}"#).unwrap();
        assert!(!workspace.configuration_changed(now + Duration::from_millis(499)));
        assert!(workspace.configuration_changed(now + Duration::from_millis(500)));
        workspace.request_resource_reload();
        assert!(workspace.configuration_changed(now + Duration::from_millis(501)));
        workspace.last_config_check = Some(Instant::now());
        workspace.poll();
        assert!(workspace.config_rx.is_some() || workspace.resource_revision > 0);
        workspace.wait_resources();
        assert_eq!(workspace.editor().preferences.font_size, 20.);
    }
    #[test]
    fn context_menu_resources_layer_and_hot_reload_with_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("Packages/Fixture");
        let user = dir.path().join("Packages/User");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(
            package.join("Context.sublime-menu"),
            r#"[{"id":"fixture","caption":"Package","command":"copy"}]"#,
        )
        .unwrap();
        let menu = user.join("Context.sublime-menu");
        std::fs::write(
            &menu,
            r#"[{"id":"fixture","caption":"User","command":"paste"}]"#,
        )
        .unwrap();
        let mut workspace =
            Workspace::new(Text::new(), Some(dir.path().into()), None, false).unwrap();
        for name in [
            "Context.sublime-menu",
            "Tab Context.sublime-menu",
            "Side Bar.sublime-menu",
        ] {
            assert!(!workspace.context_menus[name].is_empty(), "{name}");
        }
        let item = workspace.context_menus["Context.sublime-menu"]
            .iter()
            .find(|item| item.id == "fixture")
            .unwrap();
        assert_eq!(item.caption, "User");
        assert_eq!(item.command.as_deref(), Some("paste"));
        assert!(!workspace.menus.iter().any(|item| item.id == "fixture"));
        std::fs::write(
            menu,
            r#"[{"id":"fixture","caption":"Reloaded","command":"cut"}]"#,
        )
        .unwrap();
        workspace.wait_resources();
        let item = workspace.context_menus["Context.sublime-menu"]
            .iter()
            .find(|item| item.id == "fixture")
            .unwrap();
        assert_eq!(item.caption, "Reloaded");
        assert_eq!(item.command.as_deref(), Some("cut"));
    }
    /// Small paired helper for the stamp traversal changed in round 14. This measures
    /// resource polling alone; the main agent still runs full typing/scroll gates.
    #[test]
    #[ignore = "manual release benchmark; prints paired resource-stamp scan costs"]
    fn benchmark_resource_stamp_polling() {
        let mut workspace = Workspace::new(Text::new(), None, None, false).unwrap();
        let iterations = 10_000;
        let mut baseline = Vec::new();
        let mut throttled = Vec::new();
        for _ in 0..5 {
            let before = Instant::now();
            for _ in 0..iterations {
                std::hint::black_box(
                    workspace
                        .config_stamp
                        .iter()
                        .any(|(path, saved)| stamp(path) != *saved),
                );
            }
            baseline.push(before.elapsed().as_secs_f64() * 1e9 / iterations as f64);
            workspace.last_config_check = None;
            let before = Instant::now();
            for _ in 0..iterations {
                std::hint::black_box(workspace.configuration_changed(Instant::now()));
            }
            throttled.push(before.elapsed().as_secs_f64() * 1e9 / iterations as f64);
        }
        println!(
            "{}",
            serde_json::json!({"kind":"resource_stamp_poll_benchmark","iterations_per_sample":iterations,"resource_stamps":workspace.config_stamp.len(),"baseline_ns_per_poll":baseline,"throttled_ns_per_poll":throttled,"scope":"resource stamps only; no GUI/GPU or full-frame performance claim"})
        );
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

#[cfg(test)]
mod round15_tests {
    use super::*;
    use vel_config::{json, settings::Layer};
    fn location(path: PathBuf) -> FileLocation {
        FileLocation {
            path,
            line: 1,
            column: 1,
        }
    }
    fn workspace() -> Workspace {
        Workspace::new(Text::new(), None, None, false).unwrap()
    }
    #[test]
    fn missing_paths_are_named_new_documents_and_save_errors_keep_the_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.txt");
        let mut w = workspace();
        w.open(location(path.clone())).unwrap();
        assert_eq!(w.tabs[0].path.as_ref(), Some(&path));
        assert_eq!(w.editor().text.len_bytes(), 0);
        assert!(w.tabs[0].label().contains("[new]"));
        w.editor_mut()
            .run("insert", &json!({"characters":"created"}))
            .unwrap();
        w.save(0, None).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "created");
        assert_eq!(w.recent_files.first(), Some(&path));
        assert!(!w.tabs[0].dirty());
        assert!(!w.tabs[0].label().contains("[new]"));
        let missing_parent = dir.path().join("missing/child.txt");
        w.open(location(missing_parent.clone())).unwrap();
        w.editor_mut()
            .run("insert", &json!({"characters":"keep me"}))
            .unwrap();
        let error = w.save(w.active, None).unwrap_err();
        assert!(error.contains("missing"));
        assert!(!missing_parent.parent().unwrap().exists());
        assert_eq!(w.editor().text.to_string(), "keep me");
        assert!(w.tabs[w.active].dirty());
    }
    #[test]
    fn active_close_chooses_right_then_left_in_its_own_group() {
        let mut w = workspace();
        for text in ["left", "right", "other"] {
            w.new_file().unwrap();
            w.editor_mut()
                .run("insert", &json!({"characters":text}))
                .unwrap();
        }
        w.workbench
            .set_layout(vel_ui::workbench::Layout::grid(2, 1))
            .unwrap();
        w.workbench.move_selected(1).unwrap();
        let other = w.tabs[3].sheet;
        w.select(1);
        let right = w.tabs[2].sheet;
        w.close(1).unwrap();
        assert_eq!(w.tabs[w.active].sheet, right);
        assert_eq!(w.workbench.active, 0);
        w.close(w.active).unwrap();
        assert_ne!(w.tabs[w.active].sheet, other);
        assert_eq!(w.workbench.active, 0);
    }
    #[test]
    fn focusing_an_unused_pane_never_edits_another_groups_document() {
        let mut w = Workspace::new(Text::from("original group"), None, None, false).unwrap();
        w.workbench
            .set_layout(vel_ui::workbench::Layout::grid(2, 1))
            .unwrap();
        w.workbench.active = 1;
        w.focus_current_group().unwrap();
        assert_eq!(w.editor().text.len_bytes(), 0);
        w.editor_mut()
            .run("insert", &json!({"characters":"new group"}))
            .unwrap();
        assert_eq!(w.tabs[0].editor.text.to_string(), "original group");
        assert_eq!(w.workbench.group_of(w.tabs[w.active].sheet), Some(1));
    }
    #[test]
    fn final_close_leaves_no_sheet_and_typing_or_new_file_materializes_on_demand() {
        let mut w = workspace();
        w.close(0).unwrap();
        assert!(w.is_empty());
        assert!(w.workbench.groups[0].sheets.is_empty());
        assert_eq!(w.window_title(), "Velocity");
        let (window, docs) = w.snapshot().unwrap();
        assert!(window.sheets.is_empty());
        assert!(docs.is_empty());
        let mut restored = workspace();
        restored.restore_window(&window, &docs).unwrap();
        assert!(restored.is_empty());
        restored
            .editor_mut()
            .run("insert", &json!({"characters":"first edit"}))
            .unwrap();
        restored.sync_shared();
        assert!(!restored.is_empty());
        assert_eq!(restored.workbench.groups[0].sheets.len(), 1);
        assert_eq!(restored.editor().text.to_string(), "first edit");
        w.new_file().unwrap();
        assert!(!w.is_empty());
        assert_eq!(w.tabs.len(), 1);
        assert_eq!(w.workbench.groups[0].sheets.len(), 1);
    }
    #[test]
    fn save_all_skips_clean_tabs_and_continues_after_cancel_and_failure() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = workspace();
        // The initial clean untitled tab must not summon a save dialog.
        for text in ["cancel this", "failure", "save this", "read only"] {
            w.new_file().unwrap();
            w.editor_mut()
                .run("insert", &json!({"characters":text}))
                .unwrap();
        }
        w.editor_mut().read_only = true;
        let mut prompted = Vec::new();
        let errors = w.save_all(|name| {
            prompted.push(name.to_owned());
            match name {
                "cancel this" => None,
                "failure" => Some(dir.path().join("absent/child")),
                other => Some(dir.path().join(other)),
            }
        });
        assert!(!prompted.iter().any(|s| s == "Untitled"));
        assert_eq!(errors.len(), 2);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("save this")).unwrap(),
            "save this"
        );
        assert!(w.tabs[1].dirty());
        assert!(w.tabs[2].dirty());
        assert!(!w.tabs[3].dirty());
        assert_eq!(w.recent_files.first(), Some(&dir.path().join("save this")));
        assert!(w.tabs[4].dirty());
    }
    #[test]
    fn explicit_reopen_decodes_selected_codec_and_failure_preserves_dirty_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("encoding.txt");
        std::fs::write(&path, "é").unwrap();
        let mut w = workspace();
        w.open(location(path.clone())).unwrap();
        w.reopen_encoding("Western (Windows 1252)").unwrap();
        assert_eq!(w.editor().text.to_string(), "Ã©");
        w.editor_mut()
            .run("insert", &json!({"characters":"keep unsaved "}))
            .unwrap();
        let old = w.editor().text.clone();
        std::fs::write(&path, b"a").unwrap();
        assert!(w.reopen_encoding("UTF-16 LE with BOM").is_err());
        assert_eq!(w.editor().text, old);
        assert!(w.tabs[0].dirty());
    }
    #[test]
    fn disabled_hot_exit_routes_cli_projects_to_a_separate_window() {
        let mut w = workspace();
        w.settings.set(Layer::User, "hot_exit", json!(false));
        w.editor_mut()
            .run("insert", &json!({"characters":"preserve"}))
            .unwrap();
        let options = crate::cli::Options {
            project: Some("project.sublime-project".into()),
            ..Default::default()
        };
        let request = crate::instance::Request::new(&options, None).unwrap();
        assert!(crate::open_requests::request_needs_window(&w, &request));
        w.settings.set(Layer::User, "hot_exit", json!("always"));
        assert!(!crate::open_requests::request_needs_window(&w, &request));
    }
    #[test]
    fn close_deleted_files_closes_clean_watched_tabs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deleted.txt");
        std::fs::write(&path, "clean").unwrap();
        let mut w = Workspace::new(Text::new(), None, None, true).unwrap();
        w.settings
            .set(Layer::User, "close_deleted_files", json!(true));
        w.open(location(path.clone())).unwrap();
        std::fs::remove_file(path).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !w.is_empty() {
            w.poll();
            assert!(Instant::now() < deadline);
            std::thread::park_timeout(Duration::from_millis(2));
        }
        assert!(w.workbench.groups[0].sheets.is_empty());
    }
    #[test]
    fn untitled_names_are_unicode_bounded_and_close_names_have_no_dirty_marker() {
        let mut w = workspace();
        let first = "你好".repeat(40);
        w.editor_mut()
            .run(
                "insert",
                &json!({"characters":format!("  {first}\nsecond")}),
            )
            .unwrap();
        assert_eq!(w.tabs[0].name().chars().count(), 50);
        assert!(!w.tabs[0].name().contains('•'));
        assert!(w.tabs[0].label().ends_with(" •"));
        w.editor_mut()
            .settings
            .set(Layer::View, "set_unsaved_view_name", json!(false));
        assert_eq!(w.tabs[0].name(), "Untitled");
    }
    #[test]
    fn reopen_stack_is_window_local_and_restores_selection_and_scroll() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("closed.txt");
        std::fs::write(&path, "abcdef\n").unwrap();
        let mut w = workspace();
        w.open(location(path.clone())).unwrap();
        w.editor_mut()
            .set_selection(Selection::from_regions([Region::new(4, 1)]));
        w.tabs[0].scroll = (15., 96.);
        w.close(0).unwrap();
        assert_eq!(w.closed_files.len(), 1);
        assert!(workspace().closed_files.is_empty());
        w.reopen_last_file().unwrap();
        assert!(w.closed_files.is_empty());
        assert_eq!(w.tabs[0].path.as_ref(), Some(&path.canonicalize().unwrap()));
        assert_eq!(w.editor().regions(), vec![Region::new(4, 1)]);
        assert_eq!(w.tabs[0].scroll, (15., 96.));
        assert!(!w.take_pending_center());
    }
    #[test]
    fn project_save_as_preserves_relative_paths_unknown_folder_keys_and_absent_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let old_dir = dir.path().join("old");
        let new_dir = dir.path().join("new");
        std::fs::create_dir_all(old_dir.join("src")).unwrap();
        std::fs::create_dir(&new_dir).unwrap();
        let old = old_dir.join("app.sublime-project");
        std::fs::write(&old, r#"{"folders":[{"path":"src","file_include_patterns":["*.rs"],"folder_include_patterns":["code*"],"custom_folder":{"keep":7}}],"custom_project":42}"#).unwrap();
        let mut w = Workspace::new(Text::new(), None, Some(old.clone()), false).unwrap();
        assert!(w.folders[0].follow_links);
        let new = new_dir.join("renamed.sublime-project");
        w.save_project_as(new.clone()).unwrap();
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&new).unwrap()).unwrap();
        assert_eq!(saved["folders"][0]["path"], "../old/src");
        assert_eq!(saved["folders"][0]["custom_folder"]["keep"], 7);
        assert_eq!(saved["custom_project"], 42);
        for key in [
            "follow_symlinks",
            "file_exclude_patterns",
            "folder_exclude_patterns",
            "binary_file_patterns",
        ] {
            assert!(saved["folders"][0].get(key).is_none(), "{key}");
        }
        assert!(saved.get("settings").is_none());
        assert!(saved.get("build_systems").is_none());
        assert!(w.folders[0].rules.allows(
            &w.folders[0].path,
            &w.folders[0].path.join("main.rs"),
            false,
            true
        ));
        assert!(!w.folders[0].rules.allows(
            &w.folders[0].path,
            &w.folders[0].path.join("main.txt"),
            false,
            true
        ));
        assert!(
            std::fs::read_to_string(old)
                .unwrap()
                .contains("\"path\":\"src\"")
        );
    }
    #[test]
    fn documented_exclusions_hide_build_objects_but_allow_target() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("app.sublime-project");
        std::fs::write(&project, r#"{"folders":[{"path":"."}]}"#).unwrap();
        let w = Workspace::new(Text::new(), None, Some(project), false).unwrap();
        let folder = &w.folders[0];
        for name in ["target", "node_modules", "src"] {
            assert!(
                folder
                    .rules
                    .allows(&folder.path, &folder.path.join(name), true, false),
                "{name}"
            );
        }
        for name in [".svn", ".hg", ".git", "CVS"] {
            assert!(
                !folder
                    .rules
                    .allows(&folder.path, &folder.path.join(name), true, false),
                "{name}"
            );
        }
        for name in [
            ".DS_Store",
            "module.o",
            "module.obj",
            "app.sublime-workspace",
            "module.pyc",
        ] {
            assert!(
                !folder
                    .rules
                    .allows(&folder.path, &folder.path.join(name), false, true),
                "{name}"
            );
        }
    }
    #[test]
    fn indentation_choices_survive_resource_and_external_reload_and_project_watching() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("profile");
        let file = dir.path().join("indent.rs");
        let project = dir.path().join("project.sublime-project");
        std::fs::write(&file, "fn main() {\n  first();\n  second();\n}\n").unwrap();
        std::fs::write(&project, r#"{"folders":[{"path":"."}]}"#).unwrap();
        let mut w = Workspace::new(Text::new(), Some(data), None, true).unwrap();
        w.open_project(project).unwrap();
        assert!(w.watcher.is_some());
        w.open(location(file.clone())).unwrap();
        assert_eq!(w.editor().settings.usize("tab_size", 0), 2);
        w.editor_mut()
            .settings
            .set(Layer::View, "tab_size", json!(8));
        w.editor_mut()
            .settings
            .set(Layer::View, "translate_tabs_to_spaces", json!(false));
        w.request_resource_reload();
        w.wait_resources();
        assert_eq!(w.editor().settings.usize("tab_size", 0), 8);
        std::fs::write(&file, "fn main() {\n    changed();\n    again();\n}\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !w.editor().text.to_string().contains("changed") {
            w.poll();
            assert!(Instant::now() < deadline, "{:?}", w.messages);
            std::thread::park_timeout(Duration::from_millis(2));
        }
        assert_eq!(w.editor().settings.usize("tab_size", 0), 8);
        assert!(!w.editor().settings.bool("translate_tabs_to_spaces", true));
        std::fs::remove_file(&file).unwrap();
        while !w.tabs[w.active].deleted {
            w.poll();
            assert!(Instant::now() < deadline);
            std::thread::park_timeout(Duration::from_millis(2));
        }
        assert!(w.tabs[w.active].dirty());
        assert!(w.tabs[w.active].label().contains("[deleted]"));
        w.save(w.active, None).unwrap();
        assert!(file.exists());
        assert!(!w.tabs[w.active].deleted);
    }
    #[test]
    fn missing_files_do_not_abort_restore_and_close_deleted_files_keeps_dirty_text() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.txt");
        let healthy = dir.path().join("healthy.txt");
        std::fs::write(&missing, "saved").unwrap();
        std::fs::write(&healthy, "healthy").unwrap();
        let mut w = workspace();
        w.open(location(missing.clone())).unwrap();
        w.open(location(healthy)).unwrap();
        let (window, documents) = w.snapshot().unwrap();
        std::fs::remove_file(missing).unwrap();
        let mut restored = workspace();
        restored.restore_window(&window, &documents).unwrap();
        assert_eq!(restored.tabs.len(), 2);
        restored.select(0);
        assert!(restored.tabs[0].deleted);
        assert!(restored.tabs[0].dirty());
        restored.select(1);
        assert_eq!(restored.editor().text.to_string(), "healthy");
        let mut close_deleted = workspace();
        close_deleted
            .settings
            .set(Layer::User, "close_deleted_files", json!(true));
        close_deleted.restore_window(&window, &documents).unwrap();
        assert_eq!(close_deleted.tabs.len(), 1);
        let mut dirty_docs = documents;
        dirty_docs[0].dirty = true;
        let mut buffer = vel_buffer::Buffer::new("retained dirty text");
        buffer.new_view();
        dirty_docs[0].history = Some(vel_session::History::capture(&buffer));
        close_deleted.restore_window(&window, &dirty_docs).unwrap();
        assert_eq!(close_deleted.tabs.len(), 2);
        close_deleted.select(0);
        assert_eq!(
            close_deleted.editor().text.to_string(),
            "retained dirty text"
        );
    }
    #[cfg(unix)]
    #[test]
    fn failing_visible_and_lazy_restore_is_reported_once_and_other_tabs_load() {
        use std::os::unix::net::UnixListener;
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("failed.txt");
        let good = dir.path().join("good.txt");
        std::fs::write(&bad, "bad").unwrap();
        std::fs::write(&good, "good").unwrap();
        let mut w = workspace();
        w.open(location(bad.clone())).unwrap();
        w.open(location(good)).unwrap();
        let (mut window, documents) = w.snapshot().unwrap();
        // Bypass the saved history to exercise the plain file loader's lazy failure.
        let mut documents = documents;
        documents[0].history = None;
        std::fs::remove_file(&bad).unwrap();
        let _socket = UnixListener::bind(bad).unwrap();
        let mut restored = workspace();
        restored.restore_window(&window, &documents).unwrap();
        assert_eq!(restored.editor().text.to_string(), "good");
        assert!(restored.prepare_sheet(0).is_err());
        assert!(restored.tabs[0].lazy.is_none());
        assert!(restored.prepare_sheet(0).is_ok());
        let sheets = window.workbench.groups[0].sheets.clone();
        window.workbench.groups[0].selected.extend(sheets);
        let mut visible = workspace();
        visible.restore_window(&window, &documents).unwrap();
        assert_eq!(
            visible
                .messages
                .iter()
                .filter(|m| m.contains("failed.txt"))
                .count(),
            1
        );
        visible.select(0);
        visible.editor_mut();
        assert_eq!(
            visible
                .messages
                .iter()
                .filter(|m| m.contains("failed.txt"))
                .count(),
            1
        );
    }
    #[test]
    fn legacy_hot_exit_false_and_recent_encoding_commands_are_wired() {
        let mut w = workspace();
        w.settings.set(Layer::User, "hot_exit", json!(false));
        assert_eq!(w.hot_exit(), vel_session::HotExit::Disabled);
        let key = vel_config::keymap::Key::parse("super+shift+t").unwrap();
        assert!(
            matches!(w.editor_mut().resolve_key(&key), vel_config::keymap::Resolution::Command(ref command) if command.command == "reopen_last_file")
        );
        w.recent_files.push("/tmp/a.txt".into());
        w.recent_folders.push("/tmp/folder".into());
        w.recent_projects
            .push("/tmp/project.sublime-project".into());
        assert!(w.refresh_file_menus());
        assert!(!w.refresh_file_menus());
        let file = w.menus.iter().find(|m| m.id == "file").unwrap();
        let recent = file
            .children
            .iter()
            .find(|m| m.id == "open_recent")
            .unwrap();
        assert_eq!(recent.children.len(), 4);
        for command in ["reopen_encoding", "save_encoding"] {
            let submenu = file
                .children
                .iter()
                .find(|m| {
                    m.children
                        .first()
                        .is_some_and(|c| c.command.as_deref() == Some(command))
                })
                .unwrap();
            assert_eq!(submenu.children.len(), vel_fileio::ENCODINGS.len());
        }
    }
}

#[cfg(test)]
mod review15_tests {
    use super::*;
    use vel_config::json;
    fn workspace(watch: bool) -> Workspace {
        Workspace::new(Text::new(), None, None, watch).unwrap()
    }
    fn location(path: &Path) -> FileLocation {
        FileLocation {
            path: path.into(),
            line: 1,
            column: 1,
        }
    }
    fn until(w: &mut Workspace, condition: impl Fn(&Workspace) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !condition(w) {
            w.poll();
            assert!(Instant::now() < deadline, "{:?}", w.messages);
            std::thread::park_timeout(Duration::from_millis(2));
        }
    }
    #[test]
    fn empty_window_previews_never_leak_sheets_or_break_all_window_hot_exit() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "first").unwrap();
        std::fs::write(&b, "second").unwrap();
        let mut w = workspace(false);
        w.close(0).unwrap();
        let hidden = w.tabs[0].sheet;
        w.preview(location(&a)).unwrap();
        assert_eq!(w.tabs.len(), 1);
        assert_ne!(w.tabs[0].sheet, hidden);
        w.preview(location(&b)).unwrap(); // Replacing the only transient creates another empty adapter.
        assert_eq!(w.tabs.len(), 1);
        assert_eq!(w.workbench.groups[0].sheets, vec![w.tabs[0].sheet]);
        let mut other = workspace(false);
        other
            .editor_mut()
            .run("insert", &json!({"characters":"unsaved in another window"}))
            .unwrap();
        let mut sessions = crate::session::Sessions::new(Some(dir.path()));
        for window in [&mut w, &mut other] {
            let id = sessions.register();
            let (snapshot, documents) = window.snapshot().unwrap();
            sessions
                .publish(
                    id,
                    snapshot,
                    documents,
                    Default::default(),
                    crate::session::Policy {
                        hot: vel_session::HotExit::Always,
                        quitting: true,
                        closing: false,
                        remember: true,
                    },
                )
                .unwrap();
        }
        sessions.flush().unwrap();
        let restored = vel_session::load(&dir.path().join("Sessions/Session.sublime-session"));
        assert!(restored.messages.is_empty(), "{:?}", restored.messages);
        restored.session.validate().unwrap();
        assert_eq!(restored.session.windows.len(), 2);
        assert!(
            restored
                .session
                .documents
                .iter()
                .any(|d| d.history.as_ref().is_some_and(|h| h
                    .current_text()
                    .is_ok_and(|t| t == "unsaved in another window")))
        );
        w.close(w.active).unwrap(); // Escape's close, then a stale select of the old origin.
        w.select(0);
        assert!(w.is_empty());
        assert!(w.snapshot().unwrap().0.sheets.is_empty());
        w.editor_mut()
            .run("insert", &json!({"characters":"visible input"}))
            .unwrap();
        w.materialize_input();
        assert!(!w.is_empty());
        assert_eq!(w.workbench.groups[0].sheets, vec![w.tabs[0].sheet]);
        assert_eq!(w.tabs.len(), 1);
        assert!(w.closed_files.is_empty());
    }
    #[test]
    fn cancelled_goto_in_an_empty_window_leaves_no_ghost_untitled() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("preview.txt");
        std::fs::write(&file, "preview").unwrap();
        let mut w = workspace(false);
        w.close(0).unwrap();
        let mut panels = crate::panels::Panels::new(&w, false);
        panels
            .action(&mut w, "show_overlay", &json!({"overlay":"goto"}), 0..0)
            .unwrap();
        w.preview(location(&file)).unwrap();
        panels
            .key(
                &mut w,
                &vel_config::keymap::Key::parse("escape").unwrap(),
                0..7,
            )
            .unwrap();
        assert!(w.is_empty());
        assert!(w.workbench.groups.iter().all(|g| g.sheets.is_empty()));
        assert!(w.snapshot().unwrap().1.is_empty());
        assert!(w.closed_files.is_empty());
    }
    #[test]
    fn recreated_files_clear_deletion_and_reload_only_without_real_edits() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("checkout.txt");
        std::fs::write(&file, "original").unwrap();
        let mut w = workspace(true);
        w.open(location(&file)).unwrap();
        w.clone_file().unwrap();
        for replacement in ["original", "changed checkout"] {
            std::fs::remove_file(&file).unwrap();
            until(&mut w, |w| w.tabs.iter().all(|t| t.deleted));
            std::fs::write(&file, replacement).unwrap();
            until(&mut w, |w| {
                w.tabs.iter().all(|t| !t.deleted) && w.editor().text == replacement
            });
            assert!(w.tabs.iter().all(|t| !t.dirty() && t.external.is_none()));
        }
        std::fs::remove_file(&file).unwrap();
        until(&mut w, |w| w.tabs.iter().all(|t| t.deleted));
        w.editor_mut()
            .run("insert", &json!({"characters":"local edit "}))
            .unwrap();
        let local = w.editor().text.clone();
        std::fs::write(&file, "new disk text").unwrap();
        until(&mut w, |w| w.tabs[w.active].external.is_some());
        assert!(!w.tabs[w.active].deleted);
        assert!(w.tabs[w.active].dirty());
        assert_eq!(w.editor().text, local);
    }
    #[test]
    fn reopen_skips_preview_closes_and_missing_entries_but_keeps_real_view_state() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("real.txt");
        let b = dir.path().join("missing.txt");
        let c = dir.path().join("preview.txt");
        for path in [&a, &b, &c] {
            std::fs::write(path, "abcdef").unwrap();
        }
        let mut w = workspace(false);
        w.open(location(&a)).unwrap();
        w.editor_mut()
            .set_selection(Selection::from_regions([Region::new(5, 2)]));
        w.tabs[0].scroll = (2., 50.);
        w.close(0).unwrap();
        w.open(location(&b)).unwrap();
        w.close(0).unwrap();
        std::fs::remove_file(&b).unwrap();
        w.preview(location(&c)).unwrap();
        w.close(0).unwrap();
        assert_eq!(w.closed_files.len(), 2);
        w.reopen_last_file().unwrap();
        assert_eq!(
            w.tabs[w.active].path.as_deref(),
            Some(a.canonicalize().unwrap().as_path())
        );
        assert_eq!(w.editor().regions(), vec![Region::new(5, 2)]);
        assert_eq!(w.tabs[w.active].scroll, (2., 50.));
        assert!(w.closed_files.is_empty());
        w.close(w.active).unwrap();
        std::fs::remove_file(a).unwrap();
        w.reopen_last_file().unwrap();
        assert!(w.is_empty());
        assert!(w.closed_files.is_empty());
    }
    #[test]
    fn hidden_input_cannot_be_saved_as_an_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unintended.txt");
        let mut w = workspace(false);
        w.close(0).unwrap();
        assert!(w.save(0, Some(path.clone())).is_err());
        assert!(!path.exists());
        w.editor_mut()
            .run("insert", &json!({"characters":"intended"}))
            .unwrap();
        w.save(0, Some(path.clone())).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "intended");
    }
}

#[cfg(test)]
mod review15_indentation_tests {
    use super::*;
    use vel_config::json;
    #[test]
    fn untitled_and_cloned_manual_indentation_is_not_redetected_without_a_load() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = Workspace::new(Text::new(), None, None, false).unwrap();
        for empty in [false, true] {
            if empty {
                while !w.is_empty() {
                    w.close(w.active).unwrap();
                }
            } else {
                w.new_file().unwrap();
            }
            w.editor_mut()
                .settings
                .set(Layer::View, "tab_size", json!(8));
            w.editor_mut()
                .run(
                    "insert",
                    &json!({"characters":"fn main() {\n  one();\n  two();\n}\n"}),
                )
                .unwrap();
            w.materialize_input();
            w.clone_file().unwrap();
            w.set_appearance(true).unwrap();
            w.poll();
            w.save(
                w.active,
                Some(dir.path().join(format!("manual-{empty}.rs"))),
            )
            .unwrap();
            assert_eq!(w.editor().settings.usize("tab_size", 0), 8);
            let path = w.tabs[w.active].path.clone().unwrap();
            std::fs::write(&path, "fn main() {\n    changed();\n    again();\n}\n").unwrap();
            w.tabs[w.active].external = Some(fileio::load(&path, "windows-1252", |_| {}).unwrap());
            w.reload_external(w.active, true);
            w.poll();
            assert_eq!(w.editor().settings.usize("tab_size", 0), 8);
        }
    }
}

#[cfg(test)]
mod resource_log_tests {
    use super::*;
    #[test]
    fn resource_log_is_timestamped_and_rotates_past_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("Velocity.log");
        let mut reports = Vec::new();
        write_resource_log(Some(dir.path()), &["first warning".into()], &mut reports);
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.starts_with('[') && text.contains("1 resource warning(s)\nfirst warning\n"));
        std::fs::write(&log, vec![b'x'; RESOURCE_LOG_LIMIT as usize + 1]).unwrap();
        write_resource_log(Some(dir.path()), &["second warning".into()], &mut reports);
        assert_eq!(
            std::fs::metadata(dir.path().join("Velocity.log.1"))
                .unwrap()
                .len(),
            RESOURCE_LOG_LIMIT + 1
        );
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("second warning") && !text.contains('x'));
        assert!(reports.is_empty());
    }
}
