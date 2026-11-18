//! Per-window restore containment. Broken layouts/loaders must not create a launch loop.
use crate::workspace::{Tab, Workspace};
use std::{
    collections::BTreeSet,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
};
use vel_session::{Document, Window};
use vel_text::Text;

#[cfg(test)]
thread_local! { static FAIL_AT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) }; }
#[cfg(test)]
pub(crate) const STEPS: &[&str] = &[
    "folders",
    "window_settings",
    "create_sheet",
    "groups",
    "visible_views",
    "restore_history",
    "restore_file",
    "attach_lazy",
    "transfer_buffer",
    "focus",
    "synchronize",
    "validate",
];
#[cfg(test)]
pub(crate) fn inject(step: &'static str) {
    FAIL_AT.with(|at| at.set(Some(step)));
}
#[inline]
pub(crate) fn step(_step: &'static str) {
    #[cfg(test)]
    FAIL_AT.with(|at| {
        if at.get() == Some(_step) {
            at.set(None);
            panic!("injected restore failure: {_step}");
        }
    });
}
#[derive(Debug)]
pub(crate) struct Original {
    source: PathBuf,
    bytes: Vec<u8>,
    windows: Vec<Vec<u64>>,
    stamp: u128,
}
type Originals = std::collections::BTreeMap<PathBuf, std::sync::Arc<Original>>;
fn originals() -> &'static std::sync::Mutex<Originals> {
    static ORIGINALS: std::sync::OnceLock<std::sync::Mutex<Originals>> = std::sync::OnceLock::new();
    ORIGINALS.get_or_init(Default::default)
}
#[cfg(test)]
pub(crate) fn evict_original(source: &Path) {
    originals()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(source);
}
pub(crate) fn original(source: &Path) -> Option<std::sync::Arc<Original>> {
    use std::sync::Arc;
    #[derive(serde::Deserialize)]
    struct Header {
        #[serde(default)]
        windows: Vec<WindowHeader>,
    }
    #[derive(serde::Deserialize)]
    struct WindowHeader {
        #[serde(default)]
        sheets: Vec<SheetHeader>,
    }
    #[derive(serde::Deserialize)]
    struct SheetHeader {
        id: u64,
    }
    let bytes = std::fs::read(source).ok();
    // Reading/parsing a session can be large; do it outside the shared cache lock.
    let windows = bytes
        .as_ref()
        .and_then(|bytes| {
            serde_json::from_slice::<Header>(bytes).ok().map(|h| {
                h.windows
                    .into_iter()
                    .map(|w| w.sheets.into_iter().map(|s| s.id).collect())
                    .collect()
            })
        })
        .unwrap_or_default();
    let mut cache = originals().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(bytes) = bytes {
        if let Some(old) = cache.get(source)
            && old.bytes == bytes
        {
            return Some(old.clone());
        }
        let entry = Arc::new(Original {
            source: source.to_owned(),
            bytes,
            windows,
            stamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        });
        if cache.len() >= 16
            && let Some(first) = cache.keys().next().cloned()
        {
            cache.remove(&first);
        }
        cache.insert(source.to_owned(), entry.clone());
        Some(entry)
    } else {
        cache.get(source).cloned()
    }
}
impl Workspace {
    /// Restore one window behind a recoverable boundary, preserving a backup on failure.
    pub fn restore_window(
        &mut self,
        window: &Window,
        documents: &[Document],
    ) -> Result<(), String> {
        let source = self
            .data_dir
            .as_ref()
            .map(|d| d.join("Sessions/Session.sublime-session"));
        self.restore_window_from(window, documents, source.as_deref())
    }
    pub(crate) fn restore_window_from(
        &mut self,
        window: &Window,
        documents: &[Document],
        source: Option<&Path>,
    ) -> Result<(), String> {
        self.restore_source = source.map(Path::to_owned);
        self.restore_original = source.and_then(original).or_else(|| {
            self.restore_original
                .take()
                .filter(|o| Some(o.source.as_path()) == source)
        });
        let settings = self.settings.clone();
        self.restore_in_progress = true;
        self.restore_deadline = Some(std::time::Instant::now() + crate::restore_io::BUDGET);
        // Each native/startup caller restores a window through this boundary.
        // Keep siblings consistent on ordinary IO errors; quarantine unexpected
        // failures before publishing recovered text so the next launch is safe.
        let result = catch_unwind(AssertUnwindSafe(|| {
            self.restore_window_inner(window, documents)
        }));
        self.restore_in_progress = false;
        self.restore_deadline = None;
        let reason = match result {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(error)) => error,
            Err(_) => "unexpected failure during session restoration".into(),
        };
        self.settings = settings;
        self.report_restore_failure(Some(window), documents, &reason);
        self.recover_window_text(window, documents)
    }
    pub(crate) fn report_restore_failure(
        &mut self,
        window: Option<&Window>,
        documents: &[Document],
        reason: &str,
    ) {
        let backup = backup(
            self.restore_source.as_deref(),
            self.restore_original.as_deref(),
            window,
            documents,
        );
        self.resource_reports.push(match backup {
            Ok(path) => format!(
                "Session recovery: {reason}. Original session backed up at {}",
                path.display()
            ),
            Err(error) => format!("Session recovery: {reason}. Could not save the backup: {error}"),
        });
    }
    fn recover_window_text(
        &mut self,
        window: &Window,
        documents: &[Document],
    ) -> Result<(), String> {
        let referenced: BTreeSet<_> = window.sheets.iter().map(|s| s.document).collect();
        let mut tabs = Vec::new();
        for document in documents.iter().filter(|d| referenced.contains(&d.id)) {
            let text = catch_unwind(AssertUnwindSafe(|| {
                document
                    .history
                    .as_ref()
                    .filter(|h| h.retains_text())
                    .and_then(|h| h.current_text().ok())
            }))
            .ok()
            .flatten();
            if let Some(text) = text {
                let mut tab = Tab::new(text, &self.settings, &self.keymap)?;
                // Recovery is a new unsaved document. A later save must never
                // overwrite the original merely because its restore failed.
                tab.editor.buffer.mark_unsaved();
                tab.editor.sync();
                tab.suggested_name = Some(format!(
                    "Recovered {}",
                    document
                        .path
                        .as_deref()
                        .and_then(Path::file_name)
                        .unwrap_or_default()
                        .to_string_lossy()
                ));
                tabs.push(tab);
            }
        }
        if tabs.is_empty() {
            tabs.push(Tab::new(Text::new(), &self.settings, &self.keymap)?);
        }
        self.tabs = tabs;
        self.active = 0;
        self.empty = false;
        self.workbench = Default::default();
        for tab in &self.tabs {
            self.workbench.groups[0].add(tab.sheet, false);
        }
        let first = self.tabs[0].sheet;
        self.workbench.groups[0].click(first, false, false);
        self.window_layers = Default::default();
        self.distraction_free = false;
        self.fullscreen = false;
        self.validate_document_views()
    }
}
fn backup(
    source: Option<&Path>,
    original: Option<&Original>,
    window: Option<&Window>,
    documents: &[Document],
) -> Result<PathBuf, String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    if let (Some(source), Some(original)) = (source, original) {
        use std::io::Write;
        let name = source.file_name().unwrap_or_default().to_string_lossy();
        let count = original.windows.len().max(1);
        let mut paths = Vec::with_capacity(count);
        // Prepare every window's original-byte backup before removing the source.
        // Later window failures use these same numbered backups, never a JSON copy.
        for number in 1..=count {
            let destination = source.with_file_name(format!(
                "{name}.restore-failed-{}-{number}.bak",
                original.stamp
            ));
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                // Like the session writer's temporary file, backups may contain
                // unsaved private text and must be private from creation onward.
                options.mode(0o600);
            }
            match options.open(&destination) {
                Ok(mut file) => {
                    file.write_all(&original.bytes).map_err(|e| e.to_string())?;
                    file.sync_all().map_err(|e| e.to_string())?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if std::fs::read(&destination).map_err(|e| e.to_string())? != original.bytes {
                        return Err(format!(
                            "incomplete session backup: {}",
                            destination.display()
                        ));
                    }
                }
                Err(e) => return Err(e.to_string()),
            }
            paths.push(destination);
        }
        let _ = std::fs::remove_file(source);
        let ids = window.map(|w| w.sheets.iter().map(|s| s.id).collect::<Vec<_>>());
        let index = ids
            .and_then(|ids| original.windows.iter().position(|saved| *saved == ids))
            .unwrap_or(0);
        return Ok(paths[index.min(paths.len() - 1)].clone());
    }
    let bytes = source
        .and_then(|p| std::fs::read(p).ok())
        .map(Ok)
        .unwrap_or_else(|| {
            let session = vel_session::Session {
                windows: window.cloned().into_iter().collect(),
                documents: documents.to_vec(),
                ..Default::default()
            };
            serde_json::to_vec(&session).map_err(|e| e.to_string())
        })?;
    let file = tempfile::Builder::new()
        .prefix(&format!("velocity-session-{stamp}-"))
        .suffix(".bak")
        .tempfile()
        .map_err(|e| e.to_string())?;
    std::fs::write(file.path(), bytes).map_err(|e| e.to_string())?;
    file.keep().map(|(_, path)| path).map_err(|e| e.to_string())
}
