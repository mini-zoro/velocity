//! Single-threaded Win32 host. The message procedure snapshots native data;
//! app callbacks normally run only after DispatchMessage or at a frame tick.
mod drag_image;
mod input;
mod menu;
mod model;
mod shell;
mod window;

use crate::memory_guard;
use crate::requests::{ContextRequest, OpenHost, WindowTitle, open_target};
use crate::{
    AppHandler, Appearance, Command, FrameContext, FrameReason, MenuEntry, MouseEvent, MouseKind,
    Point, Presentation, Rect, ScrollEvent, Size, TextInput, WindowOptions,
};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    ffi::c_void,
    fmt, io,
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    ptr::{NonNull, null, null_mut},
    rc::{Rc, Weak},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WAIT_FAILED, WPARAM,
    },
    Graphics::Gdi::{BeginPaint, ClientToScreen, EndPaint, PAINTSTRUCT},
    System::{
        LibraryLoader::GetModuleHandleW,
        Threading::{
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, GetCurrentProcessId,
            INFINITE, SetWaitableTimer, TIMER_ALL_ACCESS,
        },
    },
    UI::{
        HiDpi::*, Input::KeyboardAndMouse::GetFocus, Shell::DragAcceptFiles, WindowsAndMessaging::*,
    },
};
use windows_sys::w;

const CLASS: *const u16 = w!("VelocityEditorWindow");
const IDLE_PERIOD: Duration = Duration::from_millis(100);
const SIZING_TIMER: usize = 1;

type Factory = Box<dyn FnMut() -> Result<(WindowOptions, Box<dyn AppHandler>), String>>;
type TitleTheme = (Option<Appearance>, Option<[f32; 4]>);

#[derive(Clone)]
pub struct WindowHandle(Rc<Host>);

impl fmt::Debug for WindowHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WindowHandle")
            .field("id", &self.0.id)
            .field("hwnd", &self.0.hwnd.get())
            .field("closed", &self.is_closed())
            .finish()
    }
}

impl WindowHandle {
    /// Uploads a floating drag image without showing it. `grab` is the pointer's
    /// offset inside the image in logical points; the next `move_drag_image` shows it.
    pub fn load_drag_image(&self, image: &crate::DragImage, grab: Point) {
        drag_image::load(image, grab);
    }
    /// Moves the loaded drag image, showing it without activating or capturing input.
    pub fn move_drag_image(&self, point: Point) {
        drag_image::follow(point);
    }
    /// Not offered: message times tick every 10-16 ms, too coarse to order a live
    /// read against queued mouse messages, so the drag image follows messages only.
    pub fn pointer_screen_location(&self) -> Option<(Point, f64)> {
        None
    }
    /// Hides the drag image and releases its pixels.
    pub fn hide_drag_image(&self) {
        drag_image::hide(true);
    }
    /// The topmost native window at the given screen point, ignoring the drag image.
    pub fn screen_window_at(&self, point: Point) -> Option<usize> {
        drag_image::window_at(point)
    }
    /// Opaque native window identity for screen hit testing.
    pub fn native_window_id(&self) -> Option<usize> {
        self.hwnd().map(|hwnd| hwnd.as_ptr() as usize)
    }

    /// A borrowed HWND for initializing a native renderer on this thread.
    /// The renderer must be owned by the window's handler and dropped before
    /// the HWND is destroyed; the raw handle does not extend its lifetime.
    pub fn hwnd(&self) -> Option<NonNull<c_void>> {
        if self.is_closed() {
            return None;
        }
        NonNull::new(self.0.hwnd.get())
    }

    /// Changes the native window title, skipping unchanged titles.
    pub fn set_title(&self, title: &str) {
        if let Some(hwnd) = self.hwnd()
            && self.0.title.borrow_mut().update(title)
        {
            let title = wide(title);
            // SAFETY: The HWND belongs to this thread; title is NUL-terminated.
            unsafe { SetWindowTextW(hwnd.as_ptr(), title.as_ptr()) };
        }
    }

    /// Registers a fallback for reopening after all editor windows close.
    pub fn set_window_factory(
        &self,
        factory: impl FnMut() -> Result<(WindowOptions, Box<dyn AppHandler>), String> + 'static,
    ) {
        if let Some(runtime) = self.0.runtime.upgrade() {
            runtime.factory.replace(Some(Box::new(factory)));
        }
    }

    /// Creates a top-level window with its own exclusive handler.
    pub fn new_window(&self, options: WindowOptions, app: impl AppHandler) -> Result<Self, String> {
        self.0
            .runtime
            .upgrade()
            .ok_or("native event loop has stopped")?
            .launch(options, Box::new(app))
    }

    /// True when this handle is only a closed window template.
    pub fn is_closed(&self) -> bool {
        self.0.closing.get() || self.0.hwnd.get().is_null()
    }

    /// Asks the app to approve closing this window.
    pub fn request_close(&self) {
        self.0.enqueue(Event::Close);
    }

    /// Closes this window without a second confirmation.
    pub fn close(&self) {
        self.0.closing.set(true);
    }

    /// Asks every live handler to approve quitting.
    pub fn request_app_quit(&self) {
        if let Some(runtime) = self.0.runtime.upgrade() {
            runtime.quit_requested.set(true);
        }
    }

    /// Activates another editor window in creation order. Auxiliary windows
    /// such as Indexing Status don't have a menu bar and are not in the cycle.
    pub fn cycle_editor_window(&self, forward: bool) {
        let Some(runtime) = self.0.runtime.upgrade() else {
            return;
        };
        let target = {
            let windows = runtime.windows.borrow();
            let editors: Vec<_> = windows
                .iter()
                .filter(|host| {
                    host.ready.get() && !host.closing.get() && host.menu.borrow().is_some()
                })
                .collect();
            editors
                .iter()
                .position(|host| host.id == self.0.id)
                .and_then(|at| {
                    let count = editors.len();
                    (count > 1).then(|| {
                        let offset = if forward { 1 } else { count - 1 };
                        WindowHandle(Rc::clone(editors[(at + offset) % count]))
                    })
                })
        };
        if let Some(target) = target {
            target.focus();
        }
    }

    /// Stops the event loop without confirmation.
    pub fn quit(&self) {
        if let Some(runtime) = self.0.runtime.upgrade() {
            runtime.stop.set(true);
        }
    }

    /// Enqueues a resource command on this window.
    pub fn dispatch_named_command(&self, name: &str, args: &str) {
        self.0.enqueue(Event::Named(menu::Action::Named(
            crate::requests::MenuCommand {
                name: name.into(),
                args: args.into(),
            },
        )));
    }

    /// Whether the window is shown and not minimized.
    pub fn is_visible(&self) -> bool {
        self.hwnd().is_some_and(|hwnd| {
            // SAFETY: both functions only query this thread's live window.
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd.as_ptr()) != 0
                    && windows_sys::Win32::UI::WindowsAndMessaging::IsIconic(hwnd.as_ptr()) == 0
            }
        })
    }
    /// True if this window receives native keyboard input.
    pub fn is_focused(&self) -> bool {
        self.hwnd().is_some_and(|hwnd| {
            // SAFETY: GetFocus only queries the calling thread's GUI queue.
            unsafe { GetFocus() == hwnd.as_ptr() }
        })
    }

    /// Animates only in a visible, foreground editor that has keyboard focus.
    pub fn caret_is_active(&self) -> bool {
        self.0.is_drawable()
            && self.is_focused()
            // SAFETY: A read-only query for the current foreground HWND.
            && unsafe { GetForegroundWindow() == self.0.hwnd.get() }
            && !window::caret_occluded(self.0.hwnd.get())
    }

    /// Time between inversions, or `None` when Windows disables caret blinking.
    pub fn caret_blink_interval(&self) -> Option<Duration> {
        // SAFETY: This system preference query takes no arguments.
        crate::windows_caret_blink_interval(unsafe { GetCaretBlinkTime() })
    }

    /// Converts a logical client point to physical virtual-desktop coordinates.
    pub fn screen_point(&self, point: Point) -> Option<Point> {
        let hwnd = self.hwnd()?;
        let scale = self.scale_factor();
        let mut point = POINT {
            x: (point.x * scale).round() as i32,
            y: (point.y * scale).round() as i32,
        };
        // SAFETY: Windows writes the converted point through the valid pointer.
        (unsafe { ClientToScreen(hwnd.as_ptr(), &mut point) } != 0).then_some(Point {
            x: point.x.into(),
            y: point.y.into(),
        })
    }
    /// Adds a logical client offset to a physical virtual-desktop point.
    pub fn screen_point_offset(&self, screen: Point, offset: Point) -> Point {
        let scale = self.scale_factor();
        Point {
            x: screen.x + offset.x * scale,
            y: screen.y + offset.y * scale,
        }
    }

    /// Marks the next display-rate tick dirty.
    pub fn request_frame(&self) {
        self.0.request(FrameReason::DisplayLink);
    }

    /// Schedules a coalesced one-shot monotonic deadline.
    pub fn request_frame_at(&self, deadline: Instant) {
        self.0.frames.borrow_mut().at(deadline);
    }

    /// Enables or stops refresh-rate-driven animation.
    pub fn set_animating(&self, active: bool) {
        self.0.frames.borrow_mut().animate(active);
    }

    /// Logical-to-physical pixel scale of this window's monitor.
    pub fn scale_factor(&self) -> f64 {
        model::scale(self.0.dpi.get())
    }

    /// Actual logical client size, excluding native decorations.
    pub fn content_size(&self) -> Size {
        self.0.size.get()
    }

    /// The current monitor's nominal refresh rate.
    pub fn maximum_frames_per_second(&self) -> f32 {
        self.0.refresh.get() as f32
    }

    /// Windows uses an HWND surface, not Metal.
    pub fn metal_layer_ptr(&self) -> *mut c_void {
        null_mut()
    }

    /// DX12 does not report Metal drawable acquisition sequences.
    pub fn last_acquired_drawable(&self) -> Option<u64> {
        None
    }

    /// DX12 FIFO present does not provide the Metal scanout observation.
    pub fn take_presentations(&self) -> Vec<Presentation> {
        Vec::new()
    }

    /// Installs this window's own native menu bar.
    pub fn set_menu(&self, entries: &[MenuEntry]) {
        if self.0.menu_entries.borrow().as_slice() != entries {
            self.0.menu_entries.replace(entries.to_vec());
            self.0.menu_dirty.set(true);
        }
    }

    /// Queues a popup at the last click, outside the app callback.
    pub fn show_context_menu(&self, entries: &[MenuEntry]) -> bool {
        self.0.context.borrow_mut().request(entries)
    }

    /// Invalidates pending popups and their observed click.
    pub fn cancel_context_menu(&self) {
        self.0.context.borrow_mut().cancel();
    }

    /// Repositions active IME windows after the application updates its caret.
    pub fn invalidate_character_coordinates(&self) {
        self.0.ime_anchor_dirty.set(true);
    }
    /// Ends conversion after the app clears its preedit and releases its borrow.
    pub fn discard_marked_text(&self) {
        self.0.ime_discard_pending.set(true);
    }
}

enum Event {
    Text(TextInput),
    Command(Command),
    Mouse(MouseEvent, Option<POINT>),
    Scroll(ScrollEvent),
    Named(menu::Action),
    Resize,
    Moved,
    PointerCancel,
    Appearance,
    Preferences,
    FocusLost,
    Open(Vec<PathBuf>),
    Close,
}

struct Host {
    id: u64,
    runtime: Weak<Runtime>,
    hwnd: Cell<HWND>,
    app: RefCell<Option<Box<dyn AppHandler>>>,
    events: RefCell<VecDeque<Event>>,
    ready: Cell<bool>,
    closing: Cell<bool>,
    visible: Cell<bool>,
    minimized: Cell<bool>,
    sizing: Cell<bool>,
    size: Cell<Size>,
    dpi: Cell<u32>,
    monitor: Cell<windows_sys::Win32::Graphics::Gdi::HMONITOR>,
    refresh: Cell<u32>,
    frames: RefCell<model::Frames>,
    idle_at: Cell<Instant>,
    first_present: Cell<bool>,
    title: RefCell<WindowTitle>,
    normal_frame: Cell<Option<Rect>>,
    minimum_size: Cell<Size>,
    fullscreen: Cell<Option<window::Fullscreen>>,
    appearance: Cell<Appearance>,
    title_theme: Cell<TitleTheme>,
    menu_entries: RefCell<Vec<MenuEntry>>,
    menu_dirty: Cell<bool>,
    menu_tracking: Cell<bool>,
    taskbar_busy: Cell<bool>,
    menu: RefCell<Option<menu::MenuBar>>,
    context: RefCell<ContextRequest<POINT>>,
    clicks: RefCell<model::Clicks>,
    pressed: Cell<u8>,
    pointer_hidden: Cell<bool>,
    chars: RefCell<model::Utf16Input>,
    ime: RefCell<model::Composition>,
    ime_anchor_dirty: Cell<bool>,
    ime_discard_pending: Cell<bool>,
    ime_discarding: Cell<bool>,
}

impl Host {
    fn new(runtime: &Rc<Runtime>, size: Size, title: String, app: Box<dyn AppHandler>) -> Rc<Self> {
        let id = runtime.next_id.get();
        runtime.next_id.set(id + 1);
        Rc::new(Self {
            id,
            runtime: Rc::downgrade(runtime),
            hwnd: Cell::new(null_mut()),
            app: RefCell::new(Some(app)),
            events: RefCell::new(VecDeque::new()),
            ready: Cell::new(false),
            closing: Cell::new(false),
            visible: Cell::new(false),
            minimized: Cell::new(false),
            sizing: Cell::new(false),
            size: Cell::new(size),
            dpi: Cell::new(model::BASE_DPI),
            monitor: Cell::new(null_mut()),
            refresh: Cell::new(60),
            frames: RefCell::new(model::Frames::default()),
            idle_at: Cell::new(Instant::now()),
            first_present: Cell::new(true),
            title: RefCell::new(WindowTitle::new(title)),
            normal_frame: Cell::new(None),
            minimum_size: Cell::new(Size::default()),
            fullscreen: Cell::new(None),
            appearance: Cell::new(window::system_appearance()),
            title_theme: Cell::new((None, None)),
            menu_entries: RefCell::new(Vec::new()),
            menu_dirty: Cell::new(false),
            menu_tracking: Cell::new(false),
            taskbar_busy: Cell::new(false),
            menu: RefCell::new(None),
            context: RefCell::new(ContextRequest::default()),
            clicks: RefCell::new(model::Clicks::default()),
            pressed: Cell::new(0),
            pointer_hidden: Cell::new(false),
            chars: RefCell::new(model::Utf16Input::default()),
            ime: RefCell::new(model::Composition::default()),
            ime_anchor_dirty: Cell::new(false),
            ime_discard_pending: Cell::new(false),
            ime_discarding: Cell::new(false),
        })
    }

    fn handle(self: &Rc<Self>) -> WindowHandle {
        WindowHandle(Rc::clone(self))
    }

    fn enqueue(&self, event: Event) {
        if !self.closing.get() {
            let mut events = self.events.borrow_mut();
            if matches!(event, Event::Resize) && matches!(events.back(), Some(Event::Resize)) {
                return;
            }
            events.push_back(event);
        }
    }

    fn request(&self, reason: FrameReason) {
        self.frames.borrow_mut().request(reason);
    }

    fn with_app<T>(
        self: &Rc<Self>,
        call: impl FnOnce(&mut dyn AppHandler, &WindowHandle) -> T,
    ) -> Option<T> {
        let mut slot = self.app.try_borrow_mut().ok()?;
        let app = slot.as_deref_mut()?;
        let window = self.handle();
        let result = call(app, &window);
        app.update_text_input_focus(&window);
        Some(result)
    }

    fn drain(self: &Rc<Self>) {
        if !self.ready.get() || self.app.try_borrow_mut().is_err() {
            return;
        }
        while !self.closing.get() {
            let Some(event) = self.events.borrow_mut().pop_front() else {
                break;
            };
            self.with_app(|app, w| match event {
                Event::Text(text) => app.on_text_input(w, text),
                Event::Command(command) => app.on_command(w, command),
                Event::Mouse(event, at) => {
                    let down = event.kind == MouseKind::Down;
                    if down {
                        self.context.borrow_mut().begin_click(at);
                    }
                    app.on_mouse(w, event);
                    if down {
                        self.context.borrow_mut().end_dispatch();
                    }
                }
                Event::Scroll(event) => app.on_scroll(w, event),
                Event::Named(action) => self.run_menu_action(app, w, action),
                Event::Resize => app.on_resize(w, self.size.get(), w.scale_factor()),
                Event::Moved => app.on_move(w),
                Event::PointerCancel => app.on_pointer_cancel(w),
                Event::Appearance => app.on_appearance(w, w.appearance()),
                Event::Preferences => {
                    app.on_accessibility_preferences(w, window::accessibility_preferences())
                }
                Event::FocusLost => {
                    app.on_pointer_cancel(w);
                    app.on_text_input_focus_lost(w);
                }
                Event::Open(paths) => app.on_open_paths(w, &paths),
                Event::Close => {
                    if app.on_window_close_requested(w) {
                        w.close();
                    }
                }
            });
            self.flush_ime_discard();
            if self.runtime.upgrade().is_none_or(|r| r.stop.get()) {
                break;
            }
        }
        self.flush_ime_anchor();
    }

    fn render(self: &Rc<Self>, reason: FrameReason, received: Instant) -> Result<(), String> {
        self.with_app(|app, w| {
            app.on_frame(&mut FrameContext {
                window: w,
                received,
                reason,
            })
        })
        .unwrap_or(Ok(()))?;
        self.flush_ime_discard();
        // Resize and panel layout can change field bounds during the frame.
        if self.ime.borrow().active {
            self.ime_anchor_dirty.set(true);
        }
        self.flush_ime_anchor();
        Ok(())
    }

    fn tick(self: &Rc<Self>, now: Instant) -> Result<(), String> {
        if !self.ready.get() || self.closing.get() {
            return Ok(());
        }
        self.drain();
        if self.closing.get() || self.runtime.upgrade().is_none_or(|r| r.stop.get()) {
            return Ok(());
        }
        self.update_menu_bar();
        self.track_pending_popup();
        if !self.is_drawable() {
            if self.idle_at.get() <= now {
                self.idle_at.set(now + IDLE_PERIOD);
                self.with_app(|app, w| app.on_idle(w));
            }
            return Ok(());
        }
        let period = Duration::from_secs_f64(1. / f64::from(self.refresh.get()));
        let reason = self.frames.borrow_mut().take(now, period);
        if let Some(reason) = reason {
            let reason = if self.first_present.replace(false) {
                FrameReason::StartupPresent
            } else {
                reason
            };
            self.render(reason, now)?;
        }
        Ok(())
    }

    fn is_drawable(&self) -> bool {
        !self.hwnd.get().is_null() && self.visible.get() && !self.minimized.get()
    }

    fn wake(&self, now: Instant) -> Option<Instant> {
        let can_track = !self.menu_tracking.get();
        if !self.events.borrow().is_empty()
            || (can_track && self.menu_dirty.get())
            || (can_track && self.pressed.get() == 0 && self.context.borrow().is_pending())
        {
            Some(now)
        } else if !self.is_drawable() {
            Some(self.idle_at.get())
        } else {
            self.frames.borrow().wake(now)
        }
    }

    fn destroy(&self) {
        self.ready.set(false);
        self.closing.set(true);
        let hwnd = self.hwnd.replace(null_mut());
        if !hwnd.is_null() {
            self.discard_ime(hwnd);
            // SAFETY: This thread created the HWND and the runtime still retains
            // a strong reference to the Host through the last native callback.
            // Its app (including any renderer) has already been dropped.
            unsafe {
                let menu = GetMenu(hwnd);
                if !menu.is_null() {
                    SetMenu(hwnd, null_mut());
                }
                DestroyWindow(hwnd);
            }
        }
        self.menu.borrow_mut().take();
    }
}

struct Runtime {
    instance: HINSTANCE,
    taskbar_created: u32,
    windows: RefCell<Vec<Rc<Host>>>,
    template: RefCell<Option<Rc<Host>>>,
    factory: RefCell<Option<Factory>>,
    next_id: Cell<u64>,
    stop: Cell<bool>,
    quit_requested: Cell<bool>,
    failure: RefCell<Option<String>>,
    external_open: RefCell<Vec<PathBuf>>,
    reopen_not_before: Cell<Instant>,
    memory_pressure: Option<memory_guard::windows::PressureSource>,
}

impl Runtime {
    fn poll_memory(&self, now: Instant, windows: &[Rc<Host>]) {
        let ready = |host: &Host| host.ready.get() && !host.closing.get();
        memory_guard::service(
            now,
            || windows.iter().any(|host| ready(host)),
            || {
                if !windows.iter().any(|host| ready(host) && host.is_drawable()) {
                    return (false, false);
                }
                let mut pid = 0;
                // SAFETY: these are read-only queries, and pid is a writable DWORD.
                let foreground = unsafe {
                    GetWindowThreadProcessId(GetForegroundWindow(), &mut pid);
                    pid == GetCurrentProcessId()
                };
                (
                    foreground,
                    foreground && windows.iter().any(|host| host.pressed.get() != 0),
                )
            },
            || self.memory_pressure.as_ref()?.poll(now),
            || {
                memory_guard::sum_documents(windows.iter().filter_map(|host| {
                    if !ready(host) {
                        return None;
                    }
                    host.app.borrow().as_ref()?.memory_guard_document_bytes()
                }))
            },
            |request| {
                memory_guard::sum_caches(windows.iter().filter_map(|host| {
                    if !ready(host) {
                        return None;
                    }
                    request.deliver(host.app.borrow_mut().as_deref_mut()?)
                }))
            },
        );
    }

    fn launch(
        self: &Rc<Self>,
        options: WindowOptions,
        app: Box<dyn AppHandler>,
    ) -> Result<WindowHandle, String> {
        if self.stop.get() {
            return Err("native event loop has stopped".into());
        }
        if ![options.size.width, options.size.height]
            .into_iter()
            .all(|n| n.is_finite() && n > 0.)
        {
            return Err("invalid window size".into());
        }
        let activate = app.activate_on_open();
        let host = Host::new(self, options.size, options.title.clone(), app);
        if let Some(minimum) = options.min_size {
            // WM_GETMINMAXINFO reads this from the first CreateWindowExW callback on.
            host.minimum_size.set(minimum);
        }
        let title = wide(&options.title);
        // Start with system-DPI estimates; resize to the actual monitor DPI as
        // soon as Windows chooses the initial monitor, still behind the scenes.
        // SAFETY: No function stores the title pointer. The Rc lives through
        // all CreateWindow callbacks and is subsequently retained by the runtime.
        let hwnd = unsafe {
            let dpi = GetDpiForSystem();
            let bounds = window::outer_rect(options.size, dpi, WS_OVERLAPPEDWINDOW, false);
            CreateWindowExW(
                0,
                CLASS,
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                options
                    .position
                    .map_or(CW_USEDEFAULT, |p| p.x.round() as i32),
                options
                    .position
                    .map_or(CW_USEDEFAULT, |p| p.y.round() as i32),
                bounds.right - bounds.left,
                bounds.bottom - bounds.top,
                null_mut(),
                null_mut(),
                self.instance,
                Rc::as_ptr(&host).cast(),
            )
        };
        if hwnd.is_null() {
            return Err(win_error("CreateWindowExW"));
        }
        self.windows.borrow_mut().push(Rc::clone(&host));
        host.initialize_geometry(options.size);
        if let (Some(p), Some(frame)) = (options.position, host.normal_frame.get()) {
            host.handle().restore_frame(Rect {
                x: p.x,
                y: p.y,
                ..frame
            });
        }
        host.apply_title_theme(None, None);
        // SAFETY: The HWND is live and owned by the calling thread.
        unsafe { DragAcceptFiles(hwnd, 1) };
        let start = (|| {
            host.with_app(|app, w| {
                app.on_platform_phase("native_window_created");
                app.on_start(w)
            })
            .expect("new host has an app")?;
            host.ready.set(true);
            if !host.closing.get() && !self.stop.get() {
                host.update_menu_bar();
                host.enqueue(Event::Preferences);
                host.drain();
                host.render(FrameReason::StartupPrepare, Instant::now())?;
            }
            Ok::<_, String>(())
        })();
        if let Err(error) = start {
            self.windows.borrow_mut().retain(|w| w.id != host.id);
            drop(host.app.borrow_mut().take());
            host.destroy();
            return Err(error);
        }
        if !host.closing.get() && !self.stop.get() {
            // SAFETY: The handler and the window were installed before showing it.
            unsafe { ShowWindow(hwnd, if activate { SW_SHOW } else { SW_SHOWNOACTIVATE }) };
            host.visible.set(true);
            host.request(FrameReason::Resize);
            if let Some(old) = self.template.borrow_mut().take() {
                self.release_template(old);
            }
        }
        Ok(host.handle())
    }

    fn lookup(&self, hwnd: HWND) -> Option<Rc<Host>> {
        if hwnd.is_null() {
            return None;
        }
        self.windows
            .borrow()
            .iter()
            .find(|host| host.hwnd.get() == hwnd)
            .cloned()
    }

    fn reap(&self) {
        let closing: Vec<_> = {
            let mut windows = self.windows.borrow_mut();
            let (keep, close) = std::mem::take(&mut *windows)
                .into_iter()
                .partition(|h| !h.closing.get());
            *windows = keep;
            close
        };
        if closing.is_empty() {
            return;
        }
        let mut retain = self.windows.borrow().is_empty();
        for host in closing.into_iter().rev() {
            if retain {
                // Keep the last handler for its profile pipe and reopen callback,
                // but release its rendering resources while the HWND is live.
                host.with_app(|app, _| app.on_retire_window());
                retain = false;
                host.visible.set(false);
                host.ready.set(false);
                host.events.borrow_mut().clear();
                host.context.borrow_mut().cancel();
                host.idle_at.set(Instant::now());
                let hwnd = host.hwnd.get();
                if !hwnd.is_null() {
                    // SAFETY: The HWND belongs to this retained Host and this thread.
                    unsafe {
                        DragAcceptFiles(hwnd, 0);
                        ShowWindow(hwnd, SW_HIDE);
                    }
                }
                if let Some(old) = self.template.replace(Some(host)) {
                    self.release_template(old);
                }
            } else {
                self.release_template(host);
            }
        }
    }

    fn release_template(&self, host: Rc<Host>) {
        // A template can open its replacement while its app is borrowed. Defer
        // releasing it until the callback returns to the runtime.
        let Ok(mut slot) = host.app.try_borrow_mut() else {
            self.windows.borrow_mut().push(Rc::clone(&host));
            return;
        };
        let app = slot.take();
        drop(slot);
        drop(app);
        host.destroy();
    }

    fn approve_quit(&self) {
        if !self.quit_requested.replace(false) {
            return;
        }
        let mut windows = self.windows.borrow().clone();
        windows.sort_by_key(|w| !w.handle().is_focused());
        if windows.iter().filter(|w| !w.closing.get()).all(|w| {
            w.with_app(|app, window| app.on_quit_requested(window))
                .unwrap_or(false)
        }) {
            let approved = if let Some(window) = windows.iter().find(|w| !w.closing.get()) {
                window.with_app(|app, w| app.on_quit_approved(w))
            } else {
                self.template
                    .borrow()
                    .as_ref()
                    .and_then(|window| window.with_app(|app, w| app.on_quit_approved(w)))
            };
            self.stop.set(approved.unwrap_or(true));
        }
    }

    fn route_external_opens(self: &Rc<Self>, now: Instant) {
        if self.external_open.borrow().is_empty() {
            return;
        }
        let windows = self.windows.borrow().clone();
        let routes: Vec<_> = windows
            .iter()
            .filter(|h| !h.closing.get())
            .map(|host| OpenHost {
                id: host.id,
                ready: host.ready.get(),
                key: host.handle().is_focused(),
            })
            .collect();
        if let Some(id) = open_target(&routes)
            && let Some(host) = windows.iter().find(|host| host.id == id)
        {
            host.enqueue(Event::Open(self.external_open.take()));
        } else if routes.is_empty() && now >= self.reopen_not_before.get() {
            self.reopen_not_before.set(now + IDLE_PERIOD);
            let template = self.template.borrow().clone();
            if let Some(template) = template {
                template.with_app(|app, w| app.on_reopen(w));
            }
            if !self.windows.borrow().iter().any(|h| !h.closing.get()) {
                let factory = self.factory.borrow_mut().take();
                if let Some(mut factory) = factory {
                    let created = factory().and_then(|(options, app)| self.launch(options, app));
                    if let Err(error) = created {
                        eprintln!("velocity: could not reopen a window: {error}");
                    }
                    let mut slot = self.factory.borrow_mut();
                    if slot.is_none() {
                        *slot = Some(factory);
                    }
                }
            }
            if !self.windows.borrow().iter().any(|h| !h.closing.get()) {
                // A handler without reopen support must not spin on the same input.
                self.external_open.borrow_mut().clear();
            }
        }
    }

    fn run_loop(self: &Rc<Self>) -> Result<(), String> {
        let timer = WakeTimer::new();
        while !self.stop.get() {
            for _ in 0..64 {
                let mut msg = MSG::default();
                // SAFETY: msg is writable; read any queued message on this GUI thread.
                if unsafe { PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) } == 0 {
                    break;
                }
                if msg.message == WM_QUIT {
                    self.stop.set(true);
                    break;
                }
                let host = self.lookup(msg.hwnd);
                if let Some(host) = &host {
                    host.drain();
                }
                let consumed = host.as_ref().is_some_and(|h| h.offer_key(&msg));
                if !consumed {
                    // SAFETY: msg is a message retrieved from this thread's queue.
                    unsafe {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                if let Some(host) = host {
                    host.drain();
                }
                if self.stop.get() || self.quit_requested.get() {
                    break;
                }
            }
            if !self.stop.get() {
                self.approve_quit();
            }
            if self.stop.get() {
                break;
            }
            self.reap();
            let now = Instant::now();
            self.route_external_opens(now);
            self.reap();
            let windows = self.windows.borrow().clone();
            for host in &windows {
                host.tick(Instant::now())?;
                if self.stop.get() {
                    break;
                }
            }
            if self.stop.get()
                || self.quit_requested.get()
                || windows.iter().any(|w| w.closing.get())
            {
                continue;
            }
            self.poll_memory(Instant::now(), &windows);
            let template = self.template.borrow().clone();
            if windows.is_empty()
                && let Some(template) = &template
                && template.idle_at.get() <= now
            {
                template.idle_at.set(now + IDLE_PERIOD);
                template.with_app(|app, w| app.on_idle(w));
                if self.stop.get() || !self.windows.borrow().is_empty() || self.quit_requested.get()
                {
                    continue;
                }
            }
            let now = Instant::now();
            let deadline = windows
                .iter()
                .filter_map(|w| w.wake(now))
                .chain(if windows.is_empty() {
                    template.as_ref().map(|w| w.idle_at.get())
                } else {
                    None
                })
                .chain(
                    (!self.external_open.borrow().is_empty())
                        .then_some(self.reopen_not_before.get()),
                )
                .min();
            timer.wait(deadline)?;
        }
        self.failure.take().map_or(Ok(()), Err)
    }
}

struct Session(Rc<Runtime>);

impl Drop for Session {
    fn drop(&mut self) {
        let runtime = &self.0;
        runtime.stop.set(true);
        let mut windows = runtime.windows.take();
        windows.extend(runtime.template.take());
        // Break any handle cycles through the app/session before destroying
        // any HWND; a DXGI surface can never outlive its native window.
        for host in &windows {
            drop(host.app.borrow_mut().take());
        }
        for host in windows {
            host.destroy();
        }
    }
}

struct Class(HINSTANCE);

impl Drop for Class {
    fn drop(&mut self) {
        // SAFETY: Session drops (and destroys its HWNDs) before unregistering.
        unsafe { UnregisterClassW(CLASS, self.0) };
    }
}

struct DpiGuard(DPI_AWARENESS_CONTEXT);

impl Drop for DpiGuard {
    fn drop(&mut self) {
        // SAFETY: We restore the calling thread's previous DPI context.
        unsafe { SetThreadDpiAwarenessContext(self.0) };
    }
}

pub(super) fn run(options: WindowOptions, app: Box<dyn AppHandler>) -> Result<(), String> {
    // SAFETY: The API changes only this process/thread DPI policy. If a process
    // manifest set it already, still request PMv2 for this GUI thread.
    let _dpi = unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let previous = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        if previous.is_null() {
            return Err(win_error("SetThreadDpiAwarenessContext"));
        }
        DpiGuard(previous)
    };
    let _apartment = shell::Apartment::new()?;
    // SAFETY: The module is the running executable (no input name). Shared
    // system cursor/icon handles need not be freed. Registration copies class data.
    let class = unsafe {
        let instance = GetModuleHandleW(null());
        if instance.is_null() {
            return Err(win_error("GetModuleHandleW"));
        }
        let icon = LoadIconW(instance, 101usize as *const u16);
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hIcon: if icon.is_null() {
                LoadIconW(null_mut(), IDI_APPLICATION)
            } else {
                icon
            },
            lpszClassName: CLASS,
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return Err(win_error("RegisterClassExW"));
        }
        Class(instance)
    };
    memory_guard::reset();
    let session = Session(Rc::new(Runtime {
        instance: class.0,
        // SAFETY: This is the terminated name of a Shell notification.
        taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarButtonCreated")) },
        windows: RefCell::default(),
        template: RefCell::default(),
        factory: RefCell::default(),
        next_id: Cell::new(1),
        stop: Cell::new(false),
        quit_requested: Cell::new(false),
        failure: RefCell::default(),
        external_open: RefCell::default(),
        reopen_not_before: Cell::new(Instant::now()),
        memory_pressure: memory_guard::windows::PressureSource::new(),
    }));
    session.0.launch(options, app)?;
    session.0.run_loop()
}

struct WakeTimer(HANDLE);

impl WakeTimer {
    fn new() -> Self {
        // SAFETY: An unnamed synchronization timer with default security attributes.
        Self(unsafe {
            CreateWaitableTimerExW(
                null(),
                null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS,
            )
        })
    }

    fn wait(&self, deadline: Option<Instant>) -> Result<(), String> {
        let mut timeout = INFINITE;
        let mut handles = 0;
        if let Some(deadline) = deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(());
            }
            let due = -(remaining.as_nanos().div_ceil(100).min(i64::MAX as u128) as i64);
            let armed = if self.0.is_null() {
                false
            } else {
                // SAFETY: due is relative; no caller-owned data is retained.
                (unsafe { SetWaitableTimer(self.0, &due, 0, None, null(), 0) }) != 0
            };
            if armed {
                handles = 1;
            } else {
                timeout = remaining
                    .as_nanos()
                    .div_ceil(1_000_000)
                    .min(u128::from(INFINITE - 1)) as u32;
            }
        }
        // SAFETY: The array has one initialized handle when handles==1, otherwise ignored.
        let wait = unsafe {
            MsgWaitForMultipleObjectsEx(handles, &self.0, timeout, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
        };
        if wait == WAIT_FAILED {
            Err(win_error("MsgWaitForMultipleObjectsEx"))
        } else {
            Ok(())
        }
    }
}

impl Drop for WakeTimer {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: This owns the successful CreateWaitableTimerExW result.
            unsafe { CloseHandle(self.0) };
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: Win32 supplies CREATESTRUCTW during WM_NCCREATE. lpCreateParams
    // comes from Runtime::launch; the Host remains retained until WM_NCDESTROY.
    let pointer = unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam as *const CREATESTRUCTW);
            let pointer = create.lpCreateParams.cast::<Host>();
            if !pointer.is_null() {
                (*pointer).hwnd.set(hwnd);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, pointer as isize);
            }
            pointer
        } else {
            GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Host
        }
    };
    // SAFETY: The pointer, if present, comes from the live Host described above.
    if let Some(host) = unsafe { pointer.as_ref() } {
        if message == WM_NCDESTROY {
            host.hwnd.set(null_mut());
            host.visible.set(false);
            // Normal teardown has already dropped the app. An unexpected
            // destruction can no longer present; stop before another frame.
            if !host.closing.replace(true)
                && host.ready.get()
                && let Some(runtime) = host.runtime.upgrade()
                && !runtime.stop.get()
            {
                runtime.failure.replace(Some(
                    "native window was destroyed outside its lifecycle".into(),
                ));
                runtime.stop.set(true);
            }
            // SAFETY: Clear the slot before the native window ceases to exist.
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
        }
        let result = catch_unwind(AssertUnwindSafe(|| {
            host.message(hwnd, message, wparam, lparam)
        }));
        match result {
            Ok(Some(result)) => return result,
            Ok(None) => {}
            Err(_) => {
                if let Some(runtime) = host.runtime.upgrade() {
                    runtime
                        .failure
                        .replace(Some(format!("panic processing Win32 message {message:#x}")));
                    runtime.stop.set(true);
                }
                return 0;
            }
        }
    }
    // SAFETY: Forward only messages we did not consume, with the original Win32 arguments.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

impl Host {
    fn message(&self, hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        match message {
            WM_CLOSE => self.enqueue(Event::Close),
            WM_QUERYENDSESSION => {
                if !self.closing.get()
                    && let Ok(mut slot) = self.app.try_borrow_mut()
                    && let Some(app) = slot.as_mut()
                    && let Some(host) = self
                        .runtime
                        .upgrade()
                        .and_then(|runtime| runtime.lookup(hwnd))
                {
                    app.on_session_ending(&host.handle());
                }
                return Some(1);
            }
            WM_ENDSESSION if wparam != 0 => {
                if let Some(runtime) = self.runtime.upgrade() {
                    runtime.stop.set(true);
                }
            }
            WM_SIZE => {
                self.minimized.set(wparam == SIZE_MINIMIZED as usize);
                if !self.minimized.get() {
                    self.sync_geometry(true);
                }
            }
            WM_MOVE => {
                self.sync_geometry(true);
                self.enqueue(Event::Moved);
            }
            WM_DISPLAYCHANGE => {
                self.monitor.set(null_mut());
                self.sync_geometry(true);
            }
            WM_DPICHANGED => {
                self.dpi_changed(wparam, lparam);
            }
            WM_GETMINMAXINFO => {
                self.minimum_track_size(hwnd, lparam);
                return None;
            }
            WM_ENTERSIZEMOVE => {
                self.sizing.set(true);
                // SAFETY: A window-owned timer, removed on WM_EXITSIZEMOVE or HWND destruction.
                unsafe { SetTimer(hwnd, SIZING_TIMER, 16, None) };
            }
            WM_EXITSIZEMOVE => {
                self.sizing.set(false);
                // SAFETY: Only the timer installed for this HWND above is removed.
                unsafe { KillTimer(hwnd, SIZING_TIMER) };
                self.sync_geometry(true);
            }
            WM_TIMER if wparam == SIZING_TIMER && self.sizing.get() => {
                if self.app.try_borrow_mut().is_ok()
                    && let Some(runtime) = self.runtime.upgrade()
                    && let Some(host) = runtime.lookup(hwnd)
                    && let Err(error) = host.tick(Instant::now())
                {
                    runtime.failure.replace(Some(error));
                    runtime.stop.set(true);
                }
            }
            WM_SHOWWINDOW => {
                self.visible.set(wparam != 0);
                if wparam != 0 {
                    self.request(FrameReason::Resize);
                }
                return None;
            }
            WM_ACTIVATEAPP if wparam != 0 => memory_guard::became_foreground(),
            WM_ACTIVATE | WM_SETFOCUS => {
                if self.is_drawable()
                    && (message == WM_SETFOCUS || wparam as u16 != WA_INACTIVE as u16)
                {
                    memory_guard::became_foreground();
                }
                self.request(FrameReason::Resize);
            }
            WM_KILLFOCUS => {
                self.discard_ime(hwnd);
                self.context.borrow_mut().cancel();
                self.enqueue(Event::FocusLost);
                self.request(FrameReason::Resize);
            }
            WM_SETTINGCHANGE | WM_THEMECHANGED | WM_SYSCOLORCHANGE => {
                self.appearance_changed();
                self.enqueue(Event::Preferences);
                return None;
            }
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                // SAFETY: Validate only this paint region without GDI drawing; DX12 owns pixels.
                unsafe {
                    BeginPaint(hwnd, &mut paint);
                    EndPaint(hwnd, &paint);
                }
                self.request(FrameReason::Resize);
            }
            WM_ERASEBKGND => return Some(1),
            WM_DROPFILES => self.enqueue(Event::Open(shell::dropped_paths(wparam))),
            WM_COPYDATA => {
                let Some(paths) = shell::copydata_paths(lparam) else {
                    return Some(0);
                };
                let Some(runtime) = self.runtime.upgrade() else {
                    return Some(0);
                };
                runtime.external_open.borrow_mut().extend(paths);
                return Some(1);
            }
            WM_ENTERMENULOOP => self.menu_tracking.set(true),
            WM_EXITMENULOOP => self.menu_tracking.set(false),
            WM_INITMENUPOPUP => {
                if (lparam >> 16) & 0xffff == 0 {
                    self.refresh_popup(wparam as HMENU);
                }
                return None;
            }
            WM_COMMAND if lparam == 0 && (wparam >> 16) & 0xffff == 0 => {
                if let Some(action) = self.menu_action(wparam as u16) {
                    self.enqueue(Event::Named(action));
                }
            }
            _ if self
                .runtime
                .upgrade()
                .is_some_and(|r| r.taskbar_created != 0 && r.taskbar_created == message) =>
            {
                self.taskbar_busy.set(false);
                self.request(FrameReason::Resize);
            }
            _ => return self.input_message(hwnd, message, wparam, lparam),
        }
        Some(0)
    }
}

fn win_error(call: &str) -> String {
    format!("{call}: {}", io::Error::last_os_error())
}

/// The Windows accent color (Settings › Personalization › Colors), stored by DWM as
/// 0xAABBGGRR under HKCU\Software\Microsoft\Windows\DWM\AccentColor.
pub(crate) fn accent_color() -> Option<[f32; 4]> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: Both strings are NUL-terminated literals and the output buffer is a live u32
    // whose size is passed in `size`; no pointer is retained by the call.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\DWM"),
            w!("AccentColor"),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        )
    };
    (status == 0).then(|| {
        let channel = |shift: u32| ((value >> shift) & 0xff) as f32 / 255.;
        [channel(0), channel(8), channel(16), 1.]
    })
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16()
        .map(|c| if c == 0 { 0xfffd } else { c })
        .chain([0])
        .collect()
}
