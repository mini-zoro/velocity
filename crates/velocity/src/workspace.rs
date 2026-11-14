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
    language_settings_revision: u64,
    needs_detection: bool,
    pub transient: bool,
    pub editor: Editor,
    pub path: Option<PathBuf>,
    pub format: Format,
    saved_format: Format,
    pub tracker: ChangeTracker,
    pub scroll: (f64, f64),
    pub loading: Option<LoadTask>,
    location: Option<FileLocation>,
    external_load: bool,
    load_started: Option<Instant>,
    disk_stamp: Option<(u64, SystemTime)>,
    pub external: Option<Loaded>,
}
impl Tab {
    fn new(text: Text, settings: &Settings, keymap: &str) -> Result<Self, String> {
        let mut editor = Editor::new(text);
        let tracker = ChangeTracker::new(&editor.buffer);
        editor.settings = settings.clone();
        editor.sync();
        editor.keymap = Keymap::default();
        editor.keymap.append(keymap)?;
        let format = fileio::default_format(&settings.editor())?;
        Ok(Self {
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
            location: None,
            external_load: false,
            load_started: None,
            disk_stamp: None,
            external: None,
        })
    }
    pub fn dirty(&self) -> bool {
        self.editor.buffer.is_dirty() || self.format != self.saved_format
    }
    pub fn label(&self) -> String {
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());
        format!("{name}{}", if self.dirty() { " •" } else { "" })
    }
    pub fn progress(&self) -> Option<(u64, u64)> {
        self.loading
            .as_ref()
            .map(|t| (t.read.load(Ordering::Relaxed), t.total))
    }
    fn install(&mut self, loaded: Loaded) {
        self.editor.cancel_composition();
        let old = self.editor.buffer.view(self.editor.id).sel().clone();
        let positions: Vec<_> = old
            .iter()
            .map(|r| {
                let p = self.editor.text.byte_to_point(r.b);
                (p.row, p.column)
            })
            .collect();
        self.editor.buffer = vel_buffer::Buffer::new(loaded.text.clone());
        self.editor.id = self.editor.buffer.new_view();
        self.editor.read_only = false;
        self.editor.sync();
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
        self.disk_stamp = self.path.as_deref().and_then(stamp);
        if let Some(location) = self.location.take() {
            self.goto(&location);
        }
    }
    pub fn goto(&mut self, location: &FileLocation) {
        let text = self.editor.buffer.text();
        let row = location.line.saturating_sub(1).min(text.len_lines() - 1);
        let start = text.line_to_byte(row);
        let line = text.line(row).to_string();
        let offset = line
            .char_indices()
            .nth(location.column.saturating_sub(1))
            .map_or(line.len(), |(i, _)| i);
        self.editor.set_caret(start + offset, false);
    }
}
#[derive(Debug)]
pub struct Workspace {
    pub assets: Arc<crate::language::Assets>,
    syntax_settings: BTreeMap<String, serde_json::Map<String, Value>>,
    schemes: BTreeMap<String, Arc<vel_color_scheme::ColorScheme>>,
    pub dark: bool,
    pub folders: Vec<vel_search::project::Folder>,
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub menus: Vec<MenuItem>,
    pub mousemap: Vec<MouseBinding>,
    pub palette: Vec<PaletteItem>,
    pub messages: Vec<String>,
    pub telemetry: Vec<Value>,
    settings: Settings,
    keymap: String,
    watcher: Option<Watcher>,
    data_dir: Option<PathBuf>,
    project: Option<PathBuf>,
    config_stamp: Vec<(PathBuf, Option<(u64, SystemTime)>)>,
}
fn stamp(path: &Path) -> Option<(u64, SystemTime)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()?))
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
impl Workspace {
    pub fn new(
        text: Text,
        data_dir: Option<PathBuf>,
        project: Option<PathBuf>,
        watch: bool,
    ) -> Result<Self, String> {
        let mut resources = Resources::default();
        for (name, source) in [
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
        if let Some(data) = &data_dir {
            resources.load(data)?;
        }
        let mut settings = Settings::default();
        let mut user = serde_json::Map::new();
        let mut syntax = serde_json::Map::new();
        let mut syntax_settings: BTreeMap<String, serde_json::Map<String, Value>> = BTreeMap::new();
        let mut config_stamp = Vec::new();
        for resource in resources.matching(".sublime-settings") {
            let file = resource.path.rsplit('/').next().unwrap_or("");
            let doc = Document::parse(resource.text()?)?;
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
            let doc = Document::parse(resource.text()?)?;
            keys.extend(doc.value.as_array().ok_or("keymap array")?.clone());
            if resource.origin.exists() {
                config_stamp.push((resource.origin.clone(), stamp(&resource.origin)));
            }
        }
        for resource in resources.matching(".sublime-menu") {
            if resource.path.ends_with("/Main.sublime-menu") {
                ui::merge_menu(&mut menus, ui::parse(resource.text()?)?);
            }
        }
        for resource in resources.matching(".sublime-mousemap") {
            mousemap.extend(ui::parse_mousemap(resource.text()?)?);
        }
        for resource in resources.matching(".sublime-commands") {
            palette.extend(ui::parse::<PaletteItem>(resource.text()?)?);
        }
        ui::extend_palette(&mut palette, &menus);
        let assets = crate::language::Assets::with_user_metadata(
            &crate::language::Assets::bundled()?,
            data_dir.as_deref(),
        )?;
        for syntax in assets.syntaxes.syntaxes().iter().filter(|s| !s.hidden) {
            palette.push(PaletteItem {
                caption: format!("Set Syntax: {}", syntax.name),
                invocation: vel_config::keymap::Invocation {
                    command: "set_file_type".into(),
                    args: serde_json::json!({"syntax": syntax.path}),
                },
            });
        }
        let mut scheme_resources = Vec::new();
        for suffix in [".sublime-color-scheme", ".hidden-color-scheme", ".tmTheme"] {
            for resource in resources.matching(suffix) {
                if resource.origin.exists() {
                    config_stamp.push((resource.origin.clone(), stamp(&resource.origin)));
                }
                scheme_resources.push((resource.path.clone(), resource.text()?.to_owned()));
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
                .map_err(|e| e.to_string())?;
                schemes.insert(name.to_owned(), Arc::new(scheme));
            }
        }
        // Track resource files and containing directories, including absent User
        // locations, so creating a new override is observable on the next poll.
        for suffix in [".sublime-menu", ".sublime-mousemap", ".sublime-commands"] {
            for resource in resources.matching(suffix) {
                if resource.origin.exists() {
                    config_stamp.push((resource.origin.clone(), stamp(&resource.origin)));
                }
            }
        }
        let parents: Vec<_> = config_stamp
            .iter()
            .filter_map(|(p, _)| p.parent().map(Path::to_owned))
            .collect();
        for parent in parents {
            config_stamp.push((parent.clone(), stamp(&parent)));
        }
        if let Some(data) = &data_dir {
            for path in [
                data.clone(),
                data.join("Packages"),
                data.join("Packages/User"),
                data.join("Installed Packages"),
            ] {
                let current = stamp(&path);
                config_stamp.push((path, current));
            }
        }
        config_stamp.sort_by(|a, b| a.0.cmp(&b.0));
        config_stamp.dedup_by(|a, b| a.0 == b.0);
        let keymap = serde_json::to_string(&keys).map_err(|e| e.to_string())?;
        let tab = Tab::new(text, &settings, &keymap)?;
        let mut messages = Vec::new();
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
        let mut workspace = Self {
            assets,
            syntax_settings,
            schemes,
            dark: true,
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
        };
        workspace.configure_language(0, None, true)?;
        Ok(workspace)
    }
    pub fn editor(&self) -> &Editor {
        &self.tabs[self.active].editor
    }
    pub fn editor_mut(&mut self) -> &mut Editor {
        &mut self.tabs[self.active].editor
    }
    pub fn language_busy(&self) -> bool {
        self.tabs.iter().any(|tab| {
            tab.loading.is_some()
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
        self.active = self.tabs.len() - 1;
        self.configure_language(self.active, None, false)?;
        Ok(())
    }
    pub fn select(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.editor_mut().unmark();
            self.editor_mut().keymap.cancel();
            self.active = index;
        }
    }
    pub fn open(&mut self, location: FileLocation) -> Result<(), String> {
        self.open_mode(location, false)
    }
    pub fn preview(&mut self, location: FileLocation) -> Result<(), String> {
        let target = absolute(&location.path)?;
        if !self.tabs.iter().any(|t| t.path.as_ref() == Some(&target))
            && let Some(index) = self.tabs.iter().position(|t| t.transient && !t.dirty())
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
        if size > 64 * 1024 * 1024 {
            tab.load_started = Some(started);
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
            self.tabs[0] = tab;
            self.active = 0;
        } else {
            self.tabs.push(tab);
            self.active = self.tabs.len() - 1;
        }
        self.configure_language(self.active, None, true)?;
        Ok(())
    }
    pub fn save(&mut self, index: usize, path: Option<PathBuf>) -> Result<(), String> {
        let tab = &mut self.tabs[index];
        tab.transient = false;
        if tab.loading.is_some() || tab.editor.read_only {
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
        tab.saved_format = tab.format.clone();
        tab.tracker.mark_saved(&tab.editor.buffer);
        tab.path = Some(target.clone());
        tab.disk_stamp = stamp(&target);
        if let Some(w) = self.watcher.as_mut() {
            w.watch(&target)?;
        }
        if let Err(e) = self.configure_language(index, None, true) {
            self.messages.push(e);
        }
        Ok(())
    }
    pub fn close(&mut self, index: usize) -> Result<(), String> {
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.new_file()?;
        }
        self.active = self.active.min(self.tabs.len() - 1);
        Ok(())
    }
    /// Polls jobs/notifications and reports whether scene content changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        for tab in &mut self.tabs {
            changed |= tab.editor.poll_language();
            if let Some(language) = tab.editor.language.borrow_mut().as_mut() {
                self.messages.append(&mut language.errors);
            }
            if tab.dirty() {
                tab.transient = false;
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
                    Err(e) => self.messages.push(e),
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
                                || tab.path.as_ref() != Some(&path)
                                || tab.disk_stamp == stamp(&path)
                            {
                                continue;
                            }
                            tab.disk_stamp = stamp(&path);
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
        if self.config_stamp.iter().any(|(p, s)| stamp(p) != *s) {
            match Self::new(
                Text::new(),
                self.data_dir.clone(),
                self.project.clone(),
                false,
            ) {
                Ok(config) => {
                    self.assets = config.assets;
                    self.syntax_settings = config.syntax_settings;
                    self.schemes = config.schemes;
                    self.folders = config.folders;
                    self.settings = config.settings;
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
                        let mut keymap = Keymap::default();
                        if keymap.append(&self.keymap).is_ok() {
                            tab.editor.keymap = keymap;
                        }
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
                        &self.tabs[index].editor.text.line(0).to_string(),
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
        for index in 0..self.tabs.len() {
            self.configure_language(index, None, false)?;
        }
        Ok(())
    }
    fn configure_language(
        &mut self,
        index: usize,
        explicit: Option<&str>,
        detect: bool,
    ) -> Result<(), String> {
        let tab = &mut self.tabs[index];
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
            &tab.editor.text.line(0).to_string(),
            chosen,
        )
        .or_else(|| {
            if chosen.is_none() {
                Some(self.assets.syntaxes.find_syntax_plain_text())
            } else {
                None
            }
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
        std::fs::write(&project, "{\"settings\":{\"tab_size\":6}}").unwrap();
        let mut workspace =
            Workspace::new(Text::new(), Some(dir.path().into()), Some(project), false).unwrap();
        assert_eq!(workspace.editor().preferences.tab_size, 6);
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
        assert!(workspace.poll());
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
