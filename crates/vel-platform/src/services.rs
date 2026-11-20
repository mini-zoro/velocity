//! Injectable operating-system services for file commands.
use std::path::Path;

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
