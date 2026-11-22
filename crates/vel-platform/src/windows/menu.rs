//! Per-window HMENU trees. Only snapshot app state before entering a native
//! tracking loop, and copy the command payload before queuing it.
use super::{Event, Host, WindowHandle, wide};
use crate::{AppHandler, MenuEntry, requests::MenuCommand};
use std::{
    collections::HashMap,
    ptr::{null, null_mut},
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Named(MenuCommand),
    Minimize,
    Zoom,
    BringAll,
    Quit,
    About(String),
}

#[derive(Debug, PartialEq, Eq)]
struct Item {
    label: String,
    action: Option<Action>,
    enabled: bool,
    checked: bool,
    children: Vec<Item>,
}

fn accelerator(chord: &str) -> String {
    let (modifiers, key) = chord.rsplit_once('+').unwrap_or(("", chord));
    let mut shown = Vec::new();
    for modifier in modifiers.split('+') {
        let label = match modifier {
            "super" | "command" | "cmd" | "control" | "ctrl" | "primary" => "Ctrl",
            "alt" | "option" => "Alt",
            "shift" => "Shift",
            "" => continue,
            _ => modifier,
        };
        if !shown.contains(&label) {
            shown.push(label);
        }
    }
    let key = match key {
        "enter" => "Enter".into(),
        "escape" => "Esc".into(),
        "backspace" => "Backspace".into(),
        "space" => "Space".into(),
        "tab" => "Tab".into(),
        "delete" => "Del".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        "left" => "Left".into(),
        "right" => "Right".into(),
        "up" => "Up".into(),
        "down" => "Down".into(),
        "pageup" => "PgUp".into(),
        "pagedown" => "PgDn".into(),
        "" => "+".into(),
        _ => key.to_uppercase(),
    };
    shown.push(&key);
    shown.join("+")
}

fn key_hint(hint: &str) -> String {
    hint.split(", ")
        .map(|chord| {
            // The shared hint formatter uses Super on Windows for the logical
            // command bit. Unlike a MenuEntry shortcut, these labels are cased.
            chord.replace("Super+", "Ctrl+").replace("Cmd+", "Ctrl+")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn label(caption: &str) -> String {
    // Literal ampersands in translated captions, file names and command labels.
    caption.replace('&', "&&").replace(['\t', '\n', '\r'], " ")
}

fn model(app: &dyn AppHandler, entries: &[MenuEntry]) -> Vec<Item> {
    let mut result = Vec::<Item>::new();
    for entry in entries {
        if entry.caption == "-" {
            if result.last().is_some_and(|item| item.label != "-") {
                result.push(Item {
                    label: "-".into(),
                    action: None,
                    enabled: false,
                    checked: false,
                    children: Vec::new(),
                });
            }
            continue;
        }
        let caption = app.menu_caption(&entry.id).unwrap_or(&entry.caption);
        let action = match entry.command.as_deref() {
            Some("native_minimize") => Some(Action::Minimize),
            Some("native_zoom") => Some(Action::Zoom),
            Some("native_bring_all_to_front") => Some(Action::BringAll),
            Some("native_quit") => Some(Action::Quit),
            Some("native_about") => Some(Action::About(caption.into())),
            Some("native_services" | "native_hide" | "native_hide_others" | "native_show_all") => {
                continue;
            }
            Some(name) if app.command_visible(name, &entry.args) => {
                Some(Action::Named(MenuCommand {
                    name: name.into(),
                    args: entry.args.clone(),
                }))
            }
            Some(_) => continue,
            None => None,
        };
        let children = model(app, &entry.children);
        if children.is_empty() && action.is_none() {
            continue;
        }
        let (enabled, checked) = match &action {
            Some(Action::Named(command)) => (
                app.command_enabled(&command.name, &command.args),
                app.command_checked(&command.name, &command.args),
            ),
            _ => (true, false),
        };
        let mut label = label(caption);
        if let Some(shortcut) = &entry.shortcut {
            label.push('\t');
            label.push_str(&accelerator(shortcut));
        } else if let Some(hint) = &entry.shortcut_hint {
            label.push('\t');
            label.push_str(&key_hint(hint));
        }
        result.push(Item {
            label,
            action,
            enabled,
            checked,
            children,
        });
    }
    if result.last().is_some_and(|item| item.label == "-") {
        result.pop();
    }
    result
}

#[derive(Default)]
struct Actions {
    next: u16,
    table: HashMap<u16, Action>,
}

impl Actions {
    fn record(&mut self, action: Action) -> Option<u16> {
        if self.table.len() >= usize::from(u16::MAX) {
            return None;
        }
        loop {
            self.next = self.next.wrapping_add(1).max(1);
            if let std::collections::hash_map::Entry::Vacant(slot) = self.table.entry(self.next) {
                slot.insert(action);
                return Some(self.next);
            }
        }
    }
}

struct OwnedMenu(HMENU);

impl OwnedMenu {
    fn new(popup: bool) -> Option<Self> {
        // SAFETY: These calls return a new owned menu or null.
        let menu = unsafe {
            if popup {
                CreatePopupMenu()
            } else {
                CreateMenu()
            }
        };
        (!menu.is_null()).then(|| Self(menu))
    }

    fn add_popup(&self, label: &str, popup: Self) -> Option<HMENU> {
        let handle = popup.0;
        let title = wide(label);
        // SAFETY: Both handles are valid. AppendMenu copies this NUL-terminated
        // string and transfers ownership of the child on success.
        if unsafe {
            AppendMenuW(
                self.0,
                MF_POPUP | MF_STRING,
                handle as usize,
                title.as_ptr(),
            )
        } == 0
        {
            return None;
        }
        std::mem::forget(popup);
        Some(handle)
    }
}

impl Drop for OwnedMenu {
    fn drop(&mut self) {
        // SAFETY: Owner of this menu and all still-attached children.
        unsafe { DestroyMenu(self.0) };
    }
}

fn populate(menu: HMENU, items: Vec<Item>, actions: &mut Actions, ids: &mut Vec<u16>) {
    for item in items {
        if item.label == "-" {
            // SAFETY: A separator requires no caption or item ID.
            unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, null()) };
        } else if !item.children.is_empty() {
            let Some(popup) = OwnedMenu::new(true) else {
                continue;
            };
            let title = wide(&item.label);
            // SAFETY: Captions are copied; the parent owns the child on success.
            if unsafe { AppendMenuW(menu, MF_POPUP | MF_STRING, popup.0 as usize, title.as_ptr()) }
                != 0
            {
                let child = popup.0;
                std::mem::forget(popup);
                populate(child, item.children, actions, ids);
            }
        } else if let Some(id) = item.action.and_then(|action| actions.record(action)) {
            let title = wide(&item.label);
            let flags = MF_STRING
                | if item.enabled { MF_ENABLED } else { MF_GRAYED }
                | if item.checked {
                    MF_CHECKED
                } else {
                    MF_UNCHECKED
                };
            // SAFETY: menu is live, and AppendMenuW copies the caption.
            if unsafe { AppendMenuW(menu, flags, usize::from(id), title.as_ptr()) } != 0 {
                ids.push(id);
            } else {
                actions.table.remove(&id);
            }
        }
    }
}

struct Popup {
    native: HMENU,
    entries: Vec<MenuEntry>,
    ids: Vec<u16>,
}

pub(super) struct MenuBar {
    root: OwnedMenu,
    popups: Vec<Popup>,
    actions: Actions,
}

impl MenuBar {
    pub(super) fn handle(&self) -> HMENU {
        self.root.0
    }

    fn new(entries: &[MenuEntry], app: &dyn AppHandler) -> Option<Self> {
        let mut menu = Self {
            root: OwnedMenu::new(false)?,
            popups: Vec::new(),
            actions: Actions::default(),
        };
        let mut mnemonics = Vec::new();
        for entry in entries {
            if entry.children.is_empty() {
                continue;
            }
            let mut title = label(&entry.caption);
            if let Some((index, letter)) = title.char_indices().find(|(_, ch)| {
                ch.is_ascii_alphabetic() && !mnemonics.contains(&ch.to_ascii_lowercase())
            }) {
                title.insert(index, '&');
                mnemonics.push(letter.to_ascii_lowercase());
            }
            if let Some(native) =
                OwnedMenu::new(true).and_then(|popup| menu.root.add_popup(&title, popup))
            {
                let mut ids = Vec::new();
                populate(
                    native,
                    model(app, &entry.children),
                    &mut menu.actions,
                    &mut ids,
                );
                menu.popups.push(Popup {
                    native,
                    entries: entry.children.clone(),
                    ids,
                });
            }
        }
        Some(menu)
    }

    fn refresh(&mut self, popup: HMENU, app: &dyn AppHandler) {
        let Some(branch) = self.popups.iter_mut().find(|branch| branch.native == popup) else {
            return;
        };
        let items = model(app, &branch.entries);
        // SAFETY: This is the top-level popup being initialized, not the menu bar.
        // DeleteMenu also destroys each removed child popup. Refilling the same
        // HMENU preserves the one that the native menu tracker is about to open.
        unsafe {
            let count = GetMenuItemCount(popup);
            for _ in 0..count {
                if DeleteMenu(popup, 0, MF_BYPOSITION) == 0 {
                    return;
                }
            }
        }
        for id in branch.ids.drain(..) {
            self.actions.table.remove(&id);
        }
        populate(popup, items, &mut self.actions, &mut branch.ids);
    }
}

impl Host {
    pub(super) fn update_menu_bar(&self) {
        if self.menu_tracking.get() || !self.menu_dirty.get() || self.closing.get() {
            return;
        }
        let Ok(slot) = self.app.try_borrow() else {
            return;
        };
        let Some(app) = slot.as_deref() else { return };
        let entries = self.menu_entries.borrow();
        let next = if entries.is_empty() {
            None
        } else {
            MenuBar::new(&entries, app)
        };
        drop(entries);
        drop(slot);
        self.menu_dirty.set(false);
        let hwnd = self.hwnd.get();
        if self.fullscreen.get().is_none() {
            let menu = next.as_ref().map_or(null_mut(), MenuBar::handle);
            // SAFETY: The HWND and new menu are live. The old menu is dropped
            // only after it is no longer attached to the HWND.
            if unsafe { SetMenu(hwnd, menu) } == 0 {
                return;
            }
        }
        self.menu.replace(next);
        // SAFETY: Redraw the menu of our live window; this may cause a client resize.
        unsafe { DrawMenuBar(hwnd) };
        self.sync_geometry(true);
    }

    pub(super) fn refresh_popup(&self, popup: HMENU) {
        if let Ok(slot) = self.app.try_borrow()
            && let Some(app) = slot.as_deref()
            && let Ok(mut menu) = self.menu.try_borrow_mut()
            && let Some(menu) = menu.as_mut()
        {
            menu.refresh(popup, app);
        }
    }

    pub(super) fn menu_action(&self, id: u16) -> Option<Action> {
        self.menu
            .try_borrow()
            .ok()?
            .as_ref()?
            .actions
            .table
            .get(&id)
            .cloned()
    }

    pub(super) fn run_menu_action(
        &self,
        app: &mut dyn AppHandler,
        window: &WindowHandle,
        action: Action,
    ) {
        match action {
            Action::Named(command) => {
                if app.command_visible(&command.name, &command.args)
                    && app.command_enabled(&command.name, &command.args)
                {
                    app.on_named_command(window, &command.name, &command.args);
                }
            }
            Action::Minimize => window.minimize(),
            Action::Zoom => window.zoom(),
            Action::Quit => window.request_app_quit(),
            Action::About(title) => {
                let title = wide(&title);
                // SAFETY: The dialog copies the terminated title and uses our live parent.
                unsafe {
                    MessageBoxW(
                        self.hwnd.get(),
                        windows_sys::w!("Velocity"),
                        title.as_ptr(),
                        MB_OK | MB_ICONINFORMATION,
                    )
                };
            }
            Action::BringAll => {
                if let Some(runtime) = self.runtime.upgrade() {
                    let windows = runtime.windows.borrow().clone();
                    for host in windows.iter().filter(|host| !host.closing.get()) {
                        // SAFETY: hwnd belongs to this runtime; do not steal focus
                        // repeatedly while bringing the other editor windows forward.
                        unsafe {
                            SetWindowPos(
                                host.hwnd.get(),
                                HWND_TOP,
                                0,
                                0,
                                0,
                                0,
                                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                            )
                        };
                    }
                    window.focus();
                }
            }
        }
        window.request_frame();
    }

    pub(super) fn track_pending_popup(&self) {
        let busy =
            self.menu_tracking.get() || self.pressed.get() != 0 || self.app.try_borrow().is_err();
        let request = self.context.borrow_mut().take_if_idle(busy);
        let Some((entries, at)) = request else { return };
        let items = self
            .app
            .borrow()
            .as_deref()
            .map(|app| model(app, &entries))
            .unwrap_or_default();
        if items.is_empty() {
            return;
        }
        let Some(popup) = OwnedMenu::new(true) else {
            return;
        };
        let mut actions = Actions::default();
        populate(popup.0, items, &mut actions, &mut Vec::new());
        let hwnd = self.hwnd.get();
        // SAFETY: Both handles remain live during the modal call. No app or menu
        // state is borrowed across tracking; the popup owns its action snapshot.
        let id = unsafe {
            TrackPopupMenuEx(
                popup.0,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                at.x,
                at.y,
                hwnd,
                null(),
            )
        };
        if let Some(action) = u16::try_from(id)
            .ok()
            .and_then(|id| actions.table.remove(&id))
        {
            self.enqueue(Event::Named(action));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FrameContext;

    struct TestApp;
    impl AppHandler for TestApp {
        fn on_frame(&mut self, _: &mut FrameContext<'_>) -> Result<(), String> {
            Ok(())
        }
        fn command_enabled(&self, name: &str, _: &str) -> bool {
            name != "copy"
        }
        fn command_visible(&self, name: &str, _: &str) -> bool {
            name != "hidden"
        }
        fn command_checked(&self, name: &str, _: &str) -> bool {
            name == "copy"
        }
    }

    #[test]
    fn menu_preserves_args_labels_and_validation() {
        let sep = MenuEntry {
            caption: "-".into(),
            ..Default::default()
        };
        let items = model(
            &TestApp,
            &[
                sep.clone(),
                MenuEntry {
                    caption: "Copy & Keep".into(),
                    command: Some("copy".into()),
                    args: r#"{"x":3}"#.into(),
                    shortcut: Some("super+shift+c".into()),
                    ..Default::default()
                },
                MenuEntry {
                    caption: "Hidden".into(),
                    command: Some("hidden".into()),
                    ..Default::default()
                },
                MenuEntry {
                    caption: "Zoom".into(),
                    command: Some("native_zoom".into()),
                    ..Default::default()
                },
                sep,
            ],
        );
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "Copy && Keep\tCtrl+Shift+C");
        assert!(!items[0].enabled && items[0].checked);
        assert_eq!(
            items[0].action,
            Some(Action::Named(MenuCommand {
                name: "copy".into(),
                args: r#"{"x":3}"#.into()
            }))
        );
        assert_eq!(items[1].action, Some(Action::Zoom));
        assert_eq!(accelerator("control+super+pageup"), "Ctrl+PgUp");
    }
}
