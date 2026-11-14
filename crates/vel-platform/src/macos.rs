//! AppKit ownership and Objective-C interop are confined to this module.
use crate::*;
use block2::RcBlock;
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, ProtocolObject, Sel},
    sel,
};
use objc2_app_kit::*;
use objc2_foundation::{
    NSArray, NSAttributedString, NSAttributedStringKey, NSNotification, NSObject, NSObjectProtocol,
    NSPoint, NSRange, NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize, NSString, NSTimer,
};
use objc2_metal::MTLDrawable;
use objc2_quartz_core::{
    CACurrentMediaTime, CADisplayLink, CAFrameRateRange, CAMetalDrawable, CAMetalLayer,
    CATransaction,
};
use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::VecDeque,
    ffi::c_void,
    fmt,
    ptr::NonNull,
    rc::{Rc, Weak},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

/// Retains the native window and layer on the main thread. Clones are inexpensive.
#[derive(Clone)]
pub struct WindowHandle(Rc<Host>);
impl fmt::Debug for WindowHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WindowHandle")
            .field("size", &self.content_size())
            .field("scale", &self.scale_factor())
            .finish()
    }
}
impl WindowHandle {
    /// Sequence of the most recently acquired Metal drawable, for scanout correlation.
    pub fn last_acquired_drawable(&self) -> Option<u64> {
        self.0
            .layer
            .get()
            .and_then(|l| l.ivars().sequence.load(Ordering::Relaxed).checked_sub(1))
    }
    /// Current effective macOS appearance, including before the first redraw.
    pub fn appearance(&self) -> Appearance {
        let Some(view) = self.0.view.get() else {
            return Appearance::Dark;
        };
        // SAFETY: AppKit's immutable appearance constants are read on the main thread.
        let (dark, light) = unsafe { (NSAppearanceNameDarkAqua, NSAppearanceNameAqua) };
        if view
            .effectiveAppearance()
            .bestMatchFromAppearancesWithNames(&NSArray::from_slice(&[dark, light]))
            .as_deref()
            == Some(dark)
        {
            Appearance::Dark
        } else {
            Appearance::Light
        }
    }
    /// Whether this native window currently receives keyboard input.
    pub fn is_focused(&self) -> bool {
        self.0.window.get().is_some_and(|w| w.isKeyWindow())
    }
    /// Coalesces a redraw and wakes the paused display link.
    pub fn request_frame(&self) {
        self.0.dirty.set(true);
        self.0.pace();
    }
    /// Schedules a one-shot deadline; it does not keep the display link spinning.
    pub fn request_frame_at(&self, deadline: Instant) {
        let Some(view) = self.0.view.get() else {
            return;
        };
        if let Some(timer) = self.0.timer.take() {
            timer.invalidate();
        }
        let delay = deadline
            .saturating_duration_since(Instant::now())
            .as_secs_f64()
            .max(0.0001);
        // SAFETY: target is retained by the host; wake: has NSTimer's required signature,
        // and both timer creation and registration occur on AppKit's main run loop.
        let timer = unsafe {
            NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                delay,
                view,
                sel!(wake:),
                None,
                false,
            )
        };
        // SAFETY: timer and common-mode constant belong to the main run loop.
        unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
        self.0.timer.set(Some(timer));
    }
    /// Runs dirty animation at the current display's maximum refresh rate.
    pub fn set_animating(&self, active: bool) {
        self.0.animating.set(active);
        self.0.pace();
    }
    /// Changes the native title.
    pub fn set_title(&self, title: &str) {
        if let Some(w) = self.0.window.get() {
            w.setTitle(&NSString::from_str(title));
        }
    }
    /// Installs resource-defined menus, retaining the standard application menu.
    pub fn set_menu(&self, entries: &[MenuEntry]) {
        let main = NSApplication::sharedApplication(self.0.mtm)
            .mainMenu()
            .expect("application menu");
        while main.numberOfItems() > 1 {
            main.removeItemAtIndex(1);
        }
        self.0.menu_commands.borrow_mut().clear();
        append_menu(&self.0, &main, entries);
    }
    /// Opens a native multi-file chooser.
    pub fn open_files(&self) -> Vec<std::path::PathBuf> {
        let panel = NSOpenPanel::openPanel(self.0.mtm);
        panel.setAllowsMultipleSelection(true);
        panel.setCanChooseDirectories(false);
        if panel.runModal() != NSModalResponseOK {
            return Vec::new();
        }
        panel
            .URLs()
            .iter()
            .filter_map(|u| u.path().map(|p| std::path::PathBuf::from(p.to_string())))
            .collect()
    }
    /// Chooses a project folder; cancellation leaves the project untouched.
    pub fn open_directory(&self) -> Option<std::path::PathBuf> {
        let panel = NSOpenPanel::openPanel(self.0.mtm);
        panel.setAllowsMultipleSelection(false);
        panel.setCanChooseDirectories(true);
        panel.setCanChooseFiles(false);
        if panel.runModal() != NSModalResponseOK {
            return None;
        }
        panel
            .URL()
            .and_then(|url| url.path())
            .map(|p| std::path::PathBuf::from(p.to_string()))
    }
    /// Chooses a destination; cancellation leaves the document untouched.
    pub fn save_file(&self, suggested: &str) -> Option<std::path::PathBuf> {
        let panel = NSSavePanel::savePanel(self.0.mtm);
        panel.setNameFieldStringValue(&NSString::from_str(suggested));
        if panel.runModal() != NSModalResponseOK {
            return None;
        }
        panel
            .URL()
            .and_then(|u| u.path())
            .map(|p| p.to_string().into())
    }
    /// Displays a native alert and returns the zero-based chosen button.
    pub fn alert(&self, message: &str, detail: &str, buttons: &[&str]) -> usize {
        let alert = NSAlert::new(self.0.mtm);
        alert.setMessageText(&NSString::from_str(message));
        alert.setInformativeText(&NSString::from_str(detail));
        for button in buttons {
            alert.addButtonWithTitle(&NSString::from_str(button));
        }
        (alert.runModal() - 1000).max(0) as usize
    }
    /// Current backing scale, updated before resize callbacks.
    pub fn scale_factor(&self) -> f64 {
        self.0.scale.get()
    }
    /// Actual content size in points.
    pub fn content_size(&self) -> Size {
        self.0.size.get()
    }
    /// Maximum frames per second on the hosting display.
    pub fn maximum_frames_per_second(&self) -> f32 {
        self.0
            .window
            .get()
            .and_then(|w| w.screen())
            .map_or(60., |s| s.maximumFramesPerSecond().max(1) as f32)
    }
    /// Borrowed CAMetalLayer pointer. A renderer must retain this handle for the
    /// entire surface lifetime, and may only create/use the surface on this thread.
    pub fn metal_layer_ptr(&self) -> *mut c_void {
        self.0.layer.get().map_or(std::ptr::null_mut(), |l| {
            Retained::as_ptr(l).cast_mut().cast()
        })
    }
    /// Takes completed drawable observations; Metal callbacks only write this queue.
    pub fn take_presentations(&self) -> Vec<Presentation> {
        self.0
            .presentations
            .lock()
            .map(|mut q| q.drain(..).collect())
            .unwrap_or_default()
    }
    /// Checks the hosting display, session lock and live window occlusion state.
    pub fn display_status(&self) -> DisplayStatus {
        let display = self
            .0
            .window
            .get()
            .and_then(|w| w.screen())
            .and_then(|screen| {
                let key = NSString::from_str("NSScreenNumber");
                screen.deviceDescription().objectForKey(&key).map(|number| {
                    // SAFETY: NSScreenNumber is documented as an NSNumber display ID.
                    unsafe { msg_send![&*number, unsignedIntValue] }
                })
            });
        let mut status = crate::display::status_for_display(display);
        status.window_occluded = self
            .0
            .window
            .get()
            .is_some_and(|w| !w.occlusionState().contains(NSWindowOcclusionState::Visible));
        status
    }
    /// Reads a UTF-8 string from the general pasteboard.
    pub fn clipboard_text(&self) -> Option<String> {
        // SAFETY: AppKit exports a process-lifetime immutable NSString constant.
        NSPasteboard::generalPasteboard()
            .stringForType(unsafe { NSPasteboardTypeString })
            .map(|s| s.to_string())
    }
    /// Writes a UTF-8 string to the general pasteboard.
    pub fn set_clipboard_text(&self, text: &str) -> bool {
        let p = NSPasteboard::generalPasteboard();
        p.clearContents();
        // SAFETY: the exported pasteboard type is immutable and both strings live through the call.
        p.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString })
    }
    /// Invalidates the native IME's cached candidate-window geometry.
    pub fn invalidate_character_coordinates(&self) {
        self.0.ime_geometry_dirty.set(true);
        self.0.flush_ime_coordinates();
    }
    /// Stops run() without calling exit, allowing reports and native teardown.
    pub fn quit(&self) {
        self.0.stopped.set(true);
        let app = NSApplication::sharedApplication(self.0.mtm);
        app.stop(None);
        // Post a harmless event so stop also works during launch/notification callbacks.
        let event = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(NSEventType::ApplicationDefined, NSPoint::new(0., 0.), NSEventModifierFlags::empty(), 0., 0, None, 0, 0, 0);
        if let Some(event) = event {
            app.postEvent_atStart(&event, true);
        }
    }
}
enum Event {
    Key(KeyEvent),
    Text(TextInput),
    Command(Command),
    NamedCommand(String, String),
    Mouse(MouseEvent),
    Scroll(ScrollEvent),
    Resize,
    Appearance(Appearance),
    QuitRequested,
}
struct Host {
    mtm: MainThreadMarker,
    app: RefCell<Option<Box<dyn AppHandler>>>,
    pending: RefCell<VecDeque<Event>>,
    dispatching: Cell<bool>,
    drawing: Cell<bool>,
    input_depth: Cell<u32>,
    ime_geometry_dirty: Cell<bool>,
    ready: Cell<bool>,
    starting: Cell<bool>,
    stopped: Cell<bool>,
    visible: Cell<bool>,
    dirty: Cell<bool>,
    animating: Cell<bool>,
    size: Cell<Size>,
    scale: Cell<f64>,
    window: OnceCell<Retained<NSWindow>>,
    view: OnceCell<Retained<EditorView>>,
    layer: OnceCell<Retained<ObservedLayer>>,
    link: OnceCell<Retained<CADisplayLink>>,
    timer: Cell<Option<Retained<NSTimer>>>,
    presentations: Arc<Mutex<VecDeque<Presentation>>>,
    error: RefCell<Option<String>>,
    menu_commands: RefCell<Vec<(String, String)>>,
    input: RefCell<TextInputState>,
}
impl Host {
    fn handle(self: &Rc<Self>) -> WindowHandle {
        WindowHandle(Rc::clone(self))
    }
    fn phase(&self, phase: &'static str) {
        if let Some(app) = self.app.borrow_mut().as_mut() {
            app.on_platform_phase(phase);
        }
    }
    fn fail(self: &Rc<Self>, error: String) {
        *self.error.borrow_mut() = Some(error);
        self.handle().quit();
    }
    fn dispatch(self: &Rc<Self>, event: Event) {
        self.pending.borrow_mut().push_back(event);
        self.drain();
    }
    fn drain(self: &Rc<Self>) {
        if self.dispatching.get() || self.drawing.get() {
            return;
        }
        self.dispatching.set(true);
        loop {
            let Some(event) = self.pending.borrow_mut().pop_front() else {
                break;
            };
            let mut slot = self.app.borrow_mut();
            let Some(app) = slot.as_mut() else {
                break;
            };
            let w = self.handle();
            match event {
                Event::Key(e) => {
                    app.on_key(&w, e);
                }
                Event::NamedCommand(name, args) => app.on_named_command(&w, &name, &args),
                Event::Text(e) => app.on_text_input(&w, e),
                Event::Command(e) => app.on_command(&w, e),
                Event::Mouse(e) => app.on_mouse(&w, e),
                Event::Scroll(e) => app.on_scroll(&w, e),
                Event::Resize => app.on_resize(&w, self.size.get(), self.scale.get()),
                Event::Appearance(e) => app.on_appearance(&w, e),
                Event::QuitRequested => {
                    if app.on_quit_requested(&w) {
                        w.quit();
                    }
                }
            }
            *self.input.borrow_mut() = app.text_input_state();
        }
        self.dispatching.set(false);
        self.flush_ime_coordinates();
    }
    fn offer_key(self: &Rc<Self>, key: KeyEvent) -> bool {
        if self.input.borrow().marked.is_some() || self.dispatching.get() || self.drawing.get() {
            return false;
        }
        self.dispatching.set(true);
        let handled = if let Some(app) = self.app.borrow_mut().as_mut() {
            let handled = app.on_key(&self.handle(), key);
            *self.input.borrow_mut() = app.text_input_state();
            handled
        } else {
            false
        };
        self.dispatching.set(false);
        self.drain();
        handled
    }
    fn flush_ime_coordinates(&self) {
        if self.dispatching.get() || self.drawing.get() || !self.ime_geometry_dirty.replace(false) {
            return;
        }
        // Query callbacks now see the freshly published text/geometry snapshot.
        if let Some(context) = self.view.get().and_then(|v| v.inputContext()) {
            context.invalidateCharacterCoordinates();
        }
    }
    fn render(self: &Rc<Self>, reason: FrameReason, force: bool) {
        // AppKit may synchronously send resize/updateLayer while ordering a
        // window front. Only the explicit two launch calls may draw at this stage.
        if self.starting.get()
            && !matches!(
                reason,
                FrameReason::StartupPrepare | FrameReason::StartupPresent
            )
        {
            return;
        }
        if self.dispatching.get()
            || !self.ready.get()
            || self.stopped.get()
            || self.drawing.replace(true)
        {
            return;
        }
        if !force && (!self.dirty.get() && !self.animating.get() || !self.visible.get()) {
            self.drawing.set(false);
            self.pace();
            return;
        }
        self.dirty.set(false);
        let result = {
            let mut app = self.app.borrow_mut();
            if let Some(app) = app.as_mut() {
                let w = self.handle();
                let result = app.on_frame(&mut FrameContext {
                    window: &w,
                    received: Instant::now(),
                    reason,
                });
                *self.input.borrow_mut() = app.text_input_state();
                result
            } else {
                Ok(())
            }
        };
        self.drawing.set(false);
        self.dispatching.set(false);
        if let Err(e) = result {
            self.fail(e);
        }
        self.drain();
        self.pace();
    }
    fn pace(&self) {
        if let Some(link) = self.link.get() {
            link.setPaused(
                self.stopped.get()
                    || !self.visible.get()
                    || !(self.dirty.get() || self.animating.get()),
            );
        }
    }
    fn flush(self: &Rc<Self>) {
        if self.input_depth.get() == 0 {
            self.render(FrameReason::Input, false);
        }
    }
    fn resize(self: &Rc<Self>) {
        let (Some(view), Some(window), Some(layer)) =
            (self.view.get(), self.window.get(), self.layer.get())
        else {
            return;
        };
        let bounds = view.bounds();
        let size = Size {
            width: bounds.size.width,
            height: bounds.size.height,
        };
        let scale = window.backingScaleFactor();
        if size.width <= 0. || size.height <= 0. {
            return;
        }
        let changed = size != self.size.get() || scale != self.scale.get();
        self.size.set(size);
        self.scale.set(scale);
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        layer.setFrame(bounds);
        layer.setContentsScale(scale);
        layer.setDrawableSize(objc2_core_foundation::CGSize::new(
            (size.width * scale).round(),
            (size.height * scale).round(),
        ));
        CATransaction::commit();
        if let Some(link) = self.link.get() {
            let max = self.handle().maximum_frames_per_second();
            link.setPreferredFrameRateRange(CAFrameRateRange::new(30f32.min(max), max, max));
        }
        self.ime_geometry_dirty.set(true);
        if !changed {
            self.flush_ime_coordinates();
            return;
        }
        self.dispatch(Event::Resize);
        self.dirty.set(true);
        self.render(FrameReason::Resize, false);
    }
}
struct LayerIvars {
    sequence: AtomicU64,
    observations: Arc<Mutex<VecDeque<Presentation>>>,
}
define_class!(
    // SAFETY: CAMetalLayer has no additional subclassing invariants; all layer
    // access remains on the main thread, and the handler captures only Send state.
    #[unsafe(super = CAMetalLayer)]
    #[ivars = LayerIvars]
    struct ObservedLayer;
    // SAFETY: NSObjectProtocol has no additional requirements.
    unsafe impl NSObjectProtocol for ObservedLayer {}
    impl ObservedLayer {
        #[unsafe(method_id(nextDrawable))]

        fn next_drawable(&self) -> Option<Retained<ProtocolObject<dyn CAMetalDrawable>>> {
            self.setAllowsNextDrawableTimeout(true);
            // SAFETY: invokes CAMetalLayer's documented nextDrawable with the exact return type.
            let drawable: Option<Retained<ProtocolObject<dyn CAMetalDrawable>>> = unsafe { msg_send![super(self), nextDrawable] };
            if let Some(drawable) = &drawable {
                let sequence = self.ivars().sequence.fetch_add(1, Ordering::Relaxed);
                let observations = Arc::clone(&self.ivars().observations);
                let handler = RcBlock::new(move |drawable: NonNull<ProtocolObject<dyn MTLDrawable>>| {
                    let callback = Instant::now();
                    let host_time = CACurrentMediaTime();
                    // SAFETY: Metal supplies a live MTLDrawable for the lifetime of this block call.
                    let presented_time = unsafe { drawable.as_ref() }.presentedTime();
                    let scanout = if presented_time > 0. && presented_time.is_finite() && host_time >= presented_time {
                        callback.checked_sub(Duration::from_secs_f64(host_time - presented_time)).unwrap_or(callback)
                    } else { callback };
                    if let Ok(mut queue) = observations.lock() {
                        if queue.len() == 4096 { queue.pop_front(); }
                        queue.push_back(Presentation { drawable: sequence, presented_time, scanout, callback });
                    }
                });
                // SAFETY: Metal copies this valid heap block; captured state outlives presentation.
                unsafe { drawable.addPresentedHandler(RcBlock::as_ptr(&handler)) };
            }
            drawable
        }
    }
);
struct ViewIvars {
    host: Weak<Host>,
}
define_class!(
    // SAFETY: NSView is main-thread-only; no ivar borrows cross calls into AppKit.
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = ViewIvars]
    struct EditorView;
    // SAFETY: NSObjectProtocol has no additional requirements.
    unsafe impl NSObjectProtocol for EditorView {}
    impl EditorView {
        #[unsafe(method(isFlipped))] fn is_flipped(&self) -> bool { true }
        #[unsafe(method(acceptsFirstResponder))] fn accepts_first_responder(&self) -> bool { true }
        #[unsafe(method(wantsUpdateLayer))] fn wants_update_layer(&self) -> bool { true }
        #[unsafe(method(updateLayer))] fn update_layer(&self) { if let Some(h) = self.ivars().host.upgrade() { h.dirty.set(true); h.render(FrameReason::Resize, false); } }
        #[unsafe(method(setFrameSize:))] fn set_frame_size(&self, size: NSSize) {
            // SAFETY: NSView's implementation accepts this NSSize, on its main thread.
            unsafe { let _: () = msg_send![super(self), setFrameSize: size]; }
            if let Some(h) = self.ivars().host.upgrade() { h.resize(); }
        }
        #[unsafe(method(viewDidChangeBackingProperties))] fn backing_changed(&self) { if let Some(h) = self.ivars().host.upgrade() { h.resize(); } }
        #[unsafe(method(viewDidChangeEffectiveAppearance))] fn appearance_changed(&self) {
            if let Some(h) = self.ivars().host.upgrade() {
                let appearance = self.effectiveAppearance();
                // SAFETY: AppKit exports immutable appearance-name constants.
                let dark = unsafe { NSAppearanceNameDarkAqua }; let light = unsafe { NSAppearanceNameAqua };
                let matched = appearance.bestMatchFromAppearancesWithNames(&NSArray::from_slice(&[dark, light]));
                h.dispatch(Event::Appearance(if matched.as_deref() == Some(dark) { Appearance::Dark } else { Appearance::Light }));
                h.dirty.set(true); h.render(FrameReason::Resize, false);
            }
        }
        #[unsafe(method(tick:))] fn tick(&self, _link: &CADisplayLink) { if let Some(h) = self.ivars().host.upgrade() { h.render(FrameReason::DisplayLink, false); } }
        #[unsafe(method(wake:))] fn wake(&self, _timer: &NSTimer) { if let Some(h) = self.ivars().host.upgrade() { h.timer.take(); h.render(FrameReason::Timer, true); } }
        #[unsafe(method(keyDown:))] fn key_down(&self, event: &NSEvent) {
            if let Some(h) = self.ivars().host.upgrade() {
                let key = key_event(event, KeyState::Down);
                h.input_depth.set(h.input_depth.get() + 1);
                let handled=h.offer_key(key);
                if !handled { self.interpretKeyEvents(&NSArray::from_slice(&[event])); }
                h.input_depth.set(h.input_depth.get() - 1); h.flush();
            }
        }
        #[unsafe(method(performKeyEquivalent:))]
        fn key_equivalent(&self,event:&NSEvent)->bool {
            if event.modifierFlags().intersects(NSEventModifierFlags::Command|NSEventModifierFlags::Control) {
            self.ivars().host.upgrade().is_some_and(|h| {
                h.input_depth.set(h.input_depth.get()+1);
                let handled=h.offer_key(key_event(event,KeyState::Down));
                h.input_depth.set(h.input_depth.get()-1);h.flush();handled
            })
            } else { false }
        }
        #[unsafe(method(velocityCommand:))]
        fn named_command(&self, item: &NSMenuItem) {
            if let Some(h) = self.ivars().host.upgrade() {
                let command = h.menu_commands.borrow().get(item.tag().max(0) as usize).cloned();
                if let Some((name,args)) = command { h.dispatch(Event::NamedCommand(name,args)); h.flush(); }
            }
        }
        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            self.ivars().host.upgrade().is_some_and(|h| {
                let commands=h.menu_commands.borrow();
                commands.get(item.tag().max(0) as usize).is_some_and(|(name,args)| {
                    h.app.try_borrow().ok().is_some_and(|app|app.as_ref().is_some_and(|app|app.command_enabled(name,args)))
                })
            })
        }
        #[unsafe(method(keyUp:))] fn key_up(&self, e: &NSEvent) { if let Some(h) = self.ivars().host.upgrade() { h.dispatch(Event::Key(key_event(e, KeyState::Up))); } }
        #[unsafe(method(flagsChanged:))] fn flags_changed(&self, e: &NSEvent) { if let Some(h) = self.ivars().host.upgrade() { h.dispatch(Event::Key(key_event(e, KeyState::FlagsChanged))); } }
        #[unsafe(method(mouseDown:))] fn mouse_down(&self, e: &NSEvent) { self.mouse(e, MouseKind::Down); }
        #[unsafe(method(mouseMoved:))] fn mouse_moved(&self, e: &NSEvent) { self.mouse(e, MouseKind::Move); }
        #[unsafe(method(mouseDragged:))] fn mouse_dragged(&self, e: &NSEvent) { self.mouse(e, MouseKind::Drag); }
        #[unsafe(method(mouseUp:))] fn mouse_up(&self, e: &NSEvent) { self.mouse(e, MouseKind::Up); }
        #[unsafe(method(rightMouseDown:))] fn right_down(&self, e: &NSEvent) { self.mouse(e, MouseKind::Down); }
        #[unsafe(method(rightMouseDragged:))] fn right_dragged(&self, e: &NSEvent) { self.mouse(e, MouseKind::Drag); }
        #[unsafe(method(rightMouseUp:))] fn right_up(&self, e: &NSEvent) { self.mouse(e, MouseKind::Up); }
        #[unsafe(method(otherMouseDown:))] fn other_down(&self, e: &NSEvent) { self.mouse(e, MouseKind::Down); }
        #[unsafe(method(otherMouseDragged:))] fn other_dragged(&self, e: &NSEvent) { self.mouse(e, MouseKind::Drag); }
        #[unsafe(method(otherMouseUp:))] fn other_up(&self, e: &NSEvent) { self.mouse(e, MouseKind::Up); }
        #[unsafe(method(resetCursorRects))] fn reset_cursor_rects(&self) {
            self.addCursorRect_cursor(self.bounds(), &NSCursor::IBeamCursor());
        }
        #[unsafe(method(scrollWheel:))] fn scroll(&self, e: &NSEvent) {
            if let Some(h) = self.ivars().host.upgrade() {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                h.dispatch(Event::Scroll(ScrollEvent { position: Point { x: p.x, y: p.y }, delta_x: e.scrollingDeltaX(), delta_y: e.scrollingDeltaY(), precise: e.hasPreciseScrollingDeltas(), phase: e.phase().0 as u64, momentum_phase: e.momentumPhase().0 as u64, modifiers: Modifiers(e.modifierFlags().0 as u64), timestamp: e.timestamp() }));
                h.handle().invalidate_character_coordinates(); h.flush();
            }
        }
        #[unsafe(method(undo:))] fn undo(&self, _sender: Option<&AnyObject>) { self.command(Command::Undo); }
        #[unsafe(method(redo:))] fn redo(&self, _sender: Option<&AnyObject>) { self.command(Command::Redo); }
        #[unsafe(method(cut:))] fn cut(&self, _sender: Option<&AnyObject>) { self.command(Command::Cut); }
        #[unsafe(method(copy:))] fn copy(&self, _sender: Option<&AnyObject>) { self.command(Command::Copy); }
        #[unsafe(method(paste:))] fn paste(&self, _sender: Option<&AnyObject>) { self.command(Command::Paste); }
        #[unsafe(method(selectAll:))] fn select_all(&self, _sender: Option<&AnyObject>) { self.command(Command::SelectAll); }
    }
    // SAFETY: all required NSTextInputClient signatures below match AppKit. Ranges
    // are UTF-16 and all nullable out-pointers are checked before writing.
    unsafe impl NSTextInputClient for EditorView {
        #[unsafe(method(insertText:replacementRange:))]
        unsafe fn insert(&self, value: &AnyObject, replacement: NSRange) { self.text(TextInput::Insert { text: plain_string(value), replacement: from_range(replacement) }); }
        #[unsafe(method(setMarkedText:selectedRange:replacementRange:))]
        unsafe fn mark(&self, value: &AnyObject, selected: NSRange, replacement: NSRange) { self.text(TextInput::Marked { text: plain_string(value), selected: from_range(selected).unwrap_or_default(), replacement: from_range(replacement) }); }
        #[unsafe(method(unmarkText))] fn unmark(&self) { self.text(TextInput::Unmark); }
        #[unsafe(method(hasMarkedText))] fn has_marked(&self) -> bool { self.state().marked.is_some_and(|r| r.length > 0) }
        #[unsafe(method(markedRange))] fn marked_range(&self) -> NSRange { to_range(self.state().marked) }
        #[unsafe(method(selectedRange))] fn selected_range(&self) -> NSRange { to_range(Some(self.state().selected)) }
        #[unsafe(method_id(validAttributesForMarkedText))]  fn valid_attributes(&self) -> Retained<NSArray<NSAttributedStringKey>> { NSArray::new() }
        #[unsafe(method_id(attributedSubstringForProposedRange:actualRange:))]

        unsafe fn substring(&self, proposed: NSRange, actual: *mut NSRange) -> Option<Retained<NSAttributedString>> {
            let result = from_range(proposed).and_then(|range| self.state().substring(range));
            if !actual.is_null() {
                // SAFETY: NSTextInputClient caller provides a writable out-range or null.
                unsafe { actual.write(to_range(result.as_ref().map(|r| r.1))) };
            }
            result.map(|(s, _)| NSAttributedString::initWithString(NSAttributedString::alloc(), &NSString::from_str(&s)))
        }
        #[unsafe(method(firstRectForCharacterRange:actualRange:))]
        unsafe fn first_rect(&self, proposed: NSRange, actual: *mut NSRange) -> NSRect {
            let state = self.state();
            let mut result = (state.caret, state.selected);
            if let Some(h) = self.ivars().host.upgrade() && let Ok(app) = h.app.try_borrow() && let Some(app) = app.as_ref() { result = app.text_input_rect(from_range(proposed).unwrap_or(state.selected)); }
            if !actual.is_null() {
                // SAFETY: AppKit's optional out-parameter is valid for one NSRange.
                unsafe { actual.write(to_range(Some(result.1))) };
            }
            let r = result.0.clamped(self.ivars().host.upgrade().map_or(Size::default(), |h| h.size.get()));
            let view_rect = NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.width, r.height));
            // convertRect handles the flipped view and Retina without multiplying by scale.
            let window_rect = self.convertRect_toView(view_rect, None);
            self.window().map_or(window_rect, |w| w.convertRectToScreen(window_rect))
        }
        #[unsafe(method(characterIndexForPoint:))]
        fn character_index(&self, point: NSPoint) -> usize {
            let Some(w) = self.window() else { return usize::MAX; };
            let p = self.convertPoint_fromView(w.convertPointFromScreen(point), None);
            self.ivars().host.upgrade().and_then(|h| h.app.try_borrow().ok().and_then(|app| app.as_ref().and_then(|a| a.character_index_for_point(Point { x: p.x, y: p.y })))).unwrap_or(usize::MAX)
        }
        #[unsafe(method(doCommandBySelector:))]
        unsafe fn do_command(&self, selector: Sel) { if let Some(command) = Command::from_selector(&selector.name().to_string_lossy()) { self.command(command); } }
    }
);
impl EditorView {
    fn state(&self) -> TextInputState {
        self.ivars()
            .host
            .upgrade()
            .map_or_else(TextInputState::default, |h| h.input.borrow().clone())
    }
    fn command(&self, c: Command) {
        if let Some(h) = self.ivars().host.upgrade() {
            h.dispatch(Event::Command(c));
            h.flush();
        }
    }
    fn text(&self, t: TextInput) {
        if let Some(h) = self.ivars().host.upgrade() {
            h.dispatch(Event::Text(t));
            h.flush();
        }
    }
    fn mouse(&self, e: &NSEvent, kind: MouseKind) {
        if let Some(h) = self.ivars().host.upgrade() {
            if kind == MouseKind::Down {
                if let Some(w) = self.window() {
                    w.makeFirstResponder(Some(self));
                }
                if let Some(c) = self.inputContext() {
                    c.discardMarkedText();
                }
            }
            let p = self.convertPoint_fromView(e.locationInWindow(), None);
            h.dispatch(Event::Mouse(MouseEvent {
                button: e.buttonNumber().max(0) as usize,
                kind,
                position: Point { x: p.x, y: p.y },
                click_count: e.clickCount().max(0) as usize,
                modifiers: Modifiers(e.modifierFlags().0 as u64),
                timestamp: e.timestamp(),
            }));
            h.flush();
        }
    }
}
fn key_event(e: &NSEvent, state: KeyState) -> KeyEvent {
    let is_key = state != KeyState::FlagsChanged;
    KeyEvent {
        physical_code: e.keyCode(),
        characters: if is_key {
            e.characters().map(|s| s.to_string()).unwrap_or_default()
        } else {
            String::new()
        },
        characters_ignoring_modifiers: if is_key {
            e.charactersIgnoringModifiers()
                .map(|s| s.to_string())
                .unwrap_or_default()
        } else {
            String::new()
        },
        modifiers: Modifiers(e.modifierFlags().0 as u64),
        repeat: is_key && e.isARepeat(),
        timestamp: e.timestamp(),
        received: Instant::now(),
        state,
    }
}
fn from_range(range: NSRange) -> Option<Utf16Range> {
    (range.location != usize::MAX && range.location != isize::MAX as usize)
        .then_some(Utf16Range::new(range.location, range.length))
}
fn to_range(range: Option<Utf16Range>) -> NSRange {
    range.map_or(NSRange::new(isize::MAX as usize, 0), |r| {
        NSRange::new(r.location, r.length)
    })
}
fn plain_string(value: &AnyObject) -> String {
    if let Some(s) = value.downcast_ref::<NSString>() {
        s.to_string()
    } else if let Some(s) = value.downcast_ref::<NSAttributedString>() {
        s.string().to_string()
    } else {
        String::new()
    }
}
struct DelegateIvars {
    host: Rc<Host>,
    options: WindowOptions,
}
define_class!(
    // SAFETY: NSObject has no subclassing invariants; delegate stays on the main thread.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = DelegateIvars]
    struct Delegate;
    // SAFETY: NSObjectProtocol has no additional requirements.
    unsafe impl NSObjectProtocol for Delegate {}
    // SAFETY: methods implement the documented NSApplicationDelegate signatures.
    unsafe impl NSApplicationDelegate for Delegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn launched(&self, _n: &NSNotification) {
            if let Err(e) = self.launch() {
                self.ivars().host.fail(e);
            }
        }
        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _app: &NSApplication) -> NSApplicationTerminateReply {
            self.close_requested();
            NSApplicationTerminateReply::TerminateCancel
        }
    }
    // SAFETY: methods match NSWindowDelegate; the host owns the NSWindow.
    unsafe impl NSWindowDelegate for Delegate {
        #[unsafe(method(windowShouldClose:))]
        fn should_close(&self, _sender: &NSWindow) -> bool {
            self.close_requested();
            false
        }
        #[unsafe(method(windowDidResize:))]
        fn resized(&self, _n: &NSNotification) {
            self.ivars().host.resize();
        }
        #[unsafe(method(windowDidChangeBackingProperties:))]
        fn scale_changed(&self, _n: &NSNotification) {
            self.ivars().host.resize();
        }
        #[unsafe(method(windowDidChangeScreen:))]
        fn screen_changed(&self, _n: &NSNotification) {
            self.ivars().host.resize();
        }
        #[unsafe(method(windowDidChangeOcclusionState:))]
        fn occlusion(&self, _n: &NSNotification) {
            let h = &self.ivars().host;
            let visible = h
                .window
                .get()
                .is_some_and(|w| w.occlusionState().contains(NSWindowOcclusionState::Visible));
            h.visible.set(visible);
            if visible {
                h.dirty.set(true);
            }
            h.pace();
        }
        #[unsafe(method(windowDidResignKey:))]
        fn resigned(&self, _n: &NSNotification) {
            if let Some(v) = self.ivars().host.view.get() {
                if let Some(c) = v.inputContext() {
                    c.discardMarkedText();
                }
                v.text(TextInput::Unmark);
            }
        }
    }
);
impl Delegate {
    fn close_requested(&self) {
        let h = &self.ivars().host;
        h.dispatch(Event::QuitRequested);
    }
    fn launch(&self) -> Result<(), String> {
        let h = &self.ivars().host;
        let mtm = h.mtm;
        h.phase("application_did_finish_launching");
        // SAFETY: main-thread allocation; releasedWhenClosed is disabled immediately.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(
                    NSPoint::new(160., 160.),
                    NSSize::new(
                        self.ivars().options.size.width,
                        self.ivars().options.size.height,
                    ),
                ),
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Miniaturizable
                    | NSWindowStyleMask::Resizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: the host retains the window until explicit teardown after run returns.
        unsafe {
            window.setReleasedWhenClosed(false);
            window.setContentMinSize(NSSize::new(320., 200.));
        }
        window.setTitle(&NSString::from_str(&self.ivars().options.title));
        window.setTabbingMode(NSWindowTabbingMode::Disallowed);
        window.setCollectionBehavior(NSWindowCollectionBehavior::FullScreenPrimary);
        window.setDelegate(Some(ProtocolObject::from_ref(self)));
        let view = EditorView::alloc(mtm).set_ivars(ViewIvars {
            host: Rc::downgrade(h),
        });
        // SAFETY: NSView initWithFrame: has this exact signature; ivars are installed first.
        let view: Retained<EditorView> = unsafe {
            msg_send![super(view), initWithFrame: NSRect::new(NSPoint::new(0., 0.), window.contentLayoutRect().size)]
        };
        let layer = ObservedLayer::alloc().set_ivars(LayerIvars {
            sequence: AtomicU64::new(0),
            observations: Arc::clone(&h.presentations),
        });
        // SAFETY: CAMetalLayer inherits NSObject's initializer without extra requirements.
        let layer: Retained<ObservedLayer> = unsafe { msg_send![super(layer), init] };
        // Layer-hosting order: install the layer before enabling wantsLayer.
        view.setLayer(Some(&layer));
        view.setWantsLayer(true);
        view.setLayerContentsRedrawPolicy(NSViewLayerContentsRedrawPolicy::DuringViewResize);
        window.setContentView(Some(&view));
        window.setAcceptsMouseMovedEvents(true);
        window.makeFirstResponder(Some(&view));
        h.window
            .set(window.clone())
            .map_err(|_| "window initialized twice")?;
        h.view
            .set(view.clone())
            .map_err(|_| "view initialized twice")?;
        h.layer.set(layer).map_err(|_| "layer initialized twice")?;
        h.resize();
        h.phase("window_layer_ready");
        build_menu(mtm);
        // SAFETY: macOS 14 availability was checked before run; tick: accepts CADisplayLink.
        let link = unsafe { view.displayLinkWithTarget_selector(&view, sel!(tick:)) };
        link.setPaused(true);
        // SAFETY: register only on the main run loop, also during live-resize tracking.
        unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes) };
        h.link
            .set(link)
            .map_err(|_| "display link initialized twice")?;
        h.resize();
        h.dispatching.set(true);
        {
            let mut slot = h.app.borrow_mut();
            let app = slot.as_mut().ok_or("missing app handler")?;
            app.on_start(&h.handle())?;
            *h.input.borrow_mut() = app.text_input_state();
        }
        h.dispatching.set(false);
        h.drain();
        h.ready.set(true);
        h.render(FrameReason::StartupPrepare, true);
        if h.stopped.get() {
            return Ok(());
        }
        h.phase("first_frame_prepared_before_order_front");
        window.makeKeyAndOrderFront(None);
        h.visible.set(true);
        h.phase("ordered_front");
        // No display-link tick, redraw notification or run-loop round trip here.
        h.render(FrameReason::StartupPresent, true);
        h.starting.set(false);
        CATransaction::flush();
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
        h.phase("launch_callback_finished");
        Ok(())
    }
}
fn build_menu(mtm: MainThreadMarker) {
    let main = NSMenu::new(mtm);
    let application = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Velocity"));
    let edit = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Edit"));
    for (menu, title) in [(&application, "Velocity"), (&edit, "Edit")] {
        // SAFETY: no selector is installed on the inert submenu parent.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                None,
                &NSString::from_str(""),
            )
        };
        item.setSubmenu(Some(menu));
        main.addItem(&item);
    }
    let entries = [
        (
            &application,
            "About Velocity",
            sel!(orderFrontStandardAboutPanel:),
            "",
            false,
        ),
        (&application, "Hide Velocity", sel!(hide:), "h", false),
        (&application, "Quit Velocity", sel!(terminate:), "q", false),
        (&edit, "Undo", sel!(undo:), "z", false),
        (&edit, "Redo", sel!(redo:), "z", true),
        (&edit, "Cut", sel!(cut:), "x", false),
        (&edit, "Copy", sel!(copy:), "c", false),
        (&edit, "Paste", sel!(paste:), "v", false),
        (&edit, "Select All", sel!(selectAll:), "a", false),
    ];
    for (menu, title, action, key, shift) in entries {
        // SAFETY: each standard selector uses the responder action signature (one optional sender).
        let item = unsafe {
            menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str(title),
                Some(action),
                &NSString::from_str(key),
            )
        };
        item.setKeyEquivalentModifierMask(
            NSEventModifierFlags::Command
                | if shift {
                    NSEventModifierFlags::Shift
                } else {
                    NSEventModifierFlags::empty()
                },
        );
    }
    NSApplication::sharedApplication(mtm).setMainMenu(Some(&main));
}
fn append_menu(host: &Rc<Host>, menu: &NSMenu, entries: &[MenuEntry]) {
    for entry in entries {
        if entry.caption == "-" {
            menu.addItem(&NSMenuItem::separatorItem(host.mtm));
            continue;
        }
        // SAFETY: velocityCommand: has the one-sender menu action signature.
        let item = unsafe {
            menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str(&entry.caption),
                entry.command.as_ref().map(|_| sel!(velocityCommand:)),
                &NSString::from_str(""),
            )
        };
        if let Some(command) = &entry.command {
            let mut commands = host.menu_commands.borrow_mut();
            item.setTag(commands.len() as isize);
            commands.push((command.clone(), entry.args.clone()));
            if let Some(view) = host.view.get() {
                // SAFETY: host retains the view until every menu is torn down.
                unsafe {
                    item.setTarget(Some(view));
                }
            }
        }
        if !entry.children.is_empty() {
            let child = NSMenu::new(host.mtm);
            append_menu(host, &child, &entry.children);
            item.setSubmenu(Some(&child));
        }
    }
}
pub(super) fn run(options: WindowOptions, app: Box<dyn AppHandler>) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("Platform::run must be called on the main thread")?;
    if !objc2::available!(macos = 14.0) {
        return Err("Velocity requires macOS 14+ for view-bound CADisplayLink".into());
    }
    if !options.size.width.is_finite()
        || !options.size.height.is_finite()
        || options.size.width <= 0.
        || options.size.height <= 0.
    {
        return Err("invalid window size".into());
    }
    let host = Rc::new(Host {
        mtm,
        app: RefCell::new(Some(app)),
        pending: RefCell::new(VecDeque::new()),
        dispatching: Cell::new(false),
        drawing: Cell::new(false),
        input_depth: Cell::new(0),
        ime_geometry_dirty: Cell::new(false),
        ready: Cell::new(false),
        starting: Cell::new(true),
        stopped: Cell::new(false),
        visible: Cell::new(false),
        dirty: Cell::new(true),
        animating: Cell::new(false),
        size: Cell::new(options.size),
        scale: Cell::new(1.),
        window: OnceCell::new(),
        view: OnceCell::new(),
        layer: OnceCell::new(),
        link: OnceCell::new(),
        timer: Cell::new(None),
        presentations: Arc::new(Mutex::new(VecDeque::new())),
        error: RefCell::new(None),
        menu_commands: RefCell::new(Vec::new()),
        input: RefCell::new(TextInputState::default()),
    });
    let delegate = Delegate::alloc(mtm).set_ivars(DelegateIvars {
        host: Rc::clone(&host),
        options,
    });
    // SAFETY: NSObject initializer, after all ivars were initialized.
    let delegate: Retained<Delegate> = unsafe { msg_send![super(delegate), init] };
    let nsapp = NSApplication::sharedApplication(mtm);
    host.phase("nsapplication_ready");
    nsapp.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    NSWindow::setAllowsAutomaticWindowTabbing(false, mtm);
    nsapp.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    nsapp.run();
    if let Some(link) = host.link.get() {
        link.invalidate();
    }
    if let Some(timer) = host.timer.take() {
        timer.invalidate();
    }
    // Drop renderer/surface before the view and its retained CAMetalLayer.
    host.app.borrow_mut().take();
    if let Some(window) = host.window.get() {
        window.setDelegate(None);
        window.orderOut(None);
        window.setContentView(None);
    }
    nsapp.setDelegate(None);
    nsapp.setMainMenu(None);
    host.error.borrow_mut().take().map_or(Ok(()), Err)
}
