//! Application-wide session coordinator shared by all native window controllers.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::Duration,
};
use vel_session::{Document, HotExit, Session, Window, Writer};

/// Checkpoints must preserve documents even when one window's UI state is broken.
fn repair_window(window: &mut Window, documents: &[Document]) {
    let docs: BTreeSet<_> = documents.iter().map(|doc| doc.id).collect();
    let mut ids = BTreeSet::new();
    let before = window.sheets.len();
    window
        .sheets
        .retain(|sheet| docs.contains(&sheet.document) && ids.insert(sheet.id));
    let mut repaired = before != window.sheets.len();
    // Keep documents even if a damaged window lost their last sheet reference.
    for document in documents {
        if !window.sheets.iter().any(|s| s.document == document.id) {
            let id = crate::workbench::next_sheet();
            ids.insert(id);
            window.sheets.push(vel_session::Sheet {
                id,
                document: document.id,
                view: Default::default(),
                syntax: None,
                transient: false,
                html: None,
                settings: None,
            });
            repaired = true;
        }
    }
    repaired |= window.workbench.normalize(&ids);
    if !window.sidebar_scroll.is_finite() || window.sidebar_scroll < 0. {
        window.sidebar_scroll = 0.;
        repaired = true;
    }
    let valid = Session {
        windows: vec![window.clone()],
        documents: documents.to_vec(),
        ..Default::default()
    }
    .validate();
    if let Err(error) = valid {
        // Last resort: retain all document histories, discard only window layout
        // and view metadata. A bad layout must never prevent saving dirty text.
        eprintln!("Saving Hot Exit documents without window layout: {error}");
        window.workbench = Default::default();
        for sheet in &mut window.sheets {
            sheet.view = Default::default();
            if sheet
                .html
                .as_ref()
                .is_some_and(|h| h.contents.len() > 4 * 1024 * 1024 || h.name.len() > 4096)
            {
                sheet.html = None;
            }
            window.workbench.groups[0].add(sheet.id, false);
        }
        repaired = true;
    }
    if repaired {
        eprintln!(
            "Repaired session window; retained {} documents",
            documents.len()
        );
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub hot: HotExit,
    pub quitting: bool,
    pub remember: bool,
    pub closing: bool,
}
#[derive(Debug)]
pub struct Sessions {
    pub restored: Session,
    pub(crate) original: Option<std::sync::Arc<crate::session_restore::Original>>,
    pub active_window: Option<u64>,
    recent: [Vec<PathBuf>; 3],
    recent_heads: BTreeMap<u64, [Option<PathBuf>; 3]>,
    forgotten_recent: [BTreeSet<PathBuf>; 3],
    recent_opened: Vec<PathBuf>,
    system_recent_enabled: Option<bool>,
    hot: HotExit,
    remember: bool,
    pub messages: Vec<String>,
    windows: BTreeMap<u64, (Window, Vec<Document>)>,
    closed: Vec<(Window, Vec<Document>)>,
    writer: Option<Writer>,
    profile: Option<PathBuf>,
    workspaces: BTreeMap<PathBuf, Writer>,
    next: u64,
    live: BTreeSet<u64>,
    transfers: BTreeMap<u64, Vec<(usize, crate::workspace::Tab)>>,
    pub(crate) open_windows: BTreeMap<u64, crate::open_requests::WindowRoute>,
    pub(crate) open_inbox: BTreeMap<u64, Vec<crate::instance::Incoming>>,
    pub(crate) menu_merges: BTreeMap<u64, u64>,
    pub(crate) persist_project_workspaces: bool,
    // Drop all writers before releasing the per-profile lock.
    pub instance: Option<crate::instance::Instance>,
}
impl Sessions {
    pub fn new(data: Option<&Path>) -> Self {
        let mut messages = Vec::new();
        #[cfg(unix)]
        if data.is_some() {
            messages.extend(vel_fileio::recover_saves(data));
        }
        let path = data.map(|p| p.join("Sessions/Session.sublime-session"));
        let recovery = path.as_deref().map(vel_session::load).unwrap_or_default();
        // Every recovered window owns the launch snapshot independently of cache eviction.
        let original = recovery
            .source
            .as_deref()
            .and_then(crate::session_restore::original);
        messages.extend(recovery.messages);
        let writer = path.and_then(|p| match Writer::new(p, Duration::from_millis(400)) {
            Ok(w) => Some(w),
            Err(e) => {
                messages.push(e);
                None
            }
        });
        Self {
            restored: recovery.session,
            original,
            instance: None,
            active_window: None,
            recent: Default::default(),
            recent_heads: BTreeMap::new(),
            forgotten_recent: Default::default(),
            recent_opened: Vec::new(),
            system_recent_enabled: None,
            hot: HotExit::Always,
            remember: true,
            messages,
            windows: BTreeMap::new(),
            closed: vec![],
            writer,
            profile: data.map(Path::to_owned),
            workspaces: BTreeMap::new(),
            next: 1,
            live: BTreeSet::new(),
            transfers: BTreeMap::new(),
            open_windows: BTreeMap::new(),
            open_inbox: BTreeMap::new(),
            menu_merges: BTreeMap::new(),
            persist_project_workspaces: true,
        }
    }
    pub fn register(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        self.active_window = Some(id);
        self.live.insert(id);
        self.recent_heads
            .insert(id, self.recent.each_ref().map(|p| p.first().cloned()));
        id
    }
    pub fn can_receive_tab(&self, id: u64) -> bool {
        self.can_receive_tabs(id, 1)
    }
    pub fn can_receive_tabs(&self, id: u64, count: usize) -> bool {
        self.live.contains(&id)
            && self
                .transfers
                .get(&id)
                .map_or(0, Vec::len)
                .saturating_add(count)
                <= 64
    }
    pub fn transfer_tab(&mut self, id: u64, group: usize, tab: crate::workspace::Tab) {
        self.transfers.entry(id).or_default().push((group, tab));
    }
    pub fn take_tabs(&mut self, id: u64) -> Vec<(usize, crate::workspace::Tab)> {
        self.transfers.remove(&id).unwrap_or_default()
    }
    /// Reserves recovered windows before any live window can replace their snapshot.
    ///
    /// Returns window IDs in restored order and retains only each window's referenced
    /// documents. Reserved copies use fresh document IDs so snapshots from the new
    /// process cannot collide with IDs from an earlier process. `restored` stays intact
    /// for lazy workspace restoration; this method does not publish to disk.
    pub fn reserve_recovered(&mut self) -> Vec<u64> {
        // Reserved windows keep their old sheet IDs until lazily restored. Do not let
        // new tabs/documents reuse those IDs before the first snapshot is published.
        if let Some(maximum) = self
            .restored
            .windows
            .iter()
            .flat_map(|window| window.sheets.iter().map(|sheet| sheet.id))
            .chain(self.restored.documents.iter().map(|document| document.id))
            .max()
        {
            crate::workbench::reserve_sheet_ids(maximum);
        }
        self.recent = [
            self.restored.recent_files.clone(),
            self.restored.recent_folders.clone(),
            self.restored.recent_projects.clone(),
        ];
        let documents: BTreeMap<_, _> = self
            .restored
            .documents
            .iter()
            .map(|document| (document.id, crate::workbench::next_sheet()))
            .collect();
        let mut reserved = Vec::with_capacity(self.restored.windows.len());
        for index in 0..self.restored.windows.len() {
            let mut window = self.restored.windows[index].clone();
            let referenced: BTreeSet<_> =
                window.sheets.iter().map(|sheet| sheet.document).collect();
            let retained = self
                .restored
                .documents
                .iter()
                .filter(|document| referenced.contains(&document.id))
                .map(|document| {
                    let mut document = document.clone();
                    document.id = documents[&document.id];
                    document
                })
                .collect();
            for sheet in &mut window.sheets {
                if let Some(document) = documents.get(&sheet.document) {
                    sheet.document = *document;
                }
            }
            let id = self.register();
            self.recent_heads.insert(
                id,
                std::array::from_fn(|index| self.recent[index].first().cloned()),
            );
            self.windows.insert(id, (window, retained));
            reserved.push(id);
        }
        self.active_window = reserved.first().copied().or(self.active_window);
        reserved
    }
    pub fn publish(
        &mut self,
        id: u64,
        mut window: Window,
        documents: Vec<Document>,
        recent: [Vec<PathBuf>; 3],
        policy: Policy,
    ) -> Result<(), String> {
        self.hot = policy.hot;
        self.remember = policy.remember;
        repair_window(&mut window, &documents);
        self.windows.insert(id, (window.clone(), documents.clone()));
        self.record_recents(id, recent);
        let mut session = self.collect(self.recent.clone());
        let hot = if !policy.quitting && !policy.closing && policy.hot == HotExit::OnlyOnQuit {
            HotExit::Always
        } else {
            policy.hot
        };
        session.apply_policy(hot, policy.quitting, policy.remember);
        if let Some(writer) = &self.writer {
            writer.changed(session)?;
        }
        if self.persist_project_workspaces
            && let Some(project) = &window.project
        {
            let path = vel_session::project::workspace_path(project);
            if !self.workspaces.contains_key(&path) {
                self.workspaces.insert(
                    path.clone(),
                    Writer::for_project(
                        project.clone(),
                        self.profile.clone(),
                        Duration::from_millis(400),
                    )?,
                );
            }
            let mut session = Session {
                windows: vec![window],
                documents,
                ..Session::default()
            };
            session.apply_policy(hot, policy.quitting, policy.remember);
            self.workspaces[&path].changed(session)?;
        }
        Ok(())
    }
    // ST4 builds 4143 and 4152: application recents are global, closed views are local.
    // https://www.sublimetext.com/download
    pub fn sync_recents(&mut self, id: u64, lists: [&mut Vec<PathBuf>; 3]) {
        if lists
            .iter()
            .zip(&self.recent)
            .all(|(local, shared)| &***local == shared.as_slice())
        {
            return;
        }
        self.record_recents(id, std::array::from_fn(|i| lists[i].clone()));
        for (local, shared) in lists.into_iter().zip(&self.recent) {
            local.clone_from(shared);
        }
        self.recent_heads
            .insert(id, self.recent.each_ref().map(|p| p.first().cloned()));
    }
    fn record_recents(&mut self, id: u64, recent: [Vec<PathBuf>; 3]) {
        let heads = std::array::from_fn(|index| recent[index].first().cloned());
        let previous = self.recent_heads.insert(id, heads);
        for (index, mut incoming) in recent.into_iter().enumerate() {
            if let Some(head) = incoming.first()
                && previous
                    .as_ref()
                    .is_some_and(|old| old[index].as_ref() != Some(head))
            {
                self.forgotten_recent[index].remove(head);
            }
            incoming.retain(|path| !self.forgotten_recent[index].contains(path));
            if index == 0 {
                for path in incoming
                    .iter()
                    .rev()
                    .filter(|p| !self.recent[0].contains(p))
                {
                    crate::workbench::remember(&mut self.recent_opened, path.clone());
                }
                if let Some(head) = incoming.first()
                    && previous
                        .as_ref()
                        .is_some_and(|old| old[0].as_ref() != Some(head))
                {
                    crate::workbench::remember(&mut self.recent_opened, head.clone());
                }
            }
            for path in incoming.iter().rev() {
                if !self.recent[index].contains(path) {
                    self.recent[index].insert(0, path.clone());
                }
            }
            if let Some(head) = incoming.first()
                && previous
                    .as_ref()
                    .is_some_and(|p| p[index].as_ref() != Some(head))
            {
                self.recent[index].retain(|p| p != head);
                self.recent[index].insert(0, head.clone());
            }
            self.recent[index].truncate(100);
        }
    }
    // ST4 4143, https://www.sublimetext.com/download: publish opened files and
    // clear the OS list when update_system_recent_files is turned off.
    pub fn update_system_recents(&mut self, enabled: bool, mut emit: impl FnMut(Option<&Path>)) {
        let previous = self.system_recent_enabled.replace(enabled);
        let opened = std::mem::take(&mut self.recent_opened);
        if enabled {
            for path in opened.iter().rev() {
                emit(Some(path));
            }
        } else if previous != Some(false) {
            emit(None);
        }
    }
    #[cfg(test)]
    pub(crate) fn test_snapshot(&self) -> Session {
        self.collect(self.recent.clone())
    }
    fn collect(&self, recent: [Vec<PathBuf>; 3]) -> Session {
        let mut docs = BTreeMap::new();
        let mut windows = Vec::new();
        let mut sheets = BTreeSet::new();
        for (window, documents) in self.windows.values().chain(self.closed.iter()) {
            let mut window = window.clone();
            repair_window(&mut window, documents);
            for sheet in &mut window.sheets {
                if !sheets.insert(sheet.id) {
                    let old = sheet.id;
                    sheet.id = crate::workbench::next_sheet();
                    sheets.insert(sheet.id);
                    for group in &mut window.workbench.groups {
                        for id in group.sheets.iter_mut().chain(&mut group.mru) {
                            if *id == old {
                                *id = sheet.id;
                            }
                        }
                        if group.selected.remove(&old) {
                            group.selected.insert(sheet.id);
                        }
                        if group.focused == Some(old) {
                            group.focused = Some(sheet.id);
                        }
                        if group.anchor == Some(old) {
                            group.anchor = Some(sheet.id);
                        }
                    }
                    eprintln!("Repaired duplicate sheet identity in session window");
                }
            }
            windows.push(window);
            for document in documents {
                docs.insert(document.id, document.clone());
            }
        }
        let [recent_files, recent_folders, recent_projects] = recent;
        Session {
            windows,
            documents: docs.into_values().collect(),
            recent_files,
            recent_folders,
            recent_projects,
            ..Session::default()
        }
    }
    pub fn close(&mut self, id: u64, retain: bool) {
        self.live.remove(&id);
        self.open_windows.remove(&id);
        self.open_inbox.remove(&id);
        self.menu_merges
            .retain(|source, destination| *source != id && *destination != id);
        self.recent_heads.remove(&id);
        if let Some(window) = self.windows.remove(&id)
            && retain
        {
            self.closed.push(window);
        }
        if self.active_window == Some(id) {
            self.active_window = self.windows.keys().next_back().copied();
        }
        if let Some(writer) = &self.writer {
            let mut session = self.collect(self.recent.clone());
            session.apply_policy(
                if self.hot == HotExit::OnlyOnQuit {
                    HotExit::Always
                } else {
                    self.hot
                },
                false,
                self.remember,
            );
            let _ = writer.changed(session);
        }
    }
    /// Retain a departing window independently before its live ID is reused by a project.
    /// The application has already published and flushed the old snapshot at this point.
    pub fn retain_departing(&mut self, id: u64, retain: bool) {
        if let Some((window, documents)) = self.windows.remove(&id)
            && retain
            && documents.iter().any(|document| {
                document.dirty
                    || (document.path.is_none()
                        && document.history.as_ref().is_some_and(|history| {
                            history
                                .current_text()
                                .is_ok_and(|text| text.len_bytes() > 0)
                        }))
            })
        {
            self.closed.push((window, documents));
        }
    }
    pub fn clear_recent(&mut self, id: u64) {
        for kind in 0..3 {
            self.clear_recent_kind(id, kind);
        }
    }
    pub fn clear_recent_kind(&mut self, id: u64, kind: usize) {
        self.forgotten_recent[kind].extend(self.recent[kind].drain(..));
        self.recent_heads.entry(id).or_default()[kind] = None;
    }
    pub fn flush(&self) -> Result<(), String> {
        // A broken project workspace must not suppress flushes for other windows.
        let mut errors = Vec::new();
        for writer in self.writer.iter().chain(self.workspaces.values()) {
            if let Err(error) = writer.flush() {
                errors.push(error);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }
    pub fn poll(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        for writer in self.writer.iter().chain(self.workspaces.values()) {
            out.extend(writer.warnings.try_iter());
            while let Ok(result) = writer.results.try_recv() {
                if let Err(e) = result {
                    out.push(format!("Session save failed: {e}"));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> (Window, Vec<Document>) {
        snapshot_text("dirty")
    }
    fn snapshot_text(text: &str) -> (Window, Vec<Document>) {
        let mut workspace =
            crate::workspace::Workspace::new(vel_text::Text::new(), None, None, false).unwrap();
        workspace
            .editor_mut()
            .run("insert", &vel_config::json!({"characters":text}))
            .unwrap();
        workspace.snapshot().unwrap()
    }
    #[test]
    fn r19_checkpoint_repairs_previous_window_without_blocking_next_publish() {
        let dir = tempfile::tempdir().unwrap();
        let mut sessions = Sessions::new(Some(dir.path()));
        let first = sessions.register();
        let second = sessions.register();
        let policy = Policy {
            hot: HotExit::Always,
            quitting: true,
            remember: true,
            closing: false,
        };
        let (window, documents) = snapshot_text("first dirty");
        sessions
            .publish(first, window, documents, Default::default(), policy)
            .unwrap();
        let (mut next, documents) = snapshot_text("second dirty");
        let (bad, _) = sessions.windows.get_mut(&first).unwrap();
        bad.workbench.groups[0].focused = None;
        bad.workbench.groups[0].selected.insert(u64::MAX);
        bad.workbench.groups[0].sheets.push(u64::MAX);
        bad.sheets[0].view.scroll.0 = f64::NAN;
        // Even cross-window sheet identity damage cannot poison a valid publish.
        let old = next.sheets[0].id;
        next.sheets[0].id = bad.sheets[0].id;
        next.workbench.groups[0].remove(old);
        next.workbench.groups[0].add(next.sheets[0].id, false);
        sessions
            .publish(second, next, documents, Default::default(), policy)
            .unwrap();
        sessions.flush().unwrap();
        let recovered = vel_session::load(&dir.path().join("Sessions/Session.sublime-session"));
        assert!(recovered.messages.is_empty(), "{:?}", recovered.messages);
        recovered.session.validate().unwrap();
        assert_eq!(recovered.session.windows.len(), 2);
        assert_eq!(recovered.session.documents.len(), 2);
        assert!(sessions.messages.is_empty());
    }
    #[test]
    fn recovered_reservations_preserve_other_dirty_windows_during_first_publish() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Sessions/Session.sublime-session");
        let (first, mut first_documents) = snapshot_text("recovered first");
        let (mut second, mut second_documents) = snapshot_text("recovered second");
        let (published, published_documents) = snapshot_text("updated first");
        // An earlier process may have used exactly the ID of a new live document.
        // Its other recovered window must survive the first live publish unchanged.
        second_documents[0].id = published_documents[0].id;
        second.sheets[0].document = second_documents[0].id;
        first_documents.extend(second_documents);
        let recovered = Session {
            windows: vec![first, second],
            documents: first_documents,
            recent_files: vec![dir.path().join("recent-file")],
            recent_folders: vec![dir.path().join("recent-folder")],
            recent_projects: vec![dir.path().join("recent-project.sublime-project")],
            ..Session::default()
        };
        vel_session::save(&path, &recovered).unwrap();
        let original = std::fs::read(&path).unwrap();
        let mut sessions = Sessions::new(Some(dir.path()));
        let reserved = sessions.reserve_recovered();
        assert_eq!(reserved.len(), 2);
        let maximum = recovered
            .windows
            .iter()
            .flat_map(|window| window.sheets.iter().map(|sheet| sheet.id))
            .chain(recovered.documents.iter().map(|document| document.id))
            .max()
            .unwrap();
        assert!(crate::workbench::next_sheet() > maximum);
        assert!(reserved[0] < reserved[1]);
        assert_eq!(sessions.active_window, Some(reserved[0]));
        assert_eq!(
            sessions.restored.windows[1].sheets[0].document,
            published_documents[0].id
        );
        for (index, id) in reserved.iter().enumerate() {
            let (window, documents) = &sessions.windows[id];
            assert_eq!(window.sheets[0].id, recovered.windows[index].sheets[0].id);
            assert_eq!(documents.len(), 1);
            assert_eq!(window.sheets[0].document, documents[0].id);
            assert!(documents[0].dirty);
            assert_ne!(documents[0].id, published_documents[0].id);
        }
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert!(sessions.register() > reserved[1]);
        sessions
            .publish(
                reserved[0],
                published,
                published_documents,
                Default::default(),
                Policy {
                    hot: HotExit::Always,
                    quitting: false,
                    closing: false,
                    remember: true,
                },
            )
            .unwrap();
        sessions.flush().unwrap();
        let persisted = vel_session::load(&path);
        assert!(persisted.messages.is_empty(), "{:?}", persisted.messages);
        let persisted = persisted.session;
        assert_eq!(persisted.windows.len(), 2);
        assert_eq!(persisted.documents.len(), 2);
        assert_eq!(persisted.recent_files, recovered.recent_files);
        assert_eq!(persisted.recent_folders, recovered.recent_folders);
        assert_eq!(persisted.recent_projects, recovered.recent_projects);
        for (index, expected) in ["updated first", "recovered second"].iter().enumerate() {
            let document = persisted
                .documents
                .iter()
                .find(|document| document.id == persisted.windows[index].sheets[0].document)
                .unwrap();
            assert!(document.dirty);
            assert_eq!(
                document.history.as_ref().unwrap().current_text().unwrap(),
                *expected
            );
        }
        persisted.validate().unwrap();
    }
    #[test]
    fn reserving_empty_recovery_creates_no_windows_or_writer() {
        let mut sessions = Sessions::new(None);
        assert!(sessions.reserve_recovered().is_empty());
        assert!(sessions.windows.is_empty());
        assert!(sessions.writer.is_none());
        assert_eq!(sessions.active_window, None);
        assert_eq!(sessions.register(), 1);
    }
    #[test]
    fn independent_windows_and_closed_hot_exit_windows_survive_fresh_storage() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("missing/profile");
        let mut sessions = Sessions::new(Some(&data));
        let a = sessions.register();
        let b = sessions.register();
        assert_eq!(sessions.active_window, Some(b));
        let (window, documents) = snapshot();
        sessions
            .publish(
                a,
                window,
                documents,
                [vec![PathBuf::from("recent-a")], Vec::new(), Vec::new()],
                Policy {
                    hot: HotExit::Always,
                    quitting: false,
                    closing: false,
                    remember: true,
                },
            )
            .unwrap();
        let (window, documents) = snapshot();
        sessions
            .publish(
                b,
                window,
                documents,
                [vec![PathBuf::from("recent-b")], Vec::new(), Vec::new()],
                Policy {
                    hot: HotExit::Always,
                    quitting: false,
                    closing: false,
                    remember: true,
                },
            )
            .unwrap();
        sessions.flush().unwrap();
        let path = data.join("Sessions/Session.sublime-session");
        let recovered = vel_session::load(&path);
        assert!(recovered.messages.is_empty());
        assert_eq!(recovered.session.windows.len(), 2);
        assert_eq!(recovered.session.documents.len(), 2);
        assert_eq!(
            recovered.session.recent_files,
            vec![PathBuf::from("recent-b"), PathBuf::from("recent-a")]
        );
        sessions.close(a, true);
        assert_eq!(sessions.active_window, Some(b));
        sessions.close(b, false);
        assert_eq!(sessions.active_window, None);
        sessions.flush().unwrap();
        let recovered = vel_session::load(&path);
        assert_eq!(recovered.session.windows.len(), 1);
        assert!(recovered.session.documents[0].dirty);
    }
    #[test]
    fn only_on_quit_crash_recovery_and_disabled_close_rules_differ() {
        for hot in [HotExit::OnlyOnQuit, HotExit::Disabled] {
            let dir = tempfile::tempdir().unwrap();
            let mut sessions = Sessions::new(Some(dir.path()));
            let id = sessions.register();
            let (window, documents) = snapshot();
            sessions
                .publish(
                    id,
                    window,
                    documents,
                    Default::default(),
                    Policy {
                        hot,
                        quitting: false,
                        closing: false,
                        remember: true,
                    },
                )
                .unwrap();
            sessions.flush().unwrap();
            let recovered = vel_session::load(&dir.path().join("Sessions/Session.sublime-session"));
            assert_eq!(
                !recovered.session.documents.is_empty(),
                hot == HotExit::OnlyOnQuit
            );
            sessions.close(id, false);
            sessions.flush().unwrap();
            assert!(
                vel_session::load(&dir.path().join("Sessions/Session.sublime-session"))
                    .session
                    .windows
                    .is_empty()
            );
        }
    }
}

#[cfg(test)]
mod round15_tests {
    use super::*;
    #[test]
    fn clear_recents_survives_other_window_autosave_and_can_remember_a_reopened_item() {
        let mut sessions = Sessions::new(None);
        let first = sessions.register();
        let second = sessions.register();
        let mut w =
            crate::workspace::Workspace::new(vel_text::Text::new(), None, None, false).unwrap();
        let (window, documents) = w.snapshot().unwrap();
        let recent = [
            vec![PathBuf::from("old.txt")],
            vec![PathBuf::from("old-folder")],
            vec![],
        ];
        let policy = Policy {
            hot: HotExit::Always,
            quitting: false,
            remember: true,
            closing: false,
        };
        for id in [first, second] {
            sessions
                .publish(
                    id,
                    window.clone(),
                    documents.clone(),
                    recent.clone(),
                    policy,
                )
                .unwrap();
        }
        sessions.clear_recent(first);
        sessions
            .publish(
                first,
                window.clone(),
                documents.clone(),
                Default::default(),
                policy,
            )
            .unwrap();
        sessions
            .publish(
                second,
                window.clone(),
                documents.clone(),
                recent.clone(),
                policy,
            )
            .unwrap();
        assert!(
            sessions
                .collect(sessions.recent.clone())
                .recent_files
                .is_empty()
        );
        sessions
            .publish(first, window, documents, recent, policy)
            .unwrap();
        assert_eq!(
            sessions.collect(sessions.recent.clone()).recent_files,
            vec![PathBuf::from("old.txt")]
        );
    }
    #[test]
    fn project_switch_retains_the_untitled_window_beside_the_new_project_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let mut sessions = Sessions::new(Some(dir.path()));
        let id = sessions.register();
        let mut w =
            crate::workspace::Workspace::new(vel_text::Text::new(), None, None, false).unwrap();
        w.editor_mut()
            .run(
                "insert",
                &serde_json::json!({"characters":"unsaved before project"}),
            )
            .unwrap();
        let (window, documents) = w.snapshot().unwrap();
        let policy = Policy {
            hot: HotExit::Always,
            quitting: false,
            remember: true,
            closing: true,
        };
        sessions
            .publish(id, window, documents, Default::default(), policy)
            .unwrap();
        sessions.flush().unwrap();
        sessions.retain_departing(id, true);
        let project = dir.path().join("new.sublime-project");
        std::fs::write(&project, "{}").unwrap();
        w.open_project(project).unwrap();
        let (window, documents) = w.snapshot().unwrap();
        sessions
            .publish(id, window, documents, Default::default(), policy)
            .unwrap();
        sessions.flush().unwrap();
        let recovered = vel_session::load(&dir.path().join("Sessions/Session.sublime-session"));
        assert!(recovered.messages.is_empty());
        assert_eq!(recovered.session.windows.len(), 2);
        assert!(recovered.session.documents.iter().any(|d| {
            d.history
                .as_ref()
                .and_then(|h| h.current_text().ok())
                .is_some_and(|text| text == "unsaved before project")
        }));
    }
}

#[cfg(test)]
mod review15_tests {
    use super::*;
    use crate::{cli::FileLocation, workspace::Workspace};
    use vel_text::Text;
    #[test]
    fn project_switches_keep_only_dirty_or_untitled_content() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("saved.txt");
        std::fs::write(&file, "clean saved text").unwrap();
        let mut w = Workspace::new(Text::new(), None, None, false).unwrap();
        w.open(FileLocation {
            path: file,
            line: 1,
            column: 1,
        })
        .unwrap();
        let mut sessions = Sessions::new(Some(dir.path()));
        let id = sessions.register();
        for _ in 0..3 {
            let (mut window, documents) = w.snapshot().unwrap();
            window.project = Some(dir.path().join("departing.sublime-project"));
            window.folders = vec![dir.path().into()];
            sessions.windows.insert(id, (window, documents));
            sessions.retain_departing(id, true);
        }
        assert!(sessions.closed.is_empty());
        for dirty in [false, true] {
            let mut unsaved =
                Workspace::new(Text::from("untitled content"), None, None, false).unwrap();
            if dirty {
                unsaved
                    .editor_mut()
                    .run("insert", &vel_config::json!({"characters":"dirty "}))
                    .unwrap();
            }
            sessions.windows.insert(id, unsaved.snapshot().unwrap());
            sessions.retain_departing(id, true);
        }
        assert_eq!(sessions.closed.len(), 2);
        let (window, documents) = w.snapshot().unwrap();
        sessions
            .publish(
                id,
                window,
                documents,
                Default::default(),
                Policy {
                    hot: HotExit::Always,
                    quitting: true,
                    closing: false,
                    remember: true,
                },
            )
            .unwrap();
        sessions.flush().unwrap();
        let persisted = vel_session::load(&dir.path().join("Sessions/Session.sublime-session"));
        assert!(persisted.messages.is_empty(), "{:?}", persisted.messages);
        assert_eq!(persisted.session.windows.len(), 3);
        persisted.session.validate().unwrap();
    }
}
