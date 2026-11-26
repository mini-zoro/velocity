//! Composition follows a text field or editor sheet, not the native view they share.
use super::*;
use vel_buffer::BufferId;
use vel_ui::input::Input;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Owner {
    Field(BufferId),
    Panel(
        BufferId,
        Option<crate::panels::Panel>,
        Option<crate::panels::Overlay>,
    ),
    Editor(u64),
}

impl App {
    pub(super) fn editor_ime_anchor(&self, mut rect: Rect) -> Rect {
        rect.x += self.content.x;
        rect.y += self.content.y;
        rect.ime_anchor_in(self.content)
    }
    pub(super) fn ime_field(&self) -> Option<&Input> {
        if let Some(rename) = &self.tab_rename {
            return Some(&rename.input);
        }
        if self.settings_page_has_focus() {
            return self.workspace.settings_page.as_ref()?.input();
        }
        if self
            .accessibility_focus
            .is_some_and(|id| self.panels.accessibility_input(id).is_none())
        {
            return None;
        }
        self.panels.input()
    }

    pub(super) fn ime_editor_has_focus(&self) -> bool {
        self.tab_rename.is_none()
            && !self.settings_page_has_focus()
            && !self.panels.captured()
            && self.accessibility_focus.is_none()
    }

    pub(super) fn composing_ime_owner(&self) -> Option<Owner> {
        if let Some(field) = self.ime_field().filter(|f| f.composing()) {
            let id = field.buffer.id();
            return Some(
                if self.tab_rename.is_some() || self.settings_page_has_focus() {
                    Owner::Field(id)
                } else {
                    Owner::Panel(id, self.panels.panel, self.panels.overlay)
                },
            );
        }
        let tab = &self.workspace.tabs[self.workspace.active];
        (self.ime_editor_has_focus()
            && !tab.editor.read_only
            && !tab.editor.command_mode()
            && tab.editor.marked.is_some())
        .then_some(Owner::Editor(tab.sheet))
    }

    pub(super) fn reconcile_ime_focus(&mut self, window: Option<&WindowHandle>) {
        if let Some(owner) = self.ime_owner
            && Some(owner) != self.composing_ime_owner()
        {
            self.ime_owner = None;
            match owner {
                Owner::Editor(sheet) => {
                    if let Some(i) = self.workspace.sheet_index(sheet) {
                        let editor = &mut self.workspace.tabs[i].editor;
                        if editor.marked.is_some() {
                            editor.cancel_composition();
                        }
                    }
                }
                Owner::Panel(id, ..) => {
                    self.panels.discard_field_composition(id);
                }
                Owner::Field(id) => {
                    if let Some(page) = &mut self.workspace.settings_page {
                        page.discard_field_composition(id);
                    }
                }
            }
            if let Some(window) = window {
                window.discard_marked_text();
            }
        }
    }

    /// Editors and panels drop provisional input on blur. Inline tab rename
    /// preserves its existing accept-on-blur behavior.
    pub(super) fn finish_ime_focus(&mut self, window: Option<&WindowHandle>) {
        self.reconcile_ime_focus(window);
        if self.composing_ime_owner().is_some() {
            let event = if self.tab_rename.is_some() {
                TextInput::Unmark
            } else {
                TextInput::Discard
            };
            self.handle_text_input(window, event);
            if let Some(window) = window {
                window.discard_marked_text();
            }
        }
    }

    /// Only used if the input method falls back to a standard selector. The
    /// keystroke must never also activate a panel, edit, or Vintage command.
    pub(super) fn ime_command(&mut self, window: Option<&WindowHandle>, command: Command) -> bool {
        if self.composing_ime_owner().is_none() {
            return false;
        }
        let event = match command {
            Command::Newline | Command::LiteralNewline => TextInput::Unmark,
            Command::Cancel => TextInput::Discard,
            _ => return true,
        };
        self.handle_text_input(window, event);
        if let Some(window) = window {
            window.discard_marked_text();
        }
        true
    }
}
