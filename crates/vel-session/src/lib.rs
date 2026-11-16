//! Atomic, debounced sessions and bounded undo reconstruction using vel-buffer's public API.
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]
pub mod history;
// Legacy code is only a reader for existing round-seven profiles.
mod legacy;
pub mod project;
pub use history::{History, Position, ViewState};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use vel_ui::workbench::Workbench;

/// Documented Hot Exit policies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotExit {
    /// Retain dirty windows on close and quit.
    #[default]
    Always,
    /// Retain dirty buffers only when quitting the application.
    OnlyOnQuit,
    /// Closing or quitting requires normal Save/Discard/Cancel handling.
    Disabled,
}
impl HotExit {
    /// Tolerant settings conversion; malformed preferences retain the default.
    pub fn parse(value: &str) -> Self {
        match value {
            "only_on_quit" => Self::OnlyOnQuit,
            "disabled" => Self::Disabled,
            _ => Self::Always,
        }
    }
    /// Accepts current strings and the legacy boolean (`false` disables Hot Exit).
    pub fn from_setting(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Bool(false) => Self::Disabled,
            serde_json::Value::String(value) => Self::parse(value),
            _ => Self::Always,
        }
    }
    /// Whether a close may retain dirty contents without prompting.
    pub fn retain(self, quitting: bool) -> bool {
        self == Self::Always || (quitting && self == Self::OnlyOnQuit)
    }
}
/// Identity of a saved disk version. A missing or changed file invalidates restored history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskStamp {
    /// Byte length.
    pub bytes: u64,
    /// Modification timestamp in nanoseconds since Unix epoch.
    pub modified_ns: u128,
}
impl DiskStamp {
    /// Reads metadata without creating directories or opening document contents.
    pub fn read(path: &Path) -> Option<Self> {
        let m = fs::metadata(path).ok()?;
        Some(Self {
            bytes: m.len(),
            modified_ns: m
                .modified()
                .ok()?
                .duration_since(UNIX_EPOCH)
                .ok()?
                .as_nanos(),
        })
    }
}
/// A shared document; several sheets can reference its ID.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Document {
    /// Session-local document identity.
    pub id: u64,
    /// Absent for untitled documents.
    pub path: Option<PathBuf>,
    /// Disk version when saved/loaded.
    pub disk: Option<DiskStamp>,
    /// File encoding name.
    pub encoding: String,
    /// Byte-order marker preference.
    pub bom: bool,
    /// LF, CRLF or CR.
    pub line_ending: String,
    /// Unsaved state, including file-format changes.
    pub dirty: bool,
    /// Dirty/untitled contents and undo. Clean, hidden files may load lazily from disk.
    pub history: Option<History>,
}
/// Retained HTML sheet content and tab name. Transient popups/phantoms are not serialized.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HtmlSheet {
    /// User-visible tab name.
    pub name: String,
    /// Minihtml document source, bounded to the parser's 4 MiB input limit.
    pub contents: String,
}
/// One sheet's view-local state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sheet {
    /// Optional retained minihtml sheet; missing in older sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<HtmlSheet>,
    /// Stable ID used in the workbench.
    pub id: u64,
    /// Shared document ID.
    pub document: u64,
    /// Selection, viewport and folds.
    pub view: ViewState,
    /// Explicit syntax override, not a detected syntax.
    pub syntax: Option<String>,
    /// Whether the sheet is a semi-transient preview.
    pub transient: bool,
}
/// Restorable window and project UI state.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Window {
    /// Project file, if any.
    pub project: Option<PathBuf>,
    /// Added folders, including unsaved projects.
    pub folders: Vec<PathBuf>,
    /// Group geometry, order and selections.
    pub workbench: Workbench,
    /// Open sheets.
    pub sheets: Vec<Sheet>,
    /// Sidebar visibility.
    pub sidebar_visible: bool,
    /// Expanded directory paths.
    pub sidebar_expanded: Vec<PathBuf>,
    /// Sidebar scroll offset.
    pub sidebar_scroll: f64,
    /// Find queries, oldest first.
    pub find_history: Vec<String>,
    /// Replacement strings, oldest first.
    pub replace_history: Vec<String>,
    /// Distraction-free state.
    pub distraction_free: bool,
    /// Sidebar visibility remembered outside distraction-free mode.
    pub normal_sidebar: Option<bool>,
    /// Sidebar visibility remembered within distraction-free mode.
    pub distraction_sidebar: Option<bool>,
    /// Whether full screen was already active when distraction-free mode was entered.
    pub fullscreen_before_distraction: bool,
    /// Native full-screen preference.
    pub fullscreen: bool,
    /// Explicit build system selection; None means Automatic.
    pub build_system: Option<String>,
}
/// Versioned application session. Unknown fields are tolerated; invalid references are rejected.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    /// Format version (currently 1).
    pub version: u32,
    /// Shared buffers.
    pub documents: Vec<Document>,
    /// Window order, first visible window first.
    pub windows: Vec<Window>,
    /// Recent file paths, newest first.
    pub recent_files: Vec<PathBuf>,
    /// Recent folders, newest first.
    pub recent_folders: Vec<PathBuf>,
    /// Recent projects, newest first.
    pub recent_projects: Vec<PathBuf>,
}
impl Default for Session {
    fn default() -> Self {
        Self {
            version: 1,
            documents: vec![],
            windows: vec![],
            recent_files: vec![],
            recent_folders: vec![],
            recent_projects: vec![],
        }
    }
}
impl Session {
    /// Checks all identities, coordinates and untrusted numeric view data.
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 || self.windows.len() > 128 || self.documents.len() > 10_000 {
            return Err("unsupported/oversized session".into());
        }
        let docs: BTreeSet<_> = self.documents.iter().map(|d| d.id).collect();
        if docs.len() != self.documents.len() {
            return Err("duplicate document identity".into());
        }
        let history_views: BTreeMap<_, BTreeSet<_>> = self
            .documents
            .iter()
            .filter_map(|d| {
                d.history
                    .as_ref()
                    .map(|h| (d.id, h.view_ids().into_iter().collect()))
            })
            .collect();
        let mut attached = BTreeSet::new();
        let mut all = BTreeSet::new();
        for w in &self.windows {
            let ids = w.sheets.iter().map(|s| s.id).collect();
            w.workbench.validate(&ids)?;
            if !w.sidebar_scroll.is_finite() || w.sidebar_scroll < 0. {
                return Err("invalid sidebar scroll".into());
            }
            for s in &w.sheets {
                if s.html
                    .as_ref()
                    .is_some_and(|h| h.contents.len() > 4 * 1024 * 1024 || h.name.len() > 4096)
                {
                    return Err("invalid HTML sheet content".into());
                }
                if !all.insert(s.id) || !docs.contains(&s.document) {
                    return Err("invalid sheet identity/document".into());
                }
                s.view.validate()?;
                if let Some(view) = s.view.history_view
                    && let Some(views) = history_views.get(&s.document)
                    && (!views.contains(&view) || !attached.insert((s.document, view)))
                {
                    return Err("invalid history view ownership".into());
                }
            }
        }
        Ok(())
    }
    /// Removes dirty contents when policy forbids retaining them. Untitled sheets are removed.
    pub fn apply_policy(&mut self, hot: HotExit, quitting: bool, remember: bool) {
        if !remember && !hot.retain(quitting) {
            self.documents.clear();
            self.windows.clear();
            return;
        }
        if hot.retain(quitting) {
            return;
        }
        let untitled: BTreeSet<_> = self
            .documents
            .iter()
            .filter(|d| d.path.is_none())
            .map(|d| d.id)
            .collect();
        self.documents.retain(|d| !untitled.contains(&d.id));
        for d in &mut self.documents {
            d.history = None;
            d.dirty = false;
        }
        for w in &mut self.windows {
            let removed: Vec<_> = w
                .sheets
                .iter()
                .filter(|s| untitled.contains(&s.document))
                .map(|s| s.id)
                .collect();
            w.sheets.retain(|s| !untitled.contains(&s.document));
            for g in &mut w.workbench.groups {
                for id in &removed {
                    g.remove(*id);
                }
            }
        }
    }
}
/// Loads without throwing startup errors; corrupt bytes are quarantined beside the original.
#[derive(Debug, Default)]
pub struct Recovery {
    /// Empty on absence, unreadable storage or invalid data.
    pub session: Session,
    /// Human-readable diagnostics, including quarantine failures.
    pub messages: Vec<String>,
}
/// Reads a session without requiring any user directory to exist.
pub fn load(path: &Path) -> Recovery {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Recovery::default(),
        Err(e) => {
            return Recovery {
                messages: vec![format!("session {}: {e}", path.display())],
                ..Recovery::default()
            };
        }
    };
    let parsed = serde_json::from_slice::<Session>(&bytes)
        .map_err(|e| e.to_string())
        .and_then(|s| s.validate().map(|()| s));
    match parsed {
        Ok(session) => Recovery {
            session,
            messages: vec![],
        },
        Err(e) => {
            let suffix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let aside = path.with_extension(format!("corrupt-{suffix}"));
            let message = match fs::rename(path, &aside) {
                Ok(()) => format!("corrupt session kept at {}: {e}", aside.display()),
                Err(io) => format!(
                    "corrupt session {} (could not quarantine: {io}): {e}",
                    path.display()
                ),
            };
            Recovery {
                messages: vec![message],
                ..Recovery::default()
            }
        }
    }
}
/// Writes a complete sibling temporary, fsyncs, and atomically replaces the destination.
/// Existing sessions survive serialization/IO failure. Missing parent directories are created.
pub fn atomic_write(
    path: &Path,
    write: impl FnOnce(&mut fs::File) -> Result<(), String>,
) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    write(temporary.as_file_mut())?;
    temporary.as_file_mut().flush().map_err(|e| e.to_string())?;
    sync_file(temporary.as_file())?;
    temporary.persist(path).map_err(|e| e.to_string())?;
    sync_file(&fs::File::open(parent).map_err(|e| e.to_string())?)?;
    Ok(())
}
/// Serializes and atomically saves one validated snapshot. Run this on a worker.
pub fn save(path: &Path, session: &Session) -> Result<(), String> {
    session.validate()?;
    atomic_write(path, |file| {
        let mut writer = std::io::BufWriter::with_capacity(64 * 1024, file);
        serde_json::to_writer(&mut writer, session).map_err(|e| e.to_string())?;
        writer.flush().map_err(|e| e.to_string())
    })
}

#[derive(Debug)]
enum Request {
    Changed,
    Retire(Session),
    Flush(mpsc::Sender<Result<Duration, String>>),
    Stop,
}
/// Latest-snapshot worker: changes debounce, flush/quit waits for durable completion.
/// Snapshots retain rope roots; serialization and disk writes never execute on the caller.
#[derive(Debug)]
pub struct Writer {
    latest: Arc<Mutex<Option<Session>>>,
    tx: mpsc::Sender<Request>,
    /// Completion times and errors from background autosaves.
    pub results: mpsc::Receiver<Result<Duration, String>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Writer {
    /// Starts a worker without touching the data directory until its first save.
    pub fn new(path: PathBuf, debounce: Duration) -> Result<Self, String> {
        let latest = Arc::new(Mutex::new(None::<Session>));
        let copy = latest.clone();
        let (tx, rx) = mpsc::channel();
        let (report, results) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("velocity-session".into())
            .spawn(move || {
                let mut dirty = false;
                loop {
                    let request = if dirty {
                        rx.recv_timeout(debounce)
                    } else {
                        rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                    };
                    match request {
                        Ok(Request::Changed) => dirty = true,
                        Ok(Request::Retire(old)) => drop(old),
                        Ok(Request::Flush(reply)) => {
                            let result = write_latest(&path, &copy);
                            let _ = reply.send(result);
                            dirty = false;
                        }
                        Ok(Request::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            let _ = report.send(write_latest(&path, &copy));
                            dirty = false;
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            latest,
            tx,
            results,
            thread: Some(thread),
        })
    }
    /// Replaces the pending snapshot. Old snapshots are retired on the worker.
    pub fn changed(&self, session: Session) -> Result<(), String> {
        // Dropping Text roots is inexpensive; full history retirement stays bounded by limits.
        let old = self
            .latest
            .lock()
            .map_err(|e| e.to_string())?
            .replace(session);
        if let Some(old) = old {
            self.tx
                .send(Request::Retire(old))
                .map_err(|e| e.to_string())?;
        }
        self.tx.send(Request::Changed).map_err(|e| e.to_string())
    }
    /// Durably writes the latest snapshot and reports worker elapsed time.
    pub fn flush(&self) -> Result<Duration, String> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(Request::Flush(tx))
            .map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())?
    }
}
fn write_latest(path: &Path, latest: &Mutex<Option<Session>>) -> Result<Duration, String> {
    let at = Instant::now();
    let session = latest.lock().map_err(|e| e.to_string())?.take();
    if let Some(session) = session
        && let Err(error) = save(path, &session)
    {
        let mut pending = latest.lock().map_err(|e| e.to_string())?;
        if pending.is_none() {
            *pending = Some(session);
        }
        return Err(error);
    }
    Ok(at.elapsed())
}
impl Drop for Writer {
    fn drop(&mut self) {
        let _ = self.tx.send(Request::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn sync_file(file: &fs::File) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: the file owns a valid descriptor throughout this POSIX fsync.
        // We require OS-crash ordering, not Apple's optional F_FULLFSYNC hardware barrier.
        if unsafe { libc::fsync(file.as_raw_fd()) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error().to_string())
        }
    }
    #[cfg(not(unix))]
    {
        file.sync_all().map_err(|e| e.to_string())
    }
}
