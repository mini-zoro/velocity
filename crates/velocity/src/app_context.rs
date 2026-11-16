//! Native menu orchestration. Tracking starts only after the platform releases App.
use super::*;
use crate::context_menu::{
    self as menus, ContextMenuKind, MenuContext, SidebarEffect, TabCloseKind, TabSnapshot,
};
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
    fn context_tab(&self, args: &Value) -> Option<usize> {
        if let Some(sheet) = args["_context_sheet"].as_u64() {
            return self.workspace.sheet_index(sheet);
        }
        let group = args["group"].as_u64()? as usize;
        let index = args["index"].as_u64()? as usize;
        let sheet = self
            .workspace
            .workbench
            .groups
            .get(group)?
            .sheets
            .get(index)?;
        self.workspace.sheet_index(*sheet)
    }
    fn context_close_targets(&self, name: &str, args: &Value) -> Vec<usize> {
        let Some(kind) = TabCloseKind::from_command(name) else {
            return vec![];
        };
        let Some(index) = self.context_tab(args) else {
            return vec![];
        };
        let sheet = self.workspace.tabs[index].sheet;
        let Some(group) = self.workspace.workbench.group_of(sheet) else {
            return vec![];
        };
        let group = &self.workspace.workbench.groups[group];
        let Some(clicked) = group.sheets.iter().position(|id| *id == sheet) else {
            return vec![];
        };
        let tabs: Vec<_> = group
            .sheets
            .iter()
            .filter_map(|sheet| {
                let tab = &self.workspace.tabs[self.workspace.sheet_index(*sheet)?];
                Some(TabSnapshot {
                    sheet: *sheet,
                    dirty: tab.dirty(),
                })
            })
            .collect();
        menus::tab_close_targets(&tabs, clicked, kind)
            .into_iter()
            .filter_map(|sheet| self.workspace.sheet_index(sheet))
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
        if TabCloseKind::from_command(name).is_some()
            && (args["_context_menu"] == "tab" || menus::COMMANDS.contains(&name))
        {
            return Some(vel_config::ui::CommandState {
                enabled: !self.context_close_targets(name, args).is_empty(),
                ..Default::default()
            });
        }
        None
    }
    /// Collects context without opening AppKit's nested tracking loop while borrowed.
    pub(super) fn begin_context_menu(&mut self, w: &WindowHandle, point: Point) {
        self.panels.review.native_context = true;
        self.panels.cancel_review_menu();
        self.pending_menu = None;
        if self.panels.overlay.is_some() {
            return;
        }
        let group = self.workspace.workbench.active;
        let mut context = MenuContext {
            event: point,
            group,
            ..Default::default()
        };
        let kind;
        let sheet;
        if let Some((id, _)) = self
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
            kind = ContextMenuKind::SideBar;
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
        self.changed(w, Instant::now(), false);
        self.flush_context_menu(w);
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
        let pending = self.pending_menu.take().expect("pending context menu");
        let dynamic = std::mem::take(&mut self.panels.review.items);
        self.panels.cancel_review_menu();
        let resources = self
            .workspace
            .context_menus
            .get(pending.kind.resource_name())
            .map(Vec::as_slice)
            .unwrap_or_default();
        let menu = menus::with_dynamic(resources, &dynamic);
        let mut context = pending.context.arguments();
        context["_context_menu"] = json!(match pending.kind {
            ContextMenuKind::Editor => "editor",
            ContextMenuKind::Tab => "tab",
            ContextMenuKind::SideBar => "sidebar",
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
        fn native(rows: Vec<vel_config::ui::ResolvedMenuItem>) -> Vec<MenuEntry> {
            rows.into_iter()
                .map(|row| MenuEntry {
                    caption: row.caption,
                    command: row.invocation.as_ref().map(|i| i.command.clone()),
                    args: row
                        .invocation
                        .map_or_else(|| "{}".into(), |i| i.args.to_string()),
                    children: native(row.children),
                })
                .collect()
        }
        w.show_context_menu(&native(rows));
    }
    /// Returns true only for a command fully handled at window scope.
    pub(super) fn context_action(
        &mut self,
        w: &WindowHandle,
        name: &str,
        args: &Value,
    ) -> Result<bool, String> {
        if TabCloseKind::from_command(name).is_some()
            && (args["_context_menu"] == "tab" || menus::COMMANDS.contains(&name))
        {
            let mut targets = self.context_close_targets(name, args);
            if self.can_close_many(w, &targets) {
                targets.sort_unstable();
                targets.dedup();
                // Same rule as `close`: closing every tab may close the window.
                let all = !targets.is_empty() && targets.len() == self.workspace.tabs.len();
                for index in targets.into_iter().rev() {
                    self.workspace.close(index)?;
                }
                if all
                    && self
                        .workspace
                        .editor()
                        .settings
                        .bool("close_windows_when_empty", false)
                {
                    w.request_close();
                }
            }
            return Ok(true);
        }
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
            SidebarEffect::Input(invocation) => {
                let viewport = self.viewport();
                self.panels.action(
                    &mut self.workspace,
                    &invocation.command,
                    &invocation.args,
                    viewport,
                )?;
            }
            SidebarEffect::ConfirmDelete { paths, on_confirm } => {
                let text = format!(
                    "Move these entries to the Trash?\n{}",
                    paths
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n")
                );
                if w.alert("Move to Trash?", &text, &["Cancel", "Move to Trash"]) == 1 {
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
                menus::apply_delete(&mut self.workspace, &paths);
                self.panels.refresh_sidebar(&mut self.workspace);
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
                path: file.with_file_name("missing"),
                line: 1,
                column: 1,
            },
        ];
        let (stream, mut client) = UnixStream::pair().unwrap();
        app.accept_request(crate::instance::Incoming { request, stream })
            .unwrap();
        let mut ack = [0];
        client.read_exact(&mut ack).unwrap();
        assert_eq!(ack, [1]);
        assert_eq!(app.workspace.tabs.len(), 1);
        assert_eq!(app.workspace.editor().caret, 5);
        assert!(!app.workspace.take_pending_center());
        assert!(app.panels.message.contains("missing"));
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
