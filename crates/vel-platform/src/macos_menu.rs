//! Native context menus hold snapshots; AppKit tracking holds no host/app borrow.
use super::*;
use crate::requests::{MenuCommand, MenuSnapshot, menu_snapshot};

struct CommandIvars {
    command: MenuCommand,
    selected: Rc<RefCell<Option<MenuCommand>>>,
}
define_class!(
    // SAFETY: NSObject has no extra subclassing invariants; actions run on the main thread.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = CommandIvars]
    struct ContextCommand;
    // SAFETY: NSObjectProtocol has no additional requirements.
    unsafe impl NSObjectProtocol for ContextCommand {}
    impl ContextCommand {
        #[unsafe(method(selectContextCommand:))]
        fn select(&self, _sender: Option<&AnyObject>) {
            *self.ivars().selected.borrow_mut() = Some(self.ivars().command.clone());
        }
    }
);

fn append(
    mtm: MainThreadMarker,
    menu: &NSMenu,
    entries: &[MenuSnapshot],
    selected: &Rc<RefCell<Option<MenuCommand>>>,
    targets: &mut Vec<Retained<ContextCommand>>,
) {
    menu.setAutoenablesItems(false);
    for entry in entries {
        if entry.caption == "-" {
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            continue;
        }
        // SAFETY: selectContextCommand: implements the documented one-sender action.
        let item = unsafe {
            menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str(&entry.caption),
                entry.command.as_ref().map(|_| sel!(selectContextCommand:)),
                &NSString::from_str(""),
            )
        };
        item.setEnabled(entry.enabled);
        item.setState(if entry.checked {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        if let Some(command) = &entry.command {
            let target = ContextCommand::alloc(mtm).set_ivars(CommandIvars {
                command: command.clone(),
                selected: Rc::clone(selected),
            });
            // SAFETY: NSObject initialization after installing all Rust ivars.
            let target: Retained<ContextCommand> = unsafe { msg_send![super(target), init] };
            // SAFETY: targets retains this action receiver until tracking ends and
            // the menu is torn down; no view or host is borrowed by the action.
            unsafe {
                item.setTarget(Some(&*target));
            }
            targets.push(target);
        }
        if !entry.children.is_empty() {
            let child = NSMenu::new(mtm);
            append(mtm, &child, &entry.children, selected, targets);
            item.setSubmenu(Some(&child));
        }
    }
}

pub(super) fn popup(host: &Rc<Host>, entries: &[MenuEntry], event: &NSEvent) {
    let snapshot = {
        let slot = host.app.borrow();
        let Some(app) = slot.as_ref() else {
            return;
        };
        menu_snapshot(&**app, entries)
    };
    let Some(view) = host.view.get() else {
        return;
    };
    let menu = NSMenu::new(host.mtm);
    let selected = Rc::new(RefCell::new(None));
    let mut targets = Vec::new();
    append(host.mtm, &menu, &snapshot, &selected, &mut targets);
    if menu.numberOfItems() > 0 {
        // Nested native tracking begins here. There is no app, host-state RefCell,
        // command-map, or snapshot borrow across this call. Targets only save a
        // command; app dispatch occurs after tracking has completely returned.
        NSMenu::popUpContextMenu_withEvent_forView(&menu, event, view);
    }
    drop(menu);
    drop(targets);
    let command = selected.borrow_mut().take();
    if let Some(command) = command {
        host.dispatch(Event::NamedCommand(command.name, command.args));
        host.flush();
    }
    lifecycle::schedule();
}

pub(super) fn update_menu_bar(host: &Rc<Host>, menu: &NSMenu) {
    let states = {
        let Ok(slot) = host.app.try_borrow() else {
            return;
        };
        let Some(app) = slot.as_ref() else {
            return;
        };
        let commands = host.menu_commands.borrow();
        menu.itemArray()
            .iter()
            .filter_map(|item| {
                if item.action() != Some(sel!(velocityCommand:)) {
                    return None;
                }
                let index = usize::try_from(item.tag()).ok()?;
                let (name, args, _) = commands.get(index)?;
                Some((
                    item,
                    app.command_enabled(name, args),
                    app.command_visible(name, args),
                    app.command_checked(name, args),
                ))
            })
            .collect::<Vec<_>>()
    };
    for (item, enabled, visible, checked) in states {
        item.setEnabled(enabled);
        item.setHidden(!visible);
        item.setState(if checked {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
    }
}
