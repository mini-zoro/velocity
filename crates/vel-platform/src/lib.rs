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
#[cfg(any(target_os = "macos", windows, test))]
mod memory_guard;
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

/// Premultiplied sRGB RGBA8 pixels for the floating drag image, top row first.
#[derive(Debug)]
pub struct DragImage {
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// Pixels per logical point.
    pub scale: f64,
    /// Tightly packed pixels, including any transparent padding.
    pub rgba: Vec<u8>,
}

/// The user's accent color in straight sRGB: the macOS control accent, or the Windows accent
/// color. `None` when the system has none (other platforms) or it cannot be read.
pub fn system_accent_color() -> Option<[f32; 4]> {
    #[cfg(target_os = "macos")]
    return macos::accent_color();
    #[cfg(windows)]
    return windows::accent_color();
    #[cfg(not(any(target_os = "macos", windows)))]
    None
}

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

    /// Clips a native IME anchor to its field or viewport, without moving a
    /// visible leading edge to make room for an offscreen trailing edge.
    pub fn ime_anchor_in(self, bounds: Self) -> Self {
        let right = bounds.x + bounds.width.max(1.);
        let bottom = bounds.y + bounds.height.max(1.);
        let x = self.x.clamp(bounds.x, right - 1.);
        let y = self.y.clamp(bounds.y, bottom - 1.);
        Self {
            x,
            y,
            width: (self.x + self.width).clamp(x + 1., right) - x,
            height: (self.y + self.height).clamp(y + 1., bottom) - y,
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
/// The cursor for a logical point in the client area, independent of IME hit testing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorShape {
    /// Window chrome, controls, and other non-text regions.
    #[default]
    Arrow,
    /// Selectable text.
    IBeam,
    /// Drag a vertical divider left or right.
    ResizeHorizontal,
    /// Drag a horizontal divider up or down.
    ResizeVertical,
}

/// Fallback time between caret inversions when the platform supplies no rate.
pub const DEFAULT_CARET_BLINK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

#[cfg(any(windows, test))]
fn windows_caret_blink_interval(milliseconds: u32) -> Option<std::time::Duration> {
    match milliseconds {
        u32::MAX => None, // GetCaretBlinkTime: INFINITE means no blinking.
        0 => Some(DEFAULT_CARET_BLINK_INTERVAL), // Query failed.
        time => Some(std::time::Duration::from_millis(u64::from(time))),
    }
}

#[test]
fn windows_caret_blink_rate_maps_time_between_inversions() {
    assert_eq!(windows_caret_blink_interval(u32::MAX), None);
    assert_eq!(
        windows_caret_blink_interval(0),
        Some(DEFAULT_CARET_BLINK_INTERVAL)
    );
    assert_eq!(
        windows_caret_blink_interval(275),
        Some(std::time::Duration::from_millis(275))
    );
}
/// What a native window is for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowKind {
    /// An editor window: it can enter full screen and is listed in the Window menu.
    #[default]
    Document,
    /// A utility window such as Settings: it opens centered on the key window, joins a
    /// full-screen space instead of taking one, and is not listed in the Window menu.
    Utility,
}
/// Configuration for a native editor window.
#[derive(Clone, Debug)]
pub struct WindowOptions {
    /// Initial title (bundle-less execution is supported).
    pub title: String,
    /// Requested logical size. The actual size is reported separately.
    pub size: Size,
    /// Smallest logical content size, applied before the window is first shown.
    pub min_size: Option<Size>,
    /// Editor or utility window.
    pub kind: WindowKind,
    /// Desired top-left at a point from `WindowHandle::screen_point`. Native hosts
    /// adjust it to keep the new window on screen.
    pub position: Option<Point>,
}
impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: "Velocity".into(),
            size: Size {
                width: 1200.,
                height: 800.,
            },
            min_size: None,
            kind: WindowKind::Document,
            position: None,
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

/// Moves the native frame by the same screen-space delta as its content anchor.
/// Both points use the same axes and units as the frame, including on AppKit.
#[cfg(any(target_os = "macos", windows, test))]
fn anchored_window_frame(
    frame: Rect,
    current: Point,
    desired: Point,
    work_area: Option<Rect>,
) -> Rect {
    let frame = Rect {
        x: frame.x + desired.x - current.x,
        y: frame.y + desired.y - current.y,
        ..frame
    };
    work_area
        .and_then(|area| clamp_window_frame(frame, &[area]))
        .unwrap_or(frame)
}

#[test]
fn anchored_window_preserves_tab_position_and_clamps_at_screen_edges() {
    let frame = Rect {
        x: 20.,
        y: 110.,
        width: 400.,
        height: 300.,
    };
    let current = Point { x: 132., y: 380. };
    let desired = Point { x: 390., y: 470. };
    let placed = anchored_window_frame(frame, current, desired, None);
    assert_eq!(
        placed,
        Rect {
            x: 278.,
            y: 200.,
            ..frame
        }
    );
    let work_area = Rect {
        x: 0.,
        y: 23.,
        width: 600.,
        height: 450.,
    };
    assert_eq!(
        anchored_window_frame(frame, current, desired, Some(work_area)),
        Rect {
            x: 200.,
            y: 173.,
            ..frame
        },
    );
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
/// A system request to release rebuildable memory.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum MemoryPressure {
    /// Available system memory is low.
    Warning,
    /// The system urgently needs memory back.
    Critical,
}

/// Approximate sizes, in bytes, of the largest rebuildable caches before trimming.
#[derive(Clone, Copy, Debug, Default)]
pub struct MemoryCacheBytes {
    /// Glyph and image atlas texture storage (without driver overhead).
    pub atlas: u64,
    /// GPU and CPU instance buffers.
    pub instances: u64,
    /// Document shaping and font cache payload.
    pub fonts: u64,
    /// Decoded image pixels.
    pub images: u64,
    /// Cached find results.
    pub find: u64,
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
    /// Whether a click at this view point acts while it activates an inactive window.
    /// Chrome that a single click is expected to operate (a file tab) answers true
    /// regardless of [`Self::accepts_first_mouse`]; text and drag surfaces answer
    /// false so the activating click does not also move a caret.
    fn accepts_first_mouse_at(&self, _point: Point) -> bool {
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
    /// Finish logical text input even when a resigned window will not draw.
    fn on_text_input_focus_lost(&mut self, window: &WindowHandle) {
        self.on_text_input(window, TextInput::Discard);
    }
    /// Do not start a native conversion in a non-editing editor mode.
    fn suppress_native_composition(&self) -> bool {
        false
    }
    /// Reconcile logical input focus before publishing native text geometry.
    fn update_text_input_focus(&mut self, _window: &WindowHandle) {}
    /// Execute a command selected from a resource-defined native menu.
    fn on_named_command(&mut self, _window: &WindowHandle, _name: &str, _args: &str) {}
    /// Open files or add folders delivered by Finder, the Dock, or a window drop.
    /// Requests wait for `on_start` and are delivered to the key window when available.
    /// All currently queued native batches coalesce in arrival order before the
    /// callback, so a one-shot forwarding handler can acknowledge them and quit.
    /// The app handles canonicalization, existing tabs, file kinds, and user-facing errors.
    fn on_open_paths(&mut self, _window: &WindowHandle, _paths: &[std::path::PathBuf]) {}
    /// An authenticated request from the Finder Sync extension in this app bundle.
    /// Deliver to the ready key window, or to the windowless handler so it can
    /// create a window here without handing the request to another app copy.
    fn on_finder_new_text_file(&mut self, _window: &WindowHandle, _directory: &std::path::Path) {}
    /// Reopen from the Dock, or before an external open with no live windows.
    /// Called on the windowless handler from `into_windowless`. Create a fresh
    /// window with `window.new_window(...)`. A registered factory is a fallback.
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
    /// A captured gesture was interrupted by deactivation or capture loss.
    fn on_pointer_cancel(&mut self, _window: &WindowHandle) {}
    /// Cursor for a point in the client area.
    fn cursor_at(&self, _point: Point) -> CursorShape {
        CursorShape::Arrow
    }
    /// Receive precise scroll deltas and both gesture and momentum phases.
    fn on_scroll(&mut self, _window: &WindowHandle, _event: ScrollEvent) {}
    /// Update geometry on resize and backing changes.
    fn on_resize(&mut self, _window: &WindowHandle, _size: Size, _scale: f64) {}
    /// Refresh screen-space geometry when the native window moves without resizing.
    fn on_move(&mut self, _window: &WindowHandle) {}
    /// Draw requested work; errors stop the event loop and are returned by run.
    fn on_frame(&mut self, context: &mut FrameContext<'_>) -> Result<(), String>;
    /// Observe system light/dark changes.
    fn on_appearance(&mut self, _window: &WindowHandle, _appearance: Appearance) {}
    /// Loaded document bytes owned by this window, or `None` to opt out of the footprint check.
    fn memory_guard_document_bytes(&self) -> Option<u64> {
        None
    }
    /// Release rebuildable caches on an idle tick, after system pressure or a soft-ceiling warning.
    /// Return cache sizes before the pass, or `None` if there was nothing to trim.
    fn on_memory_pressure(&mut self, _level: MemoryPressure) -> Option<MemoryCacheBytes> {
        None
    }
    /// Release cheap-to-rebuild caches once per prolonged period in the background.
    fn on_background_memory_idle(&mut self) -> Option<MemoryCacheBytes> {
        None
    }
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

    /// The last macOS window may supply a windowless command and open-request handler.
    /// Return only application state, not the closed window's renderer or workspace.
    #[cfg(target_os = "macos")]
    fn into_windowless(self: Box<Self>) -> Option<Box<dyn AppHandler>> {
        None
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
    /// Current process footprint in bytes (physical footprint on macOS, private commit on Windows).
    pub fn memory_footprint() -> Option<u64> {
        #[cfg(target_os = "macos")]
        return metrics::phys_footprint_bytes().ok();
        #[cfg(windows)]
        return memory_guard::windows::footprint();
        #[cfg(not(any(target_os = "macos", windows)))]
        None
    }

    /// Ask the native allocator to return unused heap pages, without evicting live pages.
    pub fn release_free_heap_pages() {
        #[cfg(target_os = "macos")]
        memory_pressure::release_free_pages();
        #[cfg(windows)]
        memory_guard::windows::compact_heap();
    }

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
