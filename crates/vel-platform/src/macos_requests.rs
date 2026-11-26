//! Deferred retirement and process-wide open routing, including without windows.
use super::*;
use crate::requests::{
    NativeOpenRequest, OpenHost, OpenRequests, ReopenPlan, open_target, reopen_plan,
};

type WindowFactory = Box<dyn FnMut() -> Result<(WindowOptions, Box<dyn AppHandler>), String>>;

#[derive(Default)]
struct ApplicationState {
    requests: RefCell<OpenRequests>,
    factory: RefCell<Option<WindowFactory>>,
    delegate: RefCell<Option<Retained<ApplicationDelegate>>>,
    retirements: RefCell<Vec<u64>>,
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
            .owners()
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
            key: host.window().is_some_and(|window| window.isKeyWindow()),
        })
        .collect()
}

pub(super) fn hosts_busy() -> bool {
    let busy = |host: &Host| {
        host.dispatching.get()
            || host.drawing.get()
            || host.input_depth.get() != 0
            || host.context_tracking.get()
            || host.app.try_borrow_mut().is_err()
    };
    WINDOWS.with_borrow(|windows| {
        windows.owners().any(|d| busy(&d.ivars().host))
            || windows.template().is_some_and(|host| busy(host))
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
pub(super) fn set_delegate(delegate: Retained<ApplicationDelegate>) {
    APPLICATION.with(|app| app.delegate.replace(Some(delegate)));
}
pub(super) fn delegate() -> Option<Retained<ApplicationDelegate>> {
    APPLICATION.with(|app| app.delegate.borrow().clone())
}
pub(super) fn template() -> Option<Rc<Host>> {
    WINDOWS.with(|windows| windows.borrow().template().cloned())
}
pub(super) fn with_template(host: &Rc<Host>, f: impl FnOnce(&mut dyn AppHandler, &WindowHandle)) {
    if let Ok(mut slot) = host.app.try_borrow_mut()
        && let Some(app) = slot.as_mut()
    {
        f(&mut **app, &host.handle());
    }
    schedule_contexts();
    schedule();
}
pub(super) fn queue_retirement(id: u64) {
    APPLICATION.with(|app| app.retirements.borrow_mut().push(id));
    schedule();
}
fn retire_closed_windows() {
    let ids = APPLICATION.with(|app| app.retirements.take());
    if ids.is_empty() {
        return;
    }
    for id in ids {
        let Some((delegate, last)) = WINDOWS.with(|windows| windows.borrow_mut().remove(id)) else {
            continue;
        };
        let host = &delegate.ivars().host;
        APPLICATION.with(|app| {
            let closed = Rc::downgrade(host);
            app.context_waiters
                .borrow_mut()
                .retain(|waiter| !waiter.ptr_eq(&closed));
        });
        let handler = host.app.take();
        let controller = if last {
            handler.and_then(|handler| handler.into_windowless())
        } else {
            drop(handler);
            None
        };
        // Dropping the handler releases the wgpu surface before disposing of its layer.
        host.release_native_window();
        if let Some(controller) = controller {
            // A fresh handle supplies app-level services without keeping the old host alive.
            let template = new_host(host.mtm, &WindowOptions::default(), controller);
            template.stopped.set(true);
            WINDOWS.with(|windows| windows.borrow_mut().keep_template(template));
        }
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
pub(super) fn receive_finder_new_text_file(directory: PathBuf) {
    APPLICATION.with(|app| {
        app.requests
            .borrow_mut()
            .push_finder_new_text_file(directory);
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
        (!app.retirements.borrow().is_empty()
            || !app.awaiting_new_request.get()
                && (app.reopen.get() || !app.requests.borrow().is_empty()))
            && !app.scheduled.replace(true)
    });
    if !needs_schedule {
        return;
    }
    if let Some(delegate) = delegate() {
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
    if hosts_busy() || APPLICATION.with(|app| app.processing.replace(true)) {
        // The outer host calls schedule again when its borrow ends. Do not spin
        // a zero-delay selector inside an alert's nested modal loop.
        return;
    }
    retire_closed_windows();
    if APPLICATION.with(|app| {
        app.awaiting_new_request.get() || !app.reopen.get() && app.requests.borrow().is_empty()
    }) {
        APPLICATION.with(|app| app.processing.set(false));
        schedule();
        return;
    }
    let mut live = live_hosts();
    if reopen_plan(&destinations(&live)) == ReopenPlan::CreateWindow {
        if let Some(template) = template() {
            with_template(&template, |app, window| {
                let directory = APPLICATION
                    .with(|state| state.requests.borrow_mut().pop_finder_for_closed_window());
                if let Some(directory) = directory {
                    app.on_finder_new_text_file(window, &directory);
                } else {
                    app.on_reopen(window);
                }
            });
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
                    // No window exists to host status feedback yet.
                    eprintln!("Could not open a window: {error}");
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
                APPLICATION.with(|app| app.requests.borrow_mut().pop(&current_destinations));
            let Some((id, request)) = request else {
                break;
            };
            let Some(host) = current.iter().find(|host| host.id == id) else {
                break;
            };
            host.dispatch(match request {
                NativeOpenRequest::Paths(paths) => Event::OpenPaths(paths),
                NativeOpenRequest::FinderNewTextFile(directory) => {
                    Event::FinderNewTextFile(directory)
                }
            });
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
    if APPLICATION.with(|app| !app.retirements.borrow().is_empty()) {
        schedule();
    }
}

fn needs_idle(host: &Host) -> bool {
    host.ready.get() && !host.starting.get() && !host.visible.get()
}

pub(super) fn update_idle_timer() {
    let hosts = live_hosts();
    let needed =
        hosts.iter().any(|host| needs_idle(host)) || (hosts.is_empty() && template().is_some());
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
    if let Some(delegate) = delegate() {
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
    let hosts = live_hosts();
    if !hosts.is_empty() {
        for host in hosts.iter().filter(|host| needs_idle(host)) {
            host.dispatch(Event::Idle);
        }
        poll_memory(hosts[0].mtm);
        update_idle_timer();
        return;
    }
    if let Some(template) = template() {
        with_template(&template, |app, window| app.on_idle(window));
    }
    schedule();
    update_idle_timer();
}

pub(super) fn poll_memory(mtm: MainThreadMarker) {
    let ready = |host: &Host| host.ready.get() && !host.starting.get() && !host.stopped.get();
    memory_guard::service(
        Instant::now(),
        || {
            !hosts_busy()
                && WINDOWS.with_borrow(|windows| windows.owners().any(|d| ready(&d.ivars().host)))
        },
        || {
            let foreground = NSApplication::sharedApplication(mtm).isActive()
                && WINDOWS.with_borrow(|windows| {
                    windows.owners().any(|delegate| {
                        let host = &delegate.ivars().host;
                        ready(host)
                            && host
                                .window()
                                .is_some_and(|w| w.isVisible() && !w.isMiniaturized())
                    })
                });
            (
                foreground,
                foreground && NSEvent::pressedMouseButtons() != 0,
            )
        },
        || None, // libdispatch queues the event independently of this poll.
        || {
            WINDOWS.with_borrow(|windows| {
                memory_guard::sum_documents(windows.owners().filter_map(|delegate| {
                    let host = &delegate.ivars().host;
                    if !ready(host) {
                        return None;
                    }
                    host.app.borrow().as_ref()?.memory_guard_document_bytes()
                }))
            })
        },
        |request| {
            memory_guard::sum_caches(live_hosts().iter().filter_map(|host| {
                if !ready(host) {
                    return None;
                }
                request.deliver(host.app.borrow_mut().as_deref_mut()?)
            }))
        },
    );
}

pub(super) fn launched() {
    WINDOWS.with(|windows| windows.borrow_mut().launched());
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
        app.delegate.take();
        app.retirements.borrow_mut().clear();
        app.factory.borrow_mut().take();
        *app.requests.borrow_mut() = OpenRequests::default();
        app.reopen.set(false);
        app.scheduled.set(false);
        app.awaiting_new_request.set(false);
        app.context_waiters.borrow_mut().clear();
        app.processing.set(false);
    });
}
