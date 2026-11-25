//! AppKit ownership and Objective-C interop are confined to this module.
use crate::drawable_pool::DrawablePool;
use crate::*;
#[path = "macos_a11y.rs"]
mod a11y;
#[path = "macos_finder.rs"]
pub(super) mod finder;
#[path = "macos_requests.rs"]
mod lifecycle;
#[path = "macos_menu.rs"]
mod menus;
use crate::requests::{ContextRequest, MenuInstallation, WindowTitle};
use block2::RcBlock;
use objc2::{
    AnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class,
    msg_send,
    rc::Retained,
    runtime::{AnyObject, ProtocolObject, Sel},
    sel,
};
use objc2_app_kit::*;
use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager, NSNotificationCenter};

pub(super) fn option_held_at_launch() -> bool {
    // SAFETY: NSEvent's class method modifierFlags returns the documented bitset.
    let flags: NSEventModifierFlags = unsafe { msg_send![objc2::class!(NSEvent), modifierFlags] };
    flags.contains(NSEventModifierFlags::Option)
}

pub(super) fn frontmost_pid() -> Option<i32> {
    let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
    // SAFETY: NSRunningApplication's processIdentifier returns a Darwin pid_t.
    let pid: i32 = unsafe { msg_send![&*app, processIdentifier] };
    (pid > 0).then_some(pid)
}

pub(super) fn activate_pid(pid: i32) {
    // https://developer.apple.com/documentation/appkit/nsrunningapplication/activate(options:)
    // SAFETY: the documented selector accepts a pid_t and returns a nullable running app.
    let app: Option<Retained<NSRunningApplication>> = unsafe {
        msg_send![objc2::class!(NSRunningApplication), runningApplicationWithProcessIdentifier: pid]
    };
    if let Some(app) = app {
        #[allow(deprecated)]
        app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
    }
}
use objc2_foundation::{
    NSArray, NSAttributedString, NSAttributedStringKey, NSData, NSFileManager, NSNotification,
    NSObject, NSObjectNSDelayedPerforming, NSObjectProtocol, NSPoint, NSPointInRect, NSRange,
    NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize, NSString, NSTimer, NSURL,
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
    path::{Path, PathBuf},
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
thread_local! {
    static SIMPLE_PRESENTATION: RefCell<Option<NSApplicationPresentationOptions>> = const { RefCell::new(None) };
    static WINDOWS: RefCell<Vec<Retained<Delegate>>> = const { RefCell::new(Vec::new()) };
}
fn restore_simple_presentation(mtm: MainThreadMarker) {
    let any = WINDOWS.with(|windows| {
        windows
            .borrow()
            .iter()
            .any(|d| d.ivars().host.simple_fullscreen.borrow().is_some())
    });
    if !any {
        SIMPLE_PRESENTATION.with(|saved| {
            if let Some(options) = saved.take() {
                NSApplication::sharedApplication(mtm).setPresentationOptions(options);
            }
        });
    }
}
impl WindowHandle {
    /// Font families currently exposed by AppKit's font manager.
    pub fn font_families(&self) -> Vec<String> {
        // SAFETY: a WindowHandle stays on the AppKit thread. Both selectors are
        // documented NSFontManager methods; the array contains NSStrings.
        unsafe {
            let manager: Retained<AnyObject> =
                msg_send![objc2::class!(NSFontManager), sharedFontManager];
            let names: Retained<NSArray<NSString>> = msg_send![&*manager, availableFontFamilies];
            names.iter().map(|name| name.to_string()).collect()
        }
    }

    /// Hide the pointer for typing without changing AppKit's cursor hide count.
    pub fn hide_pointer_until_motion(&self) {
        NSCursor::setHiddenUntilMouseMoves(true);
    }
    /// Whether this handle represents the windowless application template.
    pub fn is_closed(&self) -> bool {
        self.0.stopped.get()
    }
    /// Converts a point in the flipped client view to native screen coordinates.
    pub fn screen_point(&self, point: Point) -> Option<Point> {
        let window = self.0.window.get()?;
        let view = self.0.view.get()?;
        let p = view.convertPoint_toView(NSPoint::new(point.x, point.y), None);
        let p = window.convertPointToScreen(p);
        Some(Point { x: p.x, y: p.y })
    }
    /// Notifies macOS of a completed document open, or clears system recents.
    pub fn system_recent_document(&self, path: Option<&Path>) {
        let controller = NSDocumentController::sharedDocumentController(self.0.mtm);
        if let Some(path) = path {
            use std::os::unix::ffi::OsStrExt;
            if let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) {
                // SAFETY: NSURL reads a terminated OS path during this call.
                let url = unsafe {
                    NSURL::fileURLWithFileSystemRepresentation_isDirectory_relativeToURL(
                        NonNull::from(path.as_c_str()).cast(),
                        false,
                        None,
                    )
                };
                controller.noteNewRecentDocumentURL(&url);
            }
        } else {
            // SAFETY: AppKit's standard clear action permits a nil sender.
            unsafe { controller.clearRecentDocuments(None) };
        }
    }
    /// Reads the last windowed frame, excluding AppKit's full-screen transition.
    pub fn frame(&self) -> Option<Rect> {
        let w = self.0.window.get()?;
        if w.styleMask().contains(NSWindowStyleMask::FullScreen) {
            return None;
        }
        let frame = self
            .0
            .simple_fullscreen
            .borrow()
            .as_ref()
            .map_or_else(|| w.frame(), |(frame, _, _)| *frame);
        Some(screen_rect(frame))
    }
    /// Restores a frame within the current visible screen geometry.
    pub fn restore_frame(&self, frame: Rect) {
        if let Some(w) = self.0.window.get() {
            let screens: Vec<_> = NSScreen::screens(self.0.mtm)
                .into_iter()
                .map(|s| screen_rect(s.visibleFrame()))
                .collect();
            if let Some(frame) = clamp_window_frame(frame, &screens) {
                w.setFrame_display(
                    NSRect::new(
                        NSPoint::new(frame.x, frame.y),
                        NSSize::new(frame.width, frame.height),
                    ),
                    false,
                );
            }
        }
    }
    /// Registers a process-wide factory independent of any one window's lifetime.
    /// With a factory installed, closing the final window leaves the application
    /// running. Dock reopen and subsequent Finder opens create a fresh host.
    pub fn set_window_factory(
        &self,
        factory: impl FnMut() -> Result<(WindowOptions, Box<dyn AppHandler>), String> + 'static,
    ) {
        lifecycle::set_factory(Box::new(factory));
    }
    /// Creates another independently owned native window on the same AppKit loop.
    pub fn new_window(
        &self,
        options: WindowOptions,
        app: impl AppHandler,
    ) -> Result<WindowHandle, String> {
        launch_window(self.0.mtm, options, Box::new(app))
    }
    /// Dispatches an auxiliary window's command to its owning window.
    pub fn dispatch_named_command(&self, name: &str, args: &str) {
        if !self.is_closed() {
            self.0
                .dispatch(Event::NamedCommand(name.to_owned(), args.to_owned()));
        }
    }
    /// Requests normal per-window close handling, including unsaved changes.
    pub fn request_close(&self) {
        self.0.dispatch(Event::CloseRequested);
    }
    /// Schedule the native Quit action once the current handler borrow is released.
    pub fn request_app_quit(&self) {
        let app = NSApplication::sharedApplication(self.0.mtm);
        // SAFETY: terminate: is AppKit's normal application-menu action, accepts
        // a nil sender and runs through all existing quit callbacks.
        unsafe { app.performSelector_withObject_afterDelay(sel!(terminate:), None, 0.) }
    }
    /// Closes one approved window while other windows keep running.
    pub fn close(&self) {
        if self.0.stopped.replace(true) {
            return;
        }
        self.0.visible.set(false);
        *self.0.context.borrow_mut() = ContextRequest::default();
        self.0.pace();
        if let Some(timer) = self.0.timer.take() {
            timer.invalidate();
        }
        if let Some(timer) = self.0.drawable_timer.take() {
            timer.invalidate();
        }
        if self.0.simple_fullscreen.take().is_some() {
            restore_simple_presentation(self.0.mtm);
        }
        if let Some(window) = self.0.window.get() {
            window.orderOut(None);
        }
        let any = WINDOWS.with(|windows| {
            windows
                .borrow()
                .iter()
                .any(|d| !d.ivars().host.stopped.get())
        });
        if !any {
            lifecycle::keep_template(Rc::clone(&self.0));
        }
    }
    /// Brings this window forward for an interactive file/project request.
    pub fn focus(&self) {
        if let Some(window) = self.0.window.get() {
            if window.isMiniaturized() {
                window.deminiaturize(None);
            }
            window.makeKeyAndOrderFront(None);
        }
        #[allow(deprecated)]
        NSApplication::sharedApplication(self.0.mtm).activateIgnoringOtherApps(true);
    }
    /// Reads native full-screen state, including changes from the green window control.
    pub fn is_full_screen(&self) -> bool {
        self.0.simple_fullscreen.borrow().is_some()
            || self
                .0
                .window
                .get()
                .is_some_and(|w| w.styleMask().contains(NSWindowStyleMask::FullScreen))
    }
    /// Enters/exits native full screen without enabling macOS window tabs.
    pub fn toggle_full_screen(&self) {
        self.toggle_full_screen_with_mode(false);
    }
    /// Uses the current Space for simple full screen, restoring the exact frame/style on exit.
    /// Public behavior: <https://sublime-text-unofficial-documentation.readthedocs.io/en/latest/reference/settings.html#use-simple-full-screen>
    /// Current native support: <https://forum.sublimetext.com/t/build-4180-cannot-type-in-full-screen-mode/73072/17>
    pub fn toggle_full_screen_with_mode(&self, simple: bool) {
        let Some(window) = self.0.window.get() else {
            return;
        };
        let action = crate::fullscreen::transition(
            simple,
            window.styleMask().contains(NSWindowStyleMask::FullScreen),
            self.0.simple_fullscreen.borrow().is_some(),
        );
        if let Some((frame, style, _presentation)) = self.0.simple_fullscreen.take() {
            window.setStyleMask(style);
            window.setFrame_display(frame, true);
            restore_simple_presentation(self.0.mtm);
            window.makeKeyAndOrderFront(None);
        } else if action == crate::fullscreen::Transition::EnterSimple {
            let Some(screen) = window.screen() else {
                return;
            };
            let application = NSApplication::sharedApplication(self.0.mtm);
            SIMPLE_PRESENTATION.with(|saved| {
                saved
                    .borrow_mut()
                    .get_or_insert(application.presentationOptions());
            });
            let saved = (
                window.frame(),
                window.styleMask(),
                application.presentationOptions(),
            );
            *self.0.simple_fullscreen.borrow_mut() = Some(saved);
            // AutoHideMenuBar requires AutoHideDock; both flags are valid together.
            application.setPresentationOptions(
                NSApplicationPresentationOptions::AutoHideMenuBar
                    | NSApplicationPresentationOptions::AutoHideDock,
            );
            window.setStyleMask(NSWindowStyleMask::Borderless | NSWindowStyleMask::Resizable);
            window.setFrame_display(screen.frame(), true);
            window.makeKeyAndOrderFront(None);
        } else {
            window.toggleFullScreen(None);
        }
    }

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
    /// AppKit's resolved System Settings "Show scroll bars" preference.
    pub fn prefers_overlay_scrollers(&self) -> bool {
        NSScroller::preferredScrollerStyle(self.0.mtm) == NSScrollerStyle::Overlay
    }
    /// Whether this native window currently receives keyboard input.
    pub fn is_focused(&self) -> bool {
        self.0.window.get().is_some_and(|w| w.isKeyWindow())
    }
    /// Insertion indicators animate only in a key, active, visible window.
    pub fn caret_is_active(&self) -> bool {
        NSApplication::sharedApplication(self.0.mtm).isActive()
            && self.0.window.get().is_some_and(|w| {
                w.isKeyWindow() && w.occlusionState().contains(NSWindowOcclusionState::Visible)
            })
    }
    /// Honors Accessibility > Motion > Prefer Non-Blinking Cursor on macOS 15+.
    pub fn caret_blink_interval(&self) -> Option<std::time::Duration> {
        type Preference = unsafe extern "C" fn() -> objc2::runtime::Bool;
        static PREFERENCE: std::sync::OnceLock<Option<Preference>> = std::sync::OnceLock::new();
        let preference = PREFERENCE.get_or_init(|| {
            // SAFETY: Keep this public framework loaded for the life of the process.
            // Resolving the C API dynamically preserves macOS 14 support.
            unsafe {
                let framework = libc::dlopen(
                    c"/System/Library/Frameworks/Accessibility.framework/Accessibility".as_ptr(),
                    libc::RTLD_LAZY | libc::RTLD_LOCAL,
                );
                if framework.is_null() {
                    return None;
                }
                let symbol = libc::dlsym(
                    framework,
                    c"AXPrefersNonBlinkingTextInsertionIndicator".as_ptr(),
                );
                (!symbol.is_null()).then(|| std::mem::transmute::<*mut c_void, Preference>(symbol))
            }
        });
        // SAFETY: The resolved function has no arguments and returns Objective-C BOOL.
        (!preference.is_some_and(|prefer_solid| unsafe { prefer_solid().as_bool() }))
            .then_some(DEFAULT_CARET_BLINK_INTERVAL)
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
    /// Applies a theme appearance to the title-bar view while the document still observes OS changes.
    /// None restores the system title bar. Background colors use straight-alpha sRGB channels.
    pub fn set_title_bar_theme(
        &self,
        appearance: Option<Appearance>,
        background: Option<[f32; 4]>,
    ) {
        let Some(window) = self.0.window.get() else {
            return;
        };
        if let Some(view) = window
            .standardWindowButton(NSWindowButton::CloseButton)
            .and_then(|button| {
                // SAFETY: this retained standard AppKit button is alive and accessed on the main thread.
                unsafe { button.superview() }
            })
        {
            let native = appearance.and_then(|appearance| {
                // SAFETY: AppKit's immutable appearance constants are read on the main thread.
                let name = unsafe {
                    if appearance == Appearance::Dark {
                        NSAppearanceNameDarkAqua
                    } else {
                        NSAppearanceNameAqua
                    }
                };
                NSAppearance::appearanceNamed(name)
            });
            view.setAppearance(native.as_deref());
        }
        window.setTitlebarAppearsTransparent(background.is_some());
        let color = background
            .map(|c| {
                NSColor::colorWithSRGBRed_green_blue_alpha(
                    c[0] as f64,
                    c[1] as f64,
                    c[2] as f64,
                    c[3] as f64,
                )
            })
            .unwrap_or_else(NSColor::windowBackgroundColor);
        window.setBackgroundColor(Some(&color));
    }
    /// Changes the native title only when its text changes.
    pub fn set_title(&self, title: &str) {
        let Some(w) = self.0.window.get() else {
            return;
        };
        if !self.0.title.borrow_mut().update(title) {
            return;
        }
        w.setTitle(&NSString::from_str(title));
    }
    /// Installs the application menu. Identical requests from the window that installed the
    /// current menu are ignored: rebuilding NSMenu forces AppKit to relayout and redraw the
    /// menu bar, which dominated main-thread time while background results streamed in.
    pub fn set_menu(&self, entries: &[MenuEntry]) {
        if !INSTALLED_MENU.with(|installed| installed.borrow_mut().update(self.0.id, entries)) {
            return;
        }
        let app = NSApplication::sharedApplication(self.0.mtm);
        let main = NSMenu::new(self.0.mtm);
        self.0.menu_commands.borrow_mut().clear();
        append_menu(&self.0, &main, entries);
        app.setMainMenu(Some(&main));
        let submenu = |id: &str| {
            let index = entries.iter().position(|entry| entry.id == id)?;
            main.itemAtIndex(index as isize)?.submenu()
        };
        // Assign AppKit's Window list and Help search by role, never by title.
        // https://developer.apple.com/documentation/appkit/nsapplication/helpmenu
        // https://developer.apple.com/documentation/appkit/nsapplication/windowsmenu
        app.setHelpMenu(submenu("help").as_deref());
        let windows = submenu("window");
        app.setWindowsMenu(windows.as_deref());
        if windows.is_some() {
            for window in lifecycle::live_hosts()
                .iter()
                .filter_map(|h| h.window.get())
                .filter(|window| !window.isExcludedFromWindowsMenu())
            {
                app.addWindowsItem_title_filename(window, &window.title(), false);
            }
        }
    }
    /// Queues a native context menu for the most recent unconsumed mouse press.
    /// Its native event survives `on_mouse` so a later frame can offer spelling
    /// results. A later press replaces it; displaying a menu consumes it. Primary
    /// clicks are also captured for Git badges. Returns false without a captured
    /// press, for an empty menu, or a closed window.
    /// Tracking starts on a later main-run-loop turn,
    /// after app borrows end. Commands retain their exact serialized arguments
    /// and arrive through `on_named_command` after tracking returns.
    pub fn show_context_menu(&self, entries: &[MenuEntry]) -> bool {
        if self.0.stopped.get() || !self.0.context.borrow_mut().request(entries) {
            return false;
        }
        lifecycle::register_context(&self.0);
        self.0.schedule_context_menu();
        true
    }
    /// Cancels a deferred popup and its retained mouse origin before tracking starts.
    /// Call alongside clearing any app-side pending spelling menu on keyboard,
    /// text, command, or scroll input. An already-scheduled selector becomes a
    /// no-op. This only clears Rust state; it does not enter AppKit or dismiss
    /// a menu that is already tracking.
    pub fn cancel_context_menu(&self) {
        if !self.0.context_tracking.get() {
            self.0.context.borrow_mut().cancel();
        }
    }
    /// Moves a file or folder to the system Trash, without an unlink fallback.
    /// The caller confirms with the user before invoking this operation.
    pub fn trash_file(&self, path: &Path) -> Result<(), String> {
        let url = file_url(path)?;
        NSFileManager::defaultManager()
            .trashItemAtURL_resultingItemURL_error(&url, None)
            .map_err(|error| {
                format!(
                    "could not move {} to Trash: {}",
                    path.display(),
                    error.localizedDescription()
                )
            })
    }
    /// Selects a file or folder in Finder. Errors do not change the filesystem.
    pub fn reveal_file(&self, path: &Path) -> Result<(), String> {
        let url = file_url(path)?;
        std::fs::symlink_metadata(path)
            .map_err(|error| format!("could not reveal {}: {error}", path.display()))?;
        NSWorkspace::sharedWorkspace()
            .activateFileViewerSelectingURLs(&NSArray::from_slice(&[&*url]));
        Ok(())
    }
    /// Opens a directory in Finder, without selecting it inside its parent.
    pub fn show_directory(&self, path: &Path) -> Result<(), String> {
        if !path.is_dir() {
            return Err(format!("{}: directory does not exist", path.display()));
        }
        let url = file_url(path)?;
        if NSWorkspace::sharedWorkspace().openURL(&url) {
            Ok(())
        } else {
            Err(format!("could not open {}", path.display()))
        }
    }
    pub(crate) fn open_local_html(&self, path: &Path) -> bool {
        // Resolve the HTTPS handler without opening a network URL. The default
        // handler for the HTML *file type* could be Velocity or another editor.
        let Some(browser) = NSURL::URLWithString(&NSString::from_str("https://example.invalid"))
            .and_then(|url| NSWorkspace::sharedWorkspace().URLForApplicationToOpenURL(&url))
            .and_then(|url| path_from_url(&url))
        else {
            return false;
        };
        std::process::Command::new("/usr/bin/open")
            .arg("-a")
            .arg(browser)
            .arg(path)
            .status()
            .is_ok_and(|status| status.success())
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
        self.save_file_in(suggested, None, None)
    }
    /// Chooses a save destination with an optional initial directory.
    pub fn save_file_in(
        &self,
        suggested: &str,
        directory: Option<&Path>,
        captions: Option<(&str, &str)>,
    ) -> Option<std::path::PathBuf> {
        let panel = NSSavePanel::savePanel(self.0.mtm);
        panel.setNameFieldStringValue(&NSString::from_str(suggested));
        if let Some((label, prompt)) = captions {
            panel.setNameFieldLabel(Some(&NSString::from_str(label)));
            panel.setPrompt(Some(&NSString::from_str(prompt)));
        }
        if let Some(url) = directory.and_then(|path| file_url(path).ok()) {
            panel.setDirectoryURL(Some(&url));
        }
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
    /// Updates the minimum content extent needed by the current pane layout.
    pub fn set_minimum_content_size(&self, size: Size) {
        if let Some(window) = self.0.window.get() {
            let minimum = NSSize::new(size.width.max(320.), size.height.max(200.));
            window.setContentMinSize(minimum);
            let current = self.content_size();
            if current.width < minimum.width || current.height < minimum.height {
                window.setContentSize(NSSize::new(
                    current.width.max(minimum.width),
                    current.height.max(minimum.height),
                ));
            }
        }
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
    /// Updates the native drawable pool before acquisition. Idle service polls
    /// that do not draw must not call this. Returns the active drawable limit.
    pub fn begin_drawable_frame(&self, now: Instant) -> usize {
        crate::memory_pressure::note_activity();
        let mut pool = self.0.drawable_pool.get();
        let old = pool.limit();
        pool.frame(now);
        self.0.drawable_pool.set(pool);
        if pool.limit() != old {
            self.0.set_drawable_limit(pool.limit());
        }
        self.0.arm_drawable_timer(pool, now);
        pool.limit()
    }
    /// Reapplies the idle cap after wgpu configure, which always sets three.
    pub fn drawable_surface_configured(&self) {
        let limit = self.0.drawable_pool.get().limit();
        if limit != 3 {
            self.0.set_drawable_limit(limit);
        }
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
    pub(crate) fn write_clipboard_html(&self, html: &str, plain_text: &str) -> bool {
        let item = NSPasteboardItem::new();
        // SAFETY: these AppKit constants are immutable for the life of the process.
        let (html_type, text_type) = unsafe { (NSPasteboardTypeHTML, NSPasteboardTypeString) };
        if !item.setData_forType(&NSData::with_bytes(html.as_bytes()), html_type)
            || !item.setString_forType(&NSString::from_str(plain_text), text_type)
        {
            return false;
        }
        let p = NSPasteboard::generalPasteboard();
        p.clearContents();
        p.writeObjects(&NSArray::from_slice(&[ProtocolObject::from_ref(&*item)]))
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
    Mouse(MouseEvent, Option<Retained<NSEvent>>),
    OpenPaths(Vec<PathBuf>),
    FinderNewTextFile(PathBuf),
    Scroll(ScrollEvent),
    Resize,
    Appearance(Appearance),
    InlineTextFocusLost,
    CloseRequested,
    Accessibility(accessibility::Id, accessibility::Request),
    AccessibilityPreferences,
    ScrollerStyle,
}
thread_local! {
    /// Window that installed the current application menu, and the entries it installed.
    static INSTALLED_MENU: RefCell<MenuInstallation> = RefCell::new(MenuInstallation::default());
}
static NEXT_HOST_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
struct Host {
    /// Unique for the process lifetime, unlike the Rc address.
    id: u64,
    mtm: MainThreadMarker,
    app: RefCell<Option<Box<dyn AppHandler>>>,
    pending: RefCell<VecDeque<Event>>,
    dispatching: Cell<bool>,
    drawing: Cell<bool>,
    input_depth: Cell<u32>,
    ime_geometry_dirty: Cell<bool>,
    ready: Cell<bool>,
    starting: Cell<bool>,
    first_tick: Cell<bool>,
    stopped: Cell<bool>,
    visible: Cell<bool>,
    dirty: Cell<bool>,
    animating: Cell<bool>,
    size: Cell<Size>,
    scale: Cell<f64>,
    simple_fullscreen:
        RefCell<Option<(NSRect, NSWindowStyleMask, NSApplicationPresentationOptions)>>,
    window: OnceCell<Retained<NSWindow>>,
    view: OnceCell<Retained<EditorView>>,
    layer: OnceCell<Retained<ObservedLayer>>,
    link: OnceCell<Retained<CADisplayLink>>,
    timer: Cell<Option<Retained<NSTimer>>>,
    drawable_pool: Cell<DrawablePool>,
    drawable_timer: Cell<Option<Retained<NSTimer>>>,
    presentations: Arc<Mutex<VecDeque<Presentation>>>,
    error: RefCell<Option<String>>,
    /// Command, JSON args and the key equivalent Velocity installed (AppKit may rewrite the
    /// item's own equivalent for the active keyboard layout).
    menu_commands: RefCell<Vec<MenuBarAction>>,
    title: RefCell<WindowTitle>,
    context: RefCell<ContextRequest<Retained<NSEvent>>>,
    context_scheduled: Cell<bool>,
    context_tracking: Cell<bool>,
    cursor: Cell<Option<CursorShape>>,
    input: RefCell<TextInputState>,
    ax_elements: RefCell<std::collections::HashMap<accessibility::Id, Retained<a11y::AxElement>>>,
    ax_cache: accessibility::TreeCache,
    ax_queried: Cell<Option<Instant>>,
    ax_window_offset: Cell<Option<(accessibility::Id, usize)>>,
    ax_announcements: RefCell<accessibility::Announcements>,
    ax_focus: Cell<Option<accessibility::Id>>,
    ax_layout: Cell<u64>,
}
impl Host {
    fn update_pointer_cursor(&self, point: Point) {
        let Some(shape) = self
            .app
            .try_borrow()
            .ok()
            .and_then(|slot| slot.as_ref().map(|app| app.cursor_at(point)))
        else {
            return;
        };
        if self.cursor.replace(Some(shape)) != Some(shape) {
            // The directional divider cursor APIs require macOS 15.
            #[allow(deprecated)]
            let cursor = match shape {
                CursorShape::Arrow => NSCursor::arrowCursor(),
                CursorShape::IBeam => NSCursor::IBeamCursor(),
                CursorShape::ResizeHorizontal => NSCursor::resizeLeftRightCursor(),
                CursorShape::ResizeVertical => NSCursor::resizeUpDownCursor(),
            };
            cursor.set();
        }
    }

    fn handle(self: &Rc<Self>) -> WindowHandle {
        WindowHandle(Rc::clone(self))
    }
    fn set_drawable_limit(&self, limit: usize) {
        if let Some(layer) = self.layer.get() {
            layer.setMaximumDrawableCount(limit);
        }
    }
    fn arm_drawable_timer(&self, pool: DrawablePool, now: Instant) {
        let (Some(due), Some(view)) = (pool.deadline(), self.view.get()) else {
            return;
        };
        let current = self.drawable_timer.take();
        if current.is_some() {
            // A burst moves the deadline in Rust; reuse the pending native
            // timer, which will recheck the deadline and rearm if necessary.
            self.drawable_timer.set(current);
            return;
        }
        // SAFETY: settleDrawablePool: takes the timer on the view's main thread.
        let timer = unsafe {
            NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                due.saturating_duration_since(now).as_secs_f64().max(0.0001),
                view,
                sel!(settleDrawablePool:),
                None,
                false,
            )
        };
        // SAFETY: timer and common-mode constant belong to the main run loop.
        unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
        self.drawable_timer.set(Some(timer));
    }
    fn settle_drawable_pool(&self) {
        self.drawable_timer.take();
        if self.stopped.get() {
            return;
        }
        let now = Instant::now();
        let mut pool = self.drawable_pool.get();
        let old = pool.limit();
        if self.drawing.get() {
            // A nested AppKit run loop must not trim during a draw.
            pool.frame(now);
        } else {
            pool.timer(now);
        }
        self.drawable_pool.set(pool);
        if pool.limit() != old {
            self.set_drawable_limit(pool.limit());
        }
        self.arm_drawable_timer(pool, now);
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
        if matches!(event, Event::Key(_) | Event::Text(_) | Event::Scroll(_)) {
            crate::memory_pressure::note_activity();
        }
        self.pending.borrow_mut().push_back(event);
        self.drain();
    }
    fn drain(self: &Rc<Self>) {
        if self.stopped.get() {
            self.pending.borrow_mut().clear();
            return;
        }
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
                Event::Accessibility(id, request) => app.on_accessibility(&w, id, request),
                Event::AccessibilityPreferences => {
                    app.on_accessibility_preferences(&w, a11y::preferences())
                }
                Event::ScrollerStyle => app.on_scroller_style(&w, w.prefers_overlay_scrollers()),
                Event::Key(e) => {
                    app.on_key(&w, e);
                }
                Event::NamedCommand(name, args) => app.on_named_command(&w, &name, &args),
                Event::Text(e) => app.on_text_input(&w, e),
                Event::Command(e) => app.on_command(&w, e),
                Event::Mouse(e, native) => {
                    let button_press = e.kind == MouseKind::Down;
                    if button_press {
                        self.context.borrow_mut().begin_click(native);
                    }
                    app.on_mouse(&w, e);
                    if button_press {
                        self.context.borrow_mut().end_dispatch();
                    }
                }
                Event::OpenPaths(paths) => app.on_open_paths(&w, &paths),
                Event::FinderNewTextFile(directory) => app.on_finder_new_text_file(&w, &directory),
                Event::Scroll(e) => app.on_scroll(&w, e),
                Event::Resize => app.on_resize(&w, self.size.get(), self.scale.get()),
                Event::Appearance(e) => app.on_appearance(&w, e),
                Event::InlineTextFocusLost => app.on_inline_text_focus_lost(&w),
                Event::CloseRequested => {
                    if app.on_window_close_requested(&w) {
                        w.close();
                    }
                }
            }
            let current = app.text_input_state();
            let previous = self.input.replace(current.clone());
            drop(slot);
            a11y::notify(self, &previous, &current);
            if self.stopped.get() {
                break;
            }
        }
        if self.stopped.get() && !lifecycle::is_template(self.id) {
            self.app.borrow_mut().take();
            *self.input.borrow_mut() = TextInputState::default();
        }
        self.dispatching.set(false);
        self.flush_ime_coordinates();
        lifecycle::schedule_contexts();
        lifecycle::schedule();
    }
    fn offer_key(self: &Rc<Self>, key: KeyEvent) -> bool {
        if self.stopped.get()
            || self.dispatching.get()
            || self.drawing.get()
            || (self.input.borrow().marked.is_some()
                && !(key.modifiers.command() && self.inline_text_input()))
        {
            return false;
        }
        crate::memory_pressure::note_activity();
        self.dispatching.set(true);
        let previous = self.input.borrow().clone();
        let handled = if let Some(app) = self.app.borrow_mut().as_mut() {
            let handled = app.on_key(&self.handle(), key);
            *self.input.borrow_mut() = app.text_input_state();
            handled
        } else {
            false
        };
        self.dispatching.set(false);
        let current = self.input.borrow().clone();
        a11y::notify(self, &previous, &current);
        self.drain();
        handled
    }
    fn inline_text_input(&self) -> bool {
        self.app
            .try_borrow()
            .is_ok_and(|app| app.as_ref().is_some_and(|app| app.inline_text_input()))
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
        // window front. Wait until the visible view actually gets a display tick.
        if !launch_frame_allowed(
            self.starting.get(),
            self.first_tick.get(),
            reason,
            self.visible.get(),
        ) {
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
        let reason = if self.first_tick.get() && reason == FrameReason::DisplayLink {
            self.first_tick.set(false);
            self.phase("first_visible_display_tick");
            FrameReason::StartupPresent
        } else {
            reason
        };
        self.dirty.set(false);
        let previous = self.input.borrow().clone();
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
        let current = self.input.borrow().clone();
        a11y::notify(self, &previous, &current);
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
    fn schedule_context_menu(&self) {
        if self.stopped.get()
            || self.context_tracking.get()
            || !self.context.borrow().is_pending()
            || self.context_scheduled.replace(true)
        {
            return;
        }
        if let Some(view) = self.view.get() {
            // SAFETY: the delayed selector takes one optional object. AppKit retains
            // the view until delivery; no app borrow crosses this scheduling call.
            unsafe {
                view.performSelector_withObject_afterDelay(
                    sel!(showDeferredContextMenu:),
                    None,
                    0.,
                );
            }
        } else {
            self.context_scheduled.set(false);
        }
    }
    fn show_context_menu(self: &Rc<Self>) {
        self.context_scheduled.set(false);
        let busy = lifecycle::hosts_busy() || self.context_tracking.get();
        if self.stopped.get() {
            self.context.borrow_mut().take_if_idle(false);
            return;
        }
        let request = self.context.borrow_mut().take_if_idle(busy);
        let Some((entries, event)) = request else {
            return;
        };
        self.context_tracking.set(true);
        menus::popup(self, &entries, &event);
        self.context_tracking.set(false);
        lifecycle::schedule_contexts();
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
define_class!(
    // SAFETY: NSWindow remains main-thread-only; key eligibility is independent of
    // the titled/borderless style, so simple full screen retains keyboard focus.
    #[unsafe(super = NSWindow)]
    struct EditorWindow;
    // SAFETY: NSObjectProtocol has no additional requirements.
    unsafe impl NSObjectProtocol for EditorWindow {}
    impl EditorWindow {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool { true }
        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool { true }
    }
);
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
        #[unsafe(method(acceptsFirstMouse:))] fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            self.ivars().host.upgrade().is_some_and(|host| {
                host.app.try_borrow().ok().is_some_and(|app| {
                    app.as_ref().is_some_and(|app| app.accepts_first_mouse())
                })
            })
        }
        #[unsafe(method(isAccessibilityElement))] fn ax_element(&self) -> bool { true }
        #[unsafe(method_id(accessibilityRole))] fn ax_role(&self) -> Retained<NSString> { NSString::from_str("AXGroup") }
        #[unsafe(method_id(accessibilityLabel))] fn ax_label(&self) -> Retained<NSString> { NSString::from_str(&self.ivars().host.upgrade().map_or_else(|| "Velocity workspace".to_owned(), |h| a11y::label(&h))) }
        #[unsafe(method_id(accessibilityChildren))] fn ax_children(&self) -> Option<Retained<NSArray<AnyObject>>> { self.ivars().host.upgrade().map(|h| a11y::children(&h,None)) }
        #[unsafe(method_id(accessibilityChildrenInNavigationOrder))] fn ax_navigation(&self) -> Option<Retained<NSArray<AnyObject>>> { self.ivars().host.upgrade().map(|h| a11y::children(&h,None)) }
        #[unsafe(method_id(accessibilityFocusedUIElement))] fn ax_focused(&self) -> Option<Retained<AnyObject>> { self.ivars().host.upgrade().and_then(|h| a11y::focused(&h)) }
        #[unsafe(method_id(accessibilityHitTest:))] fn ax_hit(&self, point: NSPoint) -> Option<Retained<AnyObject>> { self.ivars().host.upgrade().and_then(|h| a11y::hit(&h,point)) }
        #[unsafe(method(accessibilityDisplayOptionsChanged:))] fn ax_preferences(&self, _notification: &NSNotification) { if let Some(h)=self.ivars().host.upgrade() { h.dispatch(Event::AccessibilityPreferences); h.flush(); } }
        #[unsafe(method(preferredScrollerStyleChanged:))] fn scroller_style(&self, _notification: &NSNotification) { if let Some(h)=self.ivars().host.upgrade() { h.dispatch(Event::ScrollerStyle); h.flush(); } }
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
        #[unsafe(method(settleDrawablePool:))]
        fn settle_drawable_pool(&self, _timer: &NSTimer) {
            if let Some(h) = self.ivars().host.upgrade() { h.settle_drawable_pool(); }
        }
        #[unsafe(method(showDeferredContextMenu:))]
        fn show_deferred_context_menu(&self, _sender: Option<&AnyObject>) {
            if let Some(h) = self.ivars().host.upgrade() { h.show_context_menu(); }
        }
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
                // The key window gets performKeyEquivalent: before the menu bar.
                // If the editor declines a chord (e.g. during composition), don't
                // execute the display shortcut as a fallback menu action.
                // https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/EventOverview/HandlingKeyEvents/HandlingKeyEvents.html
                // AppKit relocates equivalents for non-US layouts and rewrites the item's own
                // equivalent (⌘/ becomes ⌘ß on German), so compare the key against the one
                // Velocity installed. The keymap never receives the relocated chord; the
                // shortcut the menu displays runs the command, as in Sublime Text 4215
                // (black-box, German layout).
                let command = h.menu_commands.borrow().get(item.tag().max(0) as usize).cloned();
                if let Some(command) = &command
                    && !h.stopped.get() && !command.installed_equivalent.is_empty()
                    && NSApplication::sharedApplication(h.mtm).currentEvent().is_some_and(|event| {
                        event.r#type() == NSEventType::KeyDown
                            && event.modifierFlags().contains(NSEventModifierFlags::Command)
                            && !relocated_equivalent(
                                &command.installed_equivalent,
                                &event.charactersIgnoringModifiers().map(|s| s.to_string()).unwrap_or_default(),
                            )
                    })
                {
                    return;
                }
                if let Some(MenuBarAction { name, args, .. }) = command {
                    if h.stopped.get() && lifecycle::is_template(h.id) {
                        if windowless_command(&name) && let Ok(mut slot) = h.app.try_borrow_mut()
                            && let Some(app) = slot.as_mut() {
                            app.on_named_command(&h.handle(), &name, &args);
                        }
                    } else {
                        h.dispatch(Event::NamedCommand(name,args));
                        h.flush();
                    }
                }
            }
        }
        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            self.ivars().host.upgrade().is_some_and(|h| {
                let commands=h.menu_commands.borrow();
                commands.get(item.tag().max(0) as usize).is_some_and(|command| {
                    if h.stopped.get() && lifecycle::is_template(h.id) {
                        windowless_command(&command.name)
                    } else {
                        h.app.try_borrow().ok().is_some_and(|app|app.as_ref().is_some_and(|app|app.command_enabled(&command.name,&command.args)))
                    }
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
        #[unsafe(method(cursorUpdate:))] fn cursor_update(&self, e: &NSEvent) {
            self.update_pointer_cursor(e);
        }
        #[unsafe(method(mouseExited:))] fn mouse_exited(&self, _e: &NSEvent) {
            if let Some(h) = self.ivars().host.upgrade() {
                h.cursor.set(None);
            }
        }
        #[unsafe(method(scrollWheel:))] fn scroll(&self, e: &NSEvent) {
            if let Some(h) = self.ivars().host.upgrade() {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                h.dispatch(Event::Scroll(ScrollEvent { position: Point { x: p.x, y: p.y }, delta_x: e.scrollingDeltaX(), delta_y: e.scrollingDeltaY(), precise: e.hasPreciseScrollingDeltas(), phase: e.phase().0 as u64, momentum_phase: e.momentumPhase().0 as u64, modifiers: Modifiers(e.modifierFlags().0 as u64), timestamp: e.timestamp() }));
                // Coalesced: the app requests a frame and the display link draws it at vsync.
            }
        }
        #[unsafe(method(undo:))] fn undo(&self, _sender: Option<&AnyObject>) { self.command(Command::Undo); }
        #[unsafe(method(redo:))] fn redo(&self, _sender: Option<&AnyObject>) { self.command(Command::Redo); }
        #[unsafe(method(cut:))] fn cut(&self, _sender: Option<&AnyObject>) { self.command(Command::Cut); }
        #[unsafe(method(copy:))] fn copy(&self, _sender: Option<&AnyObject>) { self.command(Command::Copy); }
        #[unsafe(method(paste:))] fn paste(&self, _sender: Option<&AnyObject>) { self.command(Command::Paste); }
        #[unsafe(method(selectAll:))] fn select_all(&self, _sender: Option<&AnyObject>) { self.command(Command::SelectAll); }
    }
    // SAFETY: selectors match NSMenuDelegate; menu updates snapshot app flags
    // before mutating AppKit objects and never initiate nested menu tracking.
    unsafe impl NSMenuDelegate for EditorView {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            if let Some(host) = self.ivars().host.upgrade() { menus::update_menu_bar(&host, menu); }
        }
    }
    // SAFETY: the view is main-thread-only and the sender is AppKit's live drag
    // session. Only file URLs are accepted; the source item is never moved.
    unsafe impl NSDraggingDestination for EditorView {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            self.file_drag_operation(sender)
        }
        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            self.file_drag_operation(sender)
        }
        #[unsafe(method(prepareForDragOperation:))]
        fn prepare_drag(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            !self.file_drag_operation(sender).is_empty()
        }
        #[unsafe(method(performDragOperation:))]
        fn perform_drag(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            self.perform_file_drop(sender)
        }
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
    fn perform_file_drop(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
        if self.file_drag_operation(sender).is_empty() {
            return false;
        }
        let paths = pasteboard_paths(&sender.draggingPasteboard());
        if paths.is_empty() {
            return false;
        }
        if let Some(host) = self.ivars().host.upgrade()
            && !host.stopped.get()
        {
            host.dispatch(Event::OpenPaths(paths));
            host.handle().focus();
            host.flush();
            true
        } else {
            false
        }
    }
    fn file_drag_operation(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
        if !sender
            .draggingSourceOperationMask()
            .contains(NSDragOperation::Copy)
            || self
                .ivars()
                .host
                .upgrade()
                .is_none_or(|host| host.stopped.get())
            || pasteboard_paths(&sender.draggingPasteboard()).is_empty()
        {
            NSDragOperation::None
        } else {
            NSDragOperation::Copy
        }
    }
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
    fn finish_marked_text(&self, host: &Rc<Host>, commit: bool) {
        // Clear our preview before AppKit can call unmarkText. Inline fields
        // accept the shown text on blur; document and panel preedit still cancels.
        if host.input.borrow().marked.is_some() {
            host.dispatch(Event::Text(if commit {
                TextInput::Unmark
            } else {
                TextInput::Discard
            }));
        }
        if let Some(context) = self.inputContext() {
            context.discardMarkedText();
        }
    }
    fn update_pointer_cursor(&self, e: &NSEvent) {
        let Some(h) = self.ivars().host.upgrade() else {
            return;
        };
        // AppKit may have restored a cursor outside this tracking area.
        h.cursor.set(None);
        let p = self.convertPoint_fromView(e.locationInWindow(), None);
        h.update_pointer_cursor(Point { x: p.x, y: p.y });
    }
    fn mouse(&self, e: &NSEvent, kind: MouseKind) {
        if let Some(h) = self.ivars().host.upgrade() {
            if kind == MouseKind::Down {
                if let Some(w) = self.window() {
                    w.makeFirstResponder(Some(self));
                }
                self.finish_marked_text(&h, h.inline_text_input());
            }
            let p = self.convertPoint_fromView(e.locationInWindow(), None);
            let event = MouseEvent {
                button: e.buttonNumber().max(0) as usize,
                kind,
                position: Point { x: p.x, y: p.y },
                click_count: e.clickCount().max(0) as usize,
                modifiers: Modifiers(e.modifierFlags().0 as u64),
                timestamp: e.timestamp(),
            };
            let native = (kind == MouseKind::Down).then(|| e.retain());
            h.dispatch(Event::Mouse(event, native));
            if matches!(kind, MouseKind::Move | MouseKind::Drag)
                || kind == MouseKind::Up && NSPointInRect(p, self.bounds())
            {
                h.update_pointer_cursor(Point { x: p.x, y: p.y });
            }
            // Clicks draw immediately; moves and drags arrive faster than the display refreshes,
            // so their frames are coalesced onto the display link.
            if matches!(kind, MouseKind::Down | MouseKind::Up) {
                h.flush();
            }
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
fn file_url(path: &Path) -> Result<Retained<NSURL>, String> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(path)
    };
    let name = CString::new(absolute.as_os_str().as_bytes())
        .map_err(|_| "file path contains a NUL byte")?;
    let directory = std::fs::symlink_metadata(&absolute).is_ok_and(|metadata| metadata.is_dir());
    // SAFETY: the absolute, NUL-terminated filesystem representation lives
    // through the call, which copies it. Symlinks are not canonicalized.
    Ok(unsafe {
        NSURL::fileURLWithFileSystemRepresentation_isDirectory_relativeToURL(
            NonNull::new(name.as_ptr().cast_mut()).expect("CString pointer"),
            directory,
            None,
        )
    })
}
fn path_from_url(url: &NSURL) -> Option<PathBuf> {
    use std::{
        ffi::{CStr, OsStr},
        os::unix::ffi::OsStrExt,
    };
    if !url.isFileURL() {
        return None;
    }
    // SAFETY: NSURL owns a non-null NUL-terminated filesystem representation
    // valid for this call; copy the bytes before releasing the URL.
    let bytes = unsafe { CStr::from_ptr(url.fileSystemRepresentation().as_ptr()) }.to_bytes();
    Some(PathBuf::from(OsStr::from_bytes(bytes)))
}
fn pasteboard_paths(pasteboard: &NSPasteboard) -> Vec<PathBuf> {
    // SAFETY: the class array requests only NSURL pasteboard readers, and absent
    // options means no untyped option values. Filter out every non-file URL.
    let objects = unsafe {
        pasteboard.readObjectsForClasses_options(&NSArray::from_slice(&[NSURL::class()]), None)
    };
    objects.map_or_else(Vec::new, |objects| {
        objects
            .iter()
            .filter_map(|object| object.downcast_ref::<NSURL>().and_then(path_from_url))
            .collect()
    })
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
        #[unsafe(method(application:openURLs:))]
        fn open_urls(&self, _app: &NSApplication, urls: &NSArray<NSURL>) {
            let mut paths = Vec::new();
            for url in urls.iter() {
                if let Some(path) = path_from_url(&url) {
                    paths.push(path);
                } else {
                    let event = NSAppleEventManager::sharedAppleEventManager().currentAppleEvent();
                    finder::receive(&url, event.as_deref());
                }
            }
            if !paths.is_empty() {
                lifecycle::receive(paths);
            }
        }
        #[unsafe(method(application:openFiles:))]
        fn open_files(&self, app: &NSApplication, filenames: &NSArray<NSString>) {
            lifecycle::receive(filenames.iter().map(|name| PathBuf::from(name.to_string())).collect());
            // Success acknowledges queue ownership; file validation and user-facing
            // errors are handled by the destination app when its window is ready.
            app.replyToOpenOrPrint(NSApplicationDelegateReply::Success);
        }
        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn reopen(&self, _app: &NSApplication, has_visible_windows: bool) -> bool {
            if has_visible_windows { true } else { lifecycle::reopen(); false }
        }
        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn terminate_after_close(&self, _app: &NSApplication) -> bool { false }
        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _app: &NSApplication) -> NSApplicationTerminateReply {
            let hosts = WINDOWS.with(|windows| {
                windows
                    .borrow()
                    .iter()
                    .map(|d| d.ivars().host.clone())
                    .collect::<Vec<_>>()
            });
            for host in &hosts {
                if host.stopped.get() {
                    continue;
                }
                let Ok(mut slot) = host.app.try_borrow_mut() else {
                    return termination_reply(false);
                };
                if let Some(app) = slot.as_mut()
                    && !app.on_quit_requested(&host.handle())
                {
                    return termination_reply(false);
                }
            }
            // AppKit completes Quit and system logout only if the delegate
            // approves. Do not stop the run loop and then tell loginwindow no.
            // https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/AppArchitecture/Tasks/GracefulAppTermination.html
            termination_reply(true)
        }
        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _notification: &NSNotification) {
            let app = NSApplication::sharedApplication(self.ivars().host.mtm);
            if let Err(error) = teardown(&app) {
                eprintln!("{error}");
            }
        }
    }
    impl Delegate {
        #[unsafe(method(velocityOpenURL:withReplyEvent:))]
        fn open_url_event(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            finder::open_event(event);
        }
        #[unsafe(method(velocitySessionEnding:))]
        fn session_ending(&self, _notification: &NSNotification) {
            let hosts = WINDOWS.with(|windows| {
                windows.borrow().iter().map(|d| d.ivars().host.clone()).collect::<Vec<_>>()
            });
            for host in hosts {
                if !host.stopped.get()
                    && let Ok(mut slot) = host.app.try_borrow_mut()
                    && let Some(app) = slot.as_mut()
                {
                    app.on_session_ending(&host.handle());
                }
            }
        }
        #[unsafe(method(drainPlatformRequests:))]
        fn drain_platform_requests(&self, _sender: Option<&AnyObject>) { lifecycle::drain(); }
        #[unsafe(method(pollClosedWindow:))]
        fn poll_closed_window(&self, _timer: &NSTimer) { lifecycle::idle(); }
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
        #[unsafe(method(windowDidMove:))]
        fn moved(&self, _n: &NSNotification) {
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
        // Focus is applied on the next frame: the application menu is reinstalled for the
        // key window (menu items target the installing window), selections switch between
        // active and inactive colors, and the session records the active window. Without a
        // frame, switching windows through the Window menu, ⌘` or Mission Control left menu
        // commands such as Close Window acting on the previous window.
        #[unsafe(method(windowDidBecomeKey:))]
        fn became_key(&self, _n: &NSNotification) {
            let h = &self.ivars().host;
            h.dirty.set(true);
            h.pace();
        }
        #[unsafe(method(windowDidResignKey:))]
        fn resigned(&self, _n: &NSNotification) {
            let h = &self.ivars().host;
            let inline = h.inline_text_input();
            if let Some(v) = h.view.get() {
                v.finish_marked_text(h, inline);
            }
            if inline {
                h.dispatch(Event::InlineTextFocusLost);
            }
            h.dirty.set(true);
            h.pace();
        }
    }
);
impl Delegate {
    fn close_requested(&self) {
        let h = &self.ivars().host;
        h.dispatch(Event::CloseRequested);
    }
    fn launch(&self) -> Result<(), String> {
        let h = &self.ivars().host;
        let mtm = h.mtm;
        h.phase("application_did_finish_launching");
        let frame = NSRect::new(
            NSPoint::new(160., 160.),
            NSSize::new(
                self.ivars().options.size.width,
                self.ivars().options.size.height,
            ),
        );
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable;
        let allocated = EditorWindow::alloc(mtm).set_ivars(());
        // SAFETY: NSWindow's designated initializer, with main-thread allocation and
        // exact argument/return types. The subclass adds no state or initializer work.
        let window: Retained<EditorWindow> = unsafe {
            msg_send![super(allocated), initWithContentRect: frame, styleMask: style, backing: NSBackingStoreType::Buffered, defer: false]
        };
        let window = window.into_super();
        let utility = self.ivars().options.kind == crate::WindowKind::Utility;
        let minimum = self
            .ivars()
            .options
            .min_size
            .map_or(NSSize::new(320., 200.), |size| {
                NSSize::new(size.width.max(320.), size.height.max(200.))
            });
        // SAFETY: the host retains the window until explicit teardown after run returns.
        unsafe {
            window.setReleasedWhenClosed(false);
            window.setContentMinSize(minimum);
        }
        window.setTitle(&NSString::from_str(&self.ivars().options.title));
        let key = NSApplication::sharedApplication(mtm).keyWindow();
        if utility {
            // A utility window (Settings) opens over the window it was invoked from, near its
            // top, and stays inside that screen's visible frame.
            match key.as_ref().and_then(|previous| {
                let screen = previous.screen()?.visibleFrame();
                Some((previous.frame(), screen))
            }) {
                Some((anchor, screen)) => {
                    let size = window.frame().size;
                    let x = (anchor.origin.x + (anchor.size.width - size.width) / 2.)
                        .min(screen.origin.x + screen.size.width - size.width)
                        .max(screen.origin.x);
                    let top = (anchor.origin.y + anchor.size.height - 64.)
                        .min(screen.origin.y + screen.size.height)
                        .max(screen.origin.y + size.height);
                    window.setFrameTopLeftPoint(NSPoint::new(x, top));
                }
                None => window.center(),
            }
            window.setExcludedFromWindowsMenu(true);
        } else if let Some(point) = self.ivars().options.position {
            let size = window.frame().size;
            let screen = NSScreen::screens(mtm)
                .into_iter()
                .find(|s| NSPointInRect(NSPoint::new(point.x, point.y), s.frame()))
                .or_else(|| NSScreen::mainScreen(mtm));
            let (x, top) = screen.map_or((point.x, point.y), |screen| {
                let s = screen.visibleFrame();
                (
                    point
                        .x
                        .min(s.origin.x + s.size.width - size.width)
                        .max(s.origin.x),
                    point
                        .y
                        .max(s.origin.y + size.height)
                        .min(s.origin.y + s.size.height),
                )
            });
            window.setFrameTopLeftPoint(NSPoint::new(x, top));
        } else if let Some(previous) = key {
            // ST4 build 4152: cascade from the front window's top-left corner.
            // https://www.sublimetext.com/download
            let frame = previous.frame();
            let next = window.cascadeTopLeftFromPoint(NSPoint::new(
                frame.origin.x,
                frame.origin.y + frame.size.height,
            ));
            window.cascadeTopLeftFromPoint(next);
        }
        // ST4 4126/4131 added fixes for native tabs (https://www.sublimetext.com/download).
        // Keep them off until native tab groups participate in Velocity's window/session model.
        window.setTabbingMode(NSWindowTabbingMode::Disallowed);
        window.setCollectionBehavior(if utility {
            NSWindowCollectionBehavior::FullScreenAuxiliary
        } else {
            NSWindowCollectionBehavior::FullScreenPrimary
        });
        window.setDelegate(Some(ProtocolObject::from_ref(self)));
        let view = EditorView::alloc(mtm).set_ivars(ViewIvars {
            host: Rc::downgrade(h),
        });
        // SAFETY: NSView initWithFrame: has this exact signature; ivars are installed first.
        let view: Retained<EditorView> = unsafe {
            msg_send![super(view), initWithFrame: NSRect::new(NSPoint::new(0., 0.), window.contentLayoutRect().size)]
        };
        // SAFETY: The view owns the area, which tracks its live visible rect after resize.
        let tracking = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                view.bounds(),
                NSTrackingAreaOptions::InVisibleRect
                    | NSTrackingAreaOptions::ActiveInActiveApp
                    | NSTrackingAreaOptions::CursorUpdate
                    | NSTrackingAreaOptions::MouseEnteredAndExited,
                Some(&view),
                None,
            )
        };
        view.addTrackingArea(&tracking);
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
        // SAFETY: file-URL pasteboard types are immutable AppKit constants. The
        // one content view covers editor, tab bar, and side bar hit regions.
        view.registerForDraggedTypes(&NSArray::from_slice(&[unsafe { NSPasteboardTypeFileURL }]));
        // SAFETY: view is retained by the host; the notification uses a one-argument selector.
        unsafe {
            NSWorkspace::sharedWorkspace()
                .notificationCenter()
                .addObserver_selector_name_object(
                    &view,
                    sel!(accessibilityDisplayOptionsChanged:),
                    Some(NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification),
                    None,
                );
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                &view,
                sel!(preferredScrollerStyleChanged:),
                Some(NSPreferredScrollerStyleDidChangeNotification),
                None,
            );
        }
        h.dispatch(Event::AccessibilityPreferences);
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
        if NSApplication::sharedApplication(mtm).mainMenu().is_none() {
            build_menu(mtm);
        }
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
        let started = {
            let mut slot = h.app.borrow_mut();
            if let Some(app) = slot.as_mut() {
                let result = app.on_start(&h.handle());
                if result.is_ok() {
                    *h.input.borrow_mut() = app.text_input_state();
                }
                result
            } else {
                Err("missing app handler".into())
            }
        };
        h.dispatching.set(false);
        started?;
        h.drain();
        h.ready.set(true);
        let activate = h
            .app
            .borrow()
            .as_ref()
            .is_none_or(|app| app.activate_on_open());
        let prepared = startup_frame(activate, |step| {
            match step {
                StartupStep::Prepare => {
                    h.render(FrameReason::StartupPrepare, true);
                    if h.stopped.get() {
                        return false;
                    }
                    h.phase("first_frame_prepared_before_order_front");
                }
                StartupStep::OrderFront => window.makeKeyAndOrderFront(None),
                StartupStep::OrderBack => window.orderBack(None),
                StartupStep::Activate => {
                    #[allow(deprecated)]
                    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
                    h.phase("activated_before_first_present");
                }
            }
            if matches!(step, StartupStep::OrderFront | StartupStep::OrderBack) {
                h.phase("ordered_front");
            }
            true
        });
        if !prepared {
            return Ok(());
        }
        CATransaction::flush();
        h.phase("launch_callback_finished");
        h.visible.set(
            window
                .occlusionState()
                .contains(NSWindowOcclusionState::Visible),
        );
        h.first_tick.set(true);
        h.starting.set(false);
        h.handle().request_frame();
        lifecycle::launched();
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartupStep {
    Prepare,
    OrderFront,
    OrderBack,
    Activate,
}

fn startup_frame(activate: bool, mut step: impl FnMut(StartupStep) -> bool) -> bool {
    use StartupStep::*;
    // Pre-render without a drawable; the launch callback only orders the window.
    step(Prepare)
        && if activate {
            step(OrderFront) && step(Activate)
        } else {
            step(OrderBack)
        }
}

fn launch_frame_allowed(
    starting: bool,
    first_tick: bool,
    reason: FrameReason,
    visible: bool,
) -> bool {
    if starting {
        reason == FrameReason::StartupPrepare
    } else {
        !first_tick || (reason == FrameReason::DisplayLink && visible)
    }
}

#[cfg(test)]
#[test]
fn startup_prepares_offscreen_and_only_presents_on_visible_tick() {
    use StartupStep::*;
    for (activate, expected) in [
        (true, &[Prepare, OrderFront, Activate][..]),
        (false, &[Prepare, OrderBack][..]),
    ] {
        let mut observed = Vec::new();
        assert!(startup_frame(activate, |step| {
            observed.push(step);
            true
        }));
        assert_eq!(observed, expected);
    }
    let mut observed = Vec::new();
    assert!(!startup_frame(true, |step| {
        observed.push(step);
        false
    }));
    assert_eq!(observed, [Prepare]);
    assert!(launch_frame_allowed(
        true,
        false,
        FrameReason::StartupPrepare,
        false
    ));
    assert!(!launch_frame_allowed(
        true,
        false,
        FrameReason::DisplayLink,
        true
    ));
    for reason in [FrameReason::Timer, FrameReason::Input, FrameReason::Resize] {
        assert!(!launch_frame_allowed(false, true, reason, true));
    }
    assert!(!launch_frame_allowed(
        false,
        true,
        FrameReason::DisplayLink,
        false
    ));
    assert!(launch_frame_allowed(
        false,
        true,
        FrameReason::DisplayLink,
        true
    ));
    assert!(launch_frame_allowed(false, false, FrameReason::Input, true));
}

fn screen_rect(frame: NSRect) -> Rect {
    Rect {
        x: frame.origin.x,
        y: frame.origin.y,
        width: frame.size.width,
        height: frame.size.height,
    }
}
/// NSColor.controlAccentColor as component values; it follows System Settings › Appearance.
pub(crate) fn accent_color() -> Option<[f32; 4]> {
    let color = NSColor::controlAccentColor().colorUsingType(NSColorType::ComponentBased)?;
    Some([
        color.redComponent() as f32,
        color.greenComponent() as f32,
        color.blueComponent() as f32,
        color.alphaComponent() as f32,
    ])
}
fn windowless_command(name: &str) -> bool {
    matches!(
        name,
        "new_window"
            | "show_settings"
            | "edit_settings"
            | "open_url"
            | "open_crash_reports"
            | "prompt_open_file"
            | "prompt_open_folder_in_new_window"
            | "open_recent_file"
            | "open_recent_folder"
            | "open_project"
            | "clear_recent_items"
    )
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
#[derive(Clone)]
struct MenuBarAction {
    id: String,
    name: String,
    args: String,
    installed_equivalent: String,
    shortcut_hint: Option<String>,
}

fn append_menu(host: &Rc<Host>, menu: &NSMenu, entries: &[MenuEntry]) {
    if let Some(view) = host.view.get() {
        menu.setDelegate(Some(ProtocolObject::from_ref(&**view)));
    }
    for entry in entries {
        if entry.caption == "-" {
            menu.addItem(&NSMenuItem::separatorItem(host.mtm));
            continue;
        }
        let services = entry.command.as_deref() == Some("native_services");
        // https://developer.apple.com/design/human-interface-guidelines/the-menu-bar
        let native = match entry.command.as_deref() {
            Some("native_minimize") => Some(sel!(performMiniaturize:)),
            Some("native_zoom") => Some(sel!(performZoom:)),
            Some("native_bring_all_to_front") => Some(sel!(arrangeInFront:)),
            Some("native_hide") => Some(sel!(hide:)),
            Some("native_hide_others") => Some(sel!(hideOtherApplications:)),
            Some("native_show_all") => Some(sel!(unhideAllApplications:)),
            Some("native_quit") => Some(sel!(terminate:)),
            _ => None,
        };
        // SAFETY: These selectors all take one optional sender.
        let item = unsafe {
            menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str(&menus::item_title(
                    &entry.caption,
                    entry.shortcut_hint.as_deref(),
                )),
                native.or_else(|| {
                    (!services && entry.command.is_some()).then(|| sel!(velocityCommand:))
                }),
                &NSString::from_str(""),
            )
        };
        if let Some((key, modifiers)) = entry.shortcut.as_deref().and_then(menu_key_equivalent) {
            item.setKeyEquivalent(&NSString::from_str(&key.to_string()));
            item.setKeyEquivalentModifierMask(modifiers);
        }
        if native.is_some() {
            continue;
        }
        if services {
            // https://developer.apple.com/documentation/appkit/nsapplication/servicesmenu
            let child = NSMenu::initWithTitle(NSMenu::alloc(host.mtm), &item.title());
            item.setSubmenu(Some(&child));
            NSApplication::sharedApplication(host.mtm).setServicesMenu(Some(&child));
            continue;
        }
        if let Some(command) = &entry.command {
            let mut commands = host.menu_commands.borrow_mut();
            item.setTag(commands.len() as isize);
            let installed = entry
                .shortcut
                .as_deref()
                .and_then(menu_key_equivalent)
                .map(|(key, _)| key.to_string())
                .unwrap_or_default();
            commands.push(MenuBarAction {
                id: entry.id.clone(),
                name: command.clone(),
                args: entry.args.clone(),
                installed_equivalent: installed,
                shortcut_hint: entry.shortcut_hint.clone(),
            });
            if let Some(view) = host.view.get() {
                // SAFETY: host retains the view until every menu is torn down.
                unsafe {
                    item.setTarget(Some(view));
                }
            }
        }
        if !entry.children.is_empty() {
            let child = NSMenu::initWithTitle(NSMenu::alloc(host.mtm), &item.title());
            append_menu(host, &child, &entry.children);
            item.setSubmenu(Some(&child));
        }
    }
}

/// Whether AppKit matched a menu item through its layout-localized equivalent: the key
/// pressed produced a different character than the one the item was given.
fn relocated_equivalent(equivalent: &str, pressed: &str) -> bool {
    !pressed.is_empty() && equivalent.to_lowercase() != pressed.to_lowercase()
}

#[cfg(test)]
#[test]
fn relocated_menu_equivalents_are_told_apart_from_declined_chords() {
    // German: AppKit shows ⌘/ as ⌘ß; the keymap never sees "ß".
    assert!(relocated_equivalent("/", "ß"));
    // Same character (any case): the editor saw and declined this chord.
    assert!(!relocated_equivalent("/", "/"));
    assert!(!relocated_equivalent("z", "Z"));
    assert!(!relocated_equivalent("s", ""));
}

fn menu_key_equivalent(chord: &str) -> Option<(char, NSEventModifierFlags)> {
    use NSEventModifierFlags as Flags;
    let (modifiers, name) = chord.rsplit_once('+')?;
    let mut mask = Flags::empty();
    for modifier in modifiers.split('+') {
        mask |= match modifier {
            "super" => Flags::Command,
            "alt" => Flags::Option,
            "ctrl" => Flags::Control,
            "shift" => Flags::Shift,
            _ => return None,
        };
    }
    if !mask.contains(Flags::Command) {
        return None;
    }
    // AppKit wants the unmodified character, including US punctuation, with
    // Shift/Option supplied independently, not the current keyboard's text.
    // https://developer.apple.com/documentation/appkit/nsmenuitem/keyequivalent
    let key = match name {
        "space" => ' ',
        "tab" => '\t',
        "enter" => '\r',
        "escape" => '\u{1b}',
        "backspace" => '\u{08}',
        "delete" => '\u{f728}',
        "left" => '\u{f702}',
        "right" => '\u{f703}',
        "up" => '\u{f700}',
        "down" => '\u{f701}',
        _ => match name.as_bytes() {
            [byte] if byte.is_ascii_graphic() => *byte as char,
            _ => char::from_u32(
                0xf703
                    + name
                        .strip_prefix('f')?
                        .parse::<u32>()
                        .ok()
                        .filter(|n| (1..=35).contains(n))?,
            )?,
        },
    };
    Some((key, mask))
}

#[cfg(test)]
#[test]
fn menu_equivalents_use_unmodified_glyphs_and_require_command() {
    use NSEventModifierFlags as Flags;
    for key in [",", ".", ";", "/", "[", "]", "-", "=", "1", "s"] {
        assert_eq!(
            menu_key_equivalent(&format!("super+{key}")),
            Some((key.chars().next().unwrap(), Flags::Command))
        );
    }
    assert_eq!(
        menu_key_equivalent("alt+shift+super+["),
        Some(('[', Flags::Command | Flags::Option | Flags::Shift))
    );
    assert_eq!(menu_key_equivalent("ctrl+g"), None);
    assert_eq!(menu_key_equivalent("f5"), None);
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
    let host = create_host(mtm, &options, app)?;
    let delegate = create_delegate(mtm, host.clone(), options);
    WINDOWS.with(|windows| windows.borrow_mut().push(delegate.clone()));
    let nsapp = NSApplication::sharedApplication(mtm);
    host.phase("nsapplication_ready");
    nsapp.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    NSWindow::setAllowsAutomaticWindowTabbing(false, mtm);
    nsapp.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    let apple_events = NSAppleEventManager::sharedAppleEventManager();
    // AppKit may defer openURLs: on cold launch. Capture sender provenance from
    // the raw kAEGetURL before it goes away, without intercepting kAEOpenDocuments.
    // SAFETY: the handler is retained until run returns and implements the two-descriptor selector.
    unsafe {
        let _: () = msg_send![&*apple_events, setEventHandler: &*delegate,
            andSelector: sel!(velocityOpenURL:withReplyEvent:),
            forEventClass: finder::INTERNET_EVENT, andEventID: finder::INTERNET_EVENT];
    }
    // Apple's workspace notification is sent on this center, not the default.
    // https://developer.apple.com/documentation/appkit/nsworkspace/willpoweroffnotification
    // SAFETY: the delegate is retained while the app runs and implements the selector.
    unsafe {
        NSWorkspace::sharedWorkspace()
            .notificationCenter()
            .addObserver_selector_name_object(
                &delegate,
                sel!(velocitySessionEnding:),
                Some(NSWorkspaceWillPowerOffNotification),
                None,
            );
    }
    nsapp.run();
    // SAFETY: both arguments are UInt32 event codes, matching the registration.
    unsafe {
        let _: () = msg_send![&*apple_events, removeEventHandlerForEventClass: finder::INTERNET_EVENT,
            andEventID: finder::INTERNET_EVENT];
    }
    teardown(&nsapp)
}

fn termination_reply(approved: bool) -> NSApplicationTerminateReply {
    if approved {
        NSApplicationTerminateReply::TerminateNow
    } else {
        NSApplicationTerminateReply::TerminateCancel
    }
}

fn teardown(nsapp: &NSApplication) -> Result<(), String> {
    let windows = WINDOWS.with(|windows| std::mem::take(&mut *windows.borrow_mut()));
    let mut error = None;
    for delegate in windows {
        let host = &delegate.ivars().host;
        if let Some(link) = host.link.get() {
            link.invalidate();
        }
        if let Some(timer) = host.timer.take() {
            timer.invalidate();
        }
        host.app.borrow_mut().take();
        if let Some(window) = host.window.get() {
            window.setDelegate(None);
            window.orderOut(None);
            window.setContentView(None);
        }
        if let Some(e) = host.error.borrow_mut().take() {
            error.get_or_insert(e);
        }
    }
    nsapp.setDelegate(None);
    nsapp.setMainMenu(None);
    lifecycle::reset();
    INSTALLED_MENU.with(|installed| *installed.borrow_mut() = MenuInstallation::default());
    error.map_or(Ok(()), Err)
}

#[cfg(test)]
#[test]
fn native_termination_approves_when_checkpoint_succeeds() {
    assert_eq!(
        termination_reply(true),
        NSApplicationTerminateReply::TerminateNow
    );
    assert_eq!(
        termination_reply(false),
        NSApplicationTerminateReply::TerminateCancel
    );
}

fn launch_window(
    mtm: MainThreadMarker,
    options: WindowOptions,
    app: Box<dyn AppHandler>,
) -> Result<WindowHandle, String> {
    let host = create_host(mtm, &options, app)?;
    let delegate = create_delegate(mtm, Rc::clone(&host), options);
    WINDOWS.with(|windows| windows.borrow_mut().push(delegate.clone()));
    if let Err(error) = delegate.launch() {
        // Failed construction must not leave an unready live destination, or
        // replace the approved closed-window template needed for another retry.
        host.stopped.set(true);
        if let Some(link) = host.link.get() {
            link.invalidate();
        }
        if let Some(timer) = host.timer.take() {
            timer.invalidate();
        }
        host.app.borrow_mut().take();
        if let Some(window) = host.window.get() {
            window.setDelegate(None);
            window.orderOut(None);
            window.setContentView(None);
        }
        WINDOWS.with(|windows| {
            windows
                .borrow_mut()
                .retain(|delegate| delegate.ivars().host.id != host.id)
        });
        return Err(error);
    }
    Ok(host.handle())
}

fn create_host(
    mtm: MainThreadMarker,
    options: &WindowOptions,
    app: Box<dyn AppHandler>,
) -> Result<Rc<Host>, String> {
    if !options.size.width.is_finite()
        || !options.size.height.is_finite()
        || options.size.width <= 0.
        || options.size.height <= 0.
    {
        return Err("invalid window size".into());
    }
    let host = Rc::new(Host {
        id: NEXT_HOST_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        mtm,
        app: RefCell::new(Some(app)),
        pending: RefCell::new(VecDeque::new()),
        dispatching: Cell::new(false),
        drawing: Cell::new(false),
        input_depth: Cell::new(0),
        ime_geometry_dirty: Cell::new(false),
        ready: Cell::new(false),
        starting: Cell::new(true),
        first_tick: Cell::new(false),
        stopped: Cell::new(false),
        visible: Cell::new(false),
        dirty: Cell::new(true),
        animating: Cell::new(false),
        size: Cell::new(options.size),
        scale: Cell::new(1.),
        simple_fullscreen: RefCell::new(None),
        window: OnceCell::new(),
        view: OnceCell::new(),
        layer: OnceCell::new(),
        link: OnceCell::new(),
        timer: Cell::new(None),
        drawable_pool: Cell::new(DrawablePool::default()),
        drawable_timer: Cell::new(None),
        presentations: Arc::new(Mutex::new(VecDeque::new())),
        error: RefCell::new(None),
        menu_commands: RefCell::new(Vec::new()),
        title: RefCell::new(WindowTitle::new(options.title.clone())),
        context: RefCell::new(ContextRequest::default()),
        context_scheduled: Cell::new(false),
        context_tracking: Cell::new(false),
        cursor: Cell::new(None),
        input: RefCell::new(TextInputState::default()),
        ax_elements: RefCell::new(Default::default()),
        ax_cache: Default::default(),
        ax_queried: Cell::new(None),
        ax_window_offset: Cell::new(None),
        ax_announcements: Default::default(),
        ax_focus: Cell::new(None),
        ax_layout: Cell::new(0),
    });
    Ok(host)
}
fn create_delegate(
    mtm: MainThreadMarker,
    host: Rc<Host>,
    options: WindowOptions,
) -> Retained<Delegate> {
    let delegate = Delegate::alloc(mtm).set_ivars(DelegateIvars {
        host: Rc::clone(&host),
        options,
    });
    // SAFETY: NSObject initializer, after all ivars were initialized.
    let delegate: Retained<Delegate> = unsafe { msg_send![super(delegate), init] };
    delegate
}
