//! Native application handler and buffered benchmark observations.
use crate::app_scene_translate as translate_scene;
#[path = "app_a11y.rs"]
mod accessibility_adapter;
#[path = "app_context.rs"]
mod context_adapter;
#[path = "app_mouse.rs"]
mod mouse_adapter;
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
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use vel_config::{Value, settings::Layer};
use vel_platform::*;
use vel_render::scene::Layer as SceneLayer;
use vel_render::{FrameTimes, Renderer, font::Fonts};

type CommandStates = Vec<(u64, vel_buffer::ChangeId, vel_buffer::Selection)>;
#[derive(Clone, Debug, Serialize)]
struct Sample {
    index: usize,
    event_to_submit_ms: f64,
    event_to_present_return_ms: f64,
    acquire_ms: f64,
    encode_submit_ms: f64,
    present_call_ms: f64,
    timer_lateness_ms: Option<f64>,
}
#[derive(Debug)]
pub struct App {
    /// A playback may edit only the workbench sheet that started it, including nested chains.
    macro_origin: Option<u64>,
    pending_menu: Option<context_adapter::PendingMenu>,
    accessibility_focus: Option<accessibility::Id>,
    accessibility_preferences: accessibility::Preferences,
    releases: crate::release::Actions,
    title_theme: Option<(u64, u64)>,
    exact_scene: bool,
    exact_drawables: std::collections::BTreeSet<u64>,
    content_scanout: Option<Instant>,
    highlighted_scanout: Option<Instant>,
    language_warmed: bool,
    window_focused: bool,
    panels: crate::panels::Panels,
    window_size: Size,
    content: Rect,
    sheet_views: std::collections::BTreeMap<u64, View>,
    sheet_bounds: std::collections::BTreeMap<u64, Rect>,
    group_bounds: Vec<Rect>,
    workbench_bounds: Rect,
    tab_hits: Vec<(u64, Rect)>,
    /// Tab bar of each group and its largest scroll offset, for clicks on its empty part and
    /// for clamping wheel scrolling.
    tab_bars: Vec<(usize, Rect, f64)>,
    tab_button_hits: Vec<(usize, &'static str, Rect)>,
    keyboard_center: Option<u64>,
    tab_close_hits: Vec<(u64, Rect)>,
    dragging_splitter: Option<vel_ui::workbench::Splitter>,
    /// Text selection gesture between a press in the text area and its release.
    drag_select: Option<crate::mouse_select::DragSelect>,
    /// Selected text being dragged to a new position.
    drag_text: Option<crate::mouse_select::DragText>,
    text_drag_pressed: Option<Instant>,
    text_drag_origin: Option<(Point, usize)>,
    mouse_preferences: vel_platform::mouse::MousePreferences,
    builds: crate::build::Builds,
    normal_sidebar: bool,
    distraction_sidebar: Option<bool>,
    fullscreen_before_distraction: bool,
    sessions: std::rc::Rc<std::cell::RefCell<crate::session::Sessions>>,
    session_id: u64,
    session_due: Option<Instant>,
    restored_windows: Vec<vel_session::Window>,
    restored_reserved_ids: std::collections::VecDeque<u64>,
    waiters: Vec<crate::instance::Waiter>,
    view_buffer: Option<u64>,
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
impl App {
    /// Restores retained documents before a temporary native receiver can write the profile.
    pub(crate) fn take_over_native_primary(
        &mut self,
        instance: crate::instance::Instance,
    ) -> Result<(), String> {
        let directory = self.workspace.data_path().map(Path::to_owned);
        let mut sessions = crate::session::Sessions::new(directory.as_deref());
        let restored = sessions.restored.clone();
        let mut reserved = sessions.reserve_recovered().into_iter();
        let first = restored.windows.first().cloned();
        let project = first
            .as_ref()
            .and_then(|window| window.project.clone())
            .filter(|path| path.is_file());
        let mut workspace = Workspace::new(vel_text::Text::new(), directory, project, true)?;
        if let Some(window) = &first {
            workspace.restore_window(window, &restored.documents)?;
        }
        workspace.recent_files = restored.recent_files;
        workspace.recent_folders = restored.recent_folders;
        workspace.recent_projects = restored.recent_projects;
        workspace.messages.append(&mut sessions.messages);
        let id = reserved.next().unwrap_or_else(|| sessions.register());
        sessions.instance = Some(instance);
        self.sessions = std::rc::Rc::new(std::cell::RefCell::new(sessions));
        self.session_id = id;
        self.workspace = workspace;
        self.panels = crate::panels::Panels::new(&self.workspace, true);
        self.panels.review.native_context = true;
        self.title_theme = None;
        self.view_buffer = None;
        self.sheet_views.clear();
        if let Some(window) = &first {
            self.restore_ui(window);
        }
        self.restored_windows = restored.windows.into_iter().skip(1).collect();
        self.restored_reserved_ids = reserved.collect();
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
        if options.mode == Mode::ThemeUi
            && let Err(e) = crate::theme_bench::demo(&mut workspace)
        {
            workspace.messages.push(e);
        }
        if options.mode == Mode::Typing {
            crate::install_carets(workspace.editor_mut(), options.carets);
        }
        let view = View::new(
            Size {
                width: options.width as f64,
                height: options.height as f64,
            },
            8.,
            12.,
        );
        let session_id = sessions.borrow_mut().register();
        Self {
            macro_origin: None,
            pending_menu: None,
            accessibility_focus: None,
            accessibility_preferences: Default::default(),
            releases: Default::default(),
            title_theme: None,
            sessions,
            session_id,
            session_due: Some(Instant::now()),
            restored_windows: Vec::new(),
            restored_reserved_ids: Default::default(),
            waiters: Vec::new(),
            exact_scene: false,
            exact_drawables: Default::default(),
            content_scanout: None,
            highlighted_scanout: None,
            language_warmed: false,
            window_focused: true,
            panels: {
                let mut panels =
                    crate::panels::Panels::new(&workspace, options.mode == Mode::Interactive);
                panels.message = std::mem::take(&mut workspace.resource_reports).join("; ");
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
            sheet_bounds: Default::default(),
            group_bounds: Vec::new(),
            workbench_bounds: Rect::default(),
            tab_hits: Vec::new(),
            tab_bars: Vec::new(),
            tab_button_hits: Vec::new(),
            keyboard_center: None,
            tab_close_hits: Vec::new(),
            dragging_splitter: None,
            drag_select: None,
            drag_text: None,
            text_drag_pressed: None,
            text_drag_origin: None,
            mouse_preferences: vel_platform::mouse::MousePreferences::read(),
            builds: crate::build::Builds::default(),
            normal_sidebar: true,
            distraction_sidebar: None,
            fullscreen_before_distraction: false,
            view_buffer: None,
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
        self.normal_sidebar = window.normal_sidebar.unwrap_or(true);
        self.distraction_sidebar = window.distraction_sidebar;
        self.fullscreen_before_distraction = window.fullscreen_before_distraction;
    }
    pub fn restore_session_id(&mut self, id: u64) {
        self.session_id = id;
        self.sessions.borrow_mut().active_window = Some(id);
    }
    pub fn restore_remaining_reserved(&mut self, windows: Vec<vel_session::Window>, ids: Vec<u64>) {
        self.restored_windows = windows;
        self.restored_reserved_ids = ids.into();
    }
    pub(crate) fn open_received_paths(
        &mut self,
        paths: &[std::path::PathBuf],
    ) -> Result<(), String> {
        crate::open_requests::open_paths(&mut self.workspace, &mut self.panels, paths)
    }
    pub fn set_crash(&mut self, crash: Option<crate::release::CrashStartup>) {
        self.releases.crash = crash;
    }
    pub fn set_instance(&mut self, instance: Option<crate::instance::Instance>) {
        self.sessions.borrow_mut().instance = instance;
    }
    fn hot_exit(&self) -> vel_session::HotExit {
        self.workspace.hot_exit()
    }
    fn checkpoint(&mut self, quitting: bool) -> Result<(), String> {
        self.checkpoint_reason(quitting, false)
    }
    fn checkpoint_reason(&mut self, quitting: bool, closing: bool) -> Result<(), String> {
        // Cache stdin clients' current contents before a project switch can replace
        // this window's documents. Completion still waits until their sheets close.
        self.waiters
            .retain_mut(|waiter| !waiter.complete(&mut self.workspace));
        let active = self.workspace.active;
        self.workspace.tabs[active].scroll = (self.view.scroll_x, self.view.scroll_y);
        self.workspace.tabs[active].scroll_line_height = self.view.line_height;
        let (mut window, documents) = self.workspace.snapshot()?;
        self.panels.capture_session(&mut window);
        window.normal_sidebar = Some(self.normal_sidebar);
        window.distraction_sidebar = self.distraction_sidebar;
        window.fullscreen_before_distraction = self.fullscreen_before_distraction;
        let hot = self.hot_exit();
        let remember = self.workspace.global_bool("remember_open_files", true);
        self.sessions.borrow_mut().publish(
            self.session_id,
            window,
            documents,
            [
                self.workspace.recent_files.clone(),
                self.workspace.recent_folders.clone(),
                self.workspace.recent_projects.clone(),
            ],
            crate::session::Policy {
                hot,
                quitting,
                remember,
                closing,
            },
        )?;
        self.session_due = None;
        Ok(())
    }
    fn save_workspace_checkpoint(&mut self) -> Result<(), String> {
        self.checkpoint(true)?;
        self.sessions.borrow().flush()
    }
    fn open_native_window(
        &mut self,
        w: &WindowHandle,
        restored: Option<vel_session::Window>,
        reserved_id: Option<u64>,
    ) -> Result<WindowHandle, String> {
        let project = restored.as_ref().and_then(|s| s.project.clone());
        let mut workspace = Workspace::new(
            vel_text::Text::new(),
            self.workspace.data_path().map(Path::to_owned),
            project,
            true,
        )?;
        workspace.recent_files = self.workspace.recent_files.clone();
        workspace.recent_folders = self.workspace.recent_folders.clone();
        workspace.recent_projects = self.workspace.recent_projects.clone();
        if let Some(window) = &restored {
            let documents = self.sessions.borrow().restored.documents.clone();
            workspace.restore_window(window, &documents)?;
        }
        let mut options = self.options.clone();
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
        w.new_window(
            WindowOptions {
                title: "Velocity".into(),
                size: Size {
                    width: options.width as f64,
                    height: options.height as f64,
                },
            },
            app,
        )
    }
    fn poll_workbench(&mut self, w: &WindowHandle) -> Result<(), String> {
        if self.workspace.refresh_file_menus() {
            w.set_menu(&self.application_menu());
        }
        let remaining = std::mem::take(&mut self.restored_windows);
        for window in remaining {
            let id = self.restored_reserved_ids.pop_front();
            if let Err(e) = self.open_native_window(w, Some(window), id) {
                self.workspace.messages.push(e);
            }
        }
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
        self.waiters
            .retain_mut(|waiter| !waiter.complete(&mut self.workspace));
        let messages = self.sessions.borrow_mut().poll();
        for message in messages {
            self.panels.message = message.clone();
            self.reporter
                .emit(json!({"kind":"session_error","message":message}))?;
        }
        if self.session_due.is_some_and(|due| Instant::now() >= due)
            && let Err(e) = self.checkpoint(false)
        {
            self.panels.message = format!("Session save failed: {e}");
            self.session_due = None;
        }
        Ok(())
    }
    fn receive_request(
        &mut self,
        w: &WindowHandle,
        incoming: crate::instance::Incoming,
    ) -> Result<(), String> {
        let native_open = incoming.request.native_open;
        let new_window =
            crate::open_requests::request_needs_window(&self.workspace, &incoming.request);
        if new_window {
            let workspace = Workspace::new(
                vel_text::Text::new(),
                self.workspace.data_path().map(Path::to_owned),
                None,
                true,
            )?;
            let mut options = self.options.clone();
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
            app.accept_request(incoming)?;
            w.new_window(
                WindowOptions {
                    title: "Velocity".into(),
                    size: Size {
                        width: options.width as f64,
                        height: options.height as f64,
                    },
                },
                app,
            )?;
        } else {
            self.accept_request(incoming)?;
            w.focus();
            self.changed(w, Instant::now(), !native_open);
        }
        Ok(())
    }
    fn open_native_window_request(
        &mut self,
        w: &WindowHandle,
        request: crate::instance::Request,
    ) -> Result<(), String> {
        let (stream, _client) =
            std::os::unix::net::UnixStream::pair().map_err(|e| e.to_string())?;
        let mut request = request;
        request.queued_ack = true;
        request.new_window = true;
        self.receive_request(w, crate::instance::Incoming { request, stream })
    }
    fn accept_request(&mut self, incoming: crate::instance::Incoming) -> Result<(), String> {
        let request = incoming.request.clone();
        let mut ids = std::collections::BTreeSet::new();
        let mut errors = Vec::new();
        if let Some(project) = request.project.clone() {
            let retain = self.hot_exit().retain(false);
            let result = (|| {
                self.checkpoint_reason(false, true)?;
                self.sessions.borrow().flush()?;
                self.workspace
                    .open_project(project.clone())
                    .map_err(|error| format!("{}: {error}", project.display()))?;
                self.sessions
                    .borrow_mut()
                    .retain_departing(self.session_id, retain);
                self.panels = crate::panels::Panels::new(&self.workspace, true);
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
        if let Some(text) = request.stdin.clone() {
            let result = self.workspace.new_file().and_then(|()| {
                self.workspace
                    .editor_mut()
                    .run("insert", &json!({"characters":text}))
            });
            if let Err(error) = result {
                errors.push(error);
            } else {
                let sheet = self.workspace.tabs[self.workspace.active].sheet;
                ids.insert(sheet);
                self.workspace.stdout_sheets.insert(sheet);
            }
        }
        if request.files.is_empty() && request.stdin.is_none() && !self.workspace.is_empty() {
            ids.insert(self.workspace.tabs[self.workspace.active].sheet);
        }
        if !errors.is_empty() {
            self.panels.message = errors.join("\n");
        }
        // Path failures do not suppress the accepted/closed protocol or later paths.
        if let Some(waiter) =
            crate::instance::Waiter::acknowledge(incoming, &mut self.workspace, ids)?
        {
            self.waiters.push(waiter);
        }
        Ok(())
    }
    fn show_error(&self, w: &WindowHandle, message: &str) {
        let _ = self
            .reporter
            .emit(json!({"kind":"editor_error","message":message}));
        w.alert("Velocity", message, &["OK"]);
    }
    fn publish_resource_reports(&mut self) {
        if !self.workspace.resource_reports.is_empty() {
            self.panels.message = std::mem::take(&mut self.workspace.resource_reports).join("; ");
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
    fn command_states(&self) -> CommandStates {
        self.workspace
            .tabs
            .iter()
            .map(|tab| {
                (
                    tab.sheet,
                    tab.editor.buffer.change_id(),
                    tab.editor.buffer.view(tab.editor.id).sel().clone(),
                )
            })
            .collect()
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
        if key.name == "escape" {
            self.keyboard_center = None;
        }
        self.workspace
            .editor_mut()
            .keymap
            .set_app_context("build_running", json!(self.builds.task.is_some()));
        let intercepted_before = self.command_states();
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
    fn application_menu(&mut self) -> Vec<MenuEntry> {
        let macros = self
            .panels
            .macro_file_items(&self.workspace)
            .unwrap_or_default();
        native_menu(&self.workspace.menus, &macros)
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
    fn run_named(&mut self, window: Option<&WindowHandle>, name: &str, args: &Value) {
        if let Err(error) = self.run_named_depth(window, name, args, 0) {
            self.named_error(window, &error);
        }
    }
    fn named_error(&mut self, window: Option<&WindowHandle>, message: &str) {
        if let Some(window) = window {
            self.show_error(window, message);
        } else {
            self.panels.message = message.into();
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
        if !self.macro_view_active() {
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
                let name = command[0]
                    .as_str()
                    .ok_or("chain entry needs [name, args]")?;
                if let Err(error) = self.run_named_depth(
                    window,
                    name,
                    &context_child_args(args, command.get(1).unwrap_or(&json!({}))),
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
            && crate::context_menu::TabCloseKind::from_command(name).is_none()
            && let Some(sheet) = args["_context_sheet"].as_u64()
            && let Some(index) = self.workspace.sheet_index(sheet)
        {
            self.workspace.select(index);
            self.sync_active_view();
        }
        if !self.macro_view_active() {
            return Ok(());
        }
        if !palette_accept && !self.command_enabled(name, &args.to_string()) {
            return Ok(());
        }
        match window.map_or(Ok(false), |window| self.context_action(window, name, args)) {
            Ok(true) => {
                self.named_changed(window, false);
                return Ok(());
            }
            Ok(false) => {}
            Err(error) => {
                self.panels.message = error;
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
        let document_before = self.command_states();
        let animate = (name == "move" && args["by"].as_str() == Some("pages"))
            || matches!(name, "show_at_center" | "goto_line" | "scroll_to");
        let now = Instant::now();
        let scroll_from = animate.then(|| {
            self.view
                .prepare_keyboard_scroll(self.workspace.editor(), now)
        });
        let result = self.workspace.editor_mut().run(name, args);
        if self.workspace.editor().clipboard != before
            && let Some(window) = window
        {
            window.set_clipboard_text(&self.workspace.editor().clipboard);
        }
        let actions = self.handle_named_actions(window);
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
            );
        if scroll_from.is_none() && (changed || explicit) {
            self.view.cancel_scroll_animation();
        }
        self.named_changed(window, changed || explicit);
        if let Some(from) = scroll_from {
            self.view.animate_scroll_from(
                self.workspace.editor(),
                from,
                now,
                self.accessibility_preferences.reduce_motion,
            );
        }
        result.and(actions)
    }
    fn save_tab(&mut self, w: &WindowHandle, index: usize, save_as: bool) -> bool {
        let path = if save_as || self.workspace.tabs[index].path.is_none() {
            let suggested = self.workspace.tabs[index]
                .path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.workspace.tabs[index].name());
            let Some(path) = w.save_file(&suggested) else {
                return false;
            };
            Some(path)
        } else {
            None
        };
        match self.workspace.save(index, path) {
            Ok(()) => true,
            Err(error) => {
                self.show_error(w, &error);
                false
            }
        }
    }
    fn can_close(&mut self, w: &WindowHandle, index: usize) -> bool {
        if self
            .workspace
            .tabs
            .iter()
            .filter(|t| t.document == self.workspace.tabs[index].document)
            .count()
            > 1
        {
            return true;
        }
        self.can_close_document(w, index)
    }
    fn can_close_many(&mut self, w: &WindowHandle, indices: &[usize]) -> bool {
        self.sync_workspace_after_panels();
        let mut seen = std::collections::BTreeSet::new();
        for index in indices {
            let doc = self.workspace.tabs[*index].document;
            if !seen.insert(doc)
                || self
                    .workspace
                    .tabs
                    .iter()
                    .enumerate()
                    .any(|(i, t)| t.document == doc && !indices.contains(&i))
            {
                continue;
            }
            if !self.can_close_document(w, *index) {
                return false;
            }
        }
        true
    }
    fn can_close_document(&mut self, w: &WindowHandle, index: usize) -> bool {
        let _ = self.workspace.prepare_sheet(index);
        self.workspace.tabs[index].editor.unmark();
        if !self.workspace.tabs[index].dirty() {
            return true;
        }
        match w.alert(
            "Save changes?",
            &self.workspace.tabs[index].name(),
            &["Save", "Don't Save", "Cancel"],
        ) {
            0 => self.save_tab(w, index, false),
            1 => true,
            _ => false,
        }
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
        if self.macro_origin.is_some_and(|sheet| {
            self.workspace.is_empty() || self.workspace.tabs[self.workspace.active].sheet != sheet
        }) {
            self.panels.set_message(
                "Macro playback stopped: the starting view is no longer active".into(),
            );
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
        if self.apply_view_action(name, args)? {
            return Ok(());
        }
        if self.apply_build_action(window, name, args)? {
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
        // These actions need only the workspace/panels. Both native and headless named
        // dispatch finish them here before advancing to the next macro step.
        if vel_commands::is_find_command(name) || name == "save_macro" {
            self.panels
                .action(&mut self.workspace, name, args, viewport)?;
            return Ok(());
        }
        if name == "new_file" {
            return self.workspace.new_file();
        }
        if matches!(name, "close" | "close_file") {
            let active = self.workspace.active;
            let can_close = if args["force"].as_bool().unwrap_or(false) {
                true
            } else if let Some(window) = window {
                self.can_close(window, active)
            } else if !self.workspace.tabs[active].dirty() {
                true
            } else {
                return Err(format!(
                    "command {name} needs a native window to confirm unsaved changes"
                ));
            };
            if can_close {
                let last = self.workspace.tabs.len() == 1;
                self.workspace.close(active)?;
                if last
                    && self
                        .workspace
                        .editor()
                        .settings
                        .bool("close_windows_when_empty", false)
                    && let Some(window) = window
                {
                    window.request_close();
                }
            }
            return Ok(());
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
            return self.workspace.open(crate::cli::FileLocation::parse(file));
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
        Err(format!("command {name} needs a native window"))
    }
    fn apply_action(&mut self, w: &WindowHandle, name: &str, args: &Value) -> Result<(), String> {
        if self.apply_view_action(name, args)? {
            return Ok(());
        }
        if matches!(name, "focus_next_control" | "focus_previous_control") {
            self.next_accessibility_focus(w, name == "focus_previous_control");
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
                if self.workspace.distraction_free {
                    self.normal_sidebar = self.panels.side_visible;
                    self.fullscreen_before_distraction = self.workspace.fullscreen;
                    self.panels.side_visible = self.workspace.window_visible("show_side_bar");
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
                    self.distraction_sidebar = Some(self.panels.side_visible);
                    self.panels.side_visible = self.workspace.window_visible("show_side_bar");
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
            "open_url" => {
                let url = args["url"].as_str().ok_or("missing URL")?;
                if matches!(
                    vel_minihtml::LinkTarget::parse(url),
                    Ok(vel_minihtml::LinkTarget::Browser(_))
                ) {
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
                let source = self.panels.intel.status_source.clone();
                w.new_window(
                    WindowOptions {
                        title: "Indexing Status — Velocity".into(),
                        size: Size {
                            width: 720.,
                            height: 480.,
                        },
                    },
                    crate::intelligence::StatusWindow::new(source),
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
            "close_selected_tabs" => {
                let mut selected = self.workspace.selected_indices();
                if self.can_close_many(w, &selected) {
                    selected.sort_unstable();
                    for index in selected.into_iter().rev() {
                        self.workspace.close(index)?;
                    }
                }
                return Ok(());
            }
            "next_result" | "prev_result" => {
                if self.panels.search_results_active {
                    self.panels
                        .navigate_result(&mut self.workspace, name == "next_result")?;
                } else {
                    self.builds
                        .navigate(&mut self.workspace, name == "next_result")?;
                }
                return Ok(());
            }
            "prompt_open_project" | "open_project" => {
                let path = args["file"]
                    .as_str()
                    .map(std::path::PathBuf::from)
                    .or_else(|| w.open_files().into_iter().next());
                if let Some(path) = path {
                    if !self.hot_exit().retain(false)
                        && !self
                            .can_close_many(w, &(0..self.workspace.tabs.len()).collect::<Vec<_>>())
                    {
                        return Ok(());
                    }
                    self.checkpoint_reason(false, true)?;
                    self.sessions.borrow().flush()?;
                    let retain = self.hot_exit().retain(false);
                    self.workspace
                        .open_project(path.clone())
                        .map_err(|error| format!("{}: {error}", path.display()))?;
                    self.sessions
                        .borrow_mut()
                        .retain_departing(self.session_id, retain);
                    self.panels = crate::panels::Panels::new(&self.workspace, true);
                }
                return Ok(());
            }
            "save_project_as" => {
                if let Some(path) = args["file"]
                    .as_str()
                    .map(std::path::PathBuf::from)
                    .or_else(|| w.save_file("project.sublime-project"))
                {
                    self.workspace.save_project_as(path)?;
                }
                return Ok(());
            }
            "close_project" => {
                if !self.hot_exit().retain(false)
                    && !self.can_close_many(w, &(0..self.workspace.tabs.len()).collect::<Vec<_>>())
                {
                    return Ok(());
                }
                self.checkpoint_reason(false, true)?;
                self.sessions.borrow().flush()?;
                let retain = self.hot_exit().retain(false);
                self.workspace.close_project()?;
                self.sessions
                    .borrow_mut()
                    .retain_departing(self.session_id, retain);
                self.panels = crate::panels::Panels::new(&self.workspace, true);
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
            "quick_switch_project" | "recent_projects" => {
                let items = self
                    .workspace
                    .recent_projects
                    .iter()
                    .map(|p| vel_config::ui::PaletteItem {
                        caption: p.display().to_string(),
                        invocation: vel_config::keymap::Invocation {
                            command: "open_project".into(),
                            args: json!({"file":p}),
                        },
                    })
                    .collect();
                self.panels.choices(&mut self.workspace, items);
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
            self.replacement_confirmation(w)?;
            return Ok(());
        }
        let active = self.workspace.active;
        match name {
            "new_file" => self.workspace.new_file()?,
            "reopen_last_file" => self.workspace.reopen_last_file()?,
            "clear_recent_items" => {
                self.workspace.recent_files.clear();
                self.workspace.recent_folders.clear();
                self.workspace.recent_projects.clear();
                self.sessions.borrow_mut().clear_recent(self.session_id);
            }
            "open_recent_file" => {
                self.workspace.open(crate::cli::FileLocation {
                    path: args["file"].as_str().ok_or("missing file")?.into(),
                    line: 1,
                    column: 1,
                })?;
            }
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
            "reopen_encoding" => {
                if self.can_close_document(w, active) {
                    self.workspace
                        .reopen_encoding(args["encoding"].as_str().ok_or("missing encoding")?)?;
                }
            }
            "save_encoding" => {
                self.workspace.tabs[active].format = vel_fileio::encoding_format(
                    args["encoding"].as_str().ok_or("missing encoding")?,
                    self.workspace.tabs[active].format.line_ending,
                )?;
                self.save_tab(w, active, false);
            }
            "open_file" => {
                if let Some(file) = args["file"].as_str() {
                    self.workspace.open(crate::cli::FileLocation::parse(file))?;
                } else if (args["force"].as_bool().unwrap_or(false) || self.can_close(w, active))
                    && let Some(path) = self.workspace.tabs[active].path.clone()
                {
                    let fallback = self
                        .workspace
                        .editor()
                        .preferences
                        .fallback_encoding
                        .clone();
                    if self.workspace.editor().binary.is_some() {
                        self.workspace.tabs[active].binary_loading =
                            Some(crate::binary::Task::open(path)?);
                    } else {
                        self.workspace.tabs[active].loading =
                            Some(vel_fileio::load_background(path, fallback)?);
                    }
                    self.workspace.tabs[active].editor.read_only = true;
                }
            }
            "prompt_open_file" => {
                let errors = crate::open_requests::open_locations(
                    &mut self.workspace,
                    &mut self.panels,
                    w.open_files()
                        .into_iter()
                        .map(|path| crate::cli::FileLocation {
                            path,
                            line: 1,
                            column: 1,
                        }),
                );
                if !errors.is_empty() {
                    self.show_error(w, &errors.join("\n"));
                }
            }
            "save" | "save_and_close" => {
                let saved = if let Some(file) = args["file"].as_str() {
                    match self
                        .workspace
                        .save(active, Some(std::path::PathBuf::from(file)))
                    {
                        Ok(()) => true,
                        Err(error) => {
                            self.show_error(w, &error);
                            false
                        }
                    }
                } else {
                    self.save_tab(w, active, false)
                };
                if saved && name == "save_and_close" {
                    self.workspace.close(active)?;
                }
            }
            "prompt_save_as" => {
                self.save_tab(w, active, true);
            }
            "save_all" => {
                let errors = self
                    .workspace
                    .save_all(|suggestion| w.save_file(suggestion));
                if !errors.is_empty() {
                    self.show_error(w, &errors.join("\n"));
                }
            }
            "next_view" | "prev_view" | "select_by_index" => {
                self.select_tab_action(name, args);
            }
            "new_window" => {
                self.open_native_window(w, None, None)?;
            }
            "close_window" => {
                w.request_close();
            }
            "increase_font_size" | "decrease_font_size" | "reset_font_size" => {
                self.workspace.zoom_font(name)?;
            }
            "set_line_endings" => {
                self.workspace.tabs[active].format.line_ending = vel_fileio::LineEnding::parse(
                    args["type"].as_str().ok_or("set_line_endings needs type")?,
                )?;
            }
            "set_encoding" => {
                let requested = args["encoding"]
                    .as_str()
                    .ok_or("set_encoding needs encoding")?;
                self.workspace.tabs[active].format = vel_fileio::encoding_format(
                    requested,
                    self.workspace.tabs[active].format.line_ending,
                )?;
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
    fn select_tab_action(&mut self, name: &str, args: &Value) -> bool {
        let group = &self.workspace.workbench.groups[self.workspace.workbench.active];
        let sheet = match name {
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
            "select_by_index" => args["index"]
                .as_u64()
                .and_then(|i| group.sheets.get(i as usize))
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
        self.window_size = size;
        // Pane widths are assigned by scene; apply_style anchors each view there.
        // Applying full-window width to the focused pane here causes a second reflow.
    }
    fn reload_font_views(&mut self, fonts: Fonts, scale: f64) {
        for view in std::iter::once(&mut self.view).chain(self.sheet_views.values_mut()) {
            view.set_font_grid(fonts.cell as f64, fonts.ascent as f64);
            view.set_font_metrics(fonts.metrics, scale);
            view.ligatures = fonts.has_ligatures();
        }
        self.fonts = Some(fonts);
        self.initialize_view_metrics();
    }
    fn initialize_view_metrics(&mut self) {
        if let Some(fonts) = &self.fonts {
            let scale = self.workspace.themes.ui.borrow().density as f64;
            self.view.set_font_metrics(fonts.metrics, scale);
            self.view.ligatures = fonts.has_ligatures();
            self.panels
                .output_view
                .set_font_metrics(fonts.metrics, scale);
        }
    }
    fn sync_active_view(&mut self) {
        let current = self.workspace.tabs[self.workspace.active].sheet;
        if self.view_buffer != Some(current) {
            if let Some(old) = self.view_buffer {
                self.save_on_focus_lost(Some(old));
                if let Some(index) = self.workspace.sheet_index(old) {
                    self.workspace.tabs[index].scroll = (self.view.scroll_x, self.view.scroll_y);
                    self.workspace.tabs[index].scroll_line_height = self.view.line_height;
                }
                let replacement = View::new(
                    self.view.size,
                    self.fonts.as_ref().map_or(8., |f| f.cell as f64),
                    self.fonts.as_ref().map_or(12., |f| f.ascent as f64),
                );
                self.sheet_views
                    .insert(old, std::mem::replace(&mut self.view, replacement));
            }
            let existing = self.sheet_views.remove(&current);
            let reused = existing.is_some();
            if let Some(view) = existing {
                self.view = view;
            }
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
        self.initialize_view_metrics();
        self.view.apply_style(self.workspace.editor());
        let tab = &self.workspace.tabs[self.workspace.active];
        if tab.loading.is_none() && tab.binary_loading.is_none() {
            self.view.finish_scroll_restore(&tab.editor);
        }
        if let Some(scroll) = self.panels.restore_scroll.take() {
            self.view.scroll_x = scroll.0;
            self.view.scroll_y = scroll.1;
        }
    }
    fn set_window_focused(&mut self, focused: bool) {
        if !focused && self.panels.overlay == Some(crate::panels::Overlay::Appearance) {
            self.panels.cancel_overlay(&mut self.workspace);
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
        if !tab.dirty()
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
            self.panels.message = format!("Automatic save failed: {error}");
        }
    }
    fn sync_palette_enabled(&mut self) {
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
    fn accept_palette_command(&mut self, name: &str, args: &Value) -> bool {
        let enabled = self.command_enabled(name, &args.to_string());
        self.panels
            .finish_palette_accept(&mut self.workspace, name, args, enabled);
        enabled
    }
    fn viewport(&self) -> std::ops::Range<usize> {
        self.view.viewport(self.workspace.editor())
    }
    fn replacement_confirmation(&mut self, w: &WindowHandle) -> Result<(), String> {
        if let Some(plan) = self.panels.replacement_plan.take() {
            if self
                .workspace
                .tabs
                .iter()
                .any(|t| t.dirty() && plan.iter().any(|c| t.path.as_ref() == Some(&c.path)))
            {
                return Err("A target has unsaved edits; save it before replacing in files".into());
            }
            let count: usize = plan.iter().map(|p| p.count).sum();
            if count > 0 && w.alert("Replace in Files?", &format!("Replace {count} matches in {} files? Each original is backed up beside the file before atomic saving.", plan.len()), &["Cancel", "Replace"]) == 1 {
                let mut backups = Vec::new();
                for change in plan { backups.push(change.commit()?); }
                self.panels.message = format!("Replaced {count} matches. {} original-file backups retained.", backups.len());
            }
        }
        Ok(())
    }
    fn scene(&mut self) -> vel_render::scene::Scene {
        self.sync_active_view();
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
        let mut visible = std::collections::BTreeMap::<
            vel_buffer::BufferId,
            std::collections::BTreeSet<vel_buffer::ViewId>,
        >::new();
        for id in self
            .workspace
            .workbench
            .groups
            .iter()
            .flat_map(|g| g.visible())
        {
            if let Some(i) = self.workspace.sheet_index(id) {
                let tab = &self.workspace.tabs[i];
                visible
                    .entry(tab.document)
                    .or_default()
                    .insert(tab.shared_view);
            }
        }
        for (document, views) in visible {
            if let Some(tab) = self.workspace.tabs.iter().find(|t| t.document == document)
                && let Some(language) = tab.editor.language.borrow_mut().as_mut()
            {
                language.retain_visible_views(&views);
            }
        }
        let bounds = self.panels.layout(self.window_size);
        self.workbench_bounds = bounds;
        self.sheet_bounds.clear();
        self.group_bounds.clear();
        self.tab_hits.clear();
        self.tab_bars.clear();
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
        let mut scene = vel_render::scene::Scene::default();
        let mut chrome = SceneLayer::default();
        let mut exact = true;
        let tabset_style = self.workspace.themes.ui.borrow_mut().style(
            0xDFFF,
            &vel_theme::ElementPath(&[
                vel_theme::Element::new("window", &[]),
                vel_theme::Element::new("tabset_control", &[]),
            ]),
            &self.workspace.editor().settings,
        );
        let show_tabs = self.workspace.window_visible("show_tabs");
        let tab_height = if show_tabs {
            f64::from(tabset_style.tab_height).max(16.)
        } else {
            0.
        };
        let tab_width = f64::from(tabset_style.tab_width).max(72.);
        let groups = self.workspace.workbench.groups.clone();
        for (group_index, group) in groups.iter().enumerate() {
            let Some(group_bounds) = self.workspace.workbench.layout.rect(group_index, bounds)
            else {
                continue;
            };
            self.group_bounds.push(group_bounds);
            let visible = group.visible();
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
                let mut other;
                let reused = *sheet == focused || self.sheet_views.contains_key(sheet);
                let view = if *sheet == focused {
                    self.content = rect;
                    &mut self.view
                } else {
                    other = self.sheet_views.remove(sheet).unwrap_or_else(|| {
                        View::new(
                            Size {
                                width: rect.width,
                                height: rect.height,
                            },
                            self.fonts.as_ref().map_or(8., |f| f.cell as f64),
                            self.fonts.as_ref().map_or(12., |f| f.ascent as f64),
                        )
                    });
                    &mut other
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
                view.tab_height = if !self.workspace.window_visible("show_tabs") {
                    0.
                } else {
                    tab_height
                };
                view.focused = *sheet == focused
                    && self.window_focused
                    && self.panels.input().is_none()
                    && !self.panels.side_focused;
                view.find_matches.clear();
                if let Some(fonts) = &self.fonts {
                    view.ligatures = fonts.has_ligatures();
                    view.set_font_metrics(
                        fonts.metrics,
                        self.workspace.themes.ui.borrow().density as f64,
                    );
                }
                if reused {
                    view.apply_style(&self.workspace.tabs[index].editor);
                } else {
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
                if *sheet == focused {
                    let viewport = view.viewport(&self.workspace.tabs[index].editor);
                    let highlights = self.panels.visible_highlights();
                    let first = highlights.partition_point(|r| r.end < viewport.start);
                    view.find_matches.extend(
                        highlights[first..]
                            .iter()
                            .take_while(|r| r.start <= viewport.end)
                            .cloned(),
                    );
                }
                theme_animating |= self.workspace.themes.ui.borrow().animating();
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
                    let view = std::mem::replace(view, View::new(Size::default(), 8., 12.));
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
                let mut theme = self.workspace.themes.ui.borrow_mut();
                let bar_path = [
                    vel_theme::Element::new("window", &[]),
                    vel_theme::Element::new("tabset_control", &[]),
                ];
                let bar_style = theme.style(
                    0xD000 + group_index as u64,
                    &vel_theme::ElementPath(&bar_path),
                    &self.workspace.editor().settings,
                );
                theme.surface(&mut chrome, &bar_style, bar, bar);
                drop(theme);
                let bar = self.paint_tab_buttons(
                    &mut chrome,
                    group_index,
                    bar,
                    group.sheets.len(),
                    tab_width,
                );
                // https://www.sublimetext.com/docs/themes.html#settings
                let tab_width = if self
                    .workspace
                    .editor()
                    .settings
                    .bool("enable_tab_scrolling", true)
                {
                    tab_width
                } else {
                    tab_width.min(bar.width / group.sheets.len().max(1) as f64)
                };
                let max_scroll = (group.sheets.len() as f64 * tab_width - bar.width).max(0.);
                self.tab_bars.push((group_index, bar, max_scroll));
                let scroll = group.tab_scroll.clamp(0., max_scroll);
                for (position, sheet) in group.sheets.iter().enumerate() {
                    let Some(index) = self.workspace.sheet_index(*sheet) else {
                        continue;
                    };
                    let tab = &self.workspace.tabs[index];
                    let rect = Rect {
                        x: bar.x + position as f64 * tab_width - scroll,
                        y: bar.y,
                        width: (tab_width - 1.).max(tab_width * 0.5),
                        height: bar.height,
                    };
                    if rect.x + rect.width <= bar.x || rect.x >= bar.x + bar.width {
                        continue;
                    }
                    let hit = Rect {
                        x: rect.x.max(bar.x),
                        width: (rect.x + rect.width).min(bar.x + bar.width) - rect.x.max(bar.x),
                        ..rect
                    };
                    self.tab_hits.push((*sheet, hit));
                    let selected = group.selected.contains(sheet);
                    let mut attrs = [""; 8];
                    let mut n = 0;
                    for (enabled, attr) in [
                        (selected, "selected"),
                        (*sheet == focused, "focused"),
                        (tab.dirty(), "dirty"),
                        (tab.transient, "transient"),
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
                    theme.surface(&mut chrome, &tab_style, rect, bar);
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
                        0xD100 + *sheet,
                        &vel_theme::ElementPath(&label_path),
                        settings,
                    );
                    if let Some(icon) = tab
                        .path
                        .as_deref()
                        .and_then(|p| self.workspace.themes.file_icon(p))
                    {
                        theme.icon(
                            &mut chrome,
                            icon,
                            Rect {
                                x: rect.x + if tab_style.close_left { 26. } else { 8. },
                                y: rect.y + 8.,
                                width: 14.,
                                height: 14.,
                            },
                            bar,
                            label.foreground,
                        );
                    }
                    let caption = if let Some((read, total)) = tab.progress() {
                        format!(
                            "{} · {}%",
                            tab.label(),
                            read.saturating_mul(100) / total.max(1)
                        )
                    } else {
                        tab.label()
                    };
                    let label_offset = if rect.width < 48. {
                        4.
                    } else if tab_style.close_left {
                        46.
                    } else {
                        28.
                    };
                    theme.text(
                        &mut chrome,
                        &label,
                        &caption,
                        rect.x + label_offset,
                        rect.y + f64::from(label.size) + 8. * self.panels.ui_scale,
                        Rect {
                            width: (rect.width - if rect.width < 48. { 2. } else { 30. })
                                .max(0.)
                                .min(bar.x + bar.width - rect.x),
                            ..rect
                        },
                    );
                    let close_path = [
                        path[0],
                        path[1],
                        path[2],
                        vel_theme::Element::new("tab_close_button", &[]),
                    ];
                    let close = theme.style(
                        0xD200 + *sheet,
                        &vel_theme::ElementPath(&close_path),
                        settings,
                    );
                    let close_rect = Rect {
                        x: if tab_style.close_left {
                            rect.x + 6.
                        } else {
                            rect.x + rect.width - 20.
                        },
                        y: rect.y + (rect.height - 12.) / 2.,
                        width: 12.,
                        height: 12.,
                    };
                    // Narrow fitted tabs keep their entire target for selecting the tab.
                    if rect.width >= 28. {
                        self.tab_close_hits.push((*sheet, close_rect));
                        theme.surface(&mut chrome, &close, close_rect, hit);
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
                    if *sheet == focused {
                        node.attributes.insert("focused");
                    }
                    if tab.dirty() {
                        node.attributes.insert("dirty");
                    }
                    if tab.transient {
                        node.attributes.insert("transient");
                    }
                    self.panels.tree.roots.insert(0, id);
                    if selected && group.selected.len() > 1 {
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
                            0xD300 + *sheet,
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
        for (_, divider) in self.workspace.workbench.layout.dividers(bounds, 2.) {
            vel_ui::fill(&mut chrome, divider, bounds, [0.28, 0.3, 0.35, 1.]);
        }
        let _ = self.workspace.prepare_sheet(active);
        self.sync_workspace_after_panels();
        self.exact_scene = exact;
        scene.layers.push(chrome);
        self.panels.paint(&self.workspace, &mut scene);
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
        let settings = &self.workspace.editor().settings;
        if !settings.bool("themed_title_bar", true) {
            w.set_title_bar_theme(None, None);
            return;
        }
        let style = self.workspace.themes.ui.borrow_mut().style(
            0xDFFE,
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
        w.set_title_bar_theme(appearance, style.background);
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
    fn changed_state(&mut self, received: Instant, reveal: bool) {
        let reveal = self.panels.should_reveal(reveal);
        if reveal {
            self.view.reset_caret();
        }
        self.sync_workspace_after_panels();
        if self.options.mode == Mode::Interactive {
            self.session_due = Some(Instant::now() + Duration::from_millis(300));
        }
        self.sync_active_view();
        self.input_at.get_or_insert(received);
        if reveal {
            self.view.reveal(self.workspace.editor());
        }
    }
    fn input(&mut self, w: &WindowHandle, input: TextInput, received: Instant) {
        self.log_input("text", &input);
        self.workspace.editor_mut().text_input(input);
        self.changed(w, received, true);
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
        self.reporter.emit(json!({"kind":"startup","milestones":self.clock.milestones,"before_main_ms":self.clock.before_main_ms,"process_birth_error":self.clock.birth_error,"process_birth_method":"proc_pidinfo(PROC_PIDTBSDINFO) anchored at main wall snapshot; monotonic deltas thereafter","actual_physical_size":renderer.physical_size(),"logical_size":[w.content_size().width,w.content_size().height],"scale_factor":w.scale_factor(),"maximum_frames_per_second":w.maximum_frames_per_second(),"present_mode":"fifo","scanout_main_ms":ms(p.scanout.saturating_duration_since(self.clock.entered)),"scanout_process_ms":self.clock.before_main_ms.map(|n|n+ms(p.scanout.saturating_duration_since(self.clock.entered))),"presented_time_seconds":p.presented_time,"presented_callback_main_ms":ms(p.callback.saturating_duration_since(self.clock.entered)),"scanout_to_callback_ms":ms(p.callback.saturating_duration_since(p.scanout)),"drawable":p.drawable,"startup_path":"offscreen text frame submitted before orderFront; acquire/copy/present immediately in the same applicationDidFinishLaunching callback","surface_retries":self.retries,"skipped_drawables":self.first_scanout.skipped_drawables,"first_callback_main_ms":self.first_scanout.first_callback.map(|t|ms(t.saturating_duration_since(self.clock.entered))),"first_real_scanout":p.presented_time,"boundary":"MTLDrawable.addPresentedHandler + presentedTime; queue.present return reported separately"}))?;
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
            self.reporter.emit(json!({"kind":"input_latency","receipt_to_submit_ms":ms(times.submitted.saturating_duration_since(at)),"receipt_to_present_return_ms":ms(times.presented.saturating_duration_since(at))}))?;
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
    fn on_platform_phase(&mut self, phase: &'static str) {
        self.clock.mark(phase);
    }
    fn on_start(&mut self, w: &WindowHandle) -> Result<(), String> {
        self.panels.review.native_context = true;
        w.set_title(&self.workspace.window_title());
        w.set_menu(&self.application_menu());
        self.reporter.emit(json!({"kind":"resources","key_bindings":self.workspace.editor().keymap.len(),"mouse_bindings":self.workspace.mousemap.len(),"palette_entries":self.workspace.palette.len()}))?;
        let fonts = Fonts::load_named(&self.workspace.editor().settings.editor().font_face)?;
        self.clock.mark("primary_font_ready");
        self.view = View::new(w.content_size(), fonts.cell as f64, fonts.ascent as f64);
        self.view.set_font_metrics(fonts.metrics, w.scale_factor());
        self.window_size = w.content_size();
        self.workspace.themes.ui.borrow_mut().density = w.scale_factor() as f32;
        self.view.appearance = w.appearance();
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
        self.renderer = Some(Renderer::new(
            w.clone(),
            self.options.mode == Mode::Scroll,
            &cache_dir,
            |phase| self.clock.mark(phase),
        )?);
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
        self.accessible_tree()
    }
    fn on_accessibility(
        &mut self,
        w: &WindowHandle,
        id: accessibility::Id,
        request: accessibility::Request,
    ) {
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
        if self
            .panels
            .input()
            .is_some_and(vel_ui::input::Input::composing)
        {
            return false;
        }
        if let Some(id) = self.accessibility_focus {
            let tree = self.accessible_tree();
            if let Some(node) = tree.node(id) {
                if node.text.is_none() && matches!(key.name.as_str(), "enter" | "space") {
                    self.accessibility_action(w, id, accessibility::Request::Press);
                    return true;
                }
            } else {
                self.accessibility_focus = None;
            }
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
    fn on_named_command(&mut self, w: &WindowHandle, name: &str, args: &str) {
        if !self.options.mode.automatic() {
            match serde_json::from_str(args) {
                Ok(args) => self.run_named(Some(w), name, &args),
                Err(e) => self.show_error(w, &e.to_string()),
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
                self.show_error(w, &error);
                return;
            }
        };
        if crate::open_requests::needs_new_window(
            &request,
            self.workspace
                .global_bool("open_files_in_new_window", false),
        ) {
            if let Err(error) = self.open_native_window_request(w, request) {
                self.show_error(w, &error);
            }
            return;
        }
        if let Err(error) =
            crate::open_requests::open_paths(&mut self.workspace, &mut self.panels, paths)
        {
            self.panels.message = error;
        }
        self.changed(w, Instant::now(), false);
        w.focus();
    }
    fn on_reopen(&mut self, w: &WindowHandle) {
        if self.options.mode == Mode::Interactive
            && let Err(error) = self.open_native_window(w, None, None)
        {
            self.show_error(w, &error);
        }
    }
    fn on_idle(&mut self, w: &WindowHandle) {
        if self.options.mode != Mode::Interactive {
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
                self.show_error(w, &error);
            }
        }
    }
    fn command_enabled(&self, name: &str, args: &str) -> bool {
        let Ok(args) = serde_json::from_str::<Value>(args) else {
            return false;
        };
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
        if let Some(state) = self.context_state(name, &args) {
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
            "reopen_last_file" if self.workspace.closed_files.is_empty() => return false,
            "reopen_encoding" if self.workspace.tabs[target].path.is_none() => return false,
            "close" | "close_file" | "prompt_save_as" | "save_as" | "save" | "save_and_close"
            | "save_encoding"
                if self.workspace.is_empty() =>
            {
                return false;
            }
            "save_all" => return self.workspace.tabs.iter().any(|tab| tab.dirty()),
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
            "next_result" | "prev_result"
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
            _ => {}
        }
        if args["_context_menu"].is_null()
            && self.panels.input().is_some()
            && !self.panels.palette_accept_pending(name, &args)
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
            .is_enabled(name, &args, self.workspace.editor())
            && !((tab.loading.is_some() || tab.binary_loading.is_some())
                && self.workspace.editor().registry.kind(name)
                    == Some(vel_commands::Kind::TextCommand)
                && !matches!(
                    name,
                    "move" | "move_to" | "select_all" | "copy" | "single_selection"
                ))
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
            "set_build_system" => {
                self.workspace
                    .build_system
                    .as_deref()
                    .unwrap_or("Automatic")
                    == args["name"].as_str().unwrap_or("Automatic")
            }
            "toggle_setting" => args["setting"]
                .as_str()
                .is_some_and(|key| editor.settings.bool(key, false)),
            "toggle_side_bar" => self.panels.side_visible,
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
    fn on_text_input(&mut self, w: &WindowHandle, event: TextInput) {
        self.cancel_context(w);
        if self.options.mode.automatic() {
            self.external_input += 1;
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
            self.changed(w, Instant::now(), moved);
            w.request_frame_at(Instant::now() + Duration::from_millis(8));
            return;
        }
        if self.workspace.editor().read_only {
            return;
        }
        self.input(w, event, Instant::now());
    }
    fn on_command(&mut self, w: &WindowHandle, command: Command) {
        self.cancel_context(w);
        self.log_input("command", command);
        if self.options.mode.automatic() {
            self.external_input += 1;
            return;
        }
        if let Some(input) = self.panels.input_mut() {
            input.set_clipboard(w.clipboard_text().unwrap_or_default());
        }
        let viewport = self.viewport();
        match self.panels.command(&mut self.workspace, command, viewport) {
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
                self.show_error(w, &error);
                return;
            }
        }
        if command == Command::Cancel && self.workspace.editor().marked.is_some() {
            self.workspace.editor_mut().cancel_composition();
            self.changed(w, Instant::now(), true);
            return;
        }
        let (name, args) = crate::editor::selector_command(command);
        self.run_named(Some(w), name, &args);
    }
    fn on_mouse(&mut self, w: &WindowHandle, event: MouseEvent) {
        self.handle_mouse_event(Some(w), event);
    }
    // Native and headless events share UI hit testing, focus, binding lookup and gestures.
    fn on_scroll(&mut self, w: &WindowHandle, event: ScrollEvent) {
        self.handle_scroll_event(Some(w), event);
    }
    fn on_resize(&mut self, w: &WindowHandle, size: Size, scale: f64) {
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
        self.view.appearance = appearance;
        for view in self.sheet_views.values_mut() {
            view.appearance = appearance;
        }
        if let Err(e) = self
            .workspace
            .set_appearance(appearance == Appearance::Dark)
        {
            self.show_error(w, &e);
        }
        w.request_frame();
    }
    fn on_frame(&mut self, ctx: &mut FrameContext<'_>) -> Result<(), String> {
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
            self.save_on_focus_lost(None);
        }
        self.set_window_focused(focused);
        if focused && self.options.mode == Mode::Interactive {
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
            let mut changed = self.workspace.poll() || focus_changed;
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
            if changed && self.options.mode == Mode::Interactive && self.session_due.is_none() {
                self.session_due = Some(Instant::now() + Duration::from_millis(300));
            }
            if self.options.mode == Mode::Interactive {
                self.poll_workbench(w)?;
            }
            if focus_changed {
                w.set_menu(&self.application_menu());
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
            if let Err(error) = self.replacement_confirmation(w) {
                self.show_error(w, &error);
            }
            if self.releases.busy()
                || self.workspace.resources_busy()
                || self.panels.busy()
                || self.workspace.editor().annotations.borrow().pending > 0
                || self.workspace.editor().completion.busy()
                || self.builds.task.is_some()
                || self.workspace.themes.ui.borrow().metrics.images_pending()
            {
                w.request_frame_at(Instant::now() + Duration::from_millis(8));
            }
            if active_was_loading && self.workspace.tabs[self.workspace.active].loading.is_none() {
                self.view.reveal(self.workspace.editor());
            }
            if changed {
                w.set_menu(&self.application_menu());
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
                let reload = w.alert(
                    "File changed on disk",
                    &format!(
                        "{} has unsaved edits. Reloading discards them.",
                        self.workspace.tabs[index].label()
                    ),
                    &["Keep Edits", "Reload"],
                ) == 1;
                self.workspace.reload_external(index, reload);
            }
            for message in std::mem::take(&mut self.workspace.messages) {
                self.show_error(w, &message);
            }
            let caret_animation = self
                .workspace
                .editor()
                .settings
                .string("caret_style", "solid")
                != "solid";
            w.request_frame_at(
                Instant::now()
                    + Duration::from_millis(
                        if caret_animation
                            || self.view.scrolling()
                            || self.workspace.themes.ui.borrow().animating()
                        {
                            16
                        } else if self.workspace.language_busy()
                            || self.panels.busy()
                            || self.builds.task.is_some()
                        {
                            8
                        } else {
                            200
                        },
                    ),
            );
            if ctx.reason == FrameReason::Timer
                && !changed
                && !loading_before
                && !caret_animation
                && !self.view.scrolling()
                && !self.workspace.themes.ui.borrow().animating()
            {
                return Ok(());
            }
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
                Err(error) => self.show_error(w, &error),
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
            let scene = self.scene();
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
            let status = w.display_status();
            // AppKit can report occluded briefly immediately after orderFront.
            if status.display_asleep
                || status.screen_locked
                || waiting_since.elapsed() >= Duration::from_millis(1000)
            {
                status.check()?;
            }
            // orderFront may constrain an oversized window or change its backing
            // scale. Rebuild immediately in this same callback if that invalidated
            // the prepared texture; never copy stale-sized pixels or await redraw.
            if !self
                .renderer
                .as_ref()
                .ok_or("renderer missing")?
                .has_prepared_frame()
            {
                let scene = self.scene();
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
                let scene = self.scene();
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
                    let text = match b.completed % 8 {
                        0 => "你",
                        1 => "🦀",
                        2 => "é",
                        3 => "\n",
                        _ => "x",
                    };
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
                    let max = (self.workspace.editor().text.len_lines() as f64
                        * self.view.line_height
                        - self.view.size.height
                        + 8.)
                        .max(0.);
                    if self.view.scroll_y + 20. >= max {
                        self.view.scroll_y = 0.;
                    }
                    self.view.scroll(self.workspace.editor(), 0., 20.);
                }
            }
            if let Some(b) = self.bench.as_ref()
                && b.mode == Mode::Scroll
                && b.warmup == 0
            {
                query = Some(b.completed as u32);
            }
        }
        let scene = self.scene();
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
        self.workspace.fullscreen = w.is_full_screen();
        if self.options.mode != Mode::Interactive {
            let _=self.reporter.emit(json!({"kind":"error","stage":"user_close","message":"automatic benchmark closed before completion","complete":false}));
            return true;
        }
        for index in 0..self.workspace.tabs.len() {
            let _ = self.workspace.prepare_sheet(index);
            self.workspace.tabs[index].editor.unmark();
        }
        let _ = self.workspace.prepare_sheet(self.workspace.active);
        if self.hot_exit().retain(true) {
            if let Err(error) = self.save_workspace_checkpoint() {
                if self
                    .workspace
                    .tabs
                    .iter()
                    .any(|t| t.dirty() || (t.path.is_none() && t.editor.text.len_bytes() > 0))
                {
                    self.show_error(w,&format!("Could not save Hot Exit session: {error}. Save documents or repair the data directory before quitting."));
                    return false;
                }
                self.panels.message = error;
            }
        } else {
            if !self.can_close_many(w, &(0..self.workspace.tabs.len()).collect::<Vec<_>>()) {
                return false;
            }
            if let Err(error) = self.save_workspace_checkpoint() {
                self.panels.message = error;
            }
        }
        for waiter in &mut self.waiters {
            waiter.finish(&mut self.workspace);
        }
        true
    }
    fn on_window_close_requested(&mut self, w: &WindowHandle) -> bool {
        self.workspace.fullscreen = w.is_full_screen();
        if self.options.mode != Mode::Interactive {
            return self.on_quit_requested(w);
        }
        let retain = self.hot_exit().retain(false);
        if retain {
            if let Err(error) = self
                .checkpoint_reason(false, true)
                .and_then(|()| self.sessions.borrow().flush())
            {
                if self
                    .workspace
                    .tabs
                    .iter()
                    .any(|t| t.dirty() || (t.path.is_none() && t.editor.text.len_bytes() > 0))
                {
                    self.show_error(w, &format!("Could not save closed window: {error}"));
                    return false;
                }
                self.panels.message = error;
            }
        } else {
            if !self.can_close_many(w, &(0..self.workspace.tabs.len()).collect::<Vec<_>>()) {
                return false;
            }
            let _ = self.checkpoint_reason(false, true);
        }
        self.sessions.borrow_mut().close(self.session_id, retain);
        // Closing a window closes its sheets even when Hot Exit retains their buffers.
        // The retained Dock template must not keep CLI -w clients blocked.
        for waiter in &mut self.waiters {
            waiter.finish(&mut self.workspace);
        }
        self.waiters.clear();
        let _ = self.sessions.borrow().flush();
        true
    }
    fn text_input_state(&self) -> TextInputState {
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
        if let Some(input) = self.panels.input() {
            return input.range_rect(range);
        }
        let (mut rect, range) = self.view.range_rect(self.workspace.editor(), range);
        rect.x += self.content.x;
        rect.y += self.content.y;
        (rect, range)
    }
    fn character_index_for_point(&self, point: Point) -> Option<usize> {
        if self.panels.input().is_none() && self.workspace.editor().binary.is_some() {
            return None;
        }
        if let Some(input) = self.panels.input() {
            return Some(input.utf16_at_x(point.x));
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
    items: &[vel_config::ui::MenuItem],
    macros: &[vel_config::ui::PaletteItem],
) -> Vec<MenuEntry> {
    items
        .iter()
        .filter(|item| {
            item.platform.as_deref() != Some("!OSX")
                && item.platform.as_deref() != Some("Windows")
                && item.platform.as_deref() != Some("Linux")
        })
        .map(|item| MenuEntry {
            caption: item.caption.clone(),
            command: item.command.clone(),
            args: item.args.to_string(),
            children: if item.id == "macros" {
                macros
                    .iter()
                    .map(|row| MenuEntry {
                        caption: row.caption.clone(),
                        command: Some(row.invocation.command.clone()),
                        args: row.invocation.args.to_string(),
                        children: Vec::new(),
                    })
                    .collect()
            } else {
                native_menu(&item.children, macros)
            },
        })
        .collect()
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
            path: "Untitled 1".into(),
            number: 1,
            text: "cat".into(),
            highlights: std::iter::once(0..3).collect(),
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
        app.accept_request(crate::instance::Incoming { request, stream })
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
        let (stream, _client) = UnixStream::pair().unwrap();
        app.accept_request(crate::instance::Incoming { request, stream })
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
