//! Tab closing uses stable sheets and the same per-document save prompt everywhere.
use super::*;
#[derive(Debug)]
pub(super) enum CloseDecision {
    Save(Option<std::path::PathBuf>),
    Discard,
    Cancel,
}
impl App {
    pub(super) fn apply_save_action(
        &mut self,
        window: Option<&WindowHandle>,
        name: &str,
        args: &Value,
    ) -> Result<(), String> {
        let index = self.workspace.active;
        let path = args["file"].as_str().map(std::path::PathBuf::from);
        let saved = match self.save_tab_with_path(window, index, false, path) {
            Ok(saved) => saved,
            Err(error) => {
                self.panels.message = error;
                false
            }
        };
        if saved && name == "save_and_close" {
            self.workspace.close(index)?;
            self.close_empty_window(window);
        }
        Ok(())
    }
    pub(super) fn is_tab_close(name: &str) -> bool {
        matches!(
            name,
            "close"
                | "close_file"
                | "close_by_index"
                | "close_other_tabs"
                | "close_others_by_index"
                | "close_tabs_to_right"
                | "close_to_right_by_index"
                | "close_unmodified_tabs"
                | "close_unmodified"
                | "close_deleted_files"
                | "close_selected"
                | "close_selected_tabs"
                | "close_all"
        )
    }
    pub(super) fn tab_close_sheets(&self, name: &str, args: &Value) -> Vec<u64> {
        if self.workspace.is_empty() {
            return vec![];
        }
        if matches!(name, "close_deleted_files" | "close_all") {
            return self
                .workspace
                .tabs
                .iter()
                .rev()
                .filter(|tab| {
                    name == "close_all"
                        || tab
                            .path
                            .as_ref()
                            .is_some_and(|path| matches!(path.try_exists(), Ok(false)))
                })
                .map(|tab| tab.sheet)
                .collect();
        }
        let clicked = args["_context_sheet"].as_u64();
        let group = clicked
            .and_then(|id| self.workspace.workbench.group_of(id))
            .or_else(|| args["group"].as_u64().map(|g| g as usize))
            .unwrap_or(self.workspace.workbench.active);
        let Some(group) = self.workspace.workbench.groups.get(group) else {
            return vec![];
        };
        let index = if let Some(clicked) = clicked {
            group.sheets.iter().position(|id| *id == clicked)
        } else if let Some(index) = args["index"].as_u64() {
            usize::try_from(index)
                .ok()
                .filter(|i| *i < group.sheets.len())
        } else {
            group
                .focused
                .and_then(|id| group.sheets.iter().position(|s| *s == id))
        };
        let Some(index) = index else {
            return vec![];
        };
        if let Some(kind) = crate::context_menu::TabCloseKind::from_command(name)
            && kind != crate::context_menu::TabCloseKind::Current
        {
            let tabs = group
                .sheets
                .iter()
                .filter_map(|id| {
                    let tab = &self.workspace.tabs[self.workspace.sheet_index(*id)?];
                    Some(crate::context_menu::TabSnapshot {
                        sheet: *id,
                        dirty: self.workspace.document_dirty(tab),
                    })
                })
                .collect::<Vec<_>>();
            return crate::context_menu::tab_close_targets(&tabs, index, kind);
        }
        let sheet = group.sheets[index];
        // close_file closes the active view. Only Close Selected closes a set;
        // multi-selection does not change the meaning of a tab's close button.
        // https://docs.sublimetext.io/reference/commands.html#close-file
        group
            .sheets
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, id)| match name {
                "close_selected" | "close_selected_tabs" => group.selected.contains(id),
                _ => **id == sheet,
            })
            .map(|(_, id)| *id)
            .collect()
    }
    fn can_close_tab(
        &mut self,
        window: Option<&WindowHandle>,
        index: usize,
        force: bool,
        contents_only: bool,
    ) -> Result<bool, String> {
        if force {
            return Ok(true);
        }
        if self
            .workspace
            .tabs
            .iter()
            .filter(|t| t.session_document == self.workspace.tabs[index].session_document)
            .count()
            > 1
        {
            return Ok(true);
        }
        self.decide_close_document(window, index, contents_only)
    }
    pub(super) fn apply_tab_close_action(
        &mut self,
        window: Option<&WindowHandle>,
        name: &str,
        args: &Value,
    ) -> Result<bool, String> {
        if !Self::is_tab_close(name) {
            return Ok(false);
        }
        self.sync_workspace_after_panels();
        let sheets = self.tab_close_sheets(name, args);
        for sheet in sheets {
            let Some(index) = self.workspace.sheet_index(sheet) else {
                continue;
            };
            let close = self.can_close_tab(
                window,
                index,
                args["force"].as_bool().unwrap_or(false),
                name == "close_deleted_files",
            );
            match close {
                Ok(true) => {}
                Ok(false) => break,
                Err(error) => {
                    self.panels.message = error;
                    break;
                }
            }
            // Close immediately after each decision. Cancellation stops this whole
            // operation, preserving every tab that has not yet been prompted.
            self.workspace.close(index)?;
        }
        self.close_empty_window(window);
        Ok(true)
    }
}
