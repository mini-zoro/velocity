//! Routes commands and events to the workspace Settings sheet, not the editor underneath it.
use super::*;
use crate::settings_ui::Navigation;
use vel_config::keymap::Key;

impl App {
    pub(super) fn settings_page_has_focus(&self) -> bool {
        self.workspace.settings_page.is_some() && self.panels.overlay.is_none()
    }

    /// Settings open in their own window (⌘, / Ctrl+,). Invoked in that window, the command
    /// brings it forward with the search field focused.
    pub(super) fn show_settings_page(&mut self, window: Option<&WindowHandle>) {
        if !self.settings_window || self.workspace.settings_page.is_none() {
            if let Err(error) = self.open_settings_window(window) {
                let detail = self
                    .workspace
                    .translations
                    .tr_fmt("Could not open Settings: {detail}", &[("detail", error)]);
                self.named_error(window, &detail);
            }
            return;
        }
        if self.panels.overlay.is_some() {
            self.panels.cancel_overlay(&mut self.workspace);
        }
        self.leave_region_focus();
        if let Some(mut page) = self.workspace.settings_page.take() {
            page.refresh(&self.workspace);
            page.focus_search(&mut self.workspace);
            self.workspace.settings_page = Some(page);
        }
        if let Some(window) = window {
            window.focus();
        }
        self.settings_redraw(window);
    }

    pub(super) fn settings_redraw(&self, window: Option<&WindowHandle>) {
        if let Some(window) = window {
            window.set_title(&self.workspace.window_title());
            window.invalidate_character_coordinates();
            window.request_frame();
            if let Some(due) = self
                .workspace
                .settings_page
                .as_ref()
                .and_then(|p| p.deadline())
            {
                window.request_frame_at(due);
            }
            if let Some(due) = self
                .workspace
                .settings_page
                .as_ref()
                .and_then(|p| p.hover_deadline())
                && due > Instant::now()
            {
                window.request_frame_at(due);
            }
        }
    }

    pub(super) fn settings_scene(&mut self) -> vel_render::scene::Scene {
        let mut page = self.workspace.settings_page.take().unwrap();
        page.refresh(&self.workspace);
        let mut scene = {
            let mut theme = self.workspace.themes.ui.borrow_mut();
            theme.begin_frame(Instant::now());
            page.paint(
                &mut theme,
                &self.workspace.settings,
                &self.workspace.translations,
                self.window_size,
                self.workspace.dark,
                self.window_focused && self.panels.overlay.is_none(),
            )
        };
        self.workspace.settings_page = Some(page);
        self.exact_scene = true;
        if self.panels.overlay.is_some() {
            self.panels.layout(self.window_size);
            self.panels.paint(&self.workspace, &mut scene);
        }
        scene
    }

    pub(super) fn poll_settings_input(&mut self, now: Instant) -> bool {
        let Some(mut page) = self.workspace.settings_page.take() else {
            return false;
        };
        let changed = page.flush_due(&mut self.workspace, now);
        self.workspace.settings_page = Some(page);
        changed
    }

    pub(super) fn finish_settings_input(&mut self) {
        if let Some(mut page) = self.workspace.settings_page.take() {
            page.finish_edit(&mut self.workspace);
            self.workspace.settings_page = Some(page);
        }
    }

    fn settings_navigation(&mut self, window: Option<&WindowHandle>, nav: Navigation) {
        // The font popup lists the system's families, asked for only when it first opens.
        if let Some(mut page) = self.workspace.settings_page.take() {
            if page.take_font_request() {
                let families = match window {
                    #[cfg(target_os = "macos")]
                    Some(window) => window.font_families(),
                    #[cfg(windows)]
                    Some(_) => vel_platform::fonts::font_families(),
                    _ => Vec::new(),
                };
                page.provide_fonts(&self.workspace, families);
            }
            self.workspace.settings_page = Some(page);
        }
        match nav {
            Navigation::Stay => {}
            Navigation::Close if self.settings_window => match window {
                Some(window) => window.request_close(),
                None => self.close_settings_window(),
            },
            Navigation::Close => {
                self.workspace.settings_page = None;
                self.named_changed(window, false);
            }
            Navigation::Command(name, args) => {
                let args = crate::settings_ui::platform_args(args);
                self.run_named(window, name, &args);
            }
            Navigation::NewWindow => {
                if let Some(window) = window {
                    self.run_named(Some(window), "new_window", &json!({}));
                }
            }
            Navigation::Json | Navigation::Repair => {
                let result = self.apply_settings_action(
                    window,
                    "edit_settings",
                    &super::settings_entry::preferences_json_args(),
                );
                if let Err(error) = result {
                    let detail = self.workspace.translations.tr_fmt(
                        "Could not open User settings: {detail}",
                        &[("detail", error)],
                    );
                    self.named_error(window, &detail);
                }
            }
        }
        self.settings_redraw(window);
    }

    pub(super) fn settings_pointer(
        &mut self,
        window: Option<&WindowHandle>,
        point: Point,
        extend: bool,
        now: Instant,
    ) {
        let mut page = self.workspace.settings_page.take().unwrap();
        let nav = page.pointer(&mut self.workspace, point, extend, now);
        self.workspace.settings_page = Some(page);
        self.settings_navigation(window, nav);
    }

    pub(super) fn settings_command(
        &mut self,
        window: Option<&WindowHandle>,
        cmd: Command,
        now: Instant,
    ) {
        let mut page = self.workspace.settings_page.take().unwrap();
        if matches!(cmd, Command::Paste)
            && let (Some(clipboard), Some(input)) = (window, page.input_mut())
        {
            input.set_clipboard(clipboard.clipboard_text().unwrap_or_default());
        }
        let nav = page.command(&mut self.workspace, cmd, now);
        if matches!(cmd, Command::Copy | Command::Cut)
            && let (Some(clipboard), Some(input)) = (window, page.input())
        {
            clipboard.set_clipboard_text(input.clipboard());
        }
        self.workspace.settings_page = Some(page);
        self.settings_navigation(window, nav);
    }

    pub(super) fn settings_key(
        &mut self,
        window: Option<&WindowHandle>,
        key: &Key,
        now: Instant,
    ) -> bool {
        use Command::*;
        let cmd = match (key.name.as_str(), key.modifiers) {
            ("tab", 0) => Tab,
            ("tab", 4) => Backtab,
            ("escape", 0) => Cancel,
            ("enter" | "keypad_enter", 0) => Newline,
            ("up", 0) => Up(false),
            ("down", 0) => Down(false),
            ("left", 0) => Left(false),
            ("right", 0) => Right(false),
            ("f", 8) => {
                // ⌘F / Ctrl+F searches the settings.
                let mut page = self.workspace.settings_page.take().unwrap();
                page.focus_search(&mut self.workspace);
                self.workspace.settings_page = Some(page);
                self.settings_redraw(window);
                return true;
            }
            ("pageup", 0) => PageUp,
            ("pagedown", 0) => PageDown,
            ("space", 0)
                if self
                    .workspace
                    .settings_page
                    .as_ref()
                    .unwrap()
                    .input()
                    .is_none() =>
            {
                let mut page = self.workspace.settings_page.take().unwrap();
                let nav = page.space(&mut self.workspace, now);
                self.workspace.settings_page = Some(page);
                self.settings_navigation(window, nav);
                return true;
            }
            ("a", 8) => SelectAll,
            ("c", 8) => Copy,
            ("x", 8) => Cut,
            ("v", 8) => Paste,
            ("z", 8) => Undo,
            ("z", 12) => Redo,
            ("w", 8) => {
                self.finish_settings_input();
                self.settings_navigation(window, Navigation::Close);
                return true;
            }
            // Printable input and selection/editing selectors go through NSTextInputClient.
            // The Settings window resolves other shortcuts through the keymap.
            _ => return key.modifiers & 8 != 0 && key.name != "q" && !self.settings_window,
        };
        self.settings_command(window, cmd, now);
        true
    }

    pub(super) fn settings_named(&mut self, window: Option<&WindowHandle>, name: &str) -> bool {
        let cmd = match name {
            "close" | "close_file" => {
                self.finish_settings_input();
                self.settings_navigation(window, Navigation::Close);
                return true;
            }
            "select_all" => Command::SelectAll,
            "copy" => Command::Copy,
            "cut" => Command::Cut,
            "paste" => Command::Paste,
            "undo" | "soft_undo" => Command::Undo,
            "redo" | "soft_redo" => Command::Redo,
            _ => {
                return self.workspace.editor().registry.kind(name)
                    == Some(vel_commands::Kind::TextCommand);
            }
        };
        self.settings_command(window, cmd, Instant::now());
        true
    }
}

#[cfg(test)]
#[path = "settings_ui_tests.rs"]
mod tests;
