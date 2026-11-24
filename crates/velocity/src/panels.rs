//! Headless UI controller: navigation, panel focus, workers and project tree state.
use crate::{cli::FileLocation, ui_text::UiText, workspace::Workspace};
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
use vel_render::scene::{Layer, Scene, Underline};
use vel_search::{
    files, fuzzy,
    goto::{Query, Target},
    project::{Entry, Folder, Index, Rules, sort_sidebar_entries},
    text::{self, Search},
};
mod appearance_selection;
mod details;
pub(crate) mod find_results;
mod goto_files;
mod quick_icons;
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
    Focus, Item, ItemIcon, Node, QuickPanel, Scroll, Surface, Tree, contains, fill, input::Input,
    label,
};

/// Rows of the side bar's "Open Files" list: none for the hidden input tab of an empty window.
fn open_file_count(workspace: &Workspace) -> usize {
    if workspace.is_empty() {
        0
    } else {
        workspace.tabs.len()
            - usize::from(workspace.tabs.last().is_some_and(|tab| tab.goto_preview))
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
const STATUS_SWITCHER: u64 = 9;
pub(super) const FIND_OPTIONS: [&str; 8] = [
    "regex",
    "case_sensitive",
    "whole_word",
    "wrap",
    "in_selection",
    "preserve_case",
    "context",
    "use_buffer",
];
/// Document-format menu model; Lane B connects it to the status hit rectangles.
pub(crate) fn status_format_menu(
    workspace: &Workspace,
    item: vel_ui::status::FormatItem,
) -> Vec<vel_ui::status::FormatMenuItem> {
    let tab = &workspace.tabs[workspace.active];
    match item {
        vel_ui::status::FormatItem::Encoding => {
            let mut menu = vel_ui::status::encoding_menu(
                vel_fileio::ENCODINGS,
                tab.format.display_encoding(),
                tab.path.is_some(),
                !tab.editor.read_only && !workspace.is_empty(),
            );
            for row in &mut menu {
                row.caption = workspace.translations.caption(&row.caption).to_owned();
            }
            menu
        }
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
        let row = editor.text.byte_to_line(editor.caret);
        (
            row + 1,
            crate::status_position::selection_column(editor, row),
            regions
                .iter()
                .filter(|r| !r.empty())
                .map(|r| editor.text.slice(r.to_range()).len_chars())
                .sum(),
        )
    };
    let mode = editor.settings.string("show_line_column", "compact");
    let encoding = editor
        .settings
        .bool("show_encoding", true)
        .then_some(tab.format.display_encoding());
    let endings = editor
        .settings
        .bool("show_line_endings", true)
        .then_some(tab.format.line_ending.display_name());
    let sum = (selected > 0)
        .then(|| crate::status_position::selection_sum(editor))
        .flatten();
    if workspace.translations.language().is_english() && sum.is_none() {
        return vel_ui::status::position_mode(
            mode,
            row,
            column,
            regions.len(),
            selected,
            encoding,
            endings,
        );
    }
    let ui = &workspace.ui;
    let mut fields = Vec::with_capacity(5);
    if mode != "disabled" {
        fields.push(vel_i18n::format_named(
            if mode == "enabled" {
                &ui.full_position
            } else {
                &ui.compact_position
            },
            &[("line", row.to_string()), ("column", column.to_string())],
        ));
    }
    if regions.len() > 1 {
        fields.push(vel_i18n::format_named(
            &ui.regions,
            &[("count", regions.len().to_string())],
        ));
    }
    if selected > 0 {
        fields.push(vel_i18n::format_named(
            if selected == 1 {
                &ui.character
            } else {
                &ui.characters
            },
            &[("count", selected.to_string())],
        ));
    }
    if let Some(sum) = sum {
        fields.push(vel_i18n::format_named(
            &workspace.translations.tr("Sum: {sum}"),
            &[("sum", sum)],
        ));
    }
    fields.extend(encoding.into_iter().chain(endings).map(str::to_owned));
    fields.join(" · ")
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
/// Retained widget id of the Find in Files scope menu button.
const WHERE_ID: u64 = 61;
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
    SettingsLabel,
    SettingsInput,
    SideGuide,
    BuildInput,
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
/// Whether a match in a palette row's English caption (the alias of a translated row) reads
/// as the user's words: every run of matched characters starts a word — "save" in "File: Save
/// All", "enc" in "Save with Encoding", "utf8" in "UTF-8" — rather than letters scattered
/// through other words ("rust" in "Preferences: Customize Theme"). `positions` are the
/// matched characters' byte offsets in `label`, in order.
fn alias_matches_word_starts(label: &str, positions: &[usize]) -> bool {
    let word_start = |at: usize| {
        let Some(current) = label.get(at..).and_then(|rest| rest.chars().next()) else {
            return false;
        };
        label[..at].chars().next_back().is_none_or(|previous| {
            !previous.is_alphanumeric() || (previous.is_lowercase() && current.is_uppercase())
        })
    };
    let mut previous: Option<usize> = None;
    !positions.is_empty()
        && positions.iter().all(|&at| {
            // A run continues when this character directly follows the previous match.
            let continues = previous.is_some_and(|before| {
                label[before..]
                    .chars()
                    .next()
                    .is_some_and(|c| before + c.len_utf8() == at)
            });
            previous = Some(at);
            continues || word_start(at)
        })
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
    group: usize,
    groups: Vec<vel_ui::workbench::Group>,
    buffer: BufferId,
    version: vel_buffer::ChangeId,
    selection: Selection,
    scroll: (f64, f64),
    transient: bool,
    pending_center: bool,
    path: Option<Arc<PathBuf>>,
    position: vel_text::Point,
}

impl Preview {
    fn restore_groups(&self, workspace: &mut Workspace) {
        if self.groups.is_empty() {
            return;
        }
        workspace.workbench.active = self.group;
        for (group, original) in workspace.workbench.groups.iter_mut().zip(&self.groups) {
            // Right Arrow can open real tabs without dismissing Goto. Keep those
            // tabs and their order, but undo the preview's focus/selection changes.
            if group.sheets == original.sheets {
                group.clone_from(original);
            } else {
                group.selected.clone_from(&original.selected);
                group.selected.retain(|id| group.sheets.contains(id));
                group.focused = original.focused.filter(|id| group.sheets.contains(id));
                group.anchor = original.anchor.filter(|id| group.sheets.contains(id));
                group.tab_scroll = original.tab_scroll;
                if let Some(focus) = group.focused {
                    group.selected.insert(focus);
                    group.touch_focus();
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AcceptTabs {
    Default,
    Add,
    Right,
    ReplaceFocus,
}

impl AcceptTabs {
    fn from_modifiers(primary: bool, shift: bool, option: bool) -> Self {
        if option {
            Self::ReplaceFocus
        } else if primary {
            Self::Right
        } else if shift {
            Self::Add
        } else {
            Self::Default
        }
    }
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
    highlight_size_limit: usize,
}
#[derive(Debug)]
struct DocumentReplacement {
    task: text::ReplacementTask,
    template: String,
    only: Option<Range<usize>>,
    close: bool,
}
#[derive(Debug)]
struct GotoHighlights {
    snapshot: FindSnapshot,
    query: vel_buffer::ChangeId,
    ranges: Vec<Range<usize>>,
    accepted: bool,
}
#[derive(Debug, PartialEq, Eq)]
struct FilesIdentity {
    query: String,
    options: text::Options,
    where_: String,
    use_gitignore: bool,
    max_file_size: u64,
}

#[derive(Debug)]
struct ResultBudget {
    remaining: usize,
    truncated: bool,
    shown_paths: BTreeMap<Arc<Path>, bool>,
    hidden_matches: BTreeSet<Arc<Path>>,
}
impl ResultBudget {
    fn new(remaining: usize) -> Option<Self> {
        (remaining != 0).then(|| Self {
            remaining,
            truncated: false,
            shown_paths: BTreeMap::new(),
            hidden_matches: BTreeSet::new(),
        })
    }
    fn allows(&mut self, line: &files::Line) -> bool {
        if !self.truncated {
            let new_file = !self.shown_paths.contains_key(&line.path);
            // Charge a possible context gap and the displayed file header as
            // well as the row. Search/footer chrome is outside the budget.
            let bytes = line
                .formatted()
                .len()
                .saturating_add(12)
                .saturating_add(if new_file {
                    line.path.to_string_lossy().len() + 3
                } else {
                    0
                });
            if let Some(remaining) = self.remaining.checked_sub(bytes) {
                self.remaining = remaining;
                self.shown_paths
                    .entry(line.path.clone())
                    .and_modify(|matched| {
                        *matched |= line.match_count > 0;
                    })
                    .or_insert(line.match_count > 0);
                return true;
            }
            self.truncated = true;
        }
        if line.match_count > 0 {
            self.hidden_matches.insert(line.path.clone());
        }
        false
    }
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

fn each_goto_word(text: &vel_text::Text, mut visit: impl FnMut(&str, usize)) {
    for (row, line) in text.lines().enumerate() {
        let offset = text.line_to_byte(row);
        let text = line.to_string();
        let mut start = None;
        for (i, c) in text
            .char_indices()
            .chain(std::iter::once((text.len(), ' ')))
        {
            if c.is_alphanumeric() || c == '_' {
                start.get_or_insert(i);
            } else if let Some(begin) = start.take() {
                visit(&text[begin..i], offset + begin);
            }
        }
    }
}
/// Complete retained widget/controller state. All methods can run without a window server.
#[derive(Debug)]
pub struct Panels {
    pub(crate) find_pasteboard: Option<Box<dyn vel_platform::FindPasteboard>>,
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
    message_seen: UiText,
    message_until: Option<Instant>,
    pub search_results_active: bool,
    result_cursor: Option<usize>,
    result_budget: Option<ResultBudget>,
    result_error: Option<UiText>,
    files_searched: usize,
    file_matches: usize,
    result_status: UiText,
    results_tab: Option<find_results::ResultsTab>,
    result_destination: Option<u64>,
    result_untitled: BTreeMap<PathBuf, u64>,
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
    pub(crate) sidebar_on_right: bool,
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
    pub alternate_click: bool,
    pub build_results: Vec<(usize, FileLocation)>,
    pub build_output_offset: usize,
    pub(crate) build_input: Option<Input>,
    pub(crate) build_input_echo: Vec<Range<usize>>,
    pub find_history: Vec<String>,
    pub replace_history: Vec<String>,
    pub chrome: bool,
    pub(crate) chrome_auto_hidden: bool,
    choice_items: Vec<vel_config::ui::PaletteItem>,
    side_anchor: Option<PathBuf>,
    pub dragging_tab: Option<usize>,
    pub options: text::Options,
    files_use_gitignore: bool,
    files_use_buffer: bool,
    files_context: bool,
    pub in_selection: bool,
    pub highlights: Vec<Range<usize>>,
    pub message: UiText,
    pub result_view: files::Results,
    pub result_selected: usize,
    result_navigated: bool,
    pub result_scroll: Scroll,
    pub file_task: Option<files::Task>,
    pub file_complete: bool,
    file_signature: Option<FilesIdentity>,
    replace_rx: Option<mpsc::Receiver<Result<Vec<files::FileChange>, String>>>,
    replace_signature: Option<(FilesIdentity, String)>,
    pub replacement_plan: Option<Vec<files::FileChange>>,
    pub output: String,
    pub(crate) indexing_output: bool,
    index_labels_revision: Option<u64>,
    output_ui: Vec<(usize, UiText)>,
    output_ui_revision: Option<u64>,
    show_console: bool,
    console_messages: std::collections::VecDeque<String>,
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
    pub input_caption: UiText,
    /// The input panel caption's measured width in DIPs, from the last paint (see
    /// `fit_input_caption`); `None` until it has been measured.
    input_caption_advance: Option<f64>,
    find_sizing: details::FindSizing,
    find_measurement: Option<(Panel, f64, u64, u64)>,
    pub input_done: Option<Invocation>,
    focus_input: usize,
    selection_scope: Vec<Range<usize>>,
    scope_revision: Option<(BufferId, vel_buffer::ChangeId)>,
    scope_selection: Option<(u64, Selection)>,
    scope_manual: bool,
    search: Option<Search>,
    find_error: Option<UiText>,
    find_status: UiText,
    search_worker: text::Worker,
    search_identity: Option<FindIdentity>,
    all_matches_ready: bool,
    find_deferred: bool,
    pending_navigation: VecDeque<FindNavigation>,
    document_replacement: Option<Box<DocumentReplacement>>,
    fuzzy_worker: fuzzy::Worker,
    candidates: Arc<Vec<fuzzy::Candidate>>,
    palette_indices: Vec<usize>,
    // For a bilingual palette, each searchable caption points to its displayed
    // (primary) candidate. Empty for all other overlays.
    palette_primary: Vec<usize>,
    paths: Arc<Vec<PathBuf>>,
    file_paths: Arc<Vec<PathBuf>>,
    file_candidates: Arc<Vec<fuzzy::Candidate>>,
    goto_filter: Option<goto_files::Filter>,
    index: Option<Index>,
    folder_key: Vec<Folder>,
    startup_index_pending: bool,
    extra_paths: Vec<PathBuf>,
    extra_candidates: Arc<Vec<fuzzy::Candidate>>,
    file_recency: BTreeMap<usize, u64>,
    palette_recent: HashMap<String, u64>,
    uses: u64,
    preview: Option<Preview>,
    last_preview: Option<(PathBuf, Target)>,
    last_location_preview: Option<vel_index::SymbolLocation>,
    goto_highlights: Option<GotoHighlights>,
    tab_dropdown_anchor: Option<Rect>,
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
            message_seen: UiText::default(),
            message_until: None,
            search_results_active: false,
            result_cursor: None,
            result_budget: None,
            result_error: None,
            find_pasteboard: None,
            files_searched: 0,
            file_matches: 0,
            result_status: UiText::default(),
            results_tab: None,
            result_destination: None,
            result_untitled: BTreeMap::new(),
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
            sidebar_on_right: false,
            side_focused: false,
            open_files_visible: false,
            side_selected: 0,
            side_selection: None,
            sidebar_reveal: None,
            sidebar_aliases: BTreeMap::new(),
            side_open: None,
            side_scroll: Scroll::default(),
            side_rows: vec![],
            tree_motion: None,
            primary_click: false,
            alternate_click: false,
            build_results: Vec::new(),
            build_input: None,
            build_input_echo: Vec::new(),
            build_output_offset: 0,
            find_history: Vec::new(),
            replace_history: Vec::new(),
            chrome: true,
            chrome_auto_hidden: false,
            choice_items: Vec::new(),
            side_anchor: None,
            dragging_tab: None,
            result_view: files::Results::default(),
            quick_pending: false,
            options: text::Options::default(),
            files_use_gitignore: true,
            files_use_buffer: true,
            files_context: true,
            in_selection: false,
            highlights: vec![],
            message: UiText::default(),
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
            indexing_output: false,
            index_labels_revision: None,
            output_ui: Vec::new(),
            output_ui_revision: None,
            show_console: false,
            console_messages: std::collections::VecDeque::new(),
            output_editor: None,
            output_view: crate::view::View::new(Size::default(), 7.8, 12.),
            output_reset: false,
            output_trim: None,
            find_layout: None,
            quick_tight: false,
            status_items: None,
            tooltip_key: 0,
            output_mirrored: 0,
            input_caption: UiText::default(),
            input_caption_advance: None,
            find_sizing: details::FindSizing::default(),
            find_measurement: None,
            input_done: None,
            focus_input: 0,
            selection_scope: vec![],
            scope_revision: None,
            scope_selection: None,
            scope_manual: false,
            search: None,
            find_error: None,
            find_status: UiText::default(),
            search_worker: text::Worker::default(),
            search_identity: None,
            all_matches_ready: false,
            find_deferred: false,
            pending_navigation: VecDeque::new(),
            document_replacement: None,
            fuzzy_worker: fuzzy::Worker::new(),
            candidates: Arc::new(vec![]),
            palette_indices: Vec::new(),
            palette_primary: Vec::new(),
            paths: Arc::new(vec![]),
            file_paths: Arc::new(vec![]),
            file_candidates: Arc::new(vec![]),
            goto_filter: None,
            index: None,
            folder_key: Vec::new(),
            startup_index_pending: false,
            extra_paths: Vec::new(),
            extra_candidates: Arc::default(),
            file_recency: BTreeMap::new(),
            palette_recent: HashMap::new(),
            uses: 0,
            preview: None,
            last_preview: None,
            last_location_preview: None,
            goto_highlights: None,
            tab_dropdown_anchor: None,
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
    pub(crate) fn reset(&mut self, workspace: &Workspace) {
        let find_pasteboard = self.find_pasteboard.take();
        *self = Self::new(workspace, true);
        self.find_pasteboard = find_pasteboard;
    }
    pub fn new(workspace: &Workspace, workers: bool) -> Self {
        let mut ui = Self {
            ui_scale: vel_ui::theme::ui_scale(&workspace.editor().settings),
            chrome: workspace.window_visible("show_status_bar"),
            side_visible: workspace.window_visible("show_side_bar"),
            startup_index_pending: workspace.startup_services_pending(),
            ..Self::default()
        };
        if workers {
            ui.restart_index(workspace);
        }
        ui
    }
    pub(crate) fn start_startup_workers(&mut self, workspace: &Workspace) {
        if std::mem::take(&mut self.startup_index_pending) {
            self.restart_index(workspace);
        }
    }
    fn restart_index(&mut self, workspace: &Workspace) {
        if self.startup_index_pending {
            self.rebuild_side(workspace);
            return;
        }
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
        self.goto_filter = None;
        self.prepared.clear();
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
        self.show_console = false;
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
                self.message = e.into();
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
    pub(crate) fn clear_output(&mut self) {
        self.output.clear();
        self.build_input_echo.clear();
        self.output_ui.clear();
        self.indexing_output = false;
        self.output_reset = true;
    }
    pub(crate) fn set_build_input(&mut self, enabled: bool) {
        self.build_input = enabled.then(|| Input::bounded_line(vel_build::MAX_INPUT_LINE));
        if self.panel == Some(Panel::Output) && !self.show_console {
            if enabled {
                self.panel_focused = true;
                self.side_focused = false;
                self.focus.show(Surface::Panel, true);
            } else if self.panel_focused {
                self.focus_editor_for_context();
            }
        }
    }
    pub(crate) fn echo_build_input(&mut self, line: &str) {
        let start = self.output.len();
        self.build_input_echo.push(start..start + line.len());
        self.output.push_str(line);
    }
    fn show_build_input(&self) -> bool {
        self.panel == Some(Panel::Output) && !self.show_console && self.build_input.is_some()
    }
    pub(crate) fn append_output_ui(&mut self, message: UiText) {
        if !message.is_empty() {
            self.output_ui.push((self.output.len(), message.clone()));
            self.output.push_str(&message);
            self.output_ui_revision = None;
        }
    }
    pub(crate) fn replace_output_prefix(&mut self, removed: usize, message: UiText) {
        self.build_input_echo.retain_mut(|range| {
            if range.end <= removed {
                return false;
            }
            range.start = range.start.saturating_sub(removed) + message.len();
            range.end = range.end - removed + message.len();
            true
        });
        self.output_ui.retain_mut(|(start, text)| {
            if *start < removed {
                return false;
            }
            *start = *start - removed + message.len();
            !text.is_empty()
        });
        self.output.replace_range(..removed, &message);
        self.trim_output_prefix(removed, message.len());
        self.output_ui.insert(0, (0, message));
        self.output_ui_revision = None;
    }
    pub(super) fn refresh_output_locale(&mut self, tr: &vel_i18n::Translator) {
        if self.index_labels_revision != Some(tr.revision()) {
            self.intel.status_source.lock().unwrap().labels = crate::ui_text::IndexLabels::new(tr);
            self.index_labels_revision = Some(tr.revision());
            if self.indexing_output {
                self.refresh_index_report();
            }
        }
        if self.output_ui.is_empty() || self.output_ui_revision == Some(tr.revision()) {
            return;
        }
        self.output_ui_revision = Some(tr.revision());
        let mut shift = 0isize;
        for (start, text) in &mut self.output_ui {
            *start = start.saturating_add_signed(shift);
            let previous_len = text.len();
            if text.refresh(tr) {
                self.output
                    .replace_range(*start..*start + previous_len, text);
                let delta = text.len() as isize - previous_len as isize;
                for range in &mut self.build_input_echo {
                    if range.start >= *start + previous_len {
                        range.start = range.start.saturating_add_signed(delta);
                        range.end = range.end.saturating_add_signed(delta);
                    }
                }
                shift += delta;
                if *start < self.output_mirrored {
                    self.output_reset = true;
                }
            }
        }
    }
    fn refresh_index_report(&mut self) {
        self.output = crate::intelligence::status_text_localized(
            &self
                .intel
                .projects
                .iter()
                .map(|p| p.index.clone())
                .collect::<Vec<_>>(),
            &self.intel.status_source.lock().unwrap().labels,
        );
        self.output_reset = true;
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
    pub fn tab_dropdown(&mut self, workspace: &mut Workspace, group: usize, rect: Rect) {
        let items = workspace.workbench.groups[group]
            .sheets
            .iter()
            .filter_map(|&sheet| {
                let tab = &workspace.tabs[workspace.sheet_index(sheet)?];
                Some(vel_config::ui::PaletteItem {
                    caption: tab.ui_name(&workspace.ui),
                    invocation: Invocation {
                        command: "select_by_index".into(),
                        args: json!({"_dropdown_sheet": sheet}),
                    },
                })
            })
            .collect();
        self.choices(workspace, items);
        self.tab_dropdown_anchor = Some(rect);
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
        window.sidebar_open_files = Some(self.open_files_visible);
        window.sidebar_expanded = self.expanded.iter().cloned().collect();
        window.sidebar_expanded.sort_unstable();
        window.sidebar_scroll = self.side_scroll.offset;
        window.find_history = self.find_history.clone();
        window.replace_history = self.replace_history.clone();
        window.find_in_files_use_gitignore = Some(self.files_use_gitignore);
        window.find_in_files_use_buffer = Some(self.files_use_buffer);
        window.find_in_files_context = Some(self.files_context);
    }
    pub fn restore_session(&mut self, workspace: &Workspace, window: &vel_session::Window) {
        self.open_files_visible = window.sidebar_open_files.unwrap_or_default();
        self.expanded = window.sidebar_expanded.iter().cloned().collect();
        self.watched_expanded = false;
        self.side_scroll.offset = window.sidebar_scroll;
        self.find_history = window.find_history.clone();
        self.replace_history = window.replace_history.clone();
        self.files_use_gitignore = window.find_in_files_use_gitignore.unwrap_or(true);
        self.files_use_buffer = window.find_in_files_use_buffer.unwrap_or(true);
        self.files_context = window.find_in_files_context.unwrap_or(true);
        if let Some(value) = self.find_history.last() {
            self.query.set(value);
        }
        if let Some(value) = self.replace_history.last() {
            self.replace.set(value);
        }
        if self.startup_index_pending {
            self.rebuild_side(workspace);
        } else {
            for path in &window.sidebar_expanded {
                self.load_directory(workspace, path);
            }
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
        if self.startup_index_pending {
            return;
        }
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
        let files = self.visible_open_files(workspace);
        files + 1 + usize::from(files != 0)
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
                Some('/') => UiText::tr("Search forward /"),
                Some('?') => UiText::tr("Search backward ?"),
                _ => ":".into(),
            };
            self.show_input_caption(
                workspace,
                "show_input_panel",
                &json!({
                    "initial_text": &prompt[1..],
                    "on_done": {"command":"vintage_submit_prompt", "args":{}}
                }),
                caption,
                viewport,
            )?;
            self.vintage_prompt = Some(sheet);
        }
        Ok(())
    }
    pub(crate) fn show_input_caption(
        &mut self,
        workspace: &mut Workspace,
        command: &str,
        args: &Value,
        mut caption: UiText,
        viewport: Range<usize>,
    ) -> Result<bool, String> {
        debug_assert_eq!(command, "show_input_panel");
        let handled = self.action(workspace, command, args, viewport)?;
        caption.refresh(&workspace.translations);
        self.input_caption = caption;
        Ok(handled)
    }
    pub fn input(&self) -> Option<&Input> {
        if self.overlay.is_some() {
            Some(&self.quick_input)
        } else if self.panel_focused && self.show_build_input() {
            self.build_input.as_ref()
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
        } else if self.panel_focused && self.show_build_input() {
            self.build_input.as_mut()
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
    pub(crate) fn focus_context_input(&mut self, point: Point) -> bool {
        if self.overlay.is_some() {
            return contains(self.quick_input.bounds, point);
        }
        if self.show_build_input()
            && self
                .build_input
                .as_ref()
                .is_some_and(|input| contains(input.bounds, point))
        {
            self.panel_focused = true;
            self.side_focused = false;
            self.focus.show(Surface::Panel, true);
            return true;
        }
        let Some(panel) = self.panel else {
            return false;
        };
        let Some(index) = [&self.query, &self.replace, &self.where_input]
            .iter()
            .take(Self::find_fields(panel))
            .position(|input| contains(input.bounds, point))
        else {
            return false;
        };
        self.focus_input = index;
        self.panel_focused = true;
        self.side_focused = false;
        self.focus.set(Surface::Panel, 2);
        true
    }
    pub(crate) fn refresh_find_pasteboard(&mut self, workspace: &mut Workspace) {
        if self.overlay.is_none()
            && self.panel_focused
            && self.focus_input == 0
            && self.document_find_visible()
            && let Some(pattern) = self.find_pasteboard_pattern(workspace)
            && self.query.value() != pattern
        {
            self.query.set(&pattern);
            self.refresh_find(workspace, 0..0);
        }
    }

    fn find_pasteboard_pattern(&self, workspace: &Workspace) -> Option<String> {
        if !workspace.editor().settings.bool("use_find_clipboard", true) {
            return None;
        }
        self.find_pasteboard
            .as_ref()?
            .read()
            .filter(|s| !s.is_empty())
    }
    pub(crate) fn widget_history_menu(
        &self,
        tr: &vel_i18n::Translator,
    ) -> Vec<vel_config::ui::PaletteItem> {
        // Use the same history as Up/Down; the Where and overlay fields have no
        // history in Velocity. https://www.sublimetext.com/docs/menus.html
        if self.overlay.is_some() || !self.find_panel_visible() || self.focus_input > 1 {
            return Vec::new();
        }
        let history = if self.focus_input == 0 {
            &self.find_history
        } else {
            &self.replace_history
        };
        history
            .iter()
            .rev()
            .take(20)
            .map(|value| vel_config::ui::PaletteItem {
                caption: if value.is_empty() {
                    tr.tr("Empty Replacement").into_owned()
                } else {
                    value.replace('\n', " ↵ ")
                },
                invocation: vel_config::keymap::Invocation {
                    command: "widget_history".into(),
                    args: json!({"text":value,"field":self.focus_input}),
                },
            })
            .collect()
    }
    pub(crate) fn restore_widget_history(&mut self, args: &Value) -> bool {
        if self.overlay.is_some()
            || !self.find_panel_visible()
            || args["field"].as_u64() != Some(self.focus_input as u64)
        {
            return false;
        }
        if let Some(input) = self.input_mut() {
            input.set(args["text"].as_str().unwrap_or_default());
            return true;
        }
        false
    }
    pub(crate) fn console_visible(&self) -> bool {
        self.show_console && self.panel == Some(Panel::Output)
    }
    pub(crate) fn toggle_console(&mut self, workspace: &mut Workspace) {
        if self.console_visible() {
            self.close_panel(workspace);
            return;
        }
        self.cancel_overlay(workspace);
        self.finish_find_navigation(workspace);
        let message = self.message.clone();
        self.note_console(workspace, &message);
        self.show_console = true;
        self.panel = Some(Panel::Output);
        self.panel_focused = true;
        self.focus.show(Surface::Panel, true);
        self.focus.set(Surface::Panel, 2);
    }
    fn console_limit(workspace: &Workspace) -> usize {
        workspace.settings.usize("console_max_history_lines", 80)
    }
    #[cfg(test)]
    pub(crate) fn console_history(&self) -> &std::collections::VecDeque<String> {
        &self.console_messages
    }
    pub(crate) fn note_console(&mut self, workspace: &Workspace, message: &str) {
        // Diagnostic console only; Velocity does not expose a Python command console.
        let limit = Self::console_limit(workspace);
        let history = &mut self.console_messages;
        history.drain(..history.len().saturating_sub(limit));
        if limit == 0 {
            return;
        }
        for line in message.lines().filter(|line| !line.is_empty()) {
            let line: String = line.chars().take(2048).collect();
            if history.back() != Some(&line) {
                if history.len() == limit {
                    history.pop_front();
                }
                history.push_back(line);
            }
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
        } else if self.show_build_input() {
            if let Some(input) = &mut self.build_input {
                input.reveal();
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
            && (self.panel == Some(Panel::Incremental)
                || self
                    .search_identity
                    .as_ref()
                    .is_none_or(|key| !Self::over_highlight_limit(key, workspace)))
        {
            let origin = self
                .find_origin
                .filter(|(id, _)| *id == workspace.editor().buffer.id())
                .map_or(workspace.editor().caret, |(_, at)| at);
            self.queue_find(workspace, FindNavigation::Preview(origin));
        }
    }
    fn set_query_from_buffer(&mut self, text: &str, escape_regex: bool) {
        // https://www.sublimetext.com/download (escape patterns taken from a file).
        if self.options.regex && escape_regex {
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
            || self.document_replacement.is_some()
            || self
                .output_editor
                .as_ref()
                .is_some_and(|e| e.language.borrow().as_ref().is_some_and(|l| l.busy()))
            || self.output_pending()
            || (self.search.is_some() && !self.all_matches_ready && !self.find_deferred)
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
                item.icon = quick_icons::symbol(location.kind);
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
                        icon: quick_icons::symbol(l.kind),
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
    #[cfg(test)]
    pub(crate) fn has_pending_definition_hover(&self) -> bool {
        !self.hover_term.is_empty()
    }
    #[cfg(test)]
    pub(crate) fn sidebar_animates(&self) -> bool {
        self.tree_motion.is_some()
    }
    #[cfg(test)]
    pub(crate) fn sidebar_has_row(&self, path: &Path) -> bool {
        self.side_rows.iter().any(|row| row.path == path)
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
                crate::intelligence::jump_with_options(workspace, &location, Default::default())?;
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
            self.set_message(UiText::tr(
                "Text search is unavailable in hex and image tabs.",
            ));
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
            self.action(workspace, "show_input_panel", &json!({"initial_text":format!("0x{:x}", workspace.editor().caret),"on_done":{"command":"goto_hex_offset","args":{}}}), viewport)?;
            self.input_caption = UiText::tr("Goto byte offset (decimal or 0x hex)");
            return Ok(true);
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
                        reference: name == "goto_reference",
                    };
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
                self.clear_output();
                self.indexing_output = true;
                self.refresh_index_report();
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
                    group: workspace.workbench.active,
                    groups: if self.overlay == Some(Overlay::Goto) {
                        workspace.workbench.groups.clone()
                    } else {
                        Vec::new()
                    },
                    buffer: workspace.editor().buffer.id(),
                    version: workspace.editor().buffer.change_id(),
                    scroll: workspace.tabs[workspace.active].scroll,
                    transient: workspace.tabs[workspace.active].transient,
                    pending_center: workspace.tabs[workspace.active].pending_center,
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
                if self.overlay == Some(Overlay::Goto) {
                    self.navigation_preview_guard = Some(
                        crate::editor::navigation::Guard::suppress(&workspace.jump_history),
                    );
                } else {
                    workspace.tabs[workspace.active].transient = false;
                }
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
                self.last_location_preview = None;
                self.preview_symbol_version = None;
                self.quick.set_items(Vec::new());
                self.prepared.clear();
                self.query_quick(workspace);
            }
            "show_panel" | "find_in_files" | "show_input_panel" | "show_output_panel" => {
                self.show_console = false;
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
                if self.panel == Some(Panel::Files)
                    && let Some(use_gitignore) = args["use_gitignore"].as_bool()
                {
                    self.set_files_gitignore(use_gitignore);
                }
                if self.panel == Some(Panel::Files) {
                    if let Some(use_buffer) = args["use_buffer"].as_bool() {
                        self.files_use_buffer = use_buffer;
                    }
                    if let Some(context) = args["context"].as_bool() {
                        self.files_context = context;
                    }
                }
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
                            group: workspace.workbench.active,
                            groups: Vec::new(),
                            buffer: editor.buffer.id(),
                            version: editor.buffer.change_id(),
                            selection: editor.buffer.view(editor.id).sel().clone(),
                            scroll: workspace.tabs[workspace.active].scroll,
                            transient: workspace.tabs[workspace.active].transient,
                            pending_center: workspace.tabs[workspace.active].pending_center,
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
                    self.input_caption = args["caption"]
                        .as_str()
                        .map_or_else(|| UiText::tr("Input"), UiText::from);
                    self.query.set(args["initial_text"].as_str().unwrap_or(""));
                    self.input_done = args
                        .get("on_done")
                        .cloned()
                        .map(serde_json::from_value)
                        .transpose()
                        .map_err(|e| e.to_string())?;
                } else if name == "show_output_panel" {
                    self.clear_output();
                    self.output = args["text"].as_str().unwrap_or("").into();
                } else {
                    // https://www.sublimetext.com/download (show_panel arguments).
                    if let Some(query) = args["pattern"].as_str().or(args["text"].as_str()) {
                        self.query.set(query);
                    } else {
                        let editor = workspace.editor();
                        let selected = (editor.binary.is_none()
                            && editor.settings.bool("find_selected_text", true))
                        .then(|| editor.text.slice(editor.selection()));
                        // Selection seeding stays single-line and takes precedence.
                        if let Some(selected) = selected
                            && !selected.is_empty()
                            && !selected.chars().any(|c| matches!(c, '\n' | '\r'))
                        {
                            self.set_query_from_buffer(
                                &selected.to_string(),
                                editor.settings.bool("regex_auto_escape", true),
                            );
                        } else if self.document_find_visible()
                            && let Some(pattern) = self.find_pasteboard_pattern(workspace)
                        {
                            self.query.set(&pattern);
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
            "cancel_find_in_files" => self.cancel_files(workspace),
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
                    "use_buffer" if self.panel == Some(Panel::Files) => {
                        self.files_use_buffer = !self.files_use_buffer;
                    }
                    "context" if self.panel == Some(Panel::Files) => {
                        self.files_context = !self.files_context;
                    }
                    "use_gitignore" if self.panel == Some(Panel::Files) => {
                        self.set_files_gitignore(!self.files_use_gitignore);
                    }
                    "in_selection" => {
                        if !self.document_find_visible() {
                            // From the palette, open Find and retain the editor's scope.
                            self.action(
                                workspace,
                                "show_panel",
                                &json!({"panel": "find"}),
                                viewport.clone(),
                            )?;
                            self.in_selection = true;
                        } else {
                            self.sync_find_scope(workspace);
                            self.in_selection = !self.in_selection;
                        }
                        self.scope_manual = true;
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
                    if name == "slurp_find_string"
                        && !value.is_empty()
                        && editor.settings.bool("use_find_clipboard", true)
                        && let Some(board) = &self.find_pasteboard
                    {
                        board.write(&value);
                    }
                    self.set_query_from_buffer(
                        &value,
                        editor.settings.bool("regex_auto_escape", true),
                    );
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
                let all = name == "replace_all";
                let close = all
                    && args["_close_panel"].as_bool().unwrap_or_else(|| {
                        workspace
                            .editor()
                            .settings
                            .bool("close_find_after_replace_all", true)
                    });
                self.replace_document(workspace, all, close, viewport)?;
                if close && self.find_error.is_none() && self.document_replacement.is_none() {
                    self.close_panel(workspace);
                }
            }
            "find_in_files_start" => {
                self.start_files(workspace)?;
            }
            "find_in_files_where" => {
                let scope = match args["scope"].as_str() {
                    Some("current_file") => "<current file>",
                    Some("open_files") => "<open files>",
                    Some("open_folders") => "<open folders>",
                    _ => return Err("Unknown Find in Files scope".into()),
                };
                self.where_input.set(scope);
                self.input_changed(workspace, viewport);
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
    fn find_options_rect(&self) -> Rect {
        let close = self.close_button_rect();
        Rect {
            x: close.x - close.width - 4. * self.ui_scale,
            ..close
        }
    }
    fn show_find_options_button(&self) -> bool {
        self.panel == Some(Panel::Files) && self.panel_bounds.width >= 272. * self.ui_scale
    }
    pub(crate) fn hit_find_options(&self, point: Point) -> bool {
        self.overlay.is_none()
            && self.show_find_options_button()
            && contains(self.find_options_rect(), point)
    }
    pub(crate) fn find_option_checked(&self, option: &str) -> bool {
        match option {
            "regex" => self.options.regex,
            "case_sensitive" => self.options.case_sensitive,
            "whole_word" => self.options.whole_word,
            "wrap" => self.options.wrap,
            "in_selection" => self.in_selection,
            "preserve_case" => self.options.preserve_case,
            "use_buffer" => self.files_use_buffer,
            "context" => self.files_context,
            "use_gitignore" => self.files_use_gitignore,
            _ => false,
        }
    }
    fn set_files_gitignore(&mut self, use_gitignore: bool) {
        if self.files_use_gitignore != use_gitignore {
            self.files_use_gitignore = use_gitignore;
            self.replace_rx = None;
            self.replace_signature = None;
            self.replacement_plan = None;
        }
    }
    pub(crate) fn can_find_in_selection(&self, workspace: &Workspace) -> bool {
        let tab = &workspace.tabs[workspace.active];
        let editor = &tab.editor;
        if editor.binary.is_some() || self.panel == Some(Panel::Files) {
            return false;
        }
        let current = editor.buffer.view(editor.id).sel();
        if self.document_find_visible()
            && self
                .scope_selection
                .as_ref()
                .is_some_and(|(sheet, remembered)| {
                    *sheet == tab.sheet
                        && (remembered.same_snapshot(current) || remembered == current)
                })
        {
            // A match selected by Find itself is not an editor selection scope.
            return self.in_selection || !self.selection_scope.is_empty();
        }
        current.iter().any(|region| !region.empty())
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
        if self.overlay == Some(Overlay::Goto)
            && let Some(highlights) = &self.goto_highlights
            && highlights.query == self.quick_input.buffer.change_id()
            && highlights.snapshot == FindSnapshot::current(workspace)
        {
            return &highlights.ranges;
        }
        if self.search_identity.as_ref().is_some_and(|key| {
            key.snapshot == FindSnapshot::current(workspace)
                && !Self::over_highlight_limit(key, workspace)
        }) {
            self.visible_highlights()
        } else {
            &[]
        }
    }
    pub(crate) fn scrollbar_find_highlights(&self, workspace: &Workspace) -> &[Range<usize>] {
        if self.overlay.is_some()
            || !workspace
                .editor()
                .settings
                .bool("highlight_find_results_in_scrollbar", false)
        {
            return &[];
        }
        let highlights = self.current_highlights(workspace);
        let limit = workspace
            .editor()
            .settings
            .usize("find_scroll_highlights_limit", 4096);
        // A dense result should not leave an apparently complete, partial map.
        if limit != 0 && highlights.len() > limit {
            &[]
        } else {
            highlights
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
        self.show_console = false;
        self.finish_find_navigation(workspace);
        if matches!(self.panel, Some(Panel::Files | Panel::Results)) {
            self.cancel_files(workspace);
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
        self.document_replacement = None;
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
        let settings = &workspace.editor().settings;
        FindIdentity {
            snapshot: FindSnapshot::current(workspace),
            query: self.query.value(),
            options: self.options,
            in_selection: self.in_selection,
            highlight_size_limit: if self.options.regex {
                settings.usize("find_regex_highlight_matches_max_size", 0)
            } else {
                settings.usize("find_highlight_matches_max_size", 0)
            },
        }
    }
    fn over_highlight_limit(key: &FindIdentity, workspace: &Workspace) -> bool {
        key.highlight_size_limit != 0
            && workspace.editor().text.len_bytes() > key.highlight_size_limit
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
        self.document_replacement = None;
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
        let limited = Self::over_highlight_limit(&key, workspace);
        let result = (!snapshot.binary).then(|| Search::new(&key.query, key.options));
        self.search_identity = Some(key);
        if self.find_error.take().as_deref() == Some(self.message.as_ref())
            || (!self.find_status.is_empty() && self.find_status == self.message)
        {
            self.set_message(String::new());
        }
        self.search_worker.cancel();
        self.highlights.clear();
        self.all_matches_ready = false;
        self.find_deferred = false;
        self.find_status.clear();
        match result {
            Some(Ok(search)) => {
                let editor = workspace.editor();
                if search.query.is_empty() || (self.in_selection && self.selection_scope.is_empty())
                {
                    self.all_matches_ready = true;
                } else if snapshot.loading {
                    self.find_status = UiText::tr("Waiting for file to load…");
                } else if limited && self.pending_navigation.is_empty() {
                    // On demand navigation still works; just don't start an
                    // all-match scan on every field edit.
                    self.find_deferred = true;
                } else if editor.text.len_bytes() <= 4096 && !search.options.regex {
                    // Keep tiny literal searches immediate. A viewport may include
                    // an arbitrarily long logical line; never scan it as a preview.
                    self.highlights = search.find(&editor.text, &self.scope());
                    self.all_matches_ready = true;
                } else {
                    self.find_status = UiText::tr("Searching…");
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
                let detail = if error.lines().last() == Some("error: unclosed group") {
                    UiText::tr("Unclosed group")
                } else {
                    error.into()
                };
                self.find_status =
                    UiText::compose("Invalid regular expression: {detail}", [("detail", detail)]);
                self.find_error = Some(self.find_status.clone());
                self.set_message(self.find_status.clone());
                self.search = None;
                self.all_matches_ready = true;
            }
            None => {
                self.pending_navigation.clear();
                self.search = None;
                self.all_matches_ready = true;
                self.find_status = UiText::tr("Text search is unavailable in hex and image tabs.");
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
            UiText::default()
        } else if self.highlights.is_empty() {
            let mut flags = Vec::new();
            if search.options.case_sensitive {
                flags.push(UiText::tr("Match Case on"));
            }
            if search.options.whole_word {
                flags.push(UiText::tr("Whole Word on"));
            }
            if self.in_selection {
                flags.push(if self.selection_scope.is_empty() {
                    UiText::tr("In Selection on; no selection")
                } else {
                    UiText::tr("In Selection on")
                });
            }
            if let Some(options) = flags.into_iter().reduce(|left, right| {
                UiText::compose("{left}, {right}", [("left", left), ("right", right)])
            }) {
                UiText::compose("No matches ({options})", [("options", options)])
            } else {
                UiText::tr("No matches")
            }
        } else {
            let n = self.highlights.len();
            if n == 1 {
                UiText::tr("1 match")
            } else {
                UiText::tr_fmt("{count} matches", [("count", n.to_string())])
            }
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
            && !search.uses_perl()
        {
            self.highlights = search.find(&workspace.editor().text, &self.scope());
            self.search_worker.cancel();
            self.all_matches_ready = true;
            self.update_find_status();
        }
        self.pending_navigation.push_back(action);
        if std::mem::take(&mut self.find_deferred)
            && !self.all_matches_ready
            && let Some(search) = &self.search
        {
            self.find_status = UiText::tr("Searching…");
            self.search_worker.search(
                search.clone(),
                workspace.editor().text.clone(),
                workspace.editor().revision,
                self.scope(),
            );
        }
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
                if report
                    && self.highlights.len() > 1
                    && let Ok(index) = self
                        .highlights
                        .binary_search_by_key(&workspace.editor().selection().start, |r| r.start)
                {
                    self.find_status = UiText::tr_fmt(
                        "{index} of {count}",
                        [
                            ("index", (index + 1).to_string()),
                            ("count", self.highlights.len().to_string()),
                        ],
                    );
                }
            } else if report && !search.query.is_empty() {
                self.update_find_status();
                if !self.highlights.is_empty() {
                    self.find_status = if backwards {
                        UiText::compose(
                            "No previous match ({summary})",
                            [("summary", self.find_status.clone())],
                        )
                    } else {
                        UiText::compose(
                            "No later match ({summary})",
                            [("summary", self.find_status.clone())],
                        )
                    };
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
        close: bool,
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
        if search.uses_perl() {
            if self.document_replacement.is_none() {
                let editor = workspace.editor();
                let only = (!all).then(|| editor.selection());
                let template = self.replace.value();
                self.document_replacement = Some(Box::new(DocumentReplacement {
                    task: text::ReplacementTask::start(
                        search.clone(),
                        editor.text.clone(),
                        template.clone(),
                        self.scope(),
                        only.clone(),
                    ),
                    only,
                    template,
                    close,
                }));
                self.set_message(UiText::tr("Preparing replacement preview…"));
            }
            return Ok(());
        }
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
        self.apply_document_replacements(workspace, replacements, all, viewport);
        Ok(())
    }
    fn apply_document_replacements(
        &mut self,
        workspace: &mut Workspace,
        replacements: Vec<text::Replacement>,
        all: bool,
        viewport: Range<usize>,
    ) {
        let count = replacements.len();
        let editor = workspace.editor_mut();
        text::apply(&mut editor.buffer, editor.id, &replacements);
        editor.sync();
        self.refresh_find(workspace, viewport);
        if !all {
            self.navigate_find(workspace, false, false, false);
        }
        self.set_message(if count == 1 {
            UiText::tr("Replaced 1 occurrence")
        } else {
            UiText::tr_fmt(
                "Replaced {count} occurrences",
                [("count", count.to_string())],
            )
        });
    }
    fn poll_document_replacement(
        &mut self,
        workspace: &mut Workspace,
        viewport: Range<usize>,
    ) -> bool {
        let Some(pending) = &self.document_replacement else {
            return false;
        };
        if self.search_identity.as_ref() != Some(&self.find_identity(workspace))
            || pending.template != self.replace.value()
            || pending
                .only
                .as_ref()
                .is_some_and(|range| *range != workspace.editor().selection())
            || workspace.editor().read_only
        {
            self.document_replacement = None;
            self.set_message(UiText::tr(
                "Replacement options changed; prepare the preview again",
            ));
            return true;
        }
        let Some(result) = pending.task.poll() else {
            return false;
        };
        let all = pending.only.is_none();
        let close = pending.close;
        self.document_replacement = None;
        match result {
            Ok(edits) if !all && edits.is_empty() => self.find_next(workspace, false, false),
            Ok(edits) => {
                self.apply_document_replacements(workspace, edits, all, viewport);
                if close {
                    self.close_panel(workspace);
                }
            }
            Err(error) => self.set_message(error),
        }
        true
    }
    pub(crate) fn refresh_localized_palette(&mut self, workspace: &mut Workspace) {
        match self.overlay {
            Some(Overlay::Palette | Overlay::Syntax | Overlay::Choices) => {
                self.prepared.clear();
                self.query_quick(workspace);
            }
            Some(Overlay::Appearance) => self.refresh_appearance_locale(workspace),
            Some(Overlay::Goto) if !self.local_positions.is_empty() => {
                for item in &mut self.quick.items {
                    if let Some(at) = self.local_positions.get(item.value) {
                        item.annotation = Self::local_annotation(workspace, *at);
                    }
                }
            }
            _ => {}
        }
    }
    fn local_annotation(workspace: &Workspace, at: usize) -> String {
        workspace.translations.tr_fmt(
            "Line {number}",
            &[(
                "number",
                (workspace.editor().text.byte_to_line(at) + 1).to_string(),
            )],
        )
    }
    fn palette_match_limit(&self) -> usize {
        if self.palette_primary.is_empty()
            || !matches!(
                self.overlay,
                Some(Overlay::Palette | Overlay::Syntax | Overlay::Choices)
            )
        {
            100
        } else if self.quick_query.trim().is_empty() {
            200
        } else {
            // The worker sorts by score. Keep all bilingual matches until the
            // palette has ranked match tiers and deduplicated the labels.
            self.candidates.len()
        }
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
        if target == "palette" {
            workspace.finish_startup_palette();
        }
        if matches!(target, "files" | "project_symbols") {
            self.start_startup_workers(workspace);
        }
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
            self.set_message(UiText::tr(
                "Text navigation is unavailable in hex and image tabs.",
            ));
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
                self.fuzzy_worker
                    .query(self.quick_input.value(), self.palette_match_limit());
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
            // Score both labels with the same worker and recency. Apply the
            // palette's alias ranking when publishing, not in the file matcher.
            let mut aliases = Vec::new();
            let mut candidates = Vec::with_capacity(self.palette_indices.len());
            for (primary, &index) in self.palette_indices.iter().enumerate() {
                let item = &palette[index];
                let caption = workspace.palette_caption(item);
                let recent = self
                    .palette_recent
                    .get(&format!(
                        "{}:{}",
                        item.invocation.command, item.invocation.args
                    ))
                    .copied()
                    .unwrap_or(0);
                let mut candidate =
                    fuzzy::Candidate::new(caption.as_deref().unwrap_or(&item.caption));
                candidate.recent = recent;
                candidates.push(candidate);
                if caption
                    .as_deref()
                    .is_some_and(|label| label != item.caption)
                {
                    let mut alias = fuzzy::Candidate::new(&item.caption);
                    alias.recent = recent;
                    aliases.push((primary, alias));
                }
            }
            self.palette_primary.clear();
            if !aliases.is_empty() {
                self.palette_primary.extend(0..candidates.len());
                for (primary, alias) in aliases {
                    self.palette_primary.push(primary);
                    candidates.push(alias);
                }
            }
            self.candidates = Arc::new(candidates);
            self.local_positions.clear();
            self.fuzzy_worker.set_items(self.candidates.clone());
            self.fuzzy_worker
                .query(self.quick_input.value(), self.palette_match_limit());
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
                        UiText::tr("Indexing symbols…")
                    } else {
                        UiText::default()
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
                        each_goto_word(&snapshot, |word, at| words.push((word.into(), at)));
                        let _ = tx.send(Local::Words(generation, words));
                    });
                    self.local = Some(rx);
                    return;
                }
                Target::File => {}
            }
        }
        let filter_gitignore = Self::goto_filters_gitignore(workspace);
        let source = if filter_gitignore {
            "git-files"
        } else {
            "files"
        };
        if self.prepared == source {
            self.fuzzy_worker.query(query.file, 100);
            return;
        }
        let (paths, candidates) = if filter_gitignore {
            let filter = self
                .goto_filter
                .get_or_insert_with(|| goto_files::Filter::start(workspace.folders.clone()));
            let Some((paths, candidates)) = &filter.files else {
                self.quick.set_items(Vec::new());
                self.quick_ready = false;
                return;
            };
            (paths.clone(), candidates.clone())
        } else {
            self.goto_filter = None;
            (self.file_paths.clone(), self.file_candidates.clone())
        };
        self.prepared = source.into();
        self.quick_ready = true;
        self.local_generation += 1;
        self.local = None;
        let mut extra = Vec::new();
        for tab in &workspace.tabs {
            if !tab.goto_preview
                && let Some(path) = &tab.path
                && paths.binary_search(path).is_err()
                && (!filter_gitignore
                    || !workspace
                        .folders
                        .iter()
                        .any(|folder| path.starts_with(&folder.path)))
                && !extra.contains(path)
            {
                extra.push(path.clone());
            }
        }
        let indexed = paths.len();
        self.paths = paths;
        self.local_positions.clear();
        self.candidates = candidates;
        self.extra_paths = extra;
        self.extra_candidates = Arc::new(
            self.extra_paths
                .iter()
                .map(|path| fuzzy::Candidate::new(vel_config::paths::display(path)))
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
    fn goto_path_detail(path: &Path, folders: &[Folder], home: Option<&Path>) -> String {
        if let Some((folder, relative)) = folders
            .iter()
            .filter_map(|f| {
                path.strip_prefix(&f.path)
                    .ok()
                    .map(|relative| (f, relative))
            })
            .max_by_key(|(f, _)| f.path.as_os_str().len())
        {
            // Sublime's public changelog documents the folder prefix for multiple roots.
            return if folders.len() > 1 {
                format!("{}/{}", folder.name, relative.display())
            } else {
                relative.display().to_string()
            };
        }
        if let Some(relative) = home
            .filter(|_| cfg!(unix))
            .and_then(|home| path.strip_prefix(home).ok())
        {
            return Path::new("~").join(relative).display().to_string();
        }
        vel_config::paths::display(path)
    }
    fn goto_filters_gitignore(workspace: &Workspace) -> bool {
        // A transient preview may change the active view's syntax and settings.
        !workspace.folders.is_empty()
            && workspace
                .settings
                .bool("goto_anything_exclude_gitignore", false)
    }
    fn publish_quick(&mut self, workspace: &Workspace, hits: Vec<fuzzy::Hit>) {
        let goto_files = self.overlay == Some(Overlay::Goto) && self.local_positions.is_empty();
        let home = goto_files
            .then(|| std::env::var_os("HOME").filter(|home| !home.is_empty()))
            .flatten();
        let palette = matches!(
            self.overlay,
            Some(Overlay::Palette | Overlay::Syntax | Overlay::Choices)
        );
        let palette_items = if self.overlay == Some(Overlay::Choices) {
            &self.choice_items
        } else {
            &workspace.palette
        };
        let bilingual = palette && !self.palette_primary.is_empty();
        let (preferred, scattered) = if bilingual && !self.quick_query.trim().is_empty() {
            // Preserve fuzzy order in each tier, before deduplication: a row
            // with a direct hit must not be consumed by a scattered alias.
            hits.into_iter().partition(|hit| {
                self.palette_primary.get(hit.index).is_some_and(|&shown| {
                    shown == hit.index
                        || alias_matches_word_starts(
                            &self.candidates[hit.index].label,
                            &hit.positions,
                        )
                })
            })
        } else {
            (hits, Vec::new())
        };
        let mut seen = BTreeSet::new();
        let items = preferred
            .into_iter()
            .map(|hit| (hit, true))
            .chain(scattered.into_iter().map(|hit| (hit, false)))
            .filter_map(|(hit, preferred)| {
                let shown = if bilingual {
                    let shown = *self.palette_primary.get(hit.index)?;
                    if !seen.insert(shown) {
                        return None;
                    }
                    shown
                } else {
                    hit.index
                };
                let index = if palette {
                    *self.palette_indices.get(shown)?
                } else {
                    hit.index
                };
                let caption = &self
                    .candidates
                    .get(shown)
                    .unwrap_or_else(|| &self.extra_candidates[hit.index - self.candidates.len()])
                    .label;
                Some(Item {
                    caption: caption.clone(),
                    // An alias match applies to the whole shown label (its byte positions
                    // are not offsets into the translation), but only when it reads as the
                    // user's words; letters scattered through other words highlight nothing
                    // rather than a whole row that looks like the best match.
                    highlights: if shown == hit.index {
                        hit.positions
                    } else if preferred && !hit.positions.is_empty() {
                        caption.char_indices().map(|(byte, _)| byte).collect()
                    } else {
                        Vec::new()
                    },
                    value: index,
                    icon: if palette {
                        quick_icons::command(&palette_items[index])
                    } else if let Some(kind) = self
                        .local_kinds
                        .get(hit.index)
                        .filter(|_| !self.local_positions.is_empty())
                    {
                        quick_icons::symbol(*kind)
                    } else if !self.local_positions.is_empty() {
                        ItemIcon::Neutral
                    } else {
                        ItemIcon::File
                    },
                    annotation: if palette {
                        workspace
                            .editor()
                            .keymap
                            .hint(&palette_items[index].invocation)
                            .unwrap_or_default()
                    } else if let Some(at) = self.local_positions.get(hit.index) {
                        Self::local_annotation(workspace, *at)
                    } else {
                        String::new()
                    },
                    details: if self.tab_dropdown_anchor.is_some() {
                        self.choice_items[index].invocation.args["_dropdown_sheet"]
                            .as_u64()
                            .and_then(|sheet| workspace.sheet_index(sheet))
                            .and_then(|index| workspace.tabs[index].path.as_ref())
                            .map(|path| path.display().to_string())
                            .unwrap_or_default()
                    } else if palette {
                        palette_items[index].invocation.command.clone()
                    } else if goto_files {
                        self.quick_path(hit.index)
                            .map(|path| {
                                Self::goto_path_detail(
                                    path,
                                    &workspace.folders,
                                    home.as_deref().map(Path::new),
                                )
                            })
                            .unwrap_or_default()
                    } else {
                        self.quick_path(hit.index)
                            .map(|p| p.display().to_string())
                            .unwrap_or_default()
                    },
                })
            })
            .take(100)
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
        self.show_quick_target(workspace, false)
    }
    fn show_quick_target(
        &mut self,
        workspace: &mut Workspace,
        accepting: bool,
    ) -> Result<(), String> {
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
                let positioned = self.last_location_preview.as_ref() == Some(&location);
                if !positioned {
                    let was_hidden = workspace.tabs[workspace.active].goto_preview;
                    let group = self
                        .preview
                        .as_ref()
                        .map_or(workspace.workbench.active, |p| p.group);
                    crate::intelligence::preview_location(
                        workspace,
                        &location,
                        group,
                        self.navigation_options,
                    )?;
                    self.cleanup_split_reference_previews(workspace, true);
                    if !was_hidden {
                        self.restore_tabs_behind_preview(workspace);
                    }
                    self.last_location_preview = Some(location.clone());
                }
                if accepting {
                    self.preserve_scroll |= positioned;
                    crate::intelligence::accept_location_preview(
                        workspace,
                        &location,
                        self.navigation_options,
                    )?;
                }
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
        if query.file.is_empty()
            && let Target::Word(term) = &query.target
        {
            self.refresh_goto_highlights(workspace, term, false);
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
        if !accepting && !workspace.settings.bool("goto_anything_file_preview", true) {
            return Ok(());
        }
        let target = query.target;
        if self.last_preview.as_ref() == Some(&(path.clone(), target.clone())) {
            match &target {
                Target::Symbol(term) => self.preview_file_symbol(workspace, term),
                Target::Word(term) => {
                    self.refresh_goto_highlights(workspace, term, true);
                }
                _ => {}
            }
            return Ok(());
        }
        self.preview_symbol_version = None;
        let (line, column) = match &target {
            Target::Line(l, c) => (*l, *c),
            _ => (1, 1),
        };
        let group = self
            .preview
            .as_ref()
            .map_or(workspace.workbench.active, |p| p.group);
        let was_hidden = workspace.tabs[workspace.active].goto_preview;
        workspace.preview_goto(
            FileLocation {
                path: path.clone(),
                line,
                column,
            },
            group,
        )?;
        if !was_hidden {
            self.restore_tabs_behind_preview(workspace);
        }
        self.last_preview = Some((path, target.clone()));
        if let Target::Word(term) = target {
            self.refresh_goto_highlights(workspace, &term, true);
        } else if let Target::Symbol(term) = target {
            self.preview_file_symbol(workspace, &term);
        }
        Ok(())
    }
    fn refresh_goto_highlights(
        &mut self,
        workspace: &mut Workspace,
        term: &str,
        jump: bool,
    ) -> bool {
        let snapshot = FindSnapshot::current(workspace);
        let query = self.quick_input.buffer.change_id();
        if self
            .goto_highlights
            .as_ref()
            .is_some_and(|h| h.snapshot == snapshot && h.query == query)
        {
            return false;
        }
        let mut ranges = Vec::<Range<usize>>::new();
        if !snapshot.binary && !term.trim().is_empty() {
            let mut scorer = fuzzy::LabelMatcher::new(term);
            let mut positions = Vec::new();
            each_goto_word(&workspace.editor().text, |word, offset| {
                positions.clear();
                if scorer.score(word, Some(&mut positions)).is_some() {
                    positions.sort_unstable();
                    positions.dedup();
                    for &at in &positions {
                        if let Some(c) = word.get(at..).and_then(|s| s.chars().next()) {
                            let start = offset + at;
                            let end = start + c.len_utf8();
                            if let Some(last) = ranges.last_mut().filter(|r| r.end == start) {
                                last.end = end;
                            } else {
                                ranges.push(start..end);
                            }
                        }
                    }
                }
            });
        }
        if jump && let Some(hit) = ranges.first() {
            workspace.editor_mut().set_caret(hit.start, false);
        }
        self.goto_highlights = Some(GotoHighlights {
            snapshot,
            query,
            ranges,
            accepted: false,
        });
        true
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
    fn restore_tabs_behind_preview(&self, workspace: &mut Workspace) {
        if workspace.tabs[workspace.active].goto_preview
            && let Some(origin) = &self.preview
        {
            origin.restore_groups(workspace);
        }
    }
    pub(crate) fn navigation_overlay_open(&self) -> bool {
        matches!(
            self.overlay,
            Some(Overlay::Goto | Overlay::ProjectSymbols | Overlay::Locations)
        )
    }
    fn cleanup_split_reference_previews(&self, workspace: &mut Workspace, keep_active: bool) {
        if !self.navigation_options.reference || !self.navigation_options.side_by_side {
            return;
        }
        let Some(origin) = &self.preview else { return };
        for index in (0..workspace.tabs.len()).rev() {
            let tab = &workspace.tabs[index];
            if (!keep_active || index != workspace.active)
                && tab.transient
                && !tab.dirty()
                && !origin
                    .groups
                    .iter()
                    .any(|group| group.sheets.contains(&tab.sheet))
            {
                let _ = workspace.close(index);
            }
        }
    }
    pub(crate) fn cancel_overlay(&mut self, workspace: &mut Workspace) {
        self.goto_filter = None;
        self.goto_highlights = None;
        self.tab_dropdown_anchor = None;
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
            self.navigation_options = Default::default();
            self.navigation_preview_guard = None;
            return;
        }
        self.cleanup_split_reference_previews(workspace, false);
        workspace.discard_goto_preview();
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
                    selection: preview.selection.clone(),
                    scroll: preview.scroll,
                    path: preview.path.clone(),
                    position: preview.position,
                },
            );
            workspace.tabs[index].scroll = preview.scroll;
            workspace.tabs[index].transient = preview.transient;
            workspace.tabs[index].pending_center = preview.pending_center;
            preview.restore_groups(workspace);
            self.restore_scroll = Some(preview.scroll);
            self.preserve_scroll = true;
        }
        self.focus.show(Surface::Overlay, false);
        self.local_generation += 1;
        self.local = None;
        self.prepared.clear();
        self.navigation_origin = None;
        workspace
            .jump_history
            .borrow_mut()
            .reset_current(crate::intelligence::capture(workspace));
        self.navigation_preview_guard = None;
        self.navigation_options = Default::default();
    }
    fn accept_tab_selection(
        workspace: &mut Workspace,
        origin: &Preview,
        action: AcceptTabs,
    ) -> Result<(), String> {
        let Some(group) = workspace.workbench.group_of(origin.sheet) else {
            return Ok(());
        };
        let source = workspace
            .sheet_index(origin.sheet)
            .expect("preview origin exists");
        workspace.make_permanent(source);
        let target = workspace.tabs[workspace.active].sheet;
        if workspace.workbench.active != group {
            workspace.clone_file_into_group(workspace.active, group)?;
        }
        let target = if workspace.workbench.group_of(target) == Some(group) {
            target
        } else {
            workspace.tabs[workspace.active].sheet
        };
        let g = &mut workspace.workbench.groups[group];
        g.selected = origin.groups[origin.group]
            .selected
            .iter()
            .copied()
            .filter(|id| g.sheets.contains(id))
            .collect();
        if target != origin.sheet {
            match action {
                AcceptTabs::ReplaceFocus => {
                    // Selecting an already displayed tab just gives it focus.
                    if !g.selected.contains(&target) {
                        g.selected.remove(&origin.sheet);
                    }
                }
                AcceptTabs::Right => {
                    g.sheets.retain(|id| *id != target);
                    let at = g.sheets.iter().position(|id| *id == origin.sheet).unwrap() + 1;
                    g.selected.retain(|id| g.sheets[..at].contains(id));
                    g.sheets.insert(at, target);
                }
                _ => {}
            }
        }
        g.selected.insert(target);
        g.focused = Some(target);
        g.anchor = g.focused;
        g.touch_focus();
        Ok(())
    }

    fn accept_item(
        &mut self,
        workspace: &mut Workspace,
        tab_action: AcceptTabs,
        new_window: bool,
        keep_open: bool,
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
                self.palette_match_limit(),
            );
            self.publish_quick(workspace, hits);
            self.quick_pending = false;
        }
        if matches!(
            self.overlay,
            Some(Overlay::Palette | Overlay::Syntax | Overlay::Choices)
        ) {
            let mut invocation = self
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
            if let Some(action) = &mut invocation
                && action.command == "open_project"
                && action.args["_quick_switch"] == true
            {
                action.args["new_window"] = json!(new_window);
            }
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
            self.tab_dropdown_anchor = None;
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
            if keep_open {
                self.show_quick_target(workspace, true)?;
                workspace.commit_goto_preview();
                let active = workspace.active;
                if let Some(origin) = &mut self.preview
                    && origin.sheet == workspace.tabs[active].sheet
                {
                    origin.transient = false;
                }
                workspace.remember_goto_file();
                if let Some(highlights) = &mut self.goto_highlights {
                    highlights.ranges.clear();
                    highlights.accepted = true;
                }
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
            self.preserve_scroll |= workspace.tabs[workspace.active].goto_preview;
            self.show_quick_target(workspace, true)?;
            workspace.commit_goto_preview();
            if let Some(source) = &self.preview
                && let Some(index) = workspace.tabs.iter().position(|t| t.sheet == source.sheet)
                && index != workspace.active
            {
                workspace.tabs[index].transient = source.transient;
            }
            if tab_action != AcceptTabs::Default
                && matches!(self.overlay, Some(Overlay::Goto | Overlay::ProjectSymbols))
                && let Some(origin) = &self.preview
            {
                Self::accept_tab_selection(workspace, origin, tab_action)?;
            }
            workspace
                .jump_history
                .borrow_mut()
                .reset_current(crate::intelligence::capture(workspace));
            self.navigation_options = Default::default();
            workspace.remember_goto_file();
            self.overlay = None;
            self.goto_highlights = None;
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
        if !self.captured() && key.modifiers == 0 && workspace.tabs[workspace.active].find_results {
            match key.name.as_str() {
                "enter" if workspace.editor().marked.is_none() => {
                    if self.jump_tab_result(workspace, workspace.editor().caret)? {
                        return Ok((true, None));
                    }
                }
                "escape" if self.file_task.is_some() => {
                    self.close_panel(workspace);
                    self.cancel_files(workspace);
                    return Ok((true, None));
                }
                _ => {}
            }
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
        if self.overlay.is_none()
            && self.panel_focused
            && !self.side_focused
            && self.show_build_input()
        {
            let command = match (key.name.as_str(), key.modifiers) {
                ("enter", 0) => "build_send_input",
                ("d", 1) => "build_close_input",
                _ => return Ok((false, None)),
            };
            return Ok((
                true,
                Some(Invocation {
                    command: command.into(),
                    args: json!({}),
                }),
            ));
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
        // The navigation guide opens a result on Right without dismissing Goto.
        // Leave ordinary cursor movement, selection and IME input alone.
        if self.overlay == Some(Overlay::Goto)
            && key.name == "right"
            && key.modifiers == 0
            && !self.quick_input.composing()
            && self.quick_input.caret_at_end()
        {
            return Ok((
                true,
                self.accept_item(workspace, AcceptTabs::Default, false, true, viewport)?,
            ));
        }
        if key.name == "enter" {
            return Ok((
                true,
                self.accept_item(
                    workspace,
                    AcceptTabs::from_modifiers(
                        key.modifiers & 9 != 0,
                        key.modifiers & 4 != 0,
                        key.modifiers & 2 != 0,
                    ),
                    key.modifiers & 8 != 0,
                    false,
                    viewport,
                )?,
            ));
        }
        if key.name == "tab" && matches!(key.modifiers, 0 | 4) {
            let count = match self.panel {
                Some(Panel::Files) => 3,
                Some(Panel::Replace) => 2,
                _ => 1,
            };
            let previous = self.focus_input;
            self.focus_input =
                (self.focus_input + if key.modifiers & 4 != 0 { count - 1 } else { 1 }) % count;
            if previous != self.focus_input {
                self.refresh_find_pasteboard(workspace);
            }
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
            Command::Right(false)
                if self.overlay == Some(Overlay::Goto) && self.quick_input.caret_at_end() =>
            {
                Some("right")
            }
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
    fn start_files(&mut self, workspace: &mut Workspace) -> Result<(), String> {
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
        let mut scope = match files::Where::parse(&self.where_input.value(), base) {
            Ok(scope) => scope,
            Err(error) => {
                self.set_message(error);
                return Ok(());
            }
        };
        #[cfg(windows)]
        for root in &mut scope.roots {
            if let Ok(canonical) = std::fs::canonicalize(&*root) {
                *root = canonical;
            }
        }
        scope.use_gitignore = self.files_use_gitignore;
        self.cancel_files(workspace);
        self.file_signature = Some(self.files_identity(workspace));
        self.replace_rx = None;
        self.replacement_plan = None;
        let mut untitled = BTreeMap::new();
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
            .filter(|(i, tab)| !tab.find_results && (scope.open_files || *i == workspace.active))
            .map(|(i, tab)| {
                let path = tab.path.clone().unwrap_or_else(|| {
                    let path = PathBuf::from(format!("Untitled {}", i + 1));
                    untitled.insert(path.clone(), tab.sheet);
                    path
                });
                files::OpenFile {
                    path,
                    text: tab.editor.text.clone(),
                }
            })
            .collect();
        let decode = vel_fileio::DecodeOptions::from_settings(&workspace.settings.editor());
        let settings = &workspace.settings;
        let task = files::Task::start_with_limits(
            files::Request {
                search,
                scope,
                folders: workspace.folders.clone(),
                open,
                context: if self.files_context {
                    settings.usize("find_in_files_context_lines", 1).min(100)
                } else {
                    0
                },
            },
            decode,
            files::Limits {
                max_file_size: settings.usize("find_in_files_max_file_size", 0) as u64,
                context_characters: settings.usize("find_in_files_context_characters", 0),
                suppress_errors: settings.bool("find_in_files_suppress_errors", false),
            },
        );
        let task = match task {
            Ok(task) => task,
            Err(error) => {
                self.set_message(error);
                return Ok(());
            }
        };
        self.result_destination = if self.files_use_buffer {
            let heading = self.files_heading(workspace, 0);
            let index = workspace.show_find_results()?;
            let tab = &mut workspace.tabs[index];
            let results = self
                .results_tab
                .get_or_insert_with(|| find_results::ResultsTab::new(tab));
            if results.sheet != tab.sheet {
                *results = find_results::ResultsTab::new(tab);
            }
            results.begin(tab, heading);
            // Keep Where/Replace reachable and the panel's close button available
            // while handing text input and scrolling to the result sheet.
            self.panel = Some(Panel::Files);
            self.focus_editor_for_context();
            Some(tab.sheet)
        } else {
            self.panel = Some(Panel::Results);
            None
        };
        self.result_untitled = untitled;
        self.file_task = Some(task);
        self.search_results_active = true;
        self.result_cursor = None;
        self.result_budget =
            ResultBudget::new(workspace.settings.usize("find_in_files_max_result_size", 0));
        self.result_error = None;
        self.files_searched = 0;
        self.file_matches = 0;
        self.result_view = files::Results::ordered(&workspace.folders);
        self.result_selected = 0;
        self.result_navigated = false;
        self.result_scroll.offset = 0.;
        self.file_complete = false;
        self.result_status = UiText::tr_fmt("Searching {files} files…", [("files", "0".into())]);
        self.set_message(self.result_status.clone());
        Ok(())
    }

    fn files_heading(&self, workspace: &Workspace, files: usize) -> String {
        let query = self.file_signature.as_ref().expect("search signature");
        let location = if query.where_.trim().is_empty() {
            workspace.translations.tr("open folders").into_owned()
        } else {
            query.where_.escape_debug().to_string()
        };
        let mut heading = UiText::tr_fmt(
            "Searching {files} files for \"{query}\" in {location}",
            [
                ("files", files.to_string()),
                ("query", query.query.escape_debug().to_string()),
                ("location", location),
            ],
        );
        heading.refresh(&workspace.translations);
        heading.to_string()
    }

    fn finish_results_tab(&mut self, workspace: &mut Workspace, mut footer: UiText) {
        let Some(sheet) = self.result_destination else {
            return;
        };
        let heading = self.files_heading(workspace, self.files_searched);
        footer.refresh(&workspace.translations);
        if let (Some(results), Some(index)) = (
            self.results_tab.as_mut(),
            Self::results_sheet_index(workspace, sheet),
        ) {
            results.finish(&mut workspace.tabs[index].editor, heading, &footer);
        }
    }

    fn results_sheet_index(workspace: &Workspace, sheet: u64) -> Option<usize> {
        workspace
            .sheet_index(sheet)
            .filter(|&index| workspace.tabs[index].find_results)
    }

    fn cancel_files(&mut self, workspace: &mut Workspace) {
        let Some(task) = self.file_task.take() else {
            return;
        };
        task.cancel();
        self.files_searched = task.searched_files();
        self.file_complete = false;
        self.finish_results_tab(workspace, UiText::tr("Search cancelled"));
        self.result_status = UiText::tr_fmt(
            "Search cancelled · {count} matches in {files} files",
            [
                ("count", self.file_matches.to_string()),
                ("files", self.result_view.matched_files().to_string()),
            ],
        );
        self.set_message(self.result_status.clone());
    }

    pub(crate) fn report_file_replacements(
        &mut self,
        workspace: &mut Workspace,
        count: usize,
        files: usize,
    ) -> Result<(), String> {
        let mut report = UiText::tr_fmt(
            "Replaced {count} matches across {files} files",
            [("count", count.to_string()), ("files", files.to_string())],
        );
        if let Some(sheet) = self.result_destination {
            let index = match Self::results_sheet_index(workspace, sheet) {
                Some(index) => index,
                None => workspace.show_find_results()?,
            };
            let tab = &mut workspace.tabs[index];
            let results = self
                .results_tab
                .get_or_insert_with(|| find_results::ResultsTab::new(tab));
            if results.sheet != tab.sheet {
                *results = find_results::ResultsTab::new(tab);
            }
            self.result_destination = Some(tab.sheet);
            report.refresh(&workspace.translations);
            results.report(&mut tab.editor, &report);
        } else {
            self.panel = Some(Panel::Results);
            self.result_status = report;
        }
        Ok(())
    }
    fn prepare_files(&mut self, workspace: &Workspace) -> Result<(), String> {
        self.remember_search(true);
        if !self.file_complete {
            return Err("Finish Find in Files before preparing replacements".into());
        }
        if self.file_signature.as_ref() != Some(&self.files_identity(workspace)) {
            return Err(
                "Find in Files options changed; run the search again before replacing".into(),
            );
        }
        let search = Search::new(&self.query.value(), self.options)?;
        let mut paths: BTreeSet<_> = self
            .result_view
            .rows
            .iter()
            .filter_map(|r| match r {
                files::Row::Line(line) if line.match_count > 0 => Some(line.path.clone()),
                _ => None,
            })
            .collect();
        if let Some(budget) = &self.result_budget {
            paths.extend(budget.hidden_matches.iter().cloned());
        }
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
        self.replace_signature = Some((self.files_identity(workspace), replacement.clone()));
        std::thread::spawn(move || {
            let mut budget = 128 * 1024 * 1024usize;
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
                .filter_map(|change| match change {
                    Ok(change) if change.count == 0 => None,
                    Ok(change) if search.uses_perl() => {
                        Some(budget.checked_sub(change.preview_bytes()).map_or_else(
                            || Err("Advanced regex replacement preview exceeded 128 MiB".into()),
                            |remaining| {
                                budget = remaining;
                                Ok(change)
                            },
                        ))
                    }
                    change => Some(change),
                })
                .collect::<Result<Vec<_>, _>>();
            let _ = tx.send(result);
        });
        self.replace_rx = Some(rx);
        self.set_message(UiText::tr("Preparing replacement preview…"));
        Ok(())
    }
    pub(crate) fn file_replace_ready(&self, workspace: &Workspace) -> bool {
        self.file_complete && self.file_signature.as_ref() == Some(&self.files_identity(workspace))
    }
    fn files_identity(&self, workspace: &Workspace) -> FilesIdentity {
        FilesIdentity {
            query: self.query.value(),
            options: self.options,
            where_: self.where_input.value(),
            use_gitignore: self.files_use_gitignore,
            max_file_size: workspace.settings.usize("find_in_files_max_file_size", 0) as u64,
        }
    }
    pub fn jump_result(&mut self, workspace: &mut Workspace) -> Result<(), String> {
        if let Some(files::Row::Line(line)) = self.result_view.rows.get(self.result_selected) {
            let target = find_results::target(line, &self.result_untitled);
            self.result_cursor = Some(self.result_selected);
            self.open_result_target(workspace, target, self.result_destination)?;
        }
        Ok(())
    }
    pub(crate) fn jump_tab_result(
        &mut self,
        workspace: &mut Workspace,
        point: usize,
    ) -> Result<bool, String> {
        let sheet = workspace.tabs[workspace.active].sheet;
        let Some(results) = self.results_tab.as_mut().filter(|r| r.sheet == sheet) else {
            return Ok(false);
        };
        let Some((rank, target)) = results.at(workspace.editor(), point) else {
            return Ok(false);
        };
        if self.result_destination == Some(sheet)
            && let Some(rank) = rank
        {
            self.result_cursor = Some(rank);
            self.result_selected = rank;
            self.result_navigated = true;
        }
        self.open_result_target(workspace, target, Some(sheet))?;
        Ok(true)
    }
    pub(crate) fn results_tab_matches(
        &mut self,
        tab: &crate::workspace::Tab,
        bytes: Range<usize>,
    ) -> Vec<Range<usize>> {
        self.results_tab
            .as_mut()
            .filter(|r| r.sheet == tab.sheet)
            .map_or_else(Vec::new, |r| r.matches(&tab.editor, bytes))
    }
    fn open_result_target(
        &mut self,
        workspace: &mut Workspace,
        target: find_results::Target,
        source: Option<u64>,
    ) -> Result<(), String> {
        workspace.observe_navigation();
        let _navigation =
            crate::editor::navigation::Guard::command(&workspace.jump_history, "find_result");
        let location = FileLocation {
            path: target.path.to_path_buf(),
            line: target.line,
            column: target.column,
        };
        let beside = source
            .filter(|_| workspace.settings.bool("find_in_files_side_by_side", false))
            .and_then(|sheet| {
                Self::results_sheet_index(workspace, sheet).map(|index| (sheet, index))
            });
        if let Some((_, index)) = beside {
            workspace.select(index);
        }
        if let Some(sheet) = target.sheet {
            if let Some(index) = workspace.sheet_index(sheet) {
                workspace.select(index);
                workspace.tabs[index].goto(&location);
            }
        } else if workspace.tabs.iter().any(|t| {
            t.path.as_deref() == Some(&target.path) && (t.transient || t.preview_recent_pending)
        }) {
            workspace.preview(location)?;
        } else {
            workspace.open(location)?;
        }
        if let Some((source, _)) = beside {
            let target = workspace.tabs[workspace.active].sheet;
            if target != source
                && let Some(group) = workspace.workbench.group_of(source)
                && workspace.workbench.active == group
            {
                workspace.workbench.groups[group].selected.insert(source);
            }
        }
        self.reveal = true;
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
            let spelling = location.path.clone();
            workspace.open_from_sidebar(location, permanent)?;
            if permanent {
                self.note_sidebar_spelling(workspace, &spelling);
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
            self.message = error.into();
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
        let goto_highlights_changed = self.overlay == Some(Overlay::Goto)
            && self.goto_highlights.as_ref().is_some_and(|highlights| {
                let snapshot = FindSnapshot::current(workspace);
                !highlights.accepted
                    && highlights.query == self.quick_input.buffer.change_id()
                    && highlights.snapshot.sheet == snapshot.sheet
                    && highlights.snapshot != snapshot
            })
            && {
                let query = Query::parse(&self.quick_input.value());
                if let Target::Word(term) = query.target {
                    self.refresh_goto_highlights(workspace, &term, !query.file.is_empty())
                } else {
                    false
                }
            };
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
                            self.navigation_options,
                        );
                        workspace
                            .jump_history
                            .borrow_mut()
                            .reset_current(crate::intelligence::capture(workspace));
                        self.navigation_options = Default::default();
                    } else if result.locations.is_empty() {
                        self.set_message(UiText::tr("No indexed locations"));
                        self.navigation_origin = None;
                        self.navigation_options = Default::default();
                    } else {
                        let origin = self.navigation_origin.take();
                        let options = self.navigation_options;
                        let _ =
                            self.action(workspace, "show_overlay", &json!({}), viewport.clone());
                        self.navigation_origin = origin;
                        self.navigation_options = options;
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
        let mut changed =
            load_notice || intelligence_changed || query_changed || goto_highlights_changed;
        changed |= self.expire_message(Instant::now());
        if let Some(result) = self.replace_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.replace_rx = None;
            if self.replace_signature.as_ref()
                == Some(&(self.files_identity(workspace), self.replace.value()))
            {
                match result {
                    Ok(plan) => self.replacement_plan = Some(plan),
                    Err(error) => self.set_message(UiText::tr_fmt(
                        "Could not prepare replacement: {detail}",
                        [("detail", error)],
                    )),
                }
            } else {
                self.set_message(UiText::tr(
                    "Replacement options changed; prepare the preview again",
                ));
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
            self.goto_filter = None;
            self.prepared.clear();
            workspace.messages.extend(snapshot.errors);
            if self.overlay == Some(Overlay::Goto) {
                self.query_quick(workspace);
            }
            changed = true;
        }
        if self.overlay == Some(Overlay::Goto)
            && self.quick_target == "files"
            && (self
                .goto_filter
                .as_mut()
                .is_some_and(goto_files::Filter::poll)
                || self.goto_filter.is_some() != Self::goto_filters_gitignore(workspace))
        {
            self.prepared.clear();
            self.query_quick(workspace);
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
                        self.message = error.into();
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
                        self.set_message(UiText::tr(
                            "Recursive expansion stopped at 5,000 directories",
                        ));
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
                self.message = error.into();
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
            changed |= self.ensure_find(workspace, viewport.clone());
            if let Some(matches) = self.search_worker.poll()
                && self.search_identity.as_ref().is_some_and(|key| {
                    key.snapshot == FindSnapshot::current(workspace)
                        && key.snapshot.revision == matches.revision
                })
            {
                self.all_matches_ready = true;
                if let Some(error) = matches.error {
                    self.find_status = error.into();
                    self.find_error = Some(self.find_status.clone());
                    self.set_message(self.find_status.clone());
                    self.search = None;
                } else {
                    self.highlights = matches.ranges;
                    self.update_find_status();
                }
                changed = true;
            }
            self.finish_pending_find(workspace);
        }
        changed |= self.poll_document_replacement(workspace, viewport);
        if self
            .results_tab
            .as_ref()
            .is_some_and(|results| Self::results_sheet_index(workspace, results.sheet).is_none())
        {
            self.results_tab = None;
            if self.result_destination.is_some() {
                self.cancel_files(workspace);
            }
        }
        if let Some(task) = &self.file_task {
            let searched = task.searched_files();
            if searched != self.files_searched {
                self.files_searched = searched;
                self.result_status = UiText::tr_fmt(
                    "Searching {files} files…",
                    [("files", searched.to_string())],
                );
                self.set_message(self.result_status.clone());
                changed = true;
            }
        }
        let deadline = self
            .file_task
            .as_ref()
            .map(|_| Instant::now() + Duration::from_millis(1));
        let mut batch = find_results::Batch::default();
        let mut done = None;
        for index in 0..256 {
            if index != 0 && (batch.full() || deadline.is_some_and(|end| Instant::now() >= end)) {
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
                    if let Some(budget) = &mut self.result_budget
                        && !budget.allows(&line)
                    {
                        changed = true;
                        continue;
                    }
                    let inserted = self.result_view.push(line);
                    if self.result_destination.is_some()
                        || self.result_navigated
                        || self.result_cursor.is_some()
                    {
                        let at = self.result_view.rank(inserted);
                        if self.result_destination.is_some() {
                            batch.insert(
                                &self.result_view,
                                at,
                                inserted.added,
                                &self.result_untitled,
                            );
                        }
                        if self.result_navigated && at <= self.result_selected {
                            self.result_selected += inserted.added;
                        }
                        if let Some(cursor) = self.result_cursor.as_mut().filter(|c| at <= **c) {
                            *cursor += inserted.added;
                        }
                    }
                }
                files::Event::Error(error) => {
                    let error = UiText::tr_fmt("File search failed: {detail}", [("detail", error)]);
                    self.result_error.get_or_insert_with(|| error.clone());
                    self.set_message(error);
                }
                files::Event::Done { files, .. } => {
                    done = Some(files);
                    self.file_task = None;
                }
            }
            changed = true;
        }
        if let Some(sheet) = self.result_destination
            && let (Some(results), Some(index)) = (
                self.results_tab.as_mut(),
                Self::results_sheet_index(workspace, sheet),
            )
        {
            results.write(&mut workspace.tabs[index].editor, batch);
        }
        if let Some(files) = done {
            self.files_searched = files;
            let matched = self.result_view.matched_files()
                + self.result_budget.as_ref().map_or(0, |budget| {
                    budget
                        .hidden_matches
                        .iter()
                        .filter(|path| !budget.shown_paths.get(*path).copied().unwrap_or(false))
                        .count()
                });
            self.result_status = match (self.file_matches, matched) {
                (1, 1) => UiText::tr("1 match in 1 file"),
                (count, 1) => {
                    UiText::tr_fmt("{count} matches in 1 file", [("count", count.to_string())])
                }
                (count, files) => UiText::tr_fmt(
                    "{count} matches in {files} files",
                    [("count", count.to_string()), ("files", files.to_string())],
                ),
            };
            let mut summary = match (self.file_matches, matched) {
                (1, 1) => UiText::tr("1 match across 1 file"),
                (count, 1) => UiText::tr_fmt(
                    "{count} matches across 1 file",
                    [("count", count.to_string())],
                ),
                (count, files) => UiText::tr_fmt(
                    "{count} matches across {files} files",
                    [("count", count.to_string()), ("files", files.to_string())],
                ),
            };
            if self
                .result_budget
                .as_ref()
                .is_some_and(|budget| budget.truncated)
            {
                let note = UiText::tr("Result size limit reached; not all matches are shown.");
                summary = UiText::compose(
                    "{left}\n{right}",
                    [("left", summary), ("right", note.clone())],
                );
                self.result_status = note;
            }
            if let Some(error) = self.result_error.take() {
                summary = UiText::compose(
                    "{left}\n{right}",
                    [("left", summary), ("right", error.clone())],
                );
                self.result_status = error;
            }
            self.finish_results_tab(workspace, summary);
            self.set_message(self.result_status.clone());
            self.file_complete = true;
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
        self.refresh_locale(workspace);
        changed |= self.sync_output();
        changed
    }
    pub(crate) fn status_visible(&self) -> bool {
        self.chrome && !self.chrome_auto_hidden
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
        let editor_x = if self.sidebar_on_right { 0. } else { side };
        let status = if self.status_visible() {
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
            x: editor_x,
            y: size.height - status - bottom,
            width: (size.width - side).max(1.),
            height: bottom,
        };
        self.side_bounds = Rect {
            x: if self.sidebar_on_right {
                size.width - side
            } else {
                0.
            },
            y: (30. * self.ui_scale),
            width: side,
            height: (size.height - (30. * self.ui_scale) - status).max(1.),
        };
        self.side_scroll.set_viewport(self.side_bounds.height);
        let width = (size.width * 0.7)
            .clamp(240. * self.ui_scale, 820. * self.ui_scale)
            .min(size.width);
        let width = if self.tab_dropdown_anchor.is_some() {
            width.min(360. * self.ui_scale)
        } else {
            width
        };
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
        let (x, y) = self.tab_dropdown_anchor.map_or(
            ((size.width - width) / 2., 54. * self.ui_scale),
            |anchor| {
                (
                    (anchor.x + anchor.width - width).clamp(0., (size.width - width).max(0.)),
                    anchor.y + anchor.height,
                )
            },
        );
        let y = y.min((size.height - height).max(0.));
        self.quick_bounds = Rect {
            x,
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
        let close = self
            .panel
            .filter(|_| self.find_panel_visible())
            .map_or(0., |panel| {
                Self::find_edge_reserve(panel, self.panel_bounds.width, self.ui_scale)
            });
        let v = self.find_vertical_scale();
        // Narrow panels give the input room first: the label column shrinks, then the labels
        // are dropped, before the input would collapse under the close button. The input
        // panel's caption column follows its measured caption.
        let label = if self.panel == Some(Panel::Input) {
            Self::input_label_column(
                self.panel_bounds.width,
                self.ui_scale,
                self.input_caption_advance,
            )
        } else {
            Self::find_label_column(
                self.panel_bounds.width,
                self.ui_scale,
                close,
                self.find_sizing.caption,
            )
        };
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
        let build_input_height = if self.show_build_input() {
            let input = self.build_input.as_mut().expect("interactive build");
            let height = (34. * self.ui_scale).min((bottom - 30. * self.ui_scale).max(0.));
            let inset = (4. * self.ui_scale).min(height / 4.);
            input.scale = self.ui_scale;
            input.set_row_pitch(18. * self.ui_scale);
            input.bounds = Rect {
                x: self.panel_bounds.x + inset,
                y: self.panel_bounds.y + bottom - height + inset,
                width: (self.panel_bounds.width - 2. * inset).max(1.),
                height: height - 2. * inset,
            };
            input.reveal();
            height
        } else {
            0.
        };
        self.result_scroll.row_height =
            if self.panel == Some(Panel::Output) && self.output_editor.is_some() {
                self.output_view.line_height
            } else {
                24. * self.ui_scale
            };
        self.result_scroll
            .set_viewport((bottom - (34. * self.ui_scale) - build_input_height).max(1.));
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
            x: editor_x,
            y: 0.,
            width: (size.width - side).max(1.),
            height: (size.height - status - bottom).max(32. * self.ui_scale),
        }
    }
    fn paint_input(
        layer: &mut Layer,
        input: &Input,
        focused: bool,
        style: &vel_ui::theme::Style,
        density: f64,
    ) {
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
        let underline = |layer: &mut Layer, a, b, y| {
            paint_marked_underline(layer, rect, [a, b], y, density, style.foreground);
        };
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
                    underline(layer, x(marked.start), x(marked.end), y + 24. * s);
                }
            }
            if focused {
                fill(layer, state.caret, rect, [0.9, 0.92, 0.97, 1.]);
            }
            // Hard rows past the four the field shows: a slim indicator on its right edge
            // says how much of the text is out of sight, and where.
            let (shown, rows, first) = input.shown_rows();
            if rows > shown {
                let inset = 2. * s;
                let travel = rect.height - inset * 2.;
                let thumb = (travel * shown as f64 / rows as f64).max(16. * s);
                let at = first.min(rows - shown) as f64 / (rows - shown) as f64;
                fill(
                    layer,
                    Rect {
                        x: rect.x + rect.width - inset - 2. * s,
                        y: rect.y + inset + (travel - thumb) * at,
                        width: 2. * s,
                        height: thumb,
                    },
                    rect,
                    style.matched,
                );
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
            underline(layer, a, b, rect.y + (24. * input.scale * k));
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
            let compact = self.quick_compact();
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
                // Compact rows paint their icon, label and shortcut directly;
                // only the row itself is a retained hit target.
                if compact {
                    continue;
                }
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
                    self.files_context,
                    self.files_use_buffer,
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
                        .label = workspace.ui.find_buttons[*caption].to_string();
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
                    .label = workspace.ui.close_find.to_string();
                self.tree
                    .nodes
                    .get_mut(&2)
                    .expect("panel")
                    .children
                    .push(CLOSE_ID);
                if self.show_find_options_button() {
                    self.widget(WHERE_ID, "button", self.find_options_rect(), &[]);
                    self.tree
                        .nodes
                        .get_mut(&WHERE_ID)
                        .expect("Where button")
                        .label = workspace.ui.where_menu.to_string();
                    self.tree
                        .nodes
                        .get_mut(&2)
                        .expect("panel")
                        .children
                        .push(WHERE_ID);
                }
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
                let selected = if self.side_focused {
                    self.side_open.is_none() && self.side_selected == i
                } else {
                    // Match the editor view (including a tabless Goto preview) without
                    // selecting, expanding or scrolling the sidebar to it.
                    !row.directory
                        && workspace.tabs[workspace.active].path.as_deref() == Some(&row.path)
                };
                if selected {
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
        if let Some(rect) = self.status_switcher_rect(workspace) {
            self.widget(STATUS_SWITCHER, "sidebar_button_control", rect, &[]);
            self.tree.nodes.get_mut(&STATUS_SWITCHER).unwrap().label =
                workspace.translations.tr("Sidebar and Panels").into_owned();
            self.tree
                .nodes
                .get_mut(&4)
                .unwrap()
                .children
                .push(STATUS_SWITCHER);
        }
        let settings = &workspace.editor().settings;
        for (id, item, label, shown) in [
            (
                6,
                details::StatusItem::Indentation,
                "Indentation",
                settings.bool("show_indentation", true),
            ),
            (
                7,
                details::StatusItem::Syntax,
                "Syntax",
                settings.bool("show_syntax", true),
            ),
            (8, details::StatusItem::Git, "Git status", true),
        ] {
            if shown && let Some(rect) = self.status_item(item) {
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
                x: self.panel_bounds.x,
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
        self.refresh_locale(workspace);
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
        if self.fit_input_caption(workspace)
            || (self.find_panel_visible() && self.measure_find_controls(workspace))
        {
            self.sync_widgets(workspace);
        }
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
            for (caption, source, row) in [
                (workspace.ui.open_files.as_ref(), "Open Files", 0),
                (
                    workspace.ui.folders.as_ref(),
                    "Folders",
                    self.folder_row_base(workspace) - 1,
                ),
            ]
            .into_iter()
            .skip(usize::from(self.visible_open_files(workspace) == 0))
            {
                // A localized heading keeps the capitalization chosen by its translator.
                let mut heading = heading.clone();
                if caption != source {
                    heading.case = None;
                }
                theme.text_fit(
                    &mut layer,
                    &heading,
                    caption,
                    10. * self.ui_scale,
                    clip.y + row as f64 * (24. * self.ui_scale) - self.side_scroll.offset
                        + (17. * self.ui_scale),
                    clip.width - 20. * self.ui_scale,
                    clip,
                );
            }
            if workspace.folders.is_empty() {
                // Empty state under FOLDERS: say how to add one instead of a blank list.
                let hint = style(3, Some("label_control"), &mut theme);
                // First row under the FOLDERS heading, wherever Open Files left it.
                let top = clip.y + self.folder_row_base(workspace) as f64 * (24. * self.ui_scale)
                    - self.side_scroll.offset;
                for (line, caption) in [
                    workspace.ui.no_folders.as_ref(),
                    workspace.ui.add_folder_hint.as_ref(),
                ]
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
                let label = tab.ui_label(&workspace.ui);
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
                        format!("{} · {}", workspace.ui.find_results, self.result_status)
                    } else if self.console_visible() {
                        workspace.ui.console.to_string()
                    } else {
                        workspace.ui.output.to_string()
                    },
                    clip.x + (10. * self.ui_scale),
                    clip.y + (20. * self.ui_scale),
                    clip,
                );
                if self.console_visible() {
                    let limit = Self::console_limit(workspace);
                    let discard = self.console_messages.len().saturating_sub(limit);
                    self.console_messages.drain(..discard);
                    let pad = 10. * self.ui_scale;
                    let height = 18. * self.ui_scale;
                    let count = ((clip.height - 40. * self.ui_scale) / height).max(0.) as usize + 1;
                    for (row, line) in self
                        .console_messages
                        .iter()
                        .rev()
                        .take(count)
                        .rev()
                        .enumerate()
                    {
                        theme.text_fit(
                            &mut layer,
                            &text_style,
                            line,
                            clip.x + pad,
                            clip.y + (40. * self.ui_scale) + row as f64 * height,
                            clip.width - 2. * pad,
                            clip,
                        );
                    }
                } else if panel == Panel::Results {
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
                    // No label column in very narrow panels (see `find_label_column`). The
                    // input panel's caption is fitted to its column, 8 DIPs before the field.
                    if input.bounds.x - clip.x >= 64. * self.ui_scale {
                        theme.text_fit(
                            &mut layer,
                            &text_style,
                            if panel == Panel::Input {
                                &self.input_caption
                            } else {
                                &workspace.ui.find_fields[i]
                            },
                            clip.x + (8. * self.ui_scale),
                            input.bounds.y + (20. * self.ui_scale),
                            input.bounds.x - clip.x - 16. * self.ui_scale,
                            clip,
                        );
                    }
                    let input_style = style(20 + i as u64, None, &mut theme);
                    paint_themed_input(
                        &theme,
                        &input_style,
                        &mut layer,
                        input,
                        self.overlay.is_none() && self.panel_focused && self.focus_input == i,
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
                        "icon_context",
                        "icon_use_buffer",
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
                        let shown = theme.fit_label(
                            &button,
                            &node.label,
                            (node.rect.width - 8. * self.ui_scale).max(0.),
                        );
                        let width = theme.advance(&button, &shown);
                        theme.text(
                            &mut layer,
                            &button,
                            &shown,
                            (node.rect.x + (node.rect.width - width) / 2.).round(),
                            node.rect.y + node.rect.height / 2. + 4. * self.ui_scale,
                            node.rect,
                        );
                    }
                    self.paint_find_edge_buttons(&mut theme, settings, &mut layer, clip);
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
        if self.status_visible() {
            let status = self.tree.nodes[&4].rect;
            let status_style = style(4, None, &mut theme);
            theme.surface(&mut layer, &status_style, status, status);
            let text = style(4, Some("status_container"), &mut theme);
            let mut left = 10. * self.ui_scale;
            if let Some(rect) = self.status_switcher_rect(workspace) {
                let switcher = style(STATUS_SWITCHER, None, &mut theme);
                theme.icon_surface(&mut layer, &switcher, rect, status);
                left = rect.x + rect.width + 6. * self.ui_scale;
            }
            let editor = workspace.editor();
            let tab = &workspace.tabs[workspace.active];
            let document_hint = if let Some(binary) = &editor.binary {
                format!(
                    "{} · ",
                    vel_i18n::format_named(
                        &workspace.ui.hex,
                        &[
                            ("offset", format!("{:x}", editor.caret)),
                            ("bytes", binary.view.len_bytes().to_string()),
                        ]
                    )
                )
            } else if let Some((read, total)) = tab.progress() {
                format!(
                    "{} · ",
                    vel_i18n::format_named(
                        &workspace.ui.loading,
                        &[(
                            "percent",
                            (read.saturating_mul(100) / total.max(1)).to_string()
                        ),]
                    )
                )
            } else {
                let mut hint = crate::large::hint(editor.features, &workspace.ui);
                if editor.layout.borrow().long.pending() {
                    if !hint.is_empty() {
                        hint.push_str(" · ");
                    }
                    hint.push_str(&workspace.ui.preparing);
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
            let mut language = editor
                .language
                .borrow()
                .as_ref()
                .map(|l| l.name.clone())
                .unwrap_or_else(|| "Plain Text".into());
            if tab.find_results && language == "Find Results" {
                language = workspace.ui.find_results.to_string();
            }
            // Hex has a format label, but not a selectable text syntax.
            let text_document = editor.binary.is_none();
            let candidates = [
                (
                    if text_document {
                        details::StatusItem::Syntax
                    } else {
                        details::StatusItem::Binary
                    },
                    if !settings.bool("show_syntax", true) {
                        String::new()
                    } else if text_document {
                        language
                    } else if tab.is_image_file() {
                        String::new()
                    } else {
                        workspace.translations.tr("Binary").into_owned()
                    },
                    0.,
                ),
                (
                    details::StatusItem::Indentation,
                    if text_document && settings.bool("show_indentation", true) {
                        if workspace.translations.language().is_english() {
                            vel_ui::status::indentation(
                                editor.preferences.translate_tabs_to_spaces,
                                editor.preferences.tab_size,
                            )
                        } else {
                            workspace.ui.indent(
                                editor.preferences.translate_tabs_to_spaces,
                                editor.preferences.tab_size,
                            )
                        }
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
                        .map(|p| {
                            vel_i18n::format_named(
                                &workspace.ui.indexing,
                                &[("percent", p.to_string())],
                            )
                        })
                        .unwrap_or_default(),
                    0.,
                ),
            ];
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
                    details::StatusItem::Binary | details::StatusItem::Indexing => None,
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
                            format!("{} · ", editor.localized_vintage_status(&workspace.ui))
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
                    x: left,
                    width: (right - 8. * s - left).max(0.),
                    ..status
                },
            );
        } else {
            self.status_items = None;
        }
        scene.layers.push(layer);
        let theme_animating = theme.animating();
        drop(theme);
        if self.panel == Some(Panel::Output)
            && !self.show_console
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
            let top = self.panel_bounds.y + (30. * self.ui_scale);
            let bottom = self
                .build_input
                .as_ref()
                .map_or(self.panel_bounds.y + self.panel_bounds.height, |input| {
                    input.bounds.y - 4. * self.ui_scale
                });
            let body = Rect {
                x: self.panel_bounds.x,
                y: top,
                width: self.panel_bounds.width,
                height: (bottom - top).max(1.),
            };
            self.output_view.size = Size {
                width: body.width,
                height: body.height,
            };
            self.output_view.scroll_y = self.result_scroll.offset;
            self.output_view.apply_style(editor);
            self.result_scroll.row_height = self.output_view.line_height;
            self.result_scroll.rows = editor.layout.borrow().total();
            let mut output = self
                .output_view
                .scene_with_build_input(editor, &self.build_input_echo);
            crate::app_scene_translate(&mut output, body.x, body.y);
            scene.layers.extend(output.layers);
            if let Some(input) = &mut self.build_input
                && input.bounds.height > 0.
            {
                let mut theme = workspace.themes.ui.borrow_mut();
                let focused = self.panel_focused && self.overlay.is_none() && !self.side_focused;
                let path = [
                    vel_theme::Element::new("window", &[]),
                    vel_theme::Element::new("panel_control output_panel", &[]),
                    vel_theme::Element::new(
                        "text_line_control",
                        if focused { &["focused"] } else { &[] },
                    ),
                ];
                let style = theme.style(
                    StyleId::BuildInput.at(0),
                    &vel_theme::ElementPath(&path),
                    settings,
                );
                let mut row = Layer::default();
                paint_themed_input(&theme, &style, &mut row, input, focused);
                scene.layers.push(row);
            }
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
            let input = self.quick_input.bounds;
            let value = self.quick_input.buffer.text();
            let show_prompt = value.is_empty()
                || (self.overlay == Some(Overlay::Goto)
                    && value.len_bytes() == 1
                    && matches!(value.chars().next(), Some(':' | '@')));
            if show_prompt {
                let mut prompt_style = input_style.clone();
                prompt_style.foreground[3] *= 0.65;
                let pad = if self.quick_input.buffer.text().is_empty() {
                    10.
                } else {
                    22.
                } * self.ui_scale;
                theme.text_fit(
                    &mut overlay,
                    &prompt_style,
                    self.overlay_prompt(workspace),
                    input.x + pad,
                    input.y + input.height / 2. + 4. * self.ui_scale,
                    input.width - pad - 10. * self.ui_scale,
                    input,
                );
            }
            let rows = Rect {
                x: self.quick_bounds.x,
                y: self.quick_list_top(),
                width: self.quick_bounds.width,
                height: self.quick.scroll.viewport,
            };
            let s = self.ui_scale;
            if self.quick_ready && self.quick.items.is_empty() && !show_prompt {
                let style = self
                    .tree
                    .style(1, Some("label_control"), &mut theme, settings);
                theme.text_fit(
                    &mut overlay,
                    &style,
                    &workspace.ui.no_matches,
                    rows.x + 12. * s,
                    rows.y + 20. * s,
                    rows.width - 24. * s,
                    rows,
                );
            }
            // Appearance previews have their own row layout; other lists reserve the badge lane.
            let compact = self.quick_compact();
            let badges = self.overlay != Some(Overlay::Appearance);
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
                let annotation = &item.annotation;
                let annotation_width = theme.advance(&hint, annotation);
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
                // A command caption that does not fit keeps its last segment ("…：上",
                // "…: UTF-8"): it is what tells sibling rows apart.
                theme.text_fit_highlighted_keeping(
                    &mut overlay,
                    &caption,
                    &item.caption,
                    &item.highlights,
                    if self.overlay == Some(Overlay::Palette) {
                        &["：", ": "]
                    } else {
                        &[]
                    },
                    text_x,
                    baseline,
                    caption_end - text_x,
                    vel_ui::minihtml::intersection(
                        rect,
                        Rect {
                            width: (caption_end - rows.x).max(0.),
                            ..rows
                        },
                    ),
                );
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
                if badges && let ItemIcon::Symbol(symbol) = item.icon {
                    let (kind_class, letter) = quick_icons::symbol_style(symbol);
                    let kind = self.tree.style(id, Some(kind_class), &mut theme, settings);
                    theme.surface(&mut overlay, &kind, badge, rows);
                    let label = theme.style(
                        StyleId::OverlayKindLabel.at(row as u64),
                        &vel_theme::ElementPath(&[
                            vel_theme::Element::new("window", &[]),
                            vel_theme::Element::new(kind_class, &[]),
                            vel_theme::Element::new("kind_label", &[]),
                        ]),
                        settings,
                    );
                    let width = theme.advance(&label, letter);
                    theme.text(
                        &mut overlay,
                        &label,
                        letter,
                        (badge.x + (badge.width - width) / 2.).round(),
                        badge.y + 13. * s,
                        vel_ui::minihtml::intersection(badge, rows),
                    );
                } else if badges {
                    quick_icons::paint(
                        workspace,
                        &theme,
                        &mut overlay,
                        item,
                        badge,
                        rows,
                        hint.foreground,
                    );
                }
                theme.text(
                    &mut overlay,
                    &hint,
                    annotation,
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
                    self.overlay_footer(workspace),
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
                    let dropdown = self.tab_dropdown_anchor.is_some();
                    let replace = dropdown && self.alternate_click && !extend;
                    let toggle = dropdown && self.primary_click && !extend;
                    let action = if dropdown {
                        AcceptTabs::Default
                    } else {
                        AcceptTabs::from_modifiers(self.primary_click, extend, self.alternate_click)
                    };
                    // Choosing a project with Command opens it in a new window (Quick Switch Project).
                    let new_window = self.overlay == Some(Overlay::Choices) && self.primary_click;
                    let mut invocation =
                        self.accept_item(workspace, action, new_window, false, viewport)?;
                    if let Some(invocation) = &mut invocation {
                        if replace {
                            invocation.args["_replace_focused"] = json!(true);
                        } else if toggle {
                            invocation.args["_toggle_selected"] = json!(true);
                        }
                    }
                    self.pending_command = invocation;
                }
            }
            return Ok(true);
        }
        if let Some(invocation) = self.review_mouse(point, down) {
            self.pending_command = invocation;
            return Ok(true);
        }
        self.tree.pointer(point, Some(down));
        if down
            && self.overlay.is_none()
            && self.status_visible()
            && contains(self.indentation_rect(), point)
        {
            self.indentation_choices(workspace);
            return Ok(true);
        }
        if down
            && self.overlay.is_none()
            && self.status_visible()
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
            && self.status_visible()
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
            let query_had_focus = self.panel_focused && self.focus_input == 0;
            self.panel_focused = true;
            self.focus.show(Surface::Panel, true);
            if down {
                if !self.show_console
                    && self.panel == Some(Panel::Output)
                    && let Some(input) = &mut self.build_input
                    && contains(input.bounds, point)
                {
                    self.side_focused = false;
                    input.click_at(point, extend);
                    return Ok(true);
                }
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
                    if !query_had_focus && contains(self.query.bounds, point) {
                        self.focus_input = 0;
                        self.refresh_find_pasteboard(workspace);
                    }
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
                        if let Some(option) = FIND_OPTIONS.get(id.wrapping_sub(30) as usize) {
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
                    if self.alternate_click && !extend {
                        workspace.replace_focused_sheet(row - 1);
                    } else {
                        workspace.click_sheet(row - 1, self.primary_click, extend);
                    }
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
                    } else if self.alternate_click
                        && let Some(row) = self.side_rows.get(self.side_selected)
                        && !row.directory
                    {
                        workspace.open_replacing_focused(FileLocation {
                            path: row.path.clone(),
                            line: 1,
                            column: 1,
                        })?;
                        self.side_anchor = self.side_selection.clone();
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
        Ok(self.status_visible() && point.y >= self.size.height - (STATUS * self.ui_scale))
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
    fn marked_field_underline_matches_text_ink_and_backing_scale() {
        let mut workspace = workspace();
        let mut inks = Vec::new();
        for dark in [false, true] {
            workspace.set_appearance(dark).unwrap();
            let mut theme = workspace.themes.ui.borrow_mut();
            let style = theme.style(
                StyleId::SettingsInput.at(1),
                &vel_theme::ElementPath(&[
                    vel_theme::Element::new("window", &[]),
                    vel_theme::Element::new("panel_control find_panel", &[]),
                    vel_theme::Element::new("text_input", &["focused"]),
                ]),
                &workspace.editor().settings,
            );
            inks.push(style.foreground);
            for density in [1., 2.] {
                theme.density = density;
                for multiline in [false, true] {
                    let mut input = Input::new("");
                    input.set_multiline(multiline);
                    input.scale = 1.125;
                    input.set_row_pitch(18. * input.scale);
                    input.bounds = Rect {
                        x: 10.,
                        y: 9.25,
                        width: 240.,
                        height: if multiline { 66. } else { 32. },
                    };
                    let preedit = if multiline { "ni\nhao" } else { "ni hao" };
                    input.text_input(TextInput::Marked {
                        text: preedit.into(),
                        selected: vel_platform::Utf16Range::new(preedit.len(), 0),
                        replacement: None,
                    });
                    let mut layer = Layer::default();
                    paint_themed_input(&theme, &style, &mut layer, &mut input, true);
                    assert_eq!(layer.underlines.len(), if multiline { 2 } else { 1 });
                    for (stroke, run) in layer.underlines.iter().zip(&layer.text) {
                        assert_eq!(stroke.color, style.foreground);
                        assert_eq!(stroke.color, run.color);
                        assert_eq!(stroke.rect.height * f64::from(density), f64::from(density));
                        assert_eq!(stroke.thickness * density, density);
                        assert_eq!((stroke.rect.y * f64::from(density)).fract(), 0.);
                        assert_eq!(stroke.clip, input.bounds);
                        assert!((stroke.rect.x - f64::from(run.x)).abs() < 0.02);
                        assert!(stroke.rect.width > 0.);
                    }
                }
            }
        }
        assert_ne!(inks[0], inks[1]);
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
            .resolve_key(workspace, &crate::test_keys::parse(key), 0..11)
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
        use crate::test_keys::chord;

        let mut workspace = workspace();
        let mut panels = Panels::new(&workspace, false);
        let find = chord("super+f", "ctrl+f");
        let replace = chord("super+alt+f", "ctrl+h");
        assert_eq!(
            press(&mut panels, &mut workspace, find).as_deref(),
            Some("show_panel")
        );
        assert_eq!(panels.panel, Some(Panel::Find));
        panels.query.set("cat");
        panels.input_changed(&mut workspace, 0..11);
        wait_for_matches(&mut panels, &mut workspace);
        assert_eq!(panels.visible_highlights().len(), 3);
        // Pressing the shortcut again closes the panel and hides the matches.
        press(&mut panels, &mut workspace, find);
        assert_eq!(panels.panel, None);
        assert!(panels.visible_highlights().is_empty());
        // Background results and re-validation do not bring the highlights back while closed.
        panels.poll(&mut workspace, 0..11);
        assert!(panels.visible_highlights().is_empty());
        // The search itself survives. Reset the caret so this checks navigation independently
        // of the new typing-preview selection.
        workspace.editor_mut().set_caret(0, false);
        assert_eq!(
            press(&mut panels, &mut workspace, chord("super+g", "f3")).as_deref(),
            Some("find_next")
        );
        assert_eq!(workspace.editor().selection(), 0..3);
        assert!(panels.visible_highlights().is_empty());
        // Reopening keeps the query and shows the matches again.
        press(&mut panels, &mut workspace, find);
        assert_eq!(panels.panel, Some(Panel::Find));
        assert_eq!(panels.query.value(), "cat");
        wait_for_matches(&mut panels, &mut workspace);
        assert_eq!(panels.visible_highlights().len(), 3);
        // A different find-family shortcut switches panels instead of closing.
        press(&mut panels, &mut workspace, replace);
        assert_eq!(panels.panel, Some(Panel::Replace));
        press(&mut panels, &mut workspace, replace);
        assert_eq!(panels.panel, None);
        // Escape with keyboard focus in the editor closes a visible panel through the keymap.
        press(&mut panels, &mut workspace, find);
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
            .preview_goto(FileLocation::path_only(b), 0)
            .unwrap();
        panels
            .key(&mut workspace, &Key::parse("escape").unwrap(), 0..1)
            .unwrap();
        assert_eq!(workspace.editor().buffer.id(), source);
        assert!(workspace.tabs[workspace.active].transient);
        assert_eq!(panels.restore_scroll, Some((12., 80.)));
    }
    #[test]
    fn r41_project_symbol_and_definition_locations_use_tabless_previews() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let mut w = workspace();
        let mut p = Panels::new(&w, false);
        let source = w.tabs[w.active].sheet;
        for (overlay, file, exit) in [
            (Overlay::ProjectSymbols, "symbol.rs", "escape"),
            (Overlay::Locations, "definition.rs", "enter"),
        ] {
            let file = root.join(file);
            std::fs::write(&file, "fn target() {}\n").unwrap();
            p.action(&mut w, "show_overlay", &json!({}), 0..0).unwrap();
            p.overlay = Some(overlay);
            p.fuzzy_worker.cancel();
            p.quick_pending = false;
            p.locations = vec![vel_index::SymbolLocation {
                file: file.clone(),
                name: "target".into(),
                row: 0,
                column: 3,
                range: 3..9,
                kind: vel_index::SymbolKind::Function,
            }];
            p.location_items(&w);
            p.preview_current(&mut w).unwrap();
            let sheet = w.tabs[w.active].sheet;
            assert!(w.tabs[w.active].goto_preview);
            assert_eq!(w.workbench.groups[0].sheets, [source]);
            assert_eq!(w.editor().caret, 3);
            p.key(&mut w, &Key::parse(exit).unwrap(), 0..0).unwrap();
            assert!(p.overlay.is_none());
            if exit == "escape" {
                assert_eq!(w.tabs[w.active].sheet, source);
                assert!(w.sheet_index(sheet).is_none());
            } else {
                assert_eq!(w.workbench.groups[0].sheets, [source, sheet]);
                let tab = &w.tabs[w.active];
                assert!(!tab.goto_preview && !tab.transient);
                assert_eq!(tab.editor.caret, 3);
            }
        }
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
                    .find(|r| f64::from(r.baseline) > status_top && f64::from(r.x) < leftmost)
                    .expect("position text");
                let switcher = panels.status_switcher_rect(&workspace).unwrap();
                assert!(
                    position.clip.x >= switcher.x + switcher.width
                        && position.clip.x + position.clip.width <= leftmost,
                    "{width} @{scale}: position text runs under another control"
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
    /// A find field holding several hard rows grows to four rows and no further; past that
    /// it scrolls, with the rest of the text out of sight behind a scroll indicator.
    #[test]
    fn multi_line_find_fields_grow_to_four_rows_then_scroll() {
        for ui in [1., 2.] {
            let mut workspace = workspace();
            workspace
                .settings
                .set(vel_config::settings::Layer::User, "ui_scale", json!(ui));
            let mut panels = Panels::new(&workspace, true);
            panels
                .action(
                    &mut workspace,
                    "show_panel",
                    &json!({"panel":"find"}),
                    0..11,
                )
                .unwrap();
            panels.layout(Size {
                width: 900.,
                height: 600.,
            });
            let ui = panels.ui_scale;
            let empty = panels.query.bounds.height;
            assert_eq!(empty, 28. * ui, "{ui}");
            // Six hard rows: the field stops at four.
            panels.query.set("one\ntwo\nthree\nfour\nfive\nsix");
            panels.layout(Size {
                width: 900.,
                height: 600.,
            });
            assert_eq!(panels.query.bounds.height, (28. + 3. * 18.) * ui, "{ui}");
            let mut scene = Scene::default();
            panels.paint(&workspace, &mut scene);
            let field = panels.query.bounds;
            let inside = |x: f64, y: f64| {
                x >= field.x
                    && x < field.x + field.width
                    && y > field.y
                    && y < field.y + field.height
            };
            let rows: Vec<String> = scene
                .layers
                .iter()
                .flat_map(|l| &l.text)
                .filter(|r| inside(f64::from(r.x), f64::from(r.baseline)))
                .map(|r| r.text.clone())
                .filter(|text| !text.is_empty())
                .collect();
            assert_eq!(rows.len(), 4, "{ui}: {rows:?}");
            // The indicator sits inside the field's right edge and is shorter than it.
            let (thumb, _bar) = scene
                .layers
                .iter()
                .flat_map(|l| &l.rects)
                .filter_map(|r| {
                    (r.rect.width <= 2. * ui + 0.01
                        && r.rect.height >= 16. * ui
                        && r.rect.x + 4. * ui >= field.x + field.width
                        && r.rect.x <= field.x + field.width)
                        .then_some(r.rect)
                })
                .fold((None, 0.), |(thumb, tall): (Option<Rect>, f64), rect| {
                    if rect.height < field.height {
                        (thumb.or(Some(rect)), tall.max(rect.height))
                    } else {
                        (thumb, tall.max(rect.height))
                    }
                });
            let thumb = thumb.unwrap_or_else(|| panic!("{ui}: no scroll indicator"));
            assert!(thumb.height < field.height);
            assert!(thumb.y >= field.y && thumb.y + thumb.height <= field.y + field.height);
            // Fewer rows than fit: no field growth and no indicator.
            panels.query.set("one\ntwo");
            panels.layout(Size {
                width: 900.,
                height: 600.,
            });
            assert_eq!(panels.query.bounds.height, (28. + 18.) * ui, "{ui}");
            let mut scene = Scene::default();
            panels.paint(&workspace, &mut scene);
            let field = panels.query.bounds;
            assert!(
                scene.layers.iter().flat_map(|l| &l.rects).all(|r| {
                    !(r.rect.width <= 2. * ui + 0.01
                        && r.rect.height >= 16. * ui
                        && r.rect.x + 4. * ui >= field.x + field.width
                        && r.rect.x <= field.x + field.width)
                }),
                "{ui}: indicator without overflow"
            );
        }
    }
    /// The whole fitted run ends with an unhighlighted ellipsis and stays in the caption column.
    #[test]
    fn long_matched_captions_end_in_an_ellipsis() {
        let workspace = workspace();
        let caption = "src/render/an_unusually_long_module_name_for_truncation_checks.rs";
        let highlights: Vec<usize> = caption
            .char_indices()
            .filter(|(_, c)| *c == 'r' || *c == 's')
            .map(|(i, _)| i)
            .collect();
        let mut cut = 0;
        for width in (400..=1100).map(f64::from) {
            let mut panels = Panels::new(&workspace, false);
            panels.side_visible = false;
            panels.overlay = Some(Overlay::Goto);
            panels.quick.set_items(vec![vel_ui::Item {
                caption: caption.into(),
                icon: ItemIcon::File,
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
            // The caption is one shaped run, cut with an ellipsis; each matched byte in
            // the visible text is highlighted once, and no run leaves its column.
            let mut runs = scene
                .layers
                .iter()
                .flat_map(|l| &l.text)
                .filter(|r| r.highlight.is_some());
            let run = runs.next().unwrap_or_else(|| panic!("{width}: no caption"));
            assert!(runs.next().is_none(), "{width}: caption split into runs");
            let prefix = run.text.strip_suffix('\u{2026}').unwrap_or(&run.text);
            assert!(caption.starts_with(prefix), "{width}: {:?}", run.text);
            let ranges = run.highlight.as_ref().map_or_else(Vec::new, |h| {
                h.ranges.iter().flat_map(|r| r.clone()).collect()
            });
            assert_eq!(
                ranges,
                highlights
                    .iter()
                    .copied()
                    .filter(|&at| at < prefix.len())
                    .collect::<Vec<_>>(),
                "{width}: only the visible matched text is highlighted"
            );
            let metrics = workspace.themes.ui.borrow().metrics.clone();
            let end = f64::from(run.x) + run_width(&*metrics, run);
            assert!(
                end <= run.clip.x + run.clip.width,
                "{width}: {:?} ends at {end}, clip {:?}",
                run.text,
                run.clip
            );
            if prefix.len() < caption.len() {
                cut += 1;
            }
        }
        assert!(cut > 100, "{cut}");
    }
    /// An English alias hit highlights the whole translated caption only when its matched
    /// letters start words; letters scattered through other words do not.
    #[test]
    fn alias_hits_must_start_words() {
        let at = |label: &str, parts: &[&str]| -> Vec<usize> {
            // Byte offsets of each part's characters, searched left to right.
            let mut from = 0;
            let mut out = Vec::new();
            for part in parts {
                let start = from + label[from..].find(part).expect(part);
                out.extend(part.char_indices().map(|(i, _)| start + i));
                from = start + part.len();
            }
            out
        };
        for (label, parts) in [
            ("File: Save", &["Save"][..]),
            ("File: Save All", &["Sa"][..]),
            ("File: Save with Encoding: UTF-8", &["Save", "Enc"][..]),
            ("File: Save with Encoding: UTF-8", &["UTF", "8"][..]),
            ("Goto: Bookmarks: Clear", &["G", "B", "C"][..]),
            ("Preferences: setUiLanguage", &["Ui", "Lang"][..]),
        ] {
            assert!(
                alias_matches_word_starts(label, &at(label, parts)),
                "{label} {parts:?}"
            );
        }
        for (label, parts) in [
            ("Preferences: Customize Theme", &["r", "u", "st"][..]),
            ("Previous Modification", &["r", "u", "s", "t"][..]),
            ("File: Save", &["S", "v"][..]),
            ("File: Save", &["ave"][..]),
        ] {
            assert!(
                !alias_matches_word_starts(label, &at(label, parts)),
                "{label} {parts:?}"
            );
        }
        assert!(!alias_matches_word_starts("File: Save", &[]));
        assert!(!alias_matches_word_starts("File: Save", &[99]));
        assert!(!alias_matches_word_starts("文件", &[1]));
    }
    /// In the Chinese palette an English query that finds a row through its English caption
    /// highlights the whole Chinese caption when the query starts words there ("save" →
    /// 保存), and nothing when it only hits scattered letters ("rust" in "Preferences:
    /// Customize Theme" → 偏好设置：自定主题); direct hits keep their own positions (主题).
    #[test]
    fn chinese_palette_highlights_only_word_aligned_alias_hits() {
        let mut workspace = workspace();
        workspace.settings.set(
            vel_config::settings::Layer::User,
            "ui_language",
            json!("zh-Hans"),
        );
        assert!(workspace.sync_ui_language());
        let mut panels = Panels::new(&workspace, false);
        let mut rows = |query: &str| {
            panels
                .action(
                    &mut workspace,
                    "show_overlay",
                    &json!({"overlay": "command_palette", "text": query}),
                    0..10,
                )
                .unwrap();
            let until = Instant::now() + Duration::from_secs(15);
            while panels.ranking_pending() {
                assert!(Instant::now() < until, "no palette results for {query}");
                workspace.poll();
                panels.poll(&mut workspace, 0..10);
                std::thread::sleep(Duration::from_millis(2));
            }
            panels
                .quick
                .items
                .iter()
                .map(|item| (item.caption.clone(), item.highlights.clone()))
                .collect::<Vec<_>>()
        };
        let whole = |caption: &str| caption.char_indices().map(|(i, _)| i).collect::<Vec<_>>();
        let assert_match_tiers = |rows: &[(String, Vec<usize>)]| {
            let weak = rows.iter().position(|(_, marks)| marks.is_empty()).unwrap();
            assert!(weak > 0);
            assert!(
                rows[weak..].iter().all(|(_, marks)| marks.is_empty()),
                "preferred matches below a scattered alias: {rows:?}"
            );
        };
        let save = rows("save");
        let hit = save
            .iter()
            .find(|(caption, _)| caption == "文件：保存")
            .expect("文件：保存 for save");
        assert_eq!(hit.1, whole("文件：保存"));
        assert!(save.iter().any(|(caption, marks)| {
            caption == "文件：全部保存" && *marks == whole(caption)
        }));
        assert_match_tiers(&save);
        let rust = rows("rust");
        assert!(rust[0].0.contains("Rust"));
        for caption in ["偏好设置：自定主题…", "偏好设置：自定配色方案…"] {
            let weak = rust
                .iter()
                .find(|(label, _)| label == caption)
                .expect(caption);
            assert!(weak.1.is_empty(), "{weak:?}");
        }
        assert_match_tiers(&rust);
        // A direct hit on the shown caption keeps its precise highlight.
        let theme = rows("主题");
        let direct = theme
            .iter()
            .find(|(caption, _)| caption == "偏好设置：自定主题…")
            .expect("a direct hit for 主题");
        let start = direct.0.find("主题").unwrap();
        assert_eq!(direct.1, [start, start + "主".len()]);
    }
    /// The input panel's caption is free text: its column follows the measured caption, so the
    /// caption ends before the field instead of running under it, a caption longer than half
    /// the panel ends in an ellipsis, and the panel is one row tall — at UI scale 1 and 2.
    #[test]
    fn input_panel_captions_end_before_their_field() {
        for ui in [1., 2.] {
            let mut workspace = workspace();
            workspace
                .settings
                .set(vel_config::settings::Layer::User, "ui_scale", json!(ui));
            for (width, caption) in [
                (900., "New name for this entry"),
                (900., "此项的新名称"),
                (900., ":"),
                (900., "Goto byte offset (decimal or 0x hex)"),
                (420., "HTTPS URL of a .sublime-package archive"),
                (420., "此项的新名称（不能包含斜杠）"),
            ] {
                let mut panels = Panels::new(&workspace, true);
                panels
                    .action(
                        &mut workspace,
                        "show_input_panel",
                        &json!({"caption": caption, "initial_text": "说明.md"}),
                        0..10,
                    )
                    .unwrap();
                panels.layout(Size {
                    width,
                    height: 600.,
                });
                let mut scene = Scene::default();
                panels.paint(&workspace, &mut scene);
                let s = panels.ui_scale;
                assert_eq!(panels.panel_bounds.height, 44. * s, "{ui} {caption}");
                let field = panels.query.bounds;
                // The field keeps at least half the panel and the find panels' margins.
                assert!(field.width >= (panels.panel_bounds.width - 12. * s) / 2. - 0.01);
                assert_eq!(field.y, panels.panel_bounds.y + 8. * s);
                let metrics = workspace.themes.ui.borrow().metrics.clone();
                let run = scene
                    .layers
                    .iter()
                    .flat_map(|l| &l.text)
                    .find(|r| {
                        let shown = r.text.trim_end_matches('…');
                        !shown.is_empty()
                            && caption.starts_with(shown)
                            && f64::from(r.x) < field.x
                            && f64::from(r.baseline) > field.y
                            && f64::from(r.baseline) < field.y + field.height
                    })
                    .unwrap_or_else(|| panic!("{ui} {width} {caption}: no caption"));
                let end = f64::from(run.x) + run_width(&*metrics, run);
                assert!(
                    end <= field.x - 8. * s + 0.01,
                    "{ui} {width} {caption}: caption ends at {end}, field at {}",
                    field.x
                );
                // Short captions line up with the find panels' labels; the rest is whole
                // unless it needs more than half the panel.
                if caption == ":" {
                    assert_eq!(field.x - panels.panel_bounds.x, 80. * s);
                }
                let room = (panels.panel_bounds.width - 12. * s) / 2. - 16. * s;
                let needed = run_width(
                    &*metrics,
                    &vel_render::scene::TextRun {
                        text: caption.into(),
                        ..run.clone()
                    },
                );
                if needed <= room {
                    assert_eq!(run.text, caption, "{ui} {width}");
                } else {
                    assert!(run.text.ends_with('…'), "{ui} {width}: {}", run.text);
                }
            }
        }
    }
    /// Command rows that differ only in their last segment stay distinguishable when a narrow
    /// palette cuts them: each keeps "：上" / ": Up" and shortens the part before it, in
    /// Chinese and English, with and without a shortcut hint. Goto rows are cut at the end.
    #[test]
    fn narrow_palette_rows_keep_their_last_segment() {
        let workspace = workspace();
        let rows = |prefix: &str, separator: &str, ends: [&str; 4]| -> Vec<String> {
            ends.iter()
                .map(|end| format!("{prefix}{separator}{end}"))
                .collect()
        };
        for (captions, separator) in [
            (
                rows(
                    "显示：编辑组：将所选选项卡移至相邻编辑组",
                    "：",
                    ["上", "下", "左", "右"],
                ),
                "：",
            ),
            (
                rows(
                    "View: Groups: Move Selection to Neighboring Group",
                    ": ",
                    ["Up", "Down", "Left", "Right"],
                ),
                ": ",
            ),
        ] {
            for width in [300., 360., 440.] {
                let mut panels = Panels::new(&workspace, false);
                panels.side_visible = false;
                panels.overlay = Some(Overlay::Palette);
                panels.quick.set_items(
                    captions
                        .iter()
                        .enumerate()
                        .map(|(i, caption)| vel_ui::Item {
                            caption: caption.clone(),
                            icon: ItemIcon::Neutral,
                            annotation: if i % 2 == 0 {
                                "⌃⇧1".into()
                            } else {
                                String::new()
                            },
                            ..Default::default()
                        })
                        .collect(),
                );
                panels.layout(Size {
                    width,
                    height: 400.,
                });
                let mut scene = Scene::default();
                panels.paint(&workspace, &mut scene);
                let metrics = workspace.themes.ui.borrow().metrics.clone();
                let mut shown = Vec::new();
                for caption in &captions {
                    let end = &caption[caption.rfind(separator).unwrap()..];
                    let run = scene
                        .layers
                        .iter()
                        .flat_map(|l| &l.text)
                        .find(|r| {
                            r.text == *caption || (r.text.contains('…') && r.text.ends_with(end))
                        })
                        .unwrap_or_else(|| panic!("{width}: {caption} lost {end}"));
                    let right = f64::from(run.x) + run_width(&*metrics, run);
                    assert!(
                        right <= run.clip.x + run.clip.width + 0.01,
                        "{width}: {}",
                        run.text
                    );
                    shown.push(run.text.clone());
                }
                assert!(
                    shown.iter().any(|s| s.contains('…')),
                    "{width}: nothing was cut"
                );
                shown.sort();
                shown.dedup();
                assert_eq!(shown.len(), 4, "{width}: {shown:?}");
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
            panels.open_files_visible = true;
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

    #[test]
    fn find_in_files_gitignore_controls_disk_walk_not_open_files_and_invalidates_replace() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("nested")).unwrap();
        std::fs::write(root.join(".gitignore"), "*.txt\n!keep.txt\n").unwrap();
        std::fs::write(root.join("nested/.gitignore"), "drop.rs\n").unwrap();
        for path in ["hidden.txt", "keep.txt", "nested/drop.rs", "nested/show.rs"] {
            std::fs::write(root.join(path), "haystack\nunique needle\n").unwrap();
        }
        let mut workspace = Workspace::new(vel_text::Text::new(), None, None, false).unwrap();
        let mut panels = Panels::new(&workspace, false);
        panels.add_folder(&mut workspace, root.clone()).unwrap();
        for (use_gitignore, where_, expected) in [
            (false, "<project>", 4),
            (true, "<project>", 2),
            (true, "nested/", 1),
        ] {
            panels.action(&mut workspace, "show_panel", &json!({
                "panel": "find_in_files", "pattern": "needle", "use_gitignore": use_gitignore
            }), 0..10).unwrap();
            let scope = if where_ == "nested/" {
                root.join("nested").display().to_string()
            } else {
                where_.into()
            };
            panels.where_input.set(&scope);
            panels
                .action(&mut workspace, "find_in_files_start", &json!({}), 0..10)
                .unwrap();
            wait(&mut workspace, &mut panels, |_, p| p.file_complete);
            assert_eq!(panels.file_matches, expected);
        }
        assert!(panels.file_replace_ready(&workspace));
        panels
            .action(
                &mut workspace,
                "show_panel",
                &json!({"panel": "find_in_files"}),
                0..10,
            )
            .unwrap();
        panels
            .action(
                &mut workspace,
                "toggle_find_option",
                &json!({"option": "use_gitignore"}),
                0..10,
            )
            .unwrap();
        assert!(!panels.file_replace_ready(&workspace));
        workspace
            .open(FileLocation {
                path: root.join("hidden.txt"),
                line: 1,
                column: 1,
            })
            .unwrap();
        panels
            .action(
                &mut workspace,
                "show_panel",
                &json!({"panel":"find_in_files","pattern":"needle","use_gitignore":true}),
                0..10,
            )
            .unwrap();
        panels.where_input.set("<open files>");
        panels
            .action(&mut workspace, "find_in_files_start", &json!({}), 0..10)
            .unwrap();
        wait(&mut workspace, &mut panels, |_, p| p.file_complete);
        assert_eq!(panels.file_matches, 1);
    }

    #[test]
    #[cfg(windows)]
    fn goto_open_tab_label_hides_verbatim_prefix_but_keeps_io_path() {
        let mut workspace = Workspace::new(vel_text::Text::new(), None, None, false).unwrap();
        let path = PathBuf::from(r"\\?\C:\fixture path\open.txt");
        workspace.tabs[0].path = Some(path.clone());
        let mut panels = Panels::new(&workspace, false);
        panels
            .action(
                &mut workspace,
                "show_overlay",
                &json!({"overlay":"goto"}),
                0..10,
            )
            .unwrap();
        assert_eq!(
            panels.extra_candidates[0].label,
            r"C:\fixture path\open.txt"
        );
        assert_eq!(panels.extra_paths, [path]);
    }

    #[test]
    fn r43_goto_publishes_relative_details_and_keeps_outside_paths() {
        let folder = |name: &str| Folder {
            path: Path::new("/project-fixtures").join(name),
            name: name.into(),
            rules: Rules::new(&[], &[], &[]).unwrap(),
            follow_links: true,
        };
        let mut workspace = Workspace::new(vel_text::Text::new(), None, None, false).unwrap();
        let mut panels = Panels::new(&workspace, false);
        workspace.folders.push(folder("first"));
        panels.overlay = Some(Overlay::Goto);
        let outside = "/project-fixtures/outside.md";
        panels.paths = Arc::new(vec![
            workspace.folders[0].path.join("zh.md"),
            workspace.folders[0].path.join("sub/dir/child.md"),
        ]);
        panels.candidates = Arc::new(
            panels
                .paths
                .iter()
                .map(|path| fuzzy::Candidate::new(path.to_string_lossy()))
                .collect(),
        );
        panels.extra_paths = vec![outside.into()];
        panels.extra_candidates = Arc::new(vec![fuzzy::Candidate::new(outside)]);

        let hit = |index| fuzzy::Hit {
            index,
            score: 0,
            positions: Vec::new(),
        };
        let mut hits = (0..3).map(hit).collect::<Vec<_>>();
        panels.publish_quick(&workspace, hits.clone());
        let details = |panels: &Panels| {
            panels
                .quick
                .items
                .iter()
                .map(|item| item.details.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(details(&panels), ["zh.md", "sub/dir/child.md", outside]);

        workspace.folders.push(folder("second"));
        panels
            .extra_paths
            .push(workspace.folders[1].path.join("zh.md"));
        Arc::make_mut(&mut panels.extra_candidates).push(fuzzy::Candidate::new("zh.md"));
        hits.push(hit(3));
        panels.publish_quick(&workspace, hits);
        assert_eq!(
            details(&panels),
            [
                "first/zh.md",
                "first/sub/dir/child.md",
                outside,
                "second/zh.md"
            ]
        );
    }

    #[test]
    #[cfg(unix)]
    fn r43_goto_detail_abbreviates_home_on_component_boundaries() {
        let home = Some(Path::new("/Users/fixture"));
        assert_eq!(
            Panels::goto_path_detail(Path::new("/Users/fixture/notes/zh.md"), &[], home),
            "~/notes/zh.md"
        );
        let shared = "/Users/fixture-share/notes/zh.md";
        assert_eq!(
            Panels::goto_path_detail(Path::new(shared), &[], home),
            shared
        );
    }

    #[test]
    fn goto_anything_gitignore_is_opt_in_and_cannot_readd_ignored_open_tabs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join(".gitignore"), "excluded*\n").unwrap();
        for name in ["excluded.rs", "included.rs"] {
            std::fs::write(root.join(name), "fn fun() {}\n").unwrap();
        }
        let mut workspace = Workspace::new(vel_text::Text::new(), None, None, false).unwrap();
        let mut panels = Panels::new(&workspace, false);
        panels.add_folder(&mut workspace, root.clone()).unwrap();
        workspace
            .open(FileLocation {
                path: root.join("excluded.rs"),
                line: 1,
                column: 1,
            })
            .unwrap();
        wait(&mut workspace, &mut panels, |_, p| {
            p.file_paths.contains(&root.join("excluded.rs"))
        });
        panels
            .action(
                &mut workspace,
                "show_overlay",
                &json!({"overlay":"goto", "text":"included"}),
                0..10,
            )
            .unwrap();
        assert!(panels.paths.contains(&root.join("excluded.rs")));
        workspace.settings.set(
            vel_config::settings::Layer::User,
            "goto_anything_exclude_gitignore",
            json!(true),
        );
        wait(&mut workspace, &mut panels, |_, p| {
            p.prepared == "git-files"
        });
        assert!(panels.paths.contains(&root.join("included.rs")));
        assert!(!panels.paths.contains(&root.join("excluded.rs")));
        assert!(!panels.extra_paths.contains(&root.join("excluded.rs")));
        assert!(panels.file_paths.contains(&root.join("excluded.rs")));
    }
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
        assert_eq!(
            panels.quick.items[0].icon,
            ItemIcon::Symbol(vel_ui::SymbolBadge::Function)
        );
    }
    /// Project symbol rows carry the symbol's kind (function, type, ...) in its own badge
    /// class and letter, not the debug name of an internal enum.
    #[test]
    fn project_symbol_rows_badge_their_kind() {
        let (_root, _data, mut w, mut p) = setup();
        p.layout(Size {
            width: 900.,
            height: 600.,
        });
        p.action(&mut w, "goto_symbol_in_project", &json!({}), 0..100)
            .unwrap();
        p.quick_input.set("uniq");
        p.query_quick(&mut w);
        wait(&mut w, &mut p, |_, p| {
            !p.quick_pending && !p.quick.items.is_empty()
        });
        assert_eq!(
            p.quick.items[0].icon,
            ItemIcon::Symbol(vel_ui::SymbolBadge::Function)
        );
        p.layout(Size {
            width: 900.,
            height: 600.,
        });
        let mut scene = Scene::default();
        p.paint(&w, &mut scene);
        // The badge letter is the kind's (lowercase, like file kinds), in the function
        // kind's own ink.
        let mut ui = w.themes.ui.borrow_mut();
        let function = ui.style(
            0,
            &vel_theme::ElementPath(&[
                vel_theme::Element::new("window", &[]),
                vel_theme::Element::new("kind_container kind_function", &[]),
                vel_theme::Element::new("kind_label", &[]),
            ]),
            &w.editor().settings,
        );
        drop(ui);
        assert!(
            scene
                .layers
                .iter()
                .flat_map(|l| &l.text)
                .any(|r| r.text == "f" && r.color == function.foreground),
            "no function kind letter"
        );
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
    /// Tool tip for a status or find-panel control, or side-bar Git badge.
    fn tooltip_text(&self, workspace: &Workspace, point: Point) -> Option<String> {
        if self
            .status_switcher_rect(workspace)
            .is_some_and(|r| contains(r, point))
        {
            Some(workspace.translations.tr("Sidebar and Panels").into_owned())
        } else if let Some(tip) = self.panel_tooltip(workspace, point) {
            Some(tip)
        } else if self.side_visible && point.x >= self.side_bounds.x + self.side_bounds.width - 32.
        {
            self.review_side_path(workspace, point)
                .and_then(|(path, folder)| workspace.git.badge(&path, folder))
                .map(|b| crate::git_ui::badge_tooltip(b, &workspace.translations).into_owned())
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
            let control = self.tree.hit_test(*point);
            control.hash(&mut h);
            workspace.translations.revision().hash(&mut h);
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

pub(crate) fn paint_marked_underline(
    layer: &mut Layer,
    clip: Rect,
    [left, right]: [f64; 2],
    y: f64,
    density: f64,
    color: [f32; 4],
) {
    if left < right {
        layer.underlines.push(Underline {
            rect: Rect {
                x: left,
                y: (y * density).round() / density,
                width: right - left,
                height: 1.,
            },
            clip,
            color,
            thickness: 1.,
            squiggle: false,
            stippled: false,
        });
    }
}

pub(crate) fn paint_themed_input(
    theme: &vel_ui::theme::ThemeUi,
    style: &vel_ui::theme::Style,
    layer: &mut Layer,
    input: &mut Input,
    focused: bool,
) {
    input.configure_theme(style, theme.metrics.clone());
    let first = layer.rects.len();
    let text = layer.text.len();
    Panels::paint_input(layer, input, focused, style, f64::from(theme.density));
    layer.rects.remove(first);
    // A Velocity focus ring paints outside the field (outset is zero for Sublime themes).
    let ring = style.outset();
    let clip = Rect {
        x: input.bounds.x - ring,
        y: input.bounds.y - ring,
        width: input.bounds.width + 2. * ring,
        height: input.bounds.height + 2. * ring,
    };
    theme.surface(layer, style, input.bounds, clip);
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
        let tr = &workspace.translations;
        let heading = |source: &mut String, text: &str| {
            source.push_str("<h3>");
            source.push_str(&escape_html(text));
            source.push_str("</h3>");
        };
        let open_location = escape_html(&tr.tr("Open location"));
        for (i, location) in self.hover.iter().take(16).enumerate() {
            if i == 0 && self.hover_definitions > 0 {
                heading(&mut source, &tr.tr("Definitions"));
            }
            if i == self.hover_definitions {
                heading(&mut source, &tr.tr("References"));
            }
            source.push_str(&format!("<div><a href='definition:{i}' title='{open_location}'><b>{}</b></a> <code>{}:{}</code></div>",
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
#[cfg(any(target_os = "macos", all(windows, not(test))))]
pub(crate) use accessibility::ax_id;

#[cfg(test)]
#[path = "panels_lane_b_round18_tests.rs"]
mod lane_b_round18_tests;
