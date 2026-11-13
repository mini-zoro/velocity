use crate::Size;
use std::{ffi::c_void, time::Instant};
/// Non-macOS placeholder. Native construction requires macOS 14+.
#[derive(Clone, Debug)]
pub struct WindowHandle;
impl WindowHandle {
    /// Marks the window dirty.
    pub fn request_frame(&self) {}
    /// Schedules a one-shot wakeup.
    pub fn request_frame_at(&self, _deadline: Instant) {}
    /// Enables continuous display-linked animation.
    pub fn set_animating(&self, _active: bool) {}
    /// Changes the native window title.
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
