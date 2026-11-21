//! Injectable operating-system services for file commands.
use std::path::Path;

/// The system-wide Find pasteboard, independent of copy and paste.
pub trait FindPasteboard: std::fmt::Debug {
    /// Reads the most recent Find string, if available.
    fn read(&self) -> Option<String>;
    /// Publishes a Find string for other apps.
    fn write(&self, text: &str) -> bool;
}

/// AppKit's named Find pasteboard; unavailable on other platforms.
#[derive(Debug)]
pub struct SystemFindPasteboard;

#[cfg(target_os = "macos")]
impl FindPasteboard for SystemFindPasteboard {
    fn read(&self) -> Option<String> {
        use objc2_app_kit::{NSPasteboard, NSPasteboardNameFind, NSPasteboardTypeString};
        // SAFETY: these AppKit constants remain valid for the process lifetime.
        let (name, kind) = unsafe { (NSPasteboardNameFind, NSPasteboardTypeString) };
        NSPasteboard::pasteboardWithName(name)
            .stringForType(kind)
            .map(|s| s.to_string())
    }

    fn write(&self, text: &str) -> bool {
        use objc2_app_kit::{NSPasteboard, NSPasteboardNameFind, NSPasteboardTypeString};
        use objc2_foundation::NSString;
        // SAFETY: these AppKit constants remain valid for the process lifetime.
        let (name, kind) = unsafe { (NSPasteboardNameFind, NSPasteboardTypeString) };
        let board = NSPasteboard::pasteboardWithName(name);
        board.clearContents();
        board.setString_forType(&NSString::from_str(text), kind)
    }
}

#[cfg(not(target_os = "macos"))]
impl FindPasteboard for SystemFindPasteboard {
    fn read(&self) -> Option<String> {
        None
    }
    fn write(&self, _: &str) -> bool {
        false
    }
}

/// Opens a local HTML document in the user's default web browser.
pub trait BrowserOpener {
    /// Requests the open. The caller must keep the file alive after success.
    fn open_browser_file(&self, path: &Path) -> bool;
}

impl BrowserOpener for crate::WindowHandle {
    fn open_browser_file(&self, path: &Path) -> bool {
        self.open_local_html(path)
    }
}

/// A single clipboard item with interchangeable HTML and plain-text representations.
pub trait RichClipboard {
    /// Replaces the clipboard with one item containing both representations.
    fn set_clipboard_html(&self, html: &str, plain_text: &str) -> bool;
}

impl RichClipboard for crate::WindowHandle {
    fn set_clipboard_html(&self, html: &str, plain_text: &str) -> bool {
        self.write_clipboard_html(html, plain_text)
    }
}

/// Native operations used by file commands; tests can inject an in-memory fake.
/// Confirmation and error presentation belong to the caller, before/after this API.
pub trait NativeFileOperations {
    /// Move an item to the system Trash. Never permanently remove it on failure.
    fn trash_file(&self, path: &Path) -> Result<(), String>;
    /// Select an item in the system file viewer.
    fn reveal_file(&self, path: &Path) -> Result<(), String>;
    /// Copy plain text to the system clipboard, reporting a failed write.
    fn copy_text(&self, text: &str) -> Result<(), String>;
}

impl NativeFileOperations for crate::WindowHandle {
    fn trash_file(&self, path: &Path) -> Result<(), String> {
        self.trash_file(path)
    }
    fn reveal_file(&self, path: &Path) -> Result<(), String> {
        self.reveal_file(path)
    }
    fn copy_text(&self, text: &str) -> Result<(), String> {
        if self.set_clipboard_text(text) {
            Ok(())
        } else {
            Err("could not write to the system clipboard".into())
        }
    }
}
