//! Window workbench integration, shared-buffer ownership and session/project adapters.
use crate::{
    cli::FileLocation,
    workspace::{Tab, Workspace},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use vel_buffer::Buffer;
use vel_config::{Value, json, settings::Layer};
use vel_session::{Document, Sheet, Window};
use vel_text::Text;
use vel_ui::workbench::{Direction, Layout};
static SHEET: AtomicU64 = AtomicU64::new(1);
/// Keep new process-local identities above snapshots reserved from a previous process.
pub fn reserve_sheet_ids(maximum: u64) {
    SHEET.fetch_max(maximum.saturating_add(1), Ordering::Relaxed);
}
pub fn next_sheet() -> u64 {
    SHEET.fetch_add(1, Ordering::Relaxed)
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
impl Workspace {
    pub fn sheet_index(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.sheet == id)
    }
    pub fn selected_indices(&self) -> Vec<usize> {
        self.workbench.groups[self.workbench.active]
            .visible()
            .iter()
            .filter_map(|id| self.sheet_index(*id))
            .collect()
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
        let result = self.prepare_sheet_inner(index);
        if result.is_err()
            && let Some(tab) = self.tabs.get_mut(index)
        {
            // Consume a failing lazy restore once. Repeated paint/access must not retry IO
            // or flood the error log; explicit Reopen is the user's retry operation.
            tab.lazy = None;
            tab.editor.read_only = true;
        }
        result
    }
    fn prepare_sheet_inner(&mut self, index: usize) -> Result<(), String> {
        if index >= self.tabs.len() {
            return Err("unknown sheet".into());
        }
        if let Some((document, state)) = self.tabs[index].lazy.clone() {
            let owner = self
                .tabs
                .iter()
                .enumerate()
                .find(|(i, t)| {
                    *i != index
                        && t.restored_document == Some(document.id)
                        && t.lazy.is_none()
                        && t.editor.buffer.id() == t.document
                })
                .map(|(i, _)| i);
            if let Some(owner) = owner {
                self.tabs[index].lazy = None;
                self.tabs[index].needs_detection = self.tabs[owner].needs_detection;
                self.tabs[index].indentation_initialized = self.tabs[owner].indentation_initialized;
                let existing = self.tabs[index].shared_view;
                let view = if self.tabs[owner]
                    .editor
                    .buffer
                    .views()
                    .any(|id| id == existing)
                {
                    existing
                } else {
                    let view = self.tabs[owner].editor.buffer.new_view();
                    state.apply(&mut self.tabs[owner].editor.buffer, view);
                    view
                };
                self.tabs[owner]
                    .editor
                    .restore_folded_view_state(&state, view);
                self.tabs[index].document = self.tabs[owner].document;
                self.tabs[index].shared_view = view;
                self.tabs[index].editor.shared_shadow = true;
                self.tabs[index].editor.language = self.tabs[owner].editor.language.clone();
                self.tabs[index].editor.layout_edits = self.tabs[owner].editor.layout_edits.clone();
                self.tabs[index].editor.annotations = self.tabs[owner].editor.annotations.clone();
                self.prepare_sheet(index)?;
                self.sync_shared();
                return Ok(());
            }
            let changed = document
                .path
                .as_deref()
                .is_some_and(|p| vel_session::DiskStamp::read(p) != document.disk);
            let mut imported = false;
            let missing = document
                .path
                .as_deref()
                .is_some_and(|p| matches!(p.try_exists(), Ok(false)));
            if missing && !document.history.as_ref().is_some_and(|h| h.retains_text()) {
                self.tabs[index].deleted = true;
            } else if let Some(history) = &document.history {
                // Dirty/untitled histories retain their text and restore without reading the
                // file. Permission/decoding failures on disk cannot discard those edits.
                let dirty = document.dirty || (changed && history.retains_text());
                let restored = history.restore_views(!changed, dirty);
                imported = restored.is_ok() && !changed;
                let (buffer, map) = restored.or_else(|_| history.restore_views(false, dirty))?;
                let id = state
                    .history_view
                    .and_then(|id| map.get(&id).copied())
                    .or_else(|| map.values().next().copied())
                    .ok_or("history has no view")?;
                self.tabs[index].editor.buffer = buffer;
                self.tabs[index].editor.id = id;
                // Every clone, including lazy sheets, must keep its original edit ownership.
                for sibling in &mut self.tabs {
                    if sibling.restored_document == Some(document.id)
                        && let Some((_, view)) = &sibling.lazy
                        && let Some(id) = view.history_view.and_then(|old| map.get(&old))
                    {
                        sibling.shared_view = *id;
                    }
                }
                let tab = &mut self.tabs[index];
                tab.editor.sync();
            } else if let Some(path) = &document.path {
                let tab = &mut self.tabs[index];
                if crate::binary::is_binary(path)? {
                    tab.binary_loading = Some(crate::binary::Task::open(path.clone())?);
                    tab.editor.source_bytes =
                        std::fs::metadata(path).map_err(|e| e.to_string())?.len();
                    tab.pending_restore = Some(state.clone());
                } else if std::fs::metadata(path).map_err(|e| e.to_string())?.len()
                    >= tab
                        .editor
                        .settings
                        .usize("large_file.load_bytes", 64 * 1024 * 1024)
                        as u64
                {
                    tab.loading = Some(vel_fileio::load_background(
                        path.clone(),
                        tab.editor.preferences.fallback_encoding.clone(),
                    )?);
                    tab.editor.source_bytes =
                        std::fs::metadata(path).map_err(|e| e.to_string())?.len();
                    tab.pending_restore = Some(state.clone());
                } else {
                    let loaded =
                        vel_fileio::load(path, &tab.editor.preferences.fallback_encoding, |_| {})
                            .inspect_err(|_| {
                            tab.editor.read_only = true;
                        })?;
                    tab.install_restored(loaded);
                }
            }
            let tab = &mut self.tabs[index];
            tab.lazy = None;
            tab.editor.read_only = tab.loading.is_some() || tab.binary_loading.is_some();
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
            tab.disk_saved = document.disk;
            tab.disk_stamp = tab.path.as_deref().and_then(crate::workspace::stamp);
            if let Some(w) = self.watcher.as_mut()
                && let Some(path) = &tab.path
            {
                let _ = w.watch(path);
            }
            self.configure_language(index, None, true)?;
        }
        let doc = self.tabs[index].document;
        let owner = self.tabs.iter().position(|t| t.editor.buffer.id() == doc);
        if let Some(owner) = owner
            && owner != index
        {
            self.tabs[owner].editor.unmark();
            let target_view = self.tabs[index].shared_view;
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
        self.tabs[index].editor.adopt_document();
        Ok(())
    }
    /// Refreshes inactive clone snapshots after commands without copying document bytes.
    pub fn sync_shared(&mut self) {
        self.materialize_input();
        let owners: Vec<_> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| t.editor.buffer.id() == t.document && t.lazy.is_none())
            .map(|(i, _)| i)
            .collect();
        for owner in owners {
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
                self.tabs[index].shared_dirty = dirty;
                self.tabs[index].deleted = self.tabs[owner].deleted;
                self.tabs[index].path = self.tabs[owner].path.clone();
                self.tabs[index].format = self.tabs[owner].format.clone();
                self.tabs[index].saved_format = self.tabs[owner].saved_format.clone();
                self.tabs[index].tracker = self.tabs[owner].tracker.clone();
                self.tabs[index].disk_saved = self.tabs[owner].disk_saved.clone();
                self.tabs[index].disk_stamp = self.tabs[owner].disk_stamp;
                self.tabs[index].restored_format_dirty = self.tabs[owner].restored_format_dirty;
            }
            self.tabs[owner].shared_dirty = false;
        }
    }
    pub fn reconcile_installs(&mut self) {
        for index in 0..self.tabs.len() {
            let Some(old) = self.tabs[index].previous_document.take() else {
                continue;
            };
            let document = self.tabs[index].document;
            self.tabs[index].editor.layout_edits.borrow_mut().clear();
            for other in 0..self.tabs.len() {
                if other == index
                    || self.tabs[other].document != old
                    || self.tabs[other].lazy.is_some()
                {
                    continue;
                }
                let state = vel_session::ViewState::capture(
                    &self.tabs[other].editor.buffer,
                    self.tabs[other].editor.id,
                    self.tabs[other].scroll,
                );
                let view = self.tabs[index].editor.buffer.new_view();
                state.apply(&mut self.tabs[index].editor.buffer, view);
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
            self.new_file()?;
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
        clone.session_document = self.tabs[index].session_document;
        clone.deleted = self.tabs[index].deleted;
        clone.document = self.tabs[index].document;
        clone.shared_view = id;
        clone.disk_saved = self.tabs[index].disk_saved.clone();
        clone.disk_stamp = self.tabs[index].disk_stamp;
        clone.needs_detection = self.tabs[index].needs_detection;
        clone.indentation_initialized = self.tabs[index].indentation_initialized;
        clone.restored_document = self.tabs[index].restored_document;
        clone.editor.binary = self.tabs[index].editor.binary.clone();
        clone.editor.read_only = self.tabs[index].editor.read_only;
        clone.editor.features = self.tabs[index].editor.features;
        clone.editor.shared_shadow = true;
        clone.editor.language = self.tabs[index].editor.language.clone();
        clone.editor.layout_edits = self.tabs[index].editor.layout_edits.clone();
        clone.editor.annotations = self.tabs[index].editor.annotations.clone();
        clone.editor.layout_version = self.tabs[index].editor.layout_version;
        clone.path = self.tabs[index].path.clone();
        clone.format = self.tabs[index].format.clone();
        clone.saved_format = self.tabs[index].saved_format.clone();
        clone.tracker = self.tabs[index].tracker.clone();
        clone.scroll = self.tabs[index].scroll;
        let sheet = clone.sheet;
        self.tabs.push(clone);
        self.workbench.groups[self.workbench.active].add(sheet, false);
        self.prepare_sheet(self.tabs.len() - 1)?;
        self.active = self.tabs.len() - 1;
        self.sync_shared();
        Ok(())
    }
    pub fn prepare_close_sheet(&mut self, index: usize) -> Result<(), String> {
        self.prepare_sheet(index)?;
        let doc = self.tabs[index].document;
        let other = self
            .tabs
            .iter()
            .enumerate()
            .find(|(i, t)| *i != index && t.document == doc)
            .map(|(i, _)| i);
        let view = self.tabs[index].shared_view;
        if let Some(other) = other {
            self.prepare_sheet(other)?;
            self.tabs[other].editor.buffer.close_view(view);
        }
        Ok(())
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
                let layout = if let Some(preset) = args["preset"].as_str() {
                    match preset {
                        "columns2" => Layout::grid(2, 1),
                        "columns3" => Layout::grid(3, 1),
                        "columns4" => Layout::grid(4, 1),
                        "rows2" => Layout::grid(1, 2),
                        "rows3" => Layout::grid(1, 3),
                        "grid4" => Layout::grid(2, 2),
                        _ => Layout::default(),
                    }
                } else {
                    serde_json::from_value(args.clone()).map_err(|e| e.to_string())?
                };
                self.workbench.set_layout(layout)?;
                self.focus_current_group()?;
            }
            "focus_group" | "move_to_group" => {
                let group = args["group"].as_u64().ok_or("group index required")? as usize;
                if group >= self.workbench.groups.len() {
                    return Err("unknown group".into());
                }
                if name == "move_to_group" {
                    self.workbench.move_selected(group)?;
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
                        self.workbench.move_selected(group)?;
                    } else {
                        self.workbench.active = group;
                    }
                    self.focus_current_group()?;
                }
            }
            "new_pane" => {
                self.workbench
                    .new_pane(!matches!(args["direction"].as_str(), Some("up" | "down")))?;
                if matches!(args["direction"].as_str(), Some("left" | "up")) {
                    let last = self.workbench.layout.cells.len() - 1;
                    self.workbench
                        .layout
                        .cells
                        .swap(self.workbench.active, last);
                }
            }
            "close_pane" | "close_split" => {
                if self.workbench.groups.len() > 1 {
                    self.workbench.close_pane()?;
                    self.focus_current_group()?;
                }
            }
            "clone_file" => self.clone_file()?,
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
                let values = if self.distraction_free {
                    self.distraction_settings.clone()
                } else {
                    serde_json::Map::new()
                };
                for tab in &mut self.tabs {
                    tab.editor
                        .settings
                        .replace(Layer::DistractionFree, Value::Object(values.clone()))?;
                    tab.editor.sync();
                }
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
            if !assigned.contains(&tab.sheet) {
                continue;
            }
            let doc = tab.document;
            let identity = tab.session_document;
            if let Some((document, view)) = &tab.lazy {
                let loaded = self.tabs.iter().find(|t| {
                    t.restored_document == Some(document.id)
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
                        .map(|owner| owner.editor.folded_view_state(tab.shared_view, tab.scroll))
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
            let view = self.tabs[owner]
                .editor
                .folded_view_state(tab.shared_view, tab.scroll);
            sheets.push(Sheet {
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
                documents.push(Document {
                    id: identity,
                    path: owner.path.clone(),
                    disk: owner.disk_saved.clone(),
                    encoding: owner.format.encoding.clone(),
                    bom: owner.format.bom,
                    line_ending: owner.format.line_ending.name().into(),
                    dirty: owner.dirty(),
                    history: if owner.loading.is_some()
                        || owner.binary_loading.is_some()
                        || owner.editor.binary.is_some()
                    {
                        None
                    } else {
                        Some(if !owner.dirty() {
                            if let Some(path) = &owner.path {
                                vel_session::History::capture_saved(
                                    &owner.editor.buffer,
                                    path.clone(),
                                    owner.format.encoding.clone(),
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
        Ok((
            Window {
                project: self.project.clone(),
                folders: self.folders.iter().map(|f| f.path.clone()).collect(),
                workbench: self.workbench.clone(),
                sheets,
                distraction_free: self.distraction_free,
                fullscreen: self.fullscreen,
                build_system: self.build_system.clone(),
                ..Window::default()
            },
            documents,
        ))
    }
    /// Installs placeholders for hidden documents and loads only selected sheets.
    pub fn restore_window(
        &mut self,
        window: &Window,
        documents: &[Document],
    ) -> Result<(), String> {
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
        let docs: BTreeMap<_, _> = documents.iter().map(|d| (d.id, d)).collect();
        let mut tabs = Vec::new();
        let mut remap = BTreeMap::new();
        let mut document_ids = BTreeMap::new();
        for sheet in &window.sheets {
            let document = docs
                .get(&sheet.document)
                .ok_or("session document missing")?;
            let missing = document
                .path
                .as_deref()
                .is_some_and(|p| matches!(p.try_exists(), Ok(false)));
            if missing && self.settings.bool("close_deleted_files", false) && !document.dirty {
                continue;
            }
            let mut tab = Tab::new(Text::new(), &self.settings, &self.keymap)?;
            remap.insert(sheet.id, tab.sheet);
            tab.restored_document = Some(document.id);
            tab.session_document = *document_ids.entry(document.id).or_insert_with(next_sheet);
            tab.path = document.path.clone();
            // Missing files with retained contents must still prompt/save after restore.
            // Derive the marker from disk without extending the session wire format.
            tab.deleted = tab
                .path
                .as_deref()
                .is_some_and(|path| matches!(path.try_exists(), Ok(false)));
            tab.format = vel_fileio::Format {
                encoding: document.encoding.clone(),
                bom: document.bom,
                line_ending: vel_fileio::LineEnding::parse(&document.line_ending)?,
            };
            tab.saved_format = tab.format.clone();
            tab.disk_saved = document.disk.clone();
            tab.scroll = sheet.view.scroll;
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
            if let Some(syntax) = &sheet.syntax {
                tab.editor
                    .settings
                    .set(Layer::View, "syntax", json!(syntax));
            }
            tabs.push(tab);
        }
        self.empty = tabs.is_empty();
        if self.empty {
            tabs.push(Tab::new(Text::new(), &self.settings, &self.keymap)?);
        }
        self.tabs = tabs;
        self.active = 0;
        self.workbench = window.workbench.clone();
        for g in &mut self.workbench.groups {
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
        }
        let visible: Vec<_> = self
            .workbench
            .groups
            .iter()
            .flat_map(|g| g.visible())
            .filter_map(|s| self.sheet_index(s))
            .collect();
        for index in visible {
            if let Err(error) = self.prepare_sheet(index) {
                self.messages.push(error);
            }
        }
        self.distraction_free = false;
        self.fullscreen = window.fullscreen;
        self.build_system = window.build_system.clone();
        if window.distraction_free {
            self.workbench_action("toggle_distraction_free", &json!({}))?;
        }
        if let Err(error) = self.focus_current_group() {
            self.messages.push(error);
        }
        self.sync_shared();
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
        let path = std::fs::canonicalize(&path).map_err(|e| e.to_string())?;
        let mut config = Self::new(
            Text::new(),
            self.data_dir.clone(),
            Some(path.clone()),
            self.watcher.is_some(),
        )?;
        config.recent_files = self.recent_files.clone();
        config.recent_folders = self.recent_folders.clone();
        config.recent_projects = self.recent_projects.clone();
        remember(&mut config.recent_projects, path.clone());
        let recovery = vel_session::load(&vel_session::project::workspace_path(&path));
        config.messages.extend(recovery.messages);
        if let Some(window) = recovery.session.windows.first() {
            config.restore_window(window, &recovery.session.documents)?;
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
        let mut empty = Self::new(
            Text::new(),
            self.data_dir.clone(),
            None,
            self.watcher.is_some(),
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
        self.tabs[self.active].transient = false;
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
