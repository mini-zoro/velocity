//! Headless UI controller: navigation, panel focus, workers and project tree state.
use crate::{cli::FileLocation, workspace::Workspace};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::Range,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::Instant,
};
use vel_buffer::{BufferId, Region, Selection};
use vel_config::{
    Value, json,
    keymap::{Invocation, Key},
};
use vel_platform::{Command, Point, Rect, Size, TextInput};
use vel_render::scene::{Layer, Scene};
use vel_search::{
    files, fuzzy,
    goto::{Query, Target},
    project::{Entry, Folder, Index, Rules},
    text::{self, Search},
};
mod details;

use vel_ui::{
    Focus, Item, Node, QuickPanel, Scroll, Surface, Tree, contains, fill, input::Input, label,
};

/// Rows of the side bar's "Open Files" list: none for the hidden input tab of an empty window.
fn open_file_count(workspace: &Workspace) -> usize {
    if workspace.is_empty() {
        0
    } else {
        workspace.tabs.len()
    }
}

pub const STATUS: f64 = 24.;
const PANEL_ROW: f64 = 34.;
/// Side of the find-panel close button, in points.
const CLOSE_BUTTON: f64 = 22.;
/// Retained widget id of the find-panel close button.
const CLOSE_ID: u64 = 60;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Find,
    Replace,
    Incremental,
    Files,
    Input,
    Output,
    Results,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    Goto,
    Palette,
    Syntax,
    Choices,
    ProjectSymbols,
    Locations,
}
#[derive(Debug)]
struct Preview {
    sheet: u64,
    selection: Selection,
    scroll: (f64, f64),
    transient: bool,
}
#[derive(Clone, Copy, Debug)]
enum FindNavigation {
    Preview(usize),
    Next {
        backwards: bool,
        close: bool,
        report: bool,
    },
}
#[derive(Clone, Debug)]
pub struct SideRow {
    pub path: PathBuf,
    pub folder: usize,
    pub depth: usize,
    pub directory: bool,
}
#[derive(Debug)]
enum Local {
    Words(u64, Vec<(String, usize)>),
}
/// Complete retained widget/controller state. All methods can run without a window server.
#[derive(Debug)]
pub struct Panels {
    vintage_prompt: Option<u64>,
    pub ui_scale: f64,
    pub review: crate::review_ui::ReviewUi,
    git_tree_key: Vec<(PathBuf, u64)>,
    git_tree_dirty: bool,
    git_virtual: BTreeSet<PathBuf>,
    pub intel: crate::intelligence::Intelligence,
    locations: Vec<vel_index::SymbolLocation>,
    navigation_origin: Option<crate::intelligence::Jump>,
    hover: Vec<vel_index::SymbolLocation>,
    hover_point: Point,
    hover_term: String,
    hover_since: Instant,
    hover_requested: bool,
    hover_bounds: Rect,
    hover_html: Option<vel_ui::minihtml::Popup>,
    pub restore_scroll: Option<(f64, f64)>,
    pub reveal: bool,
    preserve_scroll: bool,
    incremental_origin: Option<(Preview, BufferId, vel_buffer::ChangeId)>,
    find_origin: Option<(BufferId, usize)>,
    history_cursor: [Option<usize>; 2],
    history_draft: [String; 2],
    message_seen: String,
    message_until: Option<Instant>,
    pub search_results_active: bool,
    result_cursor: Option<usize>,
    files_searched: usize,
    files_matched: BTreeSet<PathBuf>,
    file_matches: usize,
    result_status: String,
    palette_enabled: Vec<bool>,
    palette_accept: Option<Invocation>,
    pub pending_command: Option<Invocation>,
    pub overlay: Option<Overlay>,
    pub panel: Option<Panel>,
    panel_focused: bool,
    pub query: Input,
    pub replace: Input,
    pub where_input: Input,
    pub quick_input: Input,
    pub quick: QuickPanel,
    pub focus: Focus,
    pub tree: Tree,
    pub side_visible: bool,
    pub side_focused: bool,
    pub side_selected: usize,
    sidebar_reveal: Option<PathBuf>,
    side_open: Option<usize>,
    pub side_scroll: Scroll,
    pub side_rows: Vec<SideRow>,
    pub primary_click: bool,
    pub build_results: Vec<(usize, FileLocation)>,
    pub build_output_offset: usize,
    pub find_history: Vec<String>,
    pub replace_history: Vec<String>,
    pub chrome: bool,
    choice_items: Vec<vel_config::ui::PaletteItem>,
    side_anchor: Option<usize>,
    pub dragging_tab: Option<usize>,
    pub options: text::Options,
    pub in_selection: bool,
    pub highlights: Vec<Range<usize>>,
    pub message: String,
    pub result_view: files::Results,
    pub result_selected: usize,
    pub result_scroll: Scroll,
    pub file_task: Option<files::Task>,
    pub file_complete: bool,
    file_signature: Option<(String, text::Options, String)>,
    replace_rx: Option<mpsc::Receiver<Result<Vec<files::FileChange>, String>>>,
    replace_signature: Option<(String, text::Options, String, String)>,
    pub replacement_plan: Option<Vec<files::FileChange>>,
    pub output: String,
    pub output_editor: Option<crate::editor::Editor>,
    pub output_view: crate::view::View,
    pub output_reset: bool,
    output_mirrored: usize,
    pub input_caption: String,
    pub input_done: Option<Invocation>,
    focus_input: usize,
    selection_scope: Vec<Range<usize>>,
    scope_revision: Option<(BufferId, vel_buffer::ChangeId)>,
    search: Option<Search>,
    find_error: Option<String>,
    search_worker: text::Worker,
    search_identity: Option<(BufferId, u64, String, text::Options, bool)>,
    all_matches_ready: bool,
    pending_navigation: Option<FindNavigation>,
    fuzzy_worker: fuzzy::Worker,
    candidates: Arc<Vec<fuzzy::Candidate>>,
    palette_indices: Vec<usize>,
    paths: Arc<Vec<PathBuf>>,
    file_paths: Arc<Vec<PathBuf>>,
    file_candidates: Arc<Vec<fuzzy::Candidate>>,
    index: Option<Index>,
    folder_key: String,
    recency: HashMap<PathBuf, u64>,
    palette_recent: HashMap<String, u64>,
    uses: u64,
    preview: Option<Preview>,
    last_preview: Option<(PathBuf, Target)>,
    expanded: BTreeSet<PathBuf>,
    children: BTreeMap<PathBuf, Vec<Entry>>,
    directories: mpsc::Receiver<(PathBuf, Result<Vec<Entry>, String>)>,
    directory_tx: mpsc::Sender<(PathBuf, Result<Vec<Entry>, String>)>,
    loading: BTreeSet<PathBuf>,
    local: Option<mpsc::Receiver<Local>>,
    local_positions: Vec<usize>,
    local_generation: u64,
    quick_revision: u64,
    quick_query: String,
    quick_target: &'static str,
    quick_ready: bool,
    prepared: String,
    pub quick_elapsed_ms: f64,
    pub quick_pending: bool,
    panel_bounds: Rect,
    quick_bounds: Rect,
    side_bounds: Rect,
    size: Size,
}
impl Default for Panels {
    fn default() -> Self {
        let (directory_tx, directories) = mpsc::channel();
        Self {
            vintage_prompt: None,
            ui_scale: 1.,
            review: Default::default(),
            git_tree_key: Vec::new(),
            git_tree_dirty: false,
            git_virtual: BTreeSet::new(),
            intel: Default::default(),
            locations: Vec::new(),
            navigation_origin: None,
            hover: Vec::new(),
            hover_point: Point::default(),
            hover_term: String::new(),
            hover_since: Instant::now(),
            hover_requested: false,
            hover_bounds: Rect::default(),
            hover_html: None,
            restore_scroll: None,
            reveal: false,
            preserve_scroll: false,
            incremental_origin: None,
            find_origin: None,
            history_cursor: [None, None],
            history_draft: Default::default(),
            message_seen: String::new(),
            message_until: None,
            search_results_active: false,
            result_cursor: None,
            files_searched: 0,
            files_matched: BTreeSet::new(),
            file_matches: 0,
            result_status: String::new(),
            palette_enabled: Vec::new(),
            palette_accept: None,
            pending_command: None,
            overlay: None,
            panel: None,
            panel_focused: false,
            query: Input::default(),
            replace: Input::default(),
            where_input: Input::new("<project>"),
            quick_input: Input::default(),
            quick: QuickPanel::default(),
            focus: Focus::default(),
            tree: Tree::default(),
            side_visible: true,
            side_focused: false,
            side_selected: 0,
            sidebar_reveal: None,
            side_open: None,
            side_scroll: Scroll::default(),
            side_rows: vec![],
            primary_click: false,
            build_results: Vec::new(),
            build_output_offset: 0,
            find_history: Vec::new(),
            replace_history: Vec::new(),
            chrome: true,
            choice_items: Vec::new(),
            side_anchor: None,
            dragging_tab: None,
            result_view: files::Results::default(),
            quick_pending: false,
            options: text::Options::default(),
            in_selection: false,
            highlights: vec![],
            message: String::new(),
            result_selected: 0,
            result_scroll: Scroll::default(),
            file_task: None,
            file_complete: false,
            file_signature: None,
            replace_rx: None,
            replace_signature: None,
            replacement_plan: None,
            output: String::new(),
            output_editor: None,
            output_view: crate::view::View::new(Size::default(), 7.8, 12.),
            output_reset: false,
            output_mirrored: 0,
            input_caption: String::new(),
            input_done: None,
            focus_input: 0,
            selection_scope: vec![],
            scope_revision: None,
            search: None,
            find_error: None,
            search_worker: text::Worker::default(),
            search_identity: None,
            all_matches_ready: false,
            pending_navigation: None,
            fuzzy_worker: fuzzy::Worker::new(),
            candidates: Arc::new(vec![]),
            palette_indices: Vec::new(),
            paths: Arc::new(vec![]),
            file_paths: Arc::new(vec![]),
            file_candidates: Arc::new(vec![]),
            index: None,
            folder_key: String::new(),
            recency: HashMap::new(),
            palette_recent: HashMap::new(),
            uses: 0,
            preview: None,
            last_preview: None,
            expanded: BTreeSet::new(),
            children: BTreeMap::new(),
            directories,
            directory_tx,
            loading: BTreeSet::new(),
            local: None,
            local_positions: vec![],
            local_generation: 0,
            quick_revision: 0,
            quick_query: String::new(),
            quick_target: "",
            quick_ready: false,
            prepared: String::new(),
            quick_elapsed_ms: 0.,
            panel_bounds: Rect::default(),
            quick_bounds: Rect::default(),
            side_bounds: Rect::default(),
            size: Size::default(),
        }
    }
}
impl Panels {
    pub fn new(workspace: &Workspace, workers: bool) -> Self {
        let mut ui = Self {
            ui_scale: vel_ui::theme::ui_scale(&workspace.editor().settings),
            side_visible: workspace.editor().settings.bool("show_side_bar", true),
            ..Self::default()
        };
        if workers {
            ui.restart_index(workspace);
        }
        ui
    }
    fn restart_index(&mut self, workspace: &Workspace) {
        self.folder_key = format!("{:?}", workspace.folders);
        self.index = (!workspace.folders.is_empty()).then(|| Index::new(workspace.folders.clone()));
        self.children.clear();
        self.loading.clear();
        self.rebuild_side(workspace);
    }
    pub fn configure_output(
        &mut self,
        workspace: &Workspace,
        syntax: Option<&str>,
        word_wrap: bool,
    ) {
        let mut editor = crate::editor::Editor::new(vel_text::Text::new());
        editor.theme = workspace.themes.ui.clone();
        editor.settings = workspace.editor().settings.clone();
        for (key, value) in [
            ("word_wrap", json!(word_wrap)),
            ("line_numbers", json!(false)),
            ("gutter", json!(false)),
            ("show_minimap", json!(false)),
            ("highlight_line", json!(false)),
            ("draw_indent_guides", json!(false)),
            ("rulers", json!([])),
            ("draw_centered", json!(false)),
            ("scroll_past_end", json!(false)),
        ] {
            editor
                .settings
                .set(vel_config::settings::Layer::View, key, value);
        }
        editor.sync();
        editor.read_only = true;
        editor.buffer.set_history_limits(vel_buffer::HistoryLimits {
            entries: 0,
            bytes: 0,
            ..Default::default()
        });
        if let Some(language) = workspace.editor().language.borrow().as_ref() {
            let reference = syntax
                .and_then(|s| {
                    vel_highlight::detect_syntax(&workspace.assets.syntaxes, "", "", Some(s))
                })
                .unwrap_or_else(|| workspace.assets.syntaxes.find_syntax_plain_text());
            if let Err(e) = editor.attach_language(
                workspace.assets.clone(),
                &reference.path,
                language.scheme.clone(),
            ) {
                self.message = e;
            }
        }
        self.output_editor = Some(editor);
        self.output_mirrored = 0;
        self.output_reset = false;
        self.output_view.tab_height = 0.;
        self.output_view.focused = false;
    }
    fn sync_output(&mut self) -> bool {
        let Some(editor) = self.output_editor.as_mut() else {
            return false;
        };
        let mut changed = editor.poll_language();
        if self.output_reset || self.output_mirrored != self.output.len() {
            let (range, value) = if self.output_reset || self.output_mirrored > self.output.len() {
                (0..editor.text.len_bytes(), self.output.clone())
            } else {
                (
                    editor.text.len_bytes()..editor.text.len_bytes(),
                    self.output[self.output_mirrored..].to_owned(),
                )
            };
            editor.apply_edits(vec![(range, value)], None, vel_buffer::EditKind::Other);
            editor.sync();
            self.output_mirrored = self.output.len();
            self.output_reset = false;
            changed = true;
        }
        changed
    }
    pub fn choices(&mut self, workspace: &mut Workspace, items: Vec<vel_config::ui::PaletteItem>) {
        self.cancel_overlay(workspace);
        self.choice_items = items;
        self.overlay = Some(Overlay::Choices);
        self.quick.set_items(Vec::new());
        self.quick_input.set("");
        self.prepared.clear();
        self.focus.show(Surface::Overlay, true);
        self.query_quick(workspace);
    }
    fn remember_search(&mut self, include_empty_replace: bool) {
        fn push(history: &mut Vec<String>, value: String) {
            history.retain(|s| *s != value);
            history.push(value);
            let excess = history.len().saturating_sub(100);
            history.drain(..excess);
        }
        let query = self.query.value();
        if !query.is_empty() {
            push(&mut self.find_history, query);
        }
        let replace = self.replace.value();
        if include_empty_replace {
            push(&mut self.replace_history, replace);
        }
        self.history_cursor = [None, None];
    }
    pub fn capture_session(&mut self, window: &mut vel_session::Window) {
        window.sidebar_visible = self.side_visible;
        window.sidebar_expanded = self.expanded.iter().cloned().collect();
        window.sidebar_scroll = self.side_scroll.offset;
        window.find_history = self.find_history.clone();
        window.replace_history = self.replace_history.clone();
    }
    pub fn restore_session(&mut self, workspace: &Workspace, window: &vel_session::Window) {
        self.side_visible = window.sidebar_visible;
        self.expanded = window.sidebar_expanded.iter().cloned().collect();
        self.side_scroll.offset = window.sidebar_scroll;
        self.find_history = window.find_history.clone();
        self.replace_history = window.replace_history.clone();
        if let Some(value) = self.find_history.last() {
            self.query.set(value);
        }
        if let Some(value) = self.replace_history.last() {
            self.replace.set(value);
        }
        for path in &window.sidebar_expanded {
            self.load_directory(workspace, path);
        }
        self.chrome = !window.distraction_free;
    }
    pub fn add_folder(&mut self, workspace: &mut Workspace, path: PathBuf) -> Result<(), String> {
        let path = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
        if !path.is_dir() {
            return Err("open_dir needs a directory".into());
        }
        if !workspace.folders.iter().any(|f| f.path == path) {
            let patterns = |key| {
                workspace
                    .editor()
                    .settings
                    .get(key)
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            };
            workspace.folders.push(Folder {
                name: path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                path: path.clone(),
                rules: Rules::new(
                    &patterns("file_exclude_patterns"),
                    &patterns("folder_exclude_patterns"),
                    &patterns("binary_file_patterns"),
                )?,
                follow_links: true,
            });
            self.expanded.insert(path.clone());
            self.restart_index(workspace);
            self.load_directory(workspace, &path);
        }
        crate::workbench::remember(&mut workspace.recent_folders, path);
        self.side_visible = true;
        Ok(())
    }
    fn load_directory(&mut self, workspace: &Workspace, path: &Path) {
        self.request_directory(workspace, path, false);
    }
    fn request_directory(&mut self, workspace: &Workspace, path: &Path, refresh: bool) {
        if (!refresh && self.children.contains_key(path)) || !self.loading.insert(path.into()) {
            return;
        }
        let Some(folder) = workspace
            .folders
            .iter()
            .find(|f| path.starts_with(&f.path))
            .cloned()
        else {
            return;
        };
        let path = path.to_owned();
        let tx = self.directory_tx.clone();
        std::thread::spawn(move || {
            let children = sidebar_children(&folder, &path);
            let _ = tx.send((path, children));
        });
    }
    fn rebuild_side(&mut self, workspace: &Workspace) {
        fn visit(
            rows: &mut Vec<SideRow>,
            children: &BTreeMap<PathBuf, Vec<Entry>>,
            expanded: &BTreeSet<PathBuf>,
            path: &Path,
            folder: usize,
            depth: usize,
            directory: bool,
        ) {
            rows.push(SideRow {
                path: path.into(),
                folder,
                depth,
                directory,
            });
            if expanded.contains(path)
                && let Some(entries) = children.get(path)
            {
                for entry in entries {
                    visit(
                        rows,
                        children,
                        expanded,
                        &entry.path,
                        folder,
                        depth + 1,
                        entry.directory,
                    );
                }
            }
        }
        self.side_rows.clear();
        for (i, folder) in workspace.folders.iter().enumerate() {
            visit(
                &mut self.side_rows,
                &self.children,
                &self.expanded,
                &folder.path,
                i,
                0,
                true,
            );
        }
        self.expire_message(Instant::now());
        self.side_scroll.rows = open_file_count(workspace) + self.side_rows.len() + 2;
        self.side_scroll.row_height = 24. * self.ui_scale;
        self.side_selected = self
            .side_selected
            .min(self.side_rows.len().saturating_sub(1));
        if let Some(index) = self
            .sidebar_reveal
            .as_ref()
            .and_then(|path| self.side_rows.iter().position(|row| &row.path == path))
        {
            self.side_selected = index;
            self.side_open = None;
            self.side_scroll
                .reveal(open_file_count(workspace) + 2 + index);
            self.sidebar_reveal = None;
        }
    }
    /// Refreshes directory snapshots and indexes after an explicit filesystem operation.
    pub fn refresh_sidebar(&mut self, workspace: &mut Workspace) {
        self.children.retain(|path, _| path.is_dir());
        self.expanded.retain(|path| path.is_dir());
        self.restart_index(workspace);
        for path in self.expanded.clone() {
            self.request_directory(workspace, &path, true);
        }
        self.rebuild_side(workspace);
    }
    /// Reveals a document by expanding its ancestors; directory reads stay on workers.
    pub fn reveal_sidebar_path(
        &mut self,
        workspace: &mut Workspace,
        path: PathBuf,
    ) -> Result<(), String> {
        let path = path
            .canonicalize()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if !workspace
            .folders
            .iter()
            .any(|folder| path.starts_with(&folder.path))
        {
            self.add_folder(
                workspace,
                path.parent()
                    .ok_or("document has no parent folder")?
                    .to_owned(),
            )?;
        }
        let root = workspace
            .folders
            .iter()
            .filter(|f| path.starts_with(&f.path))
            .max_by_key(|f| f.path.components().count())
            .map(|f| f.path.clone())
            .ok_or("document is outside the sidebar folders")?;
        for parent in path
            .ancestors()
            .skip(1)
            .take_while(|p| p.starts_with(&root))
        {
            self.expanded.insert(parent.to_owned());
            self.request_directory(workspace, parent, false);
        }
        self.sidebar_reveal = Some(path);
        self.side_visible = true;
        self.rebuild_side(workspace);
        Ok(())
    }
    /// Expands an existing directory without treating it as a new project root.
    pub fn open_sidebar_directory(
        &mut self,
        workspace: &mut Workspace,
        path: PathBuf,
    ) -> Result<(), String> {
        if !workspace
            .folders
            .iter()
            .any(|folder| path.starts_with(&folder.path))
        {
            self.add_folder(workspace, path.clone())?;
        }
        self.expanded.insert(path.clone());
        self.request_directory(workspace, &path, false);
        self.side_visible = true;
        self.rebuild_side(workspace);
        Ok(())
    }
    /// Gives an editor context click the same input focus as an ordinary editor click.
    pub fn focus_editor_for_context(&mut self) {
        self.side_focused = false;
        self.panel_focused = false;
        self.focus.show(Surface::Panel, false);
    }
    /// Removes a project root without touching its files.
    pub fn remove_sidebar_folder(
        &mut self,
        workspace: &mut Workspace,
        path: &Path,
    ) -> Result<(), String> {
        let previous_folders = workspace.folders.clone();
        let previous_model = workspace.project_model.clone();
        let project = workspace.project_path().map(Path::to_owned);
        if let Some(project) = &project {
            // Edit the project file as written on disk: the other entries keep their
            // relative paths, names and keys.
            workspace.project_model = vel_session::project::Project::load(project)?;
        }
        crate::context_menu::apply_remove_folder(workspace, path);
        if let Some(project) = &project {
            // Folders added to this project window belong to the project; append them.
            let base = project.parent().unwrap_or(Path::new("."));
            let same = |a: &Path, b: &Path| {
                a == b
                    || std::fs::canonicalize(a)
                        .is_ok_and(|a| std::fs::canonicalize(b).ok() == Some(a))
            };
            for folder in &workspace.folders {
                if !workspace
                    .project_model
                    .folders
                    .iter()
                    .any(|entry| same(&base.join(&entry.path), &folder.path))
                {
                    workspace
                        .project_model
                        .folders
                        .push(vel_session::project::Folder {
                            path: folder.path.clone(),
                            name: Some(folder.name.clone()),
                            folder_exclude_patterns: vec![],
                            file_exclude_patterns: vec![],
                            binary_file_patterns: vec![],
                            follow_symlinks: (!folder.follow_links).then_some(false),
                            ..Default::default()
                        });
                }
            }
        }
        if let Some(project) = project
            && let Err(error) = workspace.project_model.save(&project)
        {
            workspace.folders = previous_folders;
            workspace.project_model = previous_model;
            return Err(error);
        }
        // An already-running worker may have read the previous project. Follow it
        // with an explicit reload so it cannot leave a detached root installed.
        workspace.request_resource_reload();
        self.children.retain(|folder, _| !folder.starts_with(path));
        self.expanded.retain(|folder| !folder.starts_with(path));
        self.restart_index(workspace);
        self.rebuild_side(workspace);
        Ok(())
    }
    /// Shared native/headless keyboard route: focused widgets, Vintage, then the keymap.
    pub fn resolve_key(
        &mut self,
        workspace: &mut Workspace,
        key: &Key,
        viewport: Range<usize>,
    ) -> Result<vel_config::keymap::Resolution, String> {
        use vel_config::keymap::Resolution;
        if self.input().is_some_and(Input::composing) {
            return Ok(Resolution::Unbound);
        }
        let (handled, invocation) = self.key(workspace, key, viewport.clone())?;
        if handled {
            return Ok(invocation.map_or(Resolution::Pending, Resolution::Command));
        }
        if self.input().is_some()
            && key.modifiers & 11 == 0
            && (key.name.chars().count() == 1 || key.name == "space")
        {
            return Ok(Resolution::Unbound);
        }
        let editor = workspace.editor_mut();
        editor.ui_panel_visible = self.panel.is_some();
        editor.ui_overlay_visible = self.overlay.is_some();
        editor.ui_panel = self.panel.map_or("", |panel| match panel {
            Panel::Find => "find",
            Panel::Replace => "replace",
            Panel::Incremental => "incremental_find",
            Panel::Files => "find_in_files",
            Panel::Input => "input",
            Panel::Output => "output.exec",
            Panel::Results => "output.find_results",
        });
        editor.ui_panel_focused = self.panel.is_some() && self.panel_focused;
        let result = editor.resolve_key(key);
        self.sync_vintage_prompt(workspace, viewport)?;
        Ok(result)
    }
    /// Opens the ordinary buffer-backed input panel for the engine's Ex/search prompt.
    pub fn sync_vintage_prompt(
        &mut self,
        workspace: &mut Workspace,
        viewport: Range<usize>,
    ) -> Result<(), String> {
        let sheet = workspace.tabs[workspace.active].sheet;
        if let Some(owner) = self.vintage_prompt
            && (owner != sheet
                || self.panel != Some(Panel::Input)
                || self.overlay.is_some()
                || workspace
                    .editor()
                    .vintage
                    .as_ref()
                    .and_then(|v| v.engine.command_line())
                    .is_none())
        {
            self.vintage_prompt = None;
            if let Some(tab) = workspace.tabs.iter_mut().find(|t| t.sheet == owner)
                && tab
                    .editor
                    .vintage
                    .as_ref()
                    .and_then(|v| v.engine.command_line())
                    .is_some()
            {
                tab.editor.key(&Key::parse("escape")?)?;
            }
            if self.panel == Some(Panel::Input) && self.overlay.is_none() {
                self.close_panel();
            }
        }
        if self.vintage_prompt.is_none()
            && let Some(prompt) = workspace
                .editor()
                .vintage
                .as_ref()
                .and_then(|v| v.engine.command_line())
        {
            let caption = match prompt.chars().next() {
                Some('/') => "Search forward /",
                Some('?') => "Search backward ?",
                _ => ":",
            };
            self.action(
                workspace,
                "show_input_panel",
                &json!({
                    "caption": caption, "initial_text": &prompt[1..],
                    "on_done": {"command":"vintage_submit_prompt", "args":{}}
                }),
                viewport,
            )?;
            self.vintage_prompt = Some(sheet);
        }
        Ok(())
    }
    pub fn input(&self) -> Option<&Input> {
        if self.overlay.is_some() {
            Some(&self.quick_input)
        } else if self.panel_focused
            && matches!(
                self.panel,
                Some(
                    Panel::Find | Panel::Replace | Panel::Incremental | Panel::Files | Panel::Input
                )
            )
        {
            Some(match self.focus_input {
                1 => &self.replace,
                2 => &self.where_input,
                _ => &self.query,
            })
        } else {
            None
        }
    }
    pub fn input_mut(&mut self) -> Option<&mut Input> {
        if self.overlay.is_some() {
            Some(&mut self.quick_input)
        } else if self.panel_focused
            && matches!(
                self.panel,
                Some(
                    Panel::Find | Panel::Replace | Panel::Incremental | Panel::Files | Panel::Input
                )
            )
        {
            Some(match self.focus_input {
                1 => &mut self.replace,
                2 => &mut self.where_input,
                _ => &mut self.query,
            })
        } else {
            None
        }
    }
    fn captured(&self) -> bool {
        self.overlay.is_some() || self.panel_focused || self.side_focused
    }
    pub fn text_input(
        &mut self,
        workspace: &mut Workspace,
        input: TextInput,
        viewport: Range<usize>,
    ) -> bool {
        if let Some(field) = self.input_mut() {
            field.text_input(input);
            self.input_changed(workspace, viewport);
            true
        } else {
            self.captured()
        }
    }
    fn input_changed(&mut self, workspace: &mut Workspace, viewport: Range<usize>) {
        if self.overlay.is_some() {
            if !self.quick_input.composing() {
                self.query_quick(workspace);
            }
        } else if self.focus_input == 0
            && self.panel != Some(Panel::Input)
            && !self.query.composing()
        {
            self.refresh_find_input(workspace, viewport);
        } else if self.focus_input == 1 {
            self.history_cursor[1] = None;
        }
    }
    fn refresh_find_input(&mut self, workspace: &mut Workspace, viewport: Range<usize>) {
        let changed = self.search_identity.as_ref().is_none_or(|key| {
            key.2 != self.query.value() || key.3 != self.options || key.4 != self.in_selection
        });
        if !changed {
            return;
        }
        self.history_cursor[0] = None;
        self.refresh_find(workspace, viewport);
        if !self.in_selection
            && matches!(self.panel, Some(Panel::Find | Panel::Incremental))
            && self.search.is_some()
        {
            let origin = self
                .find_origin
                .filter(|(id, _)| *id == workspace.editor().buffer.id())
                .map_or(workspace.editor().caret, |(_, at)| at);
            self.pending_navigation = Some(FindNavigation::Preview(origin));
        }
    }
    pub fn edited(&mut self, workspace: &mut Workspace, viewport: Range<usize>) {
        self.input_changed(workspace, viewport);
    }
    pub fn busy(&self) -> bool {
        self.review_busy()
            || self.overlay.is_some()
            || self.intel.busy()
            || (!self.hover_term.is_empty() && !self.hover_requested)
            || self.file_task.is_some()
            || self.replace_rx.is_some()
            || self
                .output_editor
                .as_ref()
                .is_some_and(|e| e.language.borrow().as_ref().is_some_and(|l| l.busy()))
            || (self.search.is_some() && !self.all_matches_ready)
    }
    pub fn ranking_pending(&self) -> bool {
        self.overlay.is_some() && self.quick_pending
    }
    pub fn seed_benchmark(workspace: &mut Workspace) {
        workspace.palette = (0..100_000)
            .map(|i| vel_config::ui::PaletteItem {
                caption: format!(
                    "Project: Open {} {i:05}",
                    ["MapReader", "main", "config", "parser"][i % 4]
                ),
                invocation: Invocation {
                    command: "show_panel".into(),
                    args: json!({"panel":"find","text":format!("{i}")}),
                },
            })
            .collect();
    }
    pub fn benchmark_step(
        &mut self,
        workspace: &mut Workspace,
        index: usize,
        viewport: Range<usize>,
    ) -> Result<(), String> {
        match index % 10 {
            0 => {
                self.action(
                    workspace,
                    "show_overlay",
                    &json!({"overlay":"command_palette"}),
                    viewport,
                )?;
            }
            1..=3 => {
                self.text_input(
                    workspace,
                    TextInput::Insert {
                        text: ["m", "a", "p"][index % 10 - 1].into(),
                        replacement: None,
                    },
                    viewport,
                );
            }
            4 => {
                self.key(workspace, &Key::parse("down")?, viewport)?;
            }
            5 => {
                self.key(workspace, &Key::parse("escape")?, viewport)?;
            }
            6 => {
                self.action(
                    workspace,
                    "show_panel",
                    &json!({"panel":"find","text":"let"}),
                    viewport,
                )?;
            }
            7 => {
                self.text_input(
                    workspace,
                    TextInput::Insert {
                        text: "n".into(),
                        replacement: None,
                    },
                    viewport,
                );
            }
            8 => {
                self.action(
                    workspace,
                    "show_panel",
                    &json!({"panel":"replace","text":"let"}),
                    viewport,
                )?;
                self.replace.set("const");
            }
            _ => {
                self.action(workspace, "hide_panel", &json!({}), viewport.clone())?;
                self.action(workspace, "toggle_side_bar", &json!({}), viewport)?;
            }
        }
        Ok(())
    }
    fn decorate_locations(&mut self) {
        for item in &mut self.quick.items {
            if let Some(location) = self.locations.get(item.value) {
                item.kind = format!("{:?}", location.kind);
                item.annotation = format!(
                    "{}:{}",
                    location
                        .file
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy(),
                    location.row + 1
                );
                item.details = location.file.display().to_string();
            }
        }
    }
    fn location_items(&mut self) {
        self.quick.set_items(
            self.locations
                .iter()
                .enumerate()
                .map(|(i, l)| Item {
                    caption: l.name.clone(),
                    value: i,
                    kind: format!("{:?}", l.kind),
                    annotation: format!(
                        "{}:{}",
                        l.file.file_name().unwrap_or_default().to_string_lossy(),
                        l.row + 1
                    ),
                    details: l.file.display().to_string(),
                    highlights: Vec::new(),
                })
                .collect(),
        );
    }
    pub fn hover_at(&mut self, workspace: &Workspace, point: Point, byte: usize) {
        if !workspace.editor().settings.bool("show_definitions", true) {
            self.hover.clear();
            self.hover_html = None;
            self.hover_term.clear();
            return;
        }
        if let Some(popup) = &mut self.hover_html {
            popup.mouse_move(point);
            if popup.visible {
                return;
            }
            self.hover.clear();
            self.hover_html = None;
        }
        if contains(self.hover_bounds, point) && !self.hover.is_empty() {
            return;
        }
        let term = if workspace.editor().settings.bool("show_definitions", true)
            && self.overlay.is_none()
            && workspace.editor().marked.is_none()
        {
            crate::intelligence::word(workspace.editor(), byte)
        } else {
            String::new()
        };
        if term != self.hover_term {
            self.hover.clear();
            self.hover_html = None;
            self.hover_term = term;
            self.hover_since = Instant::now();
            self.hover_requested = false;
        }
        self.hover_point = point;
    }
    pub fn click_definition(
        &mut self,
        workspace: &mut Workspace,
        point: Point,
    ) -> Result<bool, String> {
        if !self.hover.is_empty() && contains(self.hover_bounds, point) {
            let target = self.hover_html.as_ref().and_then(|p| p.html.link_at(point));
            let i = target
                .and_then(|target| match target {
                    vel_minihtml::LinkTarget::Callback(url) => {
                        url.strip_prefix("definition:")?.parse().ok()
                    }
                    _ => None,
                })
                .unwrap_or(0);
            if let Some(location) = self.hover.get(i).cloned() {
                self.intel.remember(crate::intelligence::capture(workspace));
                crate::intelligence::jump(workspace, &location, false)?;
            }
            self.hover.clear();
            self.hover_html = None;
            return Ok(true);
        }
        Ok(false)
    }
    pub fn action(
        &mut self,
        workspace: &mut Workspace,
        name: &str,
        args: &Value,
        viewport: Range<usize>,
    ) -> Result<bool, String> {
        if workspace.editor().binary.is_some() && name == "goto_line" {
            return self.action(workspace, "show_input_panel", &json!({"caption":"Goto byte offset (decimal or 0x hex)","initial_text":format!("0x{:x}", workspace.editor().caret),"on_done":{"command":"goto_hex_offset","args":{}}}), viewport);
        }
        if self.review_action(workspace, name, args)? {
            return Ok(true);
        }
        match name {
            "goto_symbol_in_project" => {
                self.action(workspace, "show_overlay", &json!({}), viewport)?;
                self.overlay = Some(Overlay::ProjectSymbols);
                self.prepared.clear();
                self.locations.clear();
                self.query_quick(workspace);
            }
            "goto_definition" | "goto_reference" => {
                self.hover.clear();
                let term = crate::intelligence::word(workspace.editor(), workspace.editor().caret);
                if !term.is_empty() {
                    self.navigation_origin = Some(crate::intelligence::capture(workspace));
                    self.intel.query(
                        if name == "goto_definition" {
                            crate::intelligence::Query::Definition(term)
                        } else {
                            crate::intelligence::Query::Reference(term)
                        },
                        workspace,
                    );
                }
            }
            "jump_back" | "jump_forward" => {
                self.cancel_overlay(workspace);
                self.intel.history(workspace, name == "jump_forward")?;
                self.restore_scroll = Some(workspace.tabs[workspace.active].scroll);
            }
            "show_indexing_status" => {
                self.output = crate::intelligence::status_text(
                    &self
                        .intel
                        .projects
                        .iter()
                        .map(|p| p.index.clone())
                        .collect::<Vec<_>>(),
                );
                self.panel = Some(Panel::Output);
                self.panel_focused = true;
            }
            "set_file_type" => {
                workspace.set_file_type(
                    args["syntax"]
                        .as_str()
                        .ok_or("set_file_type needs syntax")?,
                )?;
            }
            "toggle_minimap" => {
                let editor = workspace.editor_mut();
                let value = editor.settings.bool("show_minimap", true);
                editor.settings.set(
                    vel_config::settings::Layer::View,
                    "show_minimap",
                    json!(!value),
                );
                editor.sync();
            }
            "show_overlay" | "goto_line" => {
                self.cancel_overlay(workspace);
                self.overlay = Some(if args["overlay"] == "syntax" {
                    Overlay::Syntax
                } else if args["overlay"] == "command_palette" {
                    Overlay::Palette
                } else {
                    Overlay::Goto
                });
                self.preview = Some(Preview {
                    sheet: workspace.tabs[workspace.active].sheet,
                    scroll: workspace.tabs[workspace.active].scroll,
                    transient: workspace.tabs[workspace.active].transient,
                    selection: workspace
                        .editor()
                        .buffer
                        .view(workspace.editor().id)
                        .sel()
                        .clone(),
                });
                // Keep a pre-existing side-bar preview alive while Goto reuses its own transient tab.
                workspace.tabs[workspace.active].transient = false;
                self.focus.show(Surface::Overlay, true);
                self.focus.set(Surface::Overlay, 1);
                self.side_focused = false;
                self.quick_input.set(if name == "goto_line" {
                    ":"
                } else {
                    args["text"].as_str().unwrap_or("")
                });
                // Keep @/: prefixes while placing the caret after them.
                self.quick_input.command(Command::End(false));
                self.last_preview = None;
                self.quick.set_items(Vec::new());
                self.prepared.clear();
                self.query_quick(workspace);
            }
            "show_panel" | "find_in_files" | "show_input_panel" | "show_output_panel" => {
                if name == "show_panel"
                    && args["toggle"].as_bool() == Some(true)
                    && self.panel_focused
                    && self.panel
                        == Some(Self::panel_kind(args["panel"].as_str().unwrap_or("find")))
                {
                    self.close_panel();
                    return Ok(true);
                }
                self.cancel_overlay(workspace);
                let previous_panel = self.panel;
                let panel = args["panel"]
                    .as_str()
                    .unwrap_or(if name == "find_in_files" {
                        "find_in_files"
                    } else {
                        "find"
                    });
                self.panel = Some(if name == "show_input_panel" {
                    Panel::Input
                } else if name == "show_output_panel" {
                    Panel::Output
                } else {
                    Self::panel_kind(panel)
                });
                if self.panel == Some(Panel::Incremental) && previous_panel != self.panel {
                    let editor = workspace.editor();
                    self.incremental_origin = Some((
                        Preview {
                            sheet: workspace.tabs[workspace.active].sheet,
                            selection: editor.buffer.view(editor.id).sel().clone(),
                            scroll: workspace.tabs[workspace.active].scroll,
                            transient: workspace.tabs[workspace.active].transient,
                        },
                        editor.buffer.id(),
                        editor.buffer.change_id(),
                    ));
                } else if self.panel != Some(Panel::Incremental) {
                    self.incremental_origin = None;
                }
                self.find_origin = Some((
                    workspace.editor().buffer.id(),
                    workspace.editor().selection().start,
                ));
                self.history_cursor = [None, None];
                self.side_focused = false;
                self.panel_focused = true;
                self.focus_input = 0;
                self.focus.show(Surface::Panel, true);
                self.focus.set(Surface::Panel, 2);
                self.selection_scope = workspace
                    .editor()
                    .regions()
                    .iter()
                    .filter(|r| !r.empty())
                    .map(|r| r.to_range())
                    .collect();
                self.scope_revision = Some((
                    workspace.editor().buffer.id(),
                    workspace.editor().buffer.change_id(),
                ));
                if name == "show_input_panel" {
                    self.input_caption = args["caption"].as_str().unwrap_or("Input").into();
                    self.query.set(args["initial_text"].as_str().unwrap_or(""));
                    self.input_done = args
                        .get("on_done")
                        .cloned()
                        .map(serde_json::from_value)
                        .transpose()
                        .map_err(|e| e.to_string())?;
                } else if name == "show_output_panel" {
                    self.output = args["text"].as_str().unwrap_or("").into();
                } else {
                    if let Some(query) = args["text"].as_str() {
                        self.query.set(query);
                    } else if previous_panel != self.panel
                        && workspace.editor().settings.bool("find_selected_text", true)
                        && !workspace.editor().selection().is_empty()
                    {
                        let selected = workspace
                            .editor()
                            .text
                            .slice(workspace.editor().selection());
                        if !selected.chars().any(|c| c == '\n') {
                            self.query.set(&selected.to_string());
                        }
                    }
                    if workspace
                        .editor()
                        .settings
                        .bool("auto_find_in_selection", false)
                    {
                        self.in_selection = self.selection_scope.iter().any(|r| {
                            workspace.editor().text.byte_to_line(r.start)
                                != workspace.editor().text.byte_to_line(r.end)
                        });
                    }
                    self.query.run("select_all", &json!({}))?;
                    if self.panel == Some(Panel::Replace) {
                        self.replace.run("select_all", &json!({}))?;
                    }
                    self.refresh_find(workspace, viewport);
                }
            }
            "hide_panel" => {
                if args["cancel"].as_bool() == Some(true) {
                    self.cancel_incremental(workspace);
                }
                self.close_panel();
            }
            "toggle_side_bar" => {
                self.side_visible = !self.side_visible;
                self.side_focused = false;
            }
            "focus_side_bar" => {
                self.panel_focused = false;
                self.focus.show(Surface::Panel, false);
                self.side_visible = true;
                self.side_focused = true;
                self.side_open = Some(workspace.active);
                self.rebuild_side(workspace);
            }
            "open_dir" | "add_folder" => {
                self.add_folder(
                    workspace,
                    PathBuf::from(
                        args["dir"]
                            .as_str()
                            .ok_or("open_dir/add_folder needs dir")?,
                    ),
                )?;
            }

            "toggle_find_option" => {
                match args["option"]
                    .as_str()
                    .ok_or("toggle_find_option needs option")?
                {
                    "regex" => self.options.regex = !self.options.regex,
                    "case_sensitive" => self.options.case_sensitive = !self.options.case_sensitive,
                    "whole_word" => self.options.whole_word = !self.options.whole_word,
                    "wrap" => self.options.wrap = !self.options.wrap,
                    "preserve_case" => self.options.preserve_case = !self.options.preserve_case,
                    "in_selection" => self.in_selection = !self.in_selection,
                    _ => return Err("unknown find option".into()),
                }
                self.refresh_find_input(workspace, viewport);
            }
            "slurp_find_string" | "slurp_replace_string" | "find_under" | "find_under_prev" => {
                let editor = workspace.editor_mut();
                if editor.selection().is_empty() {
                    // Expand every empty caret while retaining the primary region and the others.
                    if editor.regions().len() == 1 {
                        editor.select_word(editor.caret);
                    } else {
                        let primary_at = editor.caret;
                        editor.run("find_under_expand", &json!({}))?;
                        let mut regions = editor.regions();
                        if let Some(i) = regions
                            .iter()
                            .position(|r| r.begin() <= primary_at && r.end() >= primary_at)
                        {
                            let primary = regions.remove(i);
                            regions.push(primary);
                            editor.set_selection(Selection::from_regions(regions));
                        }
                    }
                }
                let value = editor.text.slice(editor.selection()).to_string();
                if name == "slurp_replace_string" {
                    self.replace.set(&value);
                } else {
                    self.query.set(&value);
                    self.refresh_find(workspace, viewport);
                }
                if matches!(name, "find_under" | "find_under_prev") {
                    self.remember_search(false);
                    self.find_next(workspace, name.ends_with("prev"), false);
                }
            }
            "find_next" | "find_prev" => {
                self.ensure_find(workspace, viewport);
                self.remember_search(false);
                self.find_next(workspace, name == "find_prev", false);
            }
            "find_all" => {
                if self.panel == Some(Panel::Files) {
                    return Ok(true);
                }
                self.remember_search(false);
                if self.in_selection && self.selection_scope.is_empty() {
                    return Ok(true);
                }
                self.ensure_find(workspace, viewport);
                if let Some(search) = &self.search {
                    let hits = search.find(&workspace.editor().text, &self.scope());
                    let editor = workspace.editor_mut();
                    text::select_all(&mut editor.buffer, editor.id, &hits);
                    editor.sync();
                    self.close_panel();
                }
            }
            "replace_next" | "replace_all" => {
                self.replace_document(workspace, name == "replace_all", viewport)?;
            }
            "find_in_files_start" => {
                self.start_files(workspace)?;
            }
            "set_indentation" => {
                self.set_indentation(workspace, args)?;
            }
            "replace_in_files" => {
                self.prepare_files(workspace)?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
    /// Find-family panels show a close button; Escape and a toggling Cmd+F also close them.
    fn find_panel_visible(&self) -> bool {
        matches!(
            self.panel,
            Some(Panel::Find | Panel::Replace | Panel::Incremental | Panel::Files)
        )
    }
    fn close_button_rect(&self) -> Rect {
        Rect {
            x: self.panel_bounds.x + self.panel_bounds.width
                - ((CLOSE_BUTTON + 8.) * self.ui_scale),
            y: self.panel_bounds.y + (11. * self.ui_scale),
            width: CLOSE_BUTTON * self.ui_scale,
            height: CLOSE_BUTTON * self.ui_scale,
        }
    }
    fn panel_kind(panel: &str) -> Panel {
        match panel {
            "replace" => Panel::Replace,
            "incremental_find" => Panel::Incremental,
            "find_in_files" => Panel::Files,
            "find_results" => Panel::Results,
            "output.exec" => Panel::Output,
            _ => Panel::Find,
        }
    }
    /// Find matches to draw in the active view. Matches stay computed after the panel closes so
    /// `find_next`/`find_prev` keep working, but like Sublime they are only drawn while a find
    /// panel is visible.
    pub fn visible_highlights(&self) -> &[Range<usize>] {
        if self.find_panel_visible() {
            &self.highlights
        } else {
            &[]
        }
    }
    fn close_panel(&mut self) {
        if self.panel == Some(Panel::Results) && self.file_task.take().is_some() {
            self.file_complete = false;
            self.result_status = format!(
                "Search cancelled · {} matches in {} files",
                self.file_matches,
                self.files_matched.len()
            );
            self.set_message(self.result_status.clone());
        }
        self.pending_navigation = None;
        self.incremental_origin = None;
        self.panel = None;
        self.panel_focused = false;
        self.focus.show(Surface::Panel, false);
        self.focus_input = 0;
    }
    fn scope(&self) -> Vec<Range<usize>> {
        if self.in_selection {
            if self.selection_scope.is_empty() {
                std::iter::once(0..0).collect()
            } else {
                self.selection_scope.clone()
            }
        } else {
            vec![]
        }
    }
    fn ensure_find(&mut self, workspace: &mut Workspace, viewport: Range<usize>) {
        let key = (
            workspace.editor().buffer.id(),
            workspace.editor().revision,
            self.query.value(),
            self.options,
            self.in_selection,
        );
        if self.search_identity.as_ref() != Some(&key) {
            self.refresh_find(workspace, viewport);
        } else if let Some(error) = self.find_error.clone() {
            self.set_message(error);
        }
    }
    fn refresh_find(&mut self, workspace: &mut Workspace, viewport: Range<usize>) {
        let viewport = workspace.editor().text.floor_char_boundary(viewport.start)
            ..workspace.editor().text.floor_char_boundary(viewport.end);
        let buffer = &workspace.editor().buffer;
        if let Some((id, revision)) = self.scope_revision {
            if id == buffer.id() {
                self.selection_scope = self
                    .selection_scope
                    .iter()
                    .filter_map(|r| {
                        buffer
                            .transform_region_from(Region::from(r.clone()), revision)
                            .map(|r| r.to_range())
                    })
                    .collect();
            } else {
                self.selection_scope.clear();
            }
        }
        self.scope_revision = Some((buffer.id(), buffer.change_id()));
        self.search_identity = Some((
            workspace.editor().buffer.id(),
            workspace.editor().revision,
            self.query.value(),
            self.options,
            self.in_selection,
        ));
        self.search_worker.cancel();
        self.highlights.clear();
        self.all_matches_ready = false;
        self.pending_navigation = None;
        match Search::new(&self.query.value(), self.options) {
            Ok(search) => {
                if self.find_error.take().as_ref() == Some(&self.message) {
                    self.set_message(String::new());
                }
                if self.in_selection && self.selection_scope.is_empty() {
                    self.message = "No selection".into();
                    self.all_matches_ready = true;
                    self.search = Some(search);
                    return;
                }
                let scopes = self.scope();
                let ranges = if self.in_selection {
                    scopes
                        .iter()
                        .map(|r| r.start.max(viewport.start)..r.end.min(viewport.end))
                        .filter(|r| r.start <= r.end)
                        .collect::<Vec<_>>()
                } else {
                    std::iter::once(viewport).collect()
                };
                if !ranges.is_empty() {
                    self.highlights = search.find(&workspace.editor().text, &ranges);
                }
                self.search_worker.search(
                    search.clone(),
                    workspace.editor().text.clone(),
                    workspace.editor().revision,
                    scopes,
                );
                self.search = Some(search);
            }
            Err(error) => {
                self.find_error = Some(error.clone());
                self.set_message(error);
                self.search = None;
                self.all_matches_ready = true;
            }
        }
    }
    fn find_next(&mut self, workspace: &mut Workspace, backwards: bool, close: bool) {
        self.navigate_find(workspace, backwards, close, true);
    }
    fn navigate_find(
        &mut self,
        workspace: &mut Workspace,
        backwards: bool,
        close: bool,
        report: bool,
    ) {
        if report
            && self.search.is_none()
            && let Some(error) = self.find_error.clone()
        {
            self.set_message(error);
        }
        if self.search.is_some() && !self.all_matches_ready {
            self.pending_navigation = Some(FindNavigation::Next {
                backwards,
                close,
                report,
            });
            return;
        }
        if let Some(search) = &self.search {
            let current = workspace.editor().selection();
            let next = search.next(
                &self.highlights,
                workspace.editor().caret,
                Some(&current),
                backwards,
            );
            if let Some(range) = next {
                workspace
                    .editor_mut()
                    .set_selection(Selection::from_regions([Region::from(range)]));
                self.reveal = true;
            } else if report && !search.query.is_empty() {
                self.set_message(format!("Unable to find {}", search.query));
            }
        }
        if close {
            self.close_panel();
        }
    }
    fn replace_document(
        &mut self,
        workspace: &mut Workspace,
        all: bool,
        viewport: Range<usize>,
    ) -> Result<(), String> {
        self.remember_search(true);
        if workspace.editor().read_only {
            return Err("document is read only".into());
        }
        if self.in_selection && self.selection_scope.is_empty() {
            return Ok(());
        }
        self.ensure_find(workspace, viewport.clone());
        let Some(search) = &self.search else {
            return Ok(()); // Compilation feedback is inline/status-only.
        };
        let mut replacements = search.replacements(
            &workspace.editor().text,
            &self.replace.value(),
            &self.scope(),
        );
        if !all {
            let selection = workspace.editor().selection();
            if let Some(current) = replacements.iter().find(|r| r.range == selection).cloned() {
                replacements = vec![current];
            } else {
                self.find_next(workspace, false, false);
                return Ok(());
            }
        }
        let count = replacements.len();
        let editor = workspace.editor_mut();
        text::apply(&mut editor.buffer, editor.id, &replacements);
        editor.sync();
        self.refresh_find(workspace, viewport);
        if !all {
            self.navigate_find(workspace, false, false, false);
        }
        self.set_message(format!(
            "Replaced {count} occurrence{}",
            if count == 1 { "" } else { "s" }
        ));
        Ok(())
    }
    fn query_quick(&mut self, workspace: &mut Workspace) {
        let value = self.quick_input.value();
        let query = Query::parse(&value);
        let target = match self.overlay {
            Some(Overlay::Goto) if query.file.is_empty() => match query.target {
                Target::Symbol(_) => "symbol",
                Target::Word(_) => "word",
                Target::Line(_, _) => "line",
                Target::File => "files",
            },
            Some(Overlay::Goto) => "files",
            Some(Overlay::Palette) => "palette",
            Some(Overlay::Syntax) => "syntax",
            Some(Overlay::Choices) => "choices",
            Some(Overlay::Locations) => "locations",
            Some(Overlay::ProjectSymbols) => "project_symbols",
            None => return,
        };
        if self.quick_target != target {
            self.quick_target = target;
            self.quick_ready = false;
            self.prepared.clear();
            self.local_generation += 1;
            self.local = None;
            self.candidates = Arc::default();
            self.paths = Arc::default();
            self.local_positions.clear();
            self.quick.set_items(Vec::new());
        } else if self.quick_query != value {
            // A new query always starts at its best hit. Source-only refreshes retain selection.
            self.quick.set_items(Vec::new());
        }
        self.quick_query = value;
        // Symbol sources change on an edit or when analysis publishes, not on every frame.
        if self.overlay == Some(Overlay::Goto) {
            let query = Query::parse(&self.quick_input.value());
            if query.file.is_empty() && matches!(query.target, Target::Symbol(_)) {
                let source = self.local_source(workspace, "symbol");
                if self.prepared == source
                    && self.quick_revision == self.quick_input.buffer.change_id().0
                {
                    return;
                }
            }
        }
        self.fuzzy_worker.cancel();
        if self.overlay == Some(Overlay::ProjectSymbols) {
            self.quick_pending = true;
            self.intel.query(
                crate::intelligence::Query::Symbols(self.quick_input.value()),
                workspace,
            );
            return;
        }
        if self.overlay == Some(Overlay::Locations) {
            self.quick_pending = true;
            self.quick_ready = true;
            self.candidates = Arc::new(
                self.locations
                    .iter()
                    .map(|l| fuzzy::Candidate::new(&l.name))
                    .collect(),
            );
            self.fuzzy_worker.set_items(self.candidates.clone());
            self.fuzzy_worker.query(self.quick_input.value(), 100);
            return;
        }
        self.quick_pending = true;
        self.quick_revision = self.quick_input.buffer.change_id().0;
        if matches!(
            self.overlay,
            Some(Overlay::Palette | Overlay::Syntax | Overlay::Choices)
        ) {
            let source = if self.overlay == Some(Overlay::Syntax) {
                "syntax"
            } else {
                "palette"
            };
            if self.prepared == source {
                self.fuzzy_worker.query(self.quick_input.value(), 100);
                return;
            }
            self.prepared = source.into();
            self.quick_ready = true;
            let palette = if self.overlay == Some(Overlay::Choices) {
                &self.choice_items
            } else {
                &workspace.palette
            };
            self.palette_indices = palette
                .iter()
                .enumerate()
                .filter(|(i, p)| {
                    if self.overlay == Some(Overlay::Syntax)
                        && p.invocation.command != "set_file_type"
                    {
                        return false;
                    }
                    (self.overlay != Some(Overlay::Palette)
                        || self.palette_command_enabled(workspace, *i))
                        && workspace
                            .editor()
                            .registry
                            .kind(&p.invocation.command)
                            .is_some()
                })
                .map(|(i, _)| i)
                .collect();
            self.candidates = Arc::new(
                self.palette_indices
                    .iter()
                    .map(|i| {
                        let p = &palette[*i];
                        let mut c = fuzzy::Candidate::new(&p.caption);
                        c.recent = self
                            .palette_recent
                            .get(&format!("{}:{}", p.invocation.command, p.invocation.args))
                            .copied()
                            .unwrap_or(0);
                        c
                    })
                    .collect(),
            );
            self.local_positions.clear();
            self.fuzzy_worker.set_items(self.candidates.clone());
            self.fuzzy_worker.query(self.quick_input.value(), 100);
            return;
        }
        let query = Query::parse(&self.quick_input.value());
        if query.file.is_empty() && matches!(query.target, Target::Word(_) | Target::Symbol(_)) {
            let kind = if matches!(query.target, Target::Word(_)) {
                "word"
            } else {
                "symbol"
            };
            let source = self.local_source(workspace, kind);
            if self.prepared == source {
                if self.local.is_none() && self.quick_ready {
                    let term = match &query.target {
                        Target::Word(s) | Target::Symbol(s) => s.as_str(),
                        _ => "",
                    };
                    self.fuzzy_worker.query(term, 100);
                }
                return;
            }
            self.prepared = source;
            self.quick_ready = false;
            self.local_generation += 1;
            self.local = None;
        }
        if query.file.is_empty() {
            match &query.target {
                Target::Line(row, column) => {
                    self.quick_pending = false;
                    workspace.tabs[workspace.active].goto(&FileLocation {
                        path: PathBuf::new(),
                        line: *row,
                        column: *column,
                    });
                    return;
                }
                Target::Symbol(term) => {
                    let language = workspace.editor().language.borrow();
                    let analysis = language.as_ref().and_then(|l| l.analysis.as_ref());
                    let symbols = analysis.map(|a| a.symbols.clone()).unwrap_or_default();
                    self.message = if analysis.is_none() {
                        "Indexing symbols…".into()
                    } else {
                        String::new()
                    };
                    if analysis.is_some() {
                        self.set_local(symbols, term);
                    }
                    return;
                }
                Target::Word(_) => {
                    let snapshot = workspace.editor().text.clone();
                    let generation = self.local_generation;
                    let (tx, rx) = mpsc::channel();
                    std::thread::spawn(move || {
                        let mut words = Vec::new();
                        let mut offset = 0;
                        for line in snapshot.lines() {
                            let text = line.to_string();
                            let mut start = None;
                            for (i, c) in text
                                .char_indices()
                                .chain(std::iter::once((text.len(), ' ')))
                            {
                                if c.is_alphanumeric() || c == '_' {
                                    start.get_or_insert(i);
                                } else if let Some(begin) = start.take() {
                                    words.push((text[begin..i].into(), offset + begin));
                                }
                            }
                            offset += text.len() + 1;
                        }
                        let _ = tx.send(Local::Words(generation, words));
                    });
                    self.local = Some(rx);
                    return;
                }
                Target::File => {}
            }
        }
        if self.prepared == "files" {
            self.fuzzy_worker.query(query.file, 100);
            return;
        }
        self.prepared = "files".into();
        self.quick_ready = true;
        self.local_generation += 1;
        self.local = None;
        let mut extra = Vec::new();
        for tab in &workspace.tabs {
            if let Some(path) = &tab.path
                && self.file_paths.binary_search(path).is_err()
                && !extra.contains(path)
            {
                extra.push(path.clone());
            }
        }
        self.paths = if extra.is_empty() {
            self.file_paths.clone()
        } else {
            let mut paths = self.file_paths.as_ref().clone();
            paths.extend(extra);
            Arc::new(paths)
        };
        self.local_positions.clear();
        let indexed = self.file_paths.len();
        self.candidates = if self.paths.len() == indexed && self.recency.is_empty() {
            self.file_candidates.clone()
        } else {
            let mut candidates = self.file_candidates.as_ref().clone();
            for path in &self.paths[indexed..] {
                candidates.push(fuzzy::Candidate::new(path.to_string_lossy()));
            }
            for (i, path) in self.paths.iter().enumerate() {
                candidates[i].recent = self.recency.get(path).copied().unwrap_or(0);
            }
            Arc::new(candidates)
        };
        self.fuzzy_worker.set_items(self.candidates.clone());
        self.fuzzy_worker.query(query.file, 100);
    }
    fn publish_quick(&mut self, workspace: &Workspace, hits: Vec<fuzzy::Hit>) {
        let palette = matches!(
            self.overlay,
            Some(Overlay::Palette | Overlay::Syntax | Overlay::Choices)
        );
        let palette_items = if self.overlay == Some(Overlay::Choices) {
            &self.choice_items
        } else {
            &workspace.palette
        };
        let items = hits
            .into_iter()
            .map(|hit| {
                let index = if palette {
                    self.palette_indices[hit.index]
                } else {
                    hit.index
                };
                Item {
                    caption: self.candidates[hit.index].label.clone(),
                    highlights: hit.positions,
                    value: index,
                    kind: if palette {
                        "command"
                    } else if !self.local_positions.is_empty() {
                        "symbol/word"
                    } else {
                        "file"
                    }
                    .into(),
                    annotation: if palette {
                        workspace
                            .editor()
                            .keymap
                            .hint(&palette_items[index].invocation)
                            .unwrap_or_default()
                    } else {
                        String::new()
                    },
                    details: if palette {
                        palette_items[index].invocation.command.clone()
                    } else {
                        self.paths
                            .get(hit.index)
                            .map(|p| p.display().to_string())
                            .unwrap_or_default()
                    },
                }
            })
            .collect();
        self.quick.set_items(items);
        if self.overlay == Some(Overlay::Locations) {
            self.decorate_locations();
        }
    }
    fn local_source(&self, workspace: &Workspace, kind: &str) -> String {
        format!(
            "{kind}:{:?}:{}:{}",
            workspace.editor().buffer.id(),
            workspace.editor().revision,
            workspace
                .editor()
                .language
                .borrow()
                .as_ref()
                .and_then(|l| l.analysis.as_ref())
                .map(|a| a.version.0)
                .unwrap_or(u64::MAX)
        )
    }
    fn set_local(&mut self, symbols: Vec<(String, usize)>, term: &str) {
        self.quick_ready = true;
        self.paths = Arc::default();
        self.local_positions = symbols.iter().map(|(_, p)| *p).collect();
        self.candidates = Arc::new(
            symbols
                .into_iter()
                .map(|(s, _)| fuzzy::Candidate::new(s))
                .collect(),
        );
        self.fuzzy_worker.set_items(self.candidates.clone());
        self.fuzzy_worker.query(term, 100);
    }
    fn preview_current(&mut self, workspace: &mut Workspace) -> Result<(), String> {
        if matches!(
            self.overlay,
            Some(Overlay::ProjectSymbols | Overlay::Locations)
        ) {
            if let Some(location) = self
                .quick
                .current()
                .and_then(|i| self.locations.get(i))
                .cloned()
            {
                crate::intelligence::jump(workspace, &location, true)?;
            }
            return Ok(());
        }
        if self.overlay != Some(Overlay::Goto) {
            return Ok(());
        }
        let query = Query::parse(&self.quick_input.value());
        if query.file.is_empty() {
            let kind = match &query.target {
                Target::Symbol(_) => Some("symbol"),
                Target::Word(_) => Some("word"),
                _ => None,
            };
            if let Some(kind) = kind
                && (!self.quick_ready || self.prepared != self.local_source(workspace, kind))
            {
                return Ok(());
            }
        }
        let Some(index) = self.quick.current() else {
            return Ok(());
        };
        if let Some(at) = self.local_positions.get(index) {
            workspace.editor_mut().set_caret(*at, false);
            return Ok(());
        }
        let Some(path) = self.paths.get(index).cloned() else {
            return Ok(());
        };
        let target = query.target;
        if self.last_preview.as_ref() == Some(&(path.clone(), target.clone())) {
            return Ok(());
        }
        self.last_preview = Some((path.clone(), target.clone()));
        let (line, column) = match &target {
            Target::Line(l, c) => (*l, *c),
            _ => (1, 1),
        };
        workspace.preview(FileLocation { path, line, column })?;
        if let Target::Word(term) = target {
            let text = workspace.editor().text.clone();
            // Exact first occurrence is a useful initial preview; #term offers fuzzy word rows in the current file.
            let hits = Search::new(&term, text::Options::default())?.find(&text, &[]);
            if let Some(hit) = hits.first() {
                workspace.editor_mut().set_caret(hit.start, false);
            }
        } else if let Target::Symbol(term) = target {
            let symbols = workspace
                .editor()
                .language
                .borrow()
                .as_ref()
                .and_then(|l| l.analysis.as_ref())
                .map(|a| a.symbols.clone())
                .unwrap_or_default();
            let items: Vec<_> = symbols
                .iter()
                .map(|(s, _)| fuzzy::Candidate::new(s))
                .collect();
            if let Some(hit) = fuzzy::Matcher::default().rank(&items, &term, 1).first() {
                workspace
                    .editor_mut()
                    .set_caret(symbols[hit.index].1, false);
            }
        }
        Ok(())
    }
    fn cancel_overlay(&mut self, workspace: &mut Workspace) {
        self.palette_accept = None;
        if self.overlay.take().is_none() {
            return;
        }
        for index in (0..open_file_count(workspace)).rev() {
            if workspace.tabs[index].transient && !workspace.tabs[index].dirty() {
                let _ = workspace.close(index);
            }
        }
        if let Some(preview) = self.preview.take()
            && let Some(index) = workspace.tabs.iter().position(|t| t.sheet == preview.sheet)
        {
            workspace.select(index);
            workspace.editor_mut().set_selection(preview.selection);
            workspace.tabs[index].scroll = preview.scroll;
            workspace.tabs[index].transient = preview.transient;
            self.restore_scroll = Some(preview.scroll);
            self.preserve_scroll = true;
        }
        self.focus.show(Surface::Overlay, false);
        self.local_generation += 1;
        self.local = None;
        self.prepared.clear();
        self.navigation_origin = None;
    }
    fn accept(
        &mut self,
        workspace: &mut Workspace,
        side: bool,
    ) -> Result<Option<Invocation>, String> {
        if self.overlay == Some(Overlay::Goto) {
            let query = Query::parse(&self.quick_input.value());
            if query.file.is_empty() && matches!(query.target, Target::Symbol(_) | Target::Word(_))
            {
                self.query_quick(workspace);
            }
        }
        if self.overlay == Some(Overlay::Goto) && self.quick_pending {
            // An indexing/word request has no current candidates to accept yet.
            if !self.quick_ready && self.quick_target != "line" {
                return Ok(None);
            }
        }
        if self.overlay.is_some()
            && self.quick_pending
            && self.quick_ready
            && self.local.is_none()
            && self.overlay != Some(Overlay::ProjectSymbols)
        {
            let query = Query::parse(&self.quick_input.value());
            let term = if self.overlay == Some(Overlay::Goto) {
                match &query.target {
                    Target::Word(term) | Target::Symbol(term) if query.file.is_empty() => {
                        term.as_str()
                    }
                    _ => query.file.as_str(),
                }
            } else {
                &self.quick_input.value()
            };
            let hits = fuzzy::Matcher::default().rank(&self.candidates, term, 100);
            self.publish_quick(workspace, hits);
            self.quick_pending = false;
        }
        if matches!(
            self.overlay,
            Some(Overlay::Palette | Overlay::Syntax | Overlay::Choices)
        ) {
            let invocation = self
                .quick
                .current()
                .and_then(|i| {
                    if self.overlay == Some(Overlay::Choices) {
                        self.choice_items.get(i)
                    } else {
                        workspace.palette.get(i)
                    }
                })
                .map(|p| p.invocation.clone());
            if invocation.is_none() {
                return Ok(None);
            }
            if self.overlay == Some(Overlay::Palette)
                && self
                    .quick
                    .current()
                    .is_some_and(|i| !self.palette_command_enabled(workspace, i))
            {
                return Ok(None);
            }
            if self.overlay == Some(Overlay::Palette) {
                self.palette_accept = invocation.clone();
                return Ok(invocation);
            }
            if let Some(invocation) = &invocation {
                self.uses += 1;
                self.palette_recent.insert(
                    format!("{}:{}", invocation.command, invocation.args),
                    self.uses,
                );
            }
            self.overlay = None;
            if let Some(source) = self.preview.take()
                && let Some(tab) = workspace.tabs.iter_mut().find(|t| t.sheet == source.sheet)
            {
                tab.transient = source.transient;
            }
            self.prepared.clear();
            self.focus.show(Surface::Overlay, false);
            return Ok(invocation);
        }
        if self.overlay.is_some() {
            if self.quick.current().is_none()
                && !(self.overlay == Some(Overlay::Goto)
                    && matches!(
                        Query::parse(&self.quick_input.value()).target,
                        Target::Line(_, _)
                    ))
            {
                return Ok(None);
            }
            if matches!(
                self.overlay,
                Some(Overlay::ProjectSymbols | Overlay::Locations)
            ) {
                if let Some(origin) = self.navigation_origin.take() {
                    self.intel.remember(origin);
                } else if let Some(source) = &self.preview {
                    self.intel.remember(crate::intelligence::Jump {
                        sheet: source.sheet,
                        path: workspace
                            .tabs
                            .iter()
                            .find(|t| t.sheet == source.sheet)
                            .and_then(|t| t.path.clone()),
                        selection: source.selection.clone(),
                        scroll: source.scroll,
                    });
                }
            }
            self.preview_current(workspace)?;
            if let Some(source) = &self.preview
                && let Some(index) = workspace.tabs.iter().position(|t| t.sheet == source.sheet)
                && index != workspace.active
            {
                workspace.tabs[index].transient = source.transient;
            }
            if side
                && let Some(preview) = &self.preview
                && let Some(group) = workspace.workbench.group_of(preview.sheet)
            {
                if group != workspace.workbench.active {
                    workspace.workbench.move_selected(group)?;
                    workspace.focus_current_group()?;
                }
                if let Some(index) = workspace.sheet_index(preview.sheet) {
                    workspace.tabs[index].transient = false;
                }
                workspace.workbench.groups[group]
                    .selected
                    .insert(preview.sheet);
            }
            workspace.tabs[workspace.active].transient = false;
            if let Some(path) = &workspace.tabs[workspace.active].path {
                self.uses += 1;
                self.recency.insert(path.clone(), self.uses);
            }
            self.overlay = None;
            self.preview = None;
            self.prepared.clear();
            self.focus.show(Surface::Overlay, false);
            return Ok(None);
        }
        match self.panel {
            Some(Panel::Files) => self.start_files(workspace)?,
            Some(Panel::Results) => self.jump_result(workspace)?,
            Some(Panel::Input) => {
                self.vintage_prompt = None;
                let mut invocation = self.input_done.take();
                if let Some(i) = &mut invocation {
                    if !i.args.is_object() {
                        i.args = json!({});
                    }
                    i.args["text"] = json!(self.query.value());
                }
                self.close_panel();
                return Ok(invocation);
            }
            _ => {
                self.remember_search(false);
                self.find_next(workspace, false, self.panel == Some(Panel::Incremental));
            }
        }
        Ok(None)
    }
    pub fn key(
        &mut self,
        workspace: &mut Workspace,
        key: &Key,
        viewport: Range<usize>,
    ) -> Result<(bool, Option<Invocation>), String> {
        if let Some(invocation) = self.review_key(key) {
            return Ok((true, invocation));
        }
        if !self.captured() {
            return Ok((false, None));
        }
        if self.input().is_some_and(Input::composing) {
            return Ok((false, None));
        }
        if key.name == "escape" {
            if self.vintage_prompt.take().is_some() {
                workspace.editor_mut().key(&Key::parse("escape")?)?;
            }
            if self.overlay.is_some() {
                self.cancel_overlay(workspace);
            } else if self.side_focused {
                self.side_focused = false;
            } else {
                self.cancel_incremental(workspace);
                self.close_panel();
            }
            return Ok((true, None));
        }
        if key.name == "f4" {
            return Ok((false, None));
        }
        if self.side_focused {
            if workspace.is_empty() {
                self.side_open = None;
            }
            if let Some(index) = self.side_open {
                match key.name.as_str() {
                    "up" => self.side_open = Some(index.saturating_sub(1)),
                    "down" if index + 1 < open_file_count(workspace) => {
                        self.side_open = Some(index + 1)
                    }
                    "down" => {
                        self.side_open = None;
                        self.side_selected = 0;
                    }
                    "enter" | "right" => {
                        workspace.select(index.min(open_file_count(workspace).saturating_sub(1)));
                        workspace.tabs[workspace.active].transient = false;
                    }
                    _ if key.modifiers & 9 != 0 => return Ok((false, None)),
                    _ => {}
                }
                self.side_scroll.reveal(
                    self.side_open
                        .map_or(open_file_count(workspace) + 2, |i| i + 1),
                );
                return Ok((true, None));
            }
            match key.name.as_str() {
                "up" if self.side_selected == 0 && open_file_count(workspace) > 0 => {
                    self.side_open = Some(open_file_count(workspace) - 1)
                }
                "up" => self.side_selected = self.side_selected.saturating_sub(1),
                "down" => {
                    self.side_selected =
                        (self.side_selected + 1).min(self.side_rows.len().saturating_sub(1))
                }
                "enter" => self.activate_side(workspace, true)?,
                "right" => {
                    if self
                        .side_rows
                        .get(self.side_selected)
                        .is_some_and(|r| !self.expanded.contains(&r.path))
                    {
                        self.activate_side(workspace, true)?;
                    } else {
                        self.side_selected =
                            (self.side_selected + 1).min(self.side_rows.len().saturating_sub(1));
                    }
                }
                "left" => {
                    if let Some(row) = self.side_rows.get(self.side_selected) {
                        if !self.expanded.remove(&row.path)
                            && let Some(parent) = row.path.parent()
                            && let Some(index) =
                                self.side_rows.iter().position(|r| r.path == parent)
                        {
                            self.side_selected = index;
                        }
                        self.rebuild_side(workspace);
                    }
                }
                _ if key.modifiers & 9 != 0 => return Ok((false, None)),
                _ => return Ok((true, None)),
            }
            self.side_scroll.reveal(
                self.side_open
                    .map_or(open_file_count(workspace) + 2 + self.side_selected, |i| {
                        i + 1
                    }),
            );
            return Ok((true, None));
        }
        if self.overlay.is_none() && key.name == "enter" && key.modifiers & 3 == 3 {
            return Ok((false, None));
        }
        if self.overlay.is_none() && key.name == "enter" && key.modifiers == 4 {
            if matches!(
                self.panel,
                Some(Panel::Find | Panel::Replace | Panel::Incremental)
            ) {
                self.remember_search(false);
                self.find_next(workspace, true, false);
            }
            return Ok((true, None));
        }
        if self.overlay.is_none() && key.name == "enter" && key.modifiers & 2 != 0 {
            if matches!(
                self.panel,
                Some(Panel::Find | Panel::Replace | Panel::Incremental)
            ) {
                self.action(workspace, "find_all", &json!({}), viewport)?;
            }
            return Ok((true, None));
        }
        if self.overlay.is_none() && key.name == "enter" && key.modifiers != 0 {
            return Ok((false, None));
        }
        if self.overlay.is_none()
            && key.modifiers == 0
            && matches!(key.name.as_str(), "up" | "down")
            && self.panel_focused
            && self.focus_input < 2
            && self.find_panel_visible()
        {
            self.walk_history(workspace, key.name == "up", viewport);
            return Ok((true, None));
        }
        if key.name == "enter" {
            return Ok((true, self.accept(workspace, key.modifiers & 9 != 0)?));
        }
        if key.name == "tab" {
            let count = match self.panel {
                Some(Panel::Files) => 3,
                Some(Panel::Replace) => 2,
                _ => 1,
            };
            self.focus_input =
                (self.focus_input + if key.modifiers & 4 != 0 { count - 1 } else { 1 }) % count;
            return Ok((true, None));
        }
        if self.overlay.is_some()
            && matches!(key.name.as_str(), "up" | "down" | "pageup" | "pagedown")
        {
            self.quick.navigate(match key.name.as_str() {
                "up" => -1,
                "down" => 1,
                "pageup" => -10,
                _ => 10,
            });
            self.preview_current(workspace)?;
            return Ok((true, None));
        }
        if self.panel == Some(Panel::Results) {
            match key.name.as_str() {
                "up" => self.result_selected = self.result_selected.saturating_sub(1),
                "down" => {
                    self.result_selected = (self.result_selected + 1)
                        .min(self.result_view.rows.len().saturating_sub(1))
                }
                _ if key.modifiers & 9 != 0 => return Ok((false, None)),
                _ => {}
            }
            self.result_scroll.reveal(self.result_selected);
            return Ok((true, None));
        }
        if self.panel == Some(Panel::Output) {
            match key.name.as_str() {
                "f4" => return Ok((false, None)),
                "up" => self.result_scroll.scroll(-24.),
                "down" => self.result_scroll.scroll(24.),
                "pageup" => self.result_scroll.scroll(-self.result_scroll.viewport),
                "pagedown" => self.result_scroll.scroll(self.result_scroll.viewport),
                _ if key.modifiers & 9 != 0 => return Ok((false, None)),
                _ => {}
            }
            return Ok((true, None));
        }
        // Standard selectors handle ordinary text keys. Keymap text bindings remain inside the input.
        if key.modifiers & 8 != 0 && !["a", "c", "x", "v", "z"].contains(&key.name.as_str()) {
            return Ok((false, None));
        }
        Ok((false, None))
    }
    pub fn command(
        &mut self,
        workspace: &mut Workspace,
        command: Command,
        viewport: Range<usize>,
    ) -> Result<(bool, Option<Invocation>), String> {
        if !self.captured() {
            return Ok((false, None));
        }
        if self.input().is_some_and(Input::composing) {
            if command == Command::Cancel {
                self.input_mut().expect("input").cancel_composition();
            }
            return Ok((true, None));
        }
        let key = match command {
            Command::Cancel => Some("escape"),
            Command::Newline => Some("enter"),
            Command::Tab => Some("tab"),
            Command::Up(false) if self.overlay.is_some() || self.find_panel_visible() => Some("up"),
            Command::Down(false) if self.overlay.is_some() || self.find_panel_visible() => {
                Some("down")
            }
            _ => None,
        };
        if let Some(key) = key {
            return self.key(workspace, &Key::parse(key)?, viewport);
        }
        if let Some(input) = self.input_mut() {
            input.command(command);
            self.input_changed(workspace, viewport);
        }
        Ok((true, None))
    }
    fn start_files(&mut self, workspace: &Workspace) -> Result<(), String> {
        self.remember_search(false);
        let search = match Search::new(&self.query.value(), self.options) {
            Ok(search) => search,
            Err(error) => {
                self.set_message(error);
                return Ok(());
            }
        };
        let base = workspace
            .folders
            .first()
            .map_or(Path::new("."), |f| f.path.as_path());
        let scope = match files::Where::parse(&self.where_input.value(), base) {
            Ok(scope) => scope,
            Err(error) => {
                self.set_message(error);
                return Ok(());
            }
        };
        self.file_signature = Some((self.query.value(), self.options, self.where_input.value()));
        self.replace_rx = None;
        self.replacement_plan = None;
        let open = workspace
            .tabs
            .iter()
            .enumerate()
            .filter(|(i, _)| *i == workspace.active)
            .chain(
                workspace
                    .tabs
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != workspace.active),
            )
            .map(|(i, tab)| files::OpenFile {
                path: tab
                    .path
                    .clone()
                    .unwrap_or_else(|| PathBuf::from(format!("Untitled {}", i + 1))),
                text: tab.editor.text.clone(),
            })
            .collect();
        let task = files::Task::start(files::Request {
            search,
            scope,
            folders: workspace.folders.clone(),
            open,
            context: 1,
        });
        match task {
            Ok(task) => self.file_task = Some(task),
            Err(error) => {
                self.set_message(error);
                return Ok(());
            }
        }
        self.search_results_active = true;
        self.result_cursor = None;
        self.files_searched = 0;
        self.file_matches = 0;
        self.files_matched.clear();
        self.result_view = files::Results::default();
        self.result_selected = 0;
        self.result_scroll.offset = 0.;
        self.file_complete = false;
        self.result_status = "Searching 0 files…".into();
        self.set_message(self.result_status.clone());
        self.panel = Some(Panel::Results);
        Ok(())
    }
    fn prepare_files(&mut self, workspace: &Workspace) -> Result<(), String> {
        self.remember_search(true);
        if !self.file_complete {
            return Err("Finish Find in Files before preparing replacements".into());
        }
        if self.file_signature.as_ref()
            != Some(&(self.query.value(), self.options, self.where_input.value()))
        {
            return Err(
                "Find in Files options changed; run the search again before replacing".into(),
            );
        }
        let search = Search::new(&self.query.value(), self.options)?;
        let paths: BTreeSet<_> = self
            .result_view
            .rows
            .iter()
            .filter_map(|r| match r {
                files::Row::Line(line) if !line.highlights.is_empty() => Some(line.path.clone()),
                _ => None,
            })
            .collect();
        for path in &paths {
            if workspace
                .tabs
                .iter()
                .any(|t| t.path.as_ref() == Some(path) && t.dirty())
            {
                return Err(format!("Save {} before replacing in files", path.display()));
            }
        }
        let replacement = self.replace.value();
        let (tx, rx) = mpsc::channel();
        self.replace_signature = Some((
            self.query.value(),
            self.options,
            self.where_input.value(),
            replacement.clone(),
        ));
        std::thread::spawn(move || {
            let result = paths
                .into_iter()
                .map(|path| files::FileChange::prepare(path, &search, &replacement))
                .collect::<Result<Vec<_>, _>>()
                .map(|changes| changes.into_iter().filter(|c| c.count > 0).collect());
            let _ = tx.send(result);
        });
        self.replace_rx = Some(rx);
        self.message = "Preparing replacement preview…".into();
        Ok(())
    }
    pub fn jump_result(&mut self, workspace: &mut Workspace) -> Result<(), String> {
        if let Some(files::Row::Line(line)) = self.result_view.rows.get(self.result_selected) {
            self.result_cursor = Some(self.result_selected);
            self.reveal = true;
            if let Some(index) = workspace.tabs.iter().enumerate().position(|(i, t)| {
                t.path.as_ref() == Some(&line.path)
                    || (t.path.is_none() && line.path == Path::new(&format!("Untitled {}", i + 1)))
            }) {
                workspace.select(index);
                workspace.tabs[index].goto(&FileLocation {
                    path: line.path.clone(),
                    line: line.number,
                    column: line.column(),
                });
            } else {
                workspace.open(FileLocation {
                    path: line.path.clone(),
                    line: line.number,
                    column: line.column(),
                })?;
            }
        }
        Ok(())
    }
    fn activate_side(&mut self, workspace: &mut Workspace, permanent: bool) -> Result<(), String> {
        let Some(row) = self.side_rows.get(self.side_selected).cloned() else {
            return Ok(());
        };
        if row.directory {
            if !self.expanded.remove(&row.path) {
                self.expanded.insert(row.path.clone());
                self.load_directory(workspace, &row.path);
            }
            self.rebuild_side(workspace);
        } else {
            let location = FileLocation {
                path: row.path,
                line: 1,
                column: 1,
            };
            if permanent {
                workspace.open(location)?;
            } else {
                workspace.preview(location)?;
            }
        }
        Ok(())
    }
    pub fn poll(&mut self, workspace: &mut Workspace, viewport: Range<usize>) -> bool {
        if self.vintage_prompt.is_some()
            && let Err(error) = self.sync_vintage_prompt(workspace, viewport.clone())
        {
            self.message = error;
        }
        let intelligence_changed = self.intel.poll(workspace)
            | self.review_poll(workspace)
            | self.merge_git_tree(workspace);
        if self.intel.busy()
            && self.locations.is_empty()
            && self.overlay == Some(Overlay::ProjectSymbols)
            && !self.intel.pending
        {
            self.query_quick(workspace);
        }
        if !self.hover_requested
            && !self.hover_term.is_empty()
            && self.hover_since.elapsed() >= std::time::Duration::from_millis(250)
            && self.overlay.is_none()
            && !self.intel.pending
        {
            self.intel.query(
                crate::intelligence::Query::Hover(self.hover_term.clone()),
                workspace,
            );
            self.hover_requested = true;
        }
        let mut query_changed = false;
        if let Some(result) = self.intel.result() {
            query_changed = true;
            use crate::intelligence::Query;
            let editor = workspace.editor();
            let current =
                result.source == (editor.buffer.id(), editor.buffer.change_id(), editor.caret);
            match result.query {
                Query::Hover(term) if current && term == self.hover_term => {
                    self.hover = result.locations
                }
                Query::Symbols(term)
                    if self.overlay == Some(Overlay::ProjectSymbols)
                        && term == self.quick_input.value() =>
                {
                    self.locations = result.locations;
                    self.quick_pending = false;
                    self.location_items();
                    let _ = self.preview_current(workspace);
                }
                Query::Definition(_) | Query::Reference(_) if current => {
                    if result.locations.len() == 1 {
                        if let Some(origin) = self.navigation_origin.take() {
                            self.intel.remember(origin);
                        }
                        let _ = crate::intelligence::jump(workspace, &result.locations[0], false);
                    } else if result.locations.is_empty() {
                        self.message = "No indexed locations".into();
                        self.navigation_origin = None;
                    } else {
                        let origin = self.navigation_origin.take();
                        let _ =
                            self.action(workspace, "show_overlay", &json!({}), viewport.clone());
                        self.navigation_origin = origin;
                        self.overlay = Some(Overlay::Locations);
                        self.fuzzy_worker.cancel();
                        self.locations = result.locations;
                        self.quick_pending = false;
                        self.location_items();
                        let _ = self.preview_current(workspace);
                    }
                }
                _ => {}
            }
        }
        if self.overlay == Some(Overlay::Palette) && self.prepared.is_empty() {
            self.query_quick(workspace);
        }
        if self.overlay == Some(Overlay::Goto)
            && matches!(
                Query::parse(&self.quick_input.value()).target,
                Target::Symbol(_)
            )
        {
            self.query_quick(workspace);
        }
        let mut changed = intelligence_changed || query_changed;
        changed |= self.expire_message(Instant::now());
        if let Some(result) = self.replace_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.replace_rx = None;
            if self.replace_signature.as_ref()
                == Some(&(
                    self.query.value(),
                    self.options,
                    self.where_input.value(),
                    self.replace.value(),
                ))
            {
                match result {
                    Ok(plan) => self.replacement_plan = Some(plan),
                    Err(error) => self.message = error,
                }
            } else {
                self.message = "Replacement options changed; prepare the preview again".into();
            }
            changed = true;
        }
        if self.folder_key != format!("{:?}", workspace.folders) {
            self.restart_index(workspace);
            changed = true;
        }
        if let Some(snapshot) = self.index.as_ref().and_then(Index::poll) {
            self.file_paths = snapshot.files;
            self.file_candidates = snapshot.candidates;
            self.prepared.clear();
            workspace.messages.extend(snapshot.errors);
            if self.overlay == Some(Overlay::Goto) {
                self.query_quick(workspace);
            }
            changed = true;
        }
        let directories: Vec<_> = self.directories.try_iter().collect();
        for (path, children) in directories {
            self.loading.remove(&path);
            match children {
                Ok(children) => {
                    self.git_tree_dirty = true;
                    self.git_virtual
                        .retain(|p| p.parent() != Some(path.as_path()));
                    self.children.insert(path, children);
                }
                Err(error) => self.message = error,
            }
            self.rebuild_side(workspace);
            changed = true;
        }
        if let Some(Local::Words(id, words)) = self.local.as_ref().and_then(|rx| rx.try_recv().ok())
        {
            if id == self.local_generation
                && let Target::Word(term) = Query::parse(&self.quick_input.value()).target
            {
                self.set_local(words, &term);
            }
            self.local = None;
            changed = true;
        }
        if let Some(ranked) = self.fuzzy_worker.poll()
            && self.overlay.is_some()
        {
            self.quick_pending = false;
            self.quick_elapsed_ms = ranked.elapsed.as_secs_f64() * 1000.;
            self.publish_quick(workspace, ranked.hits);
            if let Err(error) = self.preview_current(workspace) {
                self.message = error;
            }
            changed = true;
        }
        if let Some(matches) = self.search_worker.poll()
            && self.search_identity.as_ref().is_some_and(|key| {
                key.0 == workspace.editor().buffer.id()
                    && key.1 == matches.revision
                    && matches.revision == workspace.editor().revision
            })
        {
            self.highlights = matches.ranges;
            self.all_matches_ready = true;
            if let Some(navigation) = self.pending_navigation.take() {
                match navigation {
                    FindNavigation::Next {
                        backwards,
                        close,
                        report,
                    } => self.navigate_find(workspace, backwards, close, report),
                    FindNavigation::Preview(origin) => self.preview_find(workspace, origin),
                }
            }
            changed = true;
        }
        if self.search.is_some() && self.overlay.is_none() {
            let old = self.search_identity.clone();
            self.ensure_find(workspace, viewport);
            changed |= old != self.search_identity;
        }
        if let Some(task) = &self.file_task {
            let searched = task.searched_files();
            if searched != self.files_searched {
                self.files_searched = searched;
                self.result_status = format!("Searching {searched} files…");
                self.set_message(self.result_status.clone());
                changed = true;
            }
        }
        let events: Vec<_> = self
            .file_task
            .as_ref()
            .map(|t| t.events.try_iter().take(256).collect())
            .unwrap_or_default();
        for event in events {
            match event {
                files::Event::Line(line) => {
                    if !line.highlights.is_empty() {
                        self.file_matches += line.highlights.len();
                        self.files_matched.insert(line.path.clone());
                    }
                    self.result_view.push(line);
                }
                files::Event::Error(error) => self.message = error,
                files::Event::Done { files, .. } => {
                    self.files_searched = files;
                    self.result_status =
                        vel_ui::status::search(self.file_matches, self.files_matched.len());
                    self.set_message(self.result_status.clone());
                    self.file_complete = true;
                    self.file_task = None;
                }
            }
            changed = true;
        }
        self.result_scroll.rows = if self.panel == Some(Panel::Output) {
            self.output.lines().count()
        } else {
            self.result_view.rows.len()
        };
        changed |= self.sync_output();
        changed
    }
    pub fn layout(&mut self, size: Size) -> Rect {
        self.size = size;
        self.side_scroll.row_height = 24. * self.ui_scale;
        self.quick_input.scale = self.ui_scale;
        self.query.scale = self.ui_scale;
        self.replace.scale = self.ui_scale;
        self.where_input.scale = self.ui_scale;
        let side = if self.side_visible {
            220f64.min(size.width * 0.3)
        } else {
            0.
        };
        let bottom = match self.panel {
            None => 0.,
            Some(Panel::Find | Panel::Incremental) => 96. * self.ui_scale,
            Some(Panel::Replace) => 130. * self.ui_scale,
            Some(Panel::Files) => 164. * self.ui_scale,
            Some(Panel::Input) => 68. * self.ui_scale,
            _ => (size.height * 0.38).clamp(60. * self.ui_scale, 320. * self.ui_scale),
        }
        .min((size.height - (80. * self.ui_scale)).max(0.));
        self.panel_bounds = Rect {
            x: side,
            y: size.height
                - if self.chrome {
                    STATUS * self.ui_scale
                } else {
                    0.
                }
                - bottom,
            width: (size.width - side).max(1.),
            height: bottom,
        };
        self.side_bounds = Rect {
            x: 0.,
            y: (30. * self.ui_scale),
            width: side,
            height: (size.height - (30. * self.ui_scale) - (STATUS * self.ui_scale)).max(1.),
        };
        self.side_scroll.viewport = self.side_bounds.height;
        self.side_scroll.scroll(0.);
        let width = (size.width * 0.7)
            .clamp(240. * self.ui_scale, 820. * self.ui_scale)
            .min(size.width);
        self.quick_bounds = Rect {
            x: (size.width - width) / 2.,
            y: (54. * self.ui_scale),
            width,
            height: (size.height * 0.65).min(520. * self.ui_scale),
        };
        self.quick_input.bounds = Rect {
            x: self.quick_bounds.x + (8. * self.ui_scale),
            y: self.quick_bounds.y + (8. * self.ui_scale),
            width: width - (16. * self.ui_scale),
            height: (32. * self.ui_scale),
        };
        self.quick.scroll.row_height = 44. * self.ui_scale;
        self.quick.scroll.viewport = (self.quick_bounds.height - (72. * self.ui_scale)).max(1.);
        self.quick.scroll.rows = self.quick.items.len();
        self.quick.scroll.scroll(0.);
        let close = if self.find_panel_visible() {
            CLOSE_BUTTON + 6.
        } else {
            0.
        };
        for (i, input) in [&mut self.query, &mut self.replace, &mut self.where_input]
            .into_iter()
            .enumerate()
        {
            input.bounds = Rect {
                x: side + (80. * self.ui_scale),
                y: self.panel_bounds.y
                    + (8. * self.ui_scale)
                    + i as f64 * PANEL_ROW * self.ui_scale,
                width: (self.panel_bounds.width - ((92. + close) * self.ui_scale)).max(1.),
                height: (28. * self.ui_scale),
            };
            input.reveal();
        }
        self.quick_input.reveal();
        self.result_scroll.row_height =
            if self.panel == Some(Panel::Output) && self.output_editor.is_some() {
                self.output_view.line_height
            } else {
                24. * self.ui_scale
            };
        self.result_scroll.viewport = (bottom - (34. * self.ui_scale)).max(1.);
        self.result_scroll.scroll(0.);
        // Retained controls expose live theme attributes and shared hit testing.
        self.tree.roots.clear();
        for node in self.tree.nodes.values_mut() {
            node.visible = false;
        }
        for (id, class, rect, visible) in [
            (1, "quick_panel", self.quick_bounds, self.overlay.is_some()),
            (
                2,
                match self.panel {
                    Some(Panel::Find | Panel::Incremental) => "panel_control find_panel",
                    Some(Panel::Replace) => "panel_control replace_panel",
                    Some(Panel::Files) => "panel_control find_in_files_panel",
                    Some(Panel::Input) => "panel_control input_panel",
                    _ => "panel_control output_panel",
                },
                self.panel_bounds,
                self.panel.is_some(),
            ),
            (3, "sidebar_container", self.side_bounds, self.side_visible),
        ] {
            let node = self
                .tree
                .nodes
                .entry(id)
                .or_insert_with(|| Node::new(class));
            node.class = class;
            node.rect = rect;
            node.visible = visible;
            node.children.clear();
            self.tree.roots.insert(0, id);
        }
        self.tree.focus = self.focus.clone();
        Rect {
            x: side,
            y: 0.,
            width: (size.width - side).max(1.),
            height: (size.height - (STATUS * self.ui_scale) - bottom).max(32. * self.ui_scale),
        }
    }
    fn paint_input(layer: &mut Layer, input: &Input, focused: bool) {
        let rect = input.bounds;
        fill(
            layer,
            rect,
            rect,
            if focused {
                [0.12, 0.17, 0.23, 1.]
            } else {
                [0.08, 0.1, 0.13, 1.]
            },
        );
        let state = input.state();
        let shown = input.visible_text();
        let selected = state.selected.bytes(&state.text);
        let value = state.text.to_string();
        let a = input.x_for_byte(&value, selected.start);
        let b = input.x_for_byte(&value, selected.end);
        if a < b {
            fill(
                layer,
                Rect {
                    x: a,
                    y: rect.y + (5. * input.scale),
                    width: b - a,
                    height: (19. * input.scale),
                },
                rect,
                [0.23, 0.35, 0.5, 1.],
            );
        }
        label(
            layer,
            &shown,
            input.x_for_byte(&value, 0),
            rect.y + (20. * input.scale),
            rect,
            [0.9, 0.92, 0.95, 1.],
        );
        if focused {
            fill(layer, state.caret, rect, [0.9, 0.92, 0.97, 1.]);
        }
        if let Some(marked) = state.marked {
            let range = marked.bytes(&state.text);
            let a = input.x_for_byte(&value, range.start);
            let b = input.x_for_byte(&value, range.end);
            fill(
                layer,
                Rect {
                    x: a,
                    y: rect.y + (24. * input.scale),
                    width: b - a,
                    height: 1.,
                },
                rect,
                [0.55, 0.8, 1., 1.],
            );
        }
    }
    fn widget(&mut self, id: u64, class: &'static str, rect: Rect, attributes: &[&'static str]) {
        let node = self
            .tree
            .nodes
            .entry(id)
            .or_insert_with(|| Node::new(class));
        node.rect = rect;
        node.visible = true;
        node.class = class;
        node.attributes
            .retain(|a| matches!(*a, "hover" | "pressed"));
        node.attributes.extend(attributes.iter().copied());
        node.children.clear();
    }
    fn sync_widgets(&mut self, workspace: &Workspace) {
        if self.overlay.is_some() {
            self.widget(10, "text_input", self.quick_input.bounds, &["focused"]);
            let list = Rect {
                x: self.quick_bounds.x,
                y: self.quick_bounds.y + (46. * self.ui_scale),
                width: self.quick_bounds.width,
                height: self.quick.scroll.viewport,
            };
            self.widget(11, "scroll_area", list, &[]);
            self.tree.nodes.get_mut(&1).expect("overlay").children = vec![10, 11];
            for row in self.quick.scroll.visible() {
                let item = &self.quick.items[row];
                let id = (1 << 40) + item.value as u64 * 5;
                let rect = Rect {
                    y: list.y + row as f64 * (44. * self.ui_scale) - self.quick.scroll.offset,
                    height: (44. * self.ui_scale),
                    ..list
                };
                let selected = row == self.quick.selected;
                self.widget(
                    id,
                    "quick_panel_row",
                    rect,
                    if selected { &["selected"] } else { &[] },
                );
                self.tree
                    .nodes
                    .get_mut(&11)
                    .expect("list")
                    .children
                    .push(id);
                for (part, class) in [
                    (1, "quick_panel_label"),
                    (2, "kind_badge"),
                    (3, "quick_panel_annotation"),
                    (4, "quick_panel_details"),
                ] {
                    self.widget(id + part, class, rect, &[]);
                    self.tree
                        .nodes
                        .get_mut(&(id + part))
                        .expect("part")
                        .interactive = false;
                    self.tree
                        .nodes
                        .get_mut(&id)
                        .expect("row")
                        .children
                        .push(id + part);
                }
            }
        }
        if let Some(panel) = self.panel {
            let fields = match panel {
                Panel::Replace => 2,
                Panel::Files => 3,
                Panel::Find | Panel::Incremental | Panel::Input => 1,
                _ => 0,
            };
            let bounds = [
                self.query.bounds,
                self.replace.bounds,
                self.where_input.bounds,
            ];
            for (i, bounds) in bounds.into_iter().enumerate().take(fields) {
                self.widget(
                    20 + i as u64,
                    "text_input",
                    bounds,
                    if self.panel_focused && self.focus_input == i {
                        &["focused"]
                    } else {
                        &[]
                    },
                );
                self.tree
                    .nodes
                    .get_mut(&2)
                    .expect("panel")
                    .children
                    .push(20 + i as u64);
            }
            if fields > 0 && panel != Panel::Input {
                let flags = [
                    self.options.regex,
                    self.options.case_sensitive,
                    self.options.whole_word,
                    self.options.wrap,
                    self.in_selection,
                    self.options.preserve_case,
                ];
                let width = self.panel_bounds.width / (6. * self.ui_scale);
                for (i, flag) in flags.into_iter().enumerate() {
                    let rect = Rect {
                        x: self.panel_bounds.x + i as f64 * width,
                        y: self.panel_bounds.y + self.panel_bounds.height - (44. * self.ui_scale),
                        width: width - (2. * self.ui_scale),
                        height: (20. * self.ui_scale),
                    };
                    self.widget(
                        30 + i as u64,
                        "button",
                        rect,
                        if flag { &["selected"] } else { &[] },
                    );
                    self.tree
                        .nodes
                        .get_mut(&2)
                        .expect("panel")
                        .children
                        .push(30 + i as u64);
                }
                let buttons = Self::panel_buttons(panel);
                for (i, (caption, _)) in buttons.iter().enumerate() {
                    let rect = Rect {
                        x: self.panel_bounds.x + self.panel_bounds.width
                            - (buttons.len() - i) as f64 * (104. * self.ui_scale),
                        y: self.panel_bounds.y + self.panel_bounds.height - (23. * self.ui_scale),
                        width: (100. * self.ui_scale),
                        height: (22. * self.ui_scale),
                    };
                    self.widget(50 + i as u64, "button", rect, &[]);
                    self.tree
                        .nodes
                        .get_mut(&(50 + i as u64))
                        .expect("button")
                        .label = (*caption).into();
                    self.tree
                        .nodes
                        .get_mut(&2)
                        .expect("panel")
                        .children
                        .push(50 + i as u64);
                }
            }
            if self.find_panel_visible() {
                self.widget(CLOSE_ID, "button", self.close_button_rect(), &[]);
                self.tree
                    .nodes
                    .get_mut(&CLOSE_ID)
                    .expect("close button")
                    .label = "Close find panel".into();
                self.tree
                    .nodes
                    .get_mut(&2)
                    .expect("panel")
                    .children
                    .push(CLOSE_ID);
            }
        }
        if self.side_visible {
            let first = self.side_scroll.visible();
            let base = open_file_count(workspace) + 2;
            for i in first.start.saturating_sub(1)
                ..first.end.saturating_sub(1).min(open_file_count(workspace))
            {
                let tab = &workspace.tabs[i];
                let mut attrs = Vec::new();
                if tab.dirty() {
                    attrs.push("dirty");
                }
                if tab.transient {
                    attrs.push("transient");
                }
                if (self.side_focused && self.side_open == Some(i))
                    || (!self.side_focused && i == workspace.active)
                {
                    attrs.push("selected");
                }
                let id = (2 << 40) + i as u64;
                self.widget(
                    id,
                    "open_file",
                    Rect {
                        y: self.side_bounds.y + (i + 1) as f64 * (24. * self.ui_scale)
                            - self.side_scroll.offset,
                        height: (24. * self.ui_scale),
                        ..self.side_bounds
                    },
                    &attrs,
                );
                self.tree.nodes.get_mut(&3).expect("side").children.push(id);
            }
            for i in first.start.saturating_sub(base)
                ..first.end.saturating_sub(base).min(self.side_rows.len())
            {
                let row = &self.side_rows[i];
                let expanded = self.expanded.contains(&row.path);
                let rect = Rect {
                    y: self.side_bounds.y + (base + i) as f64 * (24. * self.ui_scale)
                        - self.side_scroll.offset,
                    height: (24. * self.ui_scale),
                    ..self.side_bounds
                };
                let mut attrs = Vec::new();
                if self.side_focused && self.side_open.is_none() && self.side_selected == i {
                    attrs.push("selected");
                }
                if expanded {
                    attrs.push("expanded");
                }
                if row.directory {
                    attrs.push("expandable");
                }
                self.widget((3 << 40) + i as u64, "tree_row", rect, &attrs);
                self.tree
                    .nodes
                    .get_mut(&3)
                    .expect("side")
                    .children
                    .push((3 << 40) + i as u64);
            }
        }
        self.widget(
            4,
            "status_bar",
            Rect {
                x: 0.,
                y: self.size.height - (STATUS * self.ui_scale),
                width: self.size.width,
                height: (STATUS * self.ui_scale),
            },
            &[],
        );
        self.widget(6, "status_button", self.indentation_rect(), &[]);
        self.tree.nodes.get_mut(&6).expect("indentation").label = vel_ui::status::indentation(
            workspace.editor().preferences.translate_tabs_to_spaces,
            workspace.editor().preferences.tab_size,
        );
        self.tree
            .nodes
            .get_mut(&4)
            .expect("status")
            .children
            .push(6);
        self.widget(
            5,
            "text_area",
            Rect {
                x: self.side_bounds.width,
                y: (30. * self.ui_scale),
                width: self.size.width - self.side_bounds.width,
                height: (self.panel_bounds.y - (30. * self.ui_scale)).max(1.),
            },
            &[],
        );
        self.tree.roots.insert(0, 5);
        self.tree.roots.insert(0, 4);
        self.tree.nodes.retain(|_, node| node.visible);
        self.tree.index_ancestry();
    }
    pub fn paint(&mut self, workspace: &Workspace, scene: &mut Scene) {
        self.side_scroll.rows = open_file_count(workspace) + self.side_rows.len() + 2;
        self.sync_widgets(workspace);
        let mut theme = workspace.themes.ui.borrow_mut();

        let settings = &workspace.editor().settings;
        let style = |id, child, theme: &mut vel_ui::theme::ThemeUi| {
            self.tree.style(id, child, theme, settings)
        };
        let mut layer = Layer::default();
        if self.side_visible {
            let clip = self.side_bounds;
            let sidebar = style(3, None, &mut theme);
            // The sidebar begins below the tab row. Paint its cap explicitly too;
            // the renderer's cleared background is not a theme colour.
            let cap = Rect {
                y: 0.,
                height: clip.y,
                ..clip
            };
            theme.surface(&mut layer, &sidebar, cap, cap);
            theme.surface(&mut layer, &sidebar, clip, clip);
            let heading = style(3, Some("sidebar_heading"), &mut theme);
            for (caption, row) in [
                ("Open Files", 0),
                ("Folders", open_file_count(workspace) + 1),
            ] {
                theme.text(
                    &mut layer,
                    &heading,
                    caption,
                    10. * self.ui_scale,
                    clip.y + row as f64 * (24. * self.ui_scale) - self.side_scroll.offset
                        + (18. * self.ui_scale),
                    clip,
                );
            }
            let visible = self.side_scroll.visible();
            for i in visible.start.saturating_sub(1)
                ..visible
                    .end
                    .saturating_sub(1)
                    .min(open_file_count(workspace))
            {
                let tab = &workspace.tabs[i];
                let id = (2 << 40) + i as u64;
                let Some(node) = self.tree.nodes.get(&id) else {
                    continue;
                };
                let row = style(id, None, &mut theme);
                theme.surface(&mut layer, &row, node.rect, clip);
                let mut label_style = style(id, Some("sidebar_label"), &mut theme);
                if tab.transient {
                    label_style.font.italic = true;
                }
                if tab
                    .path
                    .as_deref()
                    .and_then(|p| workspace.git.badge(p, false))
                    == Some(vel_git::Badge::Ignored)
                {
                    label_style.foreground[3] *= 0.4;
                }
                if let Some(icon) = tab
                    .path
                    .as_deref()
                    .and_then(|p| workspace.themes.file_icon(p))
                {
                    theme.icon(
                        &mut layer,
                        icon,
                        Rect {
                            x: (10. * self.ui_scale),
                            y: node.rect.y + (5. * self.ui_scale),
                            width: (14. * self.ui_scale),
                            height: (14. * self.ui_scale),
                        },
                        clip,
                        label_style.foreground,
                    );
                }
                theme.text(
                    &mut layer,
                    &label_style,
                    &tab.label(),
                    (30. * self.ui_scale) + f64::from(label_style.margin.left),
                    node.rect.y + (18. * self.ui_scale),
                    Rect {
                        width: (clip.width - (30. * self.ui_scale)).max(0.),
                        ..clip
                    },
                );
                if let Some(badge) = tab
                    .path
                    .as_deref()
                    .and_then(|p| workspace.git.badge(p, false))
                {
                    let badge_style = style(id, Some("vcs_status_badge"), &mut theme);
                    theme.text(
                        &mut layer,
                        &badge_style,
                        crate::git_ui::badge_text(badge).0,
                        clip.width - (24. * self.ui_scale),
                        node.rect.y + (18. * self.ui_scale),
                        clip,
                    );
                }
            }
            let base = open_file_count(workspace) + 2;
            for i in visible.start.saturating_sub(base)
                ..visible.end.saturating_sub(base).min(self.side_rows.len())
            {
                let row = &self.side_rows[i];
                let id = (3 << 40) + i as u64;
                let Some(node) = self.tree.nodes.get(&id) else {
                    continue;
                };
                let row_style = style(id, None, &mut theme);
                theme.surface(&mut layer, &row_style, node.rect, clip);
                let mut text = style(id, Some("sidebar_label"), &mut theme);
                if workspace.git.badge(&row.path, row.directory) == Some(vel_git::Badge::Ignored) {
                    text.foreground[3] *= 0.4;
                }
                let name = if row.depth == 0 {
                    workspace.folders[row.folder].name.clone()
                } else {
                    row.path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                };
                let x = (10. * self.ui_scale) + row.depth as f64 * (16. * self.ui_scale);
                if row.directory {
                    let disclosure = style(id, Some("disclosure_button_control"), &mut theme);
                    theme.surface(
                        &mut layer,
                        &disclosure,
                        Rect {
                            x,
                            y: node.rect.y + (6. * self.ui_scale),
                            width: (12. * self.ui_scale),
                            height: (12. * self.ui_scale),
                        },
                        clip,
                    );
                    let folder = style(id, Some("icon_folder"), &mut theme);
                    theme.surface(
                        &mut layer,
                        &folder,
                        Rect {
                            x: x + (14. * self.ui_scale),
                            y: node.rect.y + (5. * self.ui_scale),
                            width: (14. * self.ui_scale),
                            height: (14. * self.ui_scale),
                        },
                        clip,
                    );
                } else if let Some(icon) = workspace.themes.file_icon(&row.path) {
                    theme.icon(
                        &mut layer,
                        icon,
                        Rect {
                            x: x + (14. * self.ui_scale),
                            y: node.rect.y + (5. * self.ui_scale),
                            width: (14. * self.ui_scale),
                            height: (14. * self.ui_scale),
                        },
                        clip,
                        text.foreground,
                    );
                }
                theme.text(
                    &mut layer,
                    &text,
                    &name,
                    x + (32. * self.ui_scale),
                    node.rect.y + (18. * self.ui_scale),
                    clip,
                );
                if let Some(badge) = workspace.git.badge(&row.path, row.directory) {
                    let badge_style = style(id, Some("vcs_status_badge"), &mut theme);
                    theme.text(
                        &mut layer,
                        &badge_style,
                        crate::git_ui::badge_text(badge).0,
                        clip.width - (24. * self.ui_scale),
                        node.rect.y + (18. * self.ui_scale),
                        clip,
                    );
                }
            }
        }
        if let Some(panel) = self.panel {
            let clip = self.panel_bounds;
            let panel_style = style(2, None, &mut theme);
            theme.surface(&mut layer, &panel_style, clip, clip);
            let text_style = style(2, Some("label_control"), &mut theme);
            if matches!(panel, Panel::Results | Panel::Output) {
                theme.text(
                    &mut layer,
                    &text_style,
                    &if panel == Panel::Results {
                        format!("Find Results · {}", self.result_status)
                    } else {
                        "Output".into()
                    },
                    clip.x + (10. * self.ui_scale),
                    clip.y + (20. * self.ui_scale),
                    clip,
                );
                if panel == Panel::Results {
                    let body = Rect {
                        y: clip.y + (30. * self.ui_scale),
                        height: (clip.height - (30. * self.ui_scale)).max(1.),
                        ..clip
                    };
                    for row in self.result_scroll.visible() {
                        let y =
                            body.y + row as f64 * (24. * self.ui_scale) - self.result_scroll.offset;
                        let attrs = if row == self.result_selected {
                            &["selected"][..]
                        } else {
                            &[]
                        };
                        let path = [
                            vel_theme::Element::new("window", &[]),
                            vel_theme::Element::new("panel_control output_panel", &[]),
                            vel_theme::Element::new("quick_panel_row", attrs),
                        ];
                        let row_style = theme.style(
                            0xB000 + row as u64,
                            &vel_theme::ElementPath(&path),
                            settings,
                        );
                        theme.surface(
                            &mut layer,
                            &row_style,
                            Rect {
                                y,
                                height: (24. * self.ui_scale),
                                ..body
                            },
                            body,
                        );
                        let line = match &self.result_view.rows[row] {
                            files::Row::Header(path) => format!("{}:", path.display()),
                            files::Row::Gap => "        …".into(),
                            files::Row::Line(line) => format!(
                                "  {:>5}{} {}",
                                line.number,
                                if line.highlights.is_empty() { ' ' } else { ':' },
                                line.text
                            ),
                        };
                        theme.text(
                            &mut layer,
                            &row_style,
                            &line,
                            body.x + (8. * self.ui_scale),
                            y + (18. * self.ui_scale),
                            body,
                        );
                    }
                }
            } else {
                let fields = if panel == Panel::Files {
                    3
                } else if panel == Panel::Replace {
                    2
                } else {
                    1
                };
                for i in 0..fields {
                    let input = match i {
                        1 => &mut self.replace,
                        2 => &mut self.where_input,
                        _ => &mut self.query,
                    };
                    theme.text(
                        &mut layer,
                        &text_style,
                        if panel == Panel::Input {
                            &self.input_caption
                        } else {
                            ["Find", "Replace", "Where"][i]
                        },
                        clip.x + (8. * self.ui_scale),
                        input.bounds.y + (20. * self.ui_scale),
                        clip,
                    );
                    let input_style = style(20 + i as u64, None, &mut theme);
                    paint_themed_input(
                        &theme,
                        &input_style,
                        &mut layer,
                        input,
                        self.overlay.is_none() && self.focus_input == i,
                    );
                }
                if panel != Panel::Input {
                    for (i, caption) in [".*", "Aa", "Word", "Wrap", "Selection", "Preserve"]
                        .into_iter()
                        .enumerate()
                    {
                        let id = 30 + i as u64;
                        let node = &self.tree.nodes[&id];
                        let button = style(id, None, &mut theme);
                        theme.surface(&mut layer, &button, node.rect, clip);
                        let icon = style(
                            id,
                            Some(
                                [
                                    "icon_regex",
                                    "icon_case",
                                    "icon_whole_word",
                                    "icon_wrap",
                                    "icon_in_selection",
                                    "icon_preserve_case",
                                ][i],
                            ),
                            &mut theme,
                        );
                        theme.surface(
                            &mut layer,
                            &icon,
                            Rect {
                                x: node.rect.x + (4. * self.ui_scale),
                                y: node.rect.y + (3. * self.ui_scale),
                                width: (14. * self.ui_scale),
                                height: (14. * self.ui_scale),
                            },
                            node.rect,
                        );
                        theme.text(
                            &mut layer,
                            &button,
                            caption,
                            node.rect.x + (22. * self.ui_scale),
                            node.rect.y + (15. * self.ui_scale),
                            node.rect,
                        );
                    }
                    for id in 50..50 + Self::panel_buttons(panel).len() as u64 {
                        let node = &self.tree.nodes[&id];
                        let button = style(id, None, &mut theme);
                        theme.surface(&mut layer, &button, node.rect, clip);
                        theme.text(
                            &mut layer,
                            &button,
                            &node.label,
                            node.rect.x + (7. * self.ui_scale),
                            node.rect.y + (16. * self.ui_scale),
                            node.rect,
                        );
                    }
                    if let Some(node) = self.tree.nodes.get(&CLOSE_ID).filter(|n| n.visible) {
                        let rect = node.rect;
                        let button = style(CLOSE_ID, None, &mut theme);
                        theme.surface(&mut layer, &button, rect, clip);
                        theme.text(
                            &mut layer,
                            &button,
                            "\u{d7}",
                            rect.x + (6. * self.ui_scale),
                            rect.y + (16. * self.ui_scale),
                            rect,
                        );
                    }
                    theme.text(
                        &mut layer,
                        &text_style,
                        &self.message,
                        clip.x + (8. * self.ui_scale),
                        clip.y + clip.height - (8. * self.ui_scale),
                        self.panel_message_clip(panel),
                    );
                }
            }
        }
        if self.chrome {
            let status = self.tree.nodes[&4].rect;
            let status_style = style(4, None, &mut theme);
            theme.surface(&mut layer, &status_style, status, status);
            let text = style(4, Some("status_container"), &mut theme);
            let editor = workspace.editor();
            let row = if editor.binary.is_some() {
                editor.caret / 16
            } else {
                editor.text.byte_to_line(editor.caret)
            };
            let start = if editor.binary.is_some() {
                0
            } else {
                editor.text.line_to_byte(row)
            };
            let col = if editor.binary.is_some() {
                editor.caret % 16 + 1
            } else {
                editor.text.slice(start..editor.caret).len_chars() + 1
            };
            let tab = &workspace.tabs[workspace.active];
            let selected: usize = if editor.binary.is_some() {
                editor.selection().len()
            } else {
                editor
                    .regions()
                    .iter()
                    .map(|r| editor.text.slice(r.to_range()).len_chars())
                    .sum()
            };
            let document_hint = if let Some(binary) = &editor.binary {
                format!(
                    "Read-only hex · Offset 0x{:x} / {} bytes · ",
                    editor.caret,
                    binary.view.len_bytes()
                )
            } else if let Some((read, total)) = tab.progress() {
                format!(
                    "Loading {}% · Read-only prefix · ",
                    read.saturating_mul(100) / total.max(1)
                )
            } else {
                let mut hint = crate::large::hint(editor.features);
                if editor.layout.borrow().long.pending() {
                    hint.push_str(" · Preparing line geometry");
                }
                if hint.is_empty() {
                    String::new()
                } else {
                    format!("{hint} · ")
                }
            };
            theme.text(
                &mut layer,
                &text,
                &if self.message.is_empty() {
                    format!(
                        "{}{}{}",
                        document_hint,
                        if editor.vintage.is_some() {
                            format!("{} · ", editor.vintage_status())
                        } else {
                            String::new()
                        },
                        vel_ui::status::position(
                            row + 1,
                            col,
                            editor.regions().len(),
                            selected,
                            editor
                                .settings
                                .bool("show_encoding", false)
                                .then_some(tab.format.encoding.as_str()),
                            editor
                                .settings
                                .bool("show_line_endings", false)
                                .then_some(tab.format.line_ending.name())
                        )
                    )
                } else {
                    self.message.clone()
                },
                10. * self.ui_scale,
                status.y + (17. * self.ui_scale),
                Rect {
                    width: (status.width
                        - (if tab
                            .path
                            .as_deref()
                            .and_then(|p| workspace.git.status(p))
                            .is_some()
                            && editor.settings.bool("show_git_status_in_status_bar", true)
                        {
                            600.
                        } else {
                            300.
                        }) * self.ui_scale)
                        .max(0.),
                    ..status
                },
            );
            if editor.settings.bool("show_git_status_in_status_bar", true)
                && let Some(git) = tab.path.as_deref().and_then(|p| workspace.git.status(p))
            {
                let git_style = style(4, Some("vcs_status"), &mut theme);
                theme.text(
                    &mut layer,
                    &git_style,
                    git,
                    (status.width - (595. * self.ui_scale)).max(10. * self.ui_scale),
                    status.y + (17. * self.ui_scale),
                    Rect {
                        x: (status.width - (600. * self.ui_scale)).max(0.),
                        width: (300. * self.ui_scale),
                        ..status
                    },
                );
            }
            if let Some(percent) = self.intel.percentage {
                theme.text(
                    &mut layer,
                    &text,
                    &format!("Indexing {percent}%"),
                    (status.width - (310. * self.ui_scale)).max(10. * self.ui_scale),
                    status.y + (17. * self.ui_scale),
                    status,
                );
            }
            let indentation = self.indentation_rect();
            theme.text(
                &mut layer,
                &text,
                &vel_ui::status::indentation(
                    editor.preferences.translate_tabs_to_spaces,
                    editor.preferences.tab_size,
                ),
                indentation.x + 6. * self.ui_scale,
                status.y + 17. * self.ui_scale,
                indentation,
            );
            let name = editor
                .language
                .borrow()
                .as_ref()
                .map(|l| l.name.clone())
                .unwrap_or_else(|| "Plain Text".into());
            theme.text(
                &mut layer,
                &text,
                &name,
                (status.width - (170. * self.ui_scale)).max(10. * self.ui_scale),
                status.y + (17. * self.ui_scale),
                status,
            );
        }
        scene.layers.push(layer);
        let theme_animating = theme.animating();
        drop(theme);
        if self.panel == Some(Panel::Output)
            && let Some(editor) = &mut self.output_editor
        {
            editor.theme = workspace.themes.ui.clone();
            for source in [
                vel_config::settings::Layer::Default,
                vel_config::settings::Layer::Platform,
                vel_config::settings::Layer::User,
                vel_config::settings::Layer::Project,
                vel_config::settings::Layer::DistractionFree,
            ] {
                if editor.settings.layer(source) != settings.layer(source) {
                    editor
                        .settings
                        .replace(source, Value::Object(settings.layer(source).clone()))
                        .expect("settings object");
                }
            }
            editor.sync();
            editor.read_only = true;
            if let Some(current) = workspace.editor().language.borrow().as_ref()
                && let Some(output) = editor.language.borrow_mut().as_mut()
                && !Arc::ptr_eq(&current.scheme, &output.scheme)
            {
                output.recolor(
                    current.scheme.clone(),
                    editor.text.clone(),
                    editor.preferences.tab_size,
                );
            }
            let body = Rect {
                x: self.panel_bounds.x,
                y: self.panel_bounds.y + (30. * self.ui_scale),
                width: self.panel_bounds.width,
                height: (self.panel_bounds.height - (30. * self.ui_scale)).max(1.),
            };
            self.output_view.size = Size {
                width: body.width,
                height: body.height,
            };
            self.output_view.scroll_y = self.result_scroll.offset;
            self.output_view.apply_style(editor);
            self.result_scroll.row_height = self.output_view.line_height;
            self.result_scroll.rows = editor.layout.borrow().total();
            let mut output = self.output_view.scene(editor);
            crate::app_scene_translate(&mut output, body.x, body.y);
            scene.layers.extend(output.layers);
        }
        if self.overlay.is_some() {
            let mut theme = workspace.themes.ui.borrow_mut();
            let mut overlay = Layer::default();
            let panel = self.tree.style(1, None, &mut theme, settings);
            theme.surface(&mut overlay, &panel, self.quick_bounds, self.quick_bounds);
            let input_style = self.tree.style(10, None, &mut theme, settings);
            paint_themed_input(
                &theme,
                &input_style,
                &mut overlay,
                &mut self.quick_input,
                true,
            );
            let rows = Rect {
                x: self.quick_bounds.x,
                y: self.quick_bounds.y + (46. * self.ui_scale),
                width: self.quick_bounds.width,
                height: self.quick.scroll.viewport,
            };
            for row in self.quick.scroll.visible() {
                let item = &self.quick.items[row];
                let id = (1 << 40) + item.value as u64 * 5;
                let rect = self.tree.nodes[&id].rect;
                let row_style = self.tree.style(id, None, &mut theme, settings);
                theme.surface(&mut overlay, &row_style, rect, rows);
                let caption = self
                    .tree
                    .style(id, Some("quick_panel_label"), &mut theme, settings);
                theme.text(
                    &mut overlay,
                    &caption,
                    &item.caption,
                    rect.x + (30. * self.ui_scale),
                    rect.y + (18. * self.ui_scale),
                    rows,
                );
                for (byte, c) in item
                    .caption
                    .char_indices()
                    .filter(|(i, _)| item.highlights.contains(i))
                {
                    let mut matched = caption.clone();
                    matched.foreground = matched.matched;
                    theme.text(
                        &mut overlay,
                        &matched,
                        &c.to_string(),
                        rect.x
                            + (30. * self.ui_scale)
                            + theme.advance(&caption, &item.caption[..byte]),
                        rect.y + (18. * self.ui_scale),
                        rows,
                    );
                }
                let kind = self.tree.style(
                    id,
                    Some(match item.kind.as_str() {
                        "function" | "symbol" => "kind_container kind_function",
                        "command" => "kind_container kind_keyword",
                        _ => "kind_container kind_navigation",
                    }),
                    &mut theme,
                    settings,
                );
                let badge = Rect {
                    x: rect.x + (7. * self.ui_scale),
                    y: rect.y + (6. * self.ui_scale),
                    width: (16. * self.ui_scale),
                    height: (16. * self.ui_scale),
                };
                theme.surface(&mut overlay, &kind, badge, rows);
                theme.text(
                    &mut overlay,
                    &kind,
                    &item.kind.chars().next().unwrap_or('·').to_string(),
                    badge.x + (3. * self.ui_scale),
                    badge.y + (12. * self.ui_scale),
                    rows,
                );
                if self.overlay == Some(Overlay::Goto)
                    && let Some(icon) = workspace.themes.file_icon(Path::new(&item.details))
                {
                    theme.icon(&mut overlay, icon, badge, rows, caption.foreground);
                }
                let hint =
                    self.tree
                        .style(id, Some("quick_panel_label hint"), &mut theme, settings);
                theme.text(
                    &mut overlay,
                    &hint,
                    &item.annotation,
                    (rect.x + rect.width
                        - theme.advance(&hint, &item.annotation)
                        - (12. * self.ui_scale))
                        .max(rect.x + (30. * self.ui_scale)),
                    rect.y + (18. * self.ui_scale),
                    rows,
                );
                let detail =
                    self.tree
                        .style(id, Some("quick_panel_detail_label"), &mut theme, settings);
                // Inline minihtml follows the engine's restricted details mode.
                let options = vel_minihtml::LayoutOptions {
                    max_width: (rect.width - (42. * self.ui_scale)).max(1.) as f32,
                    root_font_size: detail.size,
                    palette: {
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
                        palette
                    },
                    extra_css: String::new(),
                };
                scene.layers.push(std::mem::take(&mut overlay));
                if let Ok(html) = workspace
                    .editor()
                    .html
                    .borrow_mut()
                    .cached(&item.details, true)
                {
                    html.layout(&options, theme.revision, &theme.metrics);
                    html.paint(
                        scene,
                        vel_ui::minihtml::intersection(
                            Rect {
                                x: rect.x + (30. * self.ui_scale),
                                y: rect.y + (23. * self.ui_scale),
                                width: rect.width - (42. * self.ui_scale),
                                height: (20. * self.ui_scale),
                            },
                            rows,
                        ),
                    );
                }
            }
            theme.text(
                &mut overlay,
                &panel,
                self.overlay_footer(),
                rows.x + (8. * self.ui_scale),
                self.quick_bounds.y + self.quick_bounds.height - (8. * self.ui_scale),
                self.quick_bounds,
            );
            scene.layers.push(overlay);
        }
        self.paint_hover(workspace, scene);
        self.review_paint(workspace, self.size, scene);
        if theme_animating {
            workspace.themes.ui.borrow_mut().request_animation();
        }
    }
    pub fn mouse(
        &mut self,
        workspace: &mut Workspace,
        point: Point,
        down: bool,
        double: bool,
        extend: bool,
        viewport: Range<usize>,
    ) -> Result<bool, String> {
        if self.overlay.is_some() {
            if down && !contains(self.quick_bounds, point) {
                self.cancel_overlay(workspace);
            } else if down && contains(self.quick_input.bounds, point) {
                self.quick_input.click(point.x, extend);
            } else if down
                && contains(self.quick_bounds, point)
                && point.y >= self.quick_bounds.y + (46. * self.ui_scale)
            {
                let row = ((point.y - self.quick_bounds.y - (46. * self.ui_scale)
                    + self.quick.scroll.offset)
                    / (44. * self.ui_scale)) as usize;
                if row < self.quick.items.len() {
                    self.quick.selected = row;
                    self.preview_current(workspace)?;
                    self.pending_command = self.accept(workspace, false)?;
                }
            }
            return Ok(true);
        }
        if let Some(invocation) = self.review_mouse(point, down) {
            self.pending_command = invocation;
            return Ok(true);
        }
        self.tree.pointer(point, Some(down));
        if down && self.overlay.is_none() && self.chrome && contains(self.indentation_rect(), point)
        {
            self.indentation_choices(workspace);
            return Ok(true);
        }
        if down
            && self.overlay.is_none()
            && self.chrome
            && point.y >= self.size.height - (STATUS * self.ui_scale)
            && point.x >= (self.size.width - (600. * self.ui_scale)).max(0.)
            && point.x < self.size.width - (300. * self.ui_scale)
            && workspace
                .editor()
                .settings
                .bool("show_git_status_in_status_bar", true)
        {
            if let Some(path) = &workspace.tabs[workspace.active].path {
                let items = self.git_items(workspace, path, false);
                if !items.is_empty() {
                    self.review.items = items;
                    self.review.point = point;
                    self.review.selected = 0;
                }
            }
            return Ok(true);
        }
        if down
            && self.overlay.is_none()
            && self.chrome
            && point.y >= self.size.height - (STATUS * self.ui_scale)
            && point.x >= self.size.width - (180. * self.ui_scale)
        {
            self.action(
                workspace,
                "show_overlay",
                &json!({"overlay":"syntax"}),
                viewport,
            )?;
            return Ok(true);
        }
        if self.panel.is_some() && contains(self.panel_bounds, point) {
            self.panel_focused = true;
            self.focus.show(Surface::Panel, true);
            if down {
                if self.panel == Some(Panel::Output)
                    && double
                    && point.y >= self.panel_bounds.y + (30. * self.ui_scale)
                {
                    let row = ((point.y - self.panel_bounds.y - (30. * self.ui_scale)
                        + self.result_scroll.offset)
                        / (24. * self.ui_scale)) as usize;
                    if let Some((_, location)) =
                        self.build_results.iter().find(|(line, _)| *line == row)
                    {
                        workspace.open(location.clone())?;
                    }
                } else if self.panel == Some(Panel::Results) {
                    if point.y >= self.panel_bounds.y + (30. * self.ui_scale) {
                        let row = ((point.y - self.panel_bounds.y - (30. * self.ui_scale)
                            + self.result_scroll.offset)
                            / (24. * self.ui_scale)) as usize;
                        if row < self.result_view.rows.len() {
                            self.result_selected = row;
                            if double {
                                self.jump_result(workspace)?;
                            }
                        }
                    }
                } else {
                    for (i, input) in [&mut self.query, &mut self.replace, &mut self.where_input]
                        .into_iter()
                        .enumerate()
                    {
                        if contains(input.bounds, point) {
                            self.focus_input = i;
                            input.click(point.x, extend);
                            break;
                        }
                    }
                    if let Some(id) = self.tree.hit_test(point) {
                        if (30..=35).contains(&id) {
                            let option = [
                                "regex",
                                "case_sensitive",
                                "whole_word",
                                "wrap",
                                "in_selection",
                                "preserve_case",
                            ][(id - 30) as usize];
                            self.action(
                                workspace,
                                "toggle_find_option",
                                &json!({"option":option}),
                                viewport.clone(),
                            )?;
                        } else if id == CLOSE_ID {
                            self.action(workspace, "hide_panel", &json!({}), viewport.clone())?;
                        } else if let Some((_, command)) = self
                            .panel
                            .and_then(|p| Self::panel_buttons(p).get(id.wrapping_sub(50) as usize))
                        {
                            self.action(workspace, command, &json!({}), viewport.clone())?;
                        }
                    }
                }
            }
            return Ok(true);
        }
        if self.side_visible && contains(self.side_bounds, point) {
            if down {
                let row = ((point.y - self.side_bounds.y + self.side_scroll.offset)
                    / (24. * self.ui_scale)) as usize;
                if (1..=open_file_count(workspace)).contains(&row) {
                    workspace.click_sheet(row - 1, self.primary_click, extend);
                    self.side_open = Some(row - 1);
                    if double {
                        workspace.tabs[workspace.active].transient = false;
                    }
                } else if row >= open_file_count(workspace) + 2 {
                    self.side_open = None;
                    self.side_selected = (row - open_file_count(workspace) - 2)
                        .min(self.side_rows.len().saturating_sub(1));
                    if extend && let Some(anchor) = self.side_anchor {
                        for index in anchor.min(self.side_selected)..=anchor.max(self.side_selected)
                        {
                            if let Some(row) = self.side_rows.get(index)
                                && !row.directory
                            {
                                workspace.open_additive(FileLocation {
                                    path: row.path.clone(),
                                    line: 1,
                                    column: 1,
                                })?;
                            }
                        }
                    } else if self.primary_click {
                        if let Some(row) = self.side_rows.get(self.side_selected)
                            && !row.directory
                        {
                            if let Some(index) = workspace
                                .tabs
                                .iter()
                                .position(|t| t.path.as_ref() == Some(&row.path))
                            {
                                workspace.click_sheet(index, true, false);
                            } else {
                                workspace.open_additive(FileLocation {
                                    path: row.path.clone(),
                                    line: 1,
                                    column: 1,
                                })?;
                            }
                        } else {
                            self.activate_side(workspace, double)?;
                        }
                        self.side_anchor = Some(self.side_selected);
                    } else {
                        self.activate_side(workspace, double)?;
                        self.side_anchor = Some(self.side_selected);
                    }
                }
                self.side_focused = true;
                self.panel_focused = false;
                self.focus.show(Surface::Panel, false);
            }
            return Ok(true);
        }
        if down {
            self.side_focused = false;
            self.panel_focused = false;
            self.focus.show(Surface::Panel, false);
        }
        Ok(self.chrome && point.y >= self.size.height - (STATUS * self.ui_scale))
    }
    pub fn scroll(&mut self, point: Point, delta: f64) -> bool {
        if self.overlay.is_some() {
            self.quick.scroll.scroll(delta);
            true
        } else if self.side_visible && contains(self.side_bounds, point) {
            self.side_scroll.scroll(delta);
            true
        } else if matches!(self.panel, Some(Panel::Results | Panel::Output))
            && contains(self.panel_bounds, point)
        {
            self.result_scroll.scroll(delta);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn workspace() -> Workspace {
        Workspace::new(vel_text::Text::from("cat CAT\ncat"), None, None, false).unwrap()
    }
    #[test]
    fn panel_input_focus_find_replace_and_cancel_restore_preview() {
        let mut workspace = workspace();
        let mut panels = Panels::new(&workspace, false);
        panels
            .action(
                &mut workspace,
                "show_panel",
                &json!({"panel":"replace","text":"cat"}),
                0..11,
            )
            .unwrap();
        panels.replace.set("dog");
        panels.options.preserve_case = true;
        panels
            .action(&mut workspace, "replace_all", &json!({}), 0..11)
            .unwrap();
        assert_eq!(workspace.editor().text.to_string(), "dog DOG\ndog");
        workspace.editor_mut().run("undo", &json!({})).unwrap();
        assert_eq!(workspace.editor().text.to_string(), "cat CAT\ncat");
        panels
            .action(&mut workspace, "show_overlay", &json!({"text":":2"}), 0..11)
            .unwrap();
        assert_eq!(workspace.editor().caret, 8);
        panels
            .key(&mut workspace, &Key::parse("escape").unwrap(), 0..11)
            .unwrap();
        assert_eq!(workspace.editor().caret, 0);
        assert_eq!(panels.focus.route().0, Surface::Panel);
    }
    /// Resolves `key` through the panel router and runs the resulting command like the app does.
    fn press(panels: &mut Panels, workspace: &mut Workspace, key: &str) -> Option<String> {
        use vel_config::keymap::Resolution;
        match panels
            .resolve_key(workspace, &Key::parse(key).unwrap(), 0..11)
            .unwrap()
        {
            Resolution::Command(invocation) => {
                let handled = panels
                    .action(workspace, &invocation.command, &invocation.args, 0..11)
                    .unwrap();
                if !handled {
                    workspace
                        .editor_mut()
                        .run(&invocation.command, &invocation.args)
                        .unwrap();
                }
                Some(invocation.command)
            }
            Resolution::Pending => Some(String::new()),
            Resolution::Unbound => None,
        }
    }
    fn wait_for_matches(panels: &mut Panels, workspace: &mut Workspace) {
        let until = Instant::now() + std::time::Duration::from_secs(5);
        while !panels.all_matches_ready {
            panels.poll(workspace, 0..11);
            assert!(Instant::now() < until, "find worker did not finish");
            std::thread::yield_now();
        }
    }
    #[test]
    fn find_panel_toggles_closes_from_editor_focus_and_hides_matches() {
        let mut workspace = workspace();
        let mut panels = Panels::new(&workspace, false);
        assert_eq!(
            press(&mut panels, &mut workspace, "super+f").as_deref(),
            Some("show_panel")
        );
        assert_eq!(panels.panel, Some(Panel::Find));
        panels.query.set("cat");
        panels.input_changed(&mut workspace, 0..11);
        wait_for_matches(&mut panels, &mut workspace);
        assert_eq!(panels.visible_highlights().len(), 3);
        // Pressing the shortcut again closes the panel and hides the matches.
        press(&mut panels, &mut workspace, "super+f");
        assert_eq!(panels.panel, None);
        assert!(panels.visible_highlights().is_empty());
        // Background results and re-validation do not bring the highlights back while closed.
        panels.poll(&mut workspace, 0..11);
        assert!(panels.visible_highlights().is_empty());
        // The search itself survives. Reset the caret so this checks navigation independently
        // of the new typing-preview selection.
        workspace.editor_mut().set_caret(0, false);
        press(&mut panels, &mut workspace, "super+g");
        assert_eq!(workspace.editor().selection(), 0..3);
        assert!(panels.visible_highlights().is_empty());
        // Reopening keeps the query and shows the matches again.
        press(&mut panels, &mut workspace, "super+f");
        assert_eq!(panels.panel, Some(Panel::Find));
        assert_eq!(panels.query.value(), "cat");
        wait_for_matches(&mut panels, &mut workspace);
        assert_eq!(panels.visible_highlights().len(), 3);
        // A different find-family shortcut switches panels instead of closing.
        press(&mut panels, &mut workspace, "super+alt+f");
        assert_eq!(panels.panel, Some(Panel::Replace));
        press(&mut panels, &mut workspace, "super+alt+f");
        assert_eq!(panels.panel, None);
        // Escape with keyboard focus in the editor closes a visible panel through the keymap.
        press(&mut panels, &mut workspace, "super+f");
        panels.panel_focused = false;
        panels.focus.show(Surface::Panel, false);
        assert!(!panels.captured());
        assert_eq!(
            press(&mut panels, &mut workspace, "escape").as_deref(),
            Some("hide_panel")
        );
        assert_eq!(panels.panel, None);
        assert!(panels.visible_highlights().is_empty());
    }
    #[test]
    fn escape_without_a_panel_keeps_its_editor_meanings() {
        let mut workspace = workspace();
        let mut panels = Panels::new(&workspace, false);
        workspace
            .editor_mut()
            .set_selection(Selection::from_regions([
                Region::new(0, 0),
                Region::new(8, 8),
            ]));
        assert_eq!(
            press(&mut panels, &mut workspace, "escape").as_deref(),
            Some("single_selection")
        );
        assert_eq!(workspace.editor().regions().len(), 1);
        // Escape still collapses to one caret and never reports hide_panel without a panel.
        assert_eq!(
            press(&mut panels, &mut workspace, "escape").as_deref(),
            Some("single_selection")
        );
        assert_eq!(panels.panel, None);
    }
    #[test]
    fn find_panel_close_button_hides_the_panel() {
        let mut workspace = workspace();
        let mut panels = Panels::new(&workspace, false);
        panels
            .action(
                &mut workspace,
                "show_panel",
                &json!({"panel":"find","text":"cat"}),
                0..11,
            )
            .unwrap();
        panels.layout(Size {
            width: 1200.,
            height: 800.,
        });
        let mut scene = Scene::default();
        panels.paint(&workspace, &mut scene);
        let close = panels.close_button_rect();
        let query = panels.query.bounds;
        assert!(
            close.x >= query.x + query.width,
            "close button overlaps the query field"
        );
        assert!(close.x + close.width <= panels.panel_bounds.x + panels.panel_bounds.width);
        let center = Point {
            x: close.x + close.width / 2.,
            y: close.y + close.height / 2.,
        };
        panels
            .mouse(&mut workspace, center, true, false, false, 0..11)
            .unwrap();
        panels
            .mouse(&mut workspace, center, false, false, false, 0..11)
            .unwrap();
        assert_eq!(panels.panel, None);
        assert!(panels.visible_highlights().is_empty());
        // Panels without find semantics have no close button.
        panels
            .action(
                &mut workspace,
                "show_panel",
                &json!({"panel":"output.exec"}),
                0..11,
            )
            .unwrap();
        assert!(!panels.find_panel_visible());
    }
    #[test]
    fn preview_tabs_do_not_replace_or_lose_original_and_commit_pins() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        std::fs::write(&first, "first").unwrap();
        std::fs::write(&second, "second").unwrap();
        let mut workspace = workspace();
        let original = workspace.editor().buffer.id();
        workspace
            .preview(FileLocation {
                path: first,
                line: 1,
                column: 1,
            })
            .unwrap();
        assert_eq!(workspace.tabs.len(), 2);
        workspace
            .preview(FileLocation {
                path: second.clone(),
                line: 1,
                column: 1,
            })
            .unwrap();
        assert_eq!(workspace.tabs.len(), 2);
        assert!(
            workspace
                .tabs
                .iter()
                .any(|t| t.editor.buffer.id() == original)
        );
        workspace
            .open(FileLocation {
                path: second,
                line: 1,
                column: 1,
            })
            .unwrap();
        assert!(!workspace.tabs[workspace.active].transient);
    }
    #[test]
    fn shrinking_selection_replacement_and_empty_selection_regex_are_safe() {
        let mut workspace = workspace();
        let mut panels = Panels::new(&workspace, false);
        workspace
            .editor_mut()
            .set_selection(Selection::from_regions([Region::new(0, 7)]));
        panels
            .action(
                &mut workspace,
                "show_panel",
                &json!({"panel":"replace","text":"cat"}),
                0..11,
            )
            .unwrap();
        panels.in_selection = true;
        panels.replace.set("x");
        panels
            .action(&mut workspace, "replace_all", &json!({}), 0..11)
            .unwrap();
        assert_eq!(workspace.editor().text.to_string(), "x x\ncat");
        panels
            .action(&mut workspace, "replace_all", &json!({}), 0..11)
            .unwrap();
        assert_eq!(workspace.editor().text.to_string(), "x x\ncat");
        panels.selection_scope.clear();
        panels.options.regex = true;
        panels.query.set("^");
        panels
            .action(&mut workspace, "replace_all", &json!({}), 0..11)
            .unwrap();
        assert_eq!(workspace.editor().text.to_string(), "x x\ncat");
    }
    #[test]
    fn automatic_panel_workload_routes_and_renders_without_native_services() {
        let mut workspace = workspace();
        let mut panels = Panels::new(&workspace, false);
        Panels::seed_benchmark(&mut workspace);
        for i in 0..10 {
            panels.benchmark_step(&mut workspace, i, 0..11).unwrap();
            let until = Instant::now() + std::time::Duration::from_secs(5);
            while panels.ranking_pending() {
                panels.poll(&mut workspace, 0..11);
                assert!(Instant::now() < until);
                std::thread::yield_now();
            }
            panels.layout(Size {
                width: 1200.,
                height: 800.,
            });
            let mut scene = Scene::default();
            panels.paint(&workspace, &mut scene);
            assert!(!scene.layers.is_empty());
            assert!(panels.tree.nodes.len() < 200);
        }
        assert_eq!(workspace.editor().text.to_string(), "cat CAT\ncat");
        assert!(panels.overlay.is_none());
        assert!(panels.panel.is_none());
    }
    #[test]
    fn cancel_goto_restores_an_existing_sidebar_preview_and_its_viewport() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let mut workspace = workspace();
        workspace
            .preview(FileLocation {
                path: a,
                line: 1,
                column: 1,
            })
            .unwrap();
        workspace.tabs[workspace.active].scroll = (12., 80.);
        let source = workspace.editor().buffer.id();
        let mut panels = Panels::new(&workspace, false);
        panels
            .action(&mut workspace, "show_overlay", &json!({}), 0..1)
            .unwrap();
        workspace
            .preview(FileLocation {
                path: b,
                line: 1,
                column: 1,
            })
            .unwrap();
        panels
            .key(&mut workspace, &Key::parse("escape").unwrap(), 0..1)
            .unwrap();
        assert_eq!(workspace.editor().buffer.id(), source);
        assert!(workspace.tabs[workspace.active].transient);
        assert_eq!(panels.restore_scroll, Some((12., 80.)));
    }
}

#[cfg(test)]
mod intelligence_tests {
    use super::*;
    use std::time::Duration;
    #[track_caller]
    fn wait(
        workspace: &mut Workspace,
        panels: &mut Panels,
        condition: impl Fn(&Workspace, &Panels) -> bool,
    ) {
        let until = Instant::now() + Duration::from_secs(15);
        loop {
            workspace.poll();
            panels.poll(workspace, 0..100);
            if condition(workspace, panels) {
                return;
            }
            assert!(
                Instant::now() < until,
                "index navigation timed out: {} / {:?}; active {:?}; overlay {:?}; pending {}; unique {:?}; shared {:?}",
                panels.message,
                workspace.messages,
                workspace.tabs[workspace.active].path,
                panels.overlay,
                panels.intel.pending,
                panels
                    .intel
                    .projects
                    .first()
                    .map(|p| p.index.lookup_definition("unique")),
                panels
                    .intel
                    .projects
                    .first()
                    .map(|p| p.index.lookup_definition("shared"))
            );
            std::thread::park_timeout(Duration::from_millis(1));
        }
    }
    fn setup() -> (tempfile::TempDir, tempfile::TempDir, Workspace, Panels) {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().canonicalize().unwrap().join("a.rs"),
            "fn shared() {}\n// unique\n",
        )
        .unwrap();
        std::fs::write(
            root.path().canonicalize().unwrap().join("b.rs"),
            "fn shared() {}\nfn unique() {}\n",
        )
        .unwrap();
        let mut workspace =
            Workspace::new(vel_text::Text::new(), Some(data.path().into()), None, false).unwrap();
        let mut panels = Panels::new(&workspace, false);
        panels
            .add_folder(&mut workspace, root.path().into())
            .unwrap();
        for file in ["b.rs", "a.rs"] {
            workspace
                .open(FileLocation {
                    path: root.path().canonicalize().unwrap().join(file),
                    line: 1,
                    column: 1,
                })
                .unwrap();
        }
        wait(&mut workspace, &mut panels, |_, p| {
            !p.intel.projects.is_empty() && !p.intel.projects[0].index.status().active
        });
        (root, data, workspace, panels)
    }
    #[test]
    fn project_symbols_definition_previews_unicode_locations_and_bounded_history() {
        let (root, _data, mut w, mut p) = setup();
        let a = w.active;
        let source = w.editor().text.to_string();
        w.editor_mut()
            .set_caret(source.find("unique").unwrap(), false);
        let start = w.editor().caret;
        p.action(&mut w, "goto_definition", &json!({}), 0..100)
            .unwrap();
        wait(&mut w, &mut p, |w, p| {
            !p.intel.pending
                && w.tabs[w.active].path.as_ref()
                    == Some(&root.path().canonicalize().unwrap().join("b.rs"))
        });
        assert_eq!(w.editor().caret, "fn shared() {}\nfn ".len());
        p.action(&mut w, "jump_back", &json!({}), 0..100).unwrap();
        assert_eq!(w.active, a);
        assert_eq!(w.editor().caret, start);
        p.action(&mut w, "jump_forward", &json!({}), 0..100)
            .unwrap();
        assert_eq!(
            w.tabs[w.active].path.as_ref(),
            Some(&root.path().canonicalize().unwrap().join("b.rs"))
        );
        w.select(a);
        w.editor_mut().set_caret(4, false);
        p.action(&mut w, "goto_definition", &json!({}), 0..100)
            .unwrap();
        wait(&mut w, &mut p, |_, p| {
            p.overlay == Some(Overlay::Locations) && p.quick.items.len() == 2
        });
        assert!(p.quick.items.iter().all(|r| r.annotation.contains(".rs:")));
        p.key(&mut w, &Key::parse("escape").unwrap(), 0..100)
            .unwrap();
        assert_eq!(w.active, a);
        assert_eq!(w.editor().caret, 4);
        p.action(&mut w, "goto_symbol_in_project", &json!({}), 0..100)
            .unwrap();
        p.quick_input.set("uniq");
        p.query_quick(&mut w);
        wait(&mut w, &mut p, |_, p| {
            !p.quick_pending && !p.quick.items.is_empty()
        });
        assert_eq!(p.quick.items[0].caption, "unique");
        p.key(&mut w, &Key::parse("enter").unwrap(), 0..100)
            .unwrap();
        assert!(p.overlay.is_none());
        for _ in 0..140 {
            let mut jump = crate::intelligence::capture(&w);
            jump.sheet += p.intel.back.len() as u64 + 1;
            p.intel.remember(jump);
        }
        assert!(p.intel.back.len() <= 128);
    }
    #[test]
    fn unsaved_analysis_replaces_disk_and_close_restores_it() {
        let (_root, _data, mut w, mut p) = setup();
        w.editor_mut()
            .set_selection(Selection::from_regions([Region::from(3..9)]));
        w.editor_mut()
            .run("insert", &json!({"characters":"renamed"}))
            .unwrap();
        wait(&mut w, &mut p, |_, p| {
            p.intel.projects[0].index.lookup_definition("renamed").len() == 1
        });
        assert_eq!(
            p.intel.projects[0].index.lookup_definition("shared").len(),
            1
        );
        let old = w.active;
        w.editor_mut().run("undo", &json!({})).unwrap();
        wait(&mut w, &mut p, |_, p| {
            p.intel.projects[0]
                .index
                .lookup_definition("renamed")
                .is_empty()
                && p.intel.projects[0].index.lookup_definition("shared").len() == 2
        });
        w.close(old).unwrap();
        wait(&mut w, &mut p, |_, p| {
            p.intel.projects[0].index.lookup_definition("shared").len() == 2
        });
        assert!(
            crate::intelligence::status_text(&[p.intel.projects[0].index.clone()])
                .contains("files")
        );
    }
    #[test]
    fn references_hover_click_and_disabled_hover_use_the_project_service() {
        let (_root, data, mut w, mut p) = setup();
        let refs = data.path().canonicalize().unwrap().join("refs.rs");
        std::fs::write(&refs, "// shared\n").unwrap();
        let index = p.intel.projects[0].index.shared_index();
        index.write().unwrap().replace_file(
            refs.clone(),
            vel_index::FileAnalysis {
                symbols: vec![vel_index::IndexedSymbol {
                    name: "shared".into(),
                    row: 0,
                    column: 3,
                    range: 3..9,
                    kind: vel_index::SymbolKind::Function,
                    definition: false,
                    reference: true,
                }],
                ..Default::default()
            },
        );
        w.editor_mut().set_caret(4, false);
        p.action(&mut w, "goto_reference", &json!({}), 0..100)
            .unwrap();
        wait(&mut w, &mut p, |w, p| {
            !p.intel.pending && w.tabs[w.active].path.as_ref() == Some(&refs)
        });
        assert_eq!(w.editor().caret, 3);
        p.action(&mut w, "jump_back", &json!({}), 0..100).unwrap();
        p.hover_at(&w, Point { x: 100., y: 100. }, 4);
        p.hover_since = Instant::now() - Duration::from_secs(1);
        wait(&mut w, &mut p, |_, p| !p.hover.is_empty());
        p.layout(Size {
            width: 800.,
            height: 600.,
        });
        let mut scene = Scene::default();
        p.paint(&w, &mut scene);
        assert!(
            p.tree
                .nodes
                .values()
                .any(|n| n.class == "popup_control html_popup")
        );
        let point = Point {
            x: p.hover_bounds.x + 10.,
            y: p.hover_bounds.y + 10.,
        };
        assert!(p.click_definition(&mut w, point).unwrap());
        assert!(p.hover.is_empty());
        w.editor_mut().settings.set(
            vel_config::settings::Layer::View,
            "show_definitions",
            json!(false),
        );
        p.hover_at(&w, Point { x: 200., y: 200. }, 4);
        assert!(p.hover_term.is_empty());
    }
    #[test]
    fn unavailable_profile_cache_falls_back_and_index_settings_disable_it() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().canonicalize().unwrap().join("a.rs"),
            "fn available() {}\n",
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("profile");
        std::fs::write(&bad, "not a directory").unwrap();
        let mut w = Workspace::new(vel_text::Text::new(), Some(bad), None, false).unwrap();
        let mut p = Panels::new(&w, false);
        p.add_folder(&mut w, root.path().into()).unwrap();
        wait(&mut w, &mut p, |_, p| {
            p.intel
                .projects
                .first()
                .is_some_and(|p| !p.index.lookup_definition("available").is_empty())
        });
        assert!(w.messages.iter().any(|m| m.contains("temporary cache")));
        let status = p.intel.status_source.clone();
        assert_eq!(status.lock().unwrap().indices.len(), 1);
        w.settings.set(
            vel_config::settings::Layer::Project,
            "index_files",
            json!(false),
        );
        p.poll(&mut w, 0..100);
        assert!(p.intel.projects.is_empty());
        assert!(status.lock().unwrap().indices.is_empty());
        assert!(!status.lock().unwrap().loading);
    }
}

impl Panels {
    pub fn review_side_path(&self, workspace: &Workspace, point: Point) -> Option<(PathBuf, bool)> {
        if !self.side_visible || !contains(self.side_bounds, point) {
            return None;
        }
        let row = ((point.y - self.side_bounds.y + self.side_scroll.offset) / (24. * self.ui_scale))
            as usize;
        if row > 0 && row <= open_file_count(workspace) {
            return workspace.tabs[row - 1].path.clone().map(|p| (p, false));
        }
        row.checked_sub(open_file_count(workspace) + 2)
            .and_then(|i| self.side_rows.get(i))
            .map(|r| (r.path.clone(), r.directory))
    }
    pub fn review_hover(&mut self, workspace: &Workspace, point: Point) {
        if !self.review.native_context
            && !self.review.items.is_empty()
            && contains(self.review.bounds, point)
        {
            self.review.selected = (((point.y - self.review.bounds.y - 4.).max(0.) / 26.) as usize)
                .min(self.review.items.len() - 1);
            self.review.tooltip = None;
            return;
        }
        let text = if self.side_visible && point.x >= self.side_bounds.width - 32. {
            self.review_side_path(workspace, point)
                .and_then(|(path, folder)| workspace.git.badge(&path, folder))
                .map(|b| crate::git_ui::badge_text(b).1.to_owned())
        } else {
            None
        };
        if self.review.tooltip.as_ref().map(|(t, _, _)| t) != text.as_ref() {
            self.review.tooltip = text.map(|t| (t, point, Instant::now()));
        }
    }
}

fn sidebar_children(folder: &Folder, path: &Path) -> Result<Vec<Entry>, String> {
    // The sidebar shows ignored entries dimmed; search/index walkers retain ignore filtering.
    let mut walker = folder.walker(path, Some(1), false);
    walker
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .ignore(false);
    let mut entries = Vec::new();
    for result in walker.build() {
        let entry = result.map_err(|e| e.to_string())?;
        if entry.depth() > 0 {
            entries.push(Entry {
                path: entry.path().into(),
                directory: entry.file_type().is_some_and(|t| t.is_dir()),
            });
        }
    }
    entries.sort_by(|a, b| b.directory.cmp(&a.directory).then(a.path.cmp(&b.path)));
    Ok(entries)
}
impl Panels {
    fn merge_git_tree(&mut self, workspace: &Workspace) -> bool {
        let key: Vec<_> = workspace
            .git
            .snapshots
            .values()
            .map(|s| (s.root.clone(), s.generation))
            .collect();
        if key == self.git_tree_key && !self.git_tree_dirty {
            return false;
        }
        let refresh = key != self.git_tree_key;
        self.git_tree_key = key;
        self.git_tree_dirty = false;
        if refresh {
            let parents: Vec<_> = self.children.keys().cloned().collect();
            for parent in parents {
                self.request_directory(workspace, &parent, true);
            }
        }
        for children in self.children.values_mut() {
            children.retain(|e| !self.git_virtual.contains(&e.path));
        }
        self.git_virtual.clear();
        for (parent, children) in &mut self.children {
            let Some(folder) = workspace
                .folders
                .iter()
                .find(|f| parent.starts_with(&f.path))
            else {
                continue;
            };
            for snapshot in workspace.git.snapshots.values() {
                for (relative, status) in &snapshot.files {
                    if !status.statuses.iter().any(|b| {
                        matches!(b, vel_git::Badge::Missing | vel_git::Badge::StagedDeletion)
                    }) {
                        continue;
                    }
                    let path = snapshot.root.join(relative);
                    let Ok(tail) = path.strip_prefix(parent) else {
                        continue;
                    };
                    let mut parts = tail.components();
                    let Some(first) = parts.next() else {
                        continue;
                    };
                    let path = parent.join(first);
                    let directory = parts.next().is_some();
                    if folder.rules.allows(&folder.path, &path, directory, false)
                        && !children.iter().any(|e| e.path == path)
                    {
                        self.git_virtual.insert(path.clone());
                        children.push(Entry { path, directory });
                    }
                }
            }
            children.sort_by(|a, b| b.directory.cmp(&a.directory).then(a.path.cmp(&b.path)));
        }
        self.rebuild_side(workspace);
        true
    }
}

fn paint_themed_input(
    theme: &vel_ui::theme::ThemeUi,
    style: &vel_ui::theme::Style,
    layer: &mut Layer,
    input: &mut Input,
    focused: bool,
) {
    input.configure_theme(style, theme.metrics.clone());
    let first = layer.rects.len();
    let text = layer.text.len();
    Panels::paint_input(layer, input, focused);
    layer.rects.remove(first);
    theme.surface(layer, style, input.bounds, input.bounds);
    for run in &mut layer.text[text..] {
        run.color = style.foreground;
        run.size = style.size;
        run.style = style.font;
        run.family = style.family.clone();
    }
    for rect in &mut layer.rects[first..] {
        rect.color = style.matched;
        rect.color[3] = if rect.rect.width <= 1. { 1. } else { 0.3 };
    }
}

impl Panels {
    fn paint_hover(&mut self, workspace: &Workspace, scene: &mut Scene) {
        if self.hover.is_empty() || self.overlay.is_some() {
            self.hover_html = None;
            self.hover_bounds = Rect::default();
            return;
        }
        let source=self.hover.iter().take(8).enumerate().map(|(i,location)|format!("<div><a href='definition:{i}' title='Open definition'><b>{}</b></a> <code>{}:{}</code></div>",escape_html(&location.name),escape_html(&location.file.file_name().unwrap_or_default().to_string_lossy()),location.row+1)).collect::<String>();
        if self
            .hover_html
            .as_ref()
            .is_none_or(|p| p.html.source != source)
        {
            self.hover_html = vel_ui::minihtml::Popup::new(
                &source,
                vel_ui::minihtml::PopupFlags::HIDE_ON_MOUSE_MOVE_AWAY,
                None,
                Size {
                    width: 460.,
                    height: 240.,
                },
            )
            .ok();
        }
        let options =
            crate::html_ui::options(workspace.editor(), 460f64.min(self.size.width), true);
        let Some(popup) = &mut self.hover_html else {
            return;
        };
        let mut theme = workspace.themes.ui.borrow_mut();
        popup.html.layout(&options, theme.revision, &theme.metrics);
        let viewport = Rect {
            x: 0.,
            y: 0.,
            width: self.size.width,
            height: self.size.height,
        };
        if let Some(rect) = popup.place(
            Rect {
                x: self.hover_point.x,
                y: self.hover_point.y,
                width: 1.,
                height: 16.,
            },
            viewport,
            workspace.editor().completion.popup_rect(),
        ) {
            self.hover_bounds = rect;
            let node = self
                .tree
                .nodes
                .entry(9001)
                .or_insert_with(|| Node::new("popup_control html_popup"));
            node.rect = rect;
            node.visible = true;
            let style = theme.style(
                9001,
                &vel_theme::ElementPath(&[
                    vel_theme::Element::new("window", &[]),
                    vel_theme::Element::new("popup_control html_popup", &[]),
                ]),
                &workspace.editor().settings,
            );
            let mut layer = Layer::default();
            theme.surface(&mut layer, &style, rect, viewport);
            scene.layers.push(layer);
            popup.html.paint(scene, rect);
        }
    }
}
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\"', "&quot;")
        .replace('\'', "&#39;")
}

#[path = "panels_a11y.rs"]
mod accessibility;
pub(crate) use accessibility::ax_id;
