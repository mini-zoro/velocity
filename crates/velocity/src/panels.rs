//! Headless UI controller: navigation, panel focus, workers and project tree state.
use crate::{cli::FileLocation, workspace::Workspace};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use vel_buffer::{Bias, BufferId, Region, Selection};
use vel_config::{
    Value, json,
    keymap::{Invocation, Key},
};
use vel_platform::{Command, Point, Rect, Size, TextInput};
use vel_render::scene::{Layer, Scene};
use vel_search::{
    files, fuzzy,
    goto::{Query, Target},
    project::{Entry, Folder, Index, Rules, sort_sidebar_entries},
    text::{self, Search},
};
mod appearance_selection;
mod details;
#[cfg(test)]
pub(crate) use details::StatusItem;
#[path = "text_ops.rs"]
mod macro_commands;
pub(crate) use macro_commands::usable_package_binding;
#[cfg(test)]
#[path = "panels_round19_tests.rs"]
mod round19_tests;
#[cfg(test)]
#[path = "panels_round20_perf.rs"]
mod round20_perf;
#[path = "sidebar_folders.rs"]
mod sidebar_folders;

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

/// File-type icon of a tab. Views without a path (default settings, untitled buffers) are
/// typed by their name, without a trailing " (Default)"-style qualifier.
pub(crate) fn tab_icon<'a>(
    workspace: &'a Workspace,
    tab: &crate::workspace::Tab,
) -> Option<&'a str> {
    match tab.path.as_deref() {
        Some(path) => workspace.themes.file_icon(path),
        None => {
            let name = tab.name();
            let name = name.split(" (").next().unwrap_or_default();
            workspace.themes.file_icon(Path::new(name))
        }
    }
}

pub const STATUS: f64 = 24.;
/// Document-format menu model; Lane B connects it to the status hit rectangles.
pub(crate) fn status_format_menu(
    workspace: &Workspace,
    item: vel_ui::status::FormatItem,
) -> Vec<vel_ui::status::FormatMenuItem> {
    let tab = &workspace.tabs[workspace.active];
    match item {
        vel_ui::status::FormatItem::Encoding => vel_ui::status::encoding_menu(
            vel_fileio::ENCODINGS,
            tab.format.display_encoding(),
            tab.path.is_some(),
            !tab.editor.read_only && !workspace.is_empty(),
        ),
        vel_ui::status::FormatItem::LineEndings => {
            vel_ui::status::line_ending_menu(match tab.format.line_ending {
                vel_fileio::LineEnding::Lf => "unix",
                vel_fileio::LineEnding::CrLf => "windows",
                vel_fileio::LineEnding::Cr => "cr",
            })
        }
    }
}
/// Pure input model for the status text; shared by paint and headless App tests.
pub(crate) fn status_position(workspace: &Workspace) -> String {
    let editor = workspace.editor();
    let tab = &workspace.tabs[workspace.active];
    let regions = editor.regions();
    let (row, column, selected) = if editor.binary.is_some() {
        (
            editor.caret / 16 + 1,
            editor.caret % 16 + 1,
            editor.selection().len(),
        )
    } else {
        (
            editor.text.byte_to_line(editor.caret) + 1,
            editor.status_column.borrow_mut().column(
                &editor.text,
                (editor.buffer.id(), editor.buffer.change_id()),
                editor.caret,
                editor.preferences.tab_size,
                crate::large::LONG_LINE,
            ),
            regions
                .iter()
                .filter(|r| !r.empty())
                .map(|r| editor.text.slice(r.to_range()).len_chars())
                .sum(),
        )
    };
    vel_ui::status::position_mode(
        editor.settings.string("show_line_column", "compact"),
        row,
        column,
        regions.len(),
        selected,
        editor
            .settings
            .bool("show_encoding", false)
            .then_some(tab.format.display_encoding()),
        editor
            .settings
            .bool("show_line_endings", false)
            .then_some(tab.format.line_ending.display_name()),
    )
}
type DirectoryResult = (u64, PathBuf, Result<Vec<Entry>, String>);
#[derive(Debug)]
struct RecursiveExpansion {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    result: mpsc::Receiver<vel_search::project::SidebarSubtree>,
}
impl Drop for RecursiveExpansion {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
const PANEL_ROW: f64 = 34.;
/// Side of the find-panel close button, in points.
const CLOSE_BUTTON: f64 = 22.;
/// Retained widget id of the find-panel close button.
const CLOSE_ID: u64 = 60;
/// Theme style-cache ids for chrome painted outside the retained widget tree. Every element
/// kind owns a disjoint range far above sheet-, row- and widget-based ids, so caches and
/// transitions of different elements never share an id.
#[derive(Clone, Copy)]
pub(crate) enum StyleId {
    OverlayContainer = 1,
    OverlayKindLabel,
    OverlayFooter,
    FindClose,
    ToolTip,
    ToolTipLabel,
    GroupDivider,
    TabLabel,
    TabClose,
    TabConnector,
    TabBarButton,
    ReviewMenuRow,
    GitBadge,
    ResultRow,
    TabBar,
    TabSet,
    TitleBar,
    ReviewMenu,
    HoverPopup,
    QuickScroll,
    StatusButton,
    SideGuide,
}
impl StyleId {
    pub(crate) const fn at(self, index: u64) -> u64 {
        (0x56 << 56) | ((self as u64) << 40) | (index & 0xFF_FFFF_FFFF)
    }
}
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
    Appearance,
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
    sheets: BTreeSet<u64>,
    buffer: BufferId,
    version: vel_buffer::ChangeId,
    selection: Selection,
    scroll: (f64, f64),
    transient: bool,
    path: Option<Arc<PathBuf>>,
    position: vel_text::Point,
}
#[derive(Clone, Copy, Debug)]
enum FindNavigation {
    Preview(usize),
    Next {
        backwards: bool,
        close: bool,
        report: bool,
    },
    All {
        close: bool,
    },
}
impl FindNavigation {
    fn closes(self) -> bool {
        matches!(
            self,
            Self::Next { close: true, .. } | Self::All { close: true }
        )
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FindSnapshot {
    sheet: u64,
    buffer: BufferId,
    revision: u64,
    loading: bool,
    binary: bool,
}
impl FindSnapshot {
    fn current(workspace: &Workspace) -> Self {
        let tab = &workspace.tabs[workspace.active];
        Self {
            sheet: tab.sheet,
            buffer: tab.editor.buffer.id(),
            revision: tab.editor.revision,
            loading: tab.loading.is_some() || tab.binary_loading.is_some(),
            binary: tab.editor.binary.is_some(),
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
struct FindIdentity {
    snapshot: FindSnapshot,
    query: String,
    options: text::Options,
    in_selection: bool,
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
    appearance_selection: Option<appearance_selection::SelectionPanel>,
    macro_files: macro_commands::MacroFiles,
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
    hover_definitions: usize,
    hover_source: Option<(u64, BufferId, vel_buffer::ChangeId, usize)>,
    hover_caret: usize,
    navigation_options: crate::intelligence::OpenOptions,
    navigation_selection: Option<(usize, BTreeSet<u64>)>,
    navigation_preview_guard: Option<crate::editor::navigation::Guard>,
    hover_point: Point,
    hover_term: String,
    pub(crate) hover_since: Instant,
    hover_requested: bool,
    hover_retry: Option<Instant>,
    hover_bounds: Rect,
    hover_html: Option<vel_ui::minihtml::Popup>,
    pub restore_scroll: Option<(f64, f64)>,
    pub reveal: bool,
    preserve_scroll: bool,
    incremental_origin: Option<(Preview, BufferId, vel_buffer::ChangeId)>,
    find_origin: Option<(BufferId, usize)>,
    find_jump_origin: Option<crate::intelligence::Jump>,
    history_cursor: [Option<usize>; 2],
    history_draft: [String; 2],
    message_seen: String,
    message_until: Option<Instant>,
    pub search_results_active: bool,
    result_cursor: Option<usize>,
    files_searched: usize,
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
    pub open_files_visible: bool,
    pub side_focused: bool,
    pub side_selected: usize,
    side_selection: Option<PathBuf>,
    sidebar_reveal: Option<PathBuf>,
    sidebar_aliases: BTreeMap<u64, PathBuf>,
    side_open: Option<usize>,
    pub side_scroll: Scroll,
    pub side_rows: Vec<SideRow>,
    tree_motion: Option<(Instant, BTreeMap<PathBuf, f64>)>,
    pub primary_click: bool,
    pub build_results: Vec<(usize, FileLocation)>,
    pub build_output_offset: usize,
    pub find_history: Vec<String>,
    pub replace_history: Vec<String>,
    pub chrome: bool,
    choice_items: Vec<vel_config::ui::PaletteItem>,
    side_anchor: Option<PathBuf>,
    pub dragging_tab: Option<usize>,
    pub options: text::Options,
    pub in_selection: bool,
    pub highlights: Vec<Range<usize>>,
    pub message: String,
    pub result_view: files::Results,
    pub result_selected: usize,
    result_navigated: bool,
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
    /// Pending prefix replacement of the mirrored output: (old bytes removed, new bytes).
    output_trim: Option<(usize, usize)>,
    /// Overlay in a window too short for its footer (see `quick_chrome`).
    quick_tight: bool,
    /// Status-bar items placed by the last paint, right to left; clicks use these rects.
    /// None until the first paint.
    status_items: Option<Vec<(details::StatusItem, Rect)>>,
    /// Find-panel controls laid out for panel, bounds, and both scales.
    find_layout: Option<(Panel, Rect, f64, f64, details::FindControls)>,
    /// State the shown tool tip was last derived from (control, options, panel).
    tooltip_key: u64,
    output_mirrored: usize,
    pub input_caption: String,
    pub input_done: Option<Invocation>,
    focus_input: usize,
    selection_scope: Vec<Range<usize>>,
    scope_revision: Option<(BufferId, vel_buffer::ChangeId)>,
    scope_selection: Option<(u64, Selection)>,
    scope_manual: bool,
    search: Option<Search>,
    find_error: Option<String>,
    find_status: String,
    search_worker: text::Worker,
    search_identity: Option<FindIdentity>,
    all_matches_ready: bool,
    pending_navigation: VecDeque<FindNavigation>,
    fuzzy_worker: fuzzy::Worker,
    candidates: Arc<Vec<fuzzy::Candidate>>,
    palette_indices: Vec<usize>,
    paths: Arc<Vec<PathBuf>>,
    file_paths: Arc<Vec<PathBuf>>,
    file_candidates: Arc<Vec<fuzzy::Candidate>>,
    index: Option<Index>,
    folder_key: Vec<Folder>,
    extra_paths: Vec<PathBuf>,
    extra_candidates: Arc<Vec<fuzzy::Candidate>>,
    file_recency: BTreeMap<usize, u64>,
    palette_recent: HashMap<String, u64>,
    uses: u64,
    preview: Option<Preview>,
    last_preview: Option<(PathBuf, Target)>,
    preview_symbol_version: Option<(u64, vel_buffer::ChangeId)>,
    project_symbol_source: Vec<(PathBuf, usize, usize, usize)>,
    local_kinds: Vec<vel_index::SymbolKind>,
    expanded: HashSet<PathBuf>,
    recursive_expansion: Option<RecursiveExpansion>,
    children: HashMap<PathBuf, Vec<Entry>>,
    directories: mpsc::Receiver<DirectoryResult>,
    directory_tx: mpsc::Sender<DirectoryResult>,
    loading: BTreeSet<PathBuf>,
    directory_pending: BTreeSet<PathBuf>,
    directory_running: usize,
    directory_generation: u64,
    directory_cancel: Arc<AtomicBool>,
    watched_expanded: bool,
    folder_edits: Vec<sidebar_folders::Pending>,
    local: Option<mpsc::Receiver<Local>>,
    local_positions: Vec<usize>,
    local_generation: u64,
    quick_revision: u64,
    quick_query: String,
    quick_target: &'static str,
    quick_ready: bool,
    prepared: String,
    palette_scope_request: Option<(BufferId, vel_buffer::ChangeId, usize, u64)>,
    pub quick_elapsed_ms: f64,
    pub quick_pending: bool,
    panel_bounds: Rect,
    pub(crate) quick_bounds: Rect,
    pub(crate) side_bounds: Rect,
    size: Size,
}
impl Default for Panels {
    fn default() -> Self {
        let (directory_tx, directories) = mpsc::channel();
        Self {
            appearance_selection: None,
            macro_files: Default::default(),
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
            hover_definitions: 0,
            hover_source: None,
            hover_caret: 0,
            navigation_options: Default::default(),
            navigation_selection: None,
            navigation_preview_guard: None,
            hover_point: Point::default(),
            hover_term: String::new(),
            hover_since: Instant::now(),
            hover_requested: false,
            hover_retry: None,
            hover_bounds: Rect::default(),
            hover_html: None,
            restore_scroll: None,
            reveal: false,
            preserve_scroll: false,
            incremental_origin: None,
            find_origin: None,
            find_jump_origin: None,
            history_cursor: [None, None],
            history_draft: Default::default(),
            message_seen: String::new(),
            message_until: None,
            search_results_active: false,
            result_cursor: None,
            files_searched: 0,
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
            open_files_visible: true,
            side_selected: 0,
            side_selection: None,
            sidebar_reveal: None,
            sidebar_aliases: BTreeMap::new(),
            side_open: None,
            side_scroll: Scroll::default(),
            side_rows: vec![],
            tree_motion: None,
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
            result_navigated: false,
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
            output_trim: None,
            find_layout: None,
            quick_tight: false,
            status_items: None,
            tooltip_key: 0,
            output_mirrored: 0,
            input_caption: String::new(),
            input_done: None,
            focus_input: 0,
            selection_scope: vec![],
            scope_revision: None,
            scope_selection: None,
            scope_manual: false,
            search: None,
            find_error: None,
            find_status: String::new(),
            search_worker: text::Worker::default(),
            search_identity: None,
            all_matches_ready: false,
            pending_navigation: VecDeque::new(),
            fuzzy_worker: fuzzy::Worker::new(),
            candidates: Arc::new(vec![]),
            palette_indices: Vec::new(),
            paths: Arc::new(vec![]),
            file_paths: Arc::new(vec![]),
            file_candidates: Arc::new(vec![]),
            index: None,
            folder_key: Vec::new(),
            extra_paths: Vec::new(),
            extra_candidates: Arc::default(),
            file_recency: BTreeMap::new(),
            palette_recent: HashMap::new(),
            uses: 0,
            preview: None,
            last_preview: None,
            preview_symbol_version: None,
            project_symbol_source: Vec::new(),
            local_kinds: Vec::new(),
            expanded: HashSet::new(),
            recursive_expansion: None,
            children: HashMap::new(),
            directories,
            directory_tx,
            loading: BTreeSet::new(),
            directory_pending: BTreeSet::new(),
            directory_running: 0,
            directory_generation: 0,
            directory_cancel: Arc::default(),
            watched_expanded: false,
            folder_edits: Vec::new(),
            local: None,
            local_positions: vec![],
            local_generation: 0,
            quick_revision: 0,
            quick_query: String::new(),
            quick_target: "",
            quick_ready: false,
            prepared: String::new(),
            palette_scope_request: None,
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
            chrome: workspace.window_visible("show_status_bar"),
            side_visible: workspace.editor().settings.bool("show_side_bar", true),
            ..Self::default()
        };
        if workers {
            ui.restart_index(workspace);
        }
        ui
    }
    fn restart_index(&mut self, workspace: &Workspace) {
        self.folder_key.clone_from(&workspace.folders);
        self.reset_index(workspace);
        self.children.clear();
        self.expanded.retain(|path| {
            workspace
                .folders
                .iter()
                .any(|f| path.starts_with(&f.path) && f.rules.allows(&f.path, path, true, false))
        });
        self.reload_expanded(workspace);
        self.rebuild_side(workspace);
    }
    fn reset_index(&mut self, workspace: &Workspace) {
        self.recursive_expansion = None;
        self.index = (!workspace.folders.is_empty()).then(|| Index::new(workspace.folders.clone()));
        self.directory_cancel.store(true, Ordering::Release);
        self.directory_cancel = Arc::default();
        self.directory_generation = self.directory_generation.wrapping_add(1);
        self.directory_pending.clear();
        self.loading.clear();
        self.watched_expanded = false;
    }
    fn reload_expanded(&mut self, workspace: &Workspace) {
        self.directory_pending.extend(self.expanded.iter().cloned());
        self.pump_directories(workspace);
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
            // Output grows by megabytes. Pin the large-document policy (it would change the
            // minimap sampling every 10,000 lines and the highlighting mode at long lines),
            // because each policy change rebuilds the language and re-wraps every retained
            // line; nothing here is spell checked, diffed or shown in a minimap anyway.
            ("spell_check", json!(false)),
            ("large_file.enable_minimap", json!(true)),
            ("large_file.enable_highlighting", json!(true)),
            ("large_file.enable_spell_check", json!(true)),
            ("large_file.enable_diff", json!(true)),
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
    /// Records that the build output's first `removed` bytes were replaced by `inserted`
    /// bytes (retention trimming), so the mirror deletes only that prefix.
    pub(crate) fn trim_output_prefix(&mut self, removed: usize, inserted: usize) {
        if self.output_reset || self.output_trim.is_some() {
            // A second trim before the mirror caught up: rebuild it instead.
            self.output_reset = true;
            self.output_trim = None;
        } else {
            self.output_trim = Some((removed, inserted));
        }
    }
    /// Whether the read-only output view still lags the build output.
    fn output_pending(&self) -> bool {
        self.output_editor.is_some()
            && (self.output_reset
                || self.output_trim.is_some()
                || self.output_mirrored != self.output.len())
    }
    /// Mirrors build output into the read-only output view. Appends and trims are applied
    /// incrementally and at most `OUTPUT_SYNC_BYTES` are inserted per frame, so megabytes of
    /// output never stall a frame; `busy` keeps frames coming until the view caught up.
    fn sync_output(&mut self) -> bool {
        const OUTPUT_SYNC_BYTES: usize = 128 * 1024;
        let pending = self.output_pending();
        let Some(editor) = self.output_editor.as_mut() else {
            return false;
        };
        let changed = editor.poll_language();
        if !pending {
            return changed;
        }
        // The output view is read-only for users; mirror the build output through it.
        let read_only = std::mem::replace(&mut editor.read_only, false);
        let edit = |editor: &mut crate::editor::Editor, range, text| {
            editor.apply_edits(vec![(range, text)], None, vel_buffer::EditKind::Other);
            editor.sync();
        };
        if let Some((removed, inserted)) = self.output_trim.take()
            && !self.output_reset
        {
            let cut = removed.min(self.output_mirrored);
            edit(editor, 0..cut, self.output[..inserted].to_owned());
            self.output_mirrored = self.output_mirrored.max(removed) - removed + inserted;
        }
        if self.output_reset || self.output_mirrored > self.output.len() {
            edit(editor, 0..editor.text.len_bytes(), String::new());
            self.output_mirrored = 0;
            self.output_reset = false;
            self.output_trim = None;
        }
        if self.output_mirrored < self.output.len() {
            let end = self.output.floor_char_boundary(
                (self.output_mirrored + OUTPUT_SYNC_BYTES).min(self.output.len()),
            );
            let at = editor.text.len_bytes();
            edit(
                editor,
                at..at,
                self.output[self.output_mirrored..end].to_owned(),
            );
            self.output_mirrored = end;
        }
        editor.read_only = read_only;
        true
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
        window.sidebar_open_files = Some(self.open_files_visible);
        if window.distraction_free {
            window
                .distraction_ui
                .insert("show_side_bar".into(), self.side_visible);
        } else {
            window
                .normal_ui
                .insert("show_side_bar".into(), self.side_visible);
        }
        window.sidebar_expanded = self.expanded.iter().cloned().collect();
        window.sidebar_expanded.sort_unstable();
        window.sidebar_scroll = self.side_scroll.offset;
        window.find_history = self.find_history.clone();
        window.replace_history = self.replace_history.clone();
    }
    pub fn restore_session(&mut self, workspace: &Workspace, window: &vel_session::Window) {
        self.side_visible = window.sidebar_visible;
        self.open_files_visible = window.sidebar_open_files.unwrap_or(true);
        self.expanded = window.sidebar_expanded.iter().cloned().collect();
        self.watched_expanded = false;
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
        self.chrome = workspace.window_visible("show_status_bar");
        self.side_visible = workspace.window_visible("show_side_bar");
    }
    pub fn add_folder(&mut self, workspace: &mut Workspace, path: PathBuf) -> Result<(), String> {
        self.edit_folders(workspace, sidebar_folders::Edit::Add(path))
    }
    fn install_folder(&mut self, workspace: &mut Workspace, path: PathBuf) -> Result<(), String> {
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
        self.side_visible = true;
        workspace.set_window_toggle("show_side_bar", true)?;
        Ok(())
    }
    fn load_directory(&mut self, workspace: &Workspace, path: &Path) {
        self.watched_expanded = false;
        self.request_directory(workspace, path, false);
    }
    fn request_directory(&mut self, workspace: &Workspace, path: &Path, refresh: bool) {
        if !refresh && (self.children.contains_key(path) || self.loading.contains(path)) {
            return;
        }
        self.directory_pending.insert(path.to_owned());
        self.pump_directories(workspace);
    }
    fn pump_directories(&mut self, workspace: &Workspace) {
        while self.directory_running < 8 {
            let Some(path) = self.directory_pending.pop_first() else {
                break;
            };
            if self.loading.contains(&path) {
                self.directory_pending.insert(path);
                break;
            }
            let Some(folder) = workspace
                .folders
                .iter()
                .filter(|f| path.starts_with(&f.path))
                .max_by_key(|f| f.path.as_os_str().len())
                .cloned()
            else {
                continue;
            };
            let tx = self.directory_tx.clone();
            let cancel = self.directory_cancel.clone();
            let generation = self.directory_generation;
            self.loading.insert(path.clone());
            self.directory_running += 1;
            std::thread::spawn(move || {
                let children = folder.sidebar_children(&path, &cancel);
                let _ = tx.send((generation, path, children));
            });
        }
    }
    fn tree_row_position(&self, index: usize, now: Instant) -> f64 {
        let target = index as f64;
        let Some((started, positions)) = &self.tree_motion else {
            return target;
        };
        let row = &self.side_rows[index];
        let from = positions
            .get(&row.path)
            .copied()
            .or_else(|| {
                row.path
                    .ancestors()
                    .skip(1)
                    .find_map(|p| positions.get(p).copied())
            })
            .unwrap_or(target);
        let t = (now.saturating_duration_since(*started).as_secs_f64() / 0.14).clamp(0., 1.);
        from + (target - from) * (1. - (1. - t).powi(3))
    }
    fn visible_open_files(&self, workspace: &Workspace) -> usize {
        if self.open_files_visible {
            open_file_count(workspace)
        } else {
            0
        }
    }
    /// Rows above the first side bar file row: the Open Files block and the Folders heading.
    pub(crate) fn folder_row_base(&self, workspace: &Workspace) -> usize {
        self.visible_open_files(workspace) + 1 + usize::from(self.open_files_visible)
    }
    fn select_side(&mut self, index: usize) {
        self.side_selected = index;
        self.side_selection = self.side_rows.get(index).map(|row| row.path.clone());
    }

    #[cfg(test)]
    pub(crate) fn test_sidebar_point(&self, workspace: &Workspace, path: &Path) -> Point {
        let index = self
            .side_rows
            .iter()
            .position(|r| r.path == path)
            .expect("visible sidebar row");
        Point {
            x: self.side_bounds.x + 8.,
            y: self.side_bounds.y
                + (self.folder_row_base(workspace) as f64
                    + self.tree_row_position(index, Instant::now())
                    + 0.5)
                    * 24.
                    * self.ui_scale
                - self.side_scroll.offset,
        }
    }
    fn rebuild_side(&mut self, workspace: &Workspace) {
        fn visit(
            rows: &mut Vec<SideRow>,
            children: &HashMap<PathBuf, Vec<Entry>>,
            expanded: &HashSet<PathBuf>,
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
            if directory
                && expanded.contains(path)
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
        let now = Instant::now();
        let old_len = self.side_rows.len();
        let animate = workspace
            .editor()
            .settings
            .bool("tree_animation_enabled", true)
            && workspace.editor().settings.bool("animation_enabled", true)
            && !workspace.themes.ui.borrow().reduce_motion();
        let in_view = self.side_scroll.visible();
        let first_folder = self.folder_row_base(workspace);
        let first = in_view.start.saturating_sub(first_folder).min(old_len);
        let last = in_view.end.saturating_sub(first_folder).min(old_len);
        let old_rows = if animate {
            (first..last)
                .map(|i| {
                    (
                        self.side_rows[i].path.clone(),
                        self.tree_row_position(i, now),
                    )
                })
                .collect::<BTreeMap<_, _>>()
        } else {
            BTreeMap::new()
        };
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
        // A new event starts from the current drawn positions, so repeated expand/collapse
        // gestures converge without jumping or accumulating offsets.
        // https://sublime-text-unofficial-documentation.readthedocs.io/en/latest/reference/settings.html#visual-settings
        let changed_rows = old_len != self.side_rows.len()
            || self
                .side_rows
                .iter()
                .enumerate()
                .skip(first)
                .take(last.saturating_sub(first))
                .any(|(i, row)| old_rows.get(&row.path) != Some(&(i as f64)));
        if animate && !old_rows.is_empty() && changed_rows {
            self.tree_motion = Some((now, old_rows));
        } else if !changed_rows { /* retain an active transition */
        } else {
            self.tree_motion = None;
        }
        self.expire_message(now);
        self.side_scroll.rows = self.folder_row_base(workspace) + self.side_rows.len();
        self.side_scroll.row_height = 24. * self.ui_scale;
        self.side_selected = self.side_selection.as_ref().map_or(0, |path| {
            self.side_rows
                .iter()
                .position(|row| &row.path == path)
                .unwrap_or(self.side_rows.len())
        });
        if let Some(index) = self
            .sidebar_reveal
            .as_ref()
            .and_then(|path| self.side_rows.iter().position(|row| &row.path == path))
        {
            self.select_side(index);
            self.side_open = None;
            self.side_scroll
                .reveal(self.folder_row_base(workspace) + index);
            self.sidebar_reveal = None;
        }
    }
    /// Refreshes directory snapshots and indexes after an explicit filesystem operation.
    pub fn refresh_sidebar(&mut self, workspace: &mut Workspace) {
        self.reset_index(workspace);
        self.reload_expanded(workspace);
    }
    fn close_folder_list(&mut self, workspace: &mut Workspace) -> Result<(), String> {
        // https://docs.sublimetext.io/reference/commands.html#close-folder-list
        self.edit_folders(workspace, sidebar_folders::Edit::Clear)
    }
    pub(crate) fn note_sidebar_spelling(&mut self, workspace: &Workspace, path: &Path) {
        let tab = &workspace.tabs[workspace.active];
        let Ok(path) = std::path::absolute(path) else {
            return;
        };
        if tab
            .path
            .as_ref()
            .is_some_and(|canonical| canonical != &path)
            && workspace.folders.iter().any(|folder| {
                folder.follow_links
                    && path.starts_with(&folder.path)
                    && folder.rules.allows(&folder.path, &path, false, false)
            })
        {
            self.sidebar_aliases
                .retain(|sheet, _| workspace.sheet_index(*sheet).is_some());
            self.sidebar_aliases.insert(tab.sheet, path);
        }
    }
    /// Reveals a document by expanding its ancestors; directory reads stay on workers.
    pub fn reveal_sidebar_path(
        &mut self,
        workspace: &mut Workspace,
        path: PathBuf,
    ) -> Result<(), String> {
        // ST4 4126: https://www.sublimetext.com/download
        // A project with follow_symlinks enabled shows the original path spelling.
        let path = workspace
            .tabs
            .iter()
            .find(|t| t.path.as_ref() == Some(&path))
            .and_then(|t| self.sidebar_aliases.get(&t.sheet))
            .filter(|p| {
                workspace.folders.iter().any(|f| {
                    f.follow_links
                        && p.starts_with(&f.path)
                        && f.rules.allows(&f.path, p, false, false)
                })
            })
            .cloned()
            .unwrap_or(path);
        if !workspace
            .folders
            .iter()
            .any(|folder| path.starts_with(&folder.path))
        {
            let parent = path
                .parent()
                .ok_or("document has no parent folder")?
                .to_owned();
            self.sidebar_reveal = Some(path);
            return self.edit_folders(workspace, sidebar_folders::Edit::Reveal(parent));
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
        workspace.set_window_toggle("show_side_bar", true)?;
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
        workspace.set_window_toggle("show_side_bar", true)?;
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
        self.edit_folders(workspace, sidebar_folders::Edit::Remove(path.to_owned()))
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
                self.close_panel(workspace);
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
        if matches!(
            self.panel,
            Some(Panel::Find | Panel::Replace | Panel::Incremental)
        ) && self.find_jump_origin.is_none()
        {
            self.find_jump_origin = Some(crate::intelligence::capture(workspace));
        }

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
        if self.panel == Some(Panel::Files) {
            self.history_cursor[0] = None;
            return;
        }
        let changed = self.search_identity.as_ref().is_none_or(|key| {
            key.query != self.query.value()
                || key.options != self.options
                || key.in_selection != self.in_selection
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
            self.queue_find(workspace, FindNavigation::Preview(origin));
        }
    }
    fn set_query_from_buffer(&mut self, text: &str) {
        // https://www.sublimetext.com/download (escape patterns taken from a file).
        if self.options.regex {
            self.query.set(&text::escape_literal(text));
        } else {
            self.query.set(text);
        }
    }
    pub fn edited(&mut self, workspace: &mut Workspace, viewport: Range<usize>) {
        self.input_changed(workspace, viewport);
    }
    pub fn busy(&self) -> bool {
        if !self.folder_edits.is_empty()
            || self.directory_running > 0
            || self.recursive_expansion.is_some()
        {
            return true;
        }
        self.review_busy()
            || self.overlay.is_some()
            || self.intel.busy()
            || (!self.hover_term.is_empty()
                && !self.hover_requested
                && self.hover_since.elapsed() < Duration::from_millis(250))
            || self.file_task.is_some()
            || self.replace_rx.is_some()
            || self
                .output_editor
                .as_ref()
                .is_some_and(|e| e.language.borrow().as_ref().is_some_and(|l| l.busy()))
            || self.output_pending()
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
    fn location_items(&mut self, workspace: &Workspace) {
        let mut scorer = fuzzy::LabelMatcher::new(&self.quick_input.value());
        self.quick.set_items(
            self.locations
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    let path = workspace
                        .folders
                        .iter()
                        .filter(|folder| l.file.starts_with(&folder.path))
                        .max_by_key(|folder| folder.path.as_os_str().len())
                        .and_then(|folder| l.file.strip_prefix(&folder.path).ok())
                        .unwrap_or(&l.file);
                    let mut highlights = Vec::new();
                    if self.overlay == Some(Overlay::ProjectSymbols) {
                        scorer.score(&l.name, Some(&mut highlights));
                    }
                    Item {
                        caption: l.name.clone(),
                        value: i,
                        kind: format!("{:?}", l.kind),
                        annotation: format!(
                            "{}:{}",
                            l.file.file_name().unwrap_or_default().to_string_lossy(),
                            l.row + 1
                        ),
                        details: path.display().to_string(),
                        highlights,
                    }
                })
                .collect(),
        );
    }
    fn hover_eligible(editor: &crate::editor::Editor, byte: usize) -> Option<bool> {
        let language = editor.language.borrow();
        let snapshot = language.as_ref()?.scope_snapshot_at(byte)?;
        use vel_highlight::ScopeProvider;
        let scopes = snapshot.scopes_at(byte)?;
        // Symbol hover: https://www.sublimetext.com/docs/indexing.html#goto-definition
        // and https://www.sublimetext.com/download (Goto Definition Popup).
        // The detail audit narrows this to code and HTML class/id attribute values.
        let names: Vec<_> = scopes.iter().map(ToString::to_string).collect();
        let is = |prefix: &str| {
            names
                .iter()
                .any(|s| s == prefix || s.starts_with(&format!("{prefix}.")))
        };
        if is("comment") || is("text.plain") {
            return Some(false);
        }
        let html_attribute = is("text.html") && (is("meta.class-name") || is("meta.toc-list"));
        if html_attribute {
            return Some(true);
        }
        Some(!is("string") && is("source"))
    }
    /// The pointer left the text area: drop a pending definition hover. An open popup
    /// stays while the pointer is over it, as with ordinary pointer moves.
    pub fn leave_hover(&mut self, point: Point) {
        if let Some(popup) = &self.hover_html
            && contains(self.hover_bounds, point)
            && popup.visible
        {
            return;
        }
        self.hover.clear();
        self.hover_html = None;
        self.hover_term.clear();
        self.hover_source = None;
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
        let editor = workspace.editor();
        let byte = editor
            .text
            .floor_char_boundary(byte.min(editor.text.len_bytes()));
        let source = (
            workspace.tabs[workspace.active].sheet,
            editor.buffer.id(),
            editor.buffer.change_id(),
            byte,
        );
        let character = (byte < editor.text.len_bytes())
            .then(|| {
                let end = editor.text.char_to_byte(editor.text.byte_to_char(byte) + 1);
                editor.text.slice(byte..end).chars().next()
            })
            .flatten();
        let term = if workspace.editor().settings.bool("show_definitions", true)
            && self.overlay.is_none()
            && workspace.editor().marked.is_none()
            && character.is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            // Pointer moves only capture a bounded word. Display eligibility is checked
            // after the hover delay, against published scopes, without synchronous parsing.
            crate::intelligence::word(workspace.editor(), byte)
        } else {
            String::new()
        };
        if term != self.hover_term || self.hover_source != Some(source) {
            self.hover.clear();
            self.hover_html = None;
            self.hover_term = term;
            self.hover_since = Instant::now();
            self.hover_requested = false;
            self.hover_retry = None;
            self.hover_source = Some(source);
            self.hover_caret = editor.caret;
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
                workspace
                    .jump_history
                    .borrow_mut()
                    .remember(crate::intelligence::capture(workspace));
                crate::intelligence::jump_with_options(
                    workspace,
                    &location,
                    false,
                    Default::default(),
                )?;
                workspace
                    .jump_history
                    .borrow_mut()
                    .reset_current(crate::intelligence::capture(workspace));
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
        if self.overlay.is_none() {
            workspace.observe_navigation();
        }
        let _navigation =
            crate::editor::navigation::Guard::command_args(&workspace.jump_history, name, args);

        if name == "show_overlay" && args["overlay"] == "snippet_choices" {
            let items = args["names"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|name| {
                    let name = name.as_str()?;
                    let snippet = workspace.editor().snippets.get(name)?;
                    let mut invocation_args = args.clone();
                    invocation_args["name"] = json!(name);
                    Some(vel_config::ui::PaletteItem {
                        caption: if snippet.description().is_empty() {
                            name.into()
                        } else {
                            snippet.description().to_owned()
                        },
                        invocation: Invocation {
                            command: "insert_snippet".into(),
                            args: invocation_args,
                        },
                    })
                })
                .collect();
            self.choices(workspace, items);
            return Ok(true);
        }
        if name == "save_macro" || (name == "run_macro_file" && args.get("file").is_none()) {
            return self.macro_action(workspace, name, args, viewport);
        }
        if workspace.editor().binary.is_some()
            && matches!(
                name,
                "slurp_find_string"
                    | "slurp_replace_string"
                    | "find_under"
                    | "find_under_prev"
                    | "find_next"
                    | "find_prev"
                    | "find_all"
                    | "replace_next"
                    | "replace_all"
            )
        {
            self.set_message("Text search is unavailable in hex and image tabs.".into());
            return Ok(true);
        }
        // Record the document find operation at the interception boundary. Query-field
        // typing remains a panel edit and never enters the application macro buffer.
        if vel_commands::is_find_command(name) {
            if let Some(find) = args.get("_macro_find") {
                self.query.set(find["query"].as_str().unwrap_or(""));
                self.replace.set(find["replacement"].as_str().unwrap_or(""));
                self.options.regex = find["regex"].as_bool().unwrap_or(false);
                self.options.case_sensitive = find["case_sensitive"].as_bool().unwrap_or(false);
                self.options.whole_word = find["whole_word"].as_bool().unwrap_or(false);
                self.options.wrap = find["wrap"].as_bool().unwrap_or(true);
                self.options.preserve_case = find["preserve_case"].as_bool().unwrap_or(false);
                self.in_selection = find["in_selection"].as_bool().unwrap_or(false);
            }
            if workspace.editor().registry.is_recording()
                && workspace.editor().registry.playback_depth() == 0
            {
                let mut recorded = args.clone();
                recorded["_macro_find"] = json!({"query":self.query.value(), "replacement":self.replace.value(),
                    "regex":self.options.regex,"case_sensitive":self.options.case_sensitive,
                    "whole_word":self.options.whole_word,"wrap":self.options.wrap,
                    "preserve_case":self.options.preserve_case,"in_selection":self.in_selection});
                workspace
                    .editor_mut()
                    .registry
                    .record_command(name, &recorded);
            }
        }
        let _find_recording_guard = vel_commands::is_find_command(name)
            .then(|| workspace.editor().registry.suspend_recording());
        if workspace.editor().binary.is_some() && name == "goto_line" {
            return self.action(workspace, "show_input_panel", &json!({"caption":"Goto byte offset (decimal or 0x hex)","initial_text":format!("0x{:x}", workspace.editor().caret),"on_done":{"command":"goto_hex_offset","args":{}}}), viewport);
        }
        if self.review_action(workspace, name, args)? {
            return Ok(true);
        }
        match name {
            "select_color_scheme" | "select_theme" => {
                self.open_appearance(workspace, name == "select_theme");
            }
            "goto_symbol_in_project" => {
                self.action(workspace, "show_overlay", &json!({}), viewport)?;
                self.overlay = Some(Overlay::ProjectSymbols);
                self.prepared.clear();
                self.locations.clear();
                self.query_quick(workspace);
            }
            "goto_definition" | "goto_reference" => {
                self.cancel_overlay(workspace);
                self.hover.clear();
                let term = crate::intelligence::word(workspace.editor(), workspace.editor().caret);
                if !term.is_empty() {
                    self.navigation_origin = Some(crate::intelligence::capture(workspace));
                    self.navigation_options = crate::intelligence::OpenOptions {
                        side_by_side: args["side_by_side"].as_bool().unwrap_or(false),
                        clear_to_right: args["clear_to_right"].as_bool().unwrap_or(false),
                        origin: workspace.tabs[workspace.active].sheet,
                    };
                    self.navigation_selection = Some((
                        workspace.workbench.active,
                        workspace.workbench.groups[workspace.workbench.active]
                            .selected
                            .clone(),
                    ));
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
                workspace.jump_history(name == "jump_forward");
                if let Some(message) = workspace.jump_history.borrow_mut().notice.take() {
                    self.set_message(message);
                }
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
            "toggle_minimap" | "toggle_tabs" | "toggle_status_bar" => {
                let key = match name {
                    "toggle_minimap" => "show_minimap",
                    "toggle_tabs" => "show_tabs",
                    _ => "show_status_bar",
                };
                workspace.set_window_toggle(key, !workspace.window_visible(key))?;
                self.chrome = workspace.window_visible("show_status_bar");
            }
            "goto_line" if args["line"].is_number() => {
                // https://forum.sublimetext.com/t/19333: goto_line takes a one-based line.
                let line = args["line"]
                    .as_u64()
                    .ok_or("goto_line needs a nonnegative integer")?;
                self.cancel_overlay(workspace);
                workspace.observe_navigation();
                let _navigation =
                    crate::editor::navigation::Guard::command(&workspace.jump_history, "goto_line");
                workspace.tabs[workspace.active].goto(&FileLocation {
                    path: PathBuf::new(),
                    line: usize::try_from(line).unwrap_or(usize::MAX),
                    column: 1,
                });
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
                    sheets: workspace.tabs.iter().map(|tab| tab.sheet).collect(),
                    buffer: workspace.editor().buffer.id(),
                    version: workspace.editor().buffer.change_id(),
                    scroll: workspace.tabs[workspace.active].scroll,
                    transient: workspace.tabs[workspace.active].transient,
                    path: workspace.tabs[workspace.active].path.clone().map(Arc::new),
                    position: crate::editor::navigation::position(
                        &workspace.editor().text,
                        workspace.editor().caret,
                    ),
                    selection: workspace
                        .editor()
                        .buffer
                        .view(workspace.editor().id)
                        .sel()
                        .clone(),
                });
                // Keep a pre-existing side-bar preview alive while Goto reuses its own transient tab.
                if self.overlay == Some(Overlay::Goto) {
                    self.navigation_preview_guard = Some(
                        crate::editor::navigation::Guard::suppress(&workspace.jump_history),
                    );
                }
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
                self.preview_symbol_version = None;
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
                    self.close_panel(workspace);
                    return Ok(true);
                }
                self.cancel_overlay(workspace);
                let previous_panel = self.panel;
                self.finish_find_navigation(workspace);
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
                // https://www.sublimetext.com/download (multi-line find fields expand).
                self.query.set_multiline(self.find_panel_visible());
                self.replace
                    .set_multiline(matches!(self.panel, Some(Panel::Replace | Panel::Files)));
                if matches!(
                    self.panel,
                    Some(Panel::Find | Panel::Replace | Panel::Incremental)
                ) {
                    self.find_jump_origin = Some(crate::intelligence::capture(workspace));
                }
                if self.panel == Some(Panel::Incremental) && previous_panel != self.panel {
                    let editor = workspace.editor();
                    self.incremental_origin = Some((
                        Preview {
                            sheet: workspace.tabs[workspace.active].sheet,
                            sheets: workspace.tabs.iter().map(|tab| tab.sheet).collect(),
                            buffer: editor.buffer.id(),
                            version: editor.buffer.change_id(),
                            selection: editor.buffer.view(editor.id).sel().clone(),
                            scroll: workspace.tabs[workspace.active].scroll,
                            transient: workspace.tabs[workspace.active].transient,
                            path: workspace.tabs[workspace.active].path.clone().map(Arc::new),
                            position: crate::editor::navigation::position(
                                &editor.text,
                                editor.caret,
                            ),
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
                if !self.document_find_visible() || previous_panel != self.panel {
                    self.scope_manual = false;
                    self.in_selection = false;
                    self.scope_selection = None;
                    self.cancel_pending_find();
                }
                if self.document_find_visible() && self.scope_selection.is_none() {
                    self.capture_find_scope(workspace);
                    let enabled = match workspace.editor().settings.get("auto_find_in_selection") {
                        Some(Value::Bool(enabled)) => *enabled,
                        Some(Value::String(enabled)) => match enabled.as_str() {
                            "find_only" => self.panel != Some(Panel::Replace),
                            "replace_only" => self.panel == Some(Panel::Replace),
                            _ => false,
                        },
                        _ => false,
                    };
                    self.in_selection = enabled && self.multiline_scope(workspace);
                }
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
                    // https://www.sublimetext.com/download (show_panel arguments).
                    if let Some(query) = args["pattern"].as_str().or(args["text"].as_str()) {
                        self.query.set(query);
                    } else if workspace.editor().settings.bool("find_selected_text", true)
                        && workspace.editor().binary.is_none()
                        && !workspace.editor().selection().is_empty()
                    {
                        let selected = workspace
                            .editor()
                            .text
                            .slice(workspace.editor().selection());
                        // https://docs.sublimetext.io/reference/settings.html#automatic-behavior
                        // Public docs establish selection seeding, not empty-caret word seeding.
                        if !selected.chars().any(|c| matches!(c, '\n' | '\r')) {
                            self.set_query_from_buffer(&selected.to_string());
                        }
                    }
                    if let Some(replacement) = args["replace_pattern"].as_str() {
                        self.replace.set(replacement);
                    }
                    self.query.run("select_all", &json!({}))?;
                    if self.panel == Some(Panel::Replace) {
                        self.replace.run("select_all", &json!({}))?;
                    }
                    if self.document_find_visible() {
                        self.refresh_find(workspace, viewport);
                    }
                }
            }
            "hide_overlay" => self.cancel_overlay(workspace),
            "hide_panel" => {
                if args["cancel"].as_bool() == Some(true) {
                    self.cancel_incremental(workspace);
                }
                self.close_panel(workspace);
            }
            "toggle_side_bar" => {
                self.side_visible = !self.side_visible;
                workspace.set_window_toggle("show_side_bar", self.side_visible)?;
                self.side_focused = false;
            }
            "focus_side_bar" => {
                self.panel_focused = false;
                self.focus.show(Surface::Panel, false);
                self.side_visible = true;
                workspace.set_window_toggle("show_side_bar", true)?;
                self.side_focused = true;
                self.side_open = self.open_files_visible.then_some(workspace.active);
                self.rebuild_side(workspace);
            }
            // https://docs.sublimetext.io/reference/commands.html#refresh-folder-list
            "refresh_folder_list" => {
                self.refresh_sidebar(workspace);
                self.intel.rescan();
            }
            "close_folder_list" => self.close_folder_list(workspace)?,
            "toggle_show_open_files" => {
                self.open_files_visible = !self.open_files_visible;
                self.side_open = None;
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
                    "in_selection" => {
                        self.sync_find_scope(workspace);
                        self.scope_manual = true;
                        self.in_selection = !self.in_selection;
                    }
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
                    self.set_query_from_buffer(&value);
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
                self.find_next(
                    workspace,
                    name == "find_prev",
                    args["_close_panel"].as_bool().unwrap_or(false),
                );
            }
            "find_all" => {
                if self.panel == Some(Panel::Files) {
                    return Ok(true);
                }
                self.remember_search(false);
                self.ensure_find(workspace, viewport);
                let close = args["_close_panel"].as_bool().unwrap_or_else(|| {
                    workspace
                        .editor()
                        .settings
                        .bool("close_find_after_find_all", true)
                });
                self.queue_find(workspace, FindNavigation::All { close });
            }
            "replace_next" | "replace_all" => {
                self.replace_document(workspace, name == "replace_all", viewport)?;
                if name == "replace_all"
                    && self.find_error.is_none()
                    && args["_close_panel"].as_bool().unwrap_or_else(|| {
                        workspace
                            .editor()
                            .settings
                            .bool("close_find_after_replace_all", true)
                    })
                {
                    self.close_panel(workspace);
                }
            }
            "find_in_files_start" => {
                self.start_files(workspace)?;
            }
            "set_indentation" => {
                self.set_indentation(workspace, args)?;
            }
            "replace_in_files" => {
                if let Err(error) = self.prepare_files(workspace) {
                    self.set_message(error);
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
    /// Find-family panels show a close button; Escape and a toggling Cmd+F also close them.
    pub(crate) fn find_panel_visible(&self) -> bool {
        matches!(
            self.panel,
            Some(Panel::Find | Panel::Replace | Panel::Incremental | Panel::Files)
        )
    }
    fn document_find_visible(&self) -> bool {
        matches!(
            self.panel,
            Some(Panel::Find | Panel::Replace | Panel::Incremental)
        )
    }
    fn close_button_rect(&self) -> Rect {
        // Centred on the first input row, never taller than it.
        let height = (CLOSE_BUTTON * self.ui_scale).min(self.query.bounds.height);
        Rect {
            x: self.panel_bounds.x + self.panel_bounds.width
                - ((CLOSE_BUTTON + 8.) * self.ui_scale),
            y: self.query.bounds.y + (self.query.bounds.height - height) / 2.,
            width: CLOSE_BUTTON * self.ui_scale,
            height,
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
        if self.document_find_visible() {
            &self.highlights
        } else {
            &[]
        }
    }
    /// Refuse to paint an older snapshot even before the next worker poll.
    pub fn current_highlights(&self, workspace: &Workspace) -> &[Range<usize>] {
        if self
            .search_identity
            .as_ref()
            .is_some_and(|key| key.snapshot == FindSnapshot::current(workspace))
        {
            self.visible_highlights()
        } else {
            &[]
        }
    }
    fn finish_find_navigation(&mut self, workspace: &mut Workspace) {
        if let Some(origin) = self.find_jump_origin.take() {
            let current = crate::intelligence::capture(workspace);
            if !origin.same_position(&current) {
                workspace.jump_history.borrow_mut().remember(origin);
            }
            workspace.jump_history.borrow_mut().reset_current(current);
        }
    }
    fn close_panel(&mut self, workspace: &mut Workspace) {
        self.finish_find_navigation(workspace);
        if self.panel == Some(Panel::Results) && self.file_task.take().is_some() {
            self.file_complete = false;
            self.result_status = format!(
                "Search cancelled · {} matches in {} files",
                self.file_matches,
                self.result_view.matched_files()
            );
            self.set_message(self.result_status.clone());
        }
        self.cancel_pending_find();
        self.in_selection = false;
        self.scope_manual = false;
        self.scope_selection = None;
        self.selection_scope.clear();
        self.scope_revision = None;
        self.incremental_origin = None;
        self.hide_panel_input();
    }
    fn hide_panel_input(&mut self) {
        self.panel = None;
        self.panel_focused = false;
        self.focus.show(Surface::Panel, false);
        self.focus_input = 0;
    }
    fn committed_find_pending(&self) -> bool {
        self.panel.is_none() && self.pending_navigation.iter().any(|action| action.closes())
    }
    fn cancel_pending_find(&mut self) {
        self.pending_navigation.clear();
        if !self.all_matches_ready {
            self.search_worker.cancel();
            self.search_identity = None;
            self.all_matches_ready = true;
        }
    }
    fn scope(&self) -> Vec<Range<usize>> {
        if self.in_selection {
            self.selection_scope.clone()
        } else {
            vec![]
        }
    }
    fn capture_find_scope(&mut self, workspace: &Workspace) {
        let editor = workspace.editor();
        // Hex/binary views have no text offsets for a find scope (lane C round 20, H4).
        self.selection_scope = editor
            .regions()
            .iter()
            .filter(|r| editor.binary.is_none() && !r.empty())
            .map(|r| r.to_range())
            .collect();
        self.scope_revision = Some((editor.buffer.id(), editor.buffer.change_id()));
        self.remember_find_selection(workspace);
    }
    fn remember_find_selection(&mut self, workspace: &Workspace) {
        if self.document_find_visible() || self.committed_find_pending() {
            let tab = &workspace.tabs[workspace.active];
            self.scope_selection = Some((
                tab.sheet,
                tab.editor.buffer.view(tab.editor.id).sel().clone(),
            ));
        }
    }
    fn multiline_scope(&self, workspace: &Workspace) -> bool {
        let text = &workspace.editor().text;
        self.selection_scope
            .iter()
            .any(|r| text.byte_to_line(r.start) != text.byte_to_line(r.end))
    }
    /// External changes to the selection refresh the scope; a search's own
    /// selection is remembered and never becomes the new scope.
    fn sync_find_scope(&mut self, workspace: &Workspace) -> bool {
        let committed = self.committed_find_pending();
        if !self.document_find_visible() && !committed {
            return false;
        }
        let tab = &workspace.tabs[workspace.active];
        let buffer = &tab.editor.buffer;
        let Some((sheet, selection)) = self.scope_selection.as_ref() else {
            self.capture_find_scope(workspace);
            return self.in_selection;
        };
        let active = self.in_selection;
        if *sheet != tab.sheet || self.scope_revision.is_none_or(|(id, _)| id != buffer.id()) {
            self.in_selection = false;
            self.scope_manual = false;
            self.capture_find_scope(workspace);
            self.find_origin = Some((buffer.id(), tab.editor.selection().start));
            return active;
        }
        let (_, revision) = self.scope_revision.expect("scope revision");
        let current = buffer.view(tab.editor.id).sel();
        if revision != buffer.change_id() {
            // Buffer edits already move the live selection. Rebase the original
            // scope separately: the current selection can be a Find match.
            self.selection_scope = self
                .selection_scope
                .iter()
                .filter_map(|r| {
                    // A replacement starting at the scope's first byte belongs
                    // to the scope, unlike edits at a named region's boundary.
                    let start = buffer.transform_point_from(r.start, revision, Bias::Left)?;
                    let end = buffer.transform_point_from(r.end, revision, Bias::Left)?;
                    (start < end).then_some(start..end)
                })
                .collect();
            if let Some((id, at)) = self.find_origin
                && id == buffer.id()
            {
                self.find_origin = buffer
                    .transform_region_from(Region::new(at, at), revision)
                    .map(|r| (id, r.b));
            }
            self.scope_revision = Some((buffer.id(), buffer.change_id()));
            self.remember_find_selection(workspace);
        } else if !committed && !selection.same_snapshot(current) && selection != current {
            self.capture_find_scope(workspace);
            self.find_origin = Some((buffer.id(), tab.editor.selection().start));
        } else {
            return false;
        }
        if !committed && !self.scope_manual && !self.multiline_scope(workspace) {
            self.in_selection = false;
        }
        active
    }
    fn find_identity(&self, workspace: &Workspace) -> FindIdentity {
        FindIdentity {
            snapshot: FindSnapshot::current(workspace),
            query: self.query.value(),
            options: self.options,
            in_selection: self.in_selection,
        }
    }
    fn ensure_find(&mut self, workspace: &Workspace, _viewport: Range<usize>) -> bool {
        let scope_changed = self.sync_find_scope(workspace);
        let key = self.find_identity(workspace);
        if scope_changed || self.search_identity.as_ref() != Some(&key) {
            self.start_find(workspace, key);
            true
        } else {
            false
        }
    }
    fn refresh_find(&mut self, workspace: &Workspace, _viewport: Range<usize>) {
        self.sync_find_scope(workspace);
        self.start_find(workspace, self.find_identity(workspace));
    }
    fn start_find(&mut self, workspace: &Workspace, key: FindIdentity) {
        if self.search_identity.as_ref().is_none_or(|old| {
            old.snapshot.sheet != key.snapshot.sheet
                || old.query != key.query
                || old.options != key.options
                || old.in_selection != key.in_selection
        }) {
            self.pending_navigation.clear();
        } else if let Some(FindNavigation::Preview(origin)) = self.pending_navigation.front_mut() {
            *origin = self
                .find_origin
                .map_or(workspace.editor().caret, |(_, at)| at);
        }
        let snapshot = key.snapshot;
        let result = (!snapshot.binary).then(|| Search::new(&key.query, key.options));
        self.search_identity = Some(key);
        if self.find_error.take().as_ref() == Some(&self.message)
            || (!self.find_status.is_empty() && self.find_status == self.message)
        {
            self.set_message(String::new());
        }
        self.search_worker.cancel();
        self.highlights.clear();
        self.all_matches_ready = false;
        self.find_status.clear();
        match result {
            Some(Ok(search)) => {
                let editor = workspace.editor();
                if search.query.is_empty() || (self.in_selection && self.selection_scope.is_empty())
                {
                    self.all_matches_ready = true;
                } else if snapshot.loading {
                    self.find_status = "Waiting for file to load…".into();
                } else if editor.text.len_bytes() <= 4096 && !search.options.regex {
                    // Keep tiny literal searches immediate. A viewport may include
                    // an arbitrarily long logical line; never scan it as a preview.
                    self.highlights = search.find(&editor.text, &self.scope());
                    self.all_matches_ready = true;
                } else {
                    self.find_status = "Searching…".into();
                    self.search_worker.search(
                        search.clone(),
                        editor.text.clone(),
                        editor.revision,
                        self.scope(),
                    );
                }
                self.search = Some(search);
                if self.all_matches_ready {
                    self.update_find_status();
                }
            }
            Some(Err(error)) => {
                self.find_error = Some(error.clone());
                self.find_status.clone_from(&error);
                self.set_message(error);
                self.search = None;
                self.all_matches_ready = true;
            }
            None => {
                self.pending_navigation.clear();
                self.search = None;
                self.all_matches_ready = true;
                self.find_status = "Text search is unavailable in hex and image tabs.".into();
            }
        }
    }
    fn update_find_status(&mut self) {
        let Some(search) = &self.search else { return };
        if !self.find_status.is_empty() && self.message == self.find_status {
            self.message.clear();
            self.message_seen.clear();
            self.message_until = None;
        }
        self.find_status = if search.query.is_empty() {
            String::new()
        } else if self.highlights.is_empty() {
            let mut flags = Vec::new();
            if search.options.case_sensitive {
                flags.push("Match Case on");
            }
            if search.options.whole_word {
                flags.push("Whole Word on");
            }
            if self.in_selection {
                flags.push(if self.selection_scope.is_empty() {
                    "In Selection on; no selection"
                } else {
                    "In Selection on"
                });
            }
            if flags.is_empty() {
                "No matches".into()
            } else {
                format!("No matches ({})", flags.join(", "))
            }
        } else {
            let n = self.highlights.len();
            format!("{n} {}", if n == 1 { "match" } else { "matches" })
        };
    }
    fn queue_find(&mut self, workspace: &mut Workspace, action: FindNavigation) {
        let _navigation =
            crate::editor::navigation::Guard::command(&workspace.jump_history, "find_next");
        // Macro playback is ordered with subsequent text commands and cannot yield.
        if !self.all_matches_ready
            && workspace.editor().registry.playback_depth() > 1
            && !FindSnapshot::current(workspace).loading
            && let Some(search) = &self.search
        {
            self.highlights = search.find(&workspace.editor().text, &self.scope());
            self.search_worker.cancel();
            self.all_matches_ready = true;
            self.update_find_status();
        }
        self.pending_navigation.push_back(action);
        if !self.all_matches_ready && action.closes() && self.document_find_visible() {
            // Accept is a commit even before the worker returns. Release the
            // input now without discarding the pending command or its scope.
            self.incremental_origin = None;
            self.hide_panel_input();
        }
        self.finish_pending_find(workspace);
    }
    fn finish_pending_find(&mut self, workspace: &mut Workspace) {
        while self.all_matches_ready
            && let Some(action) = self.pending_navigation.pop_front()
        {
            match action {
                FindNavigation::Preview(origin) => self.preview_find(workspace, origin),
                FindNavigation::Next {
                    backwards,
                    close,
                    report,
                } => {
                    self.apply_find_next(workspace, backwards, close, report);
                }
                FindNavigation::All { close } => {
                    if self.search.is_some() {
                        let editor = workspace.editor_mut();
                        text::select_all(&mut editor.buffer, editor.id, &self.highlights);
                        editor.sync();
                        self.remember_find_selection(workspace);
                        self.reveal = !self.highlights.is_empty();
                        self.update_find_status();
                        self.finish_find_navigation(workspace);
                        if close || !self.document_find_visible() {
                            self.set_message(self.find_status.clone());
                        }
                        if close {
                            self.close_panel(workspace);
                        }
                    } else if let Some(error) = self.find_error.clone() {
                        self.set_message(error);
                    }
                }
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
        self.queue_find(
            workspace,
            FindNavigation::Next {
                backwards,
                close,
                report,
            },
        );
    }
    fn apply_find_next(
        &mut self,
        workspace: &mut Workspace,
        backwards: bool,
        close: bool,
        report: bool,
    ) {
        let _navigation = crate::editor::navigation::Guard::command(
            &workspace.jump_history,
            if backwards { "find_prev" } else { "find_next" },
        );
        if report
            && self.search.is_none()
            && let Some(error) = self.find_error.clone()
        {
            self.set_message(error);
        }
        let _preview = self
            .find_jump_origin
            .as_ref()
            .map(|_| crate::editor::navigation::Guard::suppress(&workspace.jump_history));
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
                self.remember_find_selection(workspace);
                self.reveal = true;
                self.update_find_status();
            } else if report && !search.query.is_empty() {
                self.update_find_status();
                if !self.highlights.is_empty() {
                    let direction = if backwards { "previous" } else { "later" };
                    self.find_status = format!("No {direction} match ({})", self.find_status);
                }
                self.set_message(self.find_status.clone());
            }
        }
        self.finish_find_navigation(workspace);
        if close {
            self.close_panel(workspace);
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
        self.ensure_find(workspace, viewport.clone());
        if self.in_selection && self.selection_scope.is_empty() {
            return Ok(());
        }
        let Some(search) = &self.search else {
            if let Some(error) = self.find_error.clone() {
                self.set_message(error);
            }
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
        if self.overlay == Some(Overlay::Appearance) {
            self.query_appearance(workspace, true);
            return;
        }
        let _navigation = crate::editor::navigation::Guard::suppress(&workspace.jump_history);
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
            Some(Overlay::Appearance) => unreachable!("handled above"),
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
            self.extra_paths.clear();
            self.extra_candidates = Arc::default();
            self.file_recency.clear();
            self.local_positions.clear();
            self.quick.set_items(Vec::new());
        } else if self.quick_query != value {
            // A new query always starts at its best hit. Source-only refreshes retain selection.
            self.quick.set_items(Vec::new());
        }
        self.quick_query = value;
        if workspace.editor().binary.is_some() && matches!(target, "line" | "symbol" | "word") {
            self.fuzzy_worker.cancel();
            self.quick_pending = false;
            self.set_message("Text navigation is unavailable in hex and image tabs.".into());
            return;
        }
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
            self.project_symbol_source = self.intel.symbol_source();
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
            // Display only: use current or mapped publications, never synchronously parse.
            let has_snippets = self.overlay != Some(Overlay::Syntax)
                && palette
                    .iter()
                    .any(|item| item.invocation.command == "insert_snippet");
            let editor = workspace.editor();
            let snippet_scopes = has_snippets
                .then(|| editor.published_scopes(editor.caret, false))
                .flatten();
            self.palette_scope_request = (has_snippets
                && self.overlay == Some(Overlay::Palette)
                && !editor
                    .language
                    .borrow()
                    .as_ref()
                    .is_some_and(|l| l.scopes_ready_at(&editor.text, editor.caret)))
            .then_some((
                editor.buffer.id(),
                editor.buffer.change_id(),
                editor.caret,
                self.quick_revision,
            ));
            self.palette_indices = palette
                .iter()
                .enumerate()
                .filter(|(i, p)| {
                    if self.overlay == Some(Overlay::Syntax)
                        && p.invocation.command != "set_file_type"
                    {
                        return false;
                    }
                    if p.invocation.command == "insert_snippet"
                        && let Some(snippet) = p.invocation.args["name"]
                            .as_str()
                            .and_then(|name| workspace.editor().snippets.get(name))
                        && !snippet.scope().is_empty()
                        && snippet_scopes.as_ref().is_some_and(|stack| {
                            !vel_scope::Selector::parse(snippet.scope())
                                .is_ok_and(|selector| selector.matches(stack.as_slice()))
                        })
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
                    self.message = if analysis.is_none() {
                        "Indexing symbols…".into()
                    } else {
                        String::new()
                    };
                    if let Some(analysis) = analysis {
                        // Build 4107 displays symbol kinds: https://www.sublimetext.com/download
                        // Index metadata supplies source order; keep that for an empty query.
                        let editor = workspace.editor();
                        let mut kinds = Vec::new();
                        let symbols = analysis
                            .index_symbols
                            .symbols
                            .iter()
                            .filter(|s| s.local)
                            .map(|symbol| {
                                use vel_highlight::ScopeProvider;
                                let scopes = language
                                    .as_ref()
                                    .and_then(|l| l.scopes.as_ref())
                                    .and_then(|s| s.scopes_at(symbol.range.start))
                                    .unwrap_or_default();
                                kinds.push(vel_index::SymbolKind::from_scopes(scopes));
                                (
                                    symbol.name.clone(),
                                    symbol.range.start.min(editor.text.len_bytes()),
                                )
                            })
                            .collect();
                        self.set_local(symbols, term);
                        self.local_kinds = kinds;
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
        self.paths = self.file_paths.clone();
        self.local_positions.clear();
        let indexed = self.file_paths.len();
        self.candidates = self.file_candidates.clone();
        self.extra_paths = extra;
        self.extra_candidates = Arc::new(
            self.extra_paths
                .iter()
                .map(|path| fuzzy::Candidate::new(path.to_string_lossy()))
                .collect(),
        );
        self.file_recency = workspace
            .goto_recent_files
            .iter()
            .rev()
            .enumerate()
            .filter_map(|(n, path)| {
                self.paths
                    .binary_search(path)
                    .ok()
                    .or_else(|| {
                        self.extra_paths
                            .iter()
                            .position(|p| p == path)
                            .map(|i| indexed + i)
                    })
                    .map(|i| (i, 200 - (workspace.goto_recent_files.len() - n - 1) as u64))
            })
            .collect();
        self.fuzzy_worker.set_items_with_extras(
            self.candidates.clone(),
            self.extra_candidates.clone(),
            self.file_recency.clone(),
        );
        self.fuzzy_worker.query(query.file, 100);
    }
    fn quick_path(&self, index: usize) -> Option<&PathBuf> {
        self.paths
            .get(index)
            .or_else(|| self.extra_paths.get(index.saturating_sub(self.paths.len())))
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
                    caption: self
                        .candidates
                        .get(hit.index)
                        .unwrap_or_else(|| {
                            &self.extra_candidates[hit.index - self.candidates.len()]
                        })
                        .label
                        .clone(),
                    highlights: hit.positions,
                    value: index,
                    kind: if let Some(kind) = self
                        .local_kinds
                        .get(hit.index)
                        .filter(|_| !self.local_positions.is_empty())
                    {
                        format!("{kind:?}")
                    } else if palette {
                        "command".into()
                    } else if !self.local_positions.is_empty() {
                        "word".into()
                    } else {
                        "file".into()
                    },
                    annotation: if palette {
                        workspace
                            .editor()
                            .keymap
                            .hint(&palette_items[index].invocation)
                            .unwrap_or_default()
                    } else if let Some(at) = self.local_positions.get(hit.index) {
                        format!("Line {}", workspace.editor().text.byte_to_line(*at) + 1)
                    } else {
                        String::new()
                    },
                    details: if palette {
                        palette_items[index].invocation.command.clone()
                    } else {
                        self.quick_path(hit.index)
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
        self.local_kinds.clear();
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
        if self.overlay == Some(Overlay::Appearance) {
            self.preview_appearance(workspace);
            return Ok(());
        }
        let _navigation = crate::editor::navigation::Guard::suppress(&workspace.jump_history);
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
                crate::intelligence::jump_with_options(
                    workspace,
                    &location,
                    true,
                    self.navigation_options,
                )?;
                self.cleanup_navigation_previews(workspace, true);
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
        let Some(path) = self.quick_path(index).cloned() else {
            return Ok(());
        };
        let target = query.target;
        if self.last_preview.as_ref() == Some(&(path.clone(), target.clone())) {
            if let Target::Symbol(term) = &target {
                self.preview_file_symbol(workspace, term);
            }
            return Ok(());
        }
        self.last_preview = Some((path.clone(), target.clone()));
        self.preview_symbol_version = None;
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
            self.preview_file_symbol(workspace, &term);
        }
        Ok(())
    }
    fn preview_file_symbol(&mut self, workspace: &mut Workspace, term: &str) {
        let tab = &workspace.tabs[workspace.active];
        let version = (tab.sheet, tab.editor.buffer.change_id());
        if self.preview_symbol_version == Some(version)
            || !self
                .last_preview
                .as_ref()
                .is_some_and(|(p, _)| tab.path.as_ref().is_some_and(|path| path == p))
        {
            return;
        }
        let symbols = tab
            .editor
            .language
            .borrow()
            .as_ref()
            .and_then(|l| l.analysis.as_ref())
            .filter(|analysis| analysis.version == version.1)
            .map(|a| a.symbols.clone());
        if let Some(symbols) = symbols {
            self.preview_symbol_version = Some(version);
            let items: Vec<_> = symbols
                .iter()
                .map(|(s, _)| fuzzy::Candidate::new(s))
                .collect();
            if let Some(hit) = fuzzy::Matcher::default().rank(&items, term, 1).first() {
                workspace
                    .editor_mut()
                    .set_caret(symbols[hit.index].1, false);
            }
        }
    }
    fn cleanup_navigation_previews(&self, workspace: &mut Workspace, keep_active: bool) {
        let Some(preview) = &self.preview else {
            return;
        };
        for index in (0..self.visible_open_files(workspace)).rev() {
            let tab = &workspace.tabs[index];
            if (!keep_active || index != workspace.active)
                && tab.transient
                && !tab.dirty()
                && !preview.sheets.contains(&tab.sheet)
            {
                let _ = workspace.close(index);
            }
        }
    }
    pub(crate) fn cancel_overlay(&mut self, workspace: &mut Workspace) {
        self.intel.cancel_query();
        if self.appearance_selection.take().is_some() {
            workspace.cancel_appearance_preview();
            self.overlay = None;
            self.focus.show(Surface::Overlay, false);
            self.fuzzy_worker.cancel();
            return;
        }
        let _navigation = crate::editor::navigation::Guard::suppress(&workspace.jump_history);
        self.palette_accept = None;
        if self.overlay.take().is_none() {
            self.navigation_origin = None;
            self.navigation_selection = None;
            self.navigation_options = Default::default();
            self.navigation_preview_guard = None;
            return;
        }
        self.cleanup_navigation_previews(workspace, false);
        if let Some(preview) = self.preview.take()
            && let Some(index) = workspace.tabs.iter().position(|t| t.sheet == preview.sheet)
        {
            crate::intelligence::restore(
                workspace,
                &crate::intelligence::Jump {
                    sheet: preview.sheet,
                    buffer: preview.buffer,
                    version: preview.version,
                    caret_count: preview.selection.len(),
                    selection: preview.selection,
                    scroll: preview.scroll,
                    path: preview.path,
                    position: preview.position,
                },
            );
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
        if let Some((group, selected)) = self.navigation_selection.take()
            && let Some(group) = workspace.workbench.groups.get_mut(group)
        {
            group.selected = selected
                .into_iter()
                .filter(|sheet| group.sheets.contains(sheet))
                .collect();
            if let Some(focused) = group.focused {
                group.selected.insert(focused);
            }
        }
        workspace
            .jump_history
            .borrow_mut()
            .reset_current(crate::intelligence::capture(workspace));
        self.navigation_preview_guard = None;
        self.navigation_options = Default::default();
    }
    fn accept(
        &mut self,
        workspace: &mut Workspace,
        side: bool,
        viewport: Range<usize>,
    ) -> Result<Option<Invocation>, String> {
        if self.overlay == Some(Overlay::Appearance) {
            self.accept_appearance(workspace)?;
            return Ok(None);
        }
        if self.overlay == Some(Overlay::Goto) {
            let query = Query::parse(&self.quick_input.value());
            if workspace.editor().binary.is_some()
                && query.file.is_empty()
                && !matches!(query.target, Target::File)
            {
                return Ok(None);
            }
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
            let hits = fuzzy::Matcher::default().rank_with_extras(
                &self.candidates,
                &self.extra_candidates,
                &self.file_recency,
                term,
                100,
            );
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
                Some(Overlay::Goto | Overlay::ProjectSymbols | Overlay::Locations)
            ) {
                if let Some(origin) = self.navigation_origin.take() {
                    workspace.jump_history.borrow_mut().remember(origin);
                } else if let Some(source) = &self.preview {
                    workspace
                        .jump_history
                        .borrow_mut()
                        .remember(crate::intelligence::Jump {
                            sheet: source.sheet,
                            buffer: source.buffer,
                            version: source.version,
                            caret_count: source.selection.len(),
                            selection: source.selection.clone(),
                            scroll: source.scroll,
                            path: source.path.clone(),
                            position: source.position,
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
            workspace
                .jump_history
                .borrow_mut()
                .reset_current(crate::intelligence::capture(workspace));
            self.navigation_selection = None;
            self.navigation_options = Default::default();
            workspace.remember_goto_file();
            self.overlay = None;
            self.preview = None;
            self.navigation_preview_guard = None;
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
                self.close_panel(workspace);
                return Ok(invocation);
            }
            _ => {
                self.action(
                    workspace,
                    "find_next",
                    &json!({"_close_panel": self.panel == Some(Panel::Incremental)}),
                    viewport,
                )?;
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
        if self.overlay == Some(Overlay::Appearance) && key.name == "tab" && key.modifiers & 1 != 0
        {
            self.cancel_overlay(workspace);
            return Ok((false, None));
        }
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
                self.close_panel(workspace);
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
                    "down" if index + 1 < self.visible_open_files(workspace) => {
                        self.side_open = Some(index + 1)
                    }
                    "down" => {
                        self.side_open = None;
                        self.select_side(0);
                    }
                    "enter" | "right" => {
                        workspace.select(
                            index.min(self.visible_open_files(workspace).saturating_sub(1)),
                        );
                        workspace.make_permanent(workspace.active);
                    }
                    _ if key.modifiers & 9 != 0 => return Ok((false, None)),
                    _ => {}
                }
                self.side_scroll.reveal(
                    self.side_open
                        .map_or(self.folder_row_base(workspace), |i| i + 1),
                );
                return Ok((true, None));
            }
            match key.name.as_str() {
                "up" if self.side_selected == 0 && self.visible_open_files(workspace) > 0 => {
                    self.side_open = Some(self.visible_open_files(workspace) - 1)
                }
                "up" => self.select_side(self.side_selected.saturating_sub(1)),
                "down" => {
                    let next = if self.side_selected >= self.side_rows.len() {
                        0
                    } else {
                        (self.side_selected + 1).min(self.side_rows.len() - 1)
                    };
                    self.select_side(next);
                }
                "enter" => self.activate_side(workspace, true)?,
                // https://www.sublimetext.com/download (4050): Alt+Arrow applies recursively.
                "right" if key.modifiers == 2 => {
                    if let Some(path) = self
                        .side_rows
                        .get(self.side_selected)
                        .filter(|row| row.directory)
                        .map(|row| row.path.clone())
                    {
                        self.expand_recursively(workspace, path);
                    }
                }
                "left" if key.modifiers == 2 => {
                    if let Some(row) = self.side_rows.get(self.side_selected) {
                        let path = row.path.clone();
                        self.expanded.retain(|p| !p.starts_with(&path));
                        self.cancel_recursive(&path);
                        self.directory_pending.retain(|p| !p.starts_with(&path));
                        self.watched_expanded = false;
                        self.rebuild_side(workspace);
                    }
                }
                "right" => {
                    if self
                        .side_rows
                        .get(self.side_selected)
                        .is_some_and(|r| !self.expanded.contains(&r.path))
                    {
                        self.activate_side(workspace, true)?;
                    } else {
                        self.select_side(
                            (self.side_selected + 1).min(self.side_rows.len().saturating_sub(1)),
                        );
                    }
                }
                "left" => {
                    if let Some(path) = self
                        .side_rows
                        .get(self.side_selected)
                        .map(|r| r.path.clone())
                    {
                        self.cancel_recursive(&path);
                        self.watched_expanded = false;
                        if !self.expanded.remove(&path)
                            && let Some(parent) = path.parent()
                            && let Some(index) =
                                self.side_rows.iter().position(|r| r.path == parent)
                        {
                            self.select_side(index);
                        }
                        self.rebuild_side(workspace);
                    }
                }
                _ if key.modifiers & 9 != 0 => return Ok((false, None)),
                _ => return Ok((true, None)),
            }
            self.side_scroll.reveal(
                self.side_open
                    .map_or(self.folder_row_base(workspace) + self.side_selected, |i| {
                        i + 1
                    }),
            );
            return Ok((true, None));
        }
        if self.overlay.is_none() && key.name == "enter" && key.modifiers & 3 == 3 {
            return Ok((false, None));
        }
        // https://forum.sublimetext.com/t/605: Ctrl+Enter inserts a literal newline.
        if key.name == "enter"
            && key.modifiers == 1
            && let Some(input) = self.input_mut().filter(|input| input.multiline())
        {
            input.run("insert", &json!({"characters":"\n"}))?;
            self.input_changed(workspace, viewport);
            return Ok((true, None));
        }
        if self.overlay.is_none() && key.name == "enter" && key.modifiers == 4 {
            if matches!(
                self.panel,
                Some(Panel::Find | Panel::Replace | Panel::Incremental)
            ) {
                // https://www.sublimetext.com/download (Shift+Enter closes incremental find).
                self.action(
                    workspace,
                    "find_prev",
                    &json!({"_close_panel": self.panel == Some(Panel::Incremental)}),
                    viewport,
                )?;
            }
            return Ok((true, None));
        }
        if self.overlay.is_none() && key.name == "enter" && key.modifiers & 2 != 0 {
            if matches!(
                self.panel,
                Some(Panel::Find | Panel::Replace | Panel::Incremental)
            ) {
                self.action(
                    workspace,
                    "find_all",
                    &json!({"_close_panel":true}),
                    viewport,
                )?;
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
            let forward = key.name == "down";
            if let Some(input) = self
                .input_mut()
                .filter(|input| input.can_move_vertically(forward))
            {
                input.command(if key.name == "up" {
                    Command::Up(false)
                } else {
                    Command::Down(false)
                });
                input.reveal();
            } else {
                self.walk_history(workspace, key.name == "up", viewport);
            }
            return Ok((true, None));
        }
        if key.name == "enter" {
            return Ok((
                true,
                self.accept(workspace, key.modifiers & 9 != 0, viewport)?,
            ));
        }
        if key.name == "tab" && matches!(key.modifiers, 0 | 4) {
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
                "up" => {
                    self.result_navigated |= !self.result_view.rows.is_empty();
                    self.result_selected = self.result_selected.saturating_sub(1);
                }
                "down" => {
                    self.result_navigated |= !self.result_view.rows.is_empty();
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
            Command::LiteralNewline if self.input().is_some_and(Input::multiline) => {
                Some("ctrl+enter")
            }
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
        let decode = vel_fileio::DecodeOptions::from_settings(&workspace.settings.editor());
        let task = files::Task::start_with_decode(
            files::Request {
                search,
                scope,
                folders: workspace.folders.clone(),
                open,
                context: 1,
            },
            decode,
        );
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
        self.result_view = files::Results::ordered(&workspace.folders);
        self.result_selected = 0;
        self.result_navigated = false;
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
                files::Row::Line(line) if line.match_count > 0 => Some(line.path.clone()),
                _ => None,
            })
            .collect();
        for path in &paths {
            if workspace
                .tabs
                .iter()
                .any(|t| t.path.as_deref() == Some(path.as_ref()) && t.dirty())
            {
                return Err(format!("Save {} before replacing in files", path.display()));
            }
        }
        let replacement = self.replace.value();
        let decode = vel_fileio::DecodeOptions::from_settings(&workspace.settings.editor());
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
                .map(|path| {
                    files::FileChange::prepare_with_decode(
                        path.to_path_buf(),
                        &search,
                        &replacement,
                        &decode,
                    )
                })
                .collect::<Result<Vec<_>, _>>()
                .map(|changes| changes.into_iter().filter(|c| c.count > 0).collect());
            let _ = tx.send(result);
        });
        self.replace_rx = Some(rx);
        self.message = "Preparing replacement preview…".into();
        Ok(())
    }
    pub fn jump_result(&mut self, workspace: &mut Workspace) -> Result<(), String> {
        workspace.observe_navigation();
        let _navigation =
            crate::editor::navigation::Guard::command(&workspace.jump_history, "find_result");
        if let Some(files::Row::Line(line)) = self.result_view.rows.get(self.result_selected) {
            self.result_cursor = Some(self.result_selected);
            self.reveal = true;
            if let Some(index) = workspace.tabs.iter().enumerate().position(|(i, t)| {
                t.path.as_deref() == Some(line.path.as_ref())
                    || (t.path.is_none()
                        && *line.path == *Path::new(&format!("Untitled {}", i + 1)))
            }) {
                workspace.select(index);
                workspace.tabs[index].goto(&FileLocation {
                    path: line.path.to_path_buf(),
                    line: line.number,
                    column: line.column(),
                });
            } else {
                workspace.open(FileLocation {
                    path: line.path.to_path_buf(),
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
            self.watched_expanded = false;
            if !self.expanded.remove(&row.path) {
                self.expanded.insert(row.path.clone());
                self.load_directory(workspace, &row.path);
            } else {
                self.cancel_recursive(&row.path);
                self.directory_pending.retain(|p| !p.starts_with(&row.path));
            }
            self.rebuild_side(workspace);
        } else {
            let location = FileLocation {
                path: row.path,
                line: 1,
                column: 1,
            };
            if permanent {
                let spelling = location.path.clone();
                workspace.open(location)?;
                self.note_sidebar_spelling(workspace, &spelling);
            } else {
                workspace.preview(location)?;
            }
        }
        Ok(())
    }
    fn expand_recursively(&mut self, workspace: &Workspace, path: PathBuf) {
        let Some(folder) = workspace
            .folders
            .iter()
            .filter(|f| path.starts_with(&f.path))
            .max_by_key(|f| f.path.as_os_str().len())
            .cloned()
        else {
            return;
        };
        let roots: Vec<_> = workspace.folders.iter().map(|f| f.path.clone()).collect();
        let (tx, result) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        self.recursive_expansion = Some(RecursiveExpansion {
            path: path.clone(),
            stop: stop.clone(),
            result,
        });
        std::thread::spawn(move || {
            let subtree = folder.sidebar_subtree(&path, &roots, 5000, &stop);
            if !stop.load(Ordering::Acquire) {
                let _ = tx.send(subtree);
            }
        });
    }

    fn cancel_recursive(&mut self, path: &Path) {
        if self
            .recursive_expansion
            .as_ref()
            .is_some_and(|r| r.path.starts_with(path) || path.starts_with(&r.path))
        {
            self.recursive_expansion = None;
        }
    }
    pub fn poll(&mut self, workspace: &mut Workspace, viewport: Range<usize>) -> bool {
        let load_notice = !workspace.load_notices.is_empty();
        if load_notice {
            self.set_message(std::mem::take(&mut workspace.load_notices).join("; "));
        }
        let folder_edit_changed = self.poll_folder_edits(workspace);
        if let Some(message) = workspace.appearance_notice.take() {
            self.set_message(message);
        }
        if self.appearance_selection.is_some()
            && (self.overlay != Some(Overlay::Appearance) || self.appearance_stale(workspace))
        {
            self.cancel_overlay(workspace);
        }
        if self.overlay.is_none() {
            workspace.observe_navigation();
        }
        if self.vintage_prompt.is_some()
            && let Err(error) = self.sync_vintage_prompt(workspace, viewport.clone())
        {
            self.message = error;
        }
        let intelligence_changed = folder_edit_changed
            | self.intel.poll(workspace)
            | self.review_poll(workspace)
            | self.merge_git_tree(workspace);
        if self.overlay == Some(Overlay::ProjectSymbols)
            && !self.intel.pending
            && self.project_symbol_source != self.intel.symbol_source()
        {
            self.query_quick(workspace);
        }
        if self.overlay == Some(Overlay::Goto)
            && let Some((_, Target::Symbol(term))) = &self.last_preview
            && !self.quick_pending
        {
            let term = term.clone();
            self.preview_file_symbol(workspace, &term);
        }
        if self
            .hover_source
            .is_some_and(|(sheet, buffer, version, _)| {
                let editor = workspace.editor();
                sheet != workspace.tabs[workspace.active].sheet
                    || buffer != editor.buffer.id()
                    || version != editor.buffer.change_id()
                    || self.hover_caret != editor.caret
                    || !editor.settings.bool("show_definitions", true)
            })
        {
            self.hover.clear();
            self.hover_html = None;
            self.hover_term.clear();
            self.hover_source = None;
        }
        if !self.hover_term.is_empty()
            && !self.hover_requested
            && self.hover_since.elapsed() >= Duration::from_secs(2)
        {
            self.hover_term.clear();
            self.hover_retry = None;
        }
        if !self.hover_requested
            && !self.hover_term.is_empty()
            && self.hover_since.elapsed() >= Duration::from_millis(250)
            && self.overlay.is_none()
            && !self.intel.pending
        {
            let eligible = self
                .hover_source
                .and_then(|source| Self::hover_eligible(workspace.editor(), source.3));
            match eligible {
                Some(true) => {
                    self.intel.query_at(
                        crate::intelligence::Query::Hover(self.hover_term.clone()),
                        workspace,
                        self.hover_source
                            .map_or(workspace.editor().caret, |source| source.3),
                    );
                    self.hover_requested = true;
                    self.hover_retry = None;
                }
                Some(false) => {
                    self.hover_term.clear();
                    self.hover_retry = None;
                }
                None if self.hover_retry.is_none_or(|at| Instant::now() >= at) => {
                    self.hover_retry = Some(
                        (Instant::now() + Duration::from_millis(100))
                            .min(self.hover_since + Duration::from_secs(2)),
                    );
                }
                None => {}
            }
        }
        let mut query_changed = false;
        if let Some(result) = self.intel.result() {
            query_changed = true;
            use crate::intelligence::Query;
            let editor = workspace.editor();
            let current = result.source
                == (
                    workspace.tabs[workspace.active].sheet,
                    editor.buffer.id(),
                    editor.buffer.change_id(),
                    editor.caret,
                );
            match result.query {
                Query::Hover(term)
                    if self.hover_source == Some(result.source) && term == self.hover_term =>
                {
                    let pointer = result.source.3;
                    let path = workspace.tabs[workspace.active].path.as_deref();
                    let other = |location: &vel_index::SymbolLocation| {
                        path != Some(location.file.as_path()) || !location.range.contains(&pointer)
                    };
                    self.hover = result.locations.into_iter().filter(other).collect();
                    self.hover_definitions = self.hover.len();
                    self.hover
                        .extend(result.references.into_iter().filter(other));
                }
                Query::Symbols(term)
                    if self.overlay == Some(Overlay::ProjectSymbols)
                        && term == self.quick_input.value() =>
                {
                    self.locations = result.locations;
                    self.quick_pending = false;
                    self.location_items(workspace);
                    let _ = self.preview_current(workspace);
                }
                Query::Definition(_) | Query::Reference(_) if current => {
                    if result.locations.len() == 1 {
                        if let Some(origin) = self.navigation_origin.take() {
                            workspace.jump_history.borrow_mut().remember(origin);
                        }
                        let _ = crate::intelligence::jump_with_options(
                            workspace,
                            &result.locations[0],
                            false,
                            self.navigation_options,
                        );
                        workspace
                            .jump_history
                            .borrow_mut()
                            .reset_current(crate::intelligence::capture(workspace));
                        self.navigation_selection = None;
                        self.navigation_options = Default::default();
                    } else if result.locations.is_empty() {
                        self.message = "No indexed locations".into();
                        self.navigation_origin = None;
                        self.navigation_selection = None;
                        self.navigation_options = Default::default();
                    } else {
                        let origin = self.navigation_origin.take();
                        let options = self.navigation_options;
                        let selection = self.navigation_selection.take();
                        let _ =
                            self.action(workspace, "show_overlay", &json!({}), viewport.clone());
                        self.navigation_origin = origin;
                        self.navigation_options = options;
                        self.navigation_selection = selection;
                        self.overlay = Some(Overlay::Locations);
                        self.fuzzy_worker.cancel();
                        self.locations = result.locations;
                        self.quick_pending = false;
                        self.location_items(workspace);
                        let _ = self.preview_current(workspace);
                    }
                }
                _ => {}
            }
        }
        if let Some((buffer, version, caret, query)) = self.palette_scope_request {
            let editor = workspace.editor();
            if self.overlay != Some(Overlay::Palette)
                || query != self.quick_input.buffer.change_id().0
                || (buffer, version, caret)
                    != (editor.buffer.id(), editor.buffer.change_id(), editor.caret)
            {
                self.palette_scope_request = None;
            } else if editor
                .language
                .borrow()
                .as_ref()
                .is_some_and(|l| l.scopes_ready_at(&editor.text, caret))
            {
                self.palette_scope_request = None;
                self.prepared.clear();
            }
        }
        if self.overlay == Some(Overlay::Palette) && self.prepared.is_empty() {
            self.query_quick(workspace);
        }
        if self.overlay == Some(Overlay::Goto)
            && workspace.editor().binary.is_none()
            && matches!(
                Query::parse(&self.quick_input.value()).target,
                Target::Symbol(_)
            )
        {
            self.query_quick(workspace);
        }
        let mut changed = load_notice || intelligence_changed || query_changed;
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
        if self.folder_key != workspace.folders {
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
        if let Some(index) = &self.index {
            if !self.watched_expanded {
                index.watch_directories(Arc::new(self.expanded.clone()));
                self.watched_expanded = true;
            }
            let listings = index.directory_changes();
            if listings.all {
                self.directory_pending.extend(self.expanded.iter().cloned());
            } else {
                self.directory_pending.extend(
                    listings
                        .paths
                        .into_iter()
                        .filter(|p| self.expanded.contains(p)),
                );
            }
        }
        let directories: Vec<_> = self.directories.try_iter().collect();
        let mut rows_changed = false;
        for (generation, path, children) in directories {
            self.directory_running -= 1;
            if generation != self.directory_generation {
                continue;
            }
            self.loading.remove(&path);
            if !self.expanded.contains(&path) {
                continue;
            }
            match children {
                Ok(children) => {
                    if self.children.get(&path) == Some(&children) {
                        continue;
                    }
                    self.git_tree_dirty = true;
                    self.git_virtual
                        .retain(|p| p.parent() != Some(path.as_path()));
                    self.children.insert(path.clone(), children);
                    rows_changed = true;
                }
                Err(error) => {
                    rows_changed |= self.children.remove(&path).is_some();
                    // Missing children disappear through their parent's listing.
                    if workspace.folders.iter().any(|folder| folder.path == path) {
                        self.message = error;
                        changed = true;
                    }
                }
            }
        }
        if let Some(result) = self
            .recursive_expansion
            .as_ref()
            .map(|r| r.result.try_recv())
        {
            match result {
                Ok(subtree) => {
                    self.recursive_expansion = None;
                    if !subtree.listings.is_empty() {
                        self.expanded.extend(subtree.listings.keys().cloned());
                        self.children.extend(subtree.listings);
                        self.watched_expanded = false;
                        self.git_tree_dirty = true;
                        rows_changed = true;
                    }
                    if subtree.limited {
                        self.set_message("Recursive expansion stopped at 5,000 directories".into());
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => self.recursive_expansion = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        self.pump_directories(workspace);
        if rows_changed {
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
        if self.panel.is_none()
            && !self.pending_navigation.is_empty()
            && self
                .search_identity
                .as_ref()
                .is_some_and(|key| key.snapshot.sheet != workspace.tabs[workspace.active].sheet)
        {
            self.find_jump_origin = None;
            self.close_panel(workspace);
        }
        if self.overlay.is_none()
            && (self.document_find_visible() || !self.pending_navigation.is_empty())
        {
            changed |= self.ensure_find(workspace, viewport);
            if let Some(matches) = self.search_worker.poll()
                && self.search_identity.as_ref().is_some_and(|key| {
                    key.snapshot == FindSnapshot::current(workspace)
                        && key.snapshot.revision == matches.revision
                })
            {
                self.highlights = matches.ranges;
                self.all_matches_ready = true;
                self.update_find_status();
                changed = true;
            }
            self.finish_pending_find(workspace);
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
        let deadline = self
            .file_task
            .as_ref()
            .map(|_| Instant::now() + Duration::from_millis(1));
        for index in 0..256 {
            if index != 0 && deadline.is_some_and(|end| Instant::now() >= end) {
                break;
            }
            let Some(event) = self
                .file_task
                .as_ref()
                .and_then(|task| task.events.try_recv().ok())
            else {
                break;
            };
            match event {
                files::Event::Line(line) => {
                    self.file_matches += line.match_count;
                    let inserted = self.result_view.push(line);
                    if self.result_navigated || self.result_cursor.is_some() {
                        let at = self.result_view.rank(inserted);
                        if self.result_navigated && at <= self.result_selected {
                            self.result_selected += inserted.added;
                        }
                        if let Some(cursor) = self.result_cursor.as_mut().filter(|c| at <= **c) {
                            *cursor += inserted.added;
                        }
                    }
                }
                files::Event::Error(error) => self.message = error,
                files::Event::Done { files, .. } => {
                    self.files_searched = files;
                    self.result_status =
                        vel_ui::status::search(self.file_matches, self.result_view.matched_files());
                    self.set_message(self.result_status.clone());
                    self.file_complete = true;
                    self.file_task = None;
                }
            }
            changed = true;
        }
        self.result_scroll.rows = if self.panel == Some(Panel::Output) {
            // The output view's layout supplies the rows when it exists; counting megabytes
            // of output lines on every frame would not.
            match &self.output_editor {
                Some(editor) => editor.layout.borrow().total(),
                None => self.output.lines().count(),
            }
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
        // A hidden status bar (distraction free, toggle_status_bar) leaves no unpainted strip.
        let status = if self.chrome {
            STATUS * self.ui_scale
        } else {
            0.
        };
        let bottom = match self.panel {
            None => 0.,
            // Find panels take their natural height (input rows plus action rows, which
            // depend on the width). A short window shrinks the panel and its controls rather
            // than drawing them outside it; the editor keeps at least its tab row.
            Some(
                panel @ (Panel::Find
                | Panel::Incremental
                | Panel::Replace
                | Panel::Files
                | Panel::Input),
            ) => (self.find_panel_natural_height(panel, (size.width - side).max(1.))
                * self.ui_scale)
                .min((size.height - status - 32. * self.ui_scale).max(0.)),
            _ => (size.height * 0.38)
                .clamp(60. * self.ui_scale, 320. * self.ui_scale)
                .min((size.height - (80. * self.ui_scale)).max(0.)),
        };
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
            height: (size.height - (30. * self.ui_scale) - status).max(1.),
        };
        self.side_scroll.set_viewport(self.side_bounds.height);
        let width = (size.width * 0.7)
            .clamp(240. * self.ui_scale, 820. * self.ui_scale)
            .min(size.width);
        // The overlay shrinks to whole rows (input 46 + footer 26 + one row height each), so
        // the list ends exactly where the footer starts. Short windows move it up, then
        // shrink it, so it never extends below the window.
        let row = self.quick_row_height();
        // Windows too short for the input, one row and the footer drop the footer hint and
        // tighten the input, so at least one whole row stays visible.
        self.quick_tight = size.height < (72. + row) * self.ui_scale;
        let (header, footer) = self.quick_chrome();
        let limit = (size.height * 0.65).min(520. * self.ui_scale);
        let fits = ((limit / self.ui_scale - header - footer) / row)
            .floor()
            .max(1.);
        let rows = (self.quick.items.len() as f64).min(fits);
        let height = (header + footer + row * rows) * self.ui_scale;
        let y = (54. * self.ui_scale).min((size.height - height).max(0.));
        self.quick_bounds = Rect {
            x: (size.width - width) / 2.,
            y,
            width,
            height: height.min(size.height - y).max(0.),
        };
        let inset = if self.quick_tight { 6. } else { 8. };
        self.quick_input.bounds = Rect {
            x: self.quick_bounds.x + (8. * self.ui_scale),
            y: self.quick_bounds.y + (inset * self.ui_scale),
            width: width - (16. * self.ui_scale),
            height: (header - inset - 6.) * self.ui_scale,
        };
        self.quick.scroll.row_height = row * self.ui_scale;
        self.quick.scroll.rows = self.quick.items.len();
        self.quick
            .scroll
            .set_viewport((self.quick_bounds.height - ((header + footer) * self.ui_scale)).max(1.));
        let close = if self.find_panel_visible() {
            CLOSE_BUTTON + 6.
        } else {
            0.
        };
        let v = self.find_vertical_scale();
        // Narrow panels give the input room first: the label column shrinks, then the labels
        // are dropped, before the input would collapse under the close button.
        let label = Self::find_label_column(self.panel_bounds.width, self.ui_scale, close);
        let extras = self
            .panel
            .map_or([0.; 3], |panel| self.find_input_extras(panel));
        let fields = self.panel.map_or(0, Self::find_fields);
        let mut y = 8.;
        for (i, input) in [&mut self.query, &mut self.replace, &mut self.where_input]
            .into_iter()
            .enumerate()
        {
            if i >= fields {
                input.bounds = Rect::default();
                continue;
            }
            input.bounds = Rect {
                x: side + label * self.ui_scale,
                y: self.panel_bounds.y + y * v,
                width: (self.panel_bounds.width - ((label + 12. + close) * self.ui_scale)).max(1.),
                height: (28. + extras[i]) * v,
            };
            input.set_row_pitch(18. * v);
            y += PANEL_ROW + extras[i];
            input.reveal();
        }
        self.quick_input.reveal();
        self.result_scroll.row_height =
            if self.panel == Some(Panel::Output) && self.output_editor.is_some() {
                self.output_view.line_height
            } else {
                24. * self.ui_scale
            };
        self.result_scroll
            .set_viewport((bottom - (34. * self.ui_scale)).max(1.));
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
            height: (size.height - status - bottom).max(32. * self.ui_scale),
        }
    }
    fn paint_input(layer: &mut Layer, input: &Input, focused: bool) {
        let rect = input.bounds;
        // Inputs shrunk by a very short panel scale their vertical offsets with the box, so
        // the glyphs (cap height to descender) stay inside it instead of sinking out.
        let k = (rect.height / (28. * input.scale)).clamp(0., 1.);
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
        if input.multiline() {
            let selected = state.selected.bytes(&state.text);
            let marked = state.marked.map(|m| m.bytes(&state.text));
            let s = state.caret.height / 18.;
            for (start, y, value) in input.visible_rows(&state.text) {
                let x = |byte: usize| {
                    input.x_for_byte(&value, byte.saturating_sub(start).min(value.len()))
                };
                let a = x(selected.start);
                let b = x(selected.end);
                if a < b {
                    fill(
                        layer,
                        Rect {
                            x: a,
                            y: y + 5. * s,
                            width: b - a,
                            height: 19. * s,
                        },
                        rect,
                        [0.23, 0.35, 0.5, 1.],
                    );
                }
                label(
                    layer,
                    input.visible_suffix(&value),
                    x(start),
                    y + 20. * s,
                    rect,
                    [0.9, 0.92, 0.95, 1.],
                );
                if let Some(marked) = &marked {
                    let a = x(marked.start);
                    let b = x(marked.end);
                    if a < b {
                        fill(
                            layer,
                            Rect {
                                x: a,
                                y: y + 24. * s,
                                width: b - a,
                                height: 1.,
                            },
                            rect,
                            [0.55, 0.8, 1., 1.],
                        );
                    }
                }
            }
            if focused {
                fill(layer, state.caret, rect, [0.9, 0.92, 0.97, 1.]);
            }
            return;
        }
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
                    y: rect.y + (5. * input.scale * k),
                    width: b - a,
                    height: (19. * input.scale * k),
                },
                rect,
                [0.23, 0.35, 0.5, 1.],
            );
        }
        label(
            layer,
            &shown,
            input.x_for_byte(&value, 0),
            rect.y + (20. * input.scale * k),
            rect,
            [0.9, 0.92, 0.95, 1.],
        );
        if focused {
            let caret = Rect {
                y: rect.y + (state.caret.y - rect.y) * k,
                height: state.caret.height * k,
                ..state.caret
            };
            fill(layer, caret, rect, [0.9, 0.92, 0.97, 1.]);
        }
        if let Some(marked) = state.marked {
            let range = marked.bytes(&state.text);
            let a = input.x_for_byte(&value, range.start);
            let b = input.x_for_byte(&value, range.end);
            fill(
                layer,
                Rect {
                    x: a,
                    y: rect.y + (24. * input.scale * k),
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
                y: self.quick_list_top(),
                width: self.quick_bounds.width,
                height: self.quick.scroll.viewport,
            };
            self.widget(11, "scroll_area", list, &[]);
            self.tree.nodes.get_mut(&1).expect("overlay").children = vec![10, 11];
            for row in self.quick.scroll.visible() {
                let item = &self.quick.items[row];
                let id = (1 << 40) + item.value as u64 * 5;
                let rect = Rect {
                    y: list.y + row as f64 * self.quick.scroll.row_height
                        - self.quick.scroll.offset,
                    height: self.quick.scroll.row_height,
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
                let focused = self.panel_focused && self.focus_input == i;
                let invalid = i == 0 && self.find_error.is_some();
                self.widget(
                    20 + i as u64,
                    "text_input",
                    bounds,
                    match (focused, invalid) {
                        (true, true) => &["focused", "invalid"],
                        (true, false) => &["focused"],
                        (false, true) => &["invalid"],
                        (false, false) => &[],
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
                // Option toggles under the inputs, buttons at their right (see
                // `find_controls`); only options that apply to this panel are shown.
                let controls = self.find_controls().unwrap_or_default();
                let visible = Self::find_toggle_options(panel);
                for (i, flag) in flags.into_iter().enumerate() {
                    let Some(rect) = visible
                        .iter()
                        .position(|v| *v == i)
                        .and_then(|slot| controls.toggles.get(slot).copied())
                    else {
                        continue;
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
                    let rect = controls.buttons.get(i).copied().unwrap_or_default();
                    // The first action is what Enter does in the panel: draw it as primary.
                    self.widget(
                        50 + i as u64,
                        "button",
                        rect,
                        if i == 0 { &["primary"] } else { &[] },
                    );
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
            let base = self.folder_row_base(workspace);
            for i in first.start.saturating_sub(1)
                ..first
                    .end
                    .saturating_sub(1)
                    .min(self.visible_open_files(workspace))
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
                    y: self.side_bounds.y
                        + (base as f64 + self.tree_row_position(i, Instant::now()))
                            * (24. * self.ui_scale)
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
        // Selection in a focused side bar uses the stronger focus colour (theme).
        if let Some(node) = self.tree.nodes.get_mut(&3) {
            if self.side_focused {
                node.attributes.insert("focused");
            } else {
                node.attributes.remove("focused");
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
        // The syntax and Git items are clickable too; they get hover feedback like it.
        for (id, item, label) in [
            (7, details::StatusItem::Syntax, "Syntax"),
            (8, details::StatusItem::Git, "Git status"),
        ] {
            if let Some(rect) = self.status_item(item) {
                self.widget(id, "status_button", rect, &[]);
                self.tree.nodes.get_mut(&id).expect("status item").label = label.into();
                self.tree
                    .nodes
                    .get_mut(&4)
                    .expect("status")
                    .children
                    .push(id);
            }
        }
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
        self.side_scroll.rows = self.folder_row_base(workspace) + self.side_rows.len();
        if !workspace
            .editor()
            .settings
            .bool("tree_animation_enabled", true)
            || workspace.themes.ui.borrow().reduce_motion()
            || self
                .tree_motion
                .as_ref()
                .is_some_and(|(at, _)| at.elapsed().as_secs_f64() >= 0.14)
        {
            self.tree_motion = None;
        }
        if self.tree_motion.is_some() {
            workspace.themes.ui.borrow_mut().request_animation();
        }
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
                ("Folders", self.folder_row_base(workspace) - 1),
            ]
            .into_iter()
            .skip(usize::from(!self.open_files_visible))
            {
                theme.text(
                    &mut layer,
                    &heading,
                    caption,
                    10. * self.ui_scale,
                    clip.y + row as f64 * (24. * self.ui_scale) - self.side_scroll.offset
                        + (17. * self.ui_scale),
                    clip,
                );
            }
            if workspace.folders.is_empty() {
                // Empty state under FOLDERS: say how to add one instead of a blank list.
                let hint = style(3, Some("label_control"), &mut theme);
                // First row under the FOLDERS heading, wherever Open Files left it.
                let top = clip.y + self.folder_row_base(workspace) as f64 * (24. * self.ui_scale)
                    - self.side_scroll.offset;
                for (line, caption) in ["No folders open", "Project \u{203a} Add Folder\u{2026}"]
                    .into_iter()
                    .enumerate()
                {
                    theme.text_fit(
                        &mut layer,
                        &hint,
                        caption,
                        16. * self.ui_scale,
                        top + (17. + line as f64 * 18.) * self.ui_scale,
                        clip.width - 26. * self.ui_scale,
                        clip,
                    );
                }
            }
            let visible = self.side_scroll.visible();
            for i in visible.start.saturating_sub(1)
                ..visible
                    .end
                    .saturating_sub(1)
                    .min(self.visible_open_files(workspace))
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
                let badge = tab
                    .path
                    .as_deref()
                    .and_then(|p| workspace.git.badge(p, false));
                let label_end =
                    self.paint_badge(&mut theme, &mut layer, settings, badge, node.rect, clip);
                let icon_rect = Rect {
                    x: (10. * self.ui_scale),
                    y: node.rect.y + (4. * self.ui_scale),
                    width: (16. * self.ui_scale),
                    height: (16. * self.ui_scale),
                };
                // Narrow side bars: the badge stays, the label shortens to nothing, and the
                // icon is left out before it would run into the badge.
                if self.side_column_fits(icon_rect, label_end)
                    && let Some(icon) = tab_icon(workspace, tab)
                {
                    theme.icon(&mut layer, icon, icon_rect, clip, label_style.foreground);
                }
                let label_x = (34. * self.ui_scale) + f64::from(label_style.margin.left);
                let label = tab.label();
                let shown = theme.fit_label(&label_style, &label, label_end - label_x);
                if !shown.is_empty() {
                    theme.text(
                        &mut layer,
                        &label_style,
                        &shown,
                        label_x,
                        node.rect.y + (17. * self.ui_scale),
                        clip,
                    );
                }
            }
            let base = self.folder_row_base(workspace);
            // Indent guides: one hairline per ancestor level, under the ancestor's disclosure
            // triangle. Themes without a `sidebar_indent_guide` rule draw none.
            let guide = theme.style(
                StyleId::SideGuide.at(0),
                &vel_theme::ElementPath(&[
                    vel_theme::Element::new("window", &[]),
                    vel_theme::Element::new("sidebar_container", &[]),
                    vel_theme::Element::new("sidebar_tree", &[]),
                    vel_theme::Element::new("sidebar_indent_guide", &[]),
                ]),
                settings,
            );
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
                for level in 0..row.depth {
                    let x = (10. + 8. + level as f64 * 16.) * self.ui_scale;
                    theme.surface(
                        &mut layer,
                        &guide,
                        Rect {
                            x: x.round(),
                            width: 1.,
                            ..node.rect
                        },
                        clip,
                    );
                }
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
                // 16-DIP disclosure and icon columns; a child's disclosure sits under its
                // parent's icon, and files reserve the disclosure column so siblings align.
                let x = (10. * self.ui_scale) + row.depth as f64 * (16. * self.ui_scale);
                let icon_rect = Rect {
                    x: x + (16. * self.ui_scale),
                    y: node.rect.y + (4. * self.ui_scale),
                    width: (16. * self.ui_scale),
                    height: (16. * self.ui_scale),
                };
                let badge = workspace.git.badge(&row.path, row.directory);
                let label_end =
                    self.paint_badge(&mut theme, &mut layer, settings, badge, node.rect, clip);
                // Narrow side bars: the badge stays, the label shortens to nothing, and the
                // icon (then the disclosure triangle) is left out before it would run into it.
                let disclosure_rect = Rect { x, ..icon_rect };
                if row.directory {
                    if self.side_column_fits(disclosure_rect, label_end) {
                        let disclosure = style(id, Some("disclosure_button_control"), &mut theme);
                        theme.icon_surface(&mut layer, &disclosure, disclosure_rect, clip);
                    }
                    if self.side_column_fits(icon_rect, label_end) {
                        let folder = style(id, Some("icon_folder"), &mut theme);
                        theme.icon_surface(&mut layer, &folder, icon_rect, clip);
                    }
                } else if self.side_column_fits(icon_rect, label_end)
                    && let Some(icon) = workspace.themes.file_icon(&row.path)
                {
                    theme.icon(&mut layer, icon, icon_rect, clip, text.foreground);
                }
                let label_x = x + (36. * self.ui_scale);
                theme.text_fit(
                    &mut layer,
                    &text,
                    &name,
                    label_x,
                    node.rect.y + (17. * self.ui_scale),
                    label_end - label_x,
                    clip,
                );
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
                            StyleId::ResultRow.at(row as u64),
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
                        let line = match self.result_view.rows.get(row).expect("visible result row")
                        {
                            files::Row::Header(path) => format!("{}:", path.display()),
                            files::Row::Gap => "        …".into(),
                            files::Row::Line(line) => format!(
                                "  {:>5}{} {}",
                                line.number,
                                if line.match_count == 0 { ' ' } else { ':' },
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
                    // No label column in very narrow panels (see `find_label_column`).
                    if input.bounds.x - clip.x >= 64. * self.ui_scale {
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
                    }
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
                    // Icon-only toggles (names are exposed to accessibility); the icon
                    // takes the accent colour while the option is on.
                    for (i, icon_class) in [
                        "icon_regex",
                        "icon_case",
                        "icon_whole_word",
                        "icon_wrap",
                        "icon_in_selection",
                        "icon_preserve_case",
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let id = 30 + i as u64;
                        let Some(node) = self.tree.nodes.get(&id).filter(|n| n.visible) else {
                            continue;
                        };
                        let button = style(id, None, &mut theme);
                        theme.surface(&mut layer, &button, node.rect, clip);
                        let icon = style(id, Some(icon_class), &mut theme);
                        theme.icon_surface(&mut layer, &icon, node.rect, node.rect);
                    }
                    for id in 50..50 + Self::panel_buttons(panel).len() as u64 {
                        let node = &self.tree.nodes[&id];
                        let button = style(id, None, &mut theme);
                        theme.surface(&mut layer, &button, node.rect, clip);
                        let width = theme.advance(&button, &node.label);
                        theme.text(
                            &mut layer,
                            &button,
                            &node.label,
                            (node.rect.x + (node.rect.width - width) / 2.).round(),
                            node.rect.y + node.rect.height / 2. + 4. * self.ui_scale,
                            node.rect,
                        );
                    }
                    if let Some(node) = self.tree.nodes.get(&CLOSE_ID).filter(|n| n.visible) {
                        // A plain close glyph with a hover wash, not a filled button.
                        let attributes: Vec<&str> = node.attributes.iter().copied().collect();
                        let close = theme.style(
                            StyleId::FindClose.at(0),
                            &vel_theme::ElementPath(&[
                                vel_theme::Element::new("window", &[]),
                                vel_theme::Element::new("panel_control", &[]),
                                vel_theme::Element::new("panel_close_button", &attributes),
                            ]),
                            settings,
                        );
                        theme.icon_surface(&mut layer, &close, node.rect, clip);
                    }
                    let message_style = if self.find_error.is_some() {
                        self.tree.style_child(
                            2,
                            Some("label_control"),
                            &["invalid"],
                            &mut theme,
                            settings,
                        )
                    } else {
                        text_style.clone()
                    };
                    let message_clip = self.panel_message_clip();
                    let message_clip = if self.message_in_find_panel() {
                        message_clip
                    } else {
                        // Too short to read: the status bar shows the message instead.
                        Rect::default()
                    };
                    // Multi-line errors (regex diagnostics) are shown on one line. The text is
                    // the find status (counts, active restrictions) or the panel message.
                    let message = self
                        .panel_status_message(workspace)
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ");
                    theme.text_fit(
                        &mut layer,
                        &message_style,
                        &message,
                        message_clip.x,
                        message_clip.y + message_clip.height / 2. + 4. * self.ui_scale,
                        message_clip.width,
                        message_clip,
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
            let tab = &workspace.tabs[workspace.active];
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
            // Right-hand items, laid out right to left with measured widths. Lower-priority
            // items drop out before they would crowd the position text, so nothing overlaps
            // at narrow widths; the left text is ellipsized to the room that remains.
            let s = self.ui_scale;
            let git_text = (editor.settings.bool("show_git_status_in_status_bar", true))
                .then(|| tab.path.as_deref().and_then(|p| workspace.git.status(p)))
                .flatten()
                .map(str::to_owned);
            let language = editor
                .language
                .borrow()
                .as_ref()
                .map(|l| l.name.clone())
                .unwrap_or_else(|| "Plain Text".into());
            // Bytes and pictures have no syntax or indentation to show or change.
            let text_document = editor.binary.is_none();
            let candidates = [
                (
                    details::StatusItem::Syntax,
                    if text_document {
                        language
                    } else {
                        String::new()
                    },
                    0.,
                ),
                (
                    details::StatusItem::Indentation,
                    if text_document {
                        vel_ui::status::indentation(
                            editor.preferences.translate_tabs_to_spaces,
                            editor.preferences.tab_size,
                        )
                    } else {
                        String::new()
                    },
                    0.,
                ),
                (
                    details::StatusItem::Git,
                    git_text.clone().unwrap_or_default(),
                    20.,
                ),
                (
                    details::StatusItem::Indexing,
                    self.intel
                        .percentage
                        .map(|p| format!("Indexing {p}%"))
                        .unwrap_or_default(),
                    0.,
                ),
            ];
            let left = 10. * s;
            let mut right = status.x + status.width - 6. * s;
            let mut status_items = Vec::new();
            // Item padding comes from the theme's status_button content margin (already
            // UI-scaled); labels are measured with the style they are painted with.
            let button = theme.style(
                StyleId::StatusButton.at(0),
                &vel_theme::ElementPath(&[
                    vel_theme::Element::new("window", &[]),
                    vel_theme::Element::new("status_bar", &[]),
                    vel_theme::Element::new("status_button", &[]),
                ]),
                settings,
            );
            let (pad_left, pad_right) = (
                f64::from(button.margin.left).max(4. * s),
                f64::from(button.margin.right).max(4. * s),
            );
            // Candidates are in priority order and placed right to left in that order, so
            // the first one that does not fit ends the row: a lower-priority item never
            // takes the place of a higher-priority one.
            for (item, label, icon) in candidates {
                if label.is_empty() {
                    continue;
                }
                let label_style = if item == details::StatusItem::Git {
                    style(4, Some("vcs_status"), &mut theme)
                } else {
                    text.clone()
                };
                let width = theme.advance(&label_style, &label) + pad_left + pad_right + icon * s;
                // Keep at least 140 DIPs for the position (or message) text.
                if right - width - left < 140. * s {
                    break;
                }
                let rect = Rect {
                    x: (right - width).round(),
                    y: status.y + 2. * s,
                    width: width.round(),
                    height: status.height - 4. * s,
                };
                right = rect.x - 2. * s;
                let id = match item {
                    details::StatusItem::Indentation => Some(6),
                    details::StatusItem::Syntax => Some(7),
                    details::StatusItem::Git => Some(8),
                    details::StatusItem::Indexing => None,
                };
                let item_style = id.map(|id| style(id, None, &mut theme));
                if let Some(button) = &item_style {
                    theme.surface(&mut layer, button, rect, status);
                }
                let mut x = rect.x + pad_left;
                if item == details::StatusItem::Git {
                    let branch = style(4, Some("vcs_branch_icon"), &mut theme);
                    theme.icon_surface(
                        &mut layer,
                        &branch,
                        Rect {
                            x: x - 2. * s,
                            y: status.y + 4. * s,
                            width: 16. * s,
                            height: 16. * s,
                        },
                        status,
                    );
                    x += icon * s;
                }
                theme.text(
                    &mut layer,
                    &label_style,
                    &label,
                    x,
                    status.y + 17. * s,
                    rect,
                );
                // The click target spans the bar's full height (the window edge stays
                // clickable); the drawn button is inset within it.
                status_items.push((
                    item,
                    Rect {
                        y: status.y,
                        height: status.height,
                        ..rect
                    },
                ));
            }
            self.status_items = Some(status_items);
            let message =
                if self.status_message(workspace).is_empty() || self.message_in_find_panel() {
                    // The find panels show their own message; do not repeat it here.
                    format!(
                        "{}{}{}",
                        document_hint,
                        if editor.vintage.is_some() {
                            format!("{} · ", editor.vintage_status())
                        } else {
                            String::new()
                        },
                        status_position(workspace)
                    )
                } else {
                    self.status_message(workspace).to_owned()
                };
            theme.text_fit(
                &mut layer,
                &text,
                &message,
                left,
                status.y + (17. * s),
                right - 8. * s - left,
                Rect {
                    width: (right - 8. * s).max(0.),
                    ..status
                },
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
            // The floating overlay container (background, border, shadow) holds the quick
            // panel; its shadow may extend past the panel, so clip to the window.
            let window = Rect {
                x: 0.,
                y: 0.,
                width: self.size.width,
                height: self.size.height,
            };
            let container = theme.style(
                StyleId::OverlayContainer.at(0),
                &vel_theme::ElementPath(&[
                    vel_theme::Element::new("window", &[]),
                    vel_theme::Element::new(
                        match self.overlay {
                            Some(Overlay::Palette) => "overlay_control command_palette",
                            Some(Overlay::Goto) => "overlay_control goto_file",
                            Some(Overlay::ProjectSymbols) => {
                                "overlay_control goto_symbol_in_project"
                            }
                            _ => "overlay_control",
                        },
                        &[],
                    ),
                ]),
                settings,
            );
            theme.surface(&mut overlay, &container, self.quick_bounds, window);
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
                y: self.quick_list_top(),
                width: self.quick_bounds.width,
                height: self.quick.scroll.viewport,
            };
            let s = self.ui_scale;
            // Command lists show a caption and key binding only (the command name is not
            // for users); kind badges appear where kinds differ (files, symbols).
            let compact = self.quick_compact();
            let badges = !compact && self.overlay != Some(Overlay::Appearance);
            let row_height = self.quick.scroll.row_height / s;
            for row in self.quick.scroll.visible() {
                let item = &self.quick.items[row];
                let id = (1 << 40) + item.value as u64 * 5;
                let rect = self.tree.nodes[&id].rect;
                let row_style = self.tree.style(id, None, &mut theme, settings);
                theme.surface(&mut overlay, &row_style, rect, rows);
                let caption = self
                    .tree
                    .style(id, Some("quick_panel_label"), &mut theme, settings);
                let hint =
                    self.tree
                        .style(id, Some("quick_panel_label hint"), &mut theme, settings);
                let annotation = mac_key_equivalents(&item.annotation);
                let annotation_width = theme.advance(&hint, &annotation);
                let text_x = rect.x + if badges { 36. } else { 12. } * s;
                // Rows without a details line centre their caption and badge vertically.
                let show_details = !compact && !item.details.is_empty();
                let baseline = rect.y
                    + if show_details {
                        19.
                    } else {
                        row_height / 2. + 4.
                    } * s;
                let caption_end = if item.annotation.is_empty() {
                    rect.x + rect.width - 12. * s
                } else {
                    rect.x + rect.width - annotation_width - 24. * s
                };
                if item.highlights.is_empty() {
                    theme.text_fit(
                        &mut overlay,
                        &caption,
                        &item.caption,
                        text_x,
                        baseline,
                        caption_end - text_x,
                        rows,
                    );
                } else {
                    // Matched characters are separate runs in the match colour; each glyph
                    // is drawn once, never overprinted on the unmatched caption.
                    let mut matched = caption.clone();
                    matched.foreground = matched.matched;
                    let clip = Rect {
                        width: (caption_end - rows.x).max(0.),
                        ..rows
                    };
                    // A caption too long for the row keeps its matched runs up to the cut
                    // and ends in an ellipsis, like unmatched captions.
                    let fitted = theme.fit_text(&caption, &item.caption, caption_end - text_x);
                    // Borrowed means it fits as is (a cut may be exactly as long as "…").
                    let shown = if matches!(fitted, std::borrow::Cow::Borrowed(_)) {
                        item.caption.len()
                    } else {
                        fitted
                            .strip_suffix('\u{2026}')
                            .map_or(0, str::len)
                            .min(item.caption.len())
                    };
                    let mut runs = Vec::new();
                    let mut start = 0;
                    let mut highlighted = item.highlights.contains(&0);
                    for (byte, _) in item.caption[..shown].char_indices().skip(1) {
                        let next = item.highlights.contains(&byte);
                        if next != highlighted {
                            runs.push((start, byte, highlighted));
                            start = byte;
                            highlighted = next;
                        }
                    }
                    if shown > start {
                        runs.push((start, shown, highlighted));
                    }
                    for (start, end, highlighted) in runs {
                        theme.text(
                            &mut overlay,
                            if highlighted { &matched } else { &caption },
                            &item.caption[start..end],
                            text_x + theme.advance(&caption, &item.caption[..start]),
                            baseline,
                            clip,
                        );
                    }
                    if shown < item.caption.len() && !fitted.is_empty() {
                        theme.text(
                            &mut overlay,
                            &caption,
                            "\u{2026}",
                            text_x + theme.advance(&caption, &item.caption[..shown]),
                            baseline,
                            clip,
                        );
                    }
                }
                let badge = Rect {
                    x: rect.x + 10. * s,
                    y: rect.y
                        + if show_details {
                            6.
                        } else {
                            (row_height - 18.) / 2.
                        } * s,
                    width: 18. * s,
                    height: 18. * s,
                };
                let file_icon = (self.overlay == Some(Overlay::Goto) && item.kind == "file")
                    .then(|| workspace.themes.file_icon(Path::new(&item.details)))
                    .flatten();
                if badges && let Some(icon) = file_icon {
                    theme.icon(
                        &mut overlay,
                        icon,
                        Rect {
                            x: badge.x + 1. * s,
                            y: badge.y + 1. * s,
                            width: 16. * s,
                            height: 16. * s,
                        },
                        rows,
                        hint.foreground,
                    );
                } else if badges {
                    let kind_class = match item.kind.as_str() {
                        "function" | "symbol" => "kind_container kind_function",
                        "command" => "kind_container kind_keyword",
                        _ => "kind_container kind_navigation",
                    };
                    let kind = self.tree.style(id, Some(kind_class), &mut theme, settings);
                    theme.surface(&mut overlay, &kind, badge, rows);
                    let letter = item.kind.chars().next().unwrap_or('·').to_string();
                    let label = theme.style(
                        StyleId::OverlayKindLabel.at(row as u64),
                        &vel_theme::ElementPath(&[
                            vel_theme::Element::new("window", &[]),
                            vel_theme::Element::new(kind_class, &[]),
                            vel_theme::Element::new("kind_label", &[]),
                        ]),
                        settings,
                    );
                    let width = theme.advance(&label, &letter);
                    theme.text(
                        &mut overlay,
                        &label,
                        &letter,
                        (badge.x + (badge.width - width) / 2.).round(),
                        badge.y + 13. * s,
                        vel_ui::minihtml::intersection(badge, rows),
                    );
                }
                theme.text(
                    &mut overlay,
                    &hint,
                    &annotation,
                    (rect.x + rect.width - annotation_width - 12. * s).max(text_x),
                    baseline,
                    rows,
                );
                if !show_details {
                    continue;
                }
                let detail =
                    self.tree
                        .style(id, Some("quick_panel_detail_label"), &mut theme, settings);
                // Inline minihtml follows the engine's restricted details mode.
                let options = vel_minihtml::LayoutOptions {
                    max_width: (rect.width - 48. * s).max(1.) as f32,
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
                    // One detail line per row: a long path is clipped at the row edge
                    // instead of wrapping into a second, half-hidden line.
                    extra_css: "html{white-space:nowrap}".into(),
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
                                x: text_x,
                                y: rect.y + 24. * s,
                                width: rect.width - 48. * s,
                                height: (19. * s),
                            },
                            rows,
                        ),
                    );
                }
            }
            // More results than rows: a slim scroll indicator at the list's right edge.
            let content = self.quick.items.len() as f64 * self.quick.scroll.row_height;
            if content > rows.height + 0.5 && rows.height > 0. {
                let thumb_height = (rows.height * rows.height / content).max(20. * s);
                let travel = (rows.height - thumb_height).max(0.);
                let progress = self.quick.scroll.offset / (content - rows.height).max(1.);
                let puck = theme.style(
                    StyleId::QuickScroll.at(0),
                    &vel_theme::ElementPath(&[
                        vel_theme::Element::new("window", &[]),
                        vel_theme::Element::new("quick_panel", &[]),
                        vel_theme::Element::new("puck_control", &[]),
                    ]),
                    settings,
                );
                theme.surface(
                    &mut overlay,
                    &puck,
                    Rect {
                        x: rows.x + rows.width - 7. * s,
                        y: rows.y + travel * progress.clamp(0., 1.),
                        width: 4. * s,
                        height: thumb_height,
                    },
                    rows,
                );
            }
            if !self.quick_tight {
                let footer = hint_footer(&mut theme, settings);
                theme.text(
                    &mut overlay,
                    &footer,
                    self.overlay_footer(),
                    rows.x + (12. * self.ui_scale),
                    self.quick_bounds.y + self.quick_bounds.height - (9. * self.ui_scale),
                    self.quick_bounds,
                );
            }
            scene.layers.push(overlay);
        }
        self.paint_hover(workspace, scene);
        self.refresh_tooltip(workspace);
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
        if down
            && self.overlay.is_none()
            && self.find_jump_origin.is_some()
            && !contains(self.panel_bounds, point)
        {
            self.finish_find_navigation(workspace);
        }

        if self.overlay.is_some() {
            if down && !contains(self.quick_bounds, point) {
                self.cancel_overlay(workspace);
            } else if down && contains(self.quick_input.bounds, point) {
                self.quick_input.click(point.x, extend);
            } else if down
                && contains(self.quick_bounds, point)
                && point.y >= self.quick_list_top()
                && point.y < self.quick_list_top() + self.quick.scroll.viewport
            {
                // Only the visible list selects; the footer below it is inert.
                let row = ((point.y - self.quick_list_top() + self.quick.scroll.offset)
                    / self.quick.scroll.row_height.max(1.)) as usize;
                if row < self.quick.items.len() {
                    self.quick.selected = row;
                    self.preview_current(workspace)?;
                    self.pending_command = self.accept(workspace, false, viewport)?;
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
            && self
                .status_item(details::StatusItem::Git)
                .is_some_and(|rect| contains(rect, point))
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
            && self
                .status_item(details::StatusItem::Syntax)
                .is_some_and(|rect| contains(rect, point))
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
                            self.result_navigated = true;
                            if double {
                                self.jump_result(workspace)?;
                            }
                        }
                    }
                } else if let Some(panel) = self.panel {
                    for (i, input) in [&mut self.query, &mut self.replace, &mut self.where_input]
                        .into_iter()
                        .take(Self::find_fields(panel))
                        .enumerate()
                    {
                        if contains(input.bounds, point) {
                            self.focus_input = i;
                            input.click_at(point, extend);
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
                if (1..=self.visible_open_files(workspace)).contains(&row) {
                    workspace.click_sheet(row - 1, self.primary_click, extend);
                    self.side_open = Some(row - 1);
                    if double {
                        workspace.make_permanent(workspace.active);
                    }
                } else if row >= self.folder_row_base(workspace) {
                    self.side_open = None;
                    let at = (point.y - self.side_bounds.y + self.side_scroll.offset)
                        / (24. * self.ui_scale)
                        - self.folder_row_base(workspace) as f64;
                    let now = Instant::now();
                    let index = (0..self.side_rows.len())
                        .rev()
                        .find(|&i| {
                            let y = self.tree_row_position(i, now);
                            at >= y && at < y + 1.
                        })
                        .unwrap_or_else(|| {
                            (row - self.folder_row_base(workspace))
                                .min(self.side_rows.len().saturating_sub(1))
                        });
                    self.select_side(index);
                    if extend
                        && let Some(anchor) = self
                            .side_anchor
                            .as_ref()
                            .and_then(|p| self.side_rows.iter().position(|r| &r.path == p))
                    {
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
                        self.side_anchor = self.side_selection.clone();
                    } else {
                        // Single clicks select only when preview is disabled. Double click
                        // always opens permanently, including with preview_on_click=false.
                        // https://docs.sublimetext.io/reference/settings.html#global-settings
                        // "only_left": https://www.sublimetext.com/download (ST4 Files and Folders).
                        let preview = workspace.editor().settings.get("preview_on_click");
                        let enabled = preview.and_then(Value::as_bool).unwrap_or(true)
                            || preview.and_then(Value::as_str) == Some("only_left");
                        if double
                            || enabled
                            || self
                                .side_rows
                                .get(self.side_selected)
                                .is_some_and(|r| r.directory)
                        {
                            self.activate_side(workspace, double)?;
                        }
                        self.side_anchor = self.side_selection.clone();
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
        workspace.editor_mut().settings.set(
            vel_config::settings::Layer::View,
            "close_find_after_replace_all",
            json!(false),
        );
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
        panels
            .action(
                &mut workspace,
                "toggle_find_option",
                &json!({"option":"in_selection"}),
                0..11,
            )
            .unwrap();
        panels.replace.set("x");
        panels
            .action(
                &mut workspace,
                "replace_all",
                &json!({"_close_panel":false}),
                0..11,
            )
            .unwrap();
        assert_eq!(workspace.editor().text.to_string(), "x x\ncat");
        panels
            .action(
                &mut workspace,
                "replace_all",
                &json!({"_close_panel":false}),
                0..11,
            )
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
    /// Status-bar items never overlap each other or the position text: at every width and UI
    /// scale they are laid out right to left, and the ones that do not fit are left out.
    #[test]
    fn status_items_never_overlap_at_narrow_widths() {
        let mut workspace = workspace();
        workspace.set_file_type("Rust").unwrap();
        for scale in [1., 2.] {
            workspace.editor_mut().settings.set(
                vel_config::settings::Layer::View,
                "ui_scale",
                json!(scale),
            );
            for width in [1200., 900., 640., 520., 420., 300., 220.] {
                let mut panels = Panels::new(&workspace, false);
                panels.side_visible = false;
                panels.layout(Size {
                    width,
                    height: 400.,
                });
                let mut scene = Scene::default();
                panels.paint(&workspace, &mut scene);
                let s = panels.ui_scale;
                let items = panels.status_items.clone().expect("painted");
                if width >= 640. * s {
                    assert_eq!(items.len(), 2, "{width} @{scale}: syntax and indentation");
                }
                let status_top = 400. - STATUS * s;
                for (i, (item, rect)) in items.iter().enumerate() {
                    assert!(
                        rect.x >= 0. && rect.x + rect.width <= width,
                        "{item:?} {rect:?}"
                    );
                    assert_eq!((rect.y, rect.height), (status_top, STATUS * s));
                    // Right to left, with a gap, and room left for the position text.
                    if let Some((_, previous)) = i.checked_sub(1).map(|p| &items[p]) {
                        assert!(
                            rect.x + rect.width < previous.x,
                            "{width} @{scale}: {items:?}"
                        );
                    }
                    assert!(
                        rect.x >= 140. * s,
                        "{width} @{scale}: {item:?} crowds the text"
                    );
                }
                let leftmost = items.iter().map(|(_, r)| r.x).fold(width, f64::min);
                let position = scene
                    .layers
                    .iter()
                    .flat_map(|l| &l.text)
                    .find(|r| f64::from(r.baseline) > status_top && r.text.starts_with("Line"))
                    .or_else(|| {
                        scene
                            .layers
                            .iter()
                            .flat_map(|l| &l.text)
                            .find(|r| f64::from(r.baseline) > status_top && r.clip.x == 0.)
                    })
                    .expect("position text");
                assert!(
                    position.clip.x + position.clip.width <= leftmost,
                    "{width} @{scale}: position text runs under the items"
                );
                // Each label is measured with the style it is painted with: it ends inside
                // its item.
                for (item, rect) in &items {
                    for run in status_runs(&workspace, &scene, status_top) {
                        let (x, end) = run;
                        if x >= rect.x && x < rect.x + rect.width {
                            assert!(end <= rect.x + rect.width + 0.01, "{item:?} {rect:?}");
                        }
                    }
                }
            }
        }
        // Priority: a long syntax name that does not fit never lets the indentation item
        // take its place; lower-priority items drop first.
        workspace.editor_mut().settings.set(
            vel_config::settings::Layer::View,
            "ui_scale",
            json!(1.),
        );
        workspace
            .editor()
            .language
            .borrow_mut()
            .as_mut()
            .unwrap()
            .name = "A Rather Long Syntax Name For The Status Bar".into();
        let mut dropped = 0;
        for width in (220..=900).step_by(20) {
            let mut panels = Panels::new(&workspace, false);
            panels.side_visible = false;
            panels.layout(Size {
                width: f64::from(width),
                height: 400.,
            });
            panels.paint(&workspace, &mut Scene::default());
            let shown: Vec<_> = panels
                .status_items
                .clone()
                .expect("painted")
                .into_iter()
                .map(|(item, _)| item)
                .collect();
            let order = [
                details::StatusItem::Syntax,
                details::StatusItem::Indentation,
            ];
            assert_eq!(shown, order[..shown.len()], "{width}");
            dropped += usize::from(shown.len() < 2);
        }
        assert!(dropped > 0);
    }
    /// Left and right ends of the text runs painted in the status bar.
    fn status_runs(workspace: &Workspace, scene: &Scene, top: f64) -> Vec<(f64, f64)> {
        let metrics = workspace.themes.ui.borrow().metrics.clone();
        scene
            .layers
            .iter()
            .flat_map(|l| &l.text)
            .filter(|r| f64::from(r.baseline) > top && !r.text.is_empty())
            .map(|r| (f64::from(r.x), f64::from(r.x) + run_width(&*metrics, r)))
            .collect()
    }
    /// Measured advance of a painted text run.
    fn run_width(metrics: &dyn vel_minihtml::FontMetrics, run: &vel_render::scene::TextRun) -> f64 {
        let style = vel_minihtml::TextStyle {
            font_family: run.family.clone().unwrap_or_else(|| "monospace".into()),
            font_size: run.size,
            font_weight: if run.style.bold { 700 } else { 400 },
            italic: run.style.italic,
            underline: false,
            color: vel_minihtml::Color::rgb(255, 255, 255),
        };
        f64::from(metrics.measure(&run.text, &style).advance)
    }
    /// A matched caption too long for its row is cut with an ellipsis: matched runs stop at
    /// the cut and nothing is drawn past the row's text column.
    #[test]
    fn long_matched_captions_end_in_an_ellipsis() {
        let workspace = workspace();
        let caption = "src/render/an_unusually_long_module_name_for_truncation_checks.rs";
        let highlights: Vec<usize> = caption
            .char_indices()
            .filter(|(_, c)| *c == 'r' || *c == 's')
            .map(|(i, _)| i)
            .collect();
        for width in [420., 520., 700.] {
            let mut panels = Panels::new(&workspace, false);
            panels.side_visible = false;
            panels.overlay = Some(Overlay::Goto);
            panels.quick.set_items(vec![vel_ui::Item {
                caption: caption.into(),
                kind: "file".into(),
                details: "/tmp/project/src/render".into(),
                highlights: highlights.clone(),
                ..Default::default()
            }]);
            panels.layout(Size {
                width,
                height: 400.,
            });
            let mut scene = Scene::default();
            panels.paint(&workspace, &mut scene);
            let first = scene
                .layers
                .iter()
                .flat_map(|l| &l.text)
                .find(|r| r.text == "\u{2026}")
                .unwrap_or_else(|| panic!("{width}: no ellipsis"))
                .baseline;
            let runs: Vec<_> = scene
                .layers
                .iter()
                .flat_map(|l| &l.text)
                .filter(|r| r.baseline == first && !r.text.is_empty())
                .collect();
            let shown: String = runs.iter().map(|r| r.text.as_str()).collect();
            assert!(shown.ends_with('\u{2026}'), "{width}: {shown}");
            assert!(
                caption.starts_with(shown.trim_end_matches('\u{2026}')),
                "{shown}"
            );
            let list = panels.quick_bounds;
            for run in &runs {
                let end = f64::from(run.x) + run_width(&*workspace.themes.ui.borrow().metrics, run);
                assert!(
                    end <= list.x + list.width,
                    "{width}: {:?} at {}",
                    run.text,
                    run.x
                );
            }
        }
    }
    /// The empty-state hint sits directly under the FOLDERS heading, with or without the
    /// Open Files group above it.
    #[test]
    fn empty_folder_hint_follows_the_folders_heading() {
        let workspace = workspace();
        for open_files in [true, false] {
            let mut panels = Panels::new(&workspace, false);
            panels.side_visible = true;
            panels.open_files_visible = open_files;
            panels.layout(Size {
                width: 900.,
                height: 400.,
            });
            let mut scene = Scene::default();
            panels.paint(&workspace, &mut scene);
            let baseline = |text: &str| {
                scene
                    .layers
                    .iter()
                    .flat_map(|l| &l.text)
                    .find(|r| r.text == text)
                    .map(|r| f64::from(r.baseline))
                    .unwrap_or_else(|| panic!("{text} not painted"))
            };
            let heading = baseline("FOLDERS");
            let hint = baseline("No folders open");
            assert_eq!(
                hint - heading,
                24. * panels.ui_scale,
                "open files {open_files}"
            );
        }
    }
    /// Labels in a narrow side bar never spill over the Git badges: when not even an ellipsis
    /// fits, no label glyph is drawn at all.
    #[test]
    fn narrow_side_bar_labels_never_draw_over_git_badges() {
        use vel_git::{Badge, FileStatus, Head, StatusSnapshot};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let mut workspace = workspace();
        let mut files = std::collections::BTreeMap::new();
        for name in ["first_rather_long_name.rs", "second_rather_long_name.rs"] {
            let path = root.join(name);
            std::fs::write(&path, "x\n").unwrap();
            workspace
                .open(crate::cli::FileLocation {
                    path,
                    line: 1,
                    column: 1,
                })
                .unwrap();
            files.insert(
                PathBuf::from(name),
                FileStatus {
                    statuses: [Badge::Modified].into_iter().collect(),
                },
            );
        }
        workspace.git.insert_snapshot(StatusSnapshot {
            root,
            generation: 1,
            head: Head::Branch {
                name: "main".into(),
                unborn: false,
            },
            ahead_behind: None,
            files,
            folders: std::collections::BTreeMap::new(),
        });
        let metrics = workspace.themes.ui.borrow().metrics.clone();
        let mut badges = 0;
        for width in (120..=500).step_by(10) {
            let mut panels = Panels::new(&workspace, false);
            panels.side_visible = true;
            panels.layout(Size {
                width: f64::from(width),
                height: 400.,
            });
            let mut scene = Scene::default();
            panels.paint(&workspace, &mut scene);
            let side = panels.side_bounds;
            let runs: Vec<_> = scene
                .layers
                .iter()
                .flat_map(|l| &l.text)
                .filter(|r| !r.text.is_empty() && f64::from(r.x) < side.x + side.width)
                .collect();
            for badge in runs.iter().filter(|r| r.text == "M") {
                badges += 1;
                for label in runs
                    .iter()
                    .filter(|r| r.baseline == badge.baseline && r.x < badge.x)
                {
                    let end = f64::from(label.x) + run_width(&*metrics, label);
                    assert!(
                        end <= f64::from(badge.x) + 0.01,
                        "{width}: {:?} runs into the badge at {}",
                        label.text,
                        badge.x
                    );
                }
            }
        }
        assert!(badges > 0, "no badges painted");
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
    fn goto_uses_shared_files_and_retries_unopened_file_symbol() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().canonicalize().unwrap();
        std::fs::write(dir.join("a.rs"), "fn first() {}\n\nfn beta() {}\n").unwrap();
        let mut workspace = Workspace::new(vel_text::Text::new(), None, None, false).unwrap();
        let mut panels = Panels::new(&workspace, false);
        panels.add_folder(&mut workspace, dir.clone()).unwrap();
        wait(&mut workspace, &mut panels, |_, panels| {
            panels.file_candidates.len() == 1
        });
        assert!(workspace.tabs.iter().all(|t| t.path.is_none()));
        workspace.goto_recent_files.push(dir.join("a.rs"));
        panels
            .action(
                &mut workspace,
                "show_overlay",
                &json!({"overlay":"goto","text":"a.rs@beta"}),
                0..10,
            )
            .unwrap();
        assert!(Arc::ptr_eq(&panels.candidates, &panels.file_candidates));
        wait(&mut workspace, &mut panels, |w, _| {
            w.tabs[w.active].path.as_deref() == Some(dir.join("a.rs").as_path())
                && w.editor().text.byte_to_line(w.editor().caret) == 2
        });
    }
    #[test]
    fn file_at_symbol_previews_the_first_local_picker_result() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().canonicalize().unwrap();
        let mut source = String::new();
        for i in 0..200 {
            source.push_str(&format!("fn xxxx_Dat_{i:03}() {{}}\n"));
        }
        source.push_str("fn xd_Data_000() {}\n");
        std::fs::write(dir.join("a.rs"), &source).unwrap();
        let mut workspace = Workspace::new(vel_text::Text::new(), None, None, false).unwrap();
        let mut panels = Panels::new(&workspace, false);
        panels.add_folder(&mut workspace, dir.clone()).unwrap();
        wait(&mut workspace, &mut panels, |_, panels| {
            panels.file_candidates.len() == 1
        });
        panels
            .action(
                &mut workspace,
                "show_overlay",
                &json!({"overlay":"goto","text":"a.rs@dat"}),
                0..10,
            )
            .unwrap();
        wait(&mut workspace, &mut panels, |w, p| {
            w.tabs[w.active].path.as_ref() == Some(&dir.join("a.rs"))
                && p.preview_symbol_version.is_some()
        });
        let preview = workspace.editor().caret;
        assert_eq!(workspace.editor().text.byte_to_line(preview), 200);
        panels
            .key(&mut workspace, &Key::parse("enter").unwrap(), 0..10)
            .unwrap();
        panels
            .action(
                &mut workspace,
                "show_overlay",
                &json!({"overlay":"goto","text":"@dat"}),
                0..10,
            )
            .unwrap();
        wait(&mut workspace, &mut panels, |_, p| {
            !p.quick_pending && !p.quick.items.is_empty()
        });
        assert!(panels.quick.items[0].caption.contains("xd_Data_000"));
        panels
            .key(&mut workspace, &Key::parse("enter").unwrap(), 0..10)
            .unwrap();
        assert_eq!(workspace.editor().caret, preview);
    }

    #[test]
    fn local_symbols_keep_source_order_with_kind_and_line() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("a.rs");
        std::fs::write(&file, "fn extraordinary_name() {}\nfn x() {}\n").unwrap();
        let mut workspace = Workspace::new(vel_text::Text::new(), None, None, false).unwrap();
        workspace
            .open(FileLocation {
                path: file,
                line: 1,
                column: 1,
            })
            .unwrap();
        let mut panels = Panels::new(&workspace, false);
        panels
            .action(
                &mut workspace,
                "show_overlay",
                &json!({"overlay":"goto","text":"@"}),
                0..10,
            )
            .unwrap();
        wait(&mut workspace, &mut panels, |_, p| p.quick.items.len() == 2);
        assert!(panels.quick.items[0].caption.contains("extraordinary_name"));
        assert_eq!(panels.quick.items[1].annotation, "Line 2");
        assert_eq!(panels.quick.items[0].kind, "Function");
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
            jump.sheet += w.jump_history.borrow().back.len() as u64 + 1;
            w.jump_history.borrow_mut().remember(jump);
        }
        assert!(w.jump_history.borrow().back.len() <= 128);
    }
    #[test]
    fn unsaved_analysis_replaces_disk_and_close_restores_it() {
        let (_root, _data, mut w, mut p) = setup();
        p.intel.flush_publications().unwrap();
        let attach = p.intel.defer_project_attachment();
        w.editor_mut()
            .set_selection(Selection::from_regions([Region::from(3..9)]));
        w.editor_mut()
            .run("insert", &json!({"characters":"renamed"}))
            .unwrap();
        // Force edit analysis to finish before the index attaches.
        wait(&mut w, &mut p, |w, _| {
            w.editor()
                .language
                .borrow()
                .as_ref()
                .is_some_and(|l| l.ready() && l.version == w.editor().buffer.change_id())
        });
        assert!(p.intel.projects.is_empty());
        attach();
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
        if row > 0 && row <= self.visible_open_files(workspace) {
            return workspace.tabs[row - 1].path.clone().map(|p| (p, false));
        }
        row.checked_sub(self.folder_row_base(workspace))
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
        let text = self.tooltip_text(workspace, point);
        if self.review.tooltip.as_ref().map(|(t, _, _)| t) != text.as_ref() {
            self.review.tooltip = text.map(|t| (t, point, Instant::now()));
        }
    }
    /// Tool tip for the find-panel control or side-bar Git badge under the pointer.
    fn tooltip_text(&self, workspace: &Workspace, point: Point) -> Option<String> {
        if let Some(tip) = self.panel_tooltip(workspace, point) {
            Some(tip)
        } else if self.side_visible && point.x >= self.side_bounds.width - 32. {
            self.review_side_path(workspace, point)
                .and_then(|(path, folder)| workspace.git.badge(&path, folder))
                .map(|b| crate::git_ui::badge_text(b).1.to_owned())
        } else {
            None
        }
    }
    /// Keeps a shown tool tip true to the current state: an option toggled by its shortcut
    /// updates the text, a closed panel removes it, without waiting for a pointer move.
    fn refresh_tooltip(&mut self, workspace: &Workspace) {
        let Some((text, point, _)) = &self.review.tooltip else {
            return;
        };
        // Recompute only when the control under the pointer or an option changed.
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            self.tree.hit_test(*point).hash(&mut h);
            format!("{:?}", self.panel).hash(&mut h);
            (
                self.options.regex,
                self.options.case_sensitive,
                self.options.whole_word,
                self.options.wrap,
                self.in_selection,
                self.options.preserve_case,
            )
                .hash(&mut h);
            text.hash(&mut h);
            h.finish()
        };
        if key == self.tooltip_key {
            return;
        }
        self.tooltip_key = key;
        let Some((text, point, _)) = &self.review.tooltip else {
            return;
        };
        match self.tooltip_text(workspace, *point) {
            None => self.review.tooltip = None,
            Some(fresh) if fresh != *text => {
                if let Some(tooltip) = &mut self.review.tooltip {
                    tooltip.0 = fresh;
                }
            }
            Some(_) => {}
        }
    }
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
        self.git_tree_key = key;
        self.git_tree_dirty = false;
        if workspace.git.snapshots.is_empty() && self.git_virtual.is_empty() {
            return false;
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
            let mut added = false;
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
                        added = true;
                    }
                }
            }
            if added {
                sort_sidebar_entries(children);
            }
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
        // Compact headings: the default h3 margins made a two-entry popup taller than the view.
        let mut source = String::from(
            "<style>body{margin:0}h3{font-size:0.85rem;margin:6px 0 2px 0;color:color(var(--foreground) alpha(0.6))}div{margin:2px 0}</style>",
        );
        for (i, location) in self.hover.iter().take(16).enumerate() {
            if i == 0 && self.hover_definitions > 0 {
                source.push_str("<h3>Definitions</h3>");
            }
            if i == self.hover_definitions {
                source.push_str("<h3>References</h3>");
            }
            source.push_str(&format!("<div><a href='definition:{i}' title='Open location'><b>{}</b></a> <code>{}:{}</code></div>",
                escape_html(&location.name), escape_html(&location.file.file_name().unwrap_or_default().to_string_lossy()), location.row + 1));
        }

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
                StyleId::HoverPopup.at(0),
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
            // The html background covers the surface; keep the popup's edge visible.
            let mut edge = Layer::default();
            theme.border(&mut edge, &style, rect, viewport);
            scene.layers.push(edge);
        }
    }
}
impl Panels {
    /// Paints a side-bar Git badge right-aligned in its status colour and returns the x at
    /// which the row's label must end (ellipsized) so the two never overlap.
    #[allow(clippy::too_many_arguments)]
    /// Whether a side-bar icon column ends clear of the row's Git badge (which starts 6 DIPs
    /// after `label_end`), keeping 2 DIPs between them.
    /// Rows without a badge keep their icons (the side bar edge clips them).
    fn side_column_fits(&self, column: Rect, label_end: f64) -> bool {
        let no_badge =
            label_end >= self.side_bounds.x + self.side_bounds.width - 10. * self.ui_scale - 0.01;
        no_badge || column.x + column.width <= label_end + 4. * self.ui_scale + 0.01
    }
    fn paint_badge(
        &self,
        theme: &mut vel_ui::theme::ThemeUi,
        layer: &mut Layer,
        settings: &vel_config::settings::Settings,
        badge: Option<vel_git::Badge>,
        row: Rect,
        clip: Rect,
    ) -> f64 {
        let right = clip.x + clip.width - 10. * self.ui_scale;
        let Some(badge) = badge else {
            return right;
        };
        let text = crate::git_ui::badge_text(badge).0;
        if text.is_empty() {
            return right;
        }
        use vel_git::Badge::*;
        let (index, attribute) = match badge {
            Ignored => (0, "ignored"),
            Untracked => (1, "untracked"),
            Modified => (2, "modified"),
            Missing => (3, "missing"),
            StagedAddition => (4, "added"),
            StagedModification => (5, "staged"),
            StagedDeletion => (6, "deleted"),
            Unmerged => (7, "unmerged"),
        };
        // Badge colours depend only on the status: one cached style per status.
        let style = theme.style(
            StyleId::GitBadge.at(index),
            &vel_theme::ElementPath(&[
                vel_theme::Element::new("window", &[]),
                vel_theme::Element::new("sidebar_container", &[]),
                vel_theme::Element::new("vcs_status_badge", &[attribute]),
            ]),
            settings,
        );
        let width = theme.advance(&style, text);
        let x = right - width;
        theme.text(layer, &style, text, x, row.y + 17. * self.ui_scale, clip);
        x - 6. * self.ui_scale
    }
}
/// Shows key binding hints ("Ctrl+Shift+Cmd+F, Cmd+B") the way macOS menus do ("⌃⇧⌘F ⌘B").
/// Display only; anything that is not a binding is returned unchanged.
fn mac_key_equivalents(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains('+') {
        return text.into();
    }
    let mut out = Vec::new();
    for chord in text.split(", ") {
        let mut parts: Vec<&str> = chord.split('+').collect();
        let Some(key) = parts.pop().filter(|k| !k.is_empty()) else {
            return text.into();
        };
        let mut symbols = String::new();
        for (name, symbol) in [("Ctrl", '⌃'), ("Alt", '⌥'), ("Shift", '⇧'), ("Cmd", '⌘')] {
            if parts.contains(&name) {
                symbols.push(symbol);
            }
        }
        if symbols.chars().count() != parts.len() {
            return text.into();
        }
        symbols.push_str(match key {
            "UP" => "↑",
            "DOWN" => "↓",
            "LEFT" => "←",
            "RIGHT" => "→",
            "ENTER" => "↩",
            "TAB" => "⇥",
            "ESCAPE" => "⎋",
            "BACKSPACE" => "⌫",
            "DELETE" => "⌦",
            "SPACE" => "Space",
            other => other,
        });
        out.push(symbols);
    }
    out.join(" ").into()
}
/// Secondary text style for the overlay footer.
fn hint_footer(
    theme: &mut vel_ui::theme::ThemeUi,
    settings: &vel_config::settings::Settings,
) -> vel_ui::theme::Style {
    theme.style(
        StyleId::OverlayFooter.at(0),
        &vel_theme::ElementPath(&[
            vel_theme::Element::new("window", &[]),
            vel_theme::Element::new("overlay_control", &[]),
            vel_theme::Element::new("quick_panel_label hint", &[]),
        ]),
        settings,
    )
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

#[cfg(test)]
#[path = "panels_lane_b_round18_tests.rs"]
mod lane_b_round18_tests;
