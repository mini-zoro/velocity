use crate::Size;
use std::{ffi::c_void, time::Instant};
/// Non-macOS placeholder. Native construction requires macOS 14+.
#[derive(Clone, Debug)]
pub struct WindowHandle;
impl WindowHandle {
    /// Retains no factory on unsupported platforms; native windows require macOS.
    pub fn set_window_factory(
        &self,
        _factory: impl FnMut() -> Result<(crate::WindowOptions, Box<dyn crate::AppHandler>), String>
        + 'static,
    ) {
    }
    /// Native windows require macOS.
    pub fn new_window(
        &self,
        _options: crate::WindowOptions,
        _app: impl crate::AppHandler,
    ) -> Result<Self, String> {
        Err("native windows require macOS".into())
    }
    /// No native close on unsupported platforms.
    pub fn request_close(&self) {}
    /// No native window on unsupported platforms.
    pub fn close(&self) {}
    /// No native window on unsupported platforms.
    pub fn focus(&self) {}
    /// Unsupported platforms have no full-screen native window.
    pub fn is_full_screen(&self) -> bool {
        false
    }
    /// No native full screen on unsupported platforms.
    pub fn toggle_full_screen(&self) {}
    /// No-op outside the native platform.
    pub fn toggle_full_screen_with_mode(&self, _: bool) {}

    /// Unsupported platforms do not acquire native drawables.
    pub fn last_acquired_drawable(&self) -> Option<u64> {
        None
    }
    /// Deterministic appearance for headless clients.
    pub fn appearance(&self) -> crate::Appearance {
        crate::Appearance::Dark
    }
    /// Unsupported platforms have no focused native window.
    pub fn is_focused(&self) -> bool {
        false
    }
    /// No native menus on this platform.
    pub fn set_menu(&self, _entries: &[crate::MenuEntry]) {}
    /// No native context event can be captured on unsupported platforms.
    pub fn show_context_menu(&self, _entries: &[crate::MenuEntry]) -> bool {
        false
    }
    /// No native deferred popup exists on unsupported platforms.
    pub fn cancel_context_menu(&self) {}
    /// No native Trash exists on unsupported platforms; never deletes the item.
    pub fn trash_file(&self, _path: &std::path::Path) -> Result<(), String> {
        Err("the system Trash requires macOS".into())
    }
    /// No native file viewer exists on unsupported platforms.
    pub fn reveal_file(&self, _path: &std::path::Path) -> Result<(), String> {
        Err("revealing files requires macOS".into())
    }
    /// No native chooser on this platform.
    pub fn open_files(&self) -> Vec<std::path::PathBuf> {
        Vec::new()
    }
    /// No native chooser on this platform.
    pub fn open_directory(&self) -> Option<std::path::PathBuf> {
        None
    }
    /// No native chooser on this platform.
    pub fn save_file(&self, _suggested: &str) -> Option<std::path::PathBuf> {
        None
    }
    /// No native alert on this platform.
    pub fn alert(&self, _message: &str, _detail: &str, _buttons: &[&str]) -> usize {
        0
    }

    /// No native window exists on this platform.
    pub fn display_status(&self) -> crate::DisplayStatus {
        crate::display_status()
    }
    /// Marks the window dirty.
    pub fn request_frame(&self) {}
    /// Schedules a one-shot wakeup.
    pub fn request_frame_at(&self, _deadline: Instant) {}
    /// Enables continuous display-linked animation.
    pub fn set_animating(&self, _active: bool) {}
    /// Portable no-op counterpart of native title-bar theming.
    pub fn set_title_bar_theme(
        &self,
        _appearance: Option<crate::Appearance>,
        _background: Option<[f32; 4]>,
    ) {
    }
    /// Changes the native window title (a no-op on unsupported platforms).
    pub fn set_title(&self, _title: &str) {}
    /// Logical-to-physical pixel scale.
    pub fn scale_factor(&self) -> f64 {
        1.
    }
    /// Actual native content size.
    pub fn content_size(&self) -> Size {
        Size::default()
    }
    /// Maximum display rate, if known.
    pub fn maximum_frames_per_second(&self) -> f32 {
        60.
    }
    /// Native layer pointer; always null on unsupported targets.
    pub fn metal_layer_ptr(&self) -> *mut c_void {
        std::ptr::null_mut()
    }
    /// Drains observed scanout callbacks.
    pub fn take_presentations(&self) -> Vec<crate::Presentation> {
        Vec::new()
    }
    /// Reads text from the system pasteboard.
    pub fn clipboard_text(&self) -> Option<String> {
        None
    }
    /// Writes text to the system pasteboard.
    pub fn set_clipboard_text(&self, _text: &str) -> bool {
        false
    }
    /// Invalidates native IME candidate coordinates.
    pub fn invalidate_character_coordinates(&self) {}
    /// Stops the native application.
    pub fn quit(&self) {}
}
