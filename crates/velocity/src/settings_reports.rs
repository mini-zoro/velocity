//! Non-modal settings diagnostics, once per physical file revision and across windows.
use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

#[derive(Debug, Default)]
pub(crate) struct Scope(Arc<()>);
#[derive(Default)]
struct Source {
    revision: Option<u64>,
    reported: bool,
    pending: Option<String>,
    owner: Weak<()>,
    writer: Weak<()>,
}
static SOURCES: OnceLock<Mutex<BTreeMap<PathBuf, Source>>> = OnceLock::new();

pub(crate) fn source(message: &str) -> Option<PathBuf> {
    let (prefix, _) = message.split_once(".sublime-settings")?;
    let path = PathBuf::from(format!("{prefix}.sublime-settings"));
    path.is_absolute()
        .then(|| path.canonicalize().unwrap_or(path))
}
fn registry() -> std::sync::MutexGuard<'static, BTreeMap<PathBuf, Source>> {
    SOURCES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}
fn prune_removed() {
    let paths = registry().keys().cloned().collect::<Vec<_>>();
    let removed = paths
        .into_iter()
        .filter(|p| !p.exists())
        .collect::<Vec<_>>();
    // IO stays outside the lock. Keep reported revisions across window closes,
    // but drop deleted sources rather than retaining a file/window history.
    registry().retain(|p, s| !removed.contains(p) || s.pending.is_some());
}
impl Scope {
    /// Remember the initiating window before publishing a save or queued writer.
    pub fn claim(&self, path: &Path) {
        if path.extension().is_some_and(|e| e == "sublime-settings") {
            let path = path.canonicalize().unwrap_or_else(|_| path.into());
            registry().entry(path).or_default().writer = Arc::downgrade(&self.0);
        }
    }
    pub fn take(&self, reports: Vec<String>) -> Vec<String> {
        let mut output = Vec::new();
        // Hashing/IO stays outside the shared lock. Include mtime so explicitly
        // saving the same broken bytes counts as a new file revision.
        let settings = reports
            .into_iter()
            .filter_map(|message| {
                let Some(path) = source(&message) else {
                    output.push(message);
                    return None;
                };
                let mut revision = std::collections::hash_map::DefaultHasher::new();
                std::fs::read(&path).ok().hash(&mut revision);
                std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok()
                    .hash(&mut revision);
                Some((path, revision.finish(), message))
            })
            .collect::<Vec<_>>();
        if !settings.is_empty() {
            prune_removed();
        }
        let owner = Arc::downgrade(&self.0);
        let mut sources = registry();
        for (path, revision, message) in settings {
            let entry = sources.entry(path).or_default();
            if entry.revision != Some(revision) {
                entry.revision = Some(revision);
                entry.reported = false;
                entry.pending = None;
                entry.owner = if entry.writer.strong_count() > 0 {
                    entry.writer.clone()
                } else {
                    owner.clone()
                };
            }
            if !entry.reported {
                entry.pending.get_or_insert(message);
                if entry.owner.strong_count() == 0 {
                    entry.owner = owner.clone();
                }
            }
        }
        for entry in sources.values_mut().filter(|s| s.owner.ptr_eq(&owner)) {
            if let Some(message) = entry.pending.take() {
                output.push(message);
                entry.reported = true;
            }
        }
        output
    }
}
impl crate::workspace::Workspace {
    pub(crate) fn take_resource_reports(&mut self) -> Vec<String> {
        self.settings_reports
            .take(std::mem::take(&mut self.resource_reports))
    }
}
