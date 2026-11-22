//! Native macOS and Windows windows and input, with device-free coordinates and IME queries.
//!
//! All editor coordinates exposed to AppKit are UTF-16 code units. View geometry
//! uses logical points, top-left origin; only the native bridge converts to screen
//! coordinates. No AppKit or Metal objects are constructed by unit tests.

pub mod accessibility;
mod display;
#[cfg(any(target_os = "macos", test))]
mod drawable_pool;
pub mod fonts;
pub mod fullscreen;
#[cfg(target_os = "macos")]
pub mod image_io;
mod input;
pub mod mouse;
#[cfg(any(target_os = "macos", windows, test))]
mod requests;
mod services;
pub use display::{DisplayStatus, display_status};
pub use services::{
    BrowserOpener, FindPasteboard, NativeFileOperations, RichClipboard, SystemFindPasteboard,
};
#[cfg(target_os = "macos")]
mod macos;
pub mod memory_pressure;
pub mod metrics;
#[cfg(target_os = "macos")]
pub use macos::WindowHandle;
#[cfg(not(any(target_os = "macos", windows)))]
mod stub;
pub use input::*;
#[cfg(not(any(target_os = "macos", windows)))]
pub use stub::WindowHandle;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::WindowHandle;

use std::time::Instant;

/// A position in logical points, with a top-left origin in the content view.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    /// Horizontal position.
    pub x: f64,
    /// Vertical position.
    pub y: f64,
}
/// Logical content size, independent of Retina scale.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    /// Width in points.
    pub width: f64,
    /// Height in points.
    pub height: f64,
}
/// A rectangle in logical points.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}
impl Rect {
    /// Keeps a candidate anchor inside the visible content, including offscreen carets.
    pub fn clamped(self, size: Size) -> Self {
        let width = self.width.max(1.).min(size.width.max(1.));
        let height = self.height.max(1.).min(size.height.max(1.));
        Self {
            x: self.x.clamp(0., (size.width - width).max(0.)),
            y: self.y.clamp(0., (size.height - height).max(0.)),
            width,
            height,
        }
    }
}
/// Effective system appearance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Appearance {
    /// Light background.
    Light,
    /// Dark background.
    #[default]
    Dark,
}
/// Configuration for a native editor window.
#[derive(Clone, Debug)]
pub struct WindowOptions {
    /// Initial title (bundle-less execution is supported).
    pub title: String,
    /// Requested logical size. The actual size is reported separately.
    pub size: Size,
}
impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: "Velocity".into(),
            size: Size {
                width: 1200.,
                height: 800.,
            },
        }
    }
}
/// Clamps a saved frame onto the visible portion of the screen it overlaps most.
/// If no current screen overlaps, the first (primary) screen is the fallback.
pub fn clamp_window_frame(frame: Rect, screens: &[Rect]) -> Option<Rect> {
    if ![frame.x, frame.y, frame.width, frame.height]
        .into_iter()
        .all(f64::is_finite)
        || frame.width <= 0.
        || frame.height <= 0.
    {
        return None;
    }
    let area = |screen: Rect| {
        let width =
            ((frame.x + frame.width).min(screen.x + screen.width) - frame.x.max(screen.x)).max(0.);
        let height = ((frame.y + frame.height).min(screen.y + screen.height)
            - frame.y.max(screen.y))
        .max(0.);
        width * height
    };
    let mut candidates = screens
        .iter()
        .copied()
        .filter(|s| s.width > 0. && s.height > 0.);
    let screen = candidates.next()?;
    let screen = candidates.fold(
        screen,
        |best, next| if area(next) > area(best) { next } else { best },
    );
    let width = frame.width.min(screen.width);
    let height = frame.height.min(screen.height);
    Some(Rect {
        x: frame
            .x
            .clamp(screen.x, (screen.x + screen.width - width).max(screen.x)),
        y: frame
            .y
            .clamp(screen.y, (screen.y + screen.height - height).max(screen.y)),
        width,
        height,
    })
}

#[cfg(test)]
#[test]
fn saved_frame_is_clamped_to_current_screens() {
    let primary = Rect {
        x: 0.,
        y: 25.,
        width: 1000.,
        height: 800.,
    };
    let second = Rect {
        x: -1300.,
        y: -100.,
        width: 1300.,
        height: 900.,
    };
    let partly_on_second = Rect {
        x: -400.,
        y: -200.,
        width: 500.,
        height: 450.,
    };
    let result = clamp_window_frame(partly_on_second, &[primary, second]).unwrap();
    assert_eq!(
        result,
        Rect {
            x: -500.,
            y: -100.,
            ..partly_on_second
        }
    );
    let offscreen = Rect {
        x: 1500.,
        y: 1500.,
        width: 1200.,
        height: 600.,
    };
    let result = clamp_window_frame(offscreen, &[primary]).unwrap();
    assert_eq!(
        result,
        Rect {
            x: 0.,
            y: 225.,
            width: 1000.,
            height: 600.
        }
    );
    assert!(
        clamp_window_frame(
            Rect {
                x: f64::NAN,
                ..offscreen
            },
            &[primary]
        )
        .is_none()
    );
}
/// Why a synchronous frame was requested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameReason {
    /// Prepare and submit the complete offscreen first frame before orderFront.
    StartupPrepare,
    /// Acquire/copy/present on the first visible display-link tick after ordering.
    StartupPresent,
    /// Native input can be submitted immediately, without waiting for vsync.
    Input,
    /// Dirty animation or scroll driven by the view's CADisplayLink.
    DisplayLink,
    /// A one-shot monotonic deadline, used by automatic measurement modes.
    Timer,
    /// Size, backing scale, appearance or exposure changed.
    Resize,
}
/// Borrowed synchronous frame callback context.
#[derive(Debug)]
pub struct FrameContext<'a> {
    /// Retained window/layer owner.
    pub window: &'a WindowHandle,
    /// Callback receipt timestamp, before any layout or rendering.
    pub received: Instant,
    /// Scheduling source.
    pub reason: FrameReason,
}
/// Presented-handler observation, including skipped drawables with a zero time.
#[derive(Clone, Copy, Debug)]
pub struct Presentation {
    /// Monotonic acquisition sequence (starts at zero).
    pub drawable: u64,
    /// Metal's presentedTime, in the CACurrentMediaTime timebase; zero means skipped.
    pub presented_time: f64,
    /// Positive presentedTime translated to Rust's monotonic clock. For a skipped
    /// drawable this equals callback receipt and must not be used as scanout.
    pub scanout: Instant,
    /// Receipt time, reported separately from scanout.
    pub callback: Instant,
}
/// Application callbacks. The app owns text; the platform owns native objects.
///
/// `on_start`, both startup frames, and orderFront happen within one
/// applicationDidFinishLaunching callback. `request_frame` coalesces work; idle
/// display links are paused. App callbacks never run from Metal handler threads.
pub trait AppHandler: 'static {
    /// Whether opening a window should activate the application.
    fn activate_on_open(&self) -> bool {
        true
    }
    /// Whether the editor view handles a click that activates an inactive window.
    fn accepts_first_mouse(&self) -> bool {
        false
    }
    /// Initialize the renderer while the native window is still hidden.
    fn on_start(&mut self, _window: &WindowHandle) -> Result<(), String> {
        Ok(())
    }
    /// Observe a native lifecycle phase for startup instrumentation.
    fn on_platform_phase(&mut self, _phase: &'static str) {}
    /// Offer a key to the keymap before native interpretation, unless composing.
    /// Returning true consumes it.
    fn on_key(&mut self, _window: &WindowHandle, _event: KeyEvent) -> bool {
        false
    }
    /// A transient field owns Command shortcuts and accepts marked text on blur.
    /// Other AppKit input and editing selectors still go through NSTextInputClient.
    fn inline_text_input(&self) -> bool {
        false
    }
    /// Allow transient input to finish even when a resigned window will not draw.
    fn on_inline_text_focus_lost(&mut self, _window: &WindowHandle) {}
    /// Execute a command selected from a resource-defined native menu.
    fn on_named_command(&mut self, _window: &WindowHandle, _name: &str, _args: &str) {}
    /// Open files or add folders delivered by Finder, the Dock, or a window drop.
    /// Requests wait for `on_start` and are delivered to the key window when available.
    /// All currently queued native batches coalesce in arrival order before the
    /// callback, so a one-shot forwarding handler can acknowledge them and quit.
    /// The app handles canonicalization, existing tabs, file kinds, and user-facing errors.
    fn on_open_paths(&mut self, _window: &WindowHandle, _paths: &[std::path::PathBuf]) {}
    /// An authenticated request from the Finder Sync extension in this app bundle.
    /// Deliver to the ready key window, or to the last closed handler so it can
    /// create a window here without handing the request to another app copy.
    fn on_finder_new_text_file(&mut self, _window: &WindowHandle, _directory: &std::path::Path) {}
    /// Reopen from the Dock, or before an external open with no live windows.
    /// The last closed handler and its handle are retained as a template. Create
    /// a fresh window here (for example with `window.new_window(...)`).
    /// Called directly with the template borrowed once; ordinary events are not
    /// dispatched to its closed view. A registered factory is a fallback.
    fn on_reopen(&mut self, _window: &WindowHandle) {}
    /// Poll background work such as IPC without acquiring a drawable.
    /// Runs on the main thread every 100 ms on the retained template while
    /// all windows are closed. Windows also uses it for minimized windows.
    /// A new window may be created through the supplied handle.
    fn on_idle(&mut self, _window: &WindowHandle) {}
    /// Validate a native menu action in the current view.
    fn command_enabled(&self, _name: &str, _args: &str) -> bool {
        true
    }
    /// Whether a resource-defined menu action is included in the native menu.
    fn command_visible(&self, _name: &str, _args: &str) -> bool {
        true
    }
    /// Whether a resource-defined menu action displays a native checkmark.
    fn command_checked(&self, _name: &str, _args: &str) -> bool {
        false
    }
    /// A window-dependent caption for a menu item with a stable resource role.
    fn menu_caption(&self, _id: &str) -> Option<&str> {
        None
    }
    /// Apply insert/marked/unmark updates, including native replacement ranges.
    fn on_text_input(&mut self, _window: &WindowHandle, _event: TextInput) {}
    /// Execute a standard responder command (also used by the Edit menu).
    fn on_command(&mut self, _window: &WindowHandle, _command: Command) {}
    /// Receive mouse button/drag events.
    fn on_mouse(&mut self, _window: &WindowHandle, _event: MouseEvent) {}
    /// Returns a window token and tab group when a tab may be dropped at this point.
    fn tab_drop_destination(&self, _point: Point) -> Option<(u64, usize)> {
        None
    }
    /// Receive precise scroll deltas and both gesture and momentum phases.
    fn on_scroll(&mut self, _window: &WindowHandle, _event: ScrollEvent) {}
    /// Update geometry on resize and backing changes.
    fn on_resize(&mut self, _window: &WindowHandle, _size: Size, _scale: f64) {}
    /// Draw requested work; errors stop the event loop and are returned by run.
    fn on_frame(&mut self, context: &mut FrameContext<'_>) -> Result<(), String>;
    /// Observe system light/dark changes.
    fn on_appearance(&mut self, _window: &WindowHandle, _appearance: Appearance) {}
    /// Approve a user initiated close/quit. Automatic completion uses `quit`.
    fn on_quit_requested(&mut self, _window: &WindowHandle) -> bool {
        true
    }
    /// Checkpoint on a system logout or power-off notification, before the quit request.
    fn on_session_ending(&mut self, _window: &WindowHandle) {}
    /// Approve closing this window, distinct from quitting the application.
    fn on_window_close_requested(&mut self, window: &WindowHandle) -> bool {
        self.on_quit_requested(window)
    }
    /// Supplies an on-demand accessibility hierarchy; never called by the scene hot path.
    fn accessibility_tree(&self) -> accessibility::Tree {
        accessibility::Tree::default()
    }
    /// Performs an accessibility action on the main event queue.
    fn on_accessibility(
        &mut self,
        _window: &WindowHandle,
        _id: accessibility::Id,
        _request: accessibility::Request,
    ) {
    }
    /// Geometry of a value-relative accessibility text range in logical content points.
    fn accessibility_range_rect(&self, _id: accessibility::Id, range: Utf16Range) -> Rect {
        self.text_input_rect(range).0
    }
    /// Observe live system display accessibility preferences.
    fn on_accessibility_preferences(
        &mut self,
        _window: &WindowHandle,
        _preferences: accessibility::Preferences,
    ) {
    }
    /// Observe changes to the platform's preferred scroller style.
    fn on_scroller_style(&mut self, _window: &WindowHandle, _overlay: bool) {}
    /// Return an O(1) text snapshot and document-relative UTF-16 ranges.
    fn text_input_state(&self) -> TextInputState {
        TextInputState::default()
    }
    /// Geometry of the first visual fragment of a requested UTF-16 range.
    fn text_input_rect(&self, range: Utf16Range) -> (Rect, Utf16Range) {
        let state = self.text_input_state();
        (
            state.caret,
            Utf16Range::new(range.location.min(state.text.len_utf16()), 0),
        )
    }
    /// Hit-test a view point into a document-relative UTF-16 offset.
    fn character_index_for_point(&self, _point: Point) -> Option<usize> {
        None
    }
}
/// Resource-defined native menu entry. Arguments remain opaque JSON to the platform.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuEntry {
    /// Visible caption, or "-" for a separator.
    pub caption: String,
    /// Win32 access key, separate from literal ampersands in the caption.
    pub mnemonic: Option<char>,
    /// Stable resource role, independent of the translated caption.
    pub id: String,
    /// Command registry name, absent on submenu parents.
    pub command: Option<String>,
    /// Serialized keyword arguments.
    pub args: String,
    /// Single Command chord (Sublime key syntax), or no native equivalent.
    pub shortcut: Option<String>,
    /// Display-only key sequence when there is no native equivalent.
    pub shortcut_hint: Option<String>,
    /// Submenu contents.
    pub children: Vec<MenuEntry>,
}
/// Main-thread native application bootstrap.
#[derive(Debug)]
pub struct Platform;
#[cfg(target_os = "macos")]
impl Platform {
    /// The Finder Sync extension embedded in the running application, if installed.
    pub fn finder_new_text_file_extension() -> Option<std::path::PathBuf> {
        macos::finder::extension_bundle()
    }
}
impl Platform {
    /// True when Option is held at native launch, before window creation.
    #[cfg(target_os = "macos")]
    pub fn option_held_at_launch() -> bool {
        macos::option_held_at_launch()
    }
    /// Captures the foreground application before the terminal client submits its request.
    #[cfg(target_os = "macos")]
    pub fn frontmost_pid() -> Option<i32> {
        macos::frontmost_pid()
    }
    /// Returns focus to a running application after a successful CLI wait.
    #[cfg(target_os = "macos")]
    pub fn activate_pid(pid: i32) {
        macos::activate_pid(pid)
    }
    /// Runs until explicit quit/completion or a callback failure. macOS 14+.
    /// Closing the last window retains a template handler for Dock reopen and IPC.
    pub fn run(app: impl AppHandler) -> Result<(), String> {
        Self::run_with_options(WindowOptions::default(), app)
    }
    /// Runs with the requested initial window size and title.
    pub fn run_with_options(options: WindowOptions, app: impl AppHandler) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            macos::run(options, Box::new(app))
        }
        #[cfg(windows)]
        {
            windows::run(options, Box::new(app))
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = (options, app);
            Err("Velocity's native platform requires macOS 14 or later".into())
        }
    }
}

#[cfg(test)]
mod geometry_tests {
    use super::*;
    #[test]
    fn candidate_anchor_clamps_offscreen_carets_in_logical_points() {
        let size = Size {
            width: 1200.,
            height: 800.,
        };
        let anchor = Rect {
            x: -500.,
            y: 1600.,
            width: 1.,
            height: 20.,
        }
        .clamped(size);
        assert_eq!(
            anchor,
            Rect {
                x: 0.,
                y: 780.,
                width: 1.,
                height: 20.
            }
        );
        let fractional = Rect {
            x: 100.25,
            y: 200.5,
            width: 1.,
            height: 20.,
        };
        assert_eq!(fractional.clamped(size), fractional);
        // The native bridge converts these points to screen coordinates; scale
        // never changes the app-side anchor or creates a second Retina multiplier.
        assert_eq!(
            fractional.clamped(Size {
                width: 600.,
                height: 400.
            }),
            fractional
        );
    }
}
