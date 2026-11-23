//! The standalone Settings window: one instance, opened on demand and dropped when it
//! closes. It has no session state, never receives opened files and is not an editor window
//! for routing, cycling or hot exit. Its page writes the User preferences file and passes
//! each change to the editor windows so they apply it at once.
use super::*;
use crate::settings_ui::Page;

/// The Settings window's default and smallest content sizes, in points.
pub(super) const SETTINGS_SIZE: Size = Size {
    width: 720.,
    height: 480.,
};
pub(super) const SETTINGS_MINIMUM: Size = Size {
    width: 600.,
    height: 400.,
};

/// Commands the Settings window handles itself.
fn local_command(name: &str) -> bool {
    matches!(
        name,
        "close"
            | "close_file"
            | "close_window"
            | "select_all"
            | "copy"
            | "cut"
            | "paste"
            | "undo"
            | "soft_undo"
            | "redo"
            | "soft_redo"
            | "show_settings"
            | "edit_settings"
            | "open_dir"
            | "customize_color_scheme"
            | "customize_theme"
            | "new_window"
            | "exit"
            | "hide_overlay"
    )
}

/// Window-level commands that the Settings window passes to an editor window (opening one
/// when none is left). Document commands are disabled while Settings is in front.
fn forwarded_command(name: &str) -> bool {
    matches!(
        name,
        "new_file"
            | "prompt_open_file"
            | "prompt_open_folder"
            | "prompt_open_folder_in_new_window"
            | "prompt_open_project_or_workspace"
            | "open_recent_file"
            | "open_recent_folder"
            | "open_recent_project"
            | "open_recent_project_or_workspace"
            | "open_project"
            | "open_project_or_workspace"
            | "reopen_last_file"
            | "show_overlay"
            | "show_indexing_status"
            | "open_url"
            | "open_crash_reports"
    )
}

impl App {
    /// Opens the Settings window, or brings the open one to the front with its search field
    /// focused.
    pub(super) fn open_settings_window(
        &mut self,
        window: Option<&WindowHandle>,
    ) -> Result<(), String> {
        let existing = self.sessions.borrow().settings_window.clone();
        match existing {
            Some((_, Some(handle))) if !handle.is_closed() => {
                handle.dispatch_named_command("show_settings", "{}");
                handle.focus();
                handle.request_frame();
                return Ok(());
            }
            Some((id, None)) => {
                if let Some(app) = self
                    .opened_preferences
                    .iter_mut()
                    .find(|a| a.session_id == id)
                {
                    app.show_settings_page(None);
                    return Ok(());
                }
            }
            _ => {}
        }
        let app = self.settings_window_app()?;
        let id = app.session_id;
        if let Some(window) = window {
            let handle = window.new_window(
                WindowOptions {
                    title: app.workspace.window_title(),
                    size: SETTINGS_SIZE,
                    min_size: Some(SETTINGS_MINIMUM),
                    kind: vel_platform::WindowKind::Utility,
                },
                app,
            )?;
            self.sessions.borrow_mut().settings_window = Some((id, Some(handle)));
        } else {
            self.sessions.borrow_mut().settings_window = Some((id, None));
            self.opened_preferences.push(app);
        }
        Ok(())
    }

    fn settings_window_app(&self) -> Result<Self, String> {
        let workspace = self.workspace.for_settings_window()?;
        let mut options = self.options.clone();
        options.files.clear();
        options.file = None;
        options.project = None;
        options.width = SETTINGS_SIZE.width as _;
        options.height = SETTINGS_SIZE.height as _;
        let previous = self.sessions.borrow().active_window;
        let mut app = App::new(
            options,
            crate::report::Clock::new(Instant::now(), std::time::SystemTime::now()),
            self.reporter.clone(),
            workspace,
            Arc::new(AtomicBool::new(false)),
            self.sessions.clone(),
        );
        app.sessions
            .borrow_mut()
            .release_utility(app.session_id, previous);
        app.settings_window = true;
        app.session_due = None;
        app.panels.side_visible = false;
        app.workspace.settings_page = Some(Box::new(Page::new(&app.workspace)));
        Ok(app)
    }

    /// Whether the Settings window runs `name` itself; otherwise the command is forwarded or
    /// ignored. Only meaningful for the Settings window.
    pub(super) fn settings_window_runs(&self, name: &str) -> bool {
        local_command(name)
    }

    pub(super) fn settings_window_command_enabled(&self, name: &str) -> Option<bool> {
        if !self.settings_window || local_command(name) {
            return None;
        }
        Some(forwarded_command(name))
    }

    /// Runs a window-level command in the most recently used editor window, or in a new one.
    pub(super) fn forward_from_settings(
        &mut self,
        window: Option<&WindowHandle>,
        name: &str,
        args: &Value,
    ) {
        if !forwarded_command(name) {
            return;
        }
        let target = {
            let sessions = self.sessions.borrow();
            sessions
                .active_window
                .and_then(|id| sessions.handles.get(&id))
                .or_else(|| sessions.handles.values().next_back())
                .filter(|handle| !handle.is_closed())
                .cloned()
        };
        let args = args.to_string();
        match (target, window) {
            (Some(handle), _) => {
                handle.focus();
                handle.dispatch_named_command(name, &args);
            }
            (None, Some(window)) => match self.open_native_window(window, None, None, true) {
                Ok((handle, _)) => {
                    if name != "new_file" {
                        handle.dispatch_named_command(name, &args);
                    }
                }
                Err(error) => self.named_error(Some(window), &error),
            },
            (None, None) => {}
        }
    }

    /// Per-frame work of the Settings window: pass written preferences to the editor windows,
    /// keep the menu bar current, and serve command-line requests when it is the only window.
    pub(super) fn poll_settings_window(&mut self, w: &WindowHandle) {
        for handle in self.publish_settings_changes() {
            handle.request_frame();
        }
        self.install_native_menu(w, false);
        let requests = {
            let sessions = self.sessions.borrow();
            if sessions.has_live_windows() {
                Vec::new()
            } else {
                sessions
                    .instance
                    .as_ref()
                    .map(|i| i.drain())
                    .unwrap_or_default()
            }
        };
        for mut incoming in requests {
            incoming.request.new_window = true;
            if let Err(error) = self.receive_request(w, incoming) {
                self.named_error(Some(w), &error);
            }
        }
    }

    /// Queues the preferences the page wrote for every editor window; returns the windows
    /// to wake.
    pub(super) fn publish_settings_changes(&mut self) -> Vec<WindowHandle> {
        let published = self
            .workspace
            .settings_page
            .as_mut()
            .map(|page| page.take_published())
            .unwrap_or_default();
        if published.is_empty() {
            return Vec::new();
        }
        let mut sessions = self.sessions.borrow_mut();
        for (key, value) in published {
            sessions.broadcast_preference(&key, value);
        }
        sessions.handles.values().cloned().collect()
    }

    /// An editor window applies the preferences the Settings window wrote, without waiting
    /// for its own reload of the file.
    pub(super) fn receive_preference_updates(&mut self) -> bool {
        let updates = self
            .sessions
            .borrow_mut()
            .preference_updates
            .remove(&self.session_id)
            .unwrap_or_default();
        if updates.is_empty() {
            return false;
        }
        for (key, value) in updates {
            self.workspace.apply_user_preference(&key, value);
        }
        self.workspace.sync_ui_language();
        if let Err(e) = self
            .workspace
            .themes
            .select(&self.workspace.settings, self.workspace.dark)
        {
            self.workspace.messages.push(e);
        }
        true
    }

    /// Closing the Settings window: finish a pending edit and release the single instance.
    pub(super) fn close_settings_window(&mut self) {
        self.finish_settings_input();
        for handle in self.publish_settings_changes() {
            handle.request_frame();
        }
        self.workspace.settings_page = None;
        let mut sessions = self.sessions.borrow_mut();
        if sessions
            .settings_window
            .as_ref()
            .is_some_and(|(id, _)| *id == self.session_id)
        {
            sessions.settings_window = None;
        }
    }
}
