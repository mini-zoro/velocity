//! Asynchronous Git status presentation. All repository operations use vel-git/gix.
use std::{
    cell::RefCell,
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
};
use vel_git::{Badge, GitService, Head, StatusSnapshot};
#[derive(Debug, Default)]
pub struct GitUi {
    services: BTreeMap<PathBuf, GitService>,
    badges: RefCell<BTreeMap<OsString, [Option<Option<Badge>>; 2]>>,
    pub snapshots: BTreeMap<PathBuf, Arc<StatusSnapshot>>,
    enabled: bool,
    labels: BTreeMap<PathBuf, String>,
}
impl GitUi {
    pub fn poll(&mut self, enabled: bool, paths: impl IntoIterator<Item = PathBuf>) -> bool {
        if !enabled {
            let changed = self.enabled;
            self.services.clear();
            self.snapshots.clear();
            self.badges.borrow_mut().clear();
            self.labels.clear();
            self.enabled = false;
            return changed;
        }
        self.enabled = true;
        let paths: std::collections::BTreeSet<_> = paths.into_iter().collect();
        self.services.retain(|path, _| paths.contains(path));
        let before = self.snapshots.len();
        self.snapshots.retain(|path, _| paths.contains(path));
        let mut changed = before != self.snapshots.len();
        for path in paths {
            if !self.services.contains_key(&path)
                && let Ok(service) = GitService::new(path.clone(), true)
            {
                self.services.insert(path.clone(), service);
            }
            if let Some(service) = self.services.get(&path) {
                if let Some(snapshot) = service.try_snapshot()
                    && self
                        .snapshots
                        .get(&path)
                        .is_none_or(|old| old.generation != snapshot.generation)
                {
                    self.labels
                        .insert(snapshot.root.clone(), status_label(&snapshot));
                    self.snapshots.insert(path.clone(), snapshot);
                    changed = true;
                }
                // Watch setup can fail in restricted environments; a valid status stays useful.
                let _ = service.try_error();
            }
        }
        if changed {
            self.badges.borrow_mut().clear();
        }
        changed
    }
    /// Publishes a status snapshot without a repository service (developer scene renderer).
    #[cfg(target_os = "macos")]
    pub fn insert_snapshot(&mut self, snapshot: StatusSnapshot) {
        self.enabled = true;
        self.labels
            .insert(snapshot.root.clone(), status_label(&snapshot));
        self.snapshots
            .insert(snapshot.root.clone(), Arc::new(snapshot));
        self.badges.borrow_mut().clear();
    }
    pub fn snapshot(&self, path: &Path) -> Option<&StatusSnapshot> {
        self.snapshots
            .values()
            .filter(|s| path.starts_with(&s.root))
            .max_by_key(|s| s.root.components().count())
            .map(AsRef::as_ref)
    }
    pub fn badge(&self, path: &Path, folder: bool) -> Option<Badge> {
        if self.snapshots.is_empty() {
            return None;
        }
        let column = usize::from(folder);
        if let Some(result) = self
            .badges
            .borrow()
            .get(path.as_os_str())
            .and_then(|values| values[column])
        {
            return result;
        }
        let result = self.snapshot(path).and_then(|s| {
            if folder {
                s.folders
                    .get(path.strip_prefix(&s.root).ok()?)
                    .copied()
                    .or_else(|| s.badge(path))
            } else {
                s.badge(path)
            }
        });
        let mut cache = self.badges.borrow_mut();
        if cache.len() >= 4096 {
            cache.clear();
        }
        cache
            .entry(path.as_os_str().to_owned())
            .or_insert([None; 2])[column] = Some(result);
        result
    }
    pub fn status(&self, path: &Path) -> Option<&str> {
        self.labels
            .get(&self.snapshot(path)?.root)
            .map(String::as_str)
    }
}
fn status_label(s: &StatusSnapshot) -> String {
    let name = match &s.head {
        Head::Branch { name, .. } => name.as_str(),
        Head::Detached(id) => &id[..id.len().min(8)],
    };
    let count = |badges: &[Badge]| {
        s.files
            .values()
            .filter(|f| f.statuses.iter().any(|b| badges.contains(b)))
            .count()
    };
    format!(
        "{name}  ?{} M{} S{} !{}",
        count(&[Badge::Untracked]),
        count(&[Badge::Modified, Badge::Missing]),
        count(&[
            Badge::StagedAddition,
            Badge::StagedModification,
            Badge::StagedDeletion
        ]),
        count(&[Badge::Unmerged])
    )
}
pub fn badge_text(badge: Badge) -> (&'static str, &'static str) {
    match badge {
        Badge::Ignored => ("", "Ignored"),
        Badge::Untracked => ("?", "Untracked"),
        Badge::Modified => ("M", "Modified"),
        Badge::Missing => ("!", "Missing"),
        Badge::StagedAddition => ("A+", "Staged Addition"),
        Badge::StagedModification => ("M+", "Staged Modification"),
        Badge::StagedDeletion => ("D+", "Staged Deletion"),
        Badge::Unmerged => ("U!", "Unmerged"),
    }
}

pub fn badge_tooltip(badge: Badge, tr: &vel_i18n::Translator) -> std::borrow::Cow<'static, str> {
    match badge {
        Badge::Ignored => tr.tr("Ignored"),
        Badge::Untracked => tr.tr("Untracked"),
        Badge::Modified => tr.tr("Modified"),
        Badge::Missing => tr.tr("Missing"),
        Badge::StagedAddition => tr.tr("Staged Addition"),
        Badge::StagedModification => tr.tr("Staged Modification"),
        Badge::StagedDeletion => tr.tr("Staged Deletion"),
        Badge::Unmerged => tr.tr("Unmerged"),
    }
}
