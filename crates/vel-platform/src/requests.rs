//! Headless routing and snapshots shared by the native bridge and its tests.
#[cfg(any(target_os = "macos", test))]
use crate::AppHandler;
use crate::MenuEntry;
#[cfg(any(target_os = "macos", test))]
use std::{collections::VecDeque, path::PathBuf};

/// A live host; stopped hosts are excluded before routing.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OpenHost {
    pub id: u64,
    pub ready: bool,
    pub key: bool,
}

pub(crate) fn open_target(hosts: &[OpenHost]) -> Option<u64> {
    hosts
        .iter()
        .find(|host| host.ready && host.key)
        .or_else(|| hosts.iter().rev().find(|host| host.ready))
        .map(|host| host.id)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(any(target_os = "macos", test))]
pub(crate) enum ReopenPlan {
    Focus(u64),
    WaitForStartup,
    CreateWindow,
}
#[cfg(any(target_os = "macos", test))]
pub(crate) fn reopen_plan(hosts: &[OpenHost]) -> ReopenPlan {
    if let Some(id) = open_target(hosts) {
        ReopenPlan::Focus(id)
    } else if hosts.is_empty() {
        ReopenPlan::CreateWindow
    } else {
        ReopenPlan::WaitForStartup
    }
}

#[derive(Debug, PartialEq, Eq)]
#[cfg(any(target_os = "macos", test))]
pub(crate) enum NativeOpenRequest {
    Paths(Vec<PathBuf>),
    FinderNewTextFile(PathBuf),
}

#[derive(Debug, Default)]
#[cfg(any(target_os = "macos", test))]
pub(crate) struct OpenRequests(VecDeque<NativeOpenRequest>);
#[cfg(any(target_os = "macos", test))]
impl OpenRequests {
    pub fn push(&mut self, paths: Vec<PathBuf>) {
        if !paths.is_empty() {
            self.0.push_back(NativeOpenRequest::Paths(paths));
        }
    }
    pub fn push_finder_new_text_file(&mut self, directory: PathBuf) {
        self.0
            .push_back(NativeOpenRequest::FinderNewTextFile(directory));
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn pop_finder_for_closed_window(&mut self) -> Option<PathBuf> {
        // A secondary native launch may relay path opens to a different copy of
        // the app. Give a bundle-local request first chance to open a window here.
        let index = self
            .0
            .iter()
            .position(|request| matches!(request, NativeOpenRequest::FinderNewTextFile(_)))?;
        match self.0.remove(index)? {
            NativeOpenRequest::FinderNewTextFile(directory) => Some(directory),
            NativeOpenRequest::Paths(_) => unreachable!(),
        }
    }
    /// Coalesce adjacent open-path batches in arrival order before invoking the
    /// destination. A LaunchServices proxy may forward once and quit immediately
    /// after acknowledgement. Finder creation is a separate, ordered action.
    /// Without a ready destination, every batch stays queued.
    pub fn pop(&mut self, hosts: &[OpenHost]) -> Option<(u64, NativeOpenRequest)> {
        let id = open_target(hosts)?;
        let mut request = self.0.pop_front()?;
        if let NativeOpenRequest::Paths(paths) = &mut request {
            while let Some(NativeOpenRequest::Paths(_)) = self.0.front() {
                if let Some(NativeOpenRequest::Paths(batch)) = self.0.pop_front() {
                    paths.extend(batch);
                }
            }
        }
        Some((id, request))
    }
}

/// A menu owns its command payload, independently of the menu bar's mutable tags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MenuCommand {
    pub name: String,
    pub args: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg(any(target_os = "macos", test))]
pub(crate) struct MenuSnapshot {
    pub caption: String,
    pub command: Option<MenuCommand>,
    pub enabled: bool,
    pub checked: bool,
    pub children: Vec<MenuSnapshot>,
}

/// Resolve every flag while the app is borrowed, then release it before tracking.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn menu_snapshot(app: &dyn AppHandler, entries: &[MenuEntry]) -> Vec<MenuSnapshot> {
    let mut result = Vec::new();
    for entry in entries {
        if let Some(name) = &entry.command
            && !app.command_visible(name, &entry.args)
        {
            continue;
        }
        let children = menu_snapshot(app, &entry.children);
        if entry.command.is_none() && !entry.children.is_empty() && children.is_empty() {
            continue;
        }
        if entry.caption == "-"
            && result
                .last()
                .is_none_or(|item: &MenuSnapshot| item.caption == "-")
        {
            continue;
        }
        result.push(MenuSnapshot {
            caption: entry.caption.clone(),
            command: entry.command.as_ref().map(|name| MenuCommand {
                name: name.clone(),
                args: entry.args.clone(),
            }),
            enabled: entry
                .command
                .as_ref()
                .is_none_or(|name| app.command_enabled(name, &entry.args)),
            checked: entry
                .command
                .as_ref()
                .is_some_and(|name| app.command_checked(name, &entry.args)),
            children,
        });
    }
    if result.last().is_some_and(|entry| entry.caption == "-") {
        result.pop();
    }
    result
}

#[derive(Debug)]
pub(crate) struct ContextRequest<E> {
    origin: Option<E>,
    dispatching: bool,
    pending: Option<(Vec<MenuEntry>, E)>,
}
impl<E> Default for ContextRequest<E> {
    fn default() -> Self {
        Self {
            origin: None,
            dispatching: false,
            pending: None,
        }
    }
}
impl<E> ContextRequest<E> {
    /// Invalidate delayed worker results and queued native popups together.
    /// Preserve the dispatch guard if cancellation happens inside a callback.
    pub fn cancel(&mut self) {
        self.origin = None;
        self.pending = None;
    }
}
impl<E: Clone> ContextRequest<E> {
    pub fn begin_click(&mut self, event: Option<E>) {
        self.origin = event;
        self.dispatching = true;
        self.pending = None;
    }
    pub fn end_dispatch(&mut self) {
        self.dispatching = false;
    }
    pub fn request(&mut self, entries: &[MenuEntry]) -> bool {
        let Some(event) = self.origin.as_ref().filter(|_| !entries.is_empty()) else {
            return false;
        };
        self.pending = Some((entries.to_vec(), event.clone()));
        true
    }
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
    pub fn take_if_idle(&mut self, busy: bool) -> Option<(Vec<MenuEntry>, E)> {
        if busy || self.dispatching {
            None
        } else {
            let request = self.pending.take();
            if request.is_some() {
                self.origin = None;
            }
            request
        }
    }
}

/// Cache both the owner and menu contents, preserving round-13 installation dedupe.
#[derive(Debug, Default)]
#[cfg(any(target_os = "macos", test))]
pub(crate) struct MenuInstallation(Option<(u64, Vec<MenuEntry>)>);
#[cfg(any(target_os = "macos", test))]
impl MenuInstallation {
    pub fn update(&mut self, owner: u64, entries: &[MenuEntry]) -> bool {
        if self
            .0
            .as_ref()
            .is_some_and(|(id, old)| *id == owner && old == entries)
        {
            return false;
        }
        self.0 = Some((owner, entries.to_vec()));
        true
    }
}

/// Avoids NSString allocation and native title updates for identical requests.
#[derive(Debug)]
pub(crate) struct WindowTitle(String);
impl WindowTitle {
    pub fn new(title: String) -> Self {
        Self(title)
    }
    pub fn update(&mut self, title: &str) -> bool {
        if self.0 == title {
            return false;
        }
        self.0.clear();
        self.0.push_str(title);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FrameContext, Modifiers, MouseEvent, MouseKind, Point};

    #[test]
    fn open_requests_wait_for_start_then_follow_the_current_key_window() {
        let mut requests = OpenRequests::default();
        requests.push(vec!["/file.txt".into(), "/folder".into()]);
        let mut hosts = vec![OpenHost {
            id: 1,
            ready: false,
            key: false,
        }];
        assert!(requests.pop(&[]).is_none());
        assert!(requests.pop(&hosts).is_none());
        hosts[0].ready = true;
        assert_eq!(
            requests.pop(&hosts),
            Some((
                1,
                NativeOpenRequest::Paths(vec!["/file.txt".into(), "/folder".into()])
            ))
        );
        hosts.push(OpenHost {
            id: 2,
            ready: true,
            key: true,
        });
        requests.push(vec!["/other.txt".into()]);
        assert_eq!(
            requests.pop(&hosts),
            Some((2, NativeOpenRequest::Paths(vec!["/other.txt".into()])))
        );
        // No key window: retain a live destination (for example, a minimized window).
        hosts[1].key = false;
        assert_eq!(open_target(&hosts), Some(2));
    }

    #[test]
    fn closed_window_requests_remain_queued_until_factory_host_is_ready() {
        let mut requests = OpenRequests::default();
        requests.push(vec!["/again".into()]);
        requests.push(vec!["/again".into()]);
        assert!(requests.pop(&[]).is_none());
        let fresh = [OpenHost {
            id: 3,
            ready: true,
            key: true,
        }];
        assert_eq!(
            requests.pop(&fresh),
            Some((
                3,
                NativeOpenRequest::Paths(vec!["/again".into(), "/again".into()])
            ))
        );
        // A later explicit open of the same path must still focus its tab.
        requests.push(vec!["/again".into()]);
        assert_eq!(
            requests.pop(&fresh),
            Some((3, NativeOpenRequest::Paths(vec!["/again".into()])))
        );
        requests.push(vec![]);
        assert!(requests.is_empty());
    }

    #[test]
    fn queued_native_batches_are_coalesced_before_a_one_shot_proxy_quits() {
        struct OneShotProxy {
            forwarded: Vec<PathBuf>,
            stopped: bool,
        }
        impl OneShotProxy {
            fn on_open_paths(&mut self, paths: Vec<PathBuf>) {
                assert!(
                    !self.stopped,
                    "proxy cannot accept another callback after acknowledgement"
                );
                self.forwarded = paths;
                self.stopped = true;
            }
        }

        let mut requests = OpenRequests::default();
        requests.push(vec!["/first.txt".into(), "/folder".into()]);
        requests.push(vec!["/second.txt".into(), "/first.txt".into()]);
        requests.push(vec!["/binary".into(), "/huge.txt".into()]);
        assert!(requests.pop(&[]).is_none());
        let mut hosts = [OpenHost {
            id: 1,
            ready: false,
            key: true,
        }];
        assert!(requests.pop(&hosts).is_none());
        hosts[0].ready = true;

        let mut proxy = OneShotProxy {
            forwarded: vec![],
            stopped: false,
        };
        while !proxy.stopped {
            let Some((host, NativeOpenRequest::Paths(paths))) = requests.pop(&hosts) else {
                break;
            };
            assert_eq!(host, 1);
            // Drain before the callback can forward its actual payload and quit.
            assert!(requests.is_empty());
            proxy.on_open_paths(paths);
        }
        assert!(proxy.stopped);
        assert_eq!(
            proxy.forwarded,
            vec![
                PathBuf::from("/first.txt"),
                "/folder".into(),
                "/second.txt".into(),
                "/first.txt".into(),
                "/binary".into(),
                "/huge.txt".into(),
            ]
        );
        assert!(requests.is_empty());
        assert!(requests.pop(&hosts).is_none());

        // Coalescing does not suppress a later explicit open of the same path.
        requests.push(vec!["/first.txt".into()]);
        assert_eq!(
            requests.pop(&hosts),
            Some((1, NativeOpenRequest::Paths(vec!["/first.txt".into()])))
        );
    }

    #[test]
    fn finder_creation_waits_for_the_key_window_or_stays_in_the_closed_copy() {
        let mut requests = OpenRequests::default();
        requests.push(vec!["/normal.txt".into()]);
        requests.push_finder_new_text_file("/first".into());
        requests.push_finder_new_text_file("/second".into());
        assert!(requests.pop(&[]).is_none());
        assert_eq!(
            requests.pop_finder_for_closed_window(),
            Some("/first".into())
        );
        let hosts = [
            OpenHost {
                id: 1,
                ready: true,
                key: true,
            },
            OpenHost {
                id: 2,
                ready: true,
                key: false,
            },
        ];
        assert_eq!(
            requests.pop(&hosts),
            Some((1, NativeOpenRequest::Paths(vec!["/normal.txt".into()])))
        );
        assert_eq!(
            requests.pop(&hosts),
            Some((1, NativeOpenRequest::FinderNewTextFile("/second".into())))
        );
        assert!(requests.is_empty());
    }

    #[test]
    fn reopen_creates_a_fresh_host_only_after_every_window_has_closed() {
        assert_eq!(reopen_plan(&[]), ReopenPlan::CreateWindow);
        let mut host = OpenHost {
            id: 9,
            ready: false,
            key: false,
        };
        assert_eq!(reopen_plan(&[host]), ReopenPlan::WaitForStartup);
        host.ready = true;
        assert_eq!(reopen_plan(&[host]), ReopenPlan::Focus(9));
    }

    struct MenuApp;
    impl AppHandler for MenuApp {
        fn on_frame(&mut self, _: &mut FrameContext<'_>) -> Result<(), String> {
            Ok(())
        }
        fn command_visible(&self, name: &str, _: &str) -> bool {
            name != "hidden"
        }
        fn command_enabled(&self, name: &str, _: &str) -> bool {
            name != "disabled"
        }
        fn command_checked(&self, name: &str, _: &str) -> bool {
            name == "checked"
        }
    }
    fn entry(name: &str) -> MenuEntry {
        MenuEntry {
            caption: name.into(),
            command: Some(name.into()),
            args: r#"{"paths":["/a"],"group":1,"index":2,"event":{"x":3,"y":4}}"#.into(),
            ..Default::default()
        }
    }

    #[test]
    fn menu_flags_and_exact_context_args_are_snapshotted_before_tracking() {
        let entries = [entry("hidden"), entry("disabled"), entry("checked")];
        let snapshot = menu_snapshot(&MenuApp, &entries);
        assert_eq!(snapshot.len(), 2);
        assert!(!snapshot[0].enabled);
        assert!(snapshot[1].checked);
        let command = snapshot[1].command.as_ref().unwrap();
        assert_eq!(command.name, "checked");
        assert_eq!(command.args, entries[2].args);
    }

    #[test]
    fn invisible_submenus_and_stranded_separators_are_removed() {
        let separator = MenuEntry {
            caption: "-".into(),
            ..Default::default()
        };
        let hidden_parent = MenuEntry {
            caption: "parent".into(),
            children: vec![entry("hidden")],
            ..Default::default()
        };
        let snapshot = menu_snapshot(
            &MenuApp,
            &[
                separator.clone(),
                hidden_parent,
                entry("checked"),
                separator,
            ],
        );
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].caption, "checked");
    }

    #[test]
    fn context_tracking_waits_for_dispatch_and_keeps_its_own_commands() {
        let mut request = ContextRequest::default();
        assert!(!request.request(&[entry("copy")]));
        request.begin_click(Some(42));
        let mut entries = vec![entry("copy")];
        assert!(request.request(&entries));
        entries[0].args.clear();
        assert!(request.take_if_idle(false).is_none());
        request.end_dispatch();
        assert!(request.take_if_idle(true).is_none());
        let (snapshot, event) = request.take_if_idle(false).unwrap();
        assert_eq!(event, 42);
        assert_eq!(snapshot[0].args, entry("copy").args);
        assert!(!request.is_pending());
        assert!(!request.request(&[entry("copy")]));
    }

    #[test]
    fn delayed_spelling_menu_retains_origin_until_a_new_primary_or_secondary_press() {
        let mut request = ContextRequest::default();
        request.begin_click(Some(7));
        request.end_dispatch();
        // A worker result is observed by a later frame, long after on_mouse returns.
        assert!(request.request(&[entry("suggestion")]));
        assert!(request.take_if_idle(true).is_none());
        // A primary click cancels the old popup and replaces the originating event.
        request.begin_click(Some(99));
        request.end_dispatch();
        assert!(request.take_if_idle(false).is_none());
        // A primary-click Git badge can also offer a native menu for its own event.
        assert!(request.request(&[entry("git_status")]));
        assert_eq!(request.take_if_idle(false).unwrap().1, 99);
        request.begin_click(Some(8));
        request.end_dispatch();
        assert!(request.request(&[entry("fresh suggestion")]));
        assert_eq!(request.take_if_idle(false).unwrap().1, 8);
    }

    #[test]
    fn cancel_invalidates_origin_and_queued_popup_without_native_tracking() {
        use std::rc::Rc;

        for queued in [false, true] {
            let event = Rc::new(7);
            let mut request = ContextRequest::default();
            request.begin_click(Some(Rc::clone(&event)));
            if queued {
                assert!(request.request(&[entry("suggestion")]));
            }
            request.cancel();
            request.cancel(); // Cancellation is safe before or after queueing.
            assert_eq!(Rc::strong_count(&event), 1);
            assert!(!request.is_pending());
            request.end_dispatch();
            // An already-scheduled native selector has no popup left to take.
            assert!(request.take_if_idle(false).is_none());
            assert!(!request.request(&[entry("late suggestion")]));

            // Cancellation does not prevent a fresh click from opening its menu.
            request.begin_click(Some(Rc::new(8)));
            assert!(request.request(&[entry("fresh suggestion")]));
            assert!(request.take_if_idle(false).is_none());
            request.end_dispatch();
            assert_eq!(*request.take_if_idle(false).unwrap().1, 8);
        }
    }

    #[test]
    fn right_trackpad_and_control_clicks_are_context_presses_only() {
        let mut event = MouseEvent {
            button: 1,
            kind: MouseKind::Down,
            position: Point::default(),
            click_count: 1,
            modifiers: Modifiers::default(),
            timestamp: 0.,
        };
        assert!(event.is_context_click());
        event.button = 0;
        assert!(!event.is_context_click());
        event.modifiers = Modifiers(1 << 18);
        assert!(event.is_context_click());
        for kind in [MouseKind::Up, MouseKind::Move, MouseKind::Drag] {
            event.kind = kind;
            assert!(!event.is_context_click());
        }
    }

    #[test]
    fn identical_menu_installations_are_deduplicated_only_for_the_same_owner() {
        let mut installed = MenuInstallation::default();
        let entries = [entry("copy")];
        assert!(installed.update(1, &entries));
        assert!(!installed.update(1, &entries));
        assert!(installed.update(2, &entries));
        assert!(installed.update(2, &[entry("paste")]));
    }

    #[test]
    fn unchanged_title_skips_native_work_and_changed_titles_update_once() {
        let mut title = WindowTitle::new("Velocity".into());
        assert!(!title.update("Velocity"));
        assert!(title.update("file.txt • — project"));
        assert!(!title.update("file.txt • — project"));
        assert!(title.update("file.txt — project"));
    }
}
