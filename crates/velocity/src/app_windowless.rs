//! App-level state that survives the last native window.
use super::*;

pub(super) struct Controller {
    options: Options,
    reporter: Reporter,
    data: Option<PathBuf>,
    sessions: std::rc::Rc<std::cell::RefCell<crate::session::Sessions>>,
    closed_id: u64,
    layout: vel_ui::workbench::Layout,
    dark: bool,
}

impl Controller {
    pub(super) fn new(app: &App) -> Self {
        let mut options = app.options.clone();
        options.file = None;
        options.files = Vec::new();
        options.project = None;
        options.command = None;
        Self {
            options,
            reporter: app.reporter.clone(),
            data: app.workspace.data_path().map(Path::to_owned),
            sessions: app.sessions.clone(),
            closed_id: app.session_id,
            layout: app.workspace.workbench.layout.clone(),
            dark: app.workspace.dark,
        }
    }

    // Reuse the ordinary command and window builders only when there is work.
    // This empty App never starts a renderer or registers as a session window.
    pub(super) fn command_app(&self) -> Result<App, String> {
        let mut workspace = Workspace::new_empty(self.data.clone(), None, false)?;
        workspace.dark = self.dark;
        workspace.workbench.set_layout(self.layout.clone())?;
        [
            workspace.recent_files,
            workspace.recent_folders,
            workspace.recent_projects,
        ] = self.sessions.borrow().recent_items();
        Ok(App::new_with_session_id(
            self.options.clone(),
            Clock::new(Instant::now(), std::time::SystemTime::now()),
            self.reporter.clone(),
            workspace,
            Arc::new(AtomicBool::new(false)),
            self.sessions.clone(),
            self.closed_id,
        ))
    }

    fn run(&self, f: impl FnOnce(&mut App)) {
        match self.command_app() {
            Ok(mut app) => f(&mut app),
            Err(error) => eprintln!("Could not run a windowless command: {error}"),
        }
    }
}

impl AppHandler for Controller {
    fn on_named_command(&mut self, window: &WindowHandle, name: &str, args: &str) {
        self.run(|app| app.on_named_command(window, name, args));
    }
    fn on_reopen(&mut self, window: &WindowHandle) {
        self.run(|app| app.on_reopen(window));
    }
    fn on_finder_new_text_file(&mut self, window: &WindowHandle, directory: &Path) {
        self.run(|app| app.on_finder_new_text_file(window, directory));
    }
    fn on_idle(&mut self, window: &WindowHandle) {
        let requests = self
            .sessions
            .borrow()
            .instance
            .as_ref()
            .map(|instance| instance.drain())
            .unwrap_or_default();
        if !requests.is_empty() {
            self.run(|app| {
                for mut incoming in requests {
                    incoming.request.new_window = true;
                    if let Err(error) = app.receive_request(window, incoming) {
                        app.named_error(Some(window), &error);
                    }
                }
            });
        }
    }
    fn on_frame(&mut self, _context: &mut FrameContext<'_>) -> Result<(), String> {
        Ok(())
    }
}
