//! Native application handler and buffered benchmark observations.
use crate::app_scene_translate as translate_scene;
#[path = "app_a11y.rs"]
mod accessibility_adapter;
#[path = "app_context.rs"]
mod context_adapter;
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
    tab_close_hits: Vec<(u64, Rect)>,
    dragging_splitter: Option<vel_ui::workbench::Splitter>,
    /// Text selection gesture between a press in the text area and its release.
    drag_select: Option<crate::mouse_select::DragSelect>,
    /// Selected text being dragged to a new position.
    drag_text: Option<crate::mouse_select::DragText>,
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
            tab_close_hits: Vec::new(),
            dragging_splitter: None,
            drag_select: None,
            drag_text: None,
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
            w.set_menu(&native_menu(&self.workspace.menus));
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
    fn run_named(&mut self, w: &WindowHandle, name: &str, args: &Value) {
        self.run_named_depth(w, name, args, 0);
    }
    fn run_named_depth(&mut self, w: &WindowHandle, name: &str, args: &Value, depth: usize) {
        if depth >= 32 {
            self.show_error(w, "command chain exceeds 32 levels");
            return;
        }
        if name == "chain" {
            if let Some(commands) = args["commands"].as_array() {
                for command in commands {
                    if let Some(name) = command[0].as_str() {
                        self.run_named_depth(
                            w,
                            name,
                            &context_child_args(args, command.get(1).unwrap_or(&json!({}))),
                            depth + 1,
                        );
                    } else {
                        self.show_error(w, "chain entry needs [name, args]");
                        return;
                    }
                }
            } else {
                self.show_error(w, "chain needs commands");
            }
            return;
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
        if !self.command_enabled(name, &args.to_string()) {
            return;
        }
        match self.context_action(w, name, args) {
            Ok(true) => {
                self.changed(w, Instant::now(), false);
                return;
            }
            Ok(false) => {}
            Err(error) => {
                self.panels.message = error;
                self.changed(w, Instant::now(), false);
                return;
            }
        }
        if self.workspace.editor().registry.kind(name) == Some(vel_commands::Kind::TextCommand)
            && !matches!(
                name,
                "toggle_comment"
                    | "reindent"
                    | "fold"
                    | "unfold"
                    | "fold_by_level"
                    | "unfold_all"
                    | "fold_tag_attributes"
            )
            && !crate::html_ui::COMMANDS.contains(&name)
            && let Some(input) = self.panels.input_mut()
        {
            input.set_clipboard(w.clipboard_text().unwrap_or_default());
            if let Err(error) = input.run(name, args) {
                self.show_error(w, &error);
            }
            if let Some(input) = self.panels.input() {
                w.set_clipboard_text(input.clipboard());
            }
            let viewport = self.viewport();
            self.panels.edited(&mut self.workspace, viewport);
            self.changed(w, Instant::now(), false);
            return;
        }
        self.workspace.editor_mut().clipboard = w.clipboard_text().unwrap_or_default();
        let before = self.workspace.editor().clipboard.clone();
        if let Err(error) = self.workspace.editor_mut().run(name, args) {
            self.show_error(w, &error);
        }
        if self.workspace.editor().clipboard != before {
            w.set_clipboard_text(&self.workspace.editor().clipboard);
        }
        self.handle_actions(w);
        self.changed(w, Instant::now(), name != "scroll_lines");
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
        self.workspace.sync_shared();
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
    fn handle_actions(&mut self, w: &WindowHandle) {
        let actions = std::mem::take(&mut self.workspace.editor_mut().actions);
        for (name, args) in actions {
            let previous = self.workspace.active;
            self.workspace.tabs[previous].scroll = (self.view.scroll_x, self.view.scroll_y);
            let result = self.apply_action(w, &name, &args);
            if let Err(error) = result {
                self.show_error(w, &error);
            }
            self.sync_active_view();
        }
        self.workspace.editor_mut().sync();
        w.set_title(&self.workspace.window_title());
    }
    fn apply_action(&mut self, w: &WindowHandle, name: &str, args: &Value) -> Result<(), String> {
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
                self.panels.chrome = !self.workspace.distraction_free;
                if self.workspace.distraction_free {
                    self.normal_sidebar = self.panels.side_visible;
                    self.fullscreen_before_distraction = self.workspace.fullscreen;
                    self.panels.side_visible = self.distraction_sidebar.unwrap_or(false);
                    if !self.workspace.fullscreen {
                        self.workspace.fullscreen = true;
                        w.toggle_full_screen();
                    }
                } else {
                    self.distraction_sidebar = Some(self.panels.side_visible);
                    self.panels.side_visible = self.normal_sidebar;
                    if !self.fullscreen_before_distraction && self.workspace.fullscreen {
                        self.workspace.fullscreen = false;
                        w.toggle_full_screen();
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
                w.toggle_full_screen();
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
            "show_build_systems" => {
                let items = crate::build::Builds::choices(&self.workspace, false);
                self.panels.choices(&mut self.workspace, items);
                return Ok(());
            }
            "build" | "exec" => {
                if args["cancel"].as_bool() == Some(true) || args["kill"].as_bool() == Some(true) {
                    if let Some(task) = &self.builds.task {
                        task.cancel();
                    }
                } else if args["select"].as_bool() == Some(true) {
                    let items = crate::build::Builds::choices(&self.workspace, true);
                    self.panels.choices(&mut self.workspace, items);
                } else {
                    self.builds.start(&self.workspace, &mut self.panels, args)?;
                }
                return Ok(());
            }
            "cancel_build" => {
                if let Some(task) = &self.builds.task {
                    task.cancel();
                }
                return Ok(());
            }
            "next_result" | "prev_result" => {
                self.builds
                    .navigate(&mut self.workspace, name == "next_result")?;
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
            "close" | "close_file" => {
                if args["force"].as_bool().unwrap_or(false) || self.can_close(w, active) {
                    let last = self.workspace.tabs.len() == 1;
                    self.workspace.close(active)?;
                    if last
                        && self
                            .workspace
                            .editor()
                            .settings
                            .bool("close_windows_when_empty", false)
                    {
                        w.request_close();
                    }
                }
            }
            "next_view" | "prev_view" => {
                let group = &self.workspace.workbench.groups[self.workspace.workbench.active];
                if !group.sheets.is_empty() {
                    let current = group
                        .sheets
                        .iter()
                        .position(|s| *s == self.workspace.tabs[active].sheet)
                        .unwrap_or(0);
                    let next = (current as isize + if name == "next_view" { 1 } else { -1 })
                        .rem_euclid(group.sheets.len() as isize)
                        as usize;
                    if let Some(index) = self.workspace.sheet_index(group.sheets[next]) {
                        self.workspace.select(index);
                    }
                }
            }
            "select_by_index" => {
                let group = &self.workspace.workbench.groups[self.workspace.workbench.active];
                if let Some(sheet) = args["index"]
                    .as_u64()
                    .and_then(|i| group.sheets.get(i as usize))
                    && let Some(index) = self.workspace.sheet_index(*sheet)
                {
                    self.workspace.select(index);
                }
            }
            "new_window" => {
                self.open_native_window(w, None, None)?;
            }
            "close_window" => {
                w.request_close();
            }
            "increase_font_size" | "decrease_font_size" | "reset_font_size" => {
                let size = self.workspace.editor().settings.editor().font_size;
                let size = if name == "reset_font_size" {
                    13.
                } else {
                    (size
                        + if name == "increase_font_size" {
                            1.
                        } else {
                            -1.
                        })
                    .clamp(6., 96.)
                };
                self.workspace
                    .editor_mut()
                    .settings
                    .set(Layer::View, "font_size", json!(size));
            }
            "toggle_setting" => {
                let key = args["setting"]
                    .as_str()
                    .ok_or("toggle_setting needs setting")?;
                let value = self.workspace.editor().settings.bool(key, false);
                self.workspace
                    .editor_mut()
                    .settings
                    .set(Layer::View, key, json!(!value));
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
            "scroll_lines" => {
                let amount = args["amount"].as_f64().unwrap_or(1.);
                self.view
                    .scroll(self.workspace.editor(), 0., -amount * self.view.line_height);
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
    fn sync_active_view(&mut self) {
        let current = self.workspace.tabs[self.workspace.active].sheet;
        if self.view_buffer != Some(current) {
            if let Some(old) = self.view_buffer {
                if let Some(index) = self.workspace.sheet_index(old) {
                    self.workspace.tabs[index].scroll = (self.view.scroll_x, self.view.scroll_y);
                }
                let replacement = View::new(
                    self.view.size,
                    self.fonts.as_ref().map_or(8., |f| f.cell as f64),
                    self.fonts.as_ref().map_or(12., |f| f.ascent as f64),
                );
                self.sheet_views
                    .insert(old, std::mem::replace(&mut self.view, replacement));
            }
            if let Some(view) = self.sheet_views.remove(&current) {
                self.view = view;
            }
            let scroll = self.workspace.tabs[self.workspace.active].scroll;
            self.view.scroll_x = scroll.0;
            self.view.scroll_y = scroll.1;
            self.view_buffer = Some(current);
            if let Some(bounds) = self.sheet_bounds.get(&current) {
                self.content = *bounds;
                self.view.size = Size {
                    width: bounds.width,
                    height: bounds.height,
                };
            }
        }
        if let Some(scroll) = self.panels.restore_scroll.take() {
            self.view.scroll_x = scroll.0;
            self.view.scroll_y = scroll.1;
        }
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
            self.content = Rect {
                x: 0.,
                y: 0.,
                width: self.window_size.width,
                height: self.window_size.height,
            };
            self.view.size = self.window_size;
            if self.workspace.take_pending_center() {
                self.view.center_caret(self.workspace.editor());
            }
            let scene = self.view.scene(self.workspace.editor());
            self.exact_scene = self.view.exact(self.workspace.editor())
                && self.workspace.tabs[self.workspace.active].loading.is_none();
            return scene;
        }
        self.workspace.sync_shared();
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
        let tab_height = f64::from(tabset_style.tab_height).max(16.);
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
                    other.scroll_x = self.workspace.tabs[index].scroll.0;
                    other.scroll_y = self.workspace.tabs[index].scroll.1;
                    &mut other
                };
                view.size = Size {
                    width: rect.width,
                    height: rect.height,
                };
                view.appearance = if self.workspace.dark {
                    Appearance::Dark
                } else {
                    Appearance::Light
                };
                view.tab_height = if self.workspace.distraction_free {
                    0.
                } else {
                    tab_height
                };
                view.focused = *sheet == focused
                    && self.window_focused
                    && self.panels.input().is_none()
                    && !self.panels.side_focused;
                view.find_matches.clear();
                view.apply_style(&self.workspace.tabs[index].editor);
                if std::mem::take(&mut self.workspace.tabs[index].pending_center) {
                    view.center_caret(&self.workspace.tabs[index].editor);
                    self.workspace.tabs[index].scroll = (view.scroll_x, view.scroll_y);
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
                    let view = std::mem::replace(view, View::new(Size::default(), 8., 12.));
                    self.sheet_views.insert(*sheet, view);
                }
                translate_scene(&mut part, rect.x, rect.y);
                scene.layers.extend(part.layers);
            }
            if !self.workspace.distraction_free {
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
                        width: tab_width - 1.,
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
                        vel_theme::Element::new("tab_label", &[]),
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
                    theme.text(
                        &mut chrome,
                        &label,
                        &caption,
                        rect.x + if tab_style.close_left { 46. } else { 28. },
                        rect.y + f64::from(label.size) + 8. * self.panels.ui_scale,
                        Rect {
                            width: (rect.width - 30.).min(bar.x + bar.width - rect.x),
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
                    self.tab_close_hits.push((*sheet, close_rect));
                    theme.surface(&mut chrome, &close, close_rect, bar);
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
        self.workspace.sync_shared();
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
        if reveal {
            self.view.reset_caret();
        }
        self.workspace.sync_shared();
        if self.options.mode == Mode::Interactive {
            self.session_due = Some(Instant::now() + Duration::from_millis(300));
        }
        self.sync_active_view();
        self.input_at.get_or_insert(received);
        if reveal {
            self.view.reveal(self.workspace.editor());
        }
        w.invalidate_character_coordinates();
        w.request_frame();
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
        w.set_menu(&native_menu(&self.workspace.menus));
        self.reporter.emit(json!({"kind":"resources","key_bindings":self.workspace.editor().keymap.len(),"mouse_bindings":self.workspace.mousemap.len(),"palette_entries":self.workspace.palette.len()}))?;
        let fonts = Fonts::load_named(&self.workspace.editor().settings.editor().font_face)?;
        self.clock.mark("primary_font_ready");
        self.view = View::new(w.content_size(), fonts.cell as f64, fonts.ascent as f64);
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
        if self.options.mode == Mode::Interactive
            && (self.workspace.fullscreen || self.workspace.distraction_free)
        {
            w.toggle_full_screen();
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
        let resolution = match self.panels.resolve_key(&mut self.workspace, &key, viewport) {
            Ok(resolution) => resolution,
            Err(error) => {
                self.show_error(w, &error);
                return true;
            }
        };
        match resolution {
            vel_config::keymap::Resolution::Unbound => false,
            vel_config::keymap::Resolution::Pending => {
                self.changed(w, event.received, true);
                true
            }
            vel_config::keymap::Resolution::Command(invocation) => {
                self.run_named(w, &invocation.command, &invocation.args);
                self.input_at = Some(event.received);
                true
            }
        }
    }
    fn on_named_command(&mut self, w: &WindowHandle, name: &str, args: &str) {
        if !self.options.mode.automatic() {
            match serde_json::from_str(args) {
                Ok(args) => self.run_named(w, name, &args),
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
            "next_result" | "prev_result" if self.builds.diagnostics.is_empty() => return false,
            "edit_project" | "close_project" if self.workspace.project_path().is_none() => {
                return false;
            }
            _ => {}
        }
        if args["_context_menu"].is_null()
            && self.panels.input().is_some()
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
            "toggle_setting" => args["setting"]
                .as_str()
                .is_some_and(|key| editor.settings.bool(key, false)),
            "toggle_side_bar" => self.panels.side_visible,
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
        if self
            .panels
            .text_input(&mut self.workspace, event.clone(), viewport)
        {
            self.changed(w, Instant::now(), true);
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
                    self.run_named(w, &invocation.command, &invocation.args);
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
        self.run_named(w, name, &args);
    }
    fn on_mouse(&mut self, w: &WindowHandle, event: MouseEvent) {
        if event.kind == MouseKind::Down {
            self.accessibility_focus = None;
        }
        if self.options.mode.automatic() {
            self.external_input += 1;
            return;
        }
        if event.is_context_click() {
            self.begin_context_menu(w, event.position);
            return;
        }
        if event.kind == MouseKind::Down {
            self.pending_menu = None;
            self.panels.cancel_review_menu();
        }
        let local = Point {
            x: event.position.x - self.content.x,
            y: event.position.y - self.content.y,
        };
        if event.kind == MouseKind::Move {
            self.workspace.editor().html.borrow_mut().mouse_move(local);
        }
        if event.kind == MouseKind::Down {
            let target = self.workspace.editor().html.borrow().navigate(local);
            if let Some(popup) = &mut self.workspace.editor().html.borrow_mut().popup {
                popup.mouse_action();
            }
            if let Some(target) = target {
                let action = crate::html_ui::invocation(target);
                self.run_named(w, &action.command, &action.args);
                self.changed(w, Instant::now(), false);
                return;
            }
        }

        if event.kind == MouseKind::Down {
            self.dragging_splitter =
                self.workspace
                    .workbench
                    .layout
                    .splitter(event.position, self.workbench_bounds, 4.);
            if self.dragging_splitter.is_some() {
                return;
            }
        }
        if let Some(splitter) = self.dragging_splitter {
            if event.kind == MouseKind::Drag {
                let bounds = self.workbench_bounds;
                let position = if splitter.vertical {
                    (event.position.x - bounds.x) / bounds.width
                } else {
                    (event.position.y - bounds.y) / bounds.height
                };
                self.workspace.workbench.layout.drag(splitter, position);
                self.changed(w, Instant::now(), false);
                return;
            }
            if event.kind == MouseKind::Up {
                self.dragging_splitter = None;
                return;
            }
        }
        if event.kind == MouseKind::Down {
            let point = Point {
                x: event.position.x - self.content.x,
                y: event.position.y - self.content.y,
            };
            if self.workspace.editor_mut().completion.hit(point) {
                let _ = self.workspace.editor_mut().commit_completion();
                self.changed(w, Instant::now(), true);
                return;
            }
            if self
                .panels
                .click_definition(&mut self.workspace, event.position)
                .unwrap_or(false)
            {
                self.sync_active_view();
                self.changed(w, Instant::now(), true);
                return;
            }
        }
        if event.kind == MouseKind::Move {
            self.panels.review_hover(&self.workspace, event.position);
            let byte = self.view.hit_test(
                self.workspace.editor(),
                Point {
                    x: event.position.x - self.content.x,
                    y: event.position.y - self.content.y,
                },
            );
            self.panels.hover_at(&self.workspace, event.position, byte);
            let _ = self.view.pointer(
                self.workspace.editor_mut(),
                Point {
                    x: event.position.x - self.content.x,
                    y: event.position.y - self.content.y,
                },
                false,
                false,
                false,
            );
            self.panels.tree.pointer(event.position, None);
            w.request_frame();
            return;
        }
        let viewport = self.viewport();
        self.panels.primary_click = event.modifiers.command();
        match self.panels.mouse(
            &mut self.workspace,
            event.position,
            event.kind == MouseKind::Down,
            event.click_count >= 2,
            event.modifiers.shift(),
            viewport,
        ) {
            Ok(true) => {
                if !self.panels.review.items.is_empty() {
                    // The Git status control uses the same native tracking path.
                    let items = std::mem::take(&mut self.panels.review.items);
                    let entries: Vec<_> = items
                        .into_iter()
                        .map(|item| MenuEntry {
                            caption: item.caption,
                            command: Some(item.invocation.command),
                            args: item.invocation.args.to_string(),
                            children: vec![],
                        })
                        .collect();
                    w.show_context_menu(&entries);
                }
                if let Some(invocation) = self.panels.pending_command.take() {
                    self.run_named(w, &invocation.command, &invocation.args);
                }
                if let Err(error) = self.replacement_confirmation(w) {
                    self.show_error(w, &error);
                }
                self.changed(w, Instant::now(), true);
                return;
            }
            Ok(false) => {}
            Err(error) => {
                self.show_error(w, &error);
                return;
            }
        }
        if event.kind == MouseKind::Down
            && let Some((sheet, _)) = self
                .tab_hits
                .iter()
                .find(|(_, rect)| vel_ui::contains(*rect, event.position))
                .copied()
        {
            if let Some(index) = self.workspace.sheet_index(sheet) {
                if event.button == 2 {
                    // Middle click closes the tab under the pointer, like Sublime.
                    if self.can_close_many(w, &[index]) {
                        let _ = self.workspace.close(index);
                    }
                } else if self
                    .tab_close_hits
                    .iter()
                    .any(|(id, rect)| *id == sheet && vel_ui::contains(*rect, event.position))
                {
                    let mut indices = if self.workspace.workbench.group_of(sheet).is_some_and(|g| {
                        self.workspace.workbench.groups[g].selected.contains(&sheet)
                    }) {
                        let group = self.workspace.workbench.group_of(sheet).expect("group");
                        self.workspace.workbench.groups[group]
                            .visible()
                            .iter()
                            .filter_map(|s| self.workspace.sheet_index(*s))
                            .collect::<Vec<_>>()
                    } else {
                        vec![index]
                    };
                    if self.can_close_many(w, &indices) {
                        indices.sort_unstable();
                        for index in indices.into_iter().rev() {
                            let _ = self.workspace.close(index);
                        }
                    }
                } else {
                    self.workspace.tabs[self.workspace.active].scroll =
                        (self.view.scroll_x, self.view.scroll_y);
                    self.workspace.click_sheet(
                        index,
                        event.modifiers.command(),
                        event.modifiers.shift(),
                    );
                    self.panels.dragging_tab = Some(index);
                    if event.click_count >= 2 {
                        self.workspace.tabs[index].transient = false;
                    }
                }
                self.changed(w, Instant::now(), false);
            }
            return;
        }
        if event.kind == MouseKind::Down
            && event.button == 0
            && event.click_count == 2
            && let Some(&(group, _, _)) = self
                .tab_bars
                .iter()
                .find(|(_, bar, _)| vel_ui::contains(*bar, event.position))
        {
            // A double click on the empty part of a tab bar opens a new file in that group.
            self.workspace.tabs[self.workspace.active].scroll =
                (self.view.scroll_x, self.view.scroll_y);
            self.workspace.workbench.active = group;
            self.run_named(w, "new_file", &json!({}));
            self.changed(w, Instant::now(), false);
            return;
        }
        if event.kind == MouseKind::Drag
            && let Some(from) = self.panels.dragging_tab
        {
            if let Some(target) = self
                .group_bounds
                .iter()
                .position(|b| vel_ui::contains(*b, event.position))
            {
                if target != self.workspace.workbench.active {
                    let _ = self.workspace.workbench.move_selected(target);
                    let _ = self.workspace.focus_current_group();
                } else if let Some((sheet, _)) = self
                    .tab_hits
                    .iter()
                    .find(|(_, rect)| vel_ui::contains(*rect, event.position))
                {
                    let moved = self.workspace.tabs.get(from).map(|t| t.sheet);
                    let group = &mut self.workspace.workbench.groups[target];
                    if let Some(moved) = moved
                        && moved != *sheet
                        && let Some(to) = group.sheets.iter().position(|s| s == sheet)
                    {
                        group.sheets.retain(|s| *s != moved);
                        group.sheets.insert(to.min(group.sheets.len()), moved);
                    }
                }
                self.changed(w, Instant::now(), false);
            }
            return;
        }
        if event.kind == MouseKind::Up {
            self.panels.dragging_tab = None;
        }
        if event.kind == MouseKind::Down
            && let Some((&sheet, _)) = self
                .sheet_bounds
                .iter()
                .find(|(_, rect)| vel_ui::contains(**rect, event.position))
            && let Some(index) = self.workspace.sheet_index(sheet)
            && index != self.workspace.active
        {
            self.workspace.tabs[self.workspace.active].scroll =
                (self.view.scroll_x, self.view.scroll_y);
            if let Some(group) = self.workspace.workbench.group_of(sheet) {
                self.workspace.workbench.active = group;
                self.workspace.workbench.groups[group].focused = Some(sheet);
            }
            if let Err(e) = self.workspace.prepare_sheet(index) {
                self.show_error(w, &e);
                return;
            }
            self.workspace.active = index;
            self.sync_active_view();
        }
        let command = self
            .workspace
            .mousemap
            .iter()
            .rev()
            .find(|binding| {
                let native_button = match event.button {
                    0 => 1,
                    1 => 2,
                    2 => 3,
                    n => n + 1,
                };
                let expected = [
                    ("ctrl", event.modifiers.control()),
                    ("alt", event.modifiers.option()),
                    ("shift", event.modifiers.shift()),
                    ("super", event.modifiers.command()),
                ]
                .into_iter()
                .filter(|(_, on)| *on)
                .map(|(name, _)| name)
                .collect::<std::collections::BTreeSet<_>>();
                let modifiers = binding
                    .modifiers
                    .iter()
                    .map(String::as_str)
                    .collect::<std::collections::BTreeSet<_>>();
                binding.button == native_button
                    && binding.count == event.click_count
                    && modifiers == expected
                    && self.workspace.editor().context_matches(&binding.context)
            })
            .and_then(|b| match event.kind {
                MouseKind::Down => b
                    .press_command
                    .as_ref()
                    .map(|n| (n.clone(), b.press_args.clone())),
                MouseKind::Up => b.command.as_ref().map(|n| (n.clone(), b.args.clone())),
                _ => None,
            });
        if let Some((name, args)) = command {
            self.run_named(w, &name, &args);
            return;
        }
        // The middle button makes column selections, like ⌥ with the primary button.
        let middle = event.button == 2;
        if event.button != 0 && !middle {
            return;
        }
        let local = Point {
            x: event.position.x - self.content.x,
            y: event.position.y - self.content.y,
        };
        if !middle {
            match self.view.pointer(
                self.workspace.editor_mut(),
                local,
                event.kind == MouseKind::Down,
                event.kind == MouseKind::Drag,
                event.kind == MouseKind::Up,
            ) {
                Ok(true) => {
                    self.changed(w, Instant::now(), false);
                    return;
                }
                Err(e) => {
                    self.show_error(w, &e);
                    return;
                }
                _ => {}
            }
        }
        let at = self.view.hit_test(self.workspace.editor(), local);
        if self.workspace.editor().binary.is_some() {
            // The hex view keeps byte-granular selection.
            match event.kind {
                MouseKind::Down => self
                    .workspace
                    .editor_mut()
                    .set_caret(at, event.modifiers.shift()),
                MouseKind::Drag => self.workspace.editor_mut().set_caret(at, true),
                MouseKind::Up | MouseKind::Move => {}
            }
            self.changed(w, Instant::now(), false);
            return;
        }
        match event.kind {
            MouseKind::Down => {
                use crate::mouse_select::{Combine, DragSelect, Unit};
                let m = event.modifiers;
                // A click on the line numbers selects the line (fold buttons were handled above).
                let numbers =
                    !middle && local.x >= 0. && local.x < self.view.gutter(self.workspace.editor());
                let unit = if event.click_count >= 3 || numbers {
                    Unit::Line
                } else if event.click_count == 2 {
                    Unit::Word
                } else {
                    Unit::Char
                };
                let combine = match (m.command(), m.shift()) {
                    (true, true) => Combine::Subtract,
                    (true, false) => Combine::Add,
                    _ => Combine::Replace,
                };
                self.drag_select = None;
                self.drag_text = None;
                if unit == Unit::Char
                    && !middle
                    && !(m.command() || m.shift() || m.option() || m.control())
                {
                    self.drag_text =
                        crate::mouse_select::DragText::begin(self.workspace.editor(), local, at);
                    if self.drag_text.is_some() {
                        return;
                    }
                }
                self.drag_select = Some(DragSelect::begin(
                    &self.view,
                    self.workspace.editor_mut(),
                    local,
                    at,
                    unit,
                    combine,
                    m.shift() && !m.command(),
                    middle || (m.option() && unit == Unit::Char),
                ));
            }
            MouseKind::Drag => {
                if let Some(drag) = self.drag_text.as_mut() {
                    drag.update(&mut self.view, self.workspace.editor(), local);
                } else if let Some(drag) = self.drag_select.as_mut() {
                    drag.update(&self.view, self.workspace.editor_mut(), local);
                } else {
                    return;
                }
            }
            MouseKind::Up => {
                self.drag_select = None;
                if let Some(drag) = self.drag_text.take() {
                    drag.finish(
                        &mut self.view,
                        self.workspace.editor_mut(),
                        local,
                        event.modifiers.option(),
                    );
                }
            }
            MouseKind::Move => {}
        }
        self.changed(w, Instant::now(), false);
    }
    fn on_scroll(&mut self, w: &WindowHandle, event: ScrollEvent) {
        self.cancel_context(w);
        if self.options.mode.automatic() {
            self.external_input += 1;
            return;
        }
        let scale = if event.precise {
            1.
        } else {
            self.view.line_height
        };
        if let Some(group) = self.group_bounds.iter().position(|r| {
            event.position.y >= r.y
                && event.position.y < r.y + crate::view::TAB_HEIGHT
                && vel_ui::contains(*r, event.position)
        }) {
            let max = self
                .tab_bars
                .iter()
                .find(|(g, _, _)| *g == group)
                .map_or(f64::INFINITY, |(_, _, max)| *max);
            let current = self.workspace.workbench.groups[group].tab_scroll.min(max);
            self.workspace.workbench.groups[group].tab_scroll =
                (current - (event.delta_x + event.delta_y) * scale).clamp(0., max);
            self.changed(w, Instant::now(), false);
            return;
        }
        let local = Point {
            x: event.position.x - self.content.x,
            y: event.position.y - self.content.y,
        };
        if self.workspace.editor().html.borrow_mut().scroll(
            local,
            -event.delta_x * scale,
            -event.delta_y * scale,
        ) {
            self.changed(w, Instant::now(), false);
            return;
        }
        if self.panels.scroll(event.position, -event.delta_y * scale) {
            self.changed(w, Instant::now(), false);
            return;
        }
        if let Some((&sheet, _)) = self
            .sheet_bounds
            .iter()
            .find(|(_, rect)| vel_ui::contains(**rect, event.position))
            && sheet != self.workspace.tabs[self.workspace.active].sheet
            && let Some(index) = self.workspace.sheet_index(sheet)
        {
            let active = self.workspace.active;
            if self.workspace.prepare_sheet(index).is_ok()
                && let Some(view) = self.sheet_views.get_mut(&sheet)
            {
                view.scroll(
                    &self.workspace.tabs[index].editor,
                    -event.delta_x * scale,
                    -event.delta_y * scale,
                );
                self.workspace.tabs[index].scroll = (view.scroll_x, view.scroll_y);
            }
            let _ = self.workspace.prepare_sheet(active);
            self.changed(w, Instant::now(), false);
            return;
        }
        self.view.scroll(
            self.workspace.editor(),
            -event.delta_x * scale,
            -event.delta_y * scale,
        );
        self.changed(w, Instant::now(), false);
    }
    fn on_resize(&mut self, w: &WindowHandle, size: Size, scale: f64) {
        if self.options.mode == Mode::Interactive {
            self.workspace.fullscreen = w.is_full_screen();
        }
        self.workspace.themes.ui.borrow_mut().density = scale as f32;
        self.window_size = size;
        self.view.size = size;
        self.view.clamp_scroll(self.workspace.editor());
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
        self.window_focused = focused;
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
                w.set_menu(&native_menu(&self.workspace.menus));
            }
            let viewport = self.viewport();
            changed |= self.panels.poll(&mut self.workspace, viewport);
            self.flush_context_menu(w);
            if std::mem::take(&mut self.panels.review.reveal) {
                self.view.reveal(self.workspace.editor());
            }
            if let Some(invocation) = self.panels.pending_command.take() {
                self.run_named(w, &invocation.command, &invocation.args);
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
                w.set_menu(&native_menu(&self.workspace.menus));
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
                        if caret_animation || self.workspace.themes.ui.borrow().animating() {
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
                && !self.workspace.themes.ui.borrow().animating()
            {
                return Ok(());
            }
        }
        if self.font_face != self.workspace.editor().preferences.font_face {
            let name = self.workspace.editor().preferences.font_face.clone();
            match Fonts::load_named(&name) {
                Ok(fonts) => {
                    self.view = View::new(self.view.size, fonts.cell as f64, fonts.ascent as f64);
                    self.fonts = Some(fonts);
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
fn native_menu(items: &[vel_config::ui::MenuItem]) -> Vec<MenuEntry> {
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
            children: native_menu(&item.children),
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
