//! Native application handler and buffered benchmark observations.
use crate::app_scene_translate as translate_scene;
use crate::ui_text::{Dialog, UiText};
#[path = "app_a11y.rs"]
mod accessibility_adapter;
#[path = "app_clipboard.rs"]
mod clipboard;
#[path = "app_context.rs"]
mod context_adapter;
#[cfg(target_os = "macos")]
#[path = "app_finder.rs"]
mod finder;
#[cfg(test)]
#[path = "app_lane_c_round20_tests.rs"]
mod lane_c_round20_tests;
#[path = "app_mouse.rs"]
mod mouse_adapter;
#[cfg(target_os = "macos")]
#[path = "performance_bench.rs"]
pub(crate) mod performance_bench;
#[cfg(test)]
#[path = "app_pointer_tests.rs"]
mod pointer_tests;
#[path = "app_print.rs"]
mod printing;
#[cfg(target_os = "macos")]
#[path = "render_scenes.rs"]
pub(crate) mod render_scenes;
#[cfg(test)]
#[path = "app_round20_invisible_perf.rs"]
mod round20_invisible_perf;
#[cfg(test)]
#[path = "app_round21_lane_b_tests.rs"]
mod round21_lane_b_tests;
#[cfg(test)]
#[path = "app_round26_settings_tests.rs"]
mod round26_settings_tests;
#[cfg(test)]
#[path = "app_round32_lane_b_tests.rs"]
mod round32_lane_b_tests;
#[cfg(test)]
#[path = "app_round34_lane_b_tests.rs"]
mod round34_lane_b_tests;
#[cfg(test)]
#[path = "app_round35_lane_b_tests.rs"]
mod round35_lane_b_tests;
#[cfg(test)]
#[path = "app_round36_lane_c_tests.rs"]
mod round36_lane_c_tests;
#[cfg(test)]
#[path = "app_round41_tests.rs"]
mod round41_tests;
#[cfg(test)]
#[path = "app_round43_tests.rs"]
mod round43_tests;
#[path = "settings_entry.rs"]
mod settings_entry;
#[path = "app_settings_ui.rs"]
mod settings_ui_adapter;
#[path = "app_settings_window.rs"]
mod settings_window;
#[cfg(test)]
#[path = "app_startup_tests.rs"]
mod startup_tests;
#[path = "app_status.rs"]
pub(crate) mod status_adapter;
#[path = "app_tabs.rs"]
mod tab_adapter;
#[path = "app_tab_drag.rs"]
mod tab_drag;
#[path = "app_tab_rename.rs"]
mod tab_rename;
#[path = "app_terminal.rs"]
mod terminal;
#[cfg(test)]
#[path = "app_window_close_tests.rs"]
mod window_close_tests;
use crate::{
    bench::{Bench, SCROLL_FRAMES, TYPING_INSERTIONS},
    cli::{Mode, Options},
    report::{Clock, FirstScanout, Reporter, ms},
    stats::{summarize, sweep_positions},
    view::View,
    workspace::Workspace,
};
use serde::Serialize;
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use vel_config::Value;
#[cfg(test)]
use vel_config::settings::Layer;
use vel_platform::*;
use vel_render::scene::Layer as SceneLayer;
use vel_render::{FrameTimes, Renderer, font::Fonts};

type CommandState = (u64, vel_buffer::ChangeId, vel_buffer::Selection);
const WINDOW_COMMANDS: &[&str] = &[
    "refresh_folder_list",
    "close_folder_list",
    "toggle_show_open_files",
    "move_tab_to_window",
    "move_tab_to_new_window",
    "merge_all_windows",
    "convert_to_encoding",
    "show_console",
    "find_in_files_where",
    "new_snippet",
    "widget_history",
    "set_ui_language",
    "native_minimize",
    "native_zoom",
    "native_bring_all_to_front",
    "open_as_image",
    "open_as_text",
    "build_send_input",
    "build_close_input",
    "unselect_current",
    tab_rename::RENAME_TAB,
    "open_terminal",
    "enable_finder_new_file",
    "disable_finder_new_file",
];
fn image_open_as(name: &str) -> crate::open_requests::OpenAs {
    if name == "open_as_image" {
        crate::open_requests::OpenAs::Image
    } else {
        crate::open_requests::OpenAs::Text
    }
}
fn register_window_commands(editor: &mut crate::editor::Editor) {
    for &name in WINDOW_COMMANDS {
        if editor.registry.kind(name).is_none() {
            editor
                .registry
                .register(name, vel_commands::Kind::WindowCommand, None);
        }
    }
}
enum CommandStates {
    Single(CommandState),
    Multiple(Vec<CommandState>),
}
#[cfg(test)]
#[path = "menu_settings_i18n_tests.rs"]
mod menu_settings_i18n_tests;
impl CommandStates {
    fn iter(&self) -> std::slice::Iter<'_, CommandState> {
        match self {
            Self::Single(state) => std::slice::from_ref(state).iter(),
            Self::Multiple(states) => states.iter(),
        }
    }
}
#[derive(Clone, Debug, Serialize)]
struct Sample {
    index: usize,
    event_to_submit_ms: f64,
    event_to_present_return_ms: f64,
    acquire_ms: f64,
    drawable_limit: usize,
    encode_submit_ms: f64,
    present_call_ms: f64,
    timer_lateness_ms: Option<f64>,
}
type PendingCliCommand = (
    crate::cli::Named,
    std::collections::BTreeSet<u64>,
    Option<u64>,
);

#[derive(Debug)]
pub struct App {
    print_jobs: printing::Jobs,
    /// A playback may edit only the workbench sheet that started it, including nested chains.
    macro_origin: Option<u64>,
    opened_settings: Vec<App>,
    /// The Settings window opened by a window-less App (tests and the scene renderer).
    opened_preferences: Vec<App>,
    /// This App is the Settings window: no session state, routing or document commands.
    settings_window: bool,
    typing_hides_chrome: bool,
    revealed_directory: Option<std::path::PathBuf>,
    format_hits: Vec<(status_adapter::FormatItem, Rect)>,
    last_format_menu: Vec<MenuEntry>,
    #[cfg(test)]
    last_status_switcher_menu: Vec<MenuEntry>,
    #[cfg(test)]
    close_decisions: std::collections::VecDeque<tab_adapter::CloseDecision>,
    #[cfg(test)]
    overwrite_decisions: std::collections::VecDeque<bool>,
    #[cfg(test)]
    data_loss_dialogs: Vec<Dialog>,
    #[cfg(test)]
    open_file_choices: Option<Vec<PathBuf>>,
    pending_menu: Option<context_adapter::PendingMenu>,
    menu_cache: Option<((u64, u64, u64), Vec<MenuEntry>)>,
    /// Whether this window's menu is the one installed in the menu bar. Only the key window
    /// may install it; see `install_native_menu`.
    menu_owner: bool,
    pending_utf8_saves: std::collections::VecDeque<(u64, Option<std::path::PathBuf>, String)>,
    accessibility_focus: Option<accessibility::Id>,
    accessibility_windows: std::cell::RefCell<std::collections::BTreeMap<u64, usize>>,
    accessibility_preferences: accessibility::Preferences,
    releases: crate::release::Actions,
    title_theme: Option<(u64, u64)>,
    exact_scene: bool,
    exact_drawables: std::collections::BTreeSet<u64>,
    content_scanout: Option<Instant>,
    highlighted_scanout: Option<Instant>,
    language_warmed: bool,
    window_focused: bool,
    caret_observed: Option<CommandState>,
    caret_deadline: Option<Instant>,
    panels: crate::panels::Panels,
    window_size: Size,
    content: Rect,
    sheet_views: std::collections::BTreeMap<u64, View>,
    inactive_image_decodes: std::collections::BTreeSet<u64>,
    sheet_bounds: std::collections::BTreeMap<u64, Rect>,
    group_bounds: Vec<Rect>,
    workbench_bounds: Rect,
    native_pane_minimum: Option<(f64, f64)>,
    tab_hits: Vec<(u64, Rect)>,
    /// Tab bar of each group and its largest scroll offset, for clicks on its empty part and
    /// for clamping wheel scrolling.
    tab_bars: Vec<(usize, Rect, f64)>,
    tab_strips: Vec<tab_drag::TabStrip>,
    tab_painted: Vec<(u64, usize, f64)>,
    tab_press: Option<tab_drag::TabPress>,
    tab_slides: Option<tab_drag::TabSlides>,
    #[cfg(test)]
    torn_off_windows: Vec<App>,
    tab_button_hits: Vec<(usize, &'static str, Rect)>,
    absent_tab_buttons: Option<((usize, u64, u64), [bool; 3])>,
    keyboard_center: Option<u64>,
    key_context: Option<(u64, u64, bool, bool)>,
    visible_view_keys: Vec<(u64, u64, u64, usize)>,
    visible_view_scratch: Vec<(u64, u64, u64, usize)>,
    tab_close_hits: Vec<(u64, Rect)>,
    /// Where the last click selected a tab: revealing that tab may slide the strip under the
    /// pointer, and the second click of a double click still means the same tab.
    tab_click: Option<(Point, u64)>,
    tab_rename: Option<Box<tab_rename::TabRename>>,
    /// The last image status message ("W × H" or the decode failure note): it describes the
    /// document it was shown for and goes away with it.
    image_status: Option<String>,
    /// Pointer-hovered tab close button and tab-bar button, for their theme hover states.
    hovered_close: Option<u64>,
    hovered_tab_button: Option<(usize, &'static str)>,
    /// Per group, the focused sheet whose tab was last scrolled into view.
    revealed_tabs: Vec<Option<u64>>,
    dragging_splitter: Option<vel_ui::workbench::Splitter>,
    dragging_sidebar: Option<f64>,
    /// Text selection gesture between a press in the text area and its release.
    drag_select: Option<crate::mouse_select::DragSelect>,
    scrollbar_binding_button: Option<usize>,
    /// Selected text being dragged to a new position.
    drag_text: Option<crate::mouse_select::DragText>,
    text_drag_pressed: Option<Instant>,
    text_drag_origin: Option<(Point, usize)>,
    mouse_preferences: vel_platform::mouse::MousePreferences,
    builds: crate::build::Builds,
    fullscreen_before_distraction: bool,
    window_frame: Option<[f64; 4]>,
    #[cfg(test)]
    empty_close_requested: bool,
    sessions: std::rc::Rc<std::cell::RefCell<crate::session::Sessions>>,
    session_id: u64,
    session_due: Option<Instant>,
    restored_windows: Vec<(vel_session::Window, u64)>,
    waiters: Vec<crate::instance::Waiter>,
    cli_commands: std::collections::VecDeque<PendingCliCommand>,
    restore_safety: crate::safe_mode::Startup,
    skipped_recovery: Option<std::sync::mpsc::Receiver<vel_session::Recovery>>,
    view_buffer: Option<u64>,
    /// The hidden sheet ID is reused; its document identity invalidates the view on retarget.
    view_goto_preview: Option<(u64, u64)>,
    options: Options,
    clock: Clock,
    reporter: Reporter,
    workspace: Workspace,
    view: View,
    fonts: Option<Fonts>,
    font_face: String,
    renderer: Option<Renderer>,
    first_present: bool,
    startup_done: bool,
    startup_services_pending: bool,
    bench: Option<Bench>,
    samples: Vec<Sample>,
    retries: usize,
    input_at: Option<Instant>,
    bench_received: Option<Instant>,
    done: Arc<AtomicBool>,
    external_input: usize,
    first_scanout: FirstScanout,
    waiting_since: Option<Instant>,
}
/// Shown when an image file cannot be decoded and its bytes are shown instead.
#[cfg(test)]
pub(crate) const IMAGE_FALLBACK_NOTE: &str = "Not shown as an image: unsupported or damaged";

impl App {
    /// Restores retained documents before a temporary native receiver can write the profile.
    pub(crate) fn take_over_native_primary(
        &mut self,
        instance: crate::instance::Instance,
    ) -> Result<(), String> {
        let directory = self.workspace.data_path().map(Path::to_owned);
        let mut sessions = crate::session::Sessions::new(directory.as_deref());
        let restored = sessions.restored.clone();
        let reserved = sessions.reserve_recovered();
        let first = restored.windows.first().cloned();
        let project = first
            .as_ref()
            .and_then(|window| window.project.clone())
            .filter(|path| path.is_file());
        let mut workspace = Workspace::new_empty(directory, project, true)?;
        workspace.restore_original = sessions.original.clone();
        if let Some(window) = &first {
            workspace.restore_window(window, &restored.documents)?;
        }
        workspace.recent_files = restored.recent_files;
        workspace.recent_folders = restored.recent_folders;
        workspace.recent_projects = restored.recent_projects;
        workspace.messages.append(&mut sessions.messages);
        let id = reserved
            .first()
            .copied()
            .unwrap_or_else(|| sessions.register());
        sessions.instance = Some(instance);
        self.sessions = std::rc::Rc::new(std::cell::RefCell::new(sessions));
        self.session_id = id;
        self.workspace = workspace;
        self.panels.reset(&self.workspace);
        self.startup_services_pending = false;
        self.apply_ui_language_notice();
        self.panels.review.native_context = true;
        self.title_theme = None;
        self.view_buffer = None;
        self.view_goto_preview = None;
        self.sheet_views.clear();
        if let Some(window) = &first {
            self.restore_ui(window);
        }
        self.restore_remaining_reserved(restored.windows, reserved, first.as_ref().map(|_| 0));
        self.options.forward_native_opens = false;
        Ok(())
    }
    pub fn new(
        options: Options,
        clock: Clock,
        reporter: Reporter,
        mut workspace: Workspace,
        done: Arc<AtomicBool>,
        sessions: std::rc::Rc<std::cell::RefCell<crate::session::Sessions>>,
    ) -> Self {
        #[cfg(test)]
        crate::test_login::install();
        if workspace.restore_original.is_none() {
            workspace.restore_original = sessions.borrow().original.clone();
        }
        let choices = sessions.borrow().open_as_choices.clone();
        if !std::rc::Rc::ptr_eq(&choices, &workspace.open_as_choices) {
            choices
                .borrow_mut()
                .append(&mut workspace.open_as_choices.borrow_mut());
        }
        workspace.open_as_choices = choices;
        if options.mode == Mode::ThemeUi
            && let Err(e) = crate::theme_bench::demo(&mut workspace)
        {
            workspace.messages.push(e);
        }
        if options.mode == Mode::Typing {
            crate::install_carets(workspace.editor_mut(), options.carets);
        }
        for tab in &mut workspace.tabs {
            register_window_commands(&mut tab.editor);
            if tab.path.is_some() {
                tab.open_as_choices = Some(workspace.open_as_choices.clone());
            }
        }
        let view = View::new(
            Size {
                width: options.width as f64,
                height: options.height as f64,
            },
            8.,
            12.,
        );
        let startup_services_pending =
            workspace.startup_services_pending() || sessions.borrow().startup_writer_pending();
        let session_id = sessions.borrow_mut().register();
        Self {
            macro_origin: None,
            opened_settings: Vec::new(),
            opened_preferences: Vec::new(),
            settings_window: false,
            typing_hides_chrome: false,
            revealed_directory: None,
            format_hits: Vec::new(),
            last_format_menu: Vec::new(),
            #[cfg(test)]
            last_status_switcher_menu: Vec::new(),
            #[cfg(test)]
            close_decisions: Default::default(),
            #[cfg(test)]
            overwrite_decisions: Default::default(),
            #[cfg(test)]
            data_loss_dialogs: Vec::new(),
            #[cfg(test)]
            open_file_choices: None,
            pending_menu: None,
            menu_cache: None,
            menu_owner: false,
            pending_utf8_saves: Default::default(),
            accessibility_focus: None,
            accessibility_windows: Default::default(),
            accessibility_preferences: Default::default(),
            releases: Default::default(),
            title_theme: None,
            sessions,
            session_id,
            session_due: Some(Instant::now()),
            restored_windows: Vec::new(),
            waiters: Vec::new(),
            cli_commands: Default::default(),
            restore_safety: Default::default(),
            skipped_recovery: None,
            exact_scene: false,
            exact_drawables: Default::default(),
            content_scanout: None,
            highlighted_scanout: None,
            language_warmed: false,
            window_focused: true,
            caret_observed: None,
            caret_deadline: None,
            panels: {
                let mut panels =
                    crate::panels::Panels::new(&workspace, options.mode == Mode::Interactive);
                panels.message = workspace.take_resource_reports().join("; ").into();
                if let Some(note) = workspace.ui_language_notice.take() {
                    panels.set_message(note);
                }
                if !matches!(
                    options.mode,
                    Mode::Interactive | Mode::Panels | Mode::ThemeUi
                ) {
                    panels.side_visible = false;
                }
                panels
            },
            window_size: view.size,
            content: Rect {
                x: 0.,
                y: 0.,
                width: view.size.width,
                height: view.size.height,
            },
            sheet_views: Default::default(),
            inactive_image_decodes: Default::default(),
            sheet_bounds: Default::default(),
            group_bounds: Vec::new(),
            workbench_bounds: Rect::default(),
            native_pane_minimum: None,
            tab_hits: Vec::new(),
            tab_bars: Vec::new(),
            tab_strips: Vec::new(),
            tab_painted: Vec::new(),
            tab_press: None,
            tab_slides: None,
            #[cfg(test)]
            torn_off_windows: Vec::new(),
            tab_button_hits: Vec::new(),
            absent_tab_buttons: None,
            keyboard_center: None,
            key_context: None,
            visible_view_keys: Vec::new(),
            visible_view_scratch: Vec::new(),
            tab_close_hits: Vec::new(),
            tab_click: None,
            tab_rename: None,
            image_status: None,
            hovered_close: None,
            hovered_tab_button: None,
            revealed_tabs: Vec::new(),
            print_jobs: Default::default(),
            dragging_splitter: None,
            dragging_sidebar: None,
            drag_select: None,
            scrollbar_binding_button: None,
            drag_text: None,
            text_drag_pressed: None,
            text_drag_origin: None,
            mouse_preferences: vel_platform::mouse::MousePreferences::read(),
            builds: crate::build::Builds::default(),
            fullscreen_before_distraction: false,
            window_frame: None,
            #[cfg(test)]
            empty_close_requested: false,
            view_buffer: None,
            view_goto_preview: None,
            options,
            clock,
            reporter,
            workspace,
            view,
            fonts: None,
            font_face: String::new(),
            renderer: None,
            first_present: false,
            startup_done: false,
            startup_services_pending,
            bench: None,
            samples: Vec::new(),
            retries: 0,
            input_at: None,
            bench_received: None,
            done,
            external_input: 0,
            first_scanout: FirstScanout::default(),
            waiting_since: None,
        }
    }
    pub fn restore_ui(&mut self, window: &vel_session::Window) {
        self.panels.restore_session(&self.workspace, window);
        self.fullscreen_before_distraction = window.fullscreen_before_distraction;
        self.window_frame = window.frame;
    }
    pub(crate) fn report_headless_interactive_startup(&self) -> Result<(), String> {
        let tabs = &self.workspace.tabs;
        self.reporter.emit(json!({
            "kind": "interactive_startup",
            "complete": true,
            "gui_started": false,
            "gpu_started": false,
            "tabs": if self.workspace.empty { 0 } else { tabs.len() },
            "lazy_tabs": tabs.iter().filter(|tab| tab.lazy.is_some()).count(),
            "active_path": tabs[self.workspace.active].path,
            "ui_language": self.workspace.translations.matched_language().as_str(),
            "syntax": self.workspace.editor().language.borrow().as_ref().map(|l| &l.syntax),
            "watcher_started": self.workspace.watcher.is_some(),
            "palette_deferred": self.workspace.palette_pending(),
        }))
    }

    /// An interactive window must become usable even when macOS never reports a real
    /// scanout: screen sharing and remote-control sessions skip every drawable
    /// (`presentedTime == 0`), and a window behind others or on another Space may not
    /// present for a long time. Until startup is done, forwarded files, loads and
    /// resource polls wait, so stop waiting and continue without the scanout record.
    fn start_without_scanout(&mut self, w: &WindowHandle, waited: Duration) -> Result<(), String> {
        self.clock.mark("startup_without_scanout");
        self.reporter.emit(json!({"kind":"startup_without_scanout","waited_ms":ms(waited),"skipped_drawables":self.first_scanout.skipped_drawables,"first_callback_main_ms":self.first_scanout.first_callback.map(|t|ms(t.saturating_duration_since(self.clock.entered))),"surface_retries":self.retries}))?;
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.release_prepared();
            renderer.persist_pipeline_cache()?;
        }
        self.startup_done = true;
        self.finish_startup_services("scanout_wait_expired_begin")?;
        w.request_frame_at(Instant::now());
        Ok(())
    }
    fn finish_startup_services(&mut self, phase: &'static str) -> Result<(), String> {
        if !std::mem::take(&mut self.startup_services_pending) {
            return Ok(());
        }
        let mut phases =
            crate::report::StartupPhases::new(self.clock.entered, self.reporter.is_enabled());
        phases.mark(phase);
        self.workspace.start_startup_watcher();
        phases.mark("watchers_started");
        self.panels.start_startup_workers(&self.workspace);
        self.sessions.borrow_mut().start_writer();
        phases.mark("index_session_services_started");
        self.workspace.finish_startup_palette();
        phases.mark("palette_published");
        phases.emit(&self.reporter)
    }
    /// A real action, or the first headless poll or scene, need not wait for native scanout.
    fn ensure_startup_services(&mut self) -> bool {
        if !self.startup_services_pending {
            return false;
        }
        // A reporting error must not stop the action that needed the services.
        let _ = self.finish_startup_services("startup_on_demand_begin");
        true
    }
    pub fn restore_session_id(&mut self, id: u64) {
        self.sessions
            .borrow_mut()
            .activate_reserved(self.session_id, id);
        self.session_id = id;
    }
    pub fn restore_remaining_reserved(
        &mut self,
        windows: Vec<vel_session::Window>,
        ids: Vec<u64>,
        selected: Option<usize>,
    ) {
        debug_assert_eq!(windows.len(), ids.len());
        self.restored_windows = windows
            .into_iter()
            .zip(ids)
            .enumerate()
            .filter_map(|(i, pair)| (Some(i) != selected).then_some(pair))
            .collect();
    }
    pub(crate) fn open_received_paths(
        &mut self,
        paths: &[std::path::PathBuf],
    ) -> Result<(), String> {
        if !paths.is_empty() && self.accessibility_focus.is_some() {
            self.leave_region_focus();
        }
        // A secondary native recipient can become primary if the old GUI
        // exits. Normalize project paths just as for any other forwarded open.
        self.publish_open_route();
        let request = crate::open_requests::native_request(paths)?;
        let result = match self.route_or_queue_open(crate::instance::Incoming::local(request))? {
            Some((_, incoming)) => self.accept_request(incoming),
            None => Ok(()),
        };
        self.sync_recents();
        result
    }
    pub fn set_crash(&mut self, crash: Option<crate::release::CrashStartup>) {
        self.releases.crash = crash;
    }
    pub fn set_instance(&mut self, instance: Option<crate::instance::Instance>) {
        self.sessions.borrow_mut().instance = instance;
    }
    pub fn set_restore_safety(&mut self, startup: crate::safe_mode::Startup) {
        if self.options.safe_mode {
            self.sessions.borrow_mut().persist_project_workspaces = false;
        }
        self.restore_safety = startup;
    }
    fn poll_restore_safety(&mut self, w: &WindowHandle) {
        if let Some(warning) = self.restore_safety.warning.take() {
            self.workspace.load_notices.push(warning.clone());
            self.workspace.messages.push(warning);
        }
        if let Some(guard) = self.restore_safety.guard.take() {
            guard.healthy_after_first_frame();
        }
        if w.is_focused()
            && let Some(path) = self.restore_safety.skipped.take()
            && Dialog::new(
                UiText::tr("Session Recovery"),
                UiText::tr_fmt("The previous startup crashed during restore. The session was preserved at {path}.",
                    [("path", path.display().to_string())]),
                [UiText::tr("Keep Closed"), UiText::tr("Reopen Windows")],
            ).show(&self.workspace.translations, w) == 1
        {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(vel_session::load(&path));
            });
            self.skipped_recovery = Some(rx);
        }
        if let Some(rx) = &self.skipped_recovery {
            match rx.try_recv() {
                Ok(recovery) => {
                    self.skipped_recovery = None;
                    self.workspace.messages.extend(recovery.messages);
                    let mut sessions = self.sessions.borrow_mut();
                    sessions.restored = recovery.session;
                    let ids = sessions.reserve_recovered();
                    self.restored_windows
                        .extend(sessions.restored.windows.iter().cloned().zip(ids));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.skipped_recovery = None;
                    self.workspace
                        .messages
                        .push("Could not read the preserved session".into());
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    w.request_frame_at(Instant::now() + std::time::Duration::from_millis(25));
                }
            }
        }
    }
    fn hot_exit(&self) -> vel_session::HotExit {
        if self.options.safe_mode {
            vel_session::HotExit::Disabled
        } else {
            self.workspace.hot_exit()
        }
    }
    fn session_policy(&self, quitting: bool, closing: bool) -> crate::session::Policy {
        let hot = self.hot_exit();
        crate::session::Policy {
            hot,
            quitting,
            remember: self.workspace.global_bool("remember_open_files", true),
            closing,
            project_hot_exit: self.workspace.global_bool("hot_exit_projects", true),
            remember_layout: self.workspace.remember_layout(hot),
        }
    }
    fn retain_switching_project(&self) -> bool {
        let policy = self.session_policy(false, true);
        policy.hot.retain(false)
            && (self.workspace.project_path().is_none() || policy.project_hot_exit)
    }
    fn open_project_for_session(&mut self, project: PathBuf) -> Result<(), String> {
        if self.options.safe_mode {
            self.workspace.open_project_with_restore(project, false)
        } else {
            self.workspace.open_project(project)
        }
    }
    fn switch_to_project(
        &mut self,
        window: Option<&WindowHandle>,
        path: PathBuf,
    ) -> Result<(), String> {
        if self
            .workspace
            .project_path()
            .is_some_and(|open| crate::open_requests::same_path(open, &path))
        {
            return Ok(());
        }
        if !self.retain_switching_project()
            && !self.can_close_many(window, &(0..self.workspace.tabs.len()).collect::<Vec<_>>())
        {
            return Ok(());
        }
        self.checkpoint_reason(false, true)?;
        self.sessions.borrow().flush()?;
        let retain = self.retain_switching_project();
        self.open_project_for_session(path.clone())
            .map_err(|error| format!("{}: {error}", path.display()))?;
        self.sessions
            .borrow_mut()
            .retain_departing(self.session_id, retain);
        self.panels.reset(&self.workspace);
        Ok(())
    }
    fn checkpoint(&mut self, quitting: bool) -> Result<(), String> {
        self.checkpoint_reason(quitting, false)
    }
    fn checkpoint_reason(&mut self, quitting: bool, closing: bool) -> Result<(), String> {
        if self.settings_window {
            return Ok(());
        }
        self.receive_file_renames();
        self.sync_recents();
        // Cache stdin clients' current contents before a project switch can replace
        // this window's documents. Completion still waits until their sheets close.
        self.waiters
            .retain_mut(|waiter| !waiter.complete(&mut self.workspace));
        let (window, documents) = self.capture_session_window()?;
        let policy = self.session_policy(quitting, closing);
        self.sessions.borrow_mut().publish(
            self.session_id,
            window,
            documents,
            [
                self.workspace.recent_files.clone(),
                self.workspace.recent_folders.clone(),
                self.workspace.recent_projects.clone(),
            ],
            policy,
        )?;
        self.session_due = None;
        Ok(())
    }
    fn capture_session_window(
        &mut self,
    ) -> Result<(vel_session::Window, Vec<vel_session::Document>), String> {
        let active = self.workspace.active;
        self.workspace.tabs[active].scroll = (self.view.scroll_x, self.view.scroll_y);
        self.workspace.tabs[active].scroll_line_height = self.view.line_height;
        let (mut window, documents) = self.workspace.snapshot()?;
        self.panels.capture_session(&mut window);
        window.fullscreen_before_distraction = self.fullscreen_before_distraction;
        window.frame = self.window_frame;
        Ok((window, documents))
    }
    fn prepare_window_close(&mut self, window: Option<&WindowHandle>) -> bool {
        self.ensure_startup_services();
        let policy = self.session_policy(false, true);
        let project = self.workspace.project_path().is_some()
            && self.sessions.borrow().persist_project_workspaces;
        let project_hot_exit = project && policy.project_hot_exit && policy.hot.retain(true);
        let indices: Vec<_> = (0..self.workspace.tabs.len()).collect();
        // The last macOS window is not a Quit. Only an explicit project has
        // somewhere to keep unsaved buffers other than the application session.
        if !project_hot_exit && !self.can_close_many(window, &indices) {
            return false;
        }
        if project {
            let result = self.capture_session_window().and_then(|(snapshot, docs)| {
                self.sessions
                    .borrow_mut()
                    .save_closing_project(snapshot, docs, policy)
            });
            if let Err(error) = result {
                let message = UiText::tr_fmt(
                    "Could not save project workspace: {detail}",
                    [("detail", error)],
                );
                if project_hot_exit
                    && indices
                        .iter()
                        .any(|&i| self.needs_document_close_decision(i))
                {
                    self.data_loss_error(window, message);
                } else {
                    self.status_error(message);
                }
                if project_hot_exit && !self.can_close_many(window, &indices) {
                    return false;
                }
            }
        }
        self.sync_recents();
        // Remember paths, not buffers or groups. Put the focused file at the
        // top of the reopen list, followed by the other files in this window.
        let active = self.workspace.active;
        let paths = self
            .workspace
            .closed_files
            .iter()
            .map(|(path, _)| path)
            .chain(
                (0..self.workspace.tabs.len())
                    .filter(|&index| index != active)
                    .chain([active])
                    .map(|index| &self.workspace.tabs[index])
                    .filter(|tab| !tab.transient || tab.dirty())
                    .filter_map(|tab| tab.path.as_ref()),
            )
            .cloned();
        let result = self.sessions.borrow_mut().close(
            self.session_id,
            policy.hot,
            policy.remember,
            policy.remember_layout,
            paths,
        );
        if let Err(error) = result {
            eprintln!("Could not checkpoint after closing a window: {error}");
        }
        // A closed window's files are no longer open for CLI --wait clients.
        for waiter in &mut self.waiters {
            waiter.finish(&mut self.workspace);
        }
        self.waiters.clear();
        true
    }
    fn prepare_quit(&mut self, window: Option<&WindowHandle>) -> bool {
        if self.options.mode != Mode::Interactive {
            let _=self.reporter.emit(json!({"kind":"error","stage":"user_close","message":"automatic benchmark closed before completion","complete":false}));
            return true;
        }
        self.ensure_startup_services();
        for tab in &mut self.workspace.tabs {
            if tab.lazy.is_none() && tab.editor.marked.is_some() {
                tab.editor.cancel_composition();
            }
        }
        if self.hot_exit().retain(true) {
            if let Err(error) = self.save_workspace_checkpoint() {
                if self
                    .workspace
                    .tabs
                    .iter()
                    .any(|t| t.dirty() || (t.path.is_none() && t.editor.text.len_bytes() > 0))
                {
                    self.data_loss_error(window, UiText::tr_fmt(
                        "Could not save Hot Exit session: {detail}. Save documents or repair the data directory before quitting.",
                        [("detail", error)],
                    ));
                    return false;
                }
                self.panels.message = error.into();
            }
        } else {
            if !self.can_close_many(window, &(0..self.workspace.tabs.len()).collect::<Vec<_>>()) {
                return false;
            }
            if let Err(error) = self.save_workspace_checkpoint() {
                self.panels.message = error.into();
            }
        }
        for waiter in &mut self.waiters {
            waiter.finish(&mut self.workspace);
        }
        true
    }
    fn save_workspace_checkpoint(&mut self) -> Result<(), String> {
        self.ensure_startup_services();
        self.checkpoint(true)?;
        self.sessions.borrow().flush()
    }
    fn open_native_window(
        &mut self,
        w: &WindowHandle,
        restored: Option<vel_session::Window>,
        reserved_id: Option<u64>,
        use_new_window_settings: bool,
    ) -> Result<(WindowHandle, u64), String> {
        let app = self.window_app(restored, reserved_id, use_new_window_settings)?;
        let id = app.session_id;
        w.new_window(
            WindowOptions {
                title: "Velocity".into(),
                size: Size {
                    width: app.options.width as f64,
                    height: app.options.height as f64,
                },
                ..Default::default()
            },
            app,
        )
        .map(|window| (window, id))
    }
    fn window_app(
        &self,
        restored: Option<vel_session::Window>,
        reserved_id: Option<u64>,
        use_new_window_settings: bool,
    ) -> Result<Self, String> {
        let project = restored.as_ref().and_then(|s| s.project.clone());
        let data = self.workspace.data_path().map(Path::to_owned);
        let mut workspace = if use_new_window_settings {
            Workspace::new_window(data, true)?
        } else {
            Workspace::new_empty(data, project, true)?
        };
        workspace.recent_files = self.workspace.recent_files.clone();
        workspace.recent_folders = self.workspace.recent_folders.clone();
        workspace.recent_projects = self.workspace.recent_projects.clone();
        workspace.restore_original = self.sessions.borrow().original.clone();
        if let Some(window) = &restored {
            let documents = self.sessions.borrow().restored.documents.clone();
            workspace.restore_window(window, &documents)?;
        } else if use_new_window_settings && workspace.remember_layout_override() == Some(true) {
            workspace
                .workbench
                .set_layout(self.workspace.workbench.layout.clone())?;
        }
        let mut options = self.options.clone();
        options.background &= restored.is_some();
        options.files.clear();
        options.file = None;
        options.project = None;
        let mut app = App::new(
            options.clone(),
            Clock::new(Instant::now(), std::time::SystemTime::now()),
            self.reporter.clone(),
            workspace,
            Arc::new(AtomicBool::new(false)),
            self.sessions.clone(),
        );
        if let Some(window) = &restored {
            app.restore_ui(window);
        }
        if let Some(id) = reserved_id {
            app.restore_session_id(id);
        }
        Ok(app)
    }
    fn poll_workbench(&mut self, w: &WindowHandle) -> Result<(), String> {
        self.poll_restore_safety(w);
        self.publish_open_route();
        self.sessions
            .borrow_mut()
            .handles
            .entry(self.session_id)
            .or_insert_with(|| w.clone());
        let merge = self
            .sessions
            .borrow_mut()
            .menu_merges
            .remove(&self.session_id);
        if let Some(destination) = merge
            && let Err(error) = self.merge_tabs_into(w, destination)
        {
            self.panels.message = error.into();
        }
        self.install_native_menu(w, false);
        self.sessions.borrow_mut().update_system_recents(
            self.workspace
                .global_bool("update_system_recent_files", true),
            |path| w.system_recent_document(path),
        );
        let requests = {
            let sessions = self.sessions.borrow();
            if sessions
                .active_window
                .is_none_or(|id| id == self.session_id)
            {
                sessions
                    .instance
                    .as_ref()
                    .map(|i| i.drain())
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        };
        for incoming in requests {
            if let Err(e) = self.receive_request(w, incoming) {
                self.workspace.messages.push(format!("CLI request: {e}"));
            }
        }
        for (window, id) in std::mem::take(&mut self.restored_windows) {
            if let Err(e) = self.open_native_window(w, Some(window), Some(id), false) {
                self.workspace.messages.push(e);
            }
        }
        let routed = self
            .sessions
            .borrow_mut()
            .open_inbox
            .remove(&self.session_id);
        for incoming in routed.into_iter().flatten() {
            self.apply_open_request(w, incoming)?;
        }
        self.poll_cli_commands(Some(w));
        self.waiters
            .retain_mut(|waiter| !waiter.complete(&mut self.workspace));
        let messages = self.sessions.borrow_mut().poll();
        for message in messages {
            self.panels.message = message.clone().into();
            self.reporter
                .emit(json!({"kind":"session_error","message":message}))?;
        }
        if self.session_due.is_some_and(|due| Instant::now() >= due)
            && let Err(e) = self.checkpoint(false)
        {
            self.panels.set_message(UiText::tr_fmt(
                "Session save failed: {detail}",
                [("detail", e.to_string())],
            ));
            self.session_due = None;
        }
        Ok(())
    }
    fn receive_request(
        &mut self,
        w: &WindowHandle,
        incoming: crate::instance::Incoming,
    ) -> Result<(), String> {
        use crate::open_requests::Route;
        self.ensure_startup_services();
        let Some((route, incoming)) = self.route_or_queue_open(incoming)? else {
            return Ok(());
        };
        if route == Route::New {
            self.open_request_in_new_window(w, incoming)?;
        } else {
            self.apply_open_request(w, incoming)?;
        }
        Ok(())
    }
    fn route_or_queue_open(
        &self,
        mut incoming: crate::instance::Incoming,
    ) -> Result<Option<(crate::open_requests::Route, crate::instance::Incoming)>, String> {
        let route = self.route_open(&mut incoming.request);
        if let crate::open_requests::Route::Window(id) = route
            && id != self.session_id
        {
            let mut sessions = self.sessions.borrow_mut();
            let inbox = sessions.open_inbox.entry(id).or_default();
            if inbox.len() >= 16 {
                return Err("target window's open queue is full".into());
            }
            inbox.push(incoming);
            return Ok(None);
        }
        Ok(Some((route, incoming)))
    }
    fn app_for_new_window(&self, incoming: crate::instance::Incoming) -> Result<Self, String> {
        let workspace =
            Workspace::new_empty(self.workspace.data_path().map(Path::to_owned), None, true)?;
        let mut options = self.options.clone();
        options.background = incoming.request.background;
        options.files.clear();
        options.file = None;
        options.project = None;
        let mut app = App::new(
            options,
            Clock::new(Instant::now(), std::time::SystemTime::now()),
            self.reporter.clone(),
            workspace,
            Arc::new(AtomicBool::new(false)),
            self.sessions.clone(),
        );
        app.accept_request(incoming)?;
        Ok(app)
    }
    fn open_request_in_new_window(
        &self,
        w: &WindowHandle,
        incoming: crate::instance::Incoming,
    ) -> Result<(), String> {
        let app = self.app_for_new_window(incoming)?;
        w.new_window(app.new_window_options(None), app).map(|_| ())
    }
    fn new_window_options(&self, position: Option<Point>) -> WindowOptions {
        WindowOptions {
            size: Size {
                width: self.options.width as f64,
                height: self.options.height as f64,
            },
            position,
            ..Default::default()
        }
    }
    fn tear_off_tab(
        &mut self,
        window: Option<&WindowHandle>,
        sheet: u64,
        position: Option<Point>,
    ) -> Result<(), String> {
        let index = self
            .workspace
            .sheet_index(sheet)
            .ok_or("The tab is no longer open")?;
        self.workspace.prepare_sheet(index)?;
        if !self.transferable_tab(index) {
            return Err("Finish loading or close the other views before moving this tab".into());
        }
        let mut request = crate::instance::Request::new(&Options::default(), None)?;
        request.new_window = true;
        let app = self.app_for_new_window(crate::instance::Incoming::local(request))?;
        let destination = app.session_id;
        if let Some(window) = window {
            // A native launch can fail. Do not take the live editor out of its
            // source window until the destination is ready to receive it.
            let target = window.new_window(app.new_window_options(position), app)?;
            self.move_tab_to_window(Some(window), sheet, destination, 0)?;
            target.request_frame();
            return Ok(());
        }
        #[cfg(test)]
        {
            let mut app = app;
            self.move_tab_to_window(None, sheet, destination, 0)?;
            assert!(app.receive_tabs());
            app.sync_active_view();
            self.torn_off_windows.push(app);
            Ok(())
        }
        #[cfg(not(test))]
        Err("Tearing off a tab requires a native window".into())
    }
    fn folder_window_request(
        path: Option<PathBuf>,
    ) -> Result<Option<crate::instance::Incoming>, String> {
        let Some(path) = path else {
            return Ok(None);
        };
        let mut request = crate::instance::Request::new(&Options::default(), None)?;
        request.new_window = true;
        request.native_open = true;
        request.files.push(crate::cli::FileLocation {
            path,
            line: 1,
            column: 1,
        });
        Ok(Some(crate::instance::Incoming::local(request)))
    }
    fn route_open(&self, incoming: &mut crate::instance::Request) -> crate::open_requests::Route {
        use crate::open_requests::{OpenWindow, RouteRequest, route};
        let policy = OpenWindow::setting(self.workspace.settings.get("open_files_in_new_window"));
        let request = RouteRequest::prepare(incoming, policy);
        let sessions = self.sessions.borrow();
        route(&request, &sessions.open_windows, Some(self.session_id))
    }
    fn publish_open_route(&self) {
        if self.settings_window {
            return;
        }
        self.sessions
            .borrow_mut()
            .open_windows
            .entry(self.session_id)
            .or_default()
            .update(&self.workspace, self.options.mode == Mode::Interactive);
    }
    fn open_native_window_request(
        &mut self,
        w: &WindowHandle,
        mut request: crate::instance::Request,
    ) -> Result<(), String> {
        request.new_window = true;
        self.publish_open_route();
        self.receive_request(w, crate::instance::Incoming::local(request))
    }
    pub(super) fn apply_open_request(
        &mut self,
        w: &WindowHandle,
        incoming: crate::instance::Incoming,
    ) -> Result<(), String> {
        let reveal = incoming.request.stdin.is_some();
        let background = incoming.request.background;
        self.accept_request(incoming)?;
        self.publish_open_route();
        if !background {
            w.focus();
        }
        // A positioned open schedules its own center. Revealing on every open
        // would also scroll a bare reopen whose caret was outside the viewport.
        self.changed(w, Instant::now(), reveal);
        Ok(())
    }
    pub(super) fn accept_request(
        &mut self,
        incoming: crate::instance::Incoming,
    ) -> Result<(), String> {
        self.ensure_startup_services();
        self.receive_file_renames();
        if !self.finish_tab_rename(None) {
            // Keep the field on errors, but complete the external client's protocol.
            crate::instance::Waiter::acknowledge(
                incoming,
                &mut self.workspace,
                Default::default(),
            )?;
            return Ok(());
        }
        let request = &incoming.request;
        if self.panels.navigation_overlay_open()
            && (!request.files.is_empty() || request.project.is_some() || request.stdin.is_some())
        {
            self.panels.cancel_overlay(&mut self.workspace);
            self.sync_active_view();
        }
        if self.accessibility_focus.is_some()
            && (!request.files.is_empty() || request.project.is_some() || request.stdin.is_some())
        {
            self.leave_region_focus();
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut errors = Vec::new();
        if let Some(project) = request.project.clone().filter(|p| {
            !self
                .workspace
                .project_path()
                .is_some_and(|open| crate::open_requests::same_path(open, p))
        }) {
            let retain = self.retain_switching_project();
            let result = (|| {
                if !retain
                    && self.workspace.project_path().is_some()
                    && (0..self.workspace.tabs.len()).any(|i| self.needs_document_close_decision(i))
                {
                    return Err("Save changes before switching projects".to_owned());
                }
                self.checkpoint_reason(false, true)?;
                self.sessions.borrow().flush()?;
                self.open_project_for_session(project.clone())
                    .map_err(|error| format!("{}: {error}", project.display()))?;
                self.sessions
                    .borrow_mut()
                    .retain_departing(self.session_id, retain);
                self.panels.reset(&self.workspace);
                Ok::<_, String>(())
            })();
            if let Err(error) = result {
                errors.push(error);
            }
        }
        if request.native_open {
            let paths = request
                .files
                .iter()
                .map(|location| location.path.clone())
                .collect::<Vec<_>>();
            if let Err(error) =
                crate::open_requests::open_paths(&mut self.workspace, &mut self.panels, &paths)
            {
                errors.push(error);
            }
        } else {
            for location in &request.files {
                let result = crate::open_requests::open_locations(
                    &mut self.workspace,
                    &mut self.panels,
                    std::iter::once(location.clone()),
                );
                if result.is_empty() && !location.path.is_dir() {
                    ids.insert(self.workspace.tabs[self.workspace.active].sheet);
                }
                errors.extend(result);
            }
        }
        if let Some(stdin) = &request.stdin {
            ids.insert(self.workspace.open_stdin(stdin.as_bytes())?);
        }
        if request.files.is_empty() && request.stdin.is_none() && !self.workspace.is_empty() {
            ids.insert(self.workspace.tabs[self.workspace.active].sheet);
        }
        if !errors.is_empty() {
            self.panels.message = errors.join("\n").into();
        }
        if let Some(command) = &request.command
            && self.cli_commands.len() < 16
        {
            self.cli_commands.push_back((
                command.clone(),
                ids.clone(),
                (!self.workspace.is_empty())
                    .then(|| self.workspace.tabs[self.workspace.active].sheet),
            ));
        }
        // Path failures do not suppress the accepted/closed protocol or later paths.
        if let Some(waiter) =
            crate::instance::Waiter::acknowledge(incoming, &mut self.workspace, ids)?
        {
            self.waiters.push(waiter);
        }
        Ok(())
    }
    fn poll_cli_commands(&mut self, w: Option<&WindowHandle>) {
        // Build 4200 delays CLI commands until the requested files/plugins are loaded.
        // https://www.sublimetext.com/download
        while let Some((_, sheets, _)) = self.cli_commands.front() {
            if self.workspace.tabs.iter().any(|tab| {
                sheets.contains(&tab.sheet)
                    && (tab.loading.is_some() || tab.binary_loading.is_some())
            }) {
                break;
            }
            let (command, _, target) = self.cli_commands.pop_front().unwrap();
            if let Some(index) = target.and_then(|sheet| self.workspace.sheet_index(sheet)) {
                self.workspace.activate_sheet(index);
            }
            self.run_named(w, &command.name, &command.args);
        }
    }
    fn status_error(&mut self, mut message: UiText) {
        message.refresh(&self.workspace.translations);
        self.panels.note_console(&self.workspace, &message);
        let _ = self
            .reporter
            .emit(json!({"kind":"editor_error","message":message.as_str()}));
        self.panels.set_message(message);
    }
    fn publish_resource_reports(&mut self) {
        let reports = self.workspace.take_resource_reports();
        if !reports.is_empty() {
            self.panels.message = reports.join("; ").into();
            for report in &reports {
                self.panels.note_console(&self.workspace, report);
            }
        }
    }
    fn data_loss_error(&mut self, window: Option<&WindowHandle>, mut message: UiText) {
        message.refresh(&self.workspace.translations);
        self.panels.note_console(&self.workspace, &message);
        let _ = self
            .reporter
            .emit(json!({"kind":"save_diagnostic","message":message.as_str()}));
        let mut dialog = Dialog::new(UiText::tr("Velocity Error"), message, [UiText::tr("OK")]);
        if let Some(window) = window {
            dialog.show(&self.workspace.translations, window);
        } else {
            dialog.localize(&self.workspace.translations);
            #[cfg(test)]
            self.data_loss_dialogs.push(dialog);
        }
    }
    fn save_failure(&mut self, window: Option<&WindowHandle>, error: String) {
        // The save dispatcher also reports preflight rejections. Those have
        // not attempted a write, even when collected by Save All.
        if error.lines().all(|line| {
            line.ends_with("document is read only")
                || line.ends_with("binary/hex documents cannot be saved as text")
                || line.ends_with("only regular files can be used as documents")
        }) {
            self.named_error(window, &error);
        } else {
            self.data_loss_error(
                window,
                UiText::tr_fmt("Could not save: {detail}", [("detail", error)]),
            );
        }
    }
    fn sync_workspace_after_panels(&mut self) {
        // Panels temporarily protect a sidebar preview by clearing transient.
        // Only promote that pending open once a panel has actually committed it.
        if self.panels.overlay.is_none() {
            let promote = self
                .workspace
                .tabs
                .iter()
                .enumerate()
                .filter(|(_, t)| t.preview_recent_pending && !t.transient)
                .map(|(i, _)| i)
                .collect::<Vec<_>>();
            for index in promote {
                self.workspace.make_permanent(index);
            }
        }
        self.workspace.sync_shared();
    }
    fn command_enabled_args(&self, name: &str, args: &Value) -> bool {
        let context_tab = args["_context_sheet"]
            .as_u64()
            .map(|sheet| self.workspace.sheet_index(sheet));
        if context_tab == Some(None) {
            return false;
        }
        let target = context_tab.flatten().unwrap_or(self.workspace.active);
        if args["_context_version"]
            .as_u64()
            .is_some_and(|version| version != self.workspace.tabs[target].editor.revision)
        {
            return false;
        }
        if name == tab_rename::RENAME_TAB {
            return self.tab_rename_target(target).is_some();
        }
        if name == "open_terminal" {
            return args["dir"].is_string()
                || !args["paths"].as_array().is_none_or(Vec::is_empty)
                || self.workspace.tabs[target].path.is_some()
                || !self.workspace.folders.is_empty();
        }
        if name == "unselect_current" {
            return self.workspace.workbench.groups[self.workspace.workbench.active]
                .selected
                .len()
                > 1;
        }
        if self.workspace.is_empty() && matches!(name, "close" | "close_file") {
            return self.should_close_empty_window();
        }
        if let Some(state) = self.context_state(name, args) {
            return state.enabled;
        }
        if name == "package_check_upgrades"
            && self
                .workspace
                .settings
                .get("package_index_url")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        {
            return false;
        }
        if self.releases.busy() && (name.starts_with("package_") || name == "install_cli") {
            return false;
        }
        match name {
            "open_as_image" | "open_as_text" => {
                return self.workspace.can_open_as(target, image_open_as(name));
            }
            "copy_as_html" | "print" => {
                let editor = &self.workspace.tabs[target].editor;
                return !self.workspace.is_empty()
                    && editor.binary.is_none()
                    && editor.language.borrow().is_some();
            }
            "native_minimize" | "native_zoom" | "native_bring_all_to_front" => return false,
            "refresh_folder_list" | "close_folder_list" => {
                return !self.workspace.folders.is_empty();
            }
            "toggle_show_open_files" => return true,
            "move_tab_to_window" | "move_tab_to_new_window" => {
                return self.transferable_tab(target);
            }
            "merge_all_windows" => return !self.merge_sources().is_empty(),
            "reopen_last_file" => {
                return !self.workspace.closed_files.is_empty()
                    || self.sessions.borrow().has_closed_window_files();
            }
            "reopen" | "reopen_encoding" | "revert"
                if self.workspace.tabs[target].path.is_none() =>
            {
                return false;
            }
            "prompt_save_as" | "save_as" | "save" | "save_and_close" | "save_encoding"
                if self.workspace.is_empty() =>
            {
                return false;
            }
            "save_all" => return self.workspace.tabs.iter().any(|tab| tab.dirty()),
            // Search replacements are tied to a completed Find in Files request.
            // https://docs.sublimetext.io/reference/commands.html
            "replace_in_files" => return self.panels.file_replace_ready(&self.workspace),
            "cancel_find_in_files" => return self.panels.file_task.is_some(),
            "run_syntax_tests"
                if !args["file"].is_string()
                    && self.workspace.tabs[self.workspace.active].path.is_none() =>
            {
                return false;
            }
            "toggle_find_option"
                if matches!(
                    args["option"].as_str(),
                    Some("use_gitignore" | "use_buffer" | "context")
                ) =>
            {
                return matches!(self.panels.panel, Some(crate::panels::Panel::Files));
            }
            "toggle_find_option"
                if args["_context_menu"] == "find_in_files" && args["option"] == "in_selection" =>
            {
                return false;
            }
            "toggle_find_option" if args["option"] == "in_selection" => {
                // An explicit in-session command can select an empty scope.
                // Menus and palette use the stricter check in command_enabled.
                return self.workspace.tabs[target].editor.binary.is_none()
                    && (matches!(
                        self.panels.panel,
                        Some(
                            crate::panels::Panel::Find
                                | crate::panels::Panel::Replace
                                | crate::panels::Panel::Incremental
                        )
                    ) || self.panels.can_find_in_selection(&self.workspace));
            }
            "find_in_files_where" => {
                return self.panels.panel == Some(crate::panels::Panel::Files);
            }
            "run_macro" if self.workspace.editor().registry.is_recording() => {
                return false;
            }
            "insert_snippet" if args["require_selection"] == true => {
                let editor = &self.workspace.tabs[target].editor;
                return !editor.read_only
                    && editor.binary.is_none()
                    && editor
                        .buffer
                        .view(editor.id)
                        .sel()
                        .iter()
                        .any(|region| !region.empty());
            }
            "new_pane" => return self.workspace.split_workbench(args).is_ok(),
            "focus_group" | "move_to_group"
                if args["group"]
                    .as_u64()
                    .is_none_or(|g| g as usize >= self.workspace.workbench.groups.len()) =>
            {
                return false;
            }
            "close_pane" | "close_split" if self.workspace.workbench.groups.len() < 2 => {
                return false;
            }
            "cancel_build" if self.builds.task.is_none() => return false,
            "next_result" | "prev_result" | "current_result"
                if if self.panels.search_results_active {
                    !self.panels.has_search_results()
                } else {
                    self.builds.diagnostics.is_empty()
                } =>
            {
                return false;
            }
            "edit_project" | "close_project" if self.workspace.project_path().is_none() => {
                return false;
            }
            "recent_projects" | "quick_switch_project" | "remove_deleted_recent_projects"
                if self.workspace.recent_projects.is_empty() =>
            {
                return false;
            }
            _ => {}
        }
        if (args["_context_menu"].is_null() || args["_context_menu"] == "widget")
            && self.panels.input().is_some()
            && !self.panels.palette_accept_pending(name, args)
            && self.workspace.editor().registry.kind(name) == Some(vel_commands::Kind::TextCommand)
        {
            return true;
        }
        let tab = &self.workspace.tabs[target];
        if tab.editor.read_only
            && matches!(
                name,
                "cut" | "paste" | "paste_and_indent" | "revert_hunk" | "replace_misspelling"
            )
        {
            return false;
        }
        if tab.editor.read_only
            && matches!(
                name,
                "save" | "save_as" | "prompt_save_as" | "save_encoding"
            )
        {
            return false;
        }
        if matches!(name, "goto_hex_offset" | "copy_hex" | "copy_hex_text")
            && tab.editor.binary.is_none()
        {
            return false;
        }
        self.workspace
            .editor()
            .registry
            .is_enabled(name, args, self.workspace.editor())
            && !((tab.loading.is_some() || tab.binary_loading.is_some())
                && self.workspace.editor().registry.kind(name)
                    == Some(vel_commands::Kind::TextCommand)
                && !matches!(
                    name,
                    "move" | "move_to" | "select_all" | "copy" | "single_selection"
                ))
    }
    fn command_states(&self) -> CommandStates {
        let capture = |tab: &crate::workspace::Tab| {
            (
                tab.sheet,
                tab.editor.buffer.change_id(),
                tab.editor.buffer.view(tab.editor.id).sel().clone(),
            )
        };
        if self.workspace.tabs.len() == 1 {
            CommandStates::Single(capture(&self.workspace.tabs[0]))
        } else {
            CommandStates::Multiple(self.workspace.tabs.iter().map(capture).collect())
        }
    }
    fn target_changed(&self, before: &CommandStates) -> bool {
        let tab = &self.workspace.tabs[self.workspace.active];
        before
            .iter()
            .find(|(sheet, _, _)| *sheet == tab.sheet)
            .is_some_and(|(_, change, selection)| {
                let current = tab.editor.buffer.view(tab.editor.id).sel();
                *change != tab.editor.buffer.change_id()
                    || (!selection.same_snapshot(current) && selection != current)
            })
    }
    fn dispatch_key_binding(
        &mut self,
        window: Option<&WindowHandle>,
        key: &vel_config::keymap::Key,
        received: Instant,
        viewport: std::ops::Range<usize>,
    ) -> bool {
        self.ensure_startup_services();
        if key.name == "escape" && self.cancel_tab_drag(window) {
            return true;
        }
        if self.tab_rename.is_some() {
            return self.tab_rename_key(window, key);
        }
        if self.settings_page_has_focus()
            && !matches!((key.name.as_str(), key.modifiers), (",", 8) | ("p", 12))
        {
            let handled = self.settings_key(window, key, received);
            // In the Settings window a shortcut the page does not use resolves through the
            // keymap; run_named then runs or forwards it, or ignores a document command.
            if handled || !(self.settings_window && key.modifiers & 8 != 0) {
                return handled;
            }
        }
        if key.name == "escape" {
            self.keyboard_center = None;
        }
        if self.dispatch_region_key(window, key) {
            return true;
        }
        let context = (
            self.workspace.tabs[self.workspace.active].sheet,
            self.workspace.resource_revision,
            self.builds.task.is_some(),
            self.workspace.workbench.groups[self.workspace.workbench.active]
                .selected
                .len()
                > 1,
        );
        if self.key_context != Some(context) {
            let keymap = &mut self.workspace.editor_mut().keymap;
            keymap.set_app_context("build_running", json!(context.2));
            // Side-by-side selected tabs: https://www.sublimetext.com/docs/tab_multi-select.html
            keymap.set_app_context("group_has_multiselect", json!(context.3));
            self.key_context = Some(context);
        }
        // Vintage motions and operators can edit/move while resolving Pending.
        // Capture only the active view: selection clones share their storage.
        let tab = &self.workspace.tabs[self.workspace.active];
        let intercepted_before = CommandStates::Single((
            tab.sheet,
            tab.editor.buffer.change_id(),
            tab.editor.buffer.view(tab.editor.id).sel().clone(),
        ));
        let find_reveal = self.panels.overlay.is_none()
            && key.name == "enter"
            && matches!(
                self.panels.panel,
                Some(
                    crate::panels::Panel::Find
                        | crate::panels::Panel::Replace
                        | crate::panels::Panel::Incremental
                )
            );
        let resolution = match self.panels.resolve_key(&mut self.workspace, key, viewport) {
            Ok(resolution) => resolution,
            Err(error) => {
                self.named_error(window, &error);
                return true;
            }
        };
        match resolution {
            vel_config::keymap::Resolution::Unbound => false,
            vel_config::keymap::Resolution::Pending => {
                let moved = self.target_changed(&intercepted_before) || find_reveal;
                if let Some(window) = window {
                    self.changed(window, received, moved);
                } else {
                    self.changed_state(received, moved);
                }
                true
            }
            vel_config::keymap::Resolution::Command(invocation) => {
                self.run_named(window, &invocation.command, &invocation.args);
                self.input_at = Some(received);
                true
            }
        }
    }
    fn build_application_menu(&mut self) -> Vec<MenuEntry> {
        let macros = self
            .panels
            .macro_file_items(&self.workspace)
            .unwrap_or_default();
        let tr = &self.workspace.translations;
        let mut entries = native_menu(self, &self.workspace.menus, &macros);
        if let Some(syntax) = entries
            .iter_mut()
            .find(|item| item.id == "view")
            .and_then(|view| {
                view.children
                    .iter_mut()
                    .find(|item| item.id == "syntax_list")
            })
        {
            syntax.children.extend(self.syntax_menu());
        }
        #[cfg(target_os = "macos")]
        install_application_rows(&mut entries, tr);
        #[cfg(windows)]
        crate::windows_menu::arrange(&mut entries, tr, &self.workspace.keymap);
        prune_empty_menus(&mut entries);
        entries
    }
    fn application_menu(&mut self) -> Vec<MenuEntry> {
        self.sync_recents();
        self.workspace.refresh_file_menus();
        self.build_application_menu()
    }
    fn install_native_menu(&mut self, window: &WindowHandle, focus_changed: bool) {
        if cfg!(windows) && self.settings_window {
            // A utility window on Windows has no menu bar; it is also left out of the
            // editor-window cycle, which only counts windows with menus.
            if !self.menu_owner {
                window.set_menu(&[]);
                self.menu_owner = true;
            }
            return;
        }
        let rebuild = self.refresh_native_menu_model();
        // Only the key window owns the menu bar: its items target the installing window.
        // A background window that rebuilt its model (recent files, resources), or a window
        // that just resigned key, must not take the menu back — that sent menu commands
        // such as Close Window or Close File to a window other than the one in front.
        let focused = window.is_focused();
        let install = menu_install_wanted(focused, rebuild, focus_changed, self.menu_owner);
        if install || (cfg!(windows) && (rebuild || focus_changed)) {
            window.set_menu(&self.menu_cache.as_ref().expect("installed menu").1);
        }
        self.menu_owner = focused;
    }
    fn refresh_native_menu_model(&mut self) -> bool {
        self.sync_recents();
        self.workspace.refresh_file_menus();
        let key = (
            self.workspace.resource_revision,
            self.workspace.menu_revision,
            self.workspace.translations.revision(),
        );
        if self.menu_cache.as_ref().is_some_and(|(old, _)| *old == key) {
            return false;
        }
        let entries = self.build_application_menu();
        self.menu_cache = Some((key, entries));
        true
    }
    fn syntax_menu(&self) -> Vec<MenuEntry> {
        let mut packages = std::collections::BTreeMap::<_, Vec<MenuEntry>>::new();
        for syntax in self
            .workspace
            .assets
            .syntaxes
            .syntaxes()
            .iter()
            .filter(|s| !s.hidden)
        {
            let package = syntax
                .path
                .strip_prefix("Packages/")
                .unwrap_or(&syntax.path)
                .split_once('/')
                .map_or("General", |(name, _)| name);
            packages.entry(package).or_default().push(MenuEntry {
                caption: syntax.name.clone(),
                command: Some("set_file_type".into()),
                args: json!({"syntax": syntax.path}).to_string(),
                ..Default::default()
            });
        }
        packages
            .into_iter()
            .map(|(package, mut children)| {
                children.sort_unstable_by(|a, b| a.caption.cmp(&b.caption));
                MenuEntry {
                    caption: package.into(),
                    children,
                    ..Default::default()
                }
            })
            .collect()
    }
    fn sync_recents(&mut self) {
        self.sessions.borrow_mut().sync_recents(
            self.session_id,
            [
                &mut self.workspace.recent_files,
                &mut self.workspace.recent_folders,
                &mut self.workspace.recent_projects,
            ],
        );
    }
    // Sublime Text build 4121 keeps project windows open even without folders:
    // https://www.sublimetext.com/download
    fn should_close_empty_window(&self) -> bool {
        self.workspace.is_empty()
            && self.workspace.folders.is_empty()
            && self.workspace.project_path().is_none()
            && self
                .workspace
                .editor()
                .settings
                .bool("close_windows_when_empty", true)
    }
    fn close_empty_window(&mut self, window: Option<&WindowHandle>) {
        if self.should_close_empty_window() {
            #[cfg(test)]
            {
                self.empty_close_requested = true;
            }
            if let Some(window) = window {
                window.request_close();
            }
        }
    }
    fn move_tab_to_window(
        &mut self,
        window: Option<&WindowHandle>,
        sheet: u64,
        destination: u64,
        group: usize,
    ) -> Result<(), String> {
        if destination == self.session_id || !self.sessions.borrow().can_receive_tab(destination) {
            return Err("The destination window cannot accept the tab".into());
        }
        let index = self
            .workspace
            .sheet_index(sheet)
            .ok_or("The tab is no longer open")?;
        self.workspace.prepare_sheet(index)?;
        if !self.transferable_tab(index) {
            return Err("Finish loading or close the other views before moving this tab".into());
        }
        if index == self.workspace.active {
            self.workspace.tabs[index].scroll = (self.view.scroll_x, self.view.scroll_y);
            self.workspace.tabs[index].scroll_line_height = self.view.line_height;
        }
        let mut slot = crate::workspace::Tab::new(
            vel_text::Text::new(),
            &self.workspace.settings,
            &self.workspace.keymap,
        )?;
        slot.sheet = sheet;
        // close() chooses the same neighboring sheet as a normal close. Its temporary
        // unnamed adapter does not contribute to this window's closed-file history.
        let tab = std::mem::replace(&mut self.workspace.tabs[index], slot);
        let result = self.workspace.close(index);
        self.sessions
            .borrow_mut()
            .transfer_tab(destination, group, tab);
        self.sheet_views.remove(&sheet);
        if result.is_ok() {
            self.close_empty_window(window);
        }
        result
    }
    fn transferable_tab(&self, index: usize) -> bool {
        let tab = &self.workspace.tabs[index];
        !self.workspace.is_empty()
            && !tab.find_results
            && tab.loading.is_none()
            && tab.binary_loading.is_none()
            && !self.workspace.stdout_sheets.contains(&tab.sheet)
            && self
                .workspace
                .tabs
                .iter()
                .filter(|t| t.session_document == tab.session_document)
                .count()
                == 1
    }
    fn merge_sources(&self) -> Vec<u64> {
        let sessions = self.sessions.borrow();
        let Some(active) = sessions.open_windows.get(&self.session_id) else {
            return Vec::new();
        };
        // Preserve project roots and settings when merging editor tabs.
        // https://developer.apple.com/design/human-interface-guidelines/the-menu-bar#Window-menu
        sessions
            .open_windows
            .iter()
            .filter_map(|(&id, route)| {
                (id != self.session_id
                    && route.project == active.project
                    && route.folders == active.folders)
                    .then_some(id)
            })
            .collect()
    }
    fn merge_tabs_into(&mut self, window: &WindowHandle, destination: u64) -> Result<(), String> {
        let sheets: Vec<_> = self
            .workspace
            .workbench
            .groups
            .iter()
            .flat_map(|group| group.sheets.iter().copied())
            .filter(|sheet| {
                self.workspace
                    .sheet_index(*sheet)
                    .is_some_and(|index| !self.workspace.tabs[index].find_results)
            })
            .collect();
        if !self
            .sessions
            .borrow()
            .can_receive_tabs(destination, sheets.len())
            || (!self.workspace.is_empty()
                && self
                    .workspace
                    .tabs
                    .iter()
                    .enumerate()
                    .any(|(index, tab)| !tab.find_results && !self.transferable_tab(index)))
        {
            return Err("This window has tabs that cannot be moved yet.".into());
        }
        if !self.workspace.is_empty() {
            // Materialize before moving anything so errors cannot half-merge a window.
            for index in 0..self.workspace.tabs.len() {
                if self.workspace.tabs[index].find_results {
                    continue;
                }
                self.workspace.prepare_sheet(index)?;
                if !self.transferable_tab(index) {
                    return Err("Finish opening files before merging windows.".into());
                }
            }
            for sheet in sheets {
                self.move_tab_to_window(None, sheet, destination, usize::MAX)?;
            }
        }
        window.request_close();
        Ok(())
    }
    fn poll_workspace_model(&mut self) -> bool {
        let before = (
            self.workspace.translations.revision(),
            self.workspace.palette_revision,
        );
        let started = self.ensure_startup_services();
        let transferred = self.receive_tabs();
        let renamed = self.receive_file_renames();
        let resources = self.workspace.resource_revision;
        let old_chrome = self.window_chrome_preferences();
        let changed = self.workspace.poll();
        if self.workspace.resource_revision != resources {
            let chrome = self.window_chrome_preferences();
            if old_chrome.0 != chrome.0 {
                self.panels.side_visible = chrome.0;
            }
            if old_chrome.1 != chrome.1 {
                self.panels.chrome = chrome.1;
            }
        }
        if before
            != (
                self.workspace.translations.revision(),
                self.workspace.palette_revision,
            )
        {
            self.refresh_open_palette();
            self.panels.refresh_locale(&self.workspace);
        }
        self.apply_ui_language_notice();
        changed || transferred || renamed || started
    }
    fn apply_ui_language_notice(&mut self) {
        if let Some(note) = self.workspace.ui_language_notice.take() {
            self.panels.set_message(note);
        }
    }
    fn window_chrome_preferences(&self) -> (bool, bool) {
        (
            self.workspace.window_visible("show_side_bar"),
            self.workspace.window_visible("show_status_bar"),
        )
    }
    fn receive_tabs(&mut self) -> bool {
        let incoming = self.sessions.borrow_mut().take_tabs(self.session_id);
        let changed = !incoming.is_empty();
        for (group, mut tab) in incoming {
            self.workspace.adopt_transferred_settings(&mut tab);
            tab.transient = false;
            let sheet = tab.sheet;
            if self.workspace.is_empty() {
                let placeholder = self.workspace.tabs[0].sheet;
                for group in &mut self.workspace.workbench.groups {
                    group.remove(placeholder);
                }
                self.workspace.tabs.clear();
                self.workspace.empty = false;
            }
            let group = if group < self.workspace.workbench.groups.len() {
                group
            } else {
                self.workspace.workbench.active
            };
            self.workspace.workbench.groups[group].add(sheet, true);
            self.workspace.tabs.push(tab);
            self.workspace.select(self.workspace.tabs.len() - 1);
        }
        changed
    }
    // Early movement starts an ordinary selection and commits that gesture type.
    // https://macromates.com/manual/en/expert_preferences#nsdraganddroptextdelay
    fn select_before_text_drag_delay(&mut self, point: Point, now: Instant) {
        if self.drag_text.is_some()
            && !self.text_drag_delay_elapsed(now)
            && let Some((origin, at)) = self.text_drag_origin.take()
            && (point.x != origin.x || point.y != origin.y)
        {
            self.drag_text = None;
            self.text_drag_pressed = None;
            self.drag_select = Some(crate::mouse_select::DragSelect::begin(
                &self.view,
                self.workspace.editor_mut(),
                origin,
                at,
                crate::mouse_select::Unit::Char,
                crate::mouse_select::Combine::Replace,
                false,
                false,
            ));
        }
    }
    fn text_drag_delay_elapsed(&self, now: Instant) -> bool {
        self.text_drag_pressed.is_none_or(|pressed| {
            self.mouse_preferences
                .drag_ready(now.saturating_duration_since(pressed))
        })
    }
    fn run_editor_selector(&mut self, window: Option<&WindowHandle>, command: Command) {
        if self.workspace.editor().accepts_native_selector(command) {
            let (name, args) = crate::editor::selector_command(command);
            self.run_named(window, name, &args);
        }
    }
    fn run_named(&mut self, window: Option<&WindowHandle>, name: &str, args: &Value) {
        if self.settings_window && !self.settings_window_runs(name) {
            self.forward_from_settings(window, name, args);
            return;
        }
        let overlay = self.panels.overlay;
        if let Err(error) = self.run_named_depth(window, name, args, 0) {
            self.named_error(window, &error);
        }
        if self.panels.overlay.is_some() && self.panels.overlay != overlay {
            self.accessibility_focus = None;
        } else {
            self.validate_region_focus();
        }
        if !self.workspace.recent_files.is_empty()
            || !self.workspace.recent_folders.is_empty()
            || !self.workspace.recent_projects.is_empty()
        {
            self.sync_recents();
        }
    }
    fn named_error(&mut self, window: Option<&WindowHandle>, message: &str) {
        self.status_error(UiText::command_error(message));
        if let Some(window) = window {
            window.request_frame();
        }
    }
    fn named_changed(&mut self, window: Option<&WindowHandle>, reveal: bool) {
        if let Some(window) = window {
            self.changed(window, Instant::now(), reveal);
        } else {
            self.changed_state(Instant::now(), reveal);
        }
    }
    // The optional native handle isolates clipboard/dialog/frame effects. Headless tests
    // execute this same dispatcher, Registry, action queue and panel/key interception.
    fn run_named_depth(
        &mut self,
        window: Option<&WindowHandle>,
        name: &str,
        args: &Value,
        depth: usize,
    ) -> Result<(), String> {
        self.dispatch_named(
            window,
            window.map(|w| w as &dyn RichClipboard),
            name,
            args,
            depth,
        )
    }

    fn dispatch_named(
        &mut self,
        window: Option<&WindowHandle>,
        clipboard: Option<&dyn RichClipboard>,
        name: &str,
        args: &Value,
        depth: usize,
    ) -> Result<(), String> {
        self.ensure_startup_services();
        self.receive_file_renames();
        // https://docs.sublimetext.io/reference/commands.html
        let name = match name {
            "prompt_add_folder" => "add_folder",
            "prompt_select_project" => "prompt_open_project",
            other => other,
        };
        if self.tab_rename.is_some() {
            let edit = match name {
                "select_all" => Some(Command::SelectAll),
                "copy" => Some(Command::Copy),
                "cut" => Some(Command::Cut),
                "paste" => Some(Command::Paste),
                "undo" | "soft_undo" => Some(Command::Undo),
                "redo" | "soft_redo" => Some(Command::Redo),
                _ => None,
            };
            if let Some(edit) = edit {
                self.tab_rename_command(window, edit);
                return Ok(());
            }
            if self.workspace.editor().registry.kind(name) == Some(vel_commands::Kind::TextCommand)
                || !self.finish_tab_rename(window)
            {
                return Ok(());
            }
        }
        if name == tab_rename::RENAME_TAB {
            let index = match args["_context_sheet"].as_u64() {
                Some(sheet) => self.workspace.sheet_index(sheet),
                None => Some(self.workspace.active),
            };
            if let Some(index) = index {
                self.start_tab_rename(window, index);
            }
            return Ok(());
        }
        if self.settings_page_has_focus() && self.settings_named(window, name) {
            return Ok(());
        }
        if !self.macro_view_active() {
            return Ok(());
        }
        if self.region_blocks_text()
            && self.workspace.editor().registry.kind(name) == Some(vel_commands::Kind::TextCommand)
        {
            return Ok(());
        }
        if depth >= 32 {
            return Err("command chain exceeds 32 levels".into());
        }
        if self.panels.overlay == Some(crate::panels::Overlay::Appearance)
            && matches!(
                name,
                "next_view"
                    | "prev_view"
                    | "next_view_in_stack"
                    | "prev_view_in_stack"
                    | "select_by_index"
                    | "next_sheet"
                    | "prev_sheet"
                    | "focus_group"
                    | "focus_neighboring_group"
            )
        {
            self.panels.cancel_overlay(&mut self.workspace);
        }
        if name == "goto_line"
            || (name == "show_overlay" && args["overlay"].as_str() == Some("goto"))
        {
            self.keyboard_center = Some(self.workspace.tabs[self.workspace.active].sheet);
        }
        let palette_accept = self.panels.palette_accept_pending(name, args);
        if palette_accept && !self.accept_palette_command(name, args) {
            return Ok(());
        }
        if name == "chain" {
            let commands = args["commands"].as_array().ok_or("chain needs commands")?;
            for command in commands {
                let (name, child) = vel_commands::chain_entry(command)?;
                if let Err(error) = self.dispatch_named(
                    window,
                    clipboard,
                    name,
                    &context_child_args(args, child),
                    depth + 1,
                ) {
                    if self.workspace.editor().registry.playback_depth() > 0 {
                        return Err(error);
                    }
                    self.named_error(window, &error);
                }
            }
            return Ok(());
        }
        if matches!(args["_context_menu"].as_str(), Some("editor" | "tab"))
            && !crate::context_menu::COMMANDS.contains(&name)
            && !Self::is_tab_close(name)
            && let Some(sheet) = args["_context_sheet"].as_u64()
            && let Some(index) = self.workspace.sheet_index(sheet)
        {
            self.workspace.select(index);
            self.sync_active_view();
        }
        if !self.macro_view_active() {
            return Ok(());
        }
        if !palette_accept && !self.command_enabled_args(name, args) {
            return Ok(());
        }
        if name == "move_to" && args.get("group").is_some() {
            self.workspace.workbench_action(name, args)?;
            self.named_changed(window, false);
            return Ok(());
        }
        if name == "copy_as_html" {
            self.copy_as_html(clipboard)?;
            self.named_changed(window, false);
            return Ok(());
        }
        if name == "print" {
            if let Some(window) = window {
                self.start_print()?;
                window.request_frame();
            }
            return Ok(());
        }
        if name == "reveal_in_side_bar" {
            let path = crate::context_menu::one_path(args)?;
            self.panels.reveal_sidebar_path(&mut self.workspace, path)?;
            self.named_changed(window, false);
            return Ok(());
        }
        if matches!(
            name,
            "refresh_folder_list"
                | "close_folder_list"
                | "toggle_show_open_files"
                | "move_tab_to_window"
                | "move_tab_to_new_window"
                | "merge_all_windows"
                | "convert_to_encoding"
                | "show_console"
                | "widget_history"
                | "set_ui_language"
                | "open_url"
                | "add_folder"
                | "clear_recent_items"
                | "reopen_last_file"
                | "open_recent_file"
                | "prompt_open_file"
        ) {
            let result = self.apply_named_action(window, name, args);
            // The file picker can open valid paths in a batch that also reports errors.
            if result.is_ok() || name == "prompt_open_file" {
                self.named_changed(window, name == "reopen_last_file");
            }
            return result;
        }
        match window.map_or(Ok(false), |window| self.context_action(window, name, args)) {
            Ok(true) => {
                self.named_changed(window, false);
                return Ok(());
            }
            Ok(false) => {}
            Err(error) => {
                self.panels.message = error.into();
                self.named_changed(window, false);
                return Ok(());
            }
        }
        if self.workspace.editor().registry.playback_depth() == 0
            && self.workspace.editor().registry.kind(name) == Some(vel_commands::Kind::TextCommand)
            && !matches!(
                name,
                "toggle_comment"
                    | "reindent"
                    | "expand_tabs"
                    | "unexpand_tabs"
                    | "detect_indentation"
                    | "fold"
                    | "unfold"
                    | "fold_by_level"
                    | "unfold_all"
                    | "fold_tag_attributes"
            )
            && !crate::html_ui::COMMANDS.contains(&name)
            && let Some(input) = self.panels.input_mut()
        {
            let clipboard = window.map_or_else(
                || self.workspace.editor().clipboard.clone(),
                |window| window.clipboard_text().unwrap_or_default(),
            );
            input.set_clipboard(clipboard);
            let result = input.run(name, args);
            if let Some(window) = window
                && let Some(input) = self.panels.input()
            {
                window.set_clipboard_text(input.clipboard());
            }
            let viewport = self.viewport();
            self.panels.edited(&mut self.workspace, viewport);
            self.named_changed(window, false);
            return result;
        }
        if let Some(window) = window {
            self.workspace.editor_mut().clipboard = window.clipboard_text().unwrap_or_default();
        }
        let before = self.workspace.editor().clipboard.clone();
        #[cfg(windows)]
        let clipboard_revision = self.workspace.editor().clipboard_revision;
        let document_before = if self.workspace.editor().registry.kind(name)
            == Some(vel_commands::Kind::TextCommand)
        {
            let tab = &self.workspace.tabs[self.workspace.active];
            CommandStates::Single((
                tab.sheet,
                tab.editor.buffer.change_id(),
                tab.editor.buffer.view(tab.editor.id).sel().clone(),
            ))
        } else {
            self.command_states()
        };
        let animate = (name == "move" && args["by"].as_str() == Some("pages"))
            || matches!(name, "show_at_center" | "goto_line" | "scroll_to");
        let now = animate.then(Instant::now);
        let scroll_from = now.map(|now| {
            self.view
                .prepare_keyboard_scroll(self.workspace.editor(), now)
        });
        let mut result = self.workspace.editor_mut().run(name, args);
        let publish_clipboard = self.workspace.editor().clipboard != before;
        // An explicit Windows Copy takes ownership even if the plain text
        // matches, replacing any unrelated formats from the previous owner.
        #[cfg(windows)]
        let publish_clipboard =
            publish_clipboard || self.workspace.editor().clipboard_revision != clipboard_revision;
        if publish_clipboard && let Some(window) = window {
            let written = window.set_clipboard_text(&self.workspace.editor().clipboard);
            if cfg!(windows) && !written && result.is_ok() {
                result = Err("Could not write to the clipboard".into());
            }
        }
        let actions = self.handle_named_actions(window);
        if self.accessibility_focus.is_some()
            && matches!(name, "open_file" | "open_project")
            && actions.is_ok()
        {
            self.leave_region_focus();
        }
        let changed = self.target_changed(&document_before);
        let explicit = name.starts_with("goto_")
            || matches!(
                name,
                "jump_back"
                    | "jump_forward"
                    | "next_bookmark"
                    | "prev_bookmark"
                    | "jump_to_mark"
                    | "select_to_mark"
                    | "swap_with_mark"
                    | "open_file"
                    | "show_at_center"
                    | "scroll_to"
                    | "find_next"
                    | "find_prev"
                    | "next_result"
                    | "prev_result"
                    | "current_result"
            );
        if scroll_from.is_none() && (changed || explicit) {
            self.view.cancel_scroll_animation();
        }
        self.named_changed(window, changed || explicit);
        if let Some(from) = scroll_from {
            self.view.animate_scroll_from(
                self.workspace.editor(),
                from,
                now.expect("animated command start"),
                self.accessibility_preferences.reduce_motion,
            );
        }
        result.and(actions)
    }
    fn save_tab(&mut self, w: &WindowHandle, index: usize, save_as: bool) -> bool {
        match self.save_tab_with_path(Some(w), index, save_as, None) {
            Ok(saved) => saved,
            Err(error) => {
                if !self.offer_utf8_save(index, None, &error) {
                    self.save_failure(Some(w), error);
                }
                false
            }
        }
    }
    fn save_tab_with_path(
        &mut self,
        window: Option<&WindowHandle>,
        index: usize,
        save_as: bool,
        chosen: Option<std::path::PathBuf>,
    ) -> Result<bool, String> {
        let path = if chosen.is_some() {
            chosen
        } else if save_as || self.workspace.tabs[index].path.is_none() {
            let suggested = self.workspace.tabs[index]
                .path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.workspace.tabs[index].ui_name(&self.workspace.ui));
            let Some(window) = window else {
                return Ok(false);
            };
            let Some(path) = self.choose_save_path(
                window,
                &suggested,
                self.save_dialog_directory(index).as_deref(),
            ) else {
                return Ok(false);
            };
            Some(path)
        } else {
            None
        };
        if !self.workspace.tabs[index].editor.read_only
            && !self.workspace.tabs[index].binary_document
            && self
                .workspace
                .resolve_save_conflict(index, path.as_deref())?
                .0
        {
            let overwrite = if let Some(window) = window {
                Dialog::new(
                    UiText::tr("File changed on disk"),
                    UiText::tr(
                        "The file has changed since it was loaded. Overwrite the disk contents?",
                    ),
                    [UiText::tr("Overwrite"), UiText::tr("Cancel")],
                )
                .show(&self.workspace.translations, window)
                    == 0
            } else {
                #[cfg(test)]
                {
                    self.overwrite_decisions.pop_front().unwrap_or(false)
                }
                #[cfg(not(test))]
                {
                    false
                }
            };
            if !overwrite {
                self.panels
                    .set_message(UiText::tr("Save cancelled: file changed on disk"));
                return Ok(false);
            }
            self.workspace
                .acknowledge_overwrite(index, path.as_deref())?;
        }
        self.workspace.save(index, path)?;
        Ok(true)
    }
    fn choose_save_path(
        &self,
        window: &WindowHandle,
        name: &str,
        directory: Option<&Path>,
    ) -> Option<PathBuf> {
        let tr = &self.workspace.translations;
        let label = tr.tr("Save As:");
        let button = tr.tr("Save");
        window.save_file_in(name, directory, Some((&label, &button)))
    }
    fn save_dialog_directory(&self, index: usize) -> Option<PathBuf> {
        // An empty default_dir preserves the current document directory.
        // https://sublime-text-unofficial-documentation.readthedocs.io/en/latest/reference/settings.html
        let tab = &self.workspace.tabs[index];
        let value = tab.editor.settings.string("default_dir", "");
        let directory = match value.strip_prefix("~/") {
            Some(rel) => std::env::var_os("HOME").map(|home| PathBuf::from(home).join(rel)),
            None if !value.is_empty() => Some(PathBuf::from(value)),
            None => None,
        };
        directory
            .filter(|path| path.is_absolute() && path.is_dir())
            .or_else(|| {
                tab.path
                    .as_deref()
                    .and_then(Path::parent)
                    .map(Path::to_owned)
            })
    }
    /// Encoding failures keep the document/bytes intact and offer an explicit retry.
    fn offer_utf8_save(
        &mut self,
        index: usize,
        path: Option<std::path::PathBuf>,
        error: &str,
    ) -> bool {
        if !error.contains("characters cannot be represented in") {
            return false;
        }
        // The current public docs/changelog do not establish automatic UTF-8
        // fallback on this error. Preserve our strict error and require the user
        // to select recovery; never silently change the document's encoding.
        // https://docs.sublimetext.io/reference/commands.html#save
        // https://www.sublimetext.com/download
        let mut args = json!({"encoding":"UTF-8", "_context_menu":"tab",
            "_context_sheet":self.workspace.tabs[index].sheet});
        if let Some(path) = path {
            args["file"] = json!(path);
        }
        self.panels.choices(
            &mut self.workspace,
            vec![vel_config::ui::PaletteItem {
                caption: "Save as UTF-8".into(),
                invocation: vel_config::keymap::Invocation {
                    command: "save_encoding".into(),
                    args,
                },
            }],
        );
        self.panels.set_message(UiText::tr_fmt(
            "{detail}. Choose Save as UTF-8 to save these characters.",
            [("detail", error.to_owned())],
        ));
        true
    }
    fn next_utf8_save(&mut self) {
        if self.panels.overlay.is_some() {
            return;
        }
        while let Some((sheet, path, error)) = self.pending_utf8_saves.pop_front() {
            if let Some(index) = self.workspace.sheet_index(sheet) {
                self.offer_utf8_save(index, path, &error);
                break;
            }
        }
    }
    /// One shared dispatcher for native menus, user keymaps and headless tests.
    fn format_action(
        &mut self,
        window: Option<&WindowHandle>,
        name: &str,
        args: &Value,
    ) -> Result<bool, String> {
        let active = self.workspace.active;
        match name {
            "set_line_ending" | "set_line_endings" => {
                // https://docs.sublimetext.io/reference/commands.html#set-line-ending
                self.workspace.tabs[active].format.line_ending = vel_fileio::LineEnding::parse(
                    args["type"].as_str().ok_or("set_line_ending needs type")?,
                )?;
            }
            "set_encoding" => {
                self.workspace.tabs[active].format = vel_fileio::encoding_format(
                    args["encoding"]
                        .as_str()
                        .ok_or("set_encoding needs encoding")?,
                    self.workspace.tabs[active].format.line_ending,
                )?;
                self.workspace.tabs[active].encoding_chosen = true;
                self.workspace.tabs[active].saved_in_effective_encoding = false;
                self.workspace.tabs[active].preferred_encoding =
                    Some(self.workspace.tabs[active].format.display_encoding().into());
            }
            "reopen" | "reopen_encoding" => {
                if self.workspace.tabs[active].dirty() {
                    let window = window.ok_or("Reopen requires confirmation of unsaved changes")?;
                    if !self.can_close_document(window, active) {
                        return Ok(true);
                    }
                }
                self.workspace
                    .reopen_encoding(args["encoding"].as_str().ok_or("missing encoding")?)?;
            }
            "revert" => {
                // Build 4142 confirms dirty-file reopen. Public references do not
                // specify Revert's undo policy; keep Velocity's existing reload behavior.
                // https://www.sublimetext.com/download
                let tr = self.workspace.translations.clone();
                self.workspace.revert(|| {
                    window.is_some_and(|w| {
                        Dialog::new(
                            UiText::tr("Revert File?"),
                            UiText::tr("Discard unsaved changes and reload this file from disk?"),
                            [UiText::tr("Revert"), UiText::tr("Cancel")],
                        )
                        .show(&tr, w)
                            == 0
                    })
                })?;
            }
            "save_all" => {
                let mut failures = Vec::new();
                let mut seen = std::collections::BTreeSet::new();
                for index in 0..self.workspace.tabs.len() {
                    if !self.workspace.tabs[index].dirty()
                        || !seen.insert(self.workspace.tabs[index].session_document)
                    {
                        continue;
                    }
                    let path = if self.workspace.tabs[index].path.is_none() {
                        let Some(path) = window.and_then(|window| {
                            self.choose_save_path(
                                window,
                                &self.workspace.tabs[index].ui_name(&self.workspace.ui),
                                self.save_dialog_directory(index).as_deref(),
                            )
                        }) else {
                            continue;
                        };
                        Some(path)
                    } else {
                        None
                    };
                    match self.save_tab_with_path(window, index, false, path.clone()) {
                        Ok(true) => continue,
                        Ok(false) => break, // Cancel stops the sequential document operation.
                        Err(error) => {
                            if error.contains("characters cannot be represented in") {
                                self.pending_utf8_saves.push_back((
                                    self.workspace.tabs[index].sheet,
                                    path,
                                    error,
                                ));
                            } else {
                                failures.push(format!(
                                    "{}: {error}",
                                    self.workspace.tabs[index].name()
                                ));
                            }
                        }
                    }
                }
                self.next_utf8_save();
                if !failures.is_empty() {
                    self.save_failure(window, failures.join("\n"));
                }
            }
            "save" | "save_encoding" | "save_and_close" | "save_as" | "prompt_save_as" => {
                let previous = (
                    self.workspace.tabs[active].format.clone(),
                    self.workspace.tabs[active].encoding_chosen,
                    self.workspace.tabs[active].preferred_encoding.clone(),
                );
                if name == "save_encoding" && args["encoding"].as_str().is_none() {
                    return Err("missing encoding".into());
                }
                let path = args["file"].as_str().map(std::path::PathBuf::from);
                let save_as = matches!(name, "save_as" | "prompt_save_as");
                if let Some(encoding) = args["encoding"].as_str() {
                    self.workspace.tabs[active].format = vel_fileio::encoding_format(
                        encoding,
                        self.workspace.tabs[active].format.line_ending,
                    )?;
                    self.workspace.tabs[active].encoding_chosen = true;
                    self.workspace.tabs[active].preferred_encoding = Some(encoding.into());
                }
                let saved =
                    if path.is_none() && (save_as || self.workspace.tabs[active].path.is_none()) {
                        // Save, Save As, and Save with Encoding all use the same panel
                        // for unnamed files. https://docs.sublimetext.io/reference/commands.html#save
                        window.is_some_and(|w| self.save_tab(w, active, save_as))
                    } else {
                        match self.save_tab_with_path(window, active, false, path.clone()) {
                            Ok(saved) => saved,
                            Err(error) => {
                                // A failed explicit save has not changed the disk format.
                                // Restore it before presenting a cancellable retry.
                                self.workspace.tabs[active].format = previous.0.clone();
                                self.workspace.tabs[active].encoding_chosen = previous.1;
                                self.workspace.tabs[active].preferred_encoding = previous.2.clone();
                                if !self.offer_utf8_save(active, path, &error) {
                                    self.save_failure(window, error);
                                }
                                false
                            }
                        }
                    };
                if !saved {
                    self.workspace.tabs[active].format = previous.0;
                    self.workspace.tabs[active].encoding_chosen = previous.1;
                    self.workspace.tabs[active].preferred_encoding = previous.2;
                }
                if saved && name == "save_and_close" {
                    self.workspace.close(active)?;
                    self.close_empty_window(window);
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
    fn can_close(&mut self, w: &WindowHandle, index: usize) -> bool {
        if self
            .workspace
            .tabs
            .iter()
            .filter(|t| t.session_document == self.workspace.tabs[index].session_document)
            .count()
            > 1
        {
            return true;
        }
        self.can_close_document(w, index)
    }
    fn can_close_many(&mut self, w: Option<&WindowHandle>, indices: &[usize]) -> bool {
        self.sync_workspace_after_panels();
        let mut seen = std::collections::BTreeSet::new();
        for index in indices {
            let doc = self.workspace.tabs[*index].session_document;
            if !seen.insert(doc)
                || self
                    .workspace
                    .tabs
                    .iter()
                    .enumerate()
                    .any(|(i, t)| t.session_document == doc && !indices.contains(&i))
            {
                continue;
            }
            if !self.needs_document_close_decision(*index) {
                continue;
            }
            match self.decide_close_document_with(w, *index, Self::needs_document_close_decision) {
                Ok(true) => (),
                Ok(false) => return false,
                Err(error) => {
                    self.panels.message = error.into();
                    return false;
                }
            }
        }
        true
    }
    fn can_close_document(&mut self, w: &WindowHandle, index: usize) -> bool {
        match self.decide_close_document(Some(w), index, false) {
            Ok(close) => close,
            Err(error) => {
                self.panels.message = error.into();
                false
            }
        }
    }
    fn needs_document_close_decision(&self, index: usize) -> bool {
        let tab = &self.workspace.tabs[index];
        self.workspace.document_dirty(tab)
            || (tab.path.is_none()
                && !tab.editor.read_only
                && (!tab.editor.text.is_empty()
                    || tab.lazy.as_ref().is_some_and(|(doc, _)| {
                        doc.history.as_ref().is_some_and(|history| {
                            history.current_text().is_ok_and(|text| !text.is_empty())
                        })
                    })))
    }
    // The provider supplies only the UI answer. Native and headless callers share
    // dirty checks, save-as cancellation, I/O errors and the actual save path.
    fn decide_close_document(
        &mut self,
        window: Option<&WindowHandle>,
        index: usize,
        contents_only: bool,
    ) -> Result<bool, String> {
        self.decide_close_document_with(window, index, |app, index| {
            let tab = &app.workspace.tabs[index];
            if contents_only {
                app.workspace.document_contents_dirty(tab)
            } else {
                app.workspace.document_dirty(tab)
            }
        })
    }
    fn decide_close_document_with(
        &mut self,
        window: Option<&WindowHandle>,
        index: usize,
        needs_decision: impl FnOnce(&Self, usize) -> bool,
    ) -> Result<bool, String> {
        self.workspace.prepare_sheet(index)?;
        let editor = &mut self.workspace.tabs[index].editor;
        if editor.marked.is_some() {
            editor.cancel_composition();
        }
        if !needs_decision(self, index) {
            return Ok(true);
        }
        let decision = if let Some(window) = window {
            match self
                .unsaved_changes_dialog(index)
                .show(&self.workspace.translations, window)
            {
                0 => tab_adapter::CloseDecision::Save(None),
                1 => tab_adapter::CloseDecision::Discard,
                _ => tab_adapter::CloseDecision::Cancel,
            }
        } else {
            #[cfg(test)]
            {
                self.close_decisions
                    .pop_front()
                    .ok_or("Unsaved changes require a save decision")?
            }
            #[cfg(not(test))]
            {
                return Err("Unsaved changes require a save decision".into());
            }
        };
        match decision {
            tab_adapter::CloseDecision::Save(path) => {
                match self.save_tab_with_path(window, index, false, path) {
                    Ok(saved) => Ok(saved),
                    Err(error) => {
                        self.save_failure(window, error);
                        Ok(false)
                    }
                }
            }
            tab_adapter::CloseDecision::Discard => Ok(true),
            tab_adapter::CloseDecision::Cancel => Ok(false),
        }
    }
    fn unsaved_changes_dialog(&self, index: usize) -> Dialog {
        Dialog::new(
            UiText::tr("Save changes?"),
            self.workspace.tabs[index].ui_name(&self.workspace.ui),
            [
                UiText::tr("Save"),
                UiText::tr("Don't Save"),
                UiText::tr("Cancel"),
            ],
        )
    }
    fn handle_named_actions(&mut self, window: Option<&WindowHandle>) -> Result<(), String> {
        let actions = std::mem::take(&mut self.workspace.editor_mut().actions);
        let mut result = Ok(());
        for (name, args) in actions {
            if !self.macro_view_active() {
                break;
            }
            let previous = self.workspace.active;
            self.workspace.tabs[previous].scroll = (self.view.scroll_x, self.view.scroll_y);
            self.workspace.tabs[previous].scroll_line_height = self.view.line_height;
            if let Err(error) = self.apply_named_action(window, &name, &args) {
                // Keep draining actions already produced by one command, as before.
                result = Err(error);
            }
            self.sync_active_view();
        }
        self.workspace.editor_mut().sync();
        if let Some(window) = window {
            window.set_title(&self.workspace.window_title());
        }
        result
    }
    fn macro_view_active(&mut self) -> bool {
        if self
            .macro_origin
            .is_some_and(|sheet| self.workspace.tabs[self.workspace.active].sheet != sheet)
        {
            self.panels.set_message(UiText::tr(
                "Macro playback stopped: the starting view is no longer active",
            ));
            return false;
        }
        true
    }
    fn apply_named_action(
        &mut self,
        window: Option<&WindowHandle>,
        name: &str,
        args: &Value,
    ) -> Result<(), String> {
        if matches!(name, "enable_finder_new_file" | "disable_finder_new_file") {
            #[cfg(target_os = "macos")]
            if window.is_some() {
                self.set_finder_new_text_file_enabled(name == "enable_finder_new_file");
            }
            return Ok(());
        }
        if name == "open_terminal" {
            let launch = self.terminal_launch(args)?;
            if window.is_some() {
                launch.open()?;
            }
            return Ok(());
        }
        if name == "choose_font" {
            if let Some(w) = window {
                #[cfg(target_os = "macos")]
                self.font_choices(w.font_families());
                #[cfg(windows)]
                {
                    let _ = w;
                    self.font_choices(vel_platform::fonts::font_families());
                }
            }
            return Ok(());
        }
        if name == "set_font_face" {
            let family = args["family"]
                .as_str()
                .filter(|name| !name.is_empty())
                .ok_or("Missing font family")?;
            return self
                .workspace
                .set_user_preference("font_face", json!(family));
        }
        if name == "set_ui_language" {
            // The preference is app-wide; the ordinary User writer also updates
            // other windows via resource reload. https://www.sublimetext.com/docs/settings.html
            let (language, note) = match args["language"].as_str() {
                Some(requested) if self.workspace.resolve_ui_language(requested).is_some() => {
                    (requested, None)
                }
                _ => (
                    "en",
                    Some(self.workspace.ui.unsupported_language.to_string()),
                ),
            };
            if self.workspace.ui_language_setting() != language {
                self.workspace
                    .set_user_preference("ui_language", json!(language))?;
                if self.workspace.sync_ui_language() {
                    self.refresh_open_palette();
                }
            }
            if let Some(note) = note {
                self.panels.set_message(note);
            }
            self.panels.refresh_locale(&self.workspace);
            return Ok(());
        }
        if name == "native_about" {
            if let Some(window) = window {
                self.about_dialog()
                    .show(&self.workspace.translations, window);
            }
            return Ok(());
        }
        if name == "command_note" {
            self.panels.message = args["message"].as_str().unwrap_or_default().into();
            return Ok(());
        }
        if name == "add_folder" && args["dir"].is_null() {
            // Folder selection precedes the add_folder action.
            // https://docs.sublimetext.io/reference/commands.html
            if let Some(path) = window.and_then(WindowHandle::open_directory) {
                self.panels.add_folder(&mut self.workspace, path)?;
            }
            return Ok(());
        }
        if name == "convert_to_encoding" {
            let rows = vel_fileio::ENCODINGS
                .iter()
                .map(|&(encoding, _)| vel_config::ui::PaletteItem {
                    caption: encoding.into(),
                    invocation: vel_config::keymap::Invocation {
                        command: "set_encoding".into(),
                        args: json!({"encoding":encoding}),
                    },
                })
                .collect();
            self.panels.choices(&mut self.workspace, rows);
            return Ok(());
        }
        if name == "show_console" {
            self.panels.toggle_console(&mut self.workspace);
            return Ok(());
        }
        if name == "widget_history" {
            if self.panels.restore_widget_history(args) {
                let viewport = self.viewport();
                self.panels.edited(&mut self.workspace, viewport);
            }
            return Ok(());
        }
        if name == "move_tab_to_new_window" {
            // Like the Window menu's tab operations in Apple's HIG, keep the
            // document (and undo history) intact: transfer, don't save/reopen.
            // https://developer.apple.com/design/human-interface-guidelines/the-menu-bar#Window-menu
            if let Some(window) = window {
                let sheet = self.workspace.tabs[self.workspace.active].sheet;
                self.workspace.prepare_sheet(self.workspace.active)?;
                if self.transferable_tab(self.workspace.active) {
                    let (_, destination) = self.open_native_window(window, None, None, false)?;
                    self.move_tab_to_window(Some(window), sheet, destination, 0)?;
                }
            }
            return Ok(());
        }
        if self.apply_tab_close_action(window, name, args)? {
            return Ok(());
        }
        if name == "merge_all_windows" {
            let sources = self.merge_sources();
            self.sessions
                .borrow_mut()
                .menu_merges
                .extend(sources.into_iter().map(|source| (source, self.session_id)));
            return Ok(());
        }
        if self.apply_settings_action(window, name, args)? {
            return Ok(());
        }
        if self.format_action(window, name, args)? {
            return Ok(());
        }
        if self.apply_view_action(name, args)? {
            return Ok(());
        }
        if matches!(name, "focus_next_control" | "focus_previous_control") {
            self.next_accessibility_focus(window, name == "focus_previous_control");
            return Ok(());
        }
        if self.apply_build_action(window, name, args)? {
            return Ok(());
        }
        if matches!(name, "open_as_image" | "open_as_text") {
            let Some(index) = args["_context_sheet"]
                .as_u64()
                .map(|sheet| self.workspace.sheet_index(sheet))
                .unwrap_or(Some(self.workspace.active))
            else {
                return Ok(());
            };
            self.workspace.set_open_as(index, image_open_as(name))?;
            return Ok(());
        }
        if name == "show_at_center" {
            self.view.center_caret(self.workspace.editor());
            return Ok(());
        }
        let viewport = self.viewport();
        if matches!(name, "run_macro" | "run_macro_file") {
            if let Some(playback) =
                self.panels
                    .macro_playback(&mut self.workspace, name, args, viewport)?
            {
                let _recording = playback.recording;
                let previous = self
                    .macro_origin
                    .replace(self.workspace.tabs[self.workspace.active].sheet);
                let result = (|| {
                    for (command, args) in playback.commands.iter() {
                        if !self.macro_view_active() {
                            break;
                        }
                        if self.workspace.editor().registry.kind(command).is_none() {
                            return Err(format!("unknown command {command}"));
                        }
                        self.run_named_depth(window, command, args, 0)?;
                        if !self.macro_view_active() {
                            break;
                        }
                    }
                    Ok(())
                })();
                self.macro_origin = previous;
                result?;
            }
            return Ok(());
        }
        if matches!(name, "next_result" | "prev_result" | "current_result") {
            return match (self.panels.search_results_active, name) {
                (true, "current_result") => self.panels.current_result(&mut self.workspace),
                (true, _) => self
                    .panels
                    .navigate_result(&mut self.workspace, name == "next_result"),
                (false, "current_result") => self.builds.current_result(&mut self.workspace),
                (false, _) => self
                    .builds
                    .navigate(&mut self.workspace, name == "next_result"),
            };
        }
        if name == "run_syntax_tests" {
            return self
                .builds
                .start_syntax_tests(&self.workspace, &mut self.panels, args);
        }
        // These actions need only the workspace/panels. Both native and headless named
        // dispatch finish them here before advancing to the next macro step.
        if vel_commands::is_find_command(name)
            || matches!(
                name,
                "save_macro"
                    | "cancel_find_in_files"
                    | "refresh_folder_list"
                    | "close_folder_list"
                    | "toggle_show_open_files"
            )
        {
            self.panels
                .action(&mut self.workspace, name, args, viewport)?;
            return Ok(());
        }
        if name == "new_file" {
            return self.workspace.new_file();
        }
        if name == "reopen_last_file" {
            if self.workspace.reopen_last_file()? {
                return Ok(());
            }
            loop {
                let path = self.sessions.borrow_mut().pop_closed_window_file();
                let Some(path) = path else { return Ok(()) };
                if matches!(path.try_exists(), Ok(false)) {
                    continue;
                }
                return self.workspace.open(crate::cli::FileLocation {
                    path,
                    line: 1,
                    column: 1,
                });
            }
        }
        if name == "move_tab_to_window" {
            let sheet = args["sheet"]
                .as_u64()
                .unwrap_or(self.workspace.tabs[self.workspace.active].sheet);
            return self.move_tab_to_window(
                window,
                sheet,
                args["window"]
                    .as_u64()
                    .ok_or("Expected a destination window")?,
                args["group"].as_u64().map_or(usize::MAX, |n| n as usize),
            );
        }
        if name == "save_all" {
            let mut seen = std::collections::BTreeSet::new();
            let mut errors = Vec::new();
            for index in 0..self.workspace.tabs.len() {
                let tab = &self.workspace.tabs[index];
                if !tab.dirty() || !seen.insert(tab.session_document) {
                    continue;
                }
                if let Err(error) = self.save_tab_with_path(window, index, false, None) {
                    errors.push(error);
                }
            }
            if !errors.is_empty() {
                self.save_failure(window, errors.join("\n"));
            }
            return Ok(());
        }
        if matches!(name, "save" | "save_and_close") {
            return self.apply_save_action(window, name, args);
        }
        if matches!(
            name,
            "increase_font_size" | "decrease_font_size" | "reset_font_size"
        ) {
            return self.workspace.zoom_font(name);
        }
        if name == "open_file"
            && let Some(file) = args["file"].as_str()
        {
            return self.workspace.open_file_argument(file, args);
        }
        if name == "open_recent_file" {
            let path = Path::new(args["file"].as_str().ok_or("missing file")?);
            return crate::open_requests::open_paths(
                &mut self.workspace,
                &mut self.panels,
                &[path.to_owned()],
            );
        }
        if name == "prompt_open_file" {
            let paths = window.map(|w| w.open_files());
            #[cfg(test)]
            let paths = paths.or_else(|| self.open_file_choices.take());
            return match paths {
                Some(paths) => {
                    crate::open_requests::open_paths(&mut self.workspace, &mut self.panels, &paths)
                }
                None => Ok(()),
            };
        }
        if name == "switch_file" {
            return self.workspace.switch_file(&args["extensions"]);
        }
        if name == "clear_recent_items" {
            self.sync_recents();
            self.workspace.recent_files.clear();
            self.workspace.recent_folders.clear();
            self.workspace.recent_projects.clear();
            self.workspace.goto_recent_files.clear();
            self.sessions.borrow_mut().clear_recent(self.session_id);
            if let Some(window) = window {
                self.install_native_menu(window, false);
            }
            return Ok(());
        }
        if name == "remove_deleted_recent_projects" {
            self.sync_recents();
            self.sessions.borrow_mut().remove_deleted_recent_projects(
                self.session_id,
                &mut self.workspace.recent_projects,
            );
            if let Some(window) = window {
                self.install_native_menu(window, false);
            }
            return Ok(());
        }
        if name == "toggle_indexing" {
            let enabled = self.workspace.settings.bool("index_files", true);
            self.workspace
                .set_user_preference("index_files", json!(!enabled))?;
            let mut status = self.panels.intel.status_source.lock().unwrap();
            status.enabled = self.workspace.settings.bool("index_files", true);
            return Ok(());
        }
        if matches!(name, "quick_switch_project" | "recent_projects") {
            self.sync_recents();
            let items = self
                .workspace
                .recent_projects
                .iter()
                .map(|p| vel_config::ui::PaletteItem {
                    caption: p.display().to_string(),
                    invocation: vel_config::keymap::Invocation {
                        command: "open_project".into(),
                        args: json!({"file":p, "_quick_switch":name == "quick_switch_project"}),
                    },
                })
                .collect();
            self.panels.choices(&mut self.workspace, items);
            return Ok(());
        }
        if matches!(name, "open_project" | "open_project_or_workspace")
            && args["new_window"] != true
            && let Some(path) = args["file"].as_str()
        {
            return self
                .switch_to_project(window, vel_session::project::project_path(Path::new(path)));
        }
        if matches!(name, "clear_recent_files" | "clear_recent_projects") {
            let kind = if name == "clear_recent_files" {
                self.workspace.recent_files.clear();
                self.workspace.goto_recent_files.clear();
                0
            } else {
                self.workspace.recent_projects.clear();
                2
            };
            self.sessions
                .borrow_mut()
                .clear_recent_kind(self.session_id, kind);
            return Ok(());
        }
        if self.select_tab_action(name, args) {
            return Ok(());
        }
        if let Some(window) = window {
            return self.apply_action(window, name, args);
        }
        if self.workspace.workbench_action(name, args)? {
            self.panels.chrome = self.workspace.window_visible("show_status_bar");
            self.panels.side_visible = self.workspace.window_visible("show_side_bar");
            return Ok(());
        }
        if self
            .panels
            .action(&mut self.workspace, name, args, viewport)?
        {
            return Ok(());
        }
        self.panels
            .set_message(UiText::tr("This action is available in an editor window."));
        Ok(())
    }
    fn apply_action(&mut self, w: &WindowHandle, name: &str, args: &Value) -> Result<(), String> {
        if self.apply_view_action(name, args)? {
            return Ok(());
        }
        let viewport = self.viewport();
        if self.releases.action(
            &mut self.workspace,
            &mut self.panels,
            w,
            name,
            args,
            viewport,
        )? {
            return Ok(());
        }
        if name == "toggle_distraction_free" {
            self.workspace.fullscreen = w.is_full_screen();
        }
        if self.workspace.workbench_action(name, args)? {
            if name == "toggle_distraction_free" {
                self.panels.chrome = self.workspace.window_visible("show_status_bar");
                self.panels.side_visible = self.workspace.window_visible("show_side_bar");
                if self.workspace.distraction_free {
                    self.fullscreen_before_distraction = self.workspace.fullscreen;
                    if !self.workspace.fullscreen {
                        self.workspace.fullscreen = true;
                        w.toggle_full_screen_with_mode(
                            self.workspace
                                .editor()
                                .settings
                                .bool("use_simple_full_screen", false),
                        );
                    }
                } else {
                    if !self.fullscreen_before_distraction && self.workspace.fullscreen {
                        self.workspace.fullscreen = false;
                        w.toggle_full_screen_with_mode(
                            self.workspace
                                .editor()
                                .settings
                                .bool("use_simple_full_screen", false),
                        );
                    }
                }
            }
            return Ok(());
        }
        match name {
            "prompt_open_folder_in_new_window" => {
                if let Some(request) = Self::folder_window_request(w.open_directory())? {
                    self.open_request_in_new_window(w, request)?;
                }
            }
            "open_url" => {
                let url = args["url"].as_str().ok_or("missing URL")?;
                if matches!(
                    vel_minihtml::LinkTarget::parse(url),
                    Ok(vel_minihtml::LinkTarget::Browser(_))
                ) {
                    #[cfg(windows)]
                    w.open_url(url)?;
                    #[cfg(target_os = "macos")]
                    std::process::Command::new("/usr/bin/open")
                        .arg(url)
                        .spawn()
                        .map_err(|e| e.to_string())?;
                }
            }
            "minihtml_navigate" => {
                self.panels.message = args["href"].as_str().unwrap_or("").into();
            }
            "show_indexing_status" => {
                self.panels.refresh_locale(&self.workspace);
                let source = self.panels.intel.status_source.clone();
                let title = {
                    let mut status = source.lock().unwrap();
                    status.enabled = self.workspace.settings.bool("index_files", true);
                    status.labels.title.to_string()
                };
                w.new_window(
                    WindowOptions {
                        title,
                        size: Size {
                            width: 720.,
                            height: 480.,
                        },
                        ..Default::default()
                    },
                    crate::intelligence::StatusWindow::new(source, w.clone()),
                )?;
                return Ok(());
            }
            "toggle_full_screen" => {
                self.workspace.fullscreen = !self.workspace.fullscreen;
                w.toggle_full_screen_with_mode(
                    self.workspace
                        .editor()
                        .settings
                        .bool("use_simple_full_screen", false),
                );
                return Ok(());
            }
            "prompt_open_project"
            | "open_project"
            | "open_recent_project"
            | "open_project_or_workspace" => {
                let path = args["file"]
                    .as_str()
                    .map(std::path::PathBuf::from)
                    .or_else(|| {
                        if name == "open_recent_project" {
                            self.workspace
                                .recent_projects
                                .get(args["index"].as_u64()? as usize)
                                .cloned()
                        } else {
                            w.open_files().into_iter().next()
                        }
                    });
                if let Some(path) = path {
                    let path = vel_session::project::project_path(&path);
                    if args["new_window"] == true {
                        self.open_native_window_request(
                            w,
                            crate::instance::Request {
                                project: Some(path),
                                ..crate::instance::Request::new(&Options::default(), None)?
                            },
                        )?;
                        return Ok(());
                    }
                    return self.switch_to_project(Some(w), path);
                }
                return Ok(());
            }
            "save_project_as" => {
                if let Some(path) = args["file"]
                    .as_str()
                    .map(std::path::PathBuf::from)
                    .or_else(|| self.choose_save_path(w, "project.sublime-project", None))
                    && let Err(error) = self.workspace.save_project_as(path)
                {
                    self.save_failure(Some(w), error);
                }
                return Ok(());
            }
            "close_project" => {
                if !self.retain_switching_project()
                    && !self.can_close_many(
                        Some(w),
                        &(0..self.workspace.tabs.len()).collect::<Vec<_>>(),
                    )
                {
                    return Ok(());
                }
                self.checkpoint_reason(false, true)?;
                self.sessions.borrow().flush()?;
                let retain = self.retain_switching_project();
                self.workspace.close_project()?;
                self.sessions
                    .borrow_mut()
                    .retain_departing(self.session_id, retain);
                self.panels.reset(&self.workspace);
                return Ok(());
            }
            "edit_project" => {
                if let Some(path) = self.workspace.project_path().map(Path::to_owned) {
                    self.workspace.open(crate::cli::FileLocation {
                        path,
                        line: 1,
                        column: 1,
                    })?;
                }
                return Ok(());
            }
            _ => {}
        }
        if matches!(name, "open_dir" | "add_folder") && args["dir"].is_null() {
            if let Some(path) = w.open_directory() {
                self.panels.add_folder(&mut self.workspace, path)?;
            }
            return Ok(());
        }
        let viewport = self.viewport();
        if name == "show_overlay" && args["overlay"] == "command_palette" {
            self.sync_palette_enabled();
        }
        if self
            .panels
            .action(&mut self.workspace, name, args, viewport)?
        {
            self.replacement_confirmation(w);
            return Ok(());
        }
        let active = self.workspace.active;
        match name {
            "new_file" => self.workspace.new_file()?,
            "exit" => w.request_app_quit(),
            #[cfg(windows)]
            "next_window" | "prev_window" => w.cycle_editor_window(name == "next_window"),
            "open_recent_folder" => {
                self.open_native_window_request(
                    w,
                    crate::instance::Request {
                        files: vec![crate::cli::FileLocation {
                            path: args["dir"].as_str().ok_or("missing directory")?.into(),
                            line: 1,
                            column: 1,
                        }],
                        ..crate::instance::Request::new(&Options::default(), None)?
                    },
                )?;
            }
            "open_file" => {
                if let Some(file) = args["file"].as_str() {
                    self.workspace.open_file_argument(file, args)?;
                } else if (args["force"].as_bool().unwrap_or(false) || self.can_close(w, active))
                    && let Some(path) = self.workspace.tabs[active].path.clone()
                {
                    let decode = self.workspace.tabs[active].decode_options();
                    if self.workspace.editor().binary.is_some() {
                        self.workspace.tabs[active].binary_loading =
                            Some(crate::binary::Task::open(path)?);
                    } else {
                        self.workspace.tabs[active].loading =
                            Some(vel_fileio::load_background_with_options(path, decode)?);
                    }
                    self.workspace.tabs[active].editor.read_only = true;
                }
            }
            "prompt_save_as" => {
                self.save_tab(w, active, true);
            }
            "next_view" | "prev_view" | "next_view_in_stack" | "prev_view_in_stack"
            | "select_by_index" => {
                self.select_tab_action(name, args);
            }
            "new_window" => {
                self.open_native_window(w, None, None, true)?;
            }
            "close_window" => {
                w.request_close();
            }
            "increase_font_size" | "decrease_font_size" | "reset_font_size" => {
                self.workspace.zoom_font(name)?;
            }
            "show_at_center" => {
                let caret = self
                    .view
                    .rect_at(self.workspace.editor(), self.workspace.editor().caret);
                self.view.scroll(
                    self.workspace.editor(),
                    0.,
                    caret.y - self.view.size.height / 2.,
                );
            }
            _ => return Err(format!("unknown window action {name}")),
        }
        Ok(())
    }
    fn font_choices(&mut self, mut families: Vec<String>) {
        let current = self.workspace.editor().preferences.font_face.as_str();
        families.sort_unstable();
        families.dedup();
        if let Some(n) = families.iter().position(|name| name == current) {
            families[..=n].rotate_right(1);
        } else if !current.is_empty() {
            families.insert(0, current.to_owned());
        }
        let choices = families
            .into_iter()
            .map(|family| vel_config::ui::PaletteItem {
                invocation: vel_config::keymap::Invocation {
                    command: "set_font_face".into(),
                    args: json!({"family": family}),
                },
                caption: family,
            })
            .collect();
        self.panels.choices(&mut self.workspace, choices);
    }

    fn select_tab_action(&mut self, name: &str, args: &Value) -> bool {
        if name == "unselect_current" {
            let group = &self.workspace.workbench.groups[self.workspace.workbench.active];
            if group.selected.len() > 1
                && let Some(index) = group.focused.and_then(|id| self.workspace.sheet_index(id))
            {
                self.workspace.click_sheet(index, true, false);
            }
            return true;
        }
        if name == "select_by_index"
            && let Some(sheet) = args["_dropdown_sheet"].as_u64()
        {
            if let Some(index) = self.workspace.sheet_index(sheet) {
                if args["_replace_focused"] == true {
                    self.workspace.replace_focused_sheet(index);
                } else if args["_toggle_selected"] == true {
                    self.workspace.click_sheet(index, true, false);
                } else {
                    self.workspace.select(index);
                }
            }
            return true;
        }
        let group = &self.workspace.workbench.groups[self.workspace.workbench.active];
        let sheet = match name {
            // https://docs.sublimetext.io/reference/commands.html
            // Build 3059 persists Ctrl+Tab order (https://www.sublimetext.com/3).
            // The public references do not describe a release gesture: commit each step.
            "next_view_in_stack" | "prev_view_in_stack" => {
                group.stack_neighbor(name == "prev_view_in_stack")
            }
            "next_view" | "prev_view" => {
                let current = group
                    .sheets
                    .iter()
                    .position(|id| *id == self.workspace.tabs[self.workspace.active].sheet)
                    .unwrap_or(0);
                if group.sheets.is_empty() {
                    None
                } else {
                    let next = (current as isize + if name == "next_view" { 1 } else { -1 })
                        .rem_euclid(group.sheets.len() as isize)
                        as usize;
                    group.sheets.get(next).copied()
                }
            }
            // A negative index counts from the end: the documented Cmd+9 binding passes -1
            // to select the last tab (docs.sublimetext.io reference/keyboard_shortcuts_osx).
            "select_by_index" => args["index"]
                .as_i64()
                .and_then(|i| {
                    if i < 0 {
                        group.sheets.len().checked_sub(i.unsigned_abs() as usize)
                    } else {
                        Some(i as usize)
                    }
                })
                .and_then(|i| group.sheets.get(i))
                .copied(),
            _ => return false,
        };
        if let Some(index) = sheet.and_then(|id| self.workspace.sheet_index(id)) {
            self.workspace.select(index);
        }
        true
    }
    fn resize_views(&mut self, size: Size, scale: f64) {
        self.workspace.themes.ui.borrow_mut().density = scale as f32;
        let minimum = self.pane_window_minimum();
        self.window_size = Size {
            width: size.width.max(minimum.width),
            height: size.height.max(minimum.height),
        };
        // Pane widths are assigned by scene; apply_style anchors each view there.
        // Applying full-window width to the focused pane here causes a second reflow.
    }
    fn pane_window_minimum(&self) -> Size {
        let (width, height) = self.pane_minimum_extent();
        Size {
            width: (width + (self.window_size.width - self.workspace.pane_extent.0).max(0.))
                .max(320.),
            height: (height + (self.window_size.height - self.workspace.pane_extent.1).max(0.))
                .max(200.),
        }
    }
    fn pane_minimum_extent(&self) -> (f64, f64) {
        let layout = &self.workspace.workbench.layout;
        let mut size = self.workspace.pane_minimum;
        for &[l, t, r, b] in &layout.cells {
            size.0 = size
                .0
                .max(self.workspace.pane_minimum.0 / (layout.cols[r] - layout.cols[l]));
            size.1 = size
                .1
                .max(self.workspace.pane_minimum.1 / (layout.rows[b] - layout.rows[t]));
        }
        size
    }
    fn reload_font_views(&mut self, mut fonts: Fonts, scale: f64) {
        let advances = fonts.wide_advances();
        for view in std::iter::once(&mut self.view).chain(self.sheet_views.values_mut()) {
            view.set_font_grid(fonts.cell as f64, fonts.ascent as f64);
            view.set_font_metrics(fonts.metrics, scale);
            view.set_wide_advances(advances);
            view.ligatures = fonts.has_ligatures();
        }
        self.fonts = Some(fonts);
        self.initialize_view_metrics();
    }
    fn initialize_view_metrics(&mut self) {
        if let Some(fonts) = &mut self.fonts {
            let scale = self.workspace.themes.ui.borrow().density as f64;
            self.view.set_font_metrics(fonts.metrics, scale);
            self.view.ligatures = fonts.has_ligatures();
            self.view.set_wide_advances(fonts.wide_advances());
            self.panels
                .output_view
                .set_font_metrics(fonts.metrics, scale);
            // Build output is an editor view too: Chinese diagnostics and the localized
            // "[已完成…]" footer use the same measured wide advances as documents.
            self.panels
                .output_view
                .set_wide_advances(fonts.wide_advances());
        }
    }
    fn sync_active_view(&mut self) {
        let tab = &self.workspace.tabs[self.workspace.active];
        let (current, document) = (tab.sheet, tab.session_document);
        let replaced = self
            .view_goto_preview
            .is_some_and(|(sheet, previous)| sheet == current && previous != document);
        if tab.goto_preview {
            self.view_goto_preview = Some((current, document));
        } else if let Some((sheet, _)) = self.view_goto_preview
            && (sheet == current || self.workspace.sheet_index(sheet).is_none())
        {
            self.view_goto_preview = None;
            if sheet != current {
                self.sheet_views.remove(&sheet);
                self.inactive_image_decodes.remove(&sheet);
            }
        }
        let switched = self.view_buffer != Some(current) || replaced;
        if switched {
            self.caret_observed = None;
            let system_overlay = self.view.system_scroller_overlay;
            // An image's size or decode-failure note belongs to the document it was shown for.
            if let Some(status) = self.image_status.take()
                && self.panels.message == status
            {
                self.panels.message.clear();
            }
            register_window_commands(self.workspace.editor_mut());
            if let Some(old) = self.view_buffer {
                let replacement = View::new(
                    self.view.size,
                    self.fonts.as_ref().map_or(8., |f| f.cell as f64),
                    self.fonts.as_ref().map_or(12., |f| f.ascent as f64),
                );
                let view = std::mem::replace(&mut self.view, replacement);
                if !(replaced && old == current)
                    && let Some(index) = self.workspace.sheet_index(old)
                {
                    self.save_on_focus_lost(Some(old));
                    self.workspace.tabs[index].scroll = (view.scroll_x, view.scroll_y);
                    self.workspace.tabs[index].scroll_line_height = view.line_height;
                    if view.image_preview.busy() {
                        self.inactive_image_decodes.insert(old);
                    }
                    self.sheet_views.insert(old, view);
                }
            }
            let existing = self.sheet_views.remove(&current).filter(|_| !replaced);
            self.inactive_image_decodes.remove(&current);
            let reused = existing.is_some();
            if let Some(view) = existing {
                self.view = view;
            }
            self.view.set_system_scroller_overlay(system_overlay);
            self.initialize_view_metrics();
            self.view.apply_style(self.workspace.editor());
            self.view_buffer = Some(current);
            if let Some(bounds) = self.sheet_bounds.get(&current) {
                self.content = *bounds;
                self.view.size = Size {
                    width: bounds.width,
                    height: bounds.height,
                };
            }
            let tab = &self.workspace.tabs[self.workspace.active];
            let position = crate::view::ScrollPosition {
                x: tab.scroll.0,
                visual_row: tab.scroll.1 / tab.scroll_line_height,
            };
            if tab.loading.is_some() || tab.binary_loading.is_some() {
                self.view.defer_scroll_restore(&tab.editor, position);
            } else if !reused {
                self.view.restore_scroll_position(&tab.editor, position);
            }
        }
        self.inactive_image_decodes.retain(|&sheet| {
            let Some(view) = self.sheet_views.get_mut(&sheet) else {
                return false;
            };
            if self
                .workspace
                .workbench
                .groups
                .iter()
                .any(|g| g.selected.contains(&sheet))
            {
                return view.image_preview.busy();
            }
            view.image_preview.suspend();
            false
        });
        self.initialize_view_metrics();
        self.view.apply_style(self.workspace.editor());
        let tab = &self.workspace.tabs[self.workspace.active];
        let synced = self.view.image_preview.sync(tab, &self.workspace.folders);
        let active = self.workspace.active;
        // A decode in progress keeps the last known result (no "[hex]" flicker).
        if !self.view.image_preview.busy() {
            self.workspace.tabs[active].image_failed = self.view.image_preview.failed;
        }
        let status = if let Some(image) = self.view.image_preview.image.as_ref().filter(|_| synced)
        {
            let [width, height] = image.logical_size;
            Some(format!("{width} × {height}"))
        } else if self.view.image_preview.failed && (synced || switched) {
            Some(
                crate::image_decode::image_fallback_note(&self.workspace.translations).into_owned(),
            )
        } else {
            None
        };
        if let Some(status) = status {
            self.panels.set_message(status.clone());
            self.image_status = Some(status);
        }
        let tab = &self.workspace.tabs[active];
        if tab.loading.is_none() && tab.binary_loading.is_none() {
            self.view.finish_scroll_restore(&tab.editor);
        }
        if let Some(scroll) = self.panels.restore_scroll.take() {
            self.view.scroll_x = scroll.0;
            self.view.scroll_y = scroll.1;
        }
    }
    fn set_window_focused(&mut self, focused: bool) {
        if !focused {
            self.typing_hides_chrome = false;
        }
        if !focused
            && (self.panels.overlay == Some(crate::panels::Overlay::Appearance)
                || self.panels.navigation_overlay_open())
        {
            self.panels.cancel_overlay(&mut self.workspace);
            self.sync_active_view();
        }
        self.workspace.navigation_focus(focused);
        self.window_focused = focused;
    }
    /// Shared by tab deactivation and the window focus transition; never opens a dialog.
    fn save_on_focus_lost(&mut self, sheet: Option<u64>) {
        // https://docs.sublimetext.io/reference/settings.html#automatic-behavior
        // https://www.sublimetext.com/dev: respect read-only and reload conflicts.
        let index = if let Some(sheet) = sheet {
            let Some(index) = self.workspace.sheet_index(sheet) else {
                return;
            };
            index
        } else {
            self.workspace.active
        };
        self.workspace.tabs[index]
            .editor
            .finish_closing_tag_on_leave();
        let tab = &self.workspace.tabs[index];
        if tab.goto_preview
            || !tab.dirty()
            || !tab.editor.settings.bool("save_on_focus_lost", false)
            || tab.editor.read_only
            || tab.editor.marked.is_some()
            || tab.loading.is_some()
            || tab.lazy.is_some()
            || tab.binary_loading.is_some()
            || tab.external.is_some()
            || tab.deleted
        {
            return;
        }
        let Some(path) = tab.path.as_ref() else {
            return;
        };
        let Some(disk) = vel_session::DiskStamp::read(path) else {
            return;
        };
        // The watcher may not yet have published an external change. Compare disk now.
        if tab.disk_saved.as_ref() != Some(&disk) {
            return;
        }
        if let Err(error) = self.workspace.save(index, None) {
            self.panels.set_message(UiText::tr_fmt(
                "Automatic save failed: {detail}",
                [("detail", error)],
            ));
        }
    }
    fn sync_palette_enabled(&mut self) {
        self.workspace.finish_startup_palette();
        let enabled = self
            .workspace
            .palette
            .iter()
            .map(|item| {
                self.command_enabled(&item.invocation.command, &item.invocation.args.to_string())
            })
            .collect();
        self.panels.set_palette_enabled(enabled);
    }
    fn refresh_open_palette(&mut self) {
        if self.panels.overlay == Some(crate::panels::Overlay::Palette) {
            self.sync_palette_enabled();
        }
        self.panels.refresh_localized_palette(&mut self.workspace);
    }
    fn accept_palette_command(&mut self, name: &str, args: &Value) -> bool {
        let enabled = self.command_enabled(name, &args.to_string());
        self.panels
            .finish_palette_accept(&mut self.workspace, name, args, enabled);
        enabled
    }
    fn viewport(&self) -> std::ops::Range<usize> {
        self.view.viewport(self.workspace.editor())
    }
    fn replacement_confirmation(&mut self, w: &WindowHandle) {
        let tr = self.workspace.translations.clone();
        self.confirm_file_replacements(|count, files| {
            Dialog::new(
                UiText::tr("Replace in Files?"),
                UiText::tr_fmt(
                    "Replace {count} matches in {files} files?",
                    [("count", count.to_string()), ("files", files.to_string())],
                ),
                [UiText::tr("Cancel"), UiText::tr("Replace")],
            )
            .show(&tr, w)
                == 1
        });
    }
    fn about_dialog(&self) -> Dialog {
        Dialog::new(
            UiText::tr("About Velocity"),
            UiText::tr_fmt(
                "Version {version}",
                [("version", env!("CARGO_PKG_VERSION").into())],
            ),
            [UiText::tr("OK")],
        )
    }
    fn confirm_file_replacements(&mut self, confirm: impl FnOnce(usize, usize) -> bool) {
        let Some(plan) = self.panels.replacement_plan.take() else {
            return;
        };
        let result = self.check_replacement_targets(&plan).and_then(|()| {
            let count = plan.iter().map(|p| p.count).sum();
            if count > 0 && confirm(count, plan.len()) {
                self.commit_file_replacements(plan)
            } else {
                Ok(())
            }
        });
        if let Err(message) = result {
            let _ = self
                .reporter
                .emit(json!({"kind":"editor_error","message":message}));
            self.panels.set_message(UiText::command_error(&message));
        }
    }
    fn check_replacement_targets(
        &self,
        plan: &[vel_search::files::FileChange],
    ) -> Result<(), String> {
        if self
            .workspace
            .tabs
            .iter()
            .any(|t| t.dirty() && plan.iter().any(|c| t.path.as_ref() == Some(&c.path)))
        {
            return Err("A target has unsaved edits; save it before replacing in files".into());
        }
        Ok(())
    }
    fn commit_file_replacements(
        &mut self,
        plan: Vec<vel_search::files::FileChange>,
    ) -> Result<(), String> {
        self.check_replacement_targets(&plan)?;
        let mut count = 0;
        let files = plan.len();
        let mut warnings = Vec::new();
        for change in plan {
            let saved = change.commit().map_err(|e| {
                warnings.push(format!("Replaced {count} matches before stopping: {e}"));
                warnings.join("; ")
            })?;
            self.workspace
                .accept_file_replacement(&change, saved.identity);
            count += change.count;
            warnings.extend(saved.warning);
        }
        self.panels
            .report_file_replacements(&mut self.workspace, count, files)?;
        let message = UiText::tr_fmt("Replaced {count} matches.", [("count", count.to_string())]);
        self.panels.set_message(if warnings.is_empty() {
            message
        } else {
            message.with_raw_suffix(&format!(" {}", warnings.join("; ")))
        });
        Ok(())
    }
    fn scene(&mut self) -> vel_render::scene::Scene {
        self.ensure_startup_services();
        self.render_scene()
    }
    // A native draw alone does not need the deferred services before scanout.
    fn render_scene(&mut self) -> vel_render::scene::Scene {
        if self.workspace.settings_page.is_none() {
            self.sync_active_view();
            self.observe_caret_input(Instant::now());
        }
        if !self.workspace.themes.ui.borrow().metrics.native_enabled() {
            let result = self
                .workspace
                .themes
                .ui
                .borrow_mut()
                .metrics
                .enable_native();
            if let Err(e) = result {
                self.workspace.messages.push(e);
            }
        }
        if self.workspace.settings_page.is_some() {
            return self.settings_scene();
        }
        let settings = &self.workspace.tabs[self.workspace.active].editor.settings;
        if let Err(e) = self.workspace.themes.select(settings, self.workspace.dark) {
            self.workspace.messages.push(e);
        }
        self.panels.ui_scale = vel_ui::theme::ui_scale(settings);
        let interactive = matches!(
            self.options.mode,
            Mode::Interactive | Mode::Panels | Mode::ThemeUi
        ) || self.options.groups > 1;
        if !interactive {
            if let Some(fonts) = &self.fonts {
                self.view.ligatures = fonts.has_ligatures();
                self.view.set_font_metrics(
                    fonts.metrics,
                    self.workspace.themes.ui.borrow().density as f64,
                );
            }
            self.content = Rect {
                x: 0.,
                y: 0.,
                width: self.window_size.width,
                height: self.window_size.height,
            };
            self.view.size = self.window_size;
            if self.workspace.take_pending_center() {
                let e = self.workspace.editor();
                let from = self
                    .keyboard_center
                    .take()
                    .map(|_| self.view.prepare_keyboard_scroll(e, Instant::now()));
                self.view.center_caret(e);
                if let Some(from) = from {
                    self.view.animate_scroll_from(
                        e,
                        from,
                        Instant::now(),
                        self.accessibility_preferences.reduce_motion,
                    );
                }
            }
            let scene = self.view.scene(self.workspace.editor());
            self.exact_scene = self.view.exact(self.workspace.editor())
                && self.workspace.tabs[self.workspace.active].loading.is_none();
            return scene;
        }
        self.sync_workspace_after_panels();
        // The visible view set changes on selection, document/language replacement
        // or settings reload. Typing need not allocate the same map on every frame.
        self.visible_view_scratch.clear();
        for group in &self.workspace.workbench.groups {
            for id in &group.selected {
                if let Some(index) = self.workspace.sheet_index(*id) {
                    let tab = &self.workspace.tabs[index];
                    self.visible_view_scratch.push((
                        tab.document.0,
                        tab.shared_view.0,
                        tab.editor.settings.revision(),
                        std::rc::Rc::as_ptr(&tab.editor.language) as usize,
                    ));
                }
            }
        }
        let active_tab = &self.workspace.tabs[self.workspace.active];
        if active_tab.goto_preview {
            self.visible_view_scratch.push((
                active_tab.document.0,
                active_tab.shared_view.0,
                active_tab.editor.settings.revision(),
                std::rc::Rc::as_ptr(&active_tab.editor.language) as usize,
            ));
        }
        if self.visible_view_keys != self.visible_view_scratch || self.panels.overlay.is_some() {
            let mut visible = std::collections::BTreeMap::<
                vel_buffer::BufferId,
                std::collections::BTreeSet<vel_buffer::ViewId>,
            >::new();
            for &(document, view, _, _) in &self.visible_view_scratch {
                visible
                    .entry(vel_buffer::BufferId(document))
                    .or_default()
                    .insert(vel_buffer::ViewId(view));
            }
            for (document, views) in visible {
                if let Some(tab) = self.workspace.tabs.iter().find(|t| t.document == document)
                    && let Some(language) = tab.editor.language.borrow_mut().as_mut()
                {
                    language.retain_visible_views(&views);
                }
            }
            std::mem::swap(&mut self.visible_view_keys, &mut self.visible_view_scratch);
            // A preview may publish a temporary view during overlay processing.
            // Reconcile once more as soon as the overlay has closed.
            if self.panels.overlay.is_some() {
                self.visible_view_keys.clear();
            }
        }
        self.panels.sidebar_on_right = self.workspace.settings.bool("sidebar_on_right", false);
        self.panels.chrome_auto_hidden =
            self.typing_hides_chrome && self.workspace.settings.bool("auto_hide_status_bar", false);
        let mut bounds = self.panels.layout(self.window_size);
        self.workbench_bounds = bounds;
        self.sheet_bounds.clear();
        self.group_bounds.clear();
        self.tab_hits.clear();
        // Byte documents learn whether they are images from the same project folders the
        // decoder uses (their labels say "[hex]" otherwise).
        for tab in &self.workspace.tabs {
            tab.sync_image_file(&self.workspace.folders);
        }
        self.tab_bars.clear();
        self.tab_strips.clear();
        self.tab_painted.clear();
        let now = Instant::now();
        if !self.tab_slides_active(now) {
            self.tab_slides = None;
        }
        self.tab_button_hits.clear();
        self.tab_close_hits.clear();
        self.workspace
            .themes
            .ui
            .borrow_mut()
            .begin_frame(Instant::now());
        let mut theme_animating = false;
        let active = self.workspace.active;
        let focused = self.workspace.tabs[active].sheet;
        let editor_caret = self.editor_caret_has_focus();
        let focused_tab = if self.workspace.tabs[active].goto_preview {
            self.workspace.workbench.groups[self.workspace.workbench.active].focused
        } else {
            Some(focused)
        };
        let mut scene = vel_render::scene::Scene::default();
        let mut chrome = SceneLayer::default();
        let mut lifted_tab = SceneLayer::default();
        // Tab strip backgrounds sit below the tabs' layer, so a card tab's shadow (drawn first
        // within its layer) falls on the strip instead of behind it.
        let mut strip = SceneLayer::default();
        let mut exact = true;
        let tabset_style = self.workspace.themes.ui.borrow_mut().style(
            crate::panels::StyleId::TabSet.at(0),
            &vel_theme::ElementPath(&[
                vel_theme::Element::new("window", &[]),
                vel_theme::Element::new("tabset_control", &[]),
            ]),
            &self.workspace.editor().settings,
        );
        let show_tabs = self.tabs_visible();
        let tab_height = if show_tabs {
            f64::from(tabset_style.tab_height).max(16.)
        } else {
            0.
        };
        let tab_width = f64::from(tabset_style.tab_width).max(72.);
        self.workspace.pane_minimum = (120., tab_height + 3. * self.view.line_height.max(16.));
        let minimum = self.pane_minimum_extent();
        bounds.width = bounds.width.max(minimum.0);
        bounds.height = bounds.height.max(minimum.1);
        self.workspace.pane_extent = (bounds.width, bounds.height);
        self.workbench_bounds = bounds;
        let groups = self.workspace.workbench.groups.clone();
        for (group_index, group) in groups.iter().enumerate() {
            let Some(group_bounds) = self.workspace.workbench.layout.rect(group_index, bounds)
            else {
                continue;
            };
            self.group_bounds.push(group_bounds);
            // The active group of an empty window shows its tabless input; a Goto preview
            // shows its tabless sheet (both via displayed_sheets).
            let visible = self.workspace.displayed_sheets(group_index);
            let count = visible.len().max(1);
            for (position, sheet) in visible.iter().enumerate() {
                let Some(index) = self.workspace.sheet_index(*sheet) else {
                    continue;
                };
                let rect = Rect {
                    x: group_bounds.x + position as f64 * group_bounds.width / count as f64,
                    y: group_bounds.y,
                    width: group_bounds.width / count as f64,
                    height: group_bounds.height,
                };
                self.sheet_bounds.insert(*sheet, rect);
                if let Err(e) = self.workspace.prepare_sheet(index) {
                    self.workspace.messages.push(e);
                    continue;
                }
                let reused = *sheet == focused || self.sheet_views.contains_key(sheet);
                let mut other = if *sheet != focused {
                    Some(self.sheet_views.remove(sheet).unwrap_or_else(|| {
                        View::new(
                            Size {
                                width: rect.width,
                                height: rect.height,
                            },
                            self.fonts.as_ref().map_or(8., |f| f.cell as f64),
                            self.fonts.as_ref().map_or(12., |f| f.ascent as f64),
                        )
                    }))
                } else {
                    None
                };
                let view = if let Some(view) = other.as_mut() {
                    view.set_wide_advances(self.view.wide_advances);
                    view.set_system_scroller_overlay(self.view.system_scroller_overlay);
                    view
                } else {
                    self.content = rect;
                    &mut self.view
                };
                view.mouse_preferences = self.mouse_preferences;
                view.size = Size {
                    width: rect.width,
                    height: rect.height,
                };
                view.appearance = if self.workspace.dark {
                    Appearance::Dark
                } else {
                    Appearance::Light
                };
                view.tab_height = if !show_tabs { 0. } else { tab_height };
                view.focused = *sheet == focused && editor_caret && self.window_focused;
                view.show_inactive_caret =
                    *sheet == focused && editor_caret && !self.window_focused;
                view.find_matches.clear();
                view.find_scroll_matches.clear();
                view.find_result_matches.clear();
                if let Some(fonts) = &self.fonts {
                    view.ligatures = fonts.has_ligatures();
                    view.set_font_metrics(
                        fonts.metrics,
                        self.workspace.themes.ui.borrow().density as f64,
                    );
                }
                if !reused {
                    let tab = &self.workspace.tabs[index];
                    let position = crate::view::ScrollPosition {
                        x: tab.scroll.0,
                        visual_row: tab.scroll.1 / tab.scroll_line_height,
                    };
                    if tab.loading.is_some() || tab.binary_loading.is_some() {
                        view.defer_scroll_restore(&tab.editor, position);
                    } else {
                        view.restore_scroll_position(&tab.editor, position);
                    }
                }
                if std::mem::take(&mut self.workspace.tabs[index].pending_center) {
                    let e = &self.workspace.tabs[index].editor;
                    let animate = self.keyboard_center == Some(*sheet);
                    let from = animate.then(|| view.prepare_keyboard_scroll(e, Instant::now()));
                    view.center_caret(e);
                    if let Some(from) = from {
                        view.animate_scroll_from(
                            e,
                            from,
                            Instant::now(),
                            self.accessibility_preferences.reduce_motion,
                        );
                    }
                    if self.panels.overlay != Some(crate::panels::Overlay::Goto) {
                        self.keyboard_center = None;
                    }
                    self.workspace.tabs[index].scroll = (view.scroll_x, view.scroll_y);
                    self.workspace.tabs[index].scroll_line_height = view.line_height;
                }
                if self.workspace.tabs[index].find_results {
                    let tab = &self.workspace.tabs[index];
                    view.apply_style(&tab.editor);
                    view.find_result_matches = self
                        .panels
                        .results_tab_matches(tab, view.viewport(&tab.editor));
                }
                if *sheet == focused
                    && let highlights = self.panels.current_highlights(&self.workspace)
                    && !highlights.is_empty()
                {
                    view.apply_style(&self.workspace.tabs[index].editor);
                    let viewport = view.viewport(&self.workspace.tabs[index].editor);
                    let first = highlights.partition_point(|r| r.end < viewport.start);
                    view.find_matches.extend(
                        highlights[first..]
                            .iter()
                            .take_while(|r| r.start <= viewport.end)
                            .cloned(),
                    );
                    view.find_scroll_matches.extend(
                        self.panels
                            .scrollbar_find_highlights(&self.workspace)
                            .iter()
                            .map(|range| range.start),
                    );
                }
                theme_animating |= self.workspace.themes.ui.borrow().animating();
                if *sheet != focused {
                    view.image_preview
                        .sync(&self.workspace.tabs[index], &self.workspace.folders);
                    // A decode in progress keeps the last known result (no "[hex]" flicker).
                    if !view.image_preview.busy() {
                        self.workspace.tabs[index].image_failed = view.image_preview.failed;
                    }
                    if view.image_preview.busy() {
                        self.inactive_image_decodes.insert(*sheet);
                    } else {
                        self.inactive_image_decodes.remove(sheet);
                    }
                }
                let mut part = view.scene(&self.workspace.tabs[index].editor);
                theme_animating |= self.workspace.themes.ui.borrow().animating();
                exact &= view.exact(&self.workspace.tabs[index].editor)
                    && self.workspace.tabs[index].loading.is_none();
                crate::html_ui::paint_build_errors(
                    &self.workspace.tabs[index].editor,
                    view,
                    self.builds
                        .diagnostics
                        .iter()
                        .filter(|d| self.workspace.tabs[index].path.as_ref() == Some(&d.path)),
                    &mut part,
                );
                if *sheet != focused {
                    self.workspace.tabs[index].scroll = (view.scroll_x, view.scroll_y);
                    self.workspace.tabs[index].scroll_line_height = view.line_height;
                }
                if let Some(view) = other {
                    self.sheet_views.insert(*sheet, view);
                }
                translate_scene(&mut part, rect.x, rect.y);
                scene.layers.extend(part.layers);
            }
            if show_tabs {
                let bar = Rect {
                    height: tab_height,
                    ..group_bounds
                };
                let strip_bounds = bar;
                let preview = self.tab_preview_order(group_index, &group.sheets);
                let tabs = preview.as_deref().unwrap_or(&group.sheets);
                let mut theme = self.workspace.themes.ui.borrow_mut();
                let bar_path = [
                    vel_theme::Element::new("window", &[]),
                    vel_theme::Element::new("tabset_control", &[]),
                ];
                let bar_style = theme.style(
                    crate::panels::StyleId::TabBar.at(group_index as u64),
                    &vel_theme::ElementPath(&bar_path),
                    &self.workspace.editor().settings,
                );
                theme.surface(&mut strip, &bar_style, bar, bar);
                drop(theme);
                let bar =
                    self.paint_tab_buttons(&mut chrome, group_index, bar, tabs.len(), tab_width);
                // https://www.sublimetext.com/docs/themes.html#settings
                let tab_width = if self
                    .workspace
                    .editor()
                    .settings
                    .bool("enable_tab_scrolling", true)
                {
                    tab_width
                } else {
                    tab_width.min(bar.width / tabs.len().max(1) as f64)
                };
                let max_scroll = (tabs.len() as f64 * tab_width - bar.width).max(0.);
                self.tab_bars.push((group_index, bar, max_scroll));
                let mut scroll = group.tab_scroll.clamp(0., max_scroll);
                // When a group's focused sheet changes, scroll its tab into view once;
                // later wheel or button scrolling is left alone.
                if self.revealed_tabs.len() <= group_index {
                    self.revealed_tabs.resize(group_index + 1, None);
                }
                if let Some(target) = group.focused
                    && preview.is_none()
                    && self.revealed_tabs[group_index] != Some(target)
                    && let Some(position) = group.sheets.iter().position(|s| *s == target)
                {
                    let left = position as f64 * tab_width;
                    let wanted = scroll.min(left).max(left + tab_width - bar.width);
                    scroll = wanted.clamp(0., max_scroll);
                    self.workspace.workbench.groups[group_index].tab_scroll = scroll;
                    self.revealed_tabs[group_index] = Some(target);
                }
                self.tab_strips.push(tab_drag::TabStrip {
                    group: group_index,
                    bounds: strip_bounds,
                    clip: bar,
                    step: tab_width,
                    scroll,
                });
                for (position, sheet) in tabs.iter().enumerate() {
                    let Some(index) = self.workspace.sheet_index(*sheet) else {
                        continue;
                    };
                    let tab = &self.workspace.tabs[index];
                    let mut rect = Rect {
                        x: bar.x + position as f64 * tab_width - scroll,
                        y: bar.y,
                        width: (tab_width - 1.).max(tab_width * 0.5),
                        height: bar.height,
                    };
                    let lifted = self.lifted_tab_x(*sheet, bar, rect.width);
                    if let Some(x) = lifted {
                        rect.x = x;
                        rect.y -= 2. * self.panels.ui_scale;
                        rect.width = rect.width.min(bar.width);
                    } else {
                        rect.x = self.sliding_tab_x(*sheet, group_index, rect.x, now);
                    }
                    if rect.x + rect.width <= bar.x || rect.x >= bar.x + bar.width {
                        continue;
                    }
                    let hit = Rect {
                        x: rect.x.max(bar.x),
                        width: (rect.x + rect.width).min(bar.x + bar.width) - rect.x.max(bar.x),
                        ..rect
                    };
                    self.tab_hits.push((*sheet, hit));
                    self.tab_painted.push((*sheet, group_index, rect.x));
                    let paint = if lifted.is_some() {
                        &mut lifted_tab
                    } else {
                        &mut chrome
                    };
                    let selected = group.selected.contains(sheet);
                    let mut attrs = [""; 8];
                    let mut n = 0;
                    for (enabled, attr) in [
                        (selected || lifted.is_some(), "selected"),
                        (Some(*sheet) == focused_tab || lifted.is_some(), "focused"),
                        (tab.dirty(), "dirty"),
                        (tab.transient, "transient"),
                        (lifted.is_some(), "dragging"),
                    ] {
                        if enabled {
                            attrs[n] = attr;
                            n += 1;
                        }
                    }
                    if let Some(node) = self.panels.tree.nodes.get(&(100 + *sheet)) {
                        for attr in ["hover", "pressed"] {
                            if node.attributes.contains(attr) {
                                attrs[n] = attr;
                                n += 1;
                            }
                        }
                    }
                    attrs[n] = if self.workspace.dark {
                        "file_dark"
                    } else {
                        "file_light"
                    };
                    n += 1;
                    let path = [
                        bar_path[0],
                        bar_path[1],
                        vel_theme::Element::new("tab_control", &attrs[..n]),
                    ];
                    let mut theme = self.workspace.themes.ui.borrow_mut();
                    let settings = &self.workspace.editor().settings;
                    let tab_style =
                        theme.style(100 + *sheet, &vel_theme::ElementPath(&path), settings);
                    // A selected rounded tab joins the sheet below it, so only its top
                    // corners are round: its box runs one radius past the strip and is
                    // clipped there, leaving no notches at the sheet's edge.
                    // An inset (card) tab floats inside the strip instead.
                    let surface = if selected && tab_style.radius > 0. && tab_style.inset.is_none()
                    {
                        Rect {
                            height: rect.height + f64::from(tab_style.radius),
                            ..rect
                        }
                    } else {
                        rect
                    };
                    theme.surface(paint, &tab_style, surface, bar);
                    let label_path = [
                        path[0],
                        path[1],
                        path[2],
                        vel_theme::Element::new(
                            "tab_label",
                            if tab.dirty() && settings.bool("highlight_modified_tabs", false) {
                                &["dirty"]
                            } else {
                                &[]
                            },
                        ),
                    ];
                    let label = theme.style(
                        crate::panels::StyleId::TabLabel.at(*sheet),
                        &vel_theme::ElementPath(&label_path),
                        settings,
                    );
                    let icon = crate::panels::tab_icon(&self.workspace, tab);
                    let s = self.panels.ui_scale;
                    let narrow = rect.width < 48. * s;
                    // Close buttons follow show_tab_close_buttons; narrow fitted tabs keep
                    // their entire target for selecting the tab.
                    let close_left = settings.bool("show_tab_close_buttons_on_left", false)
                        || tab_style.close_left;
                    let close_rect = Rect {
                        x: if close_left {
                            rect.x + 6. * s
                        } else {
                            rect.x + rect.width - 26. * s
                        },
                        y: rect.y + ((rect.height - 20. * s) / 2.).round(),
                        width: 20. * s,
                        height: 20. * s,
                    };
                    // The close button exists only where it is entirely visible (tabs clipped
                    // by the strip edge or the scroll / new-tab buttons get no target there)
                    // and leaves the middle of the visible tab free: a click on the centre of
                    // a tab always selects it, however narrow or clipped the tab is.
                    let middle = hit.x + hit.width / 2.;
                    let has_close = settings.bool("show_tab_close_buttons", true)
                        && close_rect.x >= hit.x
                        && close_rect.x + close_rect.width <= hit.x + hit.width
                        && if close_left {
                            close_rect.x + close_rect.width < middle
                        } else {
                            close_rect.x > middle
                        };
                    let close_left = has_close && close_left;
                    let caption = if let Some((read, total)) = tab.progress() {
                        format!(
                            "{} · {}%",
                            tab.ui_label(&self.workspace.ui),
                            read.saturating_mul(100) / total.max(1)
                        )
                    } else {
                        // A drawn close button shows the dirty dot; tabs without one draw
                        // the bullet themselves (below) so truncation never hides it.
                        let label = tab.ui_label(&self.workspace.ui);
                        label
                            .strip_suffix(" •")
                            .map_or(label.clone(), str::to_owned)
                    };
                    let bullet = !has_close && tab.dirty() && tab.progress().is_none();
                    let label_x = rect.x
                        + (if close_left {
                            30.
                        } else if narrow {
                            4.
                        } else {
                            10.
                        } + if icon.is_some() && !narrow { 22. } else { 2. })
                            * s;
                    // Labels (and a dirty bullet) end inside the visible part of the tab.
                    let label_end = (rect.x + rect.width).min(hit.x + hit.width)
                        - if has_close && !close_left {
                            30.
                        } else if narrow {
                            2.
                        } else {
                            10.
                        } * s;
                    let baseline = rect.y + f64::from(label.size) + 8. * self.panels.ui_scale;
                    let renaming = self
                        .tab_rename
                        .as_mut()
                        .filter(|rename| rename.sheet == *sheet);
                    if let Some(rename) = renaming {
                        rename.paint(
                            paint,
                            &theme,
                            &label,
                            (
                                hit,
                                Point {
                                    x: label_x,
                                    y: baseline,
                                },
                            ),
                            self.workspace.dark,
                            s,
                        );
                    } else {
                        let label_clip = Rect {
                            width: rect.width.min(bar.x + bar.width - rect.x).max(0.),
                            ..rect
                        };
                        let label_clip = if lifted.is_some() {
                            vel_ui::minihtml::intersection(label_clip, bar)
                        } else {
                            label_clip
                        };
                        let label_end = if bullet {
                            // Right-aligned before the label's end; in a tab too narrow for that,
                            // centred in the visible part, and only when it fits there at all.
                            let width = theme.advance(&label, "•");
                            let x = if label_end - width >= hit.x + 1. {
                                label_end - width
                            } else {
                                hit.x + (hit.width - width) / 2.
                            };
                            if width <= hit.width {
                                theme.text(paint, &label, "•", x, baseline, label_clip);
                            }
                            x - 3. * s
                        } else {
                            label_end
                        };
                        // "Preferences.sublime-settings (Default)" keeps its "(Default)".
                        let shown = theme.fit_label(&label, &caption, label_end - label_x);
                        if !shown.is_empty() {
                            theme.text(paint, &label, &shown, label_x, baseline, label_clip);
                        }
                        let close_path = [
                            path[0],
                            path[1],
                            path[2],
                            vel_theme::Element::new(
                                "tab_close_button",
                                if self.hovered_close == Some(*sheet) {
                                    &["hover"]
                                } else {
                                    &[]
                                },
                            ),
                        ];
                        let close = theme.style(
                            crate::panels::StyleId::TabClose.at(*sheet),
                            &vel_theme::ElementPath(&close_path),
                            settings,
                        );
                        if has_close {
                            self.tab_close_hits.push((*sheet, close_rect));
                            theme.icon_surface(paint, &close, close_rect, hit);
                        }
                    }
                    // The file icon stays in its label inset, above an editing field's fill.
                    if let Some(icon) = icon.filter(|_| !narrow) {
                        theme.icon(
                            paint,
                            icon,
                            Rect {
                                x: rect.x + if close_left { 30. } else { 10. } * s,
                                y: rect.y + ((rect.height - 16. * s) / 2.).round(),
                                width: 16. * s,
                                height: 16. * s,
                            },
                            hit,
                            label.foreground,
                        );
                    }
                    drop(theme);
                    let id = 100 + *sheet;
                    let node = self
                        .panels
                        .tree
                        .nodes
                        .entry(id)
                        .or_insert_with(|| vel_ui::Node::new("tab_control"));
                    node.rect = hit;
                    node.visible = true;
                    node.attributes
                        .retain(|a| matches!(*a, "hover" | "pressed"));
                    if selected {
                        node.attributes.insert("selected");
                    }
                    if Some(*sheet) == focused_tab {
                        node.attributes.insert("focused");
                    }
                    if tab.dirty() {
                        node.attributes.insert("dirty");
                    }
                    if tab.transient {
                        node.attributes.insert("transient");
                    }
                    self.panels.tree.roots.insert(0, id);
                    if selected && group.selected.len() > 1 && lifted.is_none() {
                        let connector = Rect {
                            y: rect.y + rect.height - 2.,
                            height: 2.,
                            ..rect
                        };
                        let mut theme = self.workspace.themes.ui.borrow_mut();
                        let path = [
                            path[0],
                            path[1],
                            vel_theme::Element::new("tab_connector", &["selected"]),
                        ];
                        let style = theme.style(
                            crate::panels::StyleId::TabConnector.at(*sheet),
                            &vel_theme::ElementPath(&path),
                            &self.workspace.editor().settings,
                        );
                        theme.surface(&mut chrome, &style, connector, bar);
                        let id = (5 << 40) + *sheet;
                        let node = self
                            .panels
                            .tree
                            .nodes
                            .entry(id)
                            .or_insert_with(|| vel_ui::Node::new("tab_connector"));
                        node.rect = connector;
                        node.visible = true;
                        node.interactive = false;
                        node.attributes.insert("selected");
                    }
                }
            }
        }
        // Group and Split View dividers use the theme's grid border (a 1-DIP hairline);
        // splitter hit testing keeps its own tolerance and is unaffected.
        let grid = self.workspace.themes.ui.borrow_mut().style(
            crate::panels::StyleId::GroupDivider.at(0),
            &vel_theme::ElementPath(&[
                vel_theme::Element::new("window", &[]),
                vel_theme::Element::new("grid_layout_control", &[]),
            ]),
            &self.workspace.editor().settings,
        );
        let (color, thickness) = grid.border().map_or(([0.28, 0.3, 0.35, 1.], 1.), |(c, w)| {
            (c, f64::from(w.max(1.)))
        });
        for (_, divider) in self.workspace.workbench.layout.dividers(bounds, thickness) {
            vel_ui::fill(&mut chrome, divider, bounds, color);
        }
        for group_index in 0..groups.len() {
            let visible = self.workspace.displayed_sheets(group_index);
            if visible.len() < 2 {
                continue;
            }
            let Some(area) = self.group_bounds.get(group_index).copied() else {
                continue;
            };
            let top = area.y + if show_tabs { tab_height } else { 0. };
            for position in 1..visible.len() {
                let x = area.x + position as f64 * area.width / visible.len() as f64;
                vel_ui::fill(
                    &mut chrome,
                    Rect {
                        x: x - thickness / 2.,
                        y: top,
                        width: thickness,
                        height: (area.y + area.height - top).max(0.),
                    },
                    bounds,
                    color,
                );
            }
        }
        let _ = self.workspace.prepare_sheet(active);
        self.sync_workspace_after_panels();
        self.exact_scene = exact;
        if !strip.surfaces.is_empty() || !strip.rects.is_empty() {
            scene.layers.push(strip);
        }
        scene.layers.push(chrome);
        if self
            .tab_press
            .as_ref()
            .is_some_and(|press| press.drag.is_some())
        {
            scene.layers.push(lifted_tab);
        }
        self.panels.paint(&self.workspace, &mut scene);
        self.paint_accessibility_focus(&mut scene);
        self.capture_format_hits(&scene);
        if theme_animating {
            self.workspace.themes.ui.borrow_mut().request_animation();
        }
        scene
    }
    fn apply_title_theme(&mut self, w: &WindowHandle) {
        let key = (
            self.workspace.themes.ui.borrow().revision,
            self.workspace.editor().settings.cache_identity(),
        );
        if self.title_theme == Some(key) {
            return;
        }
        self.title_theme = Some(key);
        let (appearance, background) = self.title_bar_style();
        w.set_title_bar_theme(appearance, background);
    }
    fn title_bar_style(&self) -> (Option<Appearance>, Option<[f32; 4]>) {
        let settings = &self.workspace.editor().settings;
        if !settings.bool("themed_title_bar", true) {
            return (None, None);
        }
        let style = self.workspace.themes.ui.borrow_mut().style(
            crate::panels::StyleId::TitleBar.at(0),
            &vel_theme::ElementPath(&[
                vel_theme::Element::new("window", &[]),
                vel_theme::Element::new("title_bar", &[]),
            ]),
            settings,
        );
        let appearance = match style.title_style {
            Some(vel_theme::Keyword::Dark) => Some(Appearance::Dark),
            Some(vel_theme::Keyword::Light) => Some(Appearance::Light),
            _ => None,
        };
        (appearance, style.background)
    }
    fn log_input(&self, kind: &str, event: impl std::fmt::Debug) {
        if self.options.log_input {
            let state = self.text_input_state();
            let _ = self.reporter.emit(json!({"kind":"input","event_kind":kind,"event":format!("{event:?}"),"selected_utf16":[state.selected.location,state.selected.length],"marked_utf16":state.marked.map(|r|[r.location,r.length])}));
        }
    }
    fn changed(&mut self, w: &WindowHandle, received: Instant, reveal: bool) {
        self.changed_state(received, reveal);
        w.invalidate_character_coordinates();
        w.request_frame();
    }
    fn benchmark_scroll_step(&mut self) {
        let editor = self.workspace.editor();
        let max = (editor.text.len_lines() as f64 * self.view.line_height - self.view.size.height
            + 8.)
            .max(0.);
        if self.view.scroll_y + 20. >= max {
            self.view.scroll_y = 0.;
        }
        self.view.scroll(editor, 0., 20.);
    }
    fn changed_state(&mut self, received: Instant, reveal: bool) {
        self.next_utf8_save();
        let reveal = self.panels.should_reveal(reveal);
        self.sync_workspace_after_panels();
        if self.options.mode == Mode::Interactive {
            self.session_due
                .get_or_insert_with(|| Instant::now() + Duration::from_millis(300));
        }
        self.sync_active_view();
        self.observe_caret_input(Instant::now());
        self.input_at.get_or_insert(received);
        if reveal {
            self.view.reveal(self.workspace.editor());
        }
    }
    fn editor_caret_has_focus(&self) -> bool {
        self.workspace.settings_page.is_none()
            && self.tab_rename.is_none()
            && self.panels.input().is_none()
            && !self.panels.side_focused
    }
    // Selection snapshots share storage; moving one caret or just polling an idle
    // window does not allocate or enumerate selections.
    fn observe_caret_input(&mut self, now: Instant) -> bool {
        let tab = &self.workspace.tabs[self.workspace.active];
        let change = tab.editor.buffer.change_id();
        let selection = tab.editor.buffer.view(tab.editor.id).sel();
        if self
            .caret_observed
            .as_ref()
            .is_some_and(|(sheet, previous, sel)| {
                *sheet == tab.sheet
                    && *previous == change
                    && (sel.same_snapshot(selection) || sel == selection)
            })
        {
            return false;
        }
        self.caret_observed = Some((tab.sheet, change, selection.clone()));
        self.view.reset_caret_after_input(now);
        true
    }
    fn update_caret_policy(
        &mut self,
        active: bool,
        interval: Option<Duration>,
        now: Instant,
    ) -> bool {
        self.view.configure_caret(
            interval.filter(|_| active && self.editor_caret_has_focus()),
            self.accessibility_preferences.reduce_motion,
            now,
        )
    }
    fn input(&mut self, w: &WindowHandle, input: TextInput, received: Instant) {
        self.log_input("text", &input);
        self.workspace.editor_mut().text_input(input);
        self.view.reset_caret_after_input(Instant::now());
        self.changed(w, received, true);
    }
    fn handle_text_input(&mut self, window: Option<&WindowHandle>, event: TextInput) {
        if self.options.mode.automatic() {
            self.external_input += 1;
            return;
        }
        self.ensure_startup_services();
        if let Some(rename) = &mut self.tab_rename {
            rename.input.text_input(event);
            self.redraw_tab_rename(window);
            return;
        }
        if self.settings_page_has_focus() {
            let page = self.workspace.settings_page.as_mut().unwrap();
            page.text_input(&self.workspace.translations, event, Instant::now());
            self.settings_redraw(window);
            return;
        }
        if self.region_blocks_text() {
            return;
        }
        let viewport = self.viewport();
        let panel_before = (
            self.workspace.tabs[self.workspace.active].sheet,
            self.workspace.editor().buffer.change_id(),
            self.workspace.editor().regions(),
        );
        if self
            .panels
            .text_input(&mut self.workspace, event.clone(), viewport)
        {
            let moved = panel_before
                != (
                    self.workspace.tabs[self.workspace.active].sheet,
                    self.workspace.editor().buffer.change_id(),
                    self.workspace.editor().regions(),
                );
            self.named_changed(window, moved);
            if let Some(w) = window {
                w.request_frame_at(Instant::now() + Duration::from_millis(8));
            }
        } else if self.accessibility_focus.is_none() && !self.workspace.editor().read_only {
            if matches!(&event, TextInput::Insert { text, .. } | TextInput::Marked { text, .. } if !text.is_empty())
            {
                self.note_editor_typing(window);
            }
            if let Some(w) = window {
                self.input(w, event, Instant::now());
            } else {
                self.workspace.editor_mut().text_input(event);
                self.view.reset_caret_after_input(Instant::now());
                self.changed_state(Instant::now(), true);
            }
        }
    }
    fn tabs_visible(&self) -> bool {
        self.workspace.window_visible("show_tabs")
            && !(self.typing_hides_chrome && self.workspace.settings.bool("auto_hide_tabs", false))
    }
    fn note_editor_typing(&mut self, window: Option<&WindowHandle>) {
        self.typing_hides_chrome = true;
        if let Some(window) = window
            && self.hide_pointer_while_typing()
        {
            window.hide_pointer_until_motion();
        }
    }
    fn hide_pointer_while_typing(&self) -> bool {
        self.workspace
            .settings
            .bool("hide_pointer_while_typing", false)
    }
    fn reveal_typing_chrome(&mut self, window: Option<&WindowHandle>) {
        if std::mem::take(&mut self.typing_hides_chrome)
            && let Some(window) = window
        {
            window.request_frame();
        }
    }
    fn start_report(&mut self, w: &WindowHandle, p: Presentation) -> Result<(), String> {
        self.content_scanout = Some(p.scanout);
        self.observe_highlight(p)?;
        self.clock.mark_at("first_real_scanout", p.scanout);
        self.renderer
            .as_mut()
            .ok_or("renderer missing")?
            .release_prepared();
        // Milestones can be received after a later phase; sort by actual monotonic
        // timestamp before emitting, keeping callback receipt in a separate field.
        self.clock
            .milestones
            .sort_by(|a, b| a.main_ms.total_cmp(&b.main_ms));
        let mut previous = 0.;
        for m in &mut self.clock.milestones {
            m.delta_ms = m.main_ms - previous;
            previous = m.main_ms;
        }
        let renderer = self.renderer.as_ref().ok_or("renderer missing")?;
        self.reporter.emit(json!({"kind":"startup","milestones":self.clock.milestones,"before_main_ms":self.clock.before_main_ms,"process_birth_error":self.clock.birth_error,"process_birth_method":"proc_pidinfo(PROC_PIDTBSDINFO) anchored at main wall snapshot; monotonic deltas thereafter","actual_physical_size":renderer.physical_size(),"logical_size":[w.content_size().width,w.content_size().height],"scale_factor":w.scale_factor(),"maximum_frames_per_second":w.maximum_frames_per_second(),"present_mode":"fifo","scanout_main_ms":ms(p.scanout.saturating_duration_since(self.clock.entered)),"scanout_process_ms":self.clock.before_main_ms.map(|n|n+ms(p.scanout.saturating_duration_since(self.clock.entered))),"presented_time_seconds":p.presented_time,"presented_callback_main_ms":ms(p.callback.saturating_duration_since(self.clock.entered)),"scanout_to_callback_ms":ms(p.callback.saturating_duration_since(p.scanout)),"drawable":p.drawable,"startup_path":"prepare offscreen, order and activate if foreground, commit the layer tree, then acquire/copy/present on the first visible display-link tick","surface_retries":self.retries,"skipped_drawables":self.first_scanout.skipped_drawables,"first_callback_main_ms":self.first_scanout.first_callback.map(|t|ms(t.saturating_duration_since(self.clock.entered))),"first_real_scanout":p.presented_time,"boundary":"MTLDrawable.addPresentedHandler + presentedTime; queue.present return reported separately"}))?;
        self.reporter.emit(json!({"kind":"environment","renderer":renderer.info,"fonts":self.fonts.as_ref().map(Fonts::metadata),"atlas":renderer.atlas_info(),"log_input":self.options.log_input,"platform":"native AppKit objc2; macOS 14+","wgpu":"30.0.2"}))?;
        renderer.wait_idle()?;
        self.reporter.rss("after_first_frame",json!({"lines":self.workspace.editor().text.len_lines(),"bytes":self.workspace.editor().text.len_bytes(),"atlas":renderer.atlas_info()}))?;
        renderer.persist_pipeline_cache()?;
        self.startup_done = true;
        match self.options.mode {
            Mode::LargeFiles => {
                self.bench = Some(Bench::new(Mode::LargeFiles, self.options.seed, vec![]));
                w.set_animating(true);
            }
            Mode::ThemeUi => {
                self.bench = Some(Bench::new(Mode::ThemeUi, self.options.seed, vec![]));
                w.set_animating(true);
            }
            Mode::Startup => self.finish(w)?,
            Mode::Open => {
                if self
                    .workspace
                    .tabs
                    .iter()
                    .all(|t| t.loading.is_none() && t.binary_loading.is_none())
                    && self.highlighted_scanout.is_some()
                {
                    self.finish(w)?;
                } else {
                    w.request_frame_at(Instant::now() + Duration::from_millis(16));
                }
            }
            Mode::Panels => {
                crate::panels::Panels::seed_benchmark(&mut self.workspace);
                self.bench = Some(Bench::new(Mode::Panels, self.options.seed, vec![]));
                w.request_frame_at(Instant::now() + Duration::from_millis(16));
            }
            Mode::Interactive => {
                self.finish_startup_services("post_present_begin")?;
                w.request_frame_at(Instant::now() + Duration::from_millis(200));
            }
            mode => {
                let mut bench = Bench::new(
                    mode,
                    self.options.seed,
                    sweep_positions(self.workspace.editor().text.len_lines(), self.view.rows()),
                );
                if mode == Mode::Typing {
                    let at = bench.schedule_next(Instant::now());
                    w.request_frame_at(at);
                } else {
                    w.set_animating(true);
                }
                self.bench = Some(bench);
            }
        }
        Ok(())
    }
    fn finish(&mut self, w: &WindowHandle) -> Result<(), String> {
        w.set_animating(false);
        if let Err(message) = self
            .renderer
            .as_ref()
            .ok_or("renderer missing")?
            .persist_pipeline_cache()
        {
            self.reporter
                .emit(json!({"kind":"cache_error","message":message}))?;
        }
        self.reporter.emit(json!({"kind":"final_render_state","atlas":self.renderer.as_ref().map(Renderer::atlas_info),"fonts":self.fonts.as_ref().map(Fonts::metadata),"external_input_events":self.external_input}))?;
        self.reporter
            .emit(json!({"kind":"complete","complete":true,"output":self.reporter.path}))?;
        self.done.store(true, Ordering::Release);
        w.quit();
        Ok(())
    }
    fn bench_report(&mut self, w: &WindowHandle) -> Result<(), String> {
        let mode = self.options.mode;
        match mode {
            Mode::LargeFiles => {
                let cpu: Vec<_> = self.samples.iter().map(|s| s.event_to_submit_ms).collect();
                self.reporter.emit(json!({"kind":"large_file_native","actions":self.samples.len(),"event_to_submit_ms":summarize(&cpu),"boundary":"scripted horizontal/vertical scroll, offset navigation or edit through CPU scene and submit","external_input_events":self.external_input}))?;
            }
            Mode::Scroll => {
                if self.samples.len() != SCROLL_FRAMES {
                    return Err(format!(
                        "expected {SCROLL_FRAMES} scroll samples, got {}",
                        self.samples.len()
                    ));
                }
                let minimap_gpu = self
                    .renderer
                    .as_ref()
                    .ok_or("renderer missing")?
                    .minimap_gpu_times()?;
                self.reporter.emit(json!({"kind":"minimap_gpu_summary","render_pass_ms":minimap_gpu.as_deref().and_then(summarize),"p95_budget_ms":1,"boundary":"isolated minimap layer render pass; excludes document, tabs, uploads, resolve/readback, and presentation","available":minimap_gpu.is_some()}))?;
                let gpu = self
                    .renderer
                    .as_ref()
                    .ok_or("renderer missing")?
                    .gpu_times()?;
                for (i, sample) in self.samples.iter().enumerate() {
                    self.reporter.emit(json!({"kind":"scroll_frame","sample":sample,"gpu_render_pass_ms":gpu.as_ref().and_then(|g|g.get(i))}))?;
                }
                let cpu: Vec<_> = self.samples.iter().map(|s| s.event_to_submit_ms).collect();
                let present: Vec<_> = self
                    .samples
                    .iter()
                    .map(|s| s.event_to_present_return_ms)
                    .collect();
                self.reporter.emit(json!({"kind":"scroll_summary","frames":self.samples.len(),"unit":"ms","cpu_event_to_submit":summarize(&cpu),"event_to_present_return":summarize(&present),"gpu_render_pass":gpu.as_deref().and_then(summarize),"gpu_status":if gpu.is_some(){"timestamp_query"}else{"TIMESTAMP_QUERY unavailable"},"gpu_scope":"render pass only: clear + all scene batches; excludes uploads, resolve/readback and presentation","readback":"deferred until all 600 measured frames submitted; 60 display-linked warmup frames excluded","surface_retries":self.retries,"external_input_events":self.external_input}))?;
            }
            Mode::Typing => {
                if self.samples.len() != TYPING_INSERTIONS {
                    return Err("typing benchmark incomplete".into());
                }
                for sample in &self.samples {
                    self.reporter
                        .emit(json!({"kind":"typing_sample","sample":sample}))?;
                }
                let submit: Vec<_> = self.samples.iter().map(|s| s.event_to_submit_ms).collect();
                let present: Vec<_> = self
                    .samples
                    .iter()
                    .map(|s| s.event_to_present_return_ms)
                    .collect();
                let late: Vec<_> = self
                    .samples
                    .iter()
                    .filter_map(|s| s.timer_lateness_ms)
                    .collect();
                self.reporter.emit(json!({"kind":"typing_summary","insertions":self.samples.len(),"unit":"ms","inject_to_submit":summarize(&submit),"inject_to_present_return":summarize(&present),"timer_lateness":summarize(&late),"seed":self.options.seed,"interval_ms_inclusive":[30,90],"carets":self.options.carets,"injection_boundary":"one-shot NSTimer -> keymap lookup -> command -> vel-buffer -> render; no OS/HID injection","presentation_boundary":"queue.present return; not scanout","surface_retries":self.retries,"external_input_events":self.external_input}))?;
            }
            Mode::Memory => {
                self.renderer
                    .as_ref()
                    .ok_or("renderer missing")?
                    .wait_idle()?;
                self.reporter.rss("after_full_sweep",json!({"viewports":self.bench.as_ref().map(|b|b.completed),"atlas":self.renderer.as_ref().map(Renderer::atlas_info),"lines":self.workspace.editor().text.len_lines(),"bytes":self.workspace.editor().text.len_bytes()}))?;
                self.reporter.emit(json!({"kind":"memory_summary","sweep_complete":true,"visited_viewports":self.bench.as_ref().map(|b|b.completed),"external_input_events":self.external_input}))?;
            }
            Mode::ThemeUi => {
                let cpu: Vec<_> = self.samples.iter().map(|s| s.event_to_submit_ms).collect();
                self.reporter.emit(json!({"kind":"theme_ui_native","actions":self.samples.len(),"action_to_submit_ms":summarize(&cpu),"boundary":"theme/minihtml action -> CPU scene -> atlas upload -> GPU submit; scanout reported separately","gui_started":true,"gpu_started":true}))?;
            }
            Mode::Panels => {
                if self.samples.len() != crate::bench::PANEL_ACTIONS {
                    return Err("incomplete panel benchmark".into());
                }
                let cpu: Vec<_> = self.samples.iter().map(|s| s.event_to_submit_ms).collect();
                for sample in &self.samples {
                    self.reporter
                        .emit(json!({"kind":"panel_sample","sample":sample}))?;
                }
                self.reporter.emit(json!({"kind":"panel_summary","actions":self.samples.len(),"candidates":100000,"event_to_submit":summarize(&cpu),"boundary":"scripted controller action -> worker polling -> CPU scene -> queue submit; not HID or scanout","external_input_events":self.external_input}))?;
            }
            _ => {}
        }
        self.reporter
            .rss("after_benchmark", json!({"mode":mode.name()}))?;
        self.finish(w)
    }
    fn record(
        &mut self,
        w: &WindowHandle,
        times: FrameTimes,
        received: Instant,
    ) -> Result<(), String> {
        if self.exact_scene
            && self.highlighted_scanout.is_none()
            && let Some(drawable) = times.drawable
        {
            self.exact_drawables.insert(drawable);
        }
        if !self.first_present {
            self.first_present = true;
            self.clock.mark_at("first_submit", times.submitted);
            self.clock
                .mark_at("first_present_returned", times.presented);
            w.request_frame_at(Instant::now() + Duration::from_millis(2));
            return Ok(());
        }
        if let Some(b) = self.bench.as_mut() {
            if !b.pending {
                return Ok(());
            }
            let at = self.bench_received.take().unwrap_or(received);
            if b.warmup == 0 && self.options.mode != Mode::Memory {
                self.samples.push(Sample {
                    index: b.completed,
                    event_to_submit_ms: ms(times.submitted.saturating_duration_since(at)),
                    event_to_present_return_ms: ms(times.presented.saturating_duration_since(at)),
                    acquire_ms: times.acquire_ms,
                    drawable_limit: times.drawable_limit,
                    encode_submit_ms: times.encode_submit_ms,
                    present_call_ms: times.present_call_ms,
                    timer_lateness_ms: b
                        .due
                        .take()
                        .map(|due| ms(at.saturating_duration_since(due))),
                });
            }
            b.submitted();
            self.input_at = None;
            if b.finished() {
                return self.bench_report(w);
            }
            if b.mode == Mode::Typing {
                let deadline = b.schedule_next(at);
                w.request_frame_at(deadline);
            }
            if matches!(b.mode, Mode::Panels | Mode::ThemeUi | Mode::LargeFiles) {
                w.request_frame_at(Instant::now() + Duration::from_millis(30));
            }
        } else if let Some(at) = self.input_at.take()
            && self.options.log_input
        {
            self.reporter.emit(json!({"kind":"input_latency","receipt_to_submit_ms":ms(times.submitted.saturating_duration_since(at)),"receipt_to_present_return_ms":ms(times.presented.saturating_duration_since(at)),"acquire_ms":times.acquire_ms,"drawable_limit":times.drawable_limit}))?;
        }
        Ok(())
    }
    fn observe_highlight(&mut self, p: Presentation) -> Result<(), String> {
        if self.highlighted_scanout.is_none()
            && p.presented_time > 0.
            && p.presented_time.is_finite()
            && self.exact_drawables.contains(&p.drawable)
            && let Some(content) = self.content_scanout
        {
            self.highlighted_scanout = Some(p.scanout);
            self.exact_drawables.clear();
            self.reporter.emit(json!({"kind":"first_highlighted_screen","after_content_scanout_ms":ms(p.scanout.saturating_duration_since(content)),"budget_ms":150,"drawable":p.drawable,"bytes":self.workspace.editor().text.len_bytes(),"boundary":"positive presentedTime of a version-matched exact styled viewport, minus first content scanout"}))?;
        }
        Ok(())
    }
}

impl AppHandler for App {
    fn activate_on_open(&self) -> bool {
        !self.options.background
    }
    fn on_platform_phase(&mut self, phase: &'static str) {
        self.clock.mark(phase);
    }
    fn on_start(&mut self, w: &WindowHandle) -> Result<(), String> {
        if self.options.mode == Mode::Interactive {
            vel_platform::memory_pressure::enable();
        }
        if self.options.mode == Mode::Interactive
            && !self.options.safe_mode
            && let Ok(executable) = std::env::current_exe()
        {
            let client = vel_packages::https::HttpsClient {
                timeout: Duration::from_secs(8),
                ..Default::default()
            };
            vel_ui::minihtml::enable_remote_images(move |url| {
                crate::remote_image::load(url, &client, &executable, Duration::from_secs(2))
            });
        }
        self.panels.find_pasteboard = Some(Box::new(SystemFindPasteboard));
        self.publish_open_route();
        if let Some([x, y, width, height]) = self.window_frame {
            w.restore_frame(Rect {
                x,
                y,
                width,
                height,
            });
        }
        self.panels.review.native_context = true;
        w.set_title(&self.workspace.window_title());
        self.install_native_menu(w, true);
        self.reporter.emit(json!({"kind":"resources","key_bindings":self.workspace.editor().keymap.len(),"mouse_bindings":self.workspace.mousemap.len(),"palette_entries":(!self.workspace.palette_pending()).then_some(self.workspace.palette.len())}))?;
        let fonts = Fonts::load_named(&self.workspace.editor().settings.editor().font_face)?;
        self.clock.mark("primary_font_ready");
        self.view = View::new(w.content_size(), fonts.cell as f64, fonts.ascent as f64);
        self.view
            .set_system_scroller_overlay(w.prefers_overlay_scrollers());
        self.view.set_font_metrics(fonts.metrics, w.scale_factor());
        self.window_size = w.content_size();
        self.workspace.themes.ui.borrow_mut().density = w.scale_factor() as f32;
        self.view.appearance = w.appearance();
        // The theme's selection and focus fills follow the system accent.
        crate::field_chrome::refresh_system_accent();
        self.workspace
            .set_appearance(w.appearance() == Appearance::Dark)?;
        self.workspace
            .themes
            .ui
            .borrow_mut()
            .metrics
            .enable_native()?;
        self.view.reveal(self.workspace.editor());
        let cache_dir = self
            .workspace
            .data_path()
            .map(|p| p.join("Cache/Renderer"))
            .unwrap_or_else(|| std::env::temp_dir().join("velocity-renderer-cache"));
        #[cfg(target_os = "macos")]
        let renderer = Renderer::new(
            w.clone(),
            self.options.mode == Mode::Scroll,
            &cache_dir,
            |phase| self.clock.mark(phase),
        )?;
        #[cfg(windows)]
        let renderer = {
            let hwnd = w
                .hwnd()
                .ok_or("native HWND missing during renderer startup")?;
            let size = w.content_size();
            let scale = w.scale_factor();
            // SAFETY: The native host owns this handler and releases its
            // renderer on this thread before destroying the HWND.
            unsafe {
                Renderer::from_hwnd(
                    hwnd.as_ptr(),
                    (size.width * scale).round() as u32,
                    (size.height * scale).round() as u32,
                    false,
                    &cache_dir,
                    |phase| self.clock.mark(phase),
                    |adapter| {
                        self.reporter.emit(json!({
                            "kind": "dx12_adapter",
                            "name": adapter.name,
                            "backend": format!("{:?}", adapter.backend),
                            "device_type": format!("{:?}", adapter.device_type),
                            "driver": adapter.driver,
                            "driver_info": adapter.driver_info,
                        }))
                    },
                )?
            }
        };
        self.renderer = Some(renderer);
        for record in std::mem::take(&mut self.workspace.telemetry) {
            self.reporter.emit(record)?;
        }
        self.font_face = self.workspace.editor().preferences.font_face.clone();
        self.fonts = Some(fonts);
        self.initialize_view_metrics();
        if self.options.mode == Mode::Interactive
            && (self.workspace.fullscreen || self.workspace.distraction_free)
        {
            w.toggle_full_screen_with_mode(
                self.workspace
                    .editor()
                    .settings
                    .bool("use_simple_full_screen", false),
            );
        }
        Ok(())
    }
    fn accessibility_tree(&self) -> accessibility::Tree {
        let mut tree = self.accessible_tree();
        tree.label = self
            .workspace
            .translations
            .tr("Velocity workspace")
            .into_owned();
        tree
    }
    fn on_accessibility(
        &mut self,
        w: &WindowHandle,
        id: accessibility::Id,
        request: accessibility::Request,
    ) {
        self.ensure_startup_services();
        self.accessibility_action(w, id, request);
    }
    fn accessibility_range_rect(&self, id: accessibility::Id, range: Utf16Range) -> Rect {
        self.accessible_range_rect(id, range)
    }
    fn on_accessibility_preferences(
        &mut self,
        w: &WindowHandle,
        preferences: accessibility::Preferences,
    ) {
        self.accessibility_preferences(preferences);
        if preferences.reduce_motion {
            self.view
                .prepare_keyboard_scroll(self.workspace.editor(), Instant::now());
        }
        w.request_frame();
    }
    fn on_scroller_style(&mut self, w: &WindowHandle, overlay: bool) {
        self.view.set_system_scroller_overlay(overlay);
        for view in self.sheet_views.values_mut() {
            view.set_system_scroller_overlay(overlay);
        }
        w.request_frame();
    }
    fn on_key(&mut self, w: &WindowHandle, event: KeyEvent) -> bool {
        if event.state == KeyState::Down {
            self.cancel_context(w);
        }
        self.log_input("key", &event);
        if self.options.mode.automatic() {
            if event.state == KeyState::Down {
                self.external_input += 1;
            }
            return true;
        }
        let Some(key) = crate::editor::binding_key(&event) else {
            return false;
        };
        if self.tab_rename.is_some() {
            return self.tab_rename_key(Some(w), &key);
        }
        let input = if self.settings_page_has_focus() {
            self.workspace
                .settings_page
                .as_ref()
                .and_then(|page| page.input())
        } else {
            self.panels.input()
        };
        if input.is_some_and(vel_ui::input::Input::composing) {
            return false;
        }
        let viewport = self.viewport();
        if self.workspace.editor().vintage.is_some() {
            let first = (self.view.scroll_y / self.view.line_height).max(0.) as usize;
            let (top, rows) = {
                let layout = self.workspace.editor().layout.borrow();
                let top = layout.locate(first).0;
                let last = layout.locate(first + self.view.rows().saturating_sub(1)).0;
                (top, last.saturating_sub(top) + 1)
            };
            self.workspace
                .editor_mut()
                .vintage
                .as_mut()
                .expect("enabled")
                .engine
                .set_viewport(top, rows);
            self.workspace.editor_mut().clipboard = w.clipboard_text().unwrap_or_default();
        }
        self.dispatch_key_binding(Some(w), &key, event.received, viewport)
    }
    fn accepts_first_mouse(&self) -> bool {
        self.workspace.settings.bool("click_through", false)
    }
    fn inline_text_input(&self) -> bool {
        self.tab_rename.is_some()
    }
    fn on_inline_text_focus_lost(&mut self, w: &WindowHandle) {
        self.finish_tab_rename(Some(w));
    }
    fn on_named_command(&mut self, w: &WindowHandle, name: &str, args: &str) {
        if !self.options.mode.automatic() {
            if w.is_closed() {
                // ST4 4131, https://www.sublimetext.com/download: File menu
                // recents remain available when the last macOS window closes.
                let parsed = serde_json::from_str::<Value>(args).unwrap_or_default();
                let mut request = match crate::instance::Request::new(&Options::default(), None) {
                    Ok(request) => request,
                    Err(e) => {
                        self.named_error(Some(w), &e);
                        return;
                    }
                };
                let paths: Vec<PathBuf> = match name {
                    "new_window"
                    | "show_settings"
                    | "edit_settings"
                    | "open_url"
                    | "open_crash_reports"
                    | "prompt_open_folder_in_new_window" => {
                        self.run_named(Some(w), name, &parsed);
                        return;
                    }
                    "clear_recent_items" => {
                        self.run_named(Some(w), name, &parsed);
                        return;
                    }
                    "prompt_open_file" => w.open_files(),
                    "open_recent_file" => parsed["file"]
                        .as_str()
                        .map(PathBuf::from)
                        .into_iter()
                        .collect(),
                    "open_recent_folder" => parsed["dir"]
                        .as_str()
                        .map(PathBuf::from)
                        .into_iter()
                        .collect(),
                    "open_project" | "open_project_or_workspace" => {
                        request.project = parsed["file"]
                            .as_str()
                            .map(Path::new)
                            .map(vel_session::project::project_path);
                        Vec::new()
                    }
                    _ => return,
                };
                request.files = paths
                    .into_iter()
                    .map(crate::cli::FileLocation::path_only)
                    .collect();
                if (request.project.is_some() || !request.files.is_empty())
                    && let Err(e) = self.open_native_window_request(w, request)
                {
                    self.named_error(Some(w), &e);
                }
                return;
            }
            match serde_json::from_str(args) {
                Ok(args) => self.run_named(Some(w), name, &args),
                Err(e) => self.named_error(Some(w), &e.to_string()),
            }
        }
    }
    fn on_open_paths(&mut self, w: &WindowHandle, paths: &[std::path::PathBuf]) {
        if self.options.mode != Mode::Interactive {
            return;
        }
        let request = match crate::open_requests::native_request(paths) {
            Ok(request) => request,
            Err(error) => {
                self.named_error(Some(w), &error);
                return;
            }
        };
        self.publish_open_route();
        if let Err(error) = self.receive_request(w, crate::instance::Incoming::local(request)) {
            self.named_error(Some(w), &error);
        }
    }
    #[cfg(target_os = "macos")]
    fn on_finder_new_text_file(&mut self, window: &WindowHandle, directory: &Path) {
        self.finder_new_text_file(window, directory);
    }
    fn on_reopen(&mut self, w: &WindowHandle) {
        if self.options.mode == Mode::Interactive
            && let Err(error) = self.open_native_window(w, None, None, false)
        {
            self.named_error(Some(w), &error);
        }
    }
    fn on_idle(&mut self, w: &WindowHandle) {
        if self.options.mode != Mode::Interactive {
            return;
        }
        #[cfg(windows)]
        if !w.is_closed() {
            // A minimized HWND cannot present, but must still service the pipe
            // and any outstanding CLI wait replies.
            self.ensure_startup_services();
            if self.settings_window {
                self.poll_settings_window(w);
            } else if let Err(error) = self.poll_workbench(w) {
                self.named_error(Some(w), &error);
            }
            self.builds.poll(&mut self.panels);
            w.set_taskbar_build_active(self.builds.task.is_some());
            return;
        }
        let requests = self
            .sessions
            .borrow()
            .instance
            .as_ref()
            .map(|i| i.drain())
            .unwrap_or_default();
        for mut incoming in requests {
            // There is no live key window; route this request into a fresh one.
            incoming.request.new_window = true;
            if let Err(error) = self.receive_request(w, incoming) {
                self.named_error(Some(w), &error);
            }
        }
    }
    fn command_enabled(&self, name: &str, args: &str) -> bool {
        if let Some(enabled) = self.settings_window_command_enabled(name) {
            return enabled;
        }
        let Ok(args) = serde_json::from_str::<Value>(args) else {
            return false;
        };
        if name == "toggle_find_option"
            && args["option"] == "in_selection"
            && !self.panels.can_find_in_selection(&self.workspace)
        {
            return false;
        }
        self.command_enabled_args(name, &args)
    }
    fn command_visible(&self, name: &str, args: &str) -> bool {
        let Ok(args) = serde_json::from_str::<Value>(args) else {
            return false;
        };
        self.context_state(name, &args)
            .is_none_or(|state| state.visible)
            && (name != "package_check_upgrades"
                || self
                    .workspace
                    .settings
                    .get("package_index_url")
                    .and_then(Value::as_str)
                    .is_some_and(|url| !url.is_empty()))
    }
    fn command_checked(&self, name: &str, args: &str) -> bool {
        let Ok(args) = serde_json::from_str::<Value>(args) else {
            return false;
        };
        if let Some(state) = self.context_state(name, &args) {
            return state.checked;
        }
        let target = args["_context_sheet"]
            .as_u64()
            .and_then(|sheet| self.workspace.sheet_index(sheet))
            .unwrap_or(self.workspace.active);
        let editor = &self.workspace.tabs[target].editor;
        match name {
            "set_file_type" => {
                editor.language.borrow().as_ref().is_some_and(|language| {
                    args["syntax"].as_str() == Some(language.syntax.as_str())
                })
            }
            "set_build_system" => {
                self.workspace
                    .build_system
                    .as_deref()
                    .unwrap_or("Automatic")
                    == args["name"].as_str().unwrap_or("Automatic")
            }
            "set_max_columns" => {
                args["columns"].as_u64() == Some(self.workspace.workbench.max_columns as u64)
            }
            "reopen" | "reopen_encoding" | "save" | "save_encoding"
                if args["encoding"].as_str().is_some() =>
            {
                vel_fileio::encoding_format(
                    args["encoding"].as_str().unwrap(),
                    self.workspace.tabs[target].format.line_ending,
                )
                .is_ok_and(|format| {
                    format.display_encoding()
                        == self.workspace.tabs[target].format.display_encoding()
                })
            }
            "set_line_ending" | "set_line_endings" => args["type"]
                .as_str()
                .and_then(|value| vel_fileio::LineEnding::parse(value).ok())
                .is_some_and(|ending| ending == self.workspace.tabs[target].format.line_ending),
            "toggle_setting" if args["setting"] == "word_wrap" => {
                crate::view::motion_layout_options(editor).width != 0
            }
            "toggle_setting" => args["setting"]
                .as_str()
                .is_some_and(|key| editor.settings.bool(key, false)),
            // Submenu checkmarks reflect the current state, not the action's label.
            // https://developer.apple.com/design/human-interface-guidelines/menus
            "set_layout" => crate::workbench::menu_layout(&args)
                .is_ok_and(|layout| layout == self.workspace.workbench.layout),
            "toggle_bookmark" => editor.bookmarks().iter().any(|mark| {
                editor.text.byte_to_line(mark.begin()) == editor.text.byte_to_line(editor.caret)
            }),
            "expand_tabs" => editor.preferences.translate_tabs_to_spaces,
            "unexpand_tabs" => !editor.preferences.translate_tabs_to_spaces,
            "show_console" => self.panels.console_visible(),
            "set_ui_language" => {
                args["language"].as_str() == Some(self.workspace.checked_ui_language())
            }
            "toggle_find_option" => args["option"]
                .as_str()
                .is_some_and(|option| self.panels.find_option_checked(option)),
            "toggle_show_open_files" => self.panels.open_files_visible,
            "toggle_minimap" => self.workspace.window_visible("show_minimap"),
            "toggle_tabs" => self.workspace.window_visible("show_tabs"),
            "toggle_status_bar" => self.workspace.window_visible("show_status_bar"),
            "toggle_inline_diff" => {
                let byte = args["_context_byte"]
                    .as_u64()
                    .map_or(editor.caret, |byte| byte as usize);
                let row = editor.text.byte_to_line(byte.min(editor.text.len_bytes()));
                let annotations = editor.annotations.borrow();
                annotations
                    .current
                    .as_ref()
                    .and_then(|snapshot| {
                        snapshot.hunks.iter().find(|hunk| {
                            hunk.new.start <= row && row < hunk.new.end.max(hunk.new.start + 1)
                        })
                    })
                    .is_some_and(|hunk| annotations.inline.contains(&hunk.new.start))
            }
            _ => false,
        }
    }
    fn menu_caption(&self, id: &str) -> Option<&str> {
        (id == "side_bar_visibility").then(|| {
            self.workspace
                .translations
                .caption(if self.panels.side_visible {
                    "Hide Side Bar"
                } else {
                    "Show Side Bar"
                })
        })
    }
    fn on_text_input(&mut self, w: &WindowHandle, event: TextInput) {
        self.cancel_context(w);
        self.handle_text_input(Some(w), event);
    }
    fn on_command(&mut self, w: &WindowHandle, command: Command) {
        if command == Command::Cancel && self.cancel_tab_drag(Some(w)) {
            return;
        }
        self.cancel_context(w);
        self.log_input("command", command);
        if self.options.mode.automatic() {
            self.external_input += 1;
            return;
        }
        self.ensure_startup_services();
        if self.tab_rename.is_some() {
            self.tab_rename_command(Some(w), command);
            return;
        }
        if self.settings_page_has_focus() {
            self.settings_command(Some(w), command, Instant::now());
            return;
        }
        if self.region_blocks_text() {
            if command == Command::Cancel {
                self.leave_region_focus();
                w.request_frame();
            }
            return;
        }
        if let Some(input) = self.panels.input_mut() {
            input.set_clipboard(w.clipboard_text().unwrap_or_default());
        }
        let viewport = self.viewport();
        let panel_command = match command {
            Command::ParagraphStart(extend) => Command::Home(extend),
            Command::ParagraphEnd(extend) => Command::End(extend),
            _ => command,
        };
        match self
            .panels
            .command(&mut self.workspace, panel_command, viewport)
        {
            Ok((true, invocation)) => {
                if let Some(input) = self.panels.input() {
                    w.set_clipboard_text(input.clipboard());
                }
                if let Some(invocation) = invocation {
                    self.run_named(Some(w), &invocation.command, &invocation.args);
                }
                self.changed(w, Instant::now(), true);
                return;
            }
            Ok((false, _)) => {}
            Err(error) => {
                self.named_error(Some(w), &error);
                return;
            }
        }
        if command == Command::Cancel && self.workspace.editor().marked.is_some() {
            self.workspace.editor_mut().cancel_composition();
            self.changed(w, Instant::now(), true);
            return;
        }
        self.run_editor_selector(Some(w), command);
    }
    fn on_mouse(&mut self, w: &WindowHandle, event: MouseEvent) {
        self.handle_mouse_event(Some(w), event);
    }
    // Native and headless events share UI hit testing, focus, binding lookup and gestures.
    fn on_scroll(&mut self, w: &WindowHandle, event: ScrollEvent) {
        self.handle_scroll_event(Some(w), event);
    }
    fn on_resize(&mut self, w: &WindowHandle, size: Size, scale: f64) {
        if let Some(frame) = w.frame() {
            self.window_frame = Some([frame.x, frame.y, frame.width, frame.height]);
        }
        if self.options.mode == Mode::Interactive {
            self.workspace.fullscreen = w.is_full_screen();
        }
        self.resize_views(size, scale);
        if let Some(r) = self.renderer.as_mut() {
            r.resize(
                (size.width * scale).round() as u32,
                (size.height * scale).round() as u32,
            );
        }
        w.request_frame();
    }
    fn on_appearance(&mut self, w: &WindowHandle, appearance: Appearance) {
        crate::field_chrome::refresh_system_accent();
        self.view.appearance = appearance;
        for view in self.sheet_views.values_mut() {
            view.appearance = appearance;
        }
        if let Err(e) = self
            .workspace
            .set_appearance(appearance == Appearance::Dark)
        {
            self.named_error(Some(w), &e);
        }
        w.request_frame();
    }
    fn on_frame(&mut self, ctx: &mut FrameContext<'_>) -> Result<(), String> {
        let minimum = if self.settings_window {
            settings_window::SETTINGS_MINIMUM
        } else {
            self.pane_window_minimum()
        };
        if self.native_pane_minimum != Some((minimum.width, minimum.height)) {
            ctx.window.set_minimum_content_size(minimum);
            self.native_pane_minimum = Some((minimum.width, minimum.height));
        }
        self.workspace.themes.ui.borrow_mut().density = ctx.window.scale_factor() as f32;
        self.workspace
            .themes
            .ui
            .borrow_mut()
            .set_accessibility_preferences(self.accessibility_preferences);
        if self.done.load(Ordering::Acquire) {
            return Ok(());
        }
        let w = ctx.window;
        if let Some(fonts) = self.fonts.as_mut() {
            if fonts.poll_fallbacks() {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.invalidate_layouts();
                }
                w.request_frame();
            }
            if fonts.fallback_pending() {
                w.request_frame();
            }
        }
        if !w.is_focused() && (self.drag_select.is_some() || self.drag_text.is_some()) {
            // No button release arrives after focus moves away mid-gesture.
            self.drag_select = None;
            self.drag_text = None;
            self.view.drop_caret = None;
        }
        if let Some(drag) = self.drag_select.as_mut()
            && drag.autoscroll(&mut self.view, self.workspace.editor_mut(), Instant::now())
        {
            self.changed(w, Instant::now(), false);
        }
        w.set_title(&self.workspace.window_title());
        let observations = w.take_presentations();
        if self.startup_done {
            for p in &observations {
                self.observe_highlight(*p)?;
            }
        }
        let focused = w.is_focused();
        let focus_changed = self.window_focused != focused;
        if focus_changed && !focused {
            self.cancel_tab_drag(Some(w));
            self.save_on_focus_lost(None);
        }
        // Take the menu bar as soon as this window is key, also during its startup frames
        // (before the first scanout the focus branch below does not run).
        if !focused {
            self.menu_owner = false;
        } else if !self.menu_owner {
            self.install_native_menu(w, true);
        }
        self.set_window_focused(focused);
        if focused && self.options.mode == Mode::Interactive && !self.settings_window {
            self.sessions.borrow_mut().active_window = Some(self.session_id);
        }
        if self.startup_done
            && matches!(
                self.options.mode,
                Mode::Interactive | Mode::Open | Mode::LargeFiles
            )
        {
            let loading_before = self
                .workspace
                .tabs
                .iter()
                .any(|t| t.loading.is_some() || t.binary_loading.is_some());
            let active_was_loading = self.workspace.tabs[self.workspace.active].loading.is_some();
            let session_changed = self.poll_workspace_model() || focus_changed;
            let mut changed = session_changed;
            if self.workspace.settings_page.is_some() {
                changed |= self.poll_settings_input(Instant::now());
            }
            self.publish_resource_reports();
            self.workspace
                .themes
                .ui
                .borrow_mut()
                .set_accessibility_preferences(self.accessibility_preferences);
            changed |= self.releases.poll(&mut self.workspace, &mut self.panels, w);
            self.workspace.themes.ui.borrow_mut().density = w.scale_factor() as f32;
            changed |= self.workspace.themes.ui.borrow().metrics.poll_images();
            changed |= self.builds.poll(&mut self.panels);
            #[cfg(windows)]
            w.set_taskbar_build_active(self.builds.task.is_some());
            if self.print_jobs.active() {
                changed |= self.poll_print(w, Instant::now());
            }
            if session_changed
                && self.options.mode == Mode::Interactive
                && self.session_due.is_none()
            {
                self.session_due = Some(Instant::now() + Duration::from_millis(300));
            }
            if self.options.mode == Mode::Interactive {
                if self.settings_window {
                    self.poll_settings_window(w);
                } else {
                    changed |= self.receive_preference_updates();
                    self.poll_workbench(w)?;
                }
            }
            if focus_changed {
                self.install_native_menu(w, true);
            }
            let viewport = self.viewport();
            changed |= self.panels.poll(&mut self.workspace, viewport);
            if let Some(deadline) = self.panels.message_deadline() {
                w.request_frame_at(deadline);
            }
            self.sync_active_view();
            changed |= self
                .view
                .advance_scroll(self.workspace.editor(), Instant::now());
            if std::mem::take(&mut self.panels.reveal) {
                self.view.reveal(self.workspace.editor());
            }
            self.flush_context_menu(w);
            if std::mem::take(&mut self.panels.review.reveal) {
                self.view.reveal(self.workspace.editor());
            }
            if let Some(invocation) = self.panels.pending_command.take() {
                self.run_named(Some(w), &invocation.command, &invocation.args);
            }
            self.replacement_confirmation(w);
            let busy = self.releases.busy()
                || self.workspace.resources_busy()
                || self.panels.busy()
                || self.view.image_preview.busy()
                || !self.inactive_image_decodes.is_empty()
                || self.workspace.editor().annotations.borrow().pending > 0
                || self.workspace.editor().completion.busy()
                || self.builds.task.is_some()
                || self.workspace.themes.ui.borrow().metrics.images_pending()
                || self.workspace.language_busy();
            if active_was_loading && self.workspace.tabs[self.workspace.active].loading.is_none() {
                self.view.reveal(self.workspace.editor());
            }
            if changed {
                self.install_native_menu(w, false);
            }
            for record in std::mem::take(&mut self.workspace.telemetry) {
                self.reporter.emit(record)?;
            }
            if self.options.mode == Mode::Open
                && self
                    .workspace
                    .tabs
                    .iter()
                    .all(|t| t.loading.is_none() && t.binary_loading.is_none())
                && self.highlighted_scanout.is_some()
            {
                if !self.workspace.messages.is_empty() {
                    return Err(self.workspace.messages.join("; "));
                }
                return self.finish(w);
            }
            let conflicts: Vec<_> = self
                .workspace
                .tabs
                .iter()
                .enumerate()
                .filter(|(_, t)| t.external.is_some())
                .map(|(i, _)| i)
                .collect();
            for index in conflicts {
                let reload = Dialog::new(
                    UiText::tr("File changed on disk"),
                    UiText::tr_fmt(
                        "{name} has unsaved edits. Reloading discards them.",
                        [(
                            "name",
                            self.workspace.tabs[index].ui_name(&self.workspace.ui),
                        )],
                    ),
                    [UiText::tr("Keep Edits"), UiText::tr("Reload")],
                )
                .show(&self.workspace.translations, w)
                    == 1;
                self.workspace.reload_external(index, reload);
            }
            for message in std::mem::take(&mut self.workspace.messages) {
                self.status_error(UiText::command_error(&message));
                changed = true;
            }
            let now = Instant::now();
            changed |= self.observe_caret_input(now);
            let caret_active = w.caret_is_active();
            let interval = caret_active.then(|| w.caret_blink_interval()).flatten();
            changed |= self.update_caret_policy(caret_active, interval, now);
            let caret_deadline = self.view.next_caret_frame(self.workspace.editor(), now);
            let animating = self.view.scrolling()
                || self.workspace.themes.ui.borrow().animating()
                || self.tab_slides.is_some();
            let poll_at = now
                + Duration::from_millis(if busy {
                    8
                } else if animating {
                    16
                } else {
                    200
                });
            w.request_frame_at(caret_deadline.map_or(poll_at, |due| due.min(poll_at)));
            let caret_due = match (self.caret_deadline, caret_deadline) {
                (None, None) => false,
                (Some(due), Some(_)) => due <= now,
                _ => true,
            };
            if ctx.reason == FrameReason::Timer
                && !changed
                && !loading_before
                && !caret_due
                && !animating
            {
                return Ok(());
            }
            self.caret_deadline = caret_deadline;
        }
        if self.font_face != self.workspace.editor().preferences.font_face {
            let name = self.workspace.editor().preferences.font_face.clone();
            match Fonts::load_named(&name) {
                Ok(fonts) => {
                    self.reload_font_views(fonts, w.scale_factor());
                    self.renderer
                        .as_mut()
                        .ok_or("renderer missing")?
                        .reset_fonts();
                }
                Err(error) => self.named_error(Some(w), &error),
            }
            self.font_face = name;
        }
        if !self.startup_done {
            let before_callback = self.first_scanout.first_callback;
            let skipped = self.first_scanout.skipped_drawables;
            let real = self.first_scanout.observe(&observations);
            if before_callback.is_none()
                && let Some(callback) = self.first_scanout.first_callback
            {
                self.clock.mark_at("first_presented_callback", callback);
            }
            if skipped != self.first_scanout.skipped_drawables {
                self.reporter.emit(json!({"kind":"drawable_skipped","skipped_drawables":self.first_scanout.skipped_drawables,"first_callback_main_ms":self.first_scanout.first_callback.map(|t|ms(t.saturating_duration_since(self.clock.entered)))}))?;
            }
            if let Some(p) = real {
                self.start_report(w, p)?;
                return Ok(());
            }
        }
        if self.startup_done
            && !self.language_warmed
            && matches!(self.options.mode, Mode::Scroll | Mode::Typing)
        {
            self.workspace.poll();
            self.publish_resource_reports();
            let ready = self
                .workspace
                .editor()
                .language
                .borrow()
                .as_ref()
                .is_some_and(|l| l.ready() && l.highlight_complete());
            if !ready {
                w.request_frame_at(Instant::now() + Duration::from_millis(8));
                return Ok(());
            }
            self.language_warmed = true;
            self.reporter.emit(json!({"kind":"language_ready","syntax":self.workspace.editor().language.borrow().as_ref().map(|l| l.name.clone()),"minimap":self.workspace.editor().settings.bool("show_minimap",true),"bytes":self.workspace.editor().text.len_bytes()}))?;
            if let Some(b) = self.bench.as_mut()
                && b.mode == Mode::Typing
            {
                b.schedule_next(Instant::now());
            }
        }
        if ctx.reason == FrameReason::StartupPrepare {
            let scene = self.render_scene();
            self.clock.mark("first_scene_ready");
            self.renderer
                .as_mut()
                .ok_or("renderer missing")?
                .pre_render(
                    &scene,
                    self.fonts.as_mut().ok_or("fonts missing")?,
                    w.scale_factor() as f32,
                )?;
            self.clock.mark("first_offscreen_submitted");
            return Ok(());
        }
        if !self.startup_done {
            let waiting_since = *self.waiting_since.get_or_insert_with(Instant::now);
            if stop_waiting_for_scanout(self.options.mode, waiting_since.elapsed()) {
                return self.start_without_scanout(w, waiting_since.elapsed());
            }
            // Main already checked the display. The first display-link tick
            // is view-bound; don't query WindowServer again before the acquire.
            if self.options.mode.automatic() && waiting_since.elapsed() >= Duration::from_secs(1) {
                w.display_status().check()?;
            }
            // Ordering can constrain an oversized window or change its scale.
            // Rebuild on this tick if a resize invalidated the offscreen texture.
            if !self
                .renderer
                .as_ref()
                .ok_or("renderer missing")?
                .has_prepared_frame()
            {
                let scene = self.render_scene();
                self.renderer
                    .as_mut()
                    .ok_or("renderer missing")?
                    .pre_render(
                        &scene,
                        self.fonts.as_mut().ok_or("fonts missing")?,
                        w.scale_factor() as f32,
                    )?;
                self.clock
                    .mark("first_frame_reprepared_after_native_resize");
            }
            if let Some(times) = self
                .renderer
                .as_mut()
                .ok_or("renderer missing")?
                .present_prepared()?
            {
                self.record(w, times, ctx.received)?;
                #[cfg(windows)]
                {
                    // The renderer can report present return, not scanout.
                    // Start IPC, load services and editing without waiting for
                    // a Metal-only presented callback.
                    self.start_without_scanout(w, waiting_since.elapsed())?;
                }
                w.request_frame_at(Instant::now() + Duration::from_millis(16));
            } else {
                self.retries += 1;
                w.request_frame_at(Instant::now() + Duration::from_millis(16));
            }
            return Ok(());
        }
        let mut query = None;
        if self.options.mode == Mode::ThemeUi {
            self.workspace.poll();
            self.publish_resource_reports();
        }
        if self.options.mode == Mode::ThemeUi && self.bench.as_ref().is_some_and(|b| !b.pending) {
            let step = self.bench.as_ref().unwrap().completed;
            self.bench.as_mut().unwrap().pending = true;
            self.bench_received = Some(Instant::now());
            crate::theme_bench::native_step(&mut self.workspace, step)?;
            let viewport = self.viewport();
            match step % 10 {
                0 => {
                    self.panels
                        .action(&mut self.workspace, "hide_panel", &json!({}), viewport)?;
                }
                6 => {
                    self.panels.action(
                        &mut self.workspace,
                        "show_panel",
                        &json!({"panel":"find"}),
                        viewport,
                    )?;
                }
                7..=9 => {
                    if let Some(node) = self.panels.tree.nodes.get(&50) {
                        let point = Point {
                            x: node.rect.x + node.rect.width / 2.,
                            y: node.rect.y + node.rect.height / 2.,
                        };
                        self.panels.tree.pointer(
                            point,
                            match step % 10 {
                                8 => Some(true),
                                9 => Some(false),
                                _ => None,
                            },
                        );
                    }
                }
                _ => {}
            }
        }

        if self.options.mode == Mode::LargeFiles {
            self.workspace.poll();
            self.publish_resource_reports();
            if self
                .workspace
                .tabs
                .iter()
                .any(|t| t.loading.is_some() || t.binary_loading.is_some())
            {
                w.request_frame_at(Instant::now() + Duration::from_millis(8));
                let scene = self.render_scene();
                self.renderer.as_mut().ok_or("renderer missing")?.render(
                    &scene,
                    self.fonts.as_mut().ok_or("fonts missing")?,
                    w.scale_factor() as f32,
                    None,
                )?;
                return Ok(());
            }
            if self.bench.as_ref().is_some_and(|b| !b.pending) {
                let step = self.bench.as_ref().unwrap().completed;
                self.bench.as_mut().unwrap().pending = true;
                self.bench_received = Some(Instant::now());
                crate::large_bench::native_step(&mut self.workspace, &mut self.view, step)?;
            }
        }
        if self.options.mode == Mode::Panels {
            if self.bench.as_ref().is_some_and(|b| !b.pending) {
                let index = self.bench.as_ref().expect("panel bench").completed;
                self.bench.as_mut().expect("panel bench").pending = true;
                self.bench_received = Some(Instant::now());
                let viewport = self.viewport();
                self.panels
                    .benchmark_step(&mut self.workspace, index, viewport)?;
            }
            let viewport = self.viewport();
            self.panels.poll(&mut self.workspace, viewport);
            if self.panels.ranking_pending() {
                w.request_frame_at(Instant::now() + Duration::from_millis(2));
                return Ok(());
            }
        }
        if let Some(b) = self.bench.as_mut() {
            if b.mode == Mode::Typing {
                if !b.pending && !b.typing_due(ctx.received) {
                    if let Some(at) = b.next {
                        w.request_frame_at(at);
                    }
                    return Ok(());
                }
                if self.bench_received.is_none() {
                    let text = crate::bench::typing_text(b.completed);
                    self.bench_received = Some(Instant::now());
                    crate::type_through_keymap(self.workspace.editor_mut(), text)?;
                    self.changed(w, self.bench_received.unwrap(), true);
                }
            } else if !b.pending {
                b.pending = true;
                self.bench_received = Some(ctx.received);
                if b.mode == Mode::Memory {
                    self.view.scroll_y = b.positions[b.completed] as f64 * self.view.line_height;
                    self.view.clamp_scroll(self.workspace.editor());
                } else {
                    self.benchmark_scroll_step();
                }
            }
            if let Some(b) = self.bench.as_ref()
                && b.mode == Mode::Scroll
                && b.warmup == 0
            {
                query = Some(b.completed as u32);
            }
        }
        let scene = self.render_scene();
        // Layout can enqueue its first image request after the worker poll above.
        if self.workspace.themes.ui.borrow().metrics.images_pending() {
            w.request_frame_at(Instant::now() + Duration::from_millis(8));
        }
        self.apply_title_theme(w);
        let result = self.renderer.as_mut().ok_or("renderer missing")?.render(
            &scene,
            self.fonts.as_mut().ok_or("fonts missing")?,
            w.scale_factor() as f32,
            query,
        )?;
        if let Some(times) = result {
            self.record(w, times, ctx.received)?;
        } else {
            self.retries += 1;
            w.request_frame_at(Instant::now() + Duration::from_millis(16));
        }
        Ok(())
    }
    fn on_quit_requested(&mut self, w: &WindowHandle) -> bool {
        if self.settings_window {
            self.close_settings_window();
            return true;
        }
        if !self.finish_tab_rename(Some(w)) {
            return false;
        }
        self.finish_settings_input();
        self.receive_tabs();
        self.workspace.fullscreen = w.is_full_screen();
        self.prepare_quit(Some(w))
    }
    fn on_session_ending(&mut self, _window: &WindowHandle) {
        if self.settings_window {
            return;
        }
        // ST dev build 4075 checkpoints on system shutdown (sublimetext.com/dev).
        // Keep the ordinary quit path responsible for prompts and the final flush.
        if self.options.mode == Mode::Interactive
            && self.hot_exit().retain(true)
            && let Err(error) = self.save_workspace_checkpoint()
        {
            self.panels.set_message(UiText::tr_fmt(
                "Could not checkpoint during logout: {detail}",
                [("detail", error)],
            ));
        }
    }
    fn on_window_close_requested(&mut self, w: &WindowHandle) -> bool {
        if self.settings_window {
            self.close_settings_window();
            return true;
        }
        if !self.finish_tab_rename(Some(w)) {
            return false;
        }
        self.finish_settings_input();
        self.receive_tabs();
        self.workspace.fullscreen = w.is_full_screen();
        if self.options.mode != Mode::Interactive {
            return self.on_quit_requested(w);
        }
        self.prepare_window_close(Some(w))
    }
    fn text_input_state(&self) -> TextInputState {
        if let Some(rename) = &self.tab_rename {
            return rename.input.state();
        }
        if self.settings_page_has_focus() {
            return self
                .workspace
                .settings_page
                .as_ref()
                .and_then(|p| p.input())
                .map_or_else(TextInputState::default, |input| input.state());
        }
        if let Some(input) = self.panels.input() {
            return input.state();
        }
        let mut caret = self
            .view
            .rect_at(self.workspace.editor(), self.workspace.editor().caret);
        caret.x += self.content.x;
        caret.y += self.content.y;
        TextInputState {
            text: self.workspace.editor().text.clone(),
            selected: self.workspace.editor().selected_utf16(),
            marked: self.workspace.editor().marked_utf16(),
            caret,
        }
    }
    fn text_input_rect(&self, range: Utf16Range) -> (Rect, Utf16Range) {
        if let Some(rename) = &self.tab_rename {
            return rename.range_rect(range);
        }
        if self.settings_page_has_focus() {
            return self
                .workspace
                .settings_page
                .as_ref()
                .and_then(|p| p.input())
                .map_or_else(|| (Rect::default(), range), |input| input.range_rect(range));
        }
        if let Some(input) = self.panels.input() {
            return input.range_rect(range);
        }
        let (mut rect, range) = self.view.range_rect(self.workspace.editor(), range);
        rect.x += self.content.x;
        rect.y += self.content.y;
        (rect, range)
    }
    fn character_index_for_point(&self, point: Point) -> Option<usize> {
        if let Some(rename) = &self.tab_rename {
            return Some(rename.input.utf16_at_point(point));
        }
        if self.settings_page_has_focus() {
            return self
                .workspace
                .settings_page
                .as_ref()?
                .input()
                .map(|input| input.utf16_at_point(point));
        }
        if self.panels.input().is_none() && self.workspace.editor().binary.is_some() {
            return None;
        }
        if let Some(input) = self.panels.input() {
            return Some(input.utf16_at_point(point));
        }
        #[cfg(windows)]
        if !vel_ui::contains(self.content, point) {
            return None;
        }
        Some(
            self.workspace
                .editor()
                .text
                .byte_to_utf16(self.view.hit_test(
                    self.workspace.editor(),
                    Point {
                        x: point.x - self.content.x,
                        y: point.y - self.content.y,
                    },
                )),
        )
    }
    fn cursor_at(&self, point: Point) -> CursorShape {
        self.pointer_cursor(point)
    }
}

fn context_child_args(parent: &Value, child: &Value) -> Value {
    if parent.get("_context_menu").is_none() {
        return child.clone();
    }
    let mut args = child.as_object().cloned().unwrap_or_default();
    for key in [
        "paths",
        "group",
        "index",
        "event",
        "_context_menu",
        "_context_sheet",
        "_context_version",
        "_context_byte",
    ] {
        if let Some(value) = parent.get(key) {
            vel_config::ui::merge_context_arg(&mut args, key, value);
        }
    }
    Value::Object(args)
}
fn native_menu(
    app: &App,
    items: &[vel_config::ui::MenuItem],
    macros: &[vel_config::ui::PaletteItem],
) -> Vec<MenuEntry> {
    let keymap = &app.workspace.keymap;
    let tr = &app.workspace.translations;
    items
        .iter()
        .filter(|item| {
            #[cfg(windows)]
            return !matches!(item.platform.as_deref(), Some("OSX" | "!Windows" | "Linux"));
            #[cfg(target_os = "macos")]
            return !matches!(item.platform.as_deref(), Some("!OSX" | "Windows" | "Linux"));
        })
        .map(|item| {
            let caption = if let Some(caption) = app.menu_caption(&item.id) {
                caption.to_owned()
            } else if item.command.as_deref() == Some("set_ui_language")
                && !matches!(item.args["language"].as_str(), Some("en" | "auto"))
            {
                item.caption.clone()
            } else {
                localized_menu_caption(tr, &item.caption, item.command.as_deref()).to_owned()
            };
            let shortcut = item.command.as_ref().and_then(|command| {
                if command.starts_with("native_") {
                    item.shortcut.clone()
                } else {
                    keymap.menu_shortcut(command, &item.args)
                }
            });
            // AppKit cannot dispatch a sequence as a key equivalent.
            let shortcut_hint =
                if (cfg!(windows) || item.id == "side_bar_visibility") && shortcut.is_none() {
                    item.command.as_ref().and_then(|command| {
                        keymap.menu_sequence_hint(&vel_config::keymap::Invocation {
                            command: command.clone(),
                            args: item.args.clone(),
                        })
                    })
                } else {
                    None
                };
            let children = if item.id == "macros" {
                macros
                    .iter()
                    .map(|row| MenuEntry {
                        caption: tr.tr_fmt(
                            "Macro: {name}",
                            &[(
                                "name",
                                crate::workspace::macro_name(&row.caption).to_owned(),
                            )],
                        ),
                        command: Some(row.invocation.command.clone()),
                        args: row.invocation.args.to_string(),
                        shortcut: keymap
                            .menu_shortcut(&row.invocation.command, &row.invocation.args),
                        ..Default::default()
                    })
                    .collect()
            } else {
                native_menu(app, &item.children, macros)
            };
            MenuEntry {
                caption,
                mnemonic: item.mnemonic.as_deref().and_then(|key| key.chars().next()),
                id: item.id.clone(),
                command: item.command.clone(),
                args: item.args.to_string(),
                shortcut,
                shortcut_hint,
                children,
            }
        })
        .collect()
}

fn localized_menu_caption<'a>(
    tr: &'a vel_i18n::Translator,
    caption: &'a str,
    command: Option<&str>,
) -> &'a str {
    // Captions that are resource/data names are not UI strings.
    if matches!(
        command,
        Some(
            "open_recent_file"
                | "open_recent_folder"
                | "open_project"
                | "run_macro_file"
                | "set_file_type"
        )
    ) || (command == Some("set_build_system") && caption != "Automatic")
    {
        caption
    } else {
        tr.caption(caption)
    }
}

#[cfg(target_os = "macos")]
fn install_application_rows(entries: &mut Vec<MenuEntry>, tr: &vel_i18n::Translator) {
    // https://developer.apple.com/design/human-interface-guidelines/the-menu-bar#App-menu
    // Advertise the JSON settings shortcut once, in the application menu.
    let settings = entries
        .iter_mut()
        .find(|item| item.id == "preferences")
        .and_then(|item| {
            item.children
                .iter_mut()
                .find(|entry| entry.id == "settings")
        })
        .map(|entry| {
            let settings = entry.clone();
            entry.shortcut = None;
            settings
        });
    let mut rows = Vec::new();
    for (caption, command, shortcut) in [
        ("About Velocity", "native_about", None),
        ("-", "", None),
        ("Settings…", "", None),
        ("-", "", None),
        ("Services", "native_services", None),
        ("-", "", None),
        ("Hide Velocity", "native_hide", Some("super+h")),
        ("Hide Others", "native_hide_others", Some("alt+super+h")),
        ("Show All", "native_show_all", None),
        ("-", "", None),
        ("Quit Velocity", "native_quit", Some("super+q")),
    ] {
        match caption {
            "Settings…" => rows.extend(settings.iter().cloned()),
            _ => rows.push(MenuEntry {
                caption: tr.tr(caption).into_owned(),
                command: (!command.is_empty()).then(|| command.into()),
                shortcut: shortcut.map(str::to_owned),
                args: "{}".into(),
                ..Default::default()
            }),
        }
    }
    // Unrecognized package menus keep relative order; Window and Help trail them.
    entries.sort_by_key(|item| match item.id.as_str() {
        "file" => 0,
        "edit" => 1,
        "selection" => 2,
        "find" => 3,
        "view" => 4,
        "goto" => 5,
        "tools" => 6,
        "project" => 7,
        "preferences" => 8,
        "window" => 10,
        "help" => 11,
        _ => 9,
    });
    entries.insert(
        0,
        MenuEntry {
            caption: "Velocity".into(),
            id: "application".into(),
            children: rows,
            ..Default::default()
        },
    );
}

/// How long an interactive window waits for its first real scanout before startup
/// continues without it. Benchmarks and other automatic modes always wait for a real
/// frame (they check the display status instead).
const INTERACTIVE_SCANOUT_WAIT: Duration = Duration::from_secs(1);
fn stop_waiting_for_scanout(mode: Mode, waited: Duration) -> bool {
    mode == Mode::Interactive && waited >= INTERACTIVE_SCANOUT_WAIT
}

#[cfg(test)]
mod scanout_wait_tests {
    use super::*;
    #[test]
    fn interactive_startup_stops_waiting_for_a_real_scanout_but_benchmarks_do_not() {
        let just_under = INTERACTIVE_SCANOUT_WAIT - Duration::from_millis(1);
        assert!(!stop_waiting_for_scanout(Mode::Interactive, Duration::ZERO));
        assert!(!stop_waiting_for_scanout(Mode::Interactive, just_under));
        assert!(stop_waiting_for_scanout(
            Mode::Interactive,
            INTERACTIVE_SCANOUT_WAIT
        ));
        assert!(stop_waiting_for_scanout(
            Mode::Interactive,
            Duration::from_secs(60)
        ));
        for mode in [
            Mode::Startup,
            Mode::Open,
            Mode::Typing,
            Mode::Scroll,
            Mode::LargeFiles,
        ] {
            assert!(
                !stop_waiting_for_scanout(mode, Duration::from_secs(60)),
                "{mode:?}"
            );
        }
    }
}

/// Whether a window installs its menu: only the key window, and only when its model changed,
/// it just became key, or another window installed its menu meanwhile.
fn menu_install_wanted(focused: bool, rebuild: bool, focus_changed: bool, owner: bool) -> bool {
    focused && (rebuild || focus_changed || !owner)
}

fn prune_empty_menus(entries: &mut Vec<MenuEntry>) {
    for entry in &mut *entries {
        prune_empty_menus(&mut entry.children);
    }
    entries.retain(|entry| {
        entry.caption == "-" || entry.command.is_some() || !entry.children.is_empty()
    });
}

#[cfg(test)]
mod workbench_scenes {
    use super::*;
    fn app(workspace: Workspace) -> App {
        let reporter = Reporter::for_options(&Options::default(), Instant::now()).unwrap();
        App::new(
            Options::default(),
            Clock::new(Instant::now(), std::time::SystemTime::now()),
            reporter,
            workspace,
            Arc::new(AtomicBool::new(false)),
            std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(None))),
        )
    }
    #[test]
    fn palette_query_before_first_present_can_accept_a_command() {
        let workspace = Workspace::new_for_startup(
            vel_text::Text::new(),
            None,
            None,
            true,
            &mut Default::default(),
        )
        .unwrap();
        let mut app = app(workspace);
        assert!(!app.startup_done && app.workspace.palette_pending());
        app.sync_palette_enabled();
        app.panels
            .action(
                &mut app.workspace,
                "show_overlay",
                &json!({"overlay":"command_palette", "text":"Open Crash Reports"}),
                0..4,
            )
            .unwrap();
        let (_, invocation) = app
            .panels
            .key(
                &mut app.workspace,
                &vel_config::keymap::Key::parse("enter").unwrap(),
                0..4,
            )
            .unwrap();
        assert_eq!(invocation.unwrap().command, "open_crash_reports");
        assert!(!app.workspace.palette_pending());
        assert!(app.workspace.startup_services_pending());
    }
    #[test]
    fn palette_filters_and_rechecks_actual_command_enablement() {
        let workspace = Workspace::new(vel_text::Text::from("text"), None, None, false).unwrap();
        let mut app = app(workspace);
        app.workspace.palette = [("Close Split", "close_split"), ("New File", "new_file")]
            .into_iter()
            .map(|(caption, command)| vel_config::ui::PaletteItem {
                caption: caption.into(),
                invocation: vel_config::keymap::Invocation {
                    command: command.into(),
                    args: json!({}),
                },
            })
            .collect();
        app.sync_palette_enabled();
        app.panels
            .action(
                &mut app.workspace,
                "show_overlay",
                &json!({"overlay":"command_palette"}),
                0..4,
            )
            .unwrap();
        // Acceptance before the worker publishes also uses the filtered candidates.
        let (_, invocation) = app
            .panels
            .key(
                &mut app.workspace,
                &vel_config::keymap::Key::parse("enter").unwrap(),
                0..4,
            )
            .unwrap();
        assert_eq!(invocation.unwrap().command, "new_file");
        crate::workbench::configure_groups(&mut app.workspace, 2).unwrap();
        app.sync_palette_enabled();
        app.panels
            .action(
                &mut app.workspace,
                "show_overlay",
                &json!({"overlay":"command_palette","text":"Close Split"}),
                0..4,
            )
            .unwrap();
        assert!(app.command_enabled("close_split", "{}"));
        crate::workbench::configure_groups(&mut app.workspace, 1).unwrap();
        app.sync_palette_enabled();
        assert!(!app.command_enabled("close_split", "{}"));
        let (_, invocation) = app
            .panels
            .key(
                &mut app.workspace,
                &vel_config::keymap::Key::parse("enter").unwrap(),
                0..4,
            )
            .unwrap();
        assert!(invocation.is_none());
        assert_eq!(app.panels.overlay, Some(crate::panels::Overlay::Palette));
    }

    #[test]
    fn palette_snapshot_is_reused_and_only_the_accepted_row_is_rechecked() {
        let workspace = Workspace::new(vel_text::Text::from("text"), None, None, false).unwrap();
        let mut app = app(workspace);
        app.workspace.palette = vec![vel_config::ui::PaletteItem {
            caption: "Close Split".into(),
            invocation: vel_config::keymap::Invocation {
                command: "close_split".into(),
                args: json!({}),
            },
        }];
        crate::workbench::configure_groups(&mut app.workspace, 2).unwrap();
        app.sync_palette_enabled();
        app.panels
            .action(
                &mut app.workspace,
                "show_overlay",
                &json!({"overlay":"command_palette"}),
                0..4,
            )
            .unwrap();
        crate::workbench::configure_groups(&mut app.workspace, 1).unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        for _ in 0..50 {
            app.panels.poll(&mut app.workspace, 0..4);
        }
        loop {
            let (_, invocation) = app
                .panels
                .key(
                    &mut app.workspace,
                    &vel_config::keymap::Key::parse("enter").unwrap(),
                    0..4,
                )
                .unwrap();
            if let Some(invocation) = invocation {
                assert!(!app.accept_palette_command(&invocation.command, &invocation.args));
                break;
            }
            assert!(Instant::now() < until);
            app.panels.poll(&mut app.workspace, 0..4);
        }
        assert_eq!(app.panels.overlay, Some(crate::panels::Overlay::Palette));
        crate::workbench::configure_groups(&mut app.workspace, 2).unwrap();
        let (_, invocation) = app
            .panels
            .key(
                &mut app.workspace,
                &vel_config::keymap::Key::parse("enter").unwrap(),
                0..4,
            )
            .unwrap();
        let invocation = invocation.unwrap();
        assert!(app.accept_palette_command(&invocation.command, &invocation.args));
        assert!(app.panels.overlay.is_none());
    }

    #[test]
    fn result_enablement_uses_the_last_search_or_build_source() {
        let workspace = Workspace::new(vel_text::Text::from("cat"), None, None, false).unwrap();
        let mut app = app(workspace);
        assert!(!app.command_enabled("next_result", "{}"));
        app.panels.result_view.push(vel_search::files::Line {
            path: Path::new("Untitled 1").into(),
            number: 1,
            text: "cat".into(),
            highlights: std::iter::once(0..3).collect(),
            column_offset: 0,
            match_count: 1,
        });
        app.panels.search_results_active = true;
        assert!(app.command_enabled("next_result", "{}"));
        assert!(app.command_enabled("prev_result", "{}"));
        app.panels.search_results_active = false;
        assert!(!app.command_enabled("next_result", "{}"));
        assert!(!app.command_enabled("prev_result", "{}"));
    }
    #[test]
    fn four_groups_render_per_group_tabs_and_keep_actual_buffer_on_focused_sheet() {
        let mut workspace =
            Workspace::new(vel_text::Text::from("hello\nworld"), None, None, false).unwrap();
        crate::workbench::configure_groups(&mut workspace, 4).unwrap();
        let mut app = app(workspace);
        let scene = app.scene();
        assert!(!scene.layers.is_empty());
        assert_eq!(app.sheet_bounds.len(), 4);
        assert_eq!(app.group_bounds.len(), 4);
        assert!(app.group_bounds[2].y > app.group_bounds[0].y);
        assert_eq!(
            app.workspace.editor().buffer.id(),
            app.workspace.tabs[app.workspace.active].document
        );
        assert_eq!(
            app.panels
                .tree
                .nodes
                .values()
                .filter(|n| n.visible && n.class == "tab_control")
                .count(),
            4
        );
    }
    #[test]
    fn multi_selected_sheets_render_in_one_group_with_connector_hooks() {
        let mut workspace = Workspace::new(vel_text::Text::from("one"), None, None, false).unwrap();
        workspace.new_file().unwrap();
        workspace.new_file().unwrap();
        workspace.click_sheet(0, false, false);
        workspace.click_sheet(2, false, true);
        let mut app = app(workspace);
        app.scene();
        assert_eq!(app.group_bounds.len(), 1);
        assert_eq!(app.sheet_bounds.len(), 3);
        assert_eq!(
            app.panels
                .tree
                .nodes
                .values()
                .filter(|n| n.visible && n.class == "tab_connector")
                .count(),
            3
        );
        let width = app.group_bounds[0].width / 3.;
        assert!(
            app.sheet_bounds
                .values()
                .all(|r| (r.width - width).abs() < 0.01)
        );
    }
}

#[cfg(test)]
#[path = "app_round16_tests.rs"]
mod round16_tests;
#[cfg(test)]
#[path = "app_round17_tests.rs"]
mod round17_tests;
#[cfg(test)]
#[path = "app_round18_tests.rs"]
mod round18_tests;
#[cfg(test)]
#[path = "app_round41_lane_b_tests.rs"]
mod round41_lane_b_tests;

#[cfg(test)]
mod round15_tests {
    use super::*;
    use std::os::unix::net::{UnixListener, UnixStream};
    fn app(workspace: Workspace, data: Option<&Path>) -> App {
        App::new(
            Options::default(),
            Clock::new(Instant::now(), std::time::SystemTime::now()),
            Reporter::for_options(&Options::default(), Instant::now()).unwrap(),
            workspace,
            Arc::new(AtomicBool::new(false)),
            std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(data))),
        )
    }
    #[test]
    fn empty_window_sidebar_omits_input_adapter_before_and_after_preview() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("preview.txt");
        std::fs::write(&file, "preview").unwrap();
        let mut workspace = Workspace::new(vel_text::Text::new(), None, None, false).unwrap();
        workspace.close(0).unwrap();
        let mut app = app(workspace, None);
        app.run_named(None, "toggle_side_bar", &json!({}));
        app.run_named(None, "toggle_show_open_files", &json!({}));
        for expected in [0, 1, 0] {
            match expected {
                1 => app
                    .workspace
                    .preview(crate::cli::FileLocation {
                        path: file.clone(),
                        line: 1,
                        column: 1,
                    })
                    .unwrap(),
                _ if !app.workspace.is_empty() => {
                    app.workspace.close(app.workspace.active).unwrap()
                }
                _ => {}
            }
            app.scene();
            assert_eq!(
                app.panels
                    .tree
                    .nodes
                    .values()
                    .filter(|node| node.visible && node.class == "open_file")
                    .count(),
                expected
            );
        }
    }
    #[test]
    fn empty_windows_disable_all_save_variants() {
        let mut workspace = Workspace::new(vel_text::Text::new(), None, None, false).unwrap();
        workspace.close(0).unwrap();
        let app = app(workspace, None);
        for command in [
            "save",
            "save_as",
            "prompt_save_as",
            "save_and_close",
            "save_encoding",
            "save_all",
        ] {
            assert!(!app.command_enabled(command, "{}"), "{command}");
        }
    }
    #[test]
    fn save_all_is_enabled_for_dirty_siblings_with_a_read_only_active_tab() {
        let mut workspace = Workspace::new(vel_text::Text::new(), None, None, false).unwrap();
        workspace
            .editor_mut()
            .run("insert", &json!({"characters":"dirty sibling"}))
            .unwrap();
        workspace.new_file().unwrap();
        workspace.editor_mut().read_only = true;
        let app = app(workspace, None);
        assert!(app.command_enabled("save_all", "{}"));
        assert!(!app.command_enabled("save", "{}"));
    }
    #[test]
    fn cli_batch_acknowledges_errors_and_keeps_all_later_successes() {
        use std::io::Read;
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.socket");
        let _socket = UnixListener::bind(&bad).unwrap();
        let good = dir.path().join("good.txt");
        std::fs::write(&good, "good").unwrap();
        let mut request = crate::instance::Request::new(&Options::default(), None).unwrap();
        request.files = [bad.clone(), good, bad]
            .into_iter()
            .map(|path| crate::cli::FileLocation {
                path,
                line: 1,
                column: 1,
            })
            .collect();
        let (stream, mut client) = UnixStream::pair().unwrap();
        let mut app = app(
            Workspace::new(vel_text::Text::new(), None, None, false).unwrap(),
            None,
        );
        app.accept_request(crate::instance::Incoming::fixture(request, stream))
            .unwrap();
        let mut ack = [0];
        client.read_exact(&mut ack).unwrap();
        assert_eq!(ack, [1]);
        assert_eq!(app.workspace.editor().text.to_string(), "good");
        assert_eq!(app.panels.message.lines().count(), 2);
    }
    #[test]
    fn cli_project_switch_retains_dirty_untitled_window_and_publishes_both() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("new.sublime-project");
        std::fs::write(&project, "{}").unwrap();
        let mut workspace = Workspace::new(vel_text::Text::new(), None, None, false).unwrap();
        workspace
            .editor_mut()
            .run("insert", &json!({"characters":"recover this untitled"}))
            .unwrap();
        let mut app = app(workspace, Some(dir.path()));
        let options = Options {
            project: Some(project),
            ..Options::default()
        };
        let request = crate::instance::Request::new(&options, None).unwrap();
        app.accept_request(crate::instance::Incoming::local(request))
            .unwrap();
        app.checkpoint(false).unwrap();
        app.sessions.borrow().flush().unwrap();
        let recovered = vel_session::load(&dir.path().join("Sessions/Session.sublime-session"));
        assert_eq!(recovered.session.windows.len(), 2);
        assert!(recovered.session.documents.iter().any(|d| {
            d.history
                .as_ref()
                .and_then(|h| h.current_text().ok())
                .is_some_and(|text| text == "recover this untitled")
        }));
    }
}

#[cfg(test)]
#[path = "round16_review_app_tests.rs"]
mod round16_review_tests;

#[cfg(test)]
#[path = "app_lane_b_round17_tests.rs"]
mod lane_b_round17_tests;

#[path = "app_build.rs"]
mod app_build;

#[path = "app_chrome.rs"]
mod app_chrome;

#[cfg(test)]
#[path = "app_lane_b_round18_tests.rs"]
mod lane_b_round18_tests;

#[cfg(test)]
#[path = "app_lane_b_round19_perf.rs"]
mod lane_b_round19_perf;

#[cfg(test)]
#[path = "app_lane_b_round19_tests.rs"]
mod lane_b_round19_tests;

#[cfg(test)]
#[path = "app_round21_review_tests.rs"]
mod round21_review;

#[cfg(all(test, target_os = "macos"))]
#[path = "app_round21_typing_perf.rs"]
mod round21_typing_perf;

#[cfg(test)]
#[path = "app_round22_tests.rs"]
mod round22;
