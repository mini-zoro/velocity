//! Window workbench integration, shared-buffer ownership and session/project adapters.
use crate::{
    cli::FileLocation,
    workspace::{Tab, Workspace},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};
use vel_buffer::Buffer;
use vel_config::{Value, json, settings::Layer};
use vel_session::{Document, Sheet, Window};
use vel_text::Text;
use vel_ui::workbench::{Direction, Layout};
static SHEET: AtomicU64 = AtomicU64::new(1);
pub(crate) fn menu_layout(args: &Value) -> Result<Layout, String> {
    Ok(match args["preset"].as_str() {
        Some("columns2") => Layout::grid(2, 1),
        Some("columns3") => Layout::grid(3, 1),
        Some("columns4") => Layout::grid(4, 1),
        Some("rows2") => Layout::grid(1, 2),
        Some("rows3") => Layout::grid(1, 3),
        Some("grid4") => Layout::grid(2, 2),
        Some(_) => Layout::default(),
        None => serde_json::from_value(args.clone()).map_err(|_| "Invalid group layout.")?,
    })
}
/// Keep new process-local identities above snapshots reserved from a previous process.
pub fn reserve_sheet_ids(maximum: u64) {
    SHEET.fetch_max(maximum.saturating_add(1), Ordering::Relaxed);
}
pub fn next_sheet() -> u64 {
    SHEET.fetch_add(1, Ordering::Relaxed)
}
pub(crate) fn sheet_state(tab: &crate::workspace::Tab) -> Option<vel_session::SettingsSheet> {
    let label = tab
        .editor
        .settings
        .get("velocity_settings_label")?
        .as_str()?
        .to_owned();
    Some(vel_session::SettingsSheet {
        resource: tab
            .editor
            .settings
            .get("velocity_settings_resource")
            .and_then(Value::as_str)
            .map(str::to_owned),
        label,
    })
}
pub(crate) fn restore_settings_tab(
    tab: &mut crate::workspace::Tab,
    state: &vel_session::SettingsSheet,
) {
    tab.editor.read_only = state.resource.is_some();
    if let Some(resource) = &state.resource {
        tab.editor
            .settings
            .set(Layer::View, "velocity_settings_resource", json!(resource));
        tab.suggested_name = Some(state.label.clone());
    }
    tab.editor
        .settings
        .set(Layer::View, "velocity_settings_label", json!(state.label));
    // The existing view-title override also names resource-backed scratch views.
    tab.editor
        .settings
        .set(Layer::View, "velocity_html_sheet_title", json!(state.label));
    tab.editor.settings.set(
        Layer::View,
        "syntax",
        json!("Packages/JSON/JSON.sublime-syntax"),
    );
}

pub fn remember(list: &mut Vec<PathBuf>, path: PathBuf) {
    list.retain(|p| *p != path);
    list.insert(0, path);
    list.truncate(100);
}
fn direction(args: &Value) -> Direction {
    match args["direction"].as_str().unwrap_or("right") {
        "left" => Direction::Left,
        "up" => Direction::Up,
        "down" => Direction::Down,
        _ => Direction::Right,
    }
}
/// Per-window UI overrides, remembered separately for distraction-free mode.
#[derive(Debug, Default, Clone)]
pub struct WindowLayers {
    pub normal: BTreeMap<String, bool>,
    pub distraction: BTreeMap<String, bool>,
}
/// The sole tab-layer installation path, also called by `Tab::new`.
/// The window's effective layer is published in its Settings, before tab creation.
pub(crate) fn register_commands(registry: &mut vel_commands::Registry) {
    for name in [
        "edit_settings",
        "edit_syntax_settings",
        "open_file_settings",
        "customize_color_scheme",
        "customize_theme",
        "open_dir",
        "set_max_columns",
        "close_by_index",
        "close_others_by_index",
        "close_to_right_by_index",
        "close_unmodified",
        "close_deleted_files",
        "close_selected",
        // Close All's public command name: https://forum.sublimetext.com/t/close-all/3841
        "close_all",
        "split_view",
    ] {
        registry.register(name, vel_commands::Kind::WindowCommand, None);
    }
}
pub(crate) fn apply_window_layers(
    tab: &mut Tab,
    window: &vel_config::settings::Settings,
) -> Result<(), String> {
    let mut changed = false;
    let mut view = tab.editor.settings.layer(Layer::View).clone();
    let before = view.len();
    for key in [
        "show_minimap",
        "show_side_bar",
        "show_tabs",
        "show_status_bar",
    ] {
        view.remove(key);
    }
    if view.len() != before {
        tab.editor
            .settings
            .replace(Layer::View, Value::Object(view))?;
        changed = true;
    }
    let values = window.layer(Layer::DistractionFree);
    if tab.editor.settings.layer(Layer::DistractionFree) != values {
        tab.editor
            .settings
            .replace(Layer::DistractionFree, Value::Object(values.clone()))?;
        changed = true;
    }
    // Identical layers must preserve Settings' cache identity across clones: changing
    // it here repeatedly invalidates their shared spelling/highlighting publications.
    if changed {
        tab.editor.sync();
    }

    Ok(())
}
impl Workspace {
    /// The owning buffer's dirty state, rather than an unsynchronized clone's.
    pub(crate) fn document_dirty(&self, tab: &Tab) -> bool {
        self.tabs
            .iter()
            .find(|t| t.editor.buffer.id() == tab.document)
            .unwrap_or(tab)
            .dirty()
    }
    /// Contents/format dirtiness for Close Deleted, independent of the deletion marker.
    pub(crate) fn document_contents_dirty(&self, tab: &Tab) -> bool {
        if tab.load_failed {
            return false;
        }
        let owner = self
            .tabs
            .iter()
            .find(|t| t.editor.buffer.id() == tab.document)
            .unwrap_or(tab);
        owner.editor.buffer.is_dirty()
            || owner.restored_format_dirty
            || owner.lazy.as_ref().is_some_and(|(doc, _)| doc.dirty)
            || owner.format != owner.saved_format
    }
    /// Commits a window preference through the one shared, atomic preferences writer.
    pub fn set_user_preference(&mut self, key: &str, value: Value) -> Result<(), String> {
        self.settings.set(Layer::User, key, value.clone());
        for tab in &mut self.tabs {
            tab.editor.settings.set(Layer::User, key, value.clone());
            let mut view = tab.editor.settings.layer(Layer::View).clone();
            view.remove(key);
            tab.editor
                .settings
                .replace(Layer::View, Value::Object(view))?;
            tab.editor.sync();
        }
        if let Some(data) = &self.data_dir {
            let path = data.join("Packages/User/Preferences.sublime-settings");
            self.settings_reports.claim(&path);
            let key = key.to_owned();
            let pending = value.clone();
            let tracked = key.clone();
            let rx = crate::release::enqueue(move || {
                let source = if vel_fileio::regular::metadata(&path, true)?.is_some() {
                    use std::io::Read;
                    let mut source = String::new();
                    vel_fileio::regular::read(&path)?
                        .read_to_string(&mut source)
                        .map_err(|e| format!("{}: {e}", path.display()))?;
                    source
                } else {
                    "{}".into()
                };
                let source = vel_config::json::Document::parse(&source)
                    .and_then(|doc| doc.set(&key, &value))
                    .map_err(|e| {
                        vel_config::json::source_diagnostic(&path.to_string_lossy(), &e)
                    })?;
                vel_session::atomic_write(&path, |file| {
                    use std::io::Write;
                    file.write_all(source.as_bytes()).map_err(|e| e.to_string())
                })
                .map_err(|e| format!("{}: {e}", path.display()))
            })?;
            self.track_settings_write(rx, &[(tracked, Some(pending))]);
        }
        Ok(())
    }
    /// Converts a preview to an ordinary tab and records it in recent files once.
    pub fn make_permanent(&mut self, index: usize) {
        if self.tabs[index].transient || self.tabs[index].preview_recent_pending {
            self.tabs[index].transient = false;
            self.tabs[index].preview_recent_pending = false;
            if let Some(path) = self.tabs[index].path.clone() {
                remember(&mut self.recent_files, path);
            }
        }
    }
    /// Visit the next existing sibling from a caller-supplied extension cycle.
    pub fn switch_file(&mut self, extensions: &Value) -> Result<(), String> {
        let mut extensions = if let Some(values) = extensions.as_array() {
            values.iter().filter_map(Value::as_str).collect::<Vec<_>>()
        } else if let Some(values) = extensions.as_str() {
            values
                .split(|ch: char| ch == ',' || ch.is_whitespace())
                .collect()
        } else {
            return Err("switch_file requires extensions".into());
        };
        for ext in &mut extensions {
            *ext = ext.trim_start_matches('.');
        }
        extensions.retain(|ext| !ext.is_empty() && !ext.contains(['/', '\\']));
        let Some(path) = self.tabs[self.active].path.as_ref() else {
            return Ok(());
        };
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return Ok(());
        };
        // ST4 supports compound extensions (https://www.sublimetext.com/download).
        // Match the longest listed extension; replacing "cc" alone keeps "foo.pb".
        let Some((start, stem)) = extensions
            .iter()
            .enumerate()
            .filter_map(|(i, ext)| {
                name.strip_suffix(ext)
                    .and_then(|base| base.strip_suffix('.'))
                    .map(|base| (i, base))
            })
            .min_by_key(|(_, stem)| stem.len())
        else {
            return Ok(());
        };
        let next = (1..extensions.len())
            .map(|step| extensions[(start + step) % extensions.len()])
            .map(|ext| path.with_file_name(format!("{stem}.{ext}")))
            .find(|candidate| candidate != path && candidate.is_file());
        if let Some(path) = next {
            self.open(crate::cli::FileLocation {
                path,
                line: 1,
                column: 1,
            })?;
        }
        Ok(())
    }
    /// Active title with explicit full-path/project-relative preferences.
    pub fn window_title(&self) -> String {
        // The reference describes show_full_path; the ST4 changelog adds show_rel_path.
        // The detailed reference establishes false on macOS (true on other platforms).
        // https://sublime-text-unofficial-documentation.readthedocs.io/en/latest/reference/settings.html#global-settings
        // https://docs.sublimetext.io/reference/settings.html#global-settings
        // https://www.sublimetext.com/download
        let tab = &self.tabs[self.active];
        let settings = &tab.editor.settings;
        let name = if self.is_empty() {
            "Velocity".into()
        } else if settings.bool("show_rel_path", false) {
            tab.path
                .as_deref()
                .and_then(|path| {
                    self.folders
                        .iter()
                        .filter_map(|f| path.strip_prefix(&f.path).ok())
                        .min_by_key(|p| p.components().count())
                })
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| {
                    if settings.bool("show_full_path", false) {
                        tab.path
                            .as_ref()
                            .map_or_else(|| tab.name(), |p| p.to_string_lossy().into_owned())
                    } else {
                        tab.name()
                    }
                })
        } else if settings.bool("show_full_path", false) {
            tab.path
                .as_ref()
                .map_or_else(|| tab.name(), |p| p.to_string_lossy().into_owned())
        } else {
            tab.name()
        };
        let title = format!(
            "{name}{}{}",
            if tab.deleted { " [deleted]" } else { "" },
            if tab.dirty() { " •" } else { "" }
        );
        self.project
            .as_deref()
            .and_then(Path::file_stem)
            .map_or_else(
                || title.clone(),
                |project| format!("{title} — {}", project.to_string_lossy()),
            )
    }
    /// Zoom is a user Preference shared by every tab, rather than a view override.
    pub fn zoom_font(&mut self, command: &str) -> Result<(), String> {
        // Zoom changes the user size. An explicit distraction-free override remains
        // authoritative in that mode; leaving the mode reveals the adjusted user size.
        let mut user_settings = self.settings.clone();
        user_settings.replace(Layer::DistractionFree, json!({}))?;
        user_settings.replace(Layer::View, json!({}))?;
        user_settings.replace(Layer::Project, json!({}))?;
        user_settings.replace(Layer::Syntax, json!({}))?;
        let size = user_settings.number("font_size", 13.);
        // Reset removes the user key unless default_font_size is supplied (build 4200).
        // https://docs.sublimetext.io/reference/commands.html#reset-font-size
        // https://www.sublimetext.com/download
        let value = match command {
            "reset_font_size" => user_settings
                .get("default_font_size")
                .and_then(Value::as_f64)
                .filter(|s| s.is_finite())
                .map(|s| json!(s.clamp(6., 96.))),
            "increase_font_size" => Some(json!((size + 1.).clamp(6., 96.))),
            _ => Some(json!((size - 1.).clamp(6., 96.))),
        };
        let mut values = self.settings.layer(Layer::User).clone();
        if let Some(value) = &value {
            values.insert("font_size".into(), value.clone());
        } else {
            values.remove("font_size");
        }
        self.settings
            .replace(Layer::User, Value::Object(values.clone()))?;
        for tab in &mut self.tabs {
            tab.editor
                .settings
                .replace(Layer::User, Value::Object(values.clone()))?;
            tab.editor.sync();
        }
        if let Some(data) = &self.data_dir {
            let path = data.join("Packages/User/Preferences.sublime-settings");
            self.settings_reports.claim(&path);
            let pending_value = value.clone();
            // Settings IO runs away from input/scroll; serialize zoom writes in enqueue order.
            let rx = crate::release::enqueue(move || {
                let source = match std::fs::read_to_string(&path) {
                    Ok(s) => s,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{}".into(),
                    Err(e) => return Err(format!("{}: {e}", path.display())),
                };
                let document = vel_config::json::Document::parse(&source).map_err(|e| {
                    vel_config::json::source_diagnostic(&path.to_string_lossy(), &e)
                })?;
                let source = if let Some(value) = value {
                    document.set("font_size", &value)?
                } else {
                    remove_font_size(&document)?
                };
                vel_session::atomic_write(&path, |file| {
                    use std::io::Write;
                    file.write_all(source.as_bytes()).map_err(|e| e.to_string())
                })
                .map_err(|e| format!("{}: {e}", path.display()))
            })?;
            self.track_settings_write(rx, &[("font_size".into(), pending_value)]);
        }
        Ok(())
    }
    pub fn refresh_window_layers(&mut self) -> Result<(), String> {
        let mut values = if self.distraction_free {
            let mut values = self.distraction_settings.clone();
            for key in [
                "show_minimap",
                "show_side_bar",
                "show_tabs",
                "show_status_bar",
            ] {
                values.entry(key.to_owned()).or_insert(json!(false));
            }
            values
        } else {
            serde_json::Map::new()
        };
        // UI state survives mode switches: https://www.sublimetext.com/docs/distraction_free.html
        let overrides = if self.distraction_free {
            &self.window_layers.distraction
        } else {
            &self.window_layers.normal
        };
        values.extend(overrides.iter().map(|(k, v)| (k.clone(), json!(v))));
        self.settings
            .replace(Layer::DistractionFree, Value::Object(values))?;
        for tab in &mut self.tabs {
            apply_window_layers(tab, &self.settings)?;
        }
        Ok(())
    }
    pub fn set_window_toggle(&mut self, key: &str, value: bool) -> Result<(), String> {
        let values = if self.distraction_free {
            &mut self.window_layers.distraction
        } else {
            &mut self.window_layers.normal
        };
        values.insert(key.into(), value);
        self.refresh_window_layers()
    }
    pub fn window_visible(&self, key: &str) -> bool {
        self.settings.bool(key, true)
    }
    pub fn sheet_index(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.sheet == id)
    }
    pub fn focus_current_group(&mut self) -> Result<(), String> {
        let group = &self.workbench.groups[self.workbench.active];
        if let Some(index) = group.focused.and_then(|id| self.sheet_index(id)) {
            self.prepare_sheet(index)?;
            self.active = index;
        } else if !self.empty {
            // Explicitly focusing an unused pane must not keep editing another pane's
            // document. Preserve its existing on-demand new-file behavior, while an
            // entirely empty window keeps its hidden input adapter without a tab.
            self.new_file()?;
        }
        Ok(())
    }
    pub fn click_sheet(&mut self, index: usize, toggle: bool, extend: bool) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let id = tab.sheet;
        if let Some(group) = self.workbench.group_of(id) {
            self.workbench.active = group;
            self.workbench.groups[group].click(id, toggle, extend);
            if let Err(e) = self.focus_current_group() {
                self.messages.push(e);
            }
        }
    }
    pub fn activate_sheet(&mut self, index: usize) {
        if let Some(group) = self.workbench.group_of(self.tabs[index].sheet) {
            self.workbench.active = group;
            self.workbench.groups[group].click(self.tabs[index].sheet, false, false);
        }
        if let Err(e) = self.prepare_sheet(index) {
            self.messages.push(e);
        }
        self.active = index;
    }
    /// Transfers the actual shared Buffer between editor adapters. Inactive clones retain
    /// read-only rope snapshots; all edits and visible scenes use this actual buffer/view.
    pub fn prepare_sheet(&mut self, index: usize) -> Result<(), String> {
        let result = if self.tabs.get(index).is_some_and(|t| t.lazy.is_some())
            && !self.restore_in_progress
        {
            match vel_crash::recoverable(|| {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    self.prepare_sheet_inner(index)
                }))
            }) {
                Ok(result) => result,
                Err(_) => {
                    self.report_restore_failure(None, &[], "lazy document restore panicked");
                    Err("lazy document restore failed; recovery saved in the status report".into())
                }
            }
        } else {
            self.prepare_sheet_inner(index)
        };
        match result {
            Err(error) if self.tabs.get(index).is_some() => {
                if error.contains(crate::restore_io::TIMEOUT) {
                    if self.restore_in_progress {
                        return Err(error);
                    }
                    self.report_restore_failure(None, &[], &error);
                }
                let message = self.tabs[index].path.as_deref().map_or_else(
                    || error.clone(),
                    |path| vel_fileio::regular::error(path, &error),
                );
                self.fail_shared_document(index)?;
                // A read failure is retryable. Register the path even when the
                // first restore failed before reaching the normal watch boundary.
                if let Some(watcher) = &mut self.watcher
                    && let Some(path) = &self.tabs[index].path
                {
                    let _ = watcher.watch(path);
                }
                // The error document is ready for normal focus/close operations.
                // A load error must not trap a tab or turn into a modal dialog.
                self.resource_reports.push(message);
                Ok(())
            }
            result => result,
        }
    }
    fn prepare_sheet_inner(&mut self, index: usize) -> Result<(), String> {
        if index >= self.tabs.len() {
            return Err("unknown sheet".into());
        }
        self.require_editor_view(index)?;
        if let Some((document, state)) = self.tabs[index].lazy.clone() {
            state.validate()?;
            let identity = self.tabs[index].session_document;
            let owner = self
                .tabs
                .iter()
                .enumerate()
                .find(|(i, t)| {
                    *i != index
                        && t.session_document == identity
                        && t.lazy.is_none()
                        && t.editor.buffer.id() == t.document
                })
                .map(|(i, _)| i);
            if let Some(owner) = owner {
                crate::session_restore::step("attach_lazy");
                let view = self.ensure_shared_view(owner, index)?;
                self.copy_document_properties(owner, index);
                if (self.tabs[owner].loading.is_some() && !self.tabs[owner].external_load)
                    || self.tabs[owner].binary_loading.is_some()
                    || self.tabs[owner].load_failed
                {
                    self.tabs[index].pending_restore = Some(state);
                }
                self.tabs[index].lazy = None;
                self.tabs[index].shared_view = view;
                self.tabs[index].editor.shared_shadow = true;
                self.prepare_sheet(index)?;
                self.sync_shared();
                return Ok(());
            }
            let deadline = self
                .restore_deadline
                .unwrap_or_else(|| Instant::now() + crate::restore_io::BUDGET);
            let retained = document.history.as_ref().is_some_and(|h| h.retains_text());
            self.tabs[index].restore_history =
                document.history.clone().map(|history| (history, false));
            self.tabs[index].pending_restore = Some(state.clone());
            // Retain native proof before a metadata failure can leave a watched
            // placeholder. Its later reload must still recognize the old codec.
            let legacy = document.disk_content.is_none()
                && (document.effective_encoding.is_none() || document.history.is_some());
            if legacy {
                let history = document.history.clone();
                let (saved, fingerprint) = crate::restore_io::run(deadline, move || {
                    Ok(history.as_ref().map_or((None, None), |history| {
                        (
                            history.saved_text(),
                            (!history.retains_text())
                                .then(|| history.text_fingerprint())
                                .flatten(),
                        )
                    }))
                })?;
                let tab = &mut self.tabs[index];
                tab.legacy_saved_text = saved;
                tab.legacy_text_fingerprint = fingerprint;
            }
            let disk_path = document.path.clone();
            let (disk, size, stamp) = if let Some(path) = disk_path {
                crate::restore_io::run(deadline, move || {
                    // Retained edits never require readable disk text; clean loads
                    // reject all special files through the common guarded boundary.
                    let metadata = vel_fileio::regular::metadata(&path, false);
                    let size = if retained {
                        metadata.ok().flatten().map_or(0, |m| m.len())
                    } else {
                        metadata?.ok_or("missing document")?.len()
                    };
                    Ok((
                        vel_session::DiskStamp::read(&path),
                        size,
                        crate::workspace::stamp(&path),
                    ))
                })?
            } else {
                (None, 0, None)
            };
            let changed = document.path.is_some() && disk != document.disk;
            if legacy {
                self.tabs[index].legacy_stamp_matches = !changed;
            }
            let mut imported = false;
            if let Some(history) = document.history.as_ref().filter(|h| h.retains_text()) {
                let dirty = document.dirty || changed;
                crate::session_restore::step("restore_history");
                let history = history.clone();
                let path = document.path.clone();
                let options = self.tabs[index].decode_options();
                let original_size = document.disk.as_ref().map(|s| s.bytes);
                let (mut buffer, map, kept) = crate::restore_io::run(deadline, move || {
                    let keep = if !changed {
                        true
                    } else if size <= 8192 {
                        path.as_deref().is_none_or(|path| {
                            vel_fileio::revision_unchanged(path, &options).unwrap_or(true)
                        })
                    } else {
                        original_size.is_none_or(|bytes| bytes == size)
                    };
                    let restored = history.restore_views(keep, dirty);
                    let kept = restored.is_ok() && keep;
                    let (buffer, map) =
                        restored.or_else(|_| history.restore_views(false, dirty))?;
                    Ok((buffer, map, kept))
                })?;
                let stored_view = state.history_view.and_then(|id| map.get(&id).copied());
                imported = kept && stored_view.is_some();
                let id = stored_view.unwrap_or_else(|| buffer.new_view());
                self.tabs[index].editor.buffer = buffer;
                self.tabs[index].editor.id = id;
                // Every clone, including lazy sheets, must keep its original edit ownership.
                for sibling in &mut self.tabs {
                    if sibling.session_document == identity
                        && let Some((_, view)) = &sibling.lazy
                        && let Some(id) = view.history_view.and_then(|old| map.get(&old))
                    {
                        sibling.shared_view = *id;
                    }
                }
                self.require_editor_view(index)?;
                let tab = &mut self.tabs[index];
                // Native history has now been installed. Its original view IDs
                // were mapped above; apply each view's folds/selection once below.
                tab.restore_history = None;
                tab.pending_restore = None;
                tab.editor.sync();
                if changed && let Some(path) = document.path.clone() {
                    let options = tab.decode_options();
                    match crate::restore_io::run(deadline, move || {
                        crate::workspace::prepare_disk_load(&path, options, true, true)
                    }) {
                        Ok(prepared) => tab.install_disk_load(prepared, true),
                        Err(error) if error.contains(crate::restore_io::TIMEOUT) => {
                            return Err(error);
                        }
                        Err(error) => tab.load_warning = Some(error),
                    }
                }
            } else if let Some(path) = &document.path {
                if document.history.is_some() {
                    crate::session_restore::step("restore_history");
                }
                crate::session_restore::step("restore_file");
                let tab = &mut self.tabs[index];
                tab.restore_history = document.history.clone().map(|history| (history, !changed));
                let background = size
                    >= tab
                        .editor
                        .settings
                        .usize("large_file.load_bytes", 64 * 1024 * 1024)
                        as u64;
                let options = tab.decode_options();
                let path = path.clone();
                let prepared = crate::restore_io::run(deadline, move || {
                    crate::workspace::prepare_disk_load(&path, options, background, false)
                })?;
                tab.install_disk_load(prepared, false);
            }
            let tab = &mut self.tabs[index];
            tab.lazy = None;
            tab.editor.read_only = tab.load_failed
                || tab.binary_loading.is_some()
                || (tab.loading.is_some() && !tab.external_load)
                || tab
                    .editor
                    .settings
                    .get("velocity_settings_resource")
                    .is_some();
            if tab.pending_restore.is_none() {
                if !imported {
                    state.apply(&mut tab.editor.buffer, tab.editor.id);
                }
                tab.editor.restore_folded_view_state(&state, tab.editor.id);
            }
            tab.editor.sync();
            tab.restored_format_dirty = document.dirty && !tab.editor.buffer.is_dirty();
            tab.document = tab.editor.buffer.id();
            tab.shared_view = tab.editor.id;
            tab.shared_dirty = false;
            if retained || tab.loading.is_some() || tab.load_failed {
                tab.disk_saved = document.disk.clone();
            } else if tab.disk_saved.is_none() {
                tab.disk_saved = disk;
            }
            tab.disk_stamp = stamp;
            tab.overwrite_allowed = None;
            // A lazy sheet references this live document from now on. Its history is
            // only a recovery source while *no* view has loaded the shared buffer.
            // clone_file shares one buffer: https://docs.sublimetext.io/reference/commands.html#clone-file
            let buffer = tab.document;
            for sibling in 0..self.tabs.len() {
                if self.tabs[sibling].session_document != identity {
                    continue;
                }
                self.tabs[sibling].document = buffer;
                if let Some((_, state)) = self.tabs[sibling].lazy.clone() {
                    state.validate()?;
                    let view = self.tabs[sibling].shared_view;
                    let view = if self.tabs[index].editor.buffer.views().any(|id| id == view) {
                        view
                    } else {
                        let view = self.tabs[index].editor.buffer.new_view();
                        state.apply(&mut self.tabs[index].editor.buffer, view);
                        view
                    };
                    // Restore once, before any live edits. Later lazy attachment must
                    // retain the buffer's rebased selections/folds, not replay old offsets.
                    self.tabs[index]
                        .editor
                        .restore_folded_view_state(&state, view);
                    self.tabs[sibling].shared_view = view;
                }
            }
            let tab = &mut self.tabs[index];
            if let Some(w) = self.watcher.as_mut()
                && let Some(path) = &tab.path
            {
                let _ = w.watch(path);
            }
            self.configure_language(index, None, true)?;
            self.reconcile_restored_views(index);
        }
        let doc = self.tabs[index].document;
        let owner = self.tabs.iter().position(|t| t.editor.buffer.id() == doc);
        if let Some(owner) = owner
            && owner != index
        {
            self.require_editor_view(owner)?;
            self.ensure_shared_view(owner, owner)?;
            let target_view = self.ensure_shared_view(owner, index)?;
            crate::session_restore::step("transfer_buffer");
            self.tabs[owner].editor.unmark();
            let source_view = self.tabs[owner].shared_view;
            let source_state = vel_session::ViewState::capture(
                &self.tabs[owner].editor.buffer,
                source_view,
                self.tabs[owner].scroll,
            );
            let (a, b) = if owner < index {
                let (a, b) = self.tabs.split_at_mut(index);
                (&mut a[owner], &mut b[0])
            } else {
                let (a, b) = self.tabs.split_at_mut(owner);
                (&mut b[0], &mut a[index])
            };
            b.inherit_document(a);
            a.transfer_document_io(b);
            std::mem::swap(&mut a.editor.buffer, &mut b.editor.buffer);
            a.editor.shared_shadow = true;
            b.editor.shared_shadow = false;
            a.editor.buffer = Buffer::new(b.editor.buffer.text().clone());
            a.editor.id = a.editor.buffer.new_view();
            source_state.apply(&mut a.editor.buffer, a.editor.id);
            b.editor.id = target_view;
            b.editor.adopt_document();
            b.editor.sync();
            a.editor.sync();
        }
        self.require_editor_view(index)?;
        self.tabs[index].editor.adopt_document();
        Ok(())
    }
    /// Refreshes inactive clone snapshots after commands without copying document bytes.
    pub fn sync_shared(&mut self) {
        self.materialize_input();
        let promote = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| (t.transient || t.preview_recent_pending) && t.dirty())
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        for index in promote {
            self.make_permanent(index);
        }
        let owners: Vec<_> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| t.editor.buffer.id() == t.document && t.lazy.is_none())
            .map(|(i, _)| i)
            .collect();
        for owner in owners {
            if let Err(error) = self.ensure_shared_view(owner, owner) {
                if let Err(repair) = self.fail_shared_document(owner) {
                    self.resource_reports.push(repair);
                }
                self.resource_reports.push(error);
                continue;
            }
            let doc = self.tabs[owner].document;
            let others: Vec<_> = self
                .tabs
                .iter()
                .enumerate()
                .filter(|(i, t)| *i != owner && t.document == doc)
                .map(|(i, _)| i)
                .collect();
            if others.is_empty() {
                self.tabs[owner].shared_dirty = false;
                continue;
            }
            let dirty = self.tabs[owner].editor.buffer.is_dirty();
            for index in others {
                if let Err(error) = self.ensure_shared_view(owner, index) {
                    if let Err(repair) = self.fail_shared_document(owner) {
                        self.resource_reports.push(repair);
                    }
                    self.resource_reports.push(error);
                    break;
                }
                let state = vel_session::ViewState::capture(
                    &self.tabs[owner].editor.buffer,
                    self.tabs[index].shared_view,
                    self.tabs[index].scroll,
                );
                let text = self.tabs[owner].editor.buffer.text().clone();
                let version = self.tabs[owner].editor.buffer.change_id().0;
                if self.tabs[index].editor.revision != version
                    || self.tabs[index].editor.text != text
                {
                    let mut placeholder = Buffer::new(text);
                    let id = placeholder.new_view();
                    state.apply(&mut placeholder, id);
                    self.tabs[index].editor.buffer = placeholder;
                    self.tabs[index].editor.id = id;
                    self.tabs[index].editor.shared_shadow = true;
                    self.tabs[index].editor.sync();
                    self.tabs[index].editor.revision = version;
                }
                if owner < index {
                    let (a, b) = self.tabs.split_at_mut(index);
                    b[0].inherit_document_mode(&a[owner]);
                } else {
                    let (a, b) = self.tabs.split_at_mut(owner);
                    a[index].inherit_document_mode(&b[0]);
                }
                self.tabs[index].shared_dirty = dirty;
                self.tabs[index].deleted = self.tabs[owner].deleted;
                self.tabs[index].path = self.tabs[owner].path.clone();
                self.tabs[index].format = self.tabs[owner].format.clone();
                self.tabs[index].saved_format = self.tabs[owner].saved_format.clone();
                self.tabs[index].load_failed = self.tabs[owner].load_failed;
                self.tabs[index].legacy_stamp_matches = self.tabs[owner].legacy_stamp_matches;
                self.tabs[index].legacy_saved_text = self.tabs[owner].legacy_saved_text.clone();
                self.tabs[index].legacy_text_fingerprint =
                    self.tabs[owner].legacy_text_fingerprint.clone();
                self.tabs[index].encoding_chosen = self.tabs[owner].encoding_chosen;
                self.tabs[index].preferred_encoding = self.tabs[owner].preferred_encoding.clone();
                self.tabs[index].tracker = self.tabs[owner].tracker.clone();
                self.tabs[index].disk_saved = self.tabs[owner].disk_saved.clone();
                self.tabs[index].disk_content = self.tabs[owner].disk_content;
                self.tabs[index].saved_in_effective_encoding =
                    self.tabs[owner].saved_in_effective_encoding;
                self.tabs[index].overwrite_allowed = self.tabs[owner].overwrite_allowed;
                self.tabs[index].disk_stamp = self.tabs[owner].disk_stamp;
                self.tabs[index].restored_format_dirty = self.tabs[owner].restored_format_dirty;
            }
            self.tabs[owner].shared_dirty = false;
        }
    }
    fn reconcile_restored_views(&mut self, index: usize) {
        let views = std::mem::take(&mut self.tabs[index].restored_history_views);
        if views.is_empty() {
            return;
        }
        let document = self.tabs[index].restored_document;
        for sibling in &mut self.tabs {
            if sibling.restored_document == document
                && let Some((_, state)) = &sibling.lazy
                && let Some(view) = state.history_view.and_then(|old| views.get(&old))
            {
                sibling.shared_view = *view;
            }
        }
    }
    pub fn reconcile_installs(&mut self) {
        for index in 0..self.tabs.len() {
            self.reconcile_restored_views(index);
            let Some(old) = self.tabs[index].previous_document.take() else {
                continue;
            };
            if let Err(error) = self.require_editor_view(index) {
                let _ = self.fail_shared_document(index);
                self.resource_reports.push(error);
                continue;
            }
            let document = self.tabs[index].document;
            self.tabs[index].editor.layout_edits.borrow_mut().clear();
            for other in 0..self.tabs.len() {
                if other == index || self.tabs[other].document != old {
                    continue;
                }
                let state = self.saved_sheet_state(other);
                if state.validate().is_err() {
                    let _ = self.fail_shared_document(index);
                    break;
                }
                let view = self.tabs[index].editor.buffer.new_view();
                state.apply(&mut self.tabs[index].editor.buffer, view);
                if self.tabs[index].loading.is_none() && self.tabs[index].binary_loading.is_none() {
                    self.tabs[other].pending_restore = None;
                }
                self.copy_document_properties(index, other);
                self.tabs[other].document = document;
                self.tabs[other].shared_view = view;
                self.tabs[other].editor.shared_shadow = true;
                self.tabs[other].editor.language = self.tabs[index].editor.language.clone();
                self.tabs[other].editor.layout_edits = self.tabs[index].editor.layout_edits.clone();
                self.tabs[other].editor.annotations = self.tabs[index].editor.annotations.clone();
            }
        }
        self.sync_shared();
    }
    pub fn clone_file(&mut self) -> Result<(), String> {
        if self.empty {
            return self.new_file();
        }
        if self.tabs[self.active].loading.is_some()
            || self.tabs[self.active].binary_loading.is_some()
        {
            return Err("Wait for loading to finish before cloning this tab".into());
        }
        self.prepare_sheet(self.active)?;
        let index = self.active;
        let id = self.tabs[index].editor.buffer.new_view();
        let state = vel_session::ViewState::capture(
            &self.tabs[index].editor.buffer,
            self.tabs[index].editor.id,
            self.tabs[index].scroll,
        );
        state.apply(&mut self.tabs[index].editor.buffer, id);
        let settings = self.tabs[index]
            .editor
            .buffer
            .view(self.tabs[index].editor.id)
            .settings
            .clone();
        self.tabs[index].editor.buffer.set_settings(id, settings);
        let mut clone = Tab::new(
            self.tabs[index].editor.text.clone(),
            &self.tabs[index].editor.settings,
            &self.keymap,
        )?;
        apply_window_layers(&mut clone, &self.settings)?;
        clone.inherit_document(&self.tabs[index]);
        clone.shared_view = id;
        clone.editor.shared_shadow = true;
        clone.editor.layout_version = self.tabs[index].editor.layout_version;
        clone.scroll = self.tabs[index].scroll;
        clone.scroll_line_height = self.tabs[index].scroll_line_height;
        let sheet = clone.sheet;
        self.tabs.push(clone);
        self.workbench.groups[self.workbench.active].add(sheet, false);
        self.prepare_sheet(self.tabs.len() - 1)?;
        self.active = self.tabs.len() - 1;
        self.sync_shared();
        Ok(())
    }
    /// User-facing Split View: adjacent shared view selected alongside its source.
    pub fn split_view(&mut self) -> Result<(), String> {
        if self.empty {
            return self.new_file();
        }
        let source = self.tabs[self.active].sheet;
        let group = &self.workbench.groups[self.workbench.active];
        let at = group
            .sheets
            .iter()
            .position(|id| *id == source)
            .ok_or("source tab missing")?
            + 1;
        self.clone_file()?;
        let sheet = self.tabs[self.active].sheet;
        // File > Split View uses tab multiselect (build 4107):
        // https://www.sublimetext.com/docs/tab_multi-select.html
        // https://www.sublimetext.com/download
        let group = &mut self.workbench.groups[self.workbench.active];
        group.sheets.retain(|id| *id != sheet);
        group.sheets.insert(at, sheet);
        group.selected.clear();
        group.selected.extend([source, sheet]);
        group.focused = Some(sheet);
        group.anchor = Some(source);
        Ok(())
    }
    pub fn prepare_close_sheet(&mut self, index: usize) -> Result<(), String> {
        self.prepare_sheet(index)?;
        let doc = self.tabs[index].session_document;
        let other = self
            .tabs
            .iter()
            .enumerate()
            .find(|(i, t)| *i != index && t.session_document == doc)
            .map(|(i, _)| i);
        let view = self.tabs[index].shared_view;
        if let Some(other) = other {
            self.prepare_sheet(other)?;
            self.tabs[other].editor.buffer.close_view(view);
        }
        Ok(())
    }
    pub(crate) fn split_workbench(
        &self,
        args: &Value,
    ) -> Result<vel_ui::workbench::Workbench, String> {
        let active = self.workbench.active;
        let cell = self.workbench.layout.cells[active];
        let columns = self
            .workbench
            .layout
            .cells
            .iter()
            .filter(|c| c[1] <= cell[1] && c[3] > cell[1])
            .count();
        let vertical = args["direction"]
            .as_str()
            .map_or(columns < self.workbench.max_columns, |direction| {
                !matches!(direction, "up" | "down")
            });
        let mut next = self.workbench.clone();
        next.new_pane(vertical)?;
        let bounds = vel_platform::Rect {
            x: 0.,
            y: 0.,
            width: self.pane_extent.0,
            height: self.pane_extent.1,
        };
        if (0..next.groups.len())
            .filter_map(|i| next.layout.rect(i, bounds))
            .any(|r| r.width + 0.01 < self.pane_minimum.0 || r.height + 0.01 < self.pane_minimum.1)
        {
            return Err("Pane is too small to split further".into());
        }
        Ok(next)
    }
    pub fn workbench_action(&mut self, name: &str, args: &Value) -> Result<bool, String> {
        match name {
            "new_html_sheet" => {
                let content = args["content"].as_str().unwrap_or("");
                vel_ui::minihtml::Html::new(content, false).map_err(|e| e.to_string())?;
                self.new_file()?;
                let editor = self.editor_mut();
                editor.settings.set(
                    vel_config::settings::Layer::View,
                    "velocity_html_sheet",
                    json!(content),
                );
                editor.settings.set(
                    vel_config::settings::Layer::View,
                    "velocity_html_sheet_title",
                    json!(args["name"].as_str().unwrap_or("HTML")),
                );
                editor.read_only = true;
            }
            "set_theme_appearance" => {
                self.set_appearance(args["dark"].as_bool().unwrap_or(!self.dark))?;
            }
            "theme_demo" => {
                crate::theme_bench::demo(self)?;
            }
            "set_layout" => {
                self.workbench.set_layout(menu_layout(args)?)?;
                self.focus_current_group()?;
            }
            "focus_group" | "move_to_group" => {
                let group = args["group"].as_u64().ok_or("group index required")? as usize;
                if group >= self.workbench.groups.len() {
                    return Err("unknown group".into());
                }
                if name == "move_to_group" {
                    self.workbench.move_active(group)?;
                } else {
                    self.workbench.active = group;
                }
                self.focus_current_group()?;
            }
            "focus_neighboring_group" | "move_to_neighboring_group" => {
                if let Some(group) = self
                    .workbench
                    .layout
                    .neighbor(self.workbench.active, direction(args))
                {
                    if name == "move_to_neighboring_group" {
                        self.workbench.move_active(group)?;
                    } else {
                        self.workbench.active = group;
                    }
                    self.focus_current_group()?;
                }
            }
            "set_max_columns" => {
                let columns = args["columns"].as_u64().ok_or("columns required")? as usize;
                if !(1..=64).contains(&columns) {
                    return Err("columns must be 1..64".into());
                }
                self.workbench.max_columns = columns;
            }
            "new_pane" => {
                // Build 4126 confirms that new panes move all selected sheets:
                // https://www.sublimetext.com/download. The direction and limit
                // implement the round-19 window contract, without pixel state.
                let active = self.workbench.active;
                self.workbench = self.split_workbench(args)?;
                let last = self.workbench.groups.len() - 1;
                if matches!(args["direction"].as_str(), Some("left" | "up")) {
                    self.workbench.layout.cells.swap(active, last);
                }
                if args["move"].as_bool().unwrap_or(true) {
                    self.workbench.move_selected(last)?;
                    self.focus_current_group()?;
                }
            }
            "close_pane" | "close_split" => {
                if self.workbench.groups.len() > 1 {
                    self.workbench.close_pane()?;
                    self.focus_current_group()?;
                }
            }
            // clone_file selects the new view alone; Split View uses multi-select.
            // https://docs.sublimetext.io/reference/commands.html#clone-file
            // https://www.sublimetext.com/download (File/Split View)
            "clone_file" => self.clone_file()?,
            "split_view" => self.split_view()?,
            "select_all_tabs" => {
                let g = &mut self.workbench.groups[self.workbench.active];
                g.selected.extend(&g.sheets);
            }
            "select_single_tab" => {
                let id = self.tabs[self.active].sheet;
                self.workbench.groups[self.workbench.active].click(id, false, false);
            }
            "select_tab" | "select_tab_to_left" | "select_tab_to_right" | "focus_selected_tab" => {
                let g = &mut self.workbench.groups[self.workbench.active];
                let ids = if name == "focus_selected_tab" {
                    g.visible()
                } else {
                    g.sheets.clone()
                };
                if !ids.is_empty() {
                    let current = g
                        .focused
                        .and_then(|id| ids.iter().position(|s| *s == id))
                        .unwrap_or(0);
                    let forward =
                        name == "select_tab_to_right" || args["forward"].as_bool().unwrap_or(false);
                    let next = (current as isize + if forward { 1 } else { -1 })
                        .rem_euclid(ids.len() as isize) as usize;
                    if name == "focus_selected_tab" {
                        g.focused = Some(ids[next]);
                    } else {
                        g.click(ids[next], true, false);
                    }
                    self.focus_current_group()?;
                }
            }
            "toggle_distraction_free" => {
                self.distraction_free = !self.distraction_free;
                self.refresh_window_layers()?;
            }
            "set_build_system" => {
                self.build_system = args["name"]
                    .as_str()
                    .filter(|s| *s != "Automatic")
                    .map(str::to_owned)
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
    pub fn snapshot(&mut self) -> Result<(Window, Vec<Document>), String> {
        let ids = if self.empty {
            BTreeSet::new()
        } else {
            self.tabs.iter().map(|tab| tab.sheet).collect()
        };
        if self.workbench.normalize(&ids) {
            eprintln!("Repaired window workbench before checkpoint");
        }
        self.materialize_input();
        self.sync_shared();
        let active = self.active;
        let mut documents = Vec::new();
        let mut sheets = Vec::new();
        let mut seen = BTreeSet::new();
        let assigned: BTreeSet<_> = self
            .workbench
            .groups
            .iter()
            .flat_map(|group| group.sheets.iter().copied())
            .collect();
        for index in 0..if self.empty { 0 } else { self.tabs.len() } {
            let tab = &self.tabs[index];
            // A preview has no committed tab order until editing/save/double click.
            // https://docs.sublimetext.io/reference/settings.html#global-settings
            if tab.transient || tab.preview_recent_pending || !assigned.contains(&tab.sheet) {
                continue;
            }
            let doc = tab.document;
            let identity = tab.session_document;
            if let Some((document, view)) = &tab.lazy {
                let loaded = self.tabs.iter().find(|t| {
                    t.session_document == identity
                        && t.lazy.is_none()
                        && t.editor.buffer.id() == t.document
                });
                let identity = tab.session_document;
                if loaded.is_none() && seen.insert(identity) {
                    let mut document = document.clone();
                    document.id = identity;
                    documents.push(document);
                }
                sheets.push(Sheet {
                    settings: sheet_state(tab),
                    html: tab
                        .editor
                        .settings
                        .get("velocity_html_sheet")
                        .and_then(Value::as_str)
                        .map(|contents| vel_session::HtmlSheet {
                            name: tab
                                .editor
                                .settings
                                .string("velocity_html_sheet_title", "HTML")
                                .into(),
                            contents: contents.into(),
                        }),
                    id: tab.sheet,
                    document: identity,
                    view: loaded
                        .filter(|owner| !owner.load_failed)
                        .map(|owner| {
                            owner
                                .editor
                                .folded_view_state(tab.shared_view, tab.scroll)
                                .with_line_height(tab.scroll_line_height)
                        })
                        .unwrap_or_else(|| view.clone()),
                    syntax: tab
                        .editor
                        .settings
                        .get("syntax")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    transient: tab.transient,
                });
                continue;
            }
            let owner = self
                .tabs
                .iter()
                .position(|t| t.editor.buffer.id() == doc)
                .unwrap_or(index);
            let view = if self.tabs[owner].load_failed && self.tabs[owner].restore_history.is_some()
            {
                self.saved_sheet_state(index)
            } else {
                self.tabs[owner]
                    .editor
                    .folded_view_state(tab.shared_view, tab.scroll)
                    .with_line_height(tab.scroll_line_height)
            };
            sheets.push(Sheet {
                settings: sheet_state(tab),
                html: tab
                    .editor
                    .settings
                    .get("velocity_html_sheet")
                    .and_then(Value::as_str)
                    .map(|contents| vel_session::HtmlSheet {
                        name: tab
                            .editor
                            .settings
                            .string("velocity_html_sheet_title", "HTML")
                            .into(),
                        contents: contents.into(),
                    }),
                id: tab.sheet,
                document: identity,
                view,
                syntax: tab
                    .editor
                    .settings
                    .get("syntax")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                transient: tab.transient,
            });
            if seen.insert(identity) {
                let owner = &mut self.tabs[owner];
                let empty_error =
                    owner.load_failed && !owner.shared_dirty && !owner.editor.buffer.is_dirty();
                let retained_dirty =
                    owner.dirty() || (owner.load_failed && owner.editor.buffer.is_dirty());
                documents.push(Document {
                    id: identity,
                    path: owner.path.clone(),
                    disk: owner.disk_saved.clone(),
                    disk_content: owner.disk_content,
                    saved_in_effective_encoding: owner.saved_in_effective_encoding,
                    effective_encoding: Some(owner.saved_format.encoding.clone()),
                    encoding: owner.format.encoding.clone(),
                    encoding_chosen: owner.encoding_chosen,
                    preferred_encoding: owner.preferred_encoding.clone(),
                    encoding_display_name: owner.format.display_name.clone(),
                    bom: owner.format.bom,
                    line_ending: owner.format.line_ending.name().into(),
                    dirty: retained_dirty && !empty_error,
                    history: if owner.load_failed && owner.restore_history.is_some() {
                        owner
                            .restore_history
                            .as_ref()
                            .map(|(history, _)| history.clone())
                    } else if (owner.loading.is_some() && !owner.external_load)
                        || owner.binary_loading.is_some()
                        || owner.editor.binary.is_some()
                        || empty_error
                    {
                        None
                    } else {
                        Some(if !retained_dirty {
                            if let Some(path) = &owner.path {
                                vel_session::History::capture_saved_with_options(
                                    &owner.editor.buffer,
                                    path.clone(),
                                    owner.decode_options(),
                                )
                            } else {
                                vel_session::History::capture(&owner.editor.buffer)
                            }
                        } else {
                            vel_session::History::capture(&owner.editor.buffer)
                        })
                    },
                });
            }
        }
        self.prepare_sheet(active)?;
        let mut workbench = self.workbench.clone();
        let retained: BTreeSet<_> = sheets.iter().map(|s| s.id).collect();
        for group in &mut workbench.groups {
            let omitted = group
                .sheets
                .iter()
                .filter(|id| !retained.contains(id))
                .copied()
                .collect::<Vec<_>>();
            for id in omitted {
                group.remove(id);
            }
        }
        Ok((
            Window {
                project: self.project.clone(),
                goto_recent_files: self.goto_recent_files.clone(),
                folders: self
                    .folders
                    .iter()
                    .filter(|f| !self.transient_folders.contains(&f.path))
                    .map(|f| f.path.clone())
                    .collect(),
                workbench,
                sheets,
                distraction_free: self.distraction_free,
                normal_ui: self.window_layers.normal.clone(),
                distraction_ui: self.window_layers.distraction.clone(),
                fullscreen: self.fullscreen,
                build_system: self.build_system.clone(),
                ..Window::default()
            },
            documents,
        ))
    }
    /// Installs placeholders for hidden documents and loads only selected sheets.
    pub(crate) fn restore_window_inner(
        &mut self,
        window: &Window,
        documents: &[Document],
    ) -> Result<(), String> {
        crate::session_restore::step("folders");
        self.goto_recent_files = window.goto_recent_files.iter().take(100).cloned().collect();
        for path in &window.folders {
            if !self.folders.iter().any(|f| &f.path == path) {
                self.folders.push(vel_search::project::Folder {
                    name: path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    path: path.clone(),
                    rules: {
                        let patterns = |key| {
                            self.settings
                                .get(key)
                                .and_then(Value::as_array)
                                .map(|v| {
                                    v.iter()
                                        .filter_map(Value::as_str)
                                        .map(str::to_owned)
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default()
                        };
                        vel_search::project::Rules::new(
                            &patterns("file_exclude_patterns"),
                            &patterns("folder_exclude_patterns"),
                            &patterns("binary_file_patterns"),
                        )?
                    },
                    follow_links: true,
                });
            }
        }
        crate::session_restore::step("window_settings");
        self.window_layers.normal = window.normal_ui.clone();
        self.window_layers.distraction = window.distraction_ui.clone();
        self.window_layers
            .normal
            .entry("show_side_bar".into())
            .or_insert(window.normal_sidebar.unwrap_or(window.sidebar_visible));
        self.window_layers
            .distraction
            .entry("show_side_bar".into())
            .or_insert(
                window
                    .distraction_sidebar
                    .unwrap_or(if window.distraction_free {
                        window.sidebar_visible
                    } else {
                        false
                    }),
            );
        self.distraction_free = window.distraction_free;
        self.refresh_window_layers()?;
        let docs: BTreeMap<_, _> = documents.iter().map(|d| (d.id, d)).collect();
        let mut tabs = Vec::new();
        let mut remap = BTreeMap::new();
        let mut document_ids = BTreeMap::new();
        let mut buffer_ids = BTreeMap::new();
        let mut paths: BTreeMap<u64, (Option<PathBuf>, bool)> = BTreeMap::new();
        for sheet in &window.sheets {
            crate::session_restore::step("create_sheet");
            let document = docs
                .get(&sheet.document)
                .ok_or("session document missing")?;
            let (path, missing) = if let Some(resolved) = paths.get(&document.id) {
                resolved.clone()
            } else {
                let path = document.path.clone();
                let resolved = crate::restore_io::run(
                    self.restore_deadline
                        .unwrap_or_else(|| Instant::now() + crate::restore_io::BUDGET),
                    move || {
                        let path = path
                            .as_deref()
                            .map(crate::workspace::absolute)
                            .transpose()?;
                        let missing = path
                            .as_deref()
                            .is_some_and(|p| matches!(p.try_exists(), Ok(false)));
                        Ok((path, missing))
                    },
                )?;
                paths.insert(document.id, resolved.clone());
                resolved
            };
            if missing && self.settings.bool("close_deleted_files", false) && !document.dirty {
                continue;
            }
            let mut tab = Tab::new(Text::new(), &self.settings, &self.keymap)?;
            remap.insert(sheet.id, tab.sheet);
            tab.restored_document = Some(document.id);
            tab.session_document = *document_ids.entry(document.id).or_insert_with(next_sheet);
            // Even unopened siblings refer to one document, never independent buffers.
            tab.document = *buffer_ids.entry(document.id).or_insert(tab.document);
            tab.path = path;
            tab.suggested_name = document
                .history
                .as_ref()
                .and_then(vel_session::History::recovered_name)
                .map(str::to_owned);
            // Missing files with retained contents must still prompt/save after restore.
            // Derive the marker from disk without extending the session wire format.
            tab.deleted = missing;
            tab.format = vel_fileio::Format {
                encoding: document.encoding.clone(),
                display_name: document.encoding_display_name.clone(),
                bom: document.bom,
                line_ending: vel_fileio::LineEnding::parse(&document.line_ending)?,
            };
            tab.saved_format = tab.format.clone();
            if let Some(effective) = &document.effective_encoding {
                tab.saved_format.encoding = effective.clone();
            }
            tab.encoding_chosen = document.encoding_chosen;
            tab.preferred_encoding = document.preferred_encoding.clone().or_else(|| {
                document
                    .encoding_chosen
                    .then(|| tab.format.display_encoding().into())
            });
            tab.disk_saved = document.disk.clone();
            tab.disk_content = document.disk_content;
            tab.saved_in_effective_encoding = document.saved_in_effective_encoding;
            tab.transient = sheet.transient;
            tab.lazy = Some(((*document).clone(), sheet.view.clone()));
            if let Some(html) = &sheet.html {
                tab.editor
                    .settings
                    .set(Layer::View, "velocity_html_sheet", json!(html.contents));
                tab.editor
                    .settings
                    .set(Layer::View, "velocity_html_sheet_title", json!(html.name));
                tab.editor.read_only = true;
            }
            if let Some(settings) = &sheet.settings {
                restore_settings_tab(&mut tab, settings);
            }
            if let Some(syntax) = &sheet.syntax {
                tab.editor
                    .settings
                    .set(Layer::View, "syntax", json!(syntax));
                // Lazy tabs still need their syntax settings for legacy pixel migration;
                // resolving the reference does not materialize the file or font service.
                if let Some(reference) = vel_highlight::detect_syntax(
                    &self.assets.syntaxes,
                    &crate::language::syntax_path(tab.path.as_deref()),
                    "",
                    Some(syntax),
                ) {
                    tab.editor.settings.replace(
                        Layer::Syntax,
                        Value::Object(
                            self.syntax_settings
                                .values(&reference.path, &reference.name),
                        ),
                    )?;
                }
            }
            tab.restore_scroll(&sheet.view);
            tabs.push(tab);
        }
        self.empty = tabs.is_empty();
        if self.empty {
            tabs.push(Tab::new(Text::new(), &self.settings, &self.keymap)?);
        }
        self.tabs = tabs;
        self.active = 0;
        crate::session_restore::step("groups");
        self.workbench = window.workbench.clone();
        for g in &mut self.workbench.groups {
            let mut seen = BTreeSet::new();
            g.mru = g
                .mru
                .iter()
                .filter_map(|s| remap.get(s).copied())
                .filter(|s| seen.insert(*s))
                .collect();
            g.sheets = g
                .sheets
                .iter()
                .filter_map(|s| remap.get(s).copied())
                .collect();
            g.selected = g
                .selected
                .iter()
                .filter_map(|s| remap.get(s).copied())
                .collect();
            g.focused = g.focused.and_then(|s| remap.get(&s).copied());
            g.anchor = g.anchor.and_then(|s| remap.get(&s).copied());
            g.touch_focus();
        }
        let visible: Vec<_> = self
            .workbench
            .groups
            .iter()
            .flat_map(|g| g.visible())
            .filter_map(|s| self.sheet_index(s))
            .collect();
        crate::session_restore::step("visible_views");
        for index in visible {
            if let Err(error) = self.prepare_sheet(index) {
                if error.contains(crate::restore_io::TIMEOUT) {
                    return Err(error);
                }
                self.resource_reports.push(error);
            }
        }
        self.distraction_free = false;
        self.fullscreen = window.fullscreen;
        self.build_system = window.build_system.clone();
        if window.distraction_free {
            self.workbench_action("toggle_distraction_free", &json!({}))?;
        }
        crate::session_restore::step("focus");
        if let Err(error) = self.focus_current_group() {
            if error.contains(crate::restore_io::TIMEOUT) {
                return Err(error);
            }
            self.resource_reports.push(error);
        }
        crate::session_restore::step("synchronize");
        self.sync_shared();
        crate::session_restore::step("validate");
        self.validate_document_views()?;
        self.workbench.validate(&if self.empty {
            BTreeSet::new()
        } else {
            self.tabs.iter().map(|t| t.sheet).collect()
        })?;
        Ok(())
    }
    /// Accept legacy boolean hot_exit without weakening the string policies.
    pub fn hot_exit(&self) -> vel_session::HotExit {
        for layer in [Layer::User, Layer::Platform, Layer::Default] {
            if let Some(value) = self.settings.layer(layer).get("hot_exit") {
                return vel_session::HotExit::from_setting(value);
            }
        }
        vel_session::HotExit::parse(self.global_string("hot_exit", "always"))
    }
    pub fn global_string<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        [Layer::User, Layer::Platform, Layer::Default]
            .into_iter()
            .find_map(|l| self.settings.layer(l).get(key))
            .and_then(Value::as_str)
            .unwrap_or(default)
    }
    pub fn global_bool(&self, key: &str, default: bool) -> bool {
        [Layer::User, Layer::Platform, Layer::Default]
            .into_iter()
            .find_map(|l| self.settings.layer(l).get(key))
            .and_then(Value::as_bool)
            .unwrap_or(default)
    }
    pub fn project_path(&self) -> Option<&Path> {
        self.project.as_deref()
    }
    pub fn data_path(&self) -> Option<&Path> {
        self.data_dir.as_deref()
    }
    pub fn open_project(&mut self, path: PathBuf) -> Result<(), String> {
        self.open_project_with_restore(path, true)
    }
    pub(crate) fn open_project_with_restore(
        &mut self,
        path: PathBuf,
        restore: bool,
    ) -> Result<(), String> {
        let path = std::fs::canonicalize(&path).map_err(|e| e.to_string())?;
        let mut config = Self::new_with_window_overrides(
            Text::new(),
            self.data_dir.clone(),
            Some(path.clone()),
            self.watcher.is_some(),
            Some(self.settings.layer(Layer::Window).clone()),
        )?;
        config.recent_files = self.recent_files.clone();
        config.recent_folders = self.recent_folders.clone();
        config.recent_projects = self.recent_projects.clone();
        remember(&mut config.recent_projects, path.clone());
        if restore {
            let recovery = vel_session::load_project_workspace(&path, self.data_path());
            config.messages.extend(recovery.messages);
            if let Some(window) = recovery.session.windows.first() {
                config.restore_window_from(
                    window,
                    &recovery.session.documents,
                    recovery.source.as_deref(),
                )?;
            }
        }
        std::mem::swap(self, &mut config);
        Ok(())
    }
    pub fn save_project_as(&mut self, path: PathBuf) -> Result<(), String> {
        // Keep exclusion rules/unknown fields from the original shareable project.
        let mut project = self.project_model.clone();
        let path = std::path::absolute(path).map_err(|e| e.to_string())?;
        let base = self.project.as_deref().unwrap_or(&path);
        project.rebase(base, &path);
        let new_base = path.parent().unwrap_or(Path::new("."));
        for folder in &self.folders {
            if !project.folders.iter().any(|f| {
                let candidate = new_base.join(&f.path);
                candidate == folder.path
                    || candidate.canonicalize().ok().as_ref() == Some(&folder.path)
            }) {
                project.folders.push(vel_session::project::Folder {
                    path: vel_session::project::relative_path(&folder.path, new_base),
                    name: Some(folder.name.clone()),
                    folder_exclude_patterns: vec![],
                    file_exclude_patterns: vec![],
                    binary_file_patterns: vec![],
                    follow_symlinks: (!folder.follow_links).then_some(false),
                    ..Default::default()
                });
            }
        }
        project.save(&path)?;
        self.project = Some(path.clone());
        self.project_model = project;
        remember(&mut self.recent_projects, path);
        Ok(())
    }
    pub fn close_project(&mut self) -> Result<(), String> {
        let mut empty = Self::new_with_window_overrides(
            Text::new(),
            self.data_dir.clone(),
            None,
            self.watcher.is_some(),
            Some(self.settings.layer(Layer::Window).clone()),
        )?;
        empty.recent_files = self.recent_files.clone();
        empty.recent_folders = self.recent_folders.clone();
        empty.recent_projects = self.recent_projects.clone();
        std::mem::swap(self, &mut empty);
        Ok(())
    }
    pub fn open_additive(&mut self, location: FileLocation) -> Result<(), String> {
        let group = self.workbench.active;
        let selected = self.workbench.groups[group].selected.clone();
        self.open(location)?;
        if self.workbench.active == group {
            self.workbench.groups[group].selected.extend(selected);
        }
        self.make_permanent(self.active);
        Ok(())
    }
}

pub fn configure_groups(workspace: &mut Workspace, count: usize) -> Result<(), String> {
    workspace.workbench.set_layout(if count == 4 {
        Layout::grid(2, 2)
    } else {
        Layout::grid(count, 1)
    })?;
    let original = workspace.tabs[workspace.active].sheet;
    for group in 1..count {
        if let Some(index) = workspace.sheet_index(original) {
            workspace.select(index);
        }
        workspace.clone_file()?;
        workspace.workbench.move_selected(group)?;
        workspace.focus_current_group()?;
    }
    workspace.workbench.active = 0;
    workspace.focus_current_group()?;
    Ok(())
}

fn remove_font_size(document: &vel_config::json::Document) -> Result<String, String> {
    let Some(span) = document.spans.get("/font_size") else {
        return Ok(document.source().into());
    };
    let source = document.source();
    // Scan strings/comments rather than searching for a key-like substring inside
    // unrelated user text. Value spans let removal preserve every neighboring byte.
    let bytes = source.as_bytes();
    let mut i = 0;
    let mut key = None;
    let mut commas = Vec::new();
    while i < span.end {
        match bytes[i] {
            b'"' => {
                let start = i;
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i += 2;
                    } else if bytes[i] == b'"' {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
                if i <= span.start
                    && serde_json::from_str::<String>(&source[start..i])
                        .is_ok_and(|s| s == "font_size")
                {
                    key = Some(start);
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() && &bytes[i..i + 2] != b"*/" {
                    i += 1;
                }
                i += 2;
            }
            b',' => {
                commas.push(i);
                i += 1;
            }
            _ => i += 1,
        }
    }
    let start = key.ok_or("font_size key span unavailable")?;
    let mut end = span.end;
    let mut after = end;
    loop {
        while bytes.get(after).is_some_and(u8::is_ascii_whitespace) {
            after += 1;
        }
        if bytes.get(after..after + 2) == Some(b"//") {
            while after < bytes.len() && bytes[after] != b'\n' {
                after += 1;
            }
        } else if bytes.get(after..after + 2) == Some(b"/*") {
            after += 2;
            while after + 1 < bytes.len() && &bytes[after..after + 2] != b"*/" {
                after += 1;
            }
            after += 2;
        } else {
            break;
        }
    }
    let mut source = source.to_owned();
    if bytes.get(after) == Some(&b',') {
        end = after + 1;
        source.replace_range(start..end, "");
    } else {
        source.replace_range(start..end, "");
        if let Some(comma) = commas.into_iter().rfind(|&c| c < start) {
            source.remove(comma);
        }
    }
    vel_config::json::Document::parse(&source)?;
    Ok(source)
}
