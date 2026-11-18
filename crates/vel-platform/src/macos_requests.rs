//! Process-wide open routing and the retained, closed-window template.
use super::*;
use crate::requests::{OpenHost, OpenRequests, ReopenPlan, open_target, reopen_plan};

type WindowFactory = Box<dyn FnMut() -> Result<(WindowOptions, Box<dyn AppHandler>), String>>;

#[derive(Default)]
struct ApplicationState {
    requests: RefCell<OpenRequests>,
    factory: RefCell<Option<WindowFactory>>,
    template: RefCell<Option<Rc<Host>>>,
    idle_timer: Cell<Option<Retained<NSTimer>>>,
    reopen: Cell<bool>,
    scheduled: Cell<bool>,
    awaiting_new_request: Cell<bool>,
    context_waiters: RefCell<Vec<Weak<Host>>>,
    processing: Cell<bool>,
}
thread_local! {
    static APPLICATION: ApplicationState = ApplicationState::default();
}

fn hosts() -> Vec<Rc<Host>> {
    WINDOWS.with(|windows| {
        windows
            .borrow()
            .iter()
            .map(|delegate| Rc::clone(&delegate.ivars().host))
            .collect()
    })
}
pub(super) fn live_hosts() -> Vec<Rc<Host>> {
    hosts()
        .into_iter()
        .filter(|host| !host.stopped.get())
        .collect()
}
fn destinations(hosts: &[Rc<Host>]) -> Vec<OpenHost> {
    hosts
        .iter()
        .map(|host| OpenHost {
            id: host.id,
            ready: host.ready.get() && !host.starting.get(),
            key: host.window.get().is_some_and(|window| window.isKeyWindow()),
        })
        .collect()
}

pub(super) fn hosts_busy() -> bool {
    hosts().iter().any(|host| {
        host.dispatching.get()
            || host.drawing.get()
            || host.input_depth.get() != 0
            || host.app.try_borrow_mut().is_err()
    })
}
pub(super) fn register_context(host: &Rc<Host>) {
    APPLICATION.with(|app| {
        let mut waiters = app.context_waiters.borrow_mut();
        if !waiters
            .iter()
            .any(|waiter| waiter.ptr_eq(&Rc::downgrade(host)))
        {
            waiters.push(Rc::downgrade(host));
        }
    });
}
pub(super) fn schedule_contexts() {
    // Usually empty: do not scan windows on mouse-move, scroll, or frame paths.
    let waiters = APPLICATION.with(|app| std::mem::take(&mut *app.context_waiters.borrow_mut()));
    let waiters = waiters
        .into_iter()
        .filter(|waiter| {
            let Some(host) = waiter.upgrade() else {
                return false;
            };
            if host.stopped.get() || !host.context.borrow().is_pending() {
                return false;
            }
            host.schedule_context_menu();
            true
        })
        .collect();
    APPLICATION.with(|app| *app.context_waiters.borrow_mut() = waiters);
}
pub(super) fn set_factory(factory: WindowFactory) {
    APPLICATION.with(|app| {
        *app.factory.borrow_mut() = Some(factory);
        app.awaiting_new_request.set(false);
    });
    schedule();
}
pub(super) fn is_template(id: u64) -> bool {
    APPLICATION.with(|app| {
        app.template
            .borrow()
            .as_ref()
            .is_some_and(|host| host.id == id)
    })
}
pub(super) fn keep_template(host: Rc<Host>) {
    let previous = APPLICATION.with(|app| app.template.replace(Some(host)));
    if let Some(previous) = previous
        && let Ok(mut slot) = previous.app.try_borrow_mut()
    {
        slot.take();
    }
    update_idle_timer();
}

pub(super) fn receive(paths: Vec<PathBuf>) {
    APPLICATION.with(|app| {
        app.requests.borrow_mut().push(paths);
        app.awaiting_new_request.set(false);
    });
    schedule();
}
pub(super) fn reopen() {
    APPLICATION.with(|app| {
        app.reopen.set(true);
        app.awaiting_new_request.set(false);
    });
    schedule();
}
pub(super) fn schedule() {
    let needs_schedule = APPLICATION.with(|app| {
        !app.awaiting_new_request.get()
            && (app.reopen.get() || !app.requests.borrow().is_empty())
            && !app.scheduled.replace(true)
    });
    if !needs_schedule {
        return;
    }
    let delegate = WINDOWS.with(|windows| windows.borrow().first().cloned());
    if let Some(delegate) = delegate {
        // SAFETY: AppKit retains this NSObject until its one-object selector runs
        // on a later default-mode turn, outside the current host callback.
        unsafe {
            delegate.performSelector_withObject_afterDelay(sel!(drainPlatformRequests:), None, 0.);
        }
    } else {
        APPLICATION.with(|app| app.scheduled.set(false));
    }
}

pub(super) fn drain() {
    APPLICATION.with(|app| app.scheduled.set(false));
    if APPLICATION.with(|app| !app.reopen.get() && app.requests.borrow().is_empty()) {
        return;
    }
    if hosts_busy() || APPLICATION.with(|app| app.processing.replace(true)) {
        // The outer host calls schedule again when its borrow ends. Do not spin
        // a zero-delay selector inside an alert's nested modal loop.
        return;
    }
    let mut live = live_hosts();
    if reopen_plan(&destinations(&live)) == ReopenPlan::CreateWindow {
        let template = APPLICATION.with(|app| app.template.borrow().clone());
        if let Some(template) = template {
            template.dispatching.set(true);
            {
                let mut slot = template.app.borrow_mut();
                if let Some(app) = slot.as_mut() {
                    app.on_reopen(&template.handle());
                }
            }
            template.dispatching.set(false);
            schedule_contexts();
        }
        live = live_hosts();
        if live.is_empty() {
            // The factory is removed before invocation: it may run arbitrary app
            // construction and callbacks without holding a process-state borrow.
            let factory = APPLICATION.with(|app| app.factory.borrow_mut().take());
            if let Some(mut factory) = factory {
                let result = factory().and_then(|(options, app)| {
                    let mtm = MainThreadMarker::new()
                        .ok_or("window factory must run on the main thread")?;
                    launch_window(mtm, options, app).map(|_| ())
                });
                APPLICATION.with(|app| {
                    let mut slot = app.factory.borrow_mut();
                    if slot.is_none() {
                        *slot = Some(factory);
                    }
                });
                if let Err(error) = result {
                    let mtm = MainThreadMarker::new().expect("main-thread callback");
                    let alert = NSAlert::new(mtm);
                    alert.setMessageText(&NSString::from_str("Could not open a window"));
                    alert.setInformativeText(&NSString::from_str(&error));
                    alert.runModal();
                }
            }
            live = live_hosts();
        }
    }
    let destinations = destinations(&live);
    if let Some(id) = open_target(&destinations) {
        if APPLICATION.with(|app| app.reopen.replace(false))
            && let Some(host) = live.iter().find(|host| host.id == id)
        {
            host.handle().focus();
        }
        loop {
            let current = live_hosts();
            let current_destinations = self::destinations(&current);
            let request =
                APPLICATION.with(|app| app.requests.borrow_mut().pop_all(&current_destinations));
            let Some((id, paths)) = request else {
                break;
            };
            let Some(host) = current.iter().find(|host| host.id == id) else {
                break;
            };
            host.dispatch(Event::OpenPaths(paths));
            if !host.stopped.get() {
                host.handle().focus();
                host.flush();
            }
        }
    } else if live.is_empty() {
        // Preserve requests for the next successful creation, but don't repeatedly
        // invoke a failing callback/factory from every background idle tick.
        APPLICATION.with(|app| app.awaiting_new_request.set(true));
    }
    update_idle_timer();
    APPLICATION.with(|app| app.processing.set(false));
}

fn update_idle_timer() {
    let needed = live_hosts().is_empty() && APPLICATION.with(|app| app.template.borrow().is_some());
    if !needed {
        APPLICATION.with(|app| {
            if let Some(timer) = app.idle_timer.take() {
                timer.invalidate();
            }
        });
        return;
    }
    let exists = APPLICATION.with(|app| {
        let timer = app.idle_timer.take();
        let exists = timer.is_some();
        app.idle_timer.set(timer);
        exists
    });
    if exists {
        return;
    }
    let delegate = WINDOWS.with(|windows| windows.borrow().first().cloned());
    if let Some(delegate) = delegate {
        // SAFETY: the retained application delegate implements this NSTimer selector;
        // common modes allow background IPC during native menu/alert tracking.
        let timer = unsafe {
            NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                0.1,
                &delegate,
                sel!(pollClosedWindow:),
                None,
                true,
            )
        };
        // SAFETY: this is AppKit's main-thread run loop and its immutable mode constant.
        unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
        APPLICATION.with(|app| app.idle_timer.set(Some(timer)));
    }
}

pub(super) fn idle() {
    if hosts_busy() || APPLICATION.with(|app| app.processing.get()) {
        return;
    }
    if !live_hosts().is_empty() {
        update_idle_timer();
        return;
    }
    let template = APPLICATION.with(|app| app.template.borrow().clone());
    if let Some(template) = template {
        template.dispatching.set(true);
        {
            let mut slot = template.app.borrow_mut();
            if let Some(app) = slot.as_mut() {
                app.on_idle(&template.handle());
            }
        }
        template.dispatching.set(false);
        schedule_contexts();
    }
    schedule();
    update_idle_timer();
}

pub(super) fn launched() {
    APPLICATION.with(|app| app.awaiting_new_request.set(false));
    update_idle_timer();
    schedule();
    schedule_contexts();
}
pub(super) fn reset() {
    APPLICATION.with(|app| {
        if let Some(timer) = app.idle_timer.take() {
            timer.invalidate();
        }
        app.template.borrow_mut().take();
        app.factory.borrow_mut().take();
        *app.requests.borrow_mut() = OpenRequests::default();
        app.reopen.set(false);
        app.scheduled.set(false);
        app.awaiting_new_request.set(false);
        app.context_waiters.borrow_mut().clear();
        app.processing.set(false);
    });
}
