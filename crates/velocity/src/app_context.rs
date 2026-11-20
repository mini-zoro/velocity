//! Native menu orchestration. Tracking starts only after the platform releases App.
use super::*;
use crate::context_menu::{self as menus, ContextMenuKind, MenuContext, SidebarEffect};
use std::path::PathBuf;

#[derive(Debug)]
pub(super) struct PendingMenu {
    kind: ContextMenuKind,
    context: MenuContext,
    sheet: Option<u64>,
    revision: Option<u64>,
    started: Instant,
}

struct FileOperations<'a>(&'a WindowHandle);
impl menus::SidebarPlatform for FileOperations<'_> {
    fn trash(&mut self, path: &Path) -> Result<(), String> {
        self.0.trash_file(path)
    }
    fn copy_text(&mut self, text: &str) -> Result<(), String> {
        if self.0.set_clipboard_text(text) {
            Ok(())
        } else {
            Err("Could not copy the path".into())
        }
    }
    fn reveal(&mut self, path: &Path) -> Result<(), String> {
        self.0.reveal_file(path)
    }
}

impl App {
    pub(super) fn cancel_context(&mut self, w: &WindowHandle) {
        self.pending_menu = None;
        self.panels.cancel_review_menu();
        w.cancel_context_menu();
    }
    fn context_roots(&self) -> Vec<PathBuf> {
        self.workspace
            .folders
            .iter()
            .map(|folder| folder.path.clone())
            .collect()
    }
    #[cfg(test)]
    fn context_close_targets(&self, name: &str, args: &Value) -> Vec<usize> {
        self.tab_close_sheets(name, args)
            .into_iter()
            .filter_map(|id| self.workspace.sheet_index(id))
            .collect()
    }
    pub(super) fn context_state(
        &self,
        name: &str,
        args: &Value,
    ) -> Option<vel_config::ui::CommandState> {
        if let Some(state) = menus::sidebar_command_state(name, args, &self.context_roots()) {
            return Some(state);
        }
        if Self::is_tab_close(name) {
            return Some(vel_config::ui::CommandState {
                enabled: !self.tab_close_sheets(name, args).is_empty(),
                ..Default::default()
            });
        }
        None
    }
    /// Collects context without opening AppKit's nested tracking loop while borrowed.
    pub(super) fn begin_context_menu(&mut self, w: &WindowHandle, point: Point) {
        self.collect_context_menu(point);
        if self.pending_menu.is_some() {
            self.changed(w, Instant::now(), false);
            self.flush_context_menu(w);
        }
    }
    fn collect_context_menu(&mut self, point: Point) {
        self.panels.review.native_context = true;
        self.panels.cancel_review_menu();
        self.pending_menu = None;
        let group = self.workspace.workbench.active;
        let mut context = MenuContext {
            event: point,
            group,
            ..Default::default()
        };
        let kind;
        let sheet;
        // The menu resource is chosen by the hit surface, not by current focus:
        // https://www.sublimetext.com/docs/menus.html#available-menus
        if self.panels.focus_context_input(point) {
            kind = ContextMenuKind::Widget;
            sheet = None;
        } else if self.panels.overlay.is_some() {
            return;
        } else if let Some((id, _)) = self
            .tab_hits
            .iter()
            .find(|(_, rect)| vel_ui::contains(*rect, point))
            .copied()
        {
            kind = ContextMenuKind::Tab;
            sheet = Some(id);
            context.group = self.workspace.workbench.group_of(id).unwrap_or(group);
            context.index = self.workspace.workbench.groups[context.group]
                .sheets
                .iter()
                .position(|sheet| *sheet == id)
                .unwrap_or(0);
            context.paths = self
                .workspace
                .sheet_index(id)
                .and_then(|i| self.workspace.tabs[i].path.clone())
                .into_iter()
                .collect();
        } else if let Some((path, folder)) = self.panels.review_side_path(&self.workspace, point) {
            kind = self.sidebar_menu_kind(&path);
            sheet = None;
            context.paths.push(path.clone());
            self.panels
                .review_context(&mut self.workspace, point, 0, Some((path, folder)));
        } else if let Some((&id, _)) = self
            .sheet_bounds
            .iter()
            .find(|(_, rect)| vel_ui::contains(**rect, point))
        {
            kind = ContextMenuKind::Editor;
            self.panels.focus_editor_for_context();
            sheet = Some(id);
            if let Some(index) = self.workspace.sheet_index(id) {
                self.workspace.select(index);
                self.sync_active_view();
            }
            context.group = self.workspace.workbench.group_of(id).unwrap_or(group);
            context.index = self.workspace.workbench.groups[context.group]
                .sheets
                .iter()
                .position(|sheet| *sheet == id)
                .unwrap_or(0);
            context.paths = self.workspace.tabs[self.workspace.active]
                .path
                .clone()
                .into_iter()
                .collect();
            let byte = self.view.hit_test(
                self.workspace.editor(),
                Point {
                    x: point.x - self.content.x,
                    y: point.y - self.content.y,
                },
            );
            self.panels
                .review_context(&mut self.workspace, point, byte, None);
        } else {
            return;
        }
        self.pending_menu = Some(PendingMenu {
            kind,
            context,
            sheet,
            revision: sheet
                .and_then(|id| self.workspace.sheet_index(id))
                .map(|i| self.workspace.tabs[i].editor.revision),
            started: Instant::now(),
        });
    }
    pub(super) fn flush_context_menu(&mut self, w: &WindowHandle) {
        let Some(pending) = &self.pending_menu else {
            return;
        };
        if pending.kind == ContextMenuKind::Editor
            && pending.sheet.is_none_or(|sheet| {
                self.workspace.sheet_index(sheet).is_none_or(|i| {
                    i != self.workspace.active
                        || Some(self.workspace.tabs[i].editor.revision) != pending.revision
                })
            })
        {
            self.cancel_context(w);
            return;
        }
        if self.panels.review_suggestions_pending()
            && pending.started.elapsed() < Duration::from_millis(250)
        {
            w.request_frame_at(Instant::now() + Duration::from_millis(8));
            return;
        }
        w.show_context_menu(&self.take_context_entries());
    }
    fn take_context_entries(&mut self) -> Vec<MenuEntry> {
        let Some(pending) = self.pending_menu.take() else {
            return Vec::new();
        };
        let dynamic = if pending.kind == ContextMenuKind::Widget {
            self.panels
                .widget_history_menu(&self.workspace.translations)
        } else {
            std::mem::take(&mut self.panels.review.items)
        };
        self.panels.cancel_review_menu();
        self.context_entries(&pending, &dynamic)
    }
    #[cfg(test)]
    pub(super) fn headless_context_menu(&mut self, point: Point) -> Vec<MenuEntry> {
        self.collect_context_menu(point);
        self.take_context_entries()
    }
    pub(super) fn sidebar_menu_kind(&self, path: &Path) -> ContextMenuKind {
        if self
            .workspace
            .folders
            .iter()
            .any(|folder| folder.path == path)
        {
            ContextMenuKind::MountPoint
        } else {
            ContextMenuKind::SideBar
        }
    }
    pub(super) fn show_find_options(&mut self, w: &WindowHandle, point: Point) {
        w.show_context_menu(&self.find_options_menu(point));
    }
    pub(super) fn find_options_menu(&self, point: Point) -> Vec<MenuEntry> {
        let pending = PendingMenu {
            kind: ContextMenuKind::FindInFiles,
            context: MenuContext {
                event: point,
                ..Default::default()
            },
            sheet: None,
            revision: None,
            started: Instant::now(),
        };
        self.context_entries(&pending, &[])
    }
    fn context_entries(
        &self,
        pending: &PendingMenu,
        dynamic: &[vel_config::ui::PaletteItem],
    ) -> Vec<MenuEntry> {
        let resources = self
            .workspace
            .context_menus
            .get(pending.kind.resource_name())
            .map(Vec::as_slice)
            .unwrap_or_default();
        let menu = menus::with_dynamic(resources, dynamic);
        let mut context = pending.context.arguments();
        context["_context_menu"] = json!(match pending.kind {
            ContextMenuKind::Editor => "editor",
            ContextMenuKind::Tab => "tab",
            ContextMenuKind::SideBar | ContextMenuKind::MountPoint => "sidebar",
            ContextMenuKind::Widget => "widget",
            ContextMenuKind::FindInFiles => "find_in_files",
        });
        if let Some(sheet) = pending.sheet {
            context["_context_sheet"] = json!(sheet);
        }
        let rows = menus::resolve_menu(&menu, &context, |invocation| {
            let args = invocation.args.to_string();
            vel_config::ui::CommandState {
                enabled: self.command_enabled(&invocation.command, &args),
                visible: self.command_visible(&invocation.command, &args),
                checked: self.command_checked(&invocation.command, &args),
            }
        });
        fn native(
            rows: Vec<vel_config::ui::ResolvedMenuItem>,
            tr: &vel_i18n::Translator,
        ) -> Vec<MenuEntry> {
            rows.into_iter()
                .map(|row| MenuEntry {
                    caption: if row.invocation.as_ref().is_some_and(|i| {
                        i.command == "replace_misspelling" || i.command == "widget_history"
                    }) {
                        row.caption
                    } else {
                        tr.caption(&row.caption).to_owned()
                    },
                    command: row.invocation.as_ref().map(|i| i.command.clone()),
                    args: row
                        .invocation
                        .map_or_else(|| "{}".into(), |i| i.args.to_string()),
                    children: native(row.children, tr),
                    ..Default::default()
                })
                .collect()
        }
        native(rows, &self.workspace.translations)
    }
    /// Returns true only for a command fully handled at window scope.
    pub(super) fn context_action(
        &mut self,
        w: &WindowHandle,
        name: &str,
        args: &Value,
    ) -> Result<bool, String> {
        if !menus::COMMANDS.contains(&name) {
            return Ok(false);
        }
        if (name == "side_bar_rename" && args.get("text").is_some())
            || (name == "side_bar_delete" && args["confirmed"] == true)
        {
            let paths = args["paths"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .collect::<Vec<_>>();
            menus::prepare_file_operation(&mut self.workspace, &paths)?;
        }
        let roots = self.context_roots();
        let Some(effect) = menus::sidebar_command(name, args, &roots, &mut FileOperations(w))?
        else {
            return Ok(false);
        };
        match effect {
            SidebarEffect::Input(invocation, caption) => {
                let viewport = self.viewport();
                self.panels.show_input_caption(
                    &mut self.workspace,
                    &invocation.command,
                    &invocation.args,
                    caption,
                    viewport,
                )?;
            }
            SidebarEffect::ConfirmDelete { paths, on_confirm } => {
                let text = UiText::tr_fmt(
                    "Move these entries to the Trash?\n{paths}",
                    [(
                        "paths",
                        paths
                            .iter()
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )],
                );
                if Dialog::new(
                    UiText::tr("Move to Trash?"),
                    text,
                    [UiText::tr("Cancel"), UiText::tr("Move to Trash")],
                )
                .show(&self.workspace.translations, w)
                    == 1
                {
                    self.context_action(w, &on_confirm.command, &on_confirm.args)?;
                }
            }
            SidebarEffect::Created { path, directory } => {
                self.panels.refresh_sidebar(&mut self.workspace);
                if !directory {
                    crate::open_requests::open_paths(
                        &mut self.workspace,
                        &mut self.panels,
                        &[path],
                    )?;
                }
            }
            SidebarEffect::Renamed { from, to } => {
                let result = menus::apply_rename(&mut self.workspace, &from, &to);
                self.panels.refresh_sidebar(&mut self.workspace);
                result?;
            }
            SidebarEffect::Deleted { paths, failures } => {
                crate::open_requests::apply_trashed(&mut self.workspace, &mut self.panels, &paths)?;
                if !failures.is_empty() {
                    return Err(failures.join("\n"));
                }
            }
            SidebarEffect::Open(paths) => {
                for path in paths {
                    if path.is_dir() {
                        self.panels
                            .open_sidebar_directory(&mut self.workspace, path)?;
                    } else {
                        crate::open_requests::open_paths(
                            &mut self.workspace,
                            &mut self.panels,
                            &[path],
                        )?;
                    }
                }
            }
            SidebarEffect::FindInFolder(path) => {
                let viewport = self.viewport();
                self.panels.action(
                    &mut self.workspace,
                    "find_in_files",
                    &json!({"panel":"find_in_files"}),
                    viewport,
                )?;
                self.panels.where_input.set(&path.to_string_lossy());
            }
            SidebarEffect::RemoveFolder(path) => self
                .panels
                .remove_sidebar_folder(&mut self.workspace, &path)?,
            SidebarEffect::RevealInSideBar(path) => {
                self.panels.reveal_sidebar_path(&mut self.workspace, path)?
            }
            SidebarEffect::Done => {}
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app(workspace: Workspace) -> App {
        App::new(
            Options::default(),
            Clock::new(Instant::now(), std::time::SystemTime::now()),
            Reporter::for_options(&Options::default(), Instant::now()).unwrap(),
            workspace,
            Arc::new(AtomicBool::new(false)),
            std::rc::Rc::new(std::cell::RefCell::new(crate::session::Sessions::new(None))),
        )
    }
    #[test]
    fn forwarded_native_open_uses_literal_paths_keeps_selection_and_acknowledges_errors() {
        use std::{io::Read, os::unix::net::UnixStream};
        let directory = tempfile::tempdir().unwrap();
        let file = directory
            .path()
            .canonicalize()
            .unwrap()
            .join("literal:120:7");
        std::fs::write(&file, "native document").unwrap();
        let socket = file.with_file_name("failed.socket");
        let _socket = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let mut workspace = Workspace::new(vel_text::Text::new(), None, None, false).unwrap();
        workspace
            .open(crate::cli::FileLocation {
                path: file.clone(),
                line: 1,
                column: 1,
            })
            .unwrap();
        workspace.editor_mut().set_caret(5, false);
        workspace.take_pending_center();
        let mut app = app(workspace);
        let mut request = crate::instance::Request::new(&Options::default(), None).unwrap();
        request.native_open = true;
        request.files = vec![
            crate::cli::FileLocation {
                path: file.clone(),
                line: 120,
                column: 7,
            },
            crate::cli::FileLocation {
                path: socket,
                line: 1,
                column: 1,
            },
        ];
        let (stream, mut client) = UnixStream::pair().unwrap();
        app.accept_request(crate::instance::Incoming::fixture(request, stream))
            .unwrap();
        let mut ack = [0];
        client.read_exact(&mut ack).unwrap();
        assert_eq!(ack, [1]);
        assert_eq!(app.workspace.tabs.len(), 1);
        assert_eq!(app.workspace.editor().caret, 5);
        assert!(!app.workspace.take_pending_center());
        assert!(app.panels.message.contains("failed.socket"));
    }
    #[test]
    fn context_state_uses_clicked_sheet_and_disables_stale_or_read_only_actions() {
        let mut workspace = Workspace::new(vel_text::Text::from("one"), None, None, false).unwrap();
        workspace.new_file().unwrap();
        workspace.new_file().unwrap();
        let clicked = workspace.tabs[0].sheet;
        workspace.tabs[0].editor.read_only = true;
        workspace.tabs[0].editor.settings.set(
            vel_config::settings::Layer::View,
            "word_wrap",
            json!(true),
        );
        workspace.tabs[2].editor.settings.set(
            vel_config::settings::Layer::View,
            "word_wrap",
            json!(false),
        );
        let mut app = app(workspace);
        let args = json!({"_context_menu":"tab","_context_sheet":clicked,"group":0,"index":0});
        assert_eq!(
            app.context_close_targets("close_tabs_to_right", &args),
            [2, 1]
        );
        assert!(app.command_enabled("close_tabs_to_right", &args.to_string()));
        assert!(!app.command_enabled("cut", &args.to_string()));
        assert!(app.command_enabled("copy", &args.to_string()));
        let mut checked_args = args.clone();
        checked_args["setting"] = json!("word_wrap");
        assert!(app.command_checked("toggle_setting", &checked_args.to_string()));
        // Group-local numeric indices may change during tracking; identity stays authoritative.
        app.workspace.workbench.groups[0].sheets.reverse();
        assert!(
            app.context_close_targets("close_tabs_to_right", &args)
                .is_empty()
        );
        app.workspace.close(0).unwrap();
        assert!(!app.command_enabled("close_file", &args.to_string()));
        assert!(!app.command_enabled("copy", &args.to_string()));
    }
    #[test]
    fn chains_inherit_actual_context_and_preserve_each_childs_command_arguments() {
        let parent = json!({"_context_menu":"tab","_context_sheet":42,"group":2,"index":1,
            "paths":["/fixture/a"],"event":{"x":10,"y":20},"commands":[]});
        let args = context_child_args(&parent, &json!({"setting":"word_wrap","group":99}));
        assert_eq!(args["_context_sheet"], 42);
        // A child's concrete argument wins; its `-1` placeholder asks for the click.
        assert_eq!(args["group"], 99);
        let placeholder = context_child_args(&parent, &json!({"group":-1,"index":-1}));
        assert_eq!(
            (placeholder["group"].clone(), placeholder["index"].clone()),
            (json!(2), json!(1))
        );
        assert_eq!(args["paths"], parent["paths"]);
        assert_eq!(args["event"], parent["event"]);
        assert_eq!(args["setting"], "word_wrap");
        assert!(args.get("commands").is_none());
        assert_eq!(
            context_child_args(&json!({"commands":[]}), &json!("unchanged")),
            "unchanged"
        );
    }
    #[test]
    fn startup_and_later_location_requests_center_after_workbench_geometry() {
        let mut workspace = Workspace::new(
            vel_text::Text::from("line\n".repeat(1000).as_str()),
            None,
            None,
            false,
        )
        .unwrap();
        workspace.tabs[0].scroll = (0., 7000.);
        workspace.tabs[0].goto(&crate::cli::FileLocation {
            path: PathBuf::from("fixture"),
            line: 120,
            column: 2,
        });
        let mut app = app(workspace);
        for (line, height) in [(120, 800.), (700, 520.)] {
            app.window_size.height = height;
            let active = app.workspace.active;
            app.workspace.tabs[active].goto(&crate::cli::FileLocation {
                path: PathBuf::from("fixture"),
                line,
                column: 2,
            });
            app.scene();
            let rect = app
                .view
                .rect_at(app.workspace.editor(), app.workspace.editor().caret);
            assert!(
                (rect.y + rect.height / 2. - (app.view.tab_height + app.view.size.height) / 2.)
                    .abs()
                    < 0.01
            );
            assert!(!app.workspace.take_pending_center());
        }
    }
}
