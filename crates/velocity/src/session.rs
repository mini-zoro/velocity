//! Application-wide session coordinator shared by all native window controllers.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::Duration,
};
use vel_session::{Document, HotExit, Session, Window, Writer};
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
    pub instance: Option<crate::instance::Instance>,
    pub active_window: Option<u64>,
    recent: [Vec<PathBuf>; 3],
    recent_heads: BTreeMap<u64, [Option<PathBuf>; 3]>,
    forgotten_recent: [BTreeSet<PathBuf>; 3],
    hot: HotExit,
    remember: bool,
    pub messages: Vec<String>,
    windows: BTreeMap<u64, (Window, Vec<Document>)>,
    closed: Vec<(Window, Vec<Document>)>,
    writer: Option<Writer>,
    workspaces: BTreeMap<PathBuf, Writer>,
    next: u64,
}
impl Sessions {
    pub fn new(data: Option<&Path>) -> Self {
        let path = data.map(|p| p.join("Sessions/Session.sublime-session"));
        let recovery = path.as_deref().map(vel_session::load).unwrap_or_default();
        let mut messages = recovery.messages;
        let writer = path.and_then(|p| match Writer::new(p, Duration::from_millis(400)) {
            Ok(w) => Some(w),
            Err(e) => {
                messages.push(e);
                None
            }
        });
        Self {
            restored: recovery.session,
            instance: None,
            active_window: None,
            recent: Default::default(),
            recent_heads: BTreeMap::new(),
            forgotten_recent: Default::default(),
            hot: HotExit::Always,
            remember: true,
            messages,
            windows: BTreeMap::new(),
            closed: vec![],
            writer,
            workspaces: BTreeMap::new(),
            next: 1,
        }
    }
    pub fn register(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        self.active_window = Some(id);
        id
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
        window: Window,
        documents: Vec<Document>,
        recent: [Vec<PathBuf>; 3],
        policy: Policy,
    ) -> Result<(), String> {
        self.hot = policy.hot;
        self.remember = policy.remember;
        self.windows.insert(id, (window.clone(), documents.clone()));
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
        if let Some(project) = &window.project {
            let path = vel_session::project::workspace_path(project);
            if !self.workspaces.contains_key(&path) {
                self.workspaces.insert(
                    path.clone(),
                    Writer::new(path.clone(), Duration::from_millis(400))?,
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
    fn collect(&self, recent: [Vec<PathBuf>; 3]) -> Session {
        let mut docs = BTreeMap::new();
        let mut windows = Vec::new();
        for (window, documents) in self.windows.values().chain(self.closed.iter()) {
            windows.push(window.clone());
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
        for (paths, forgotten) in self.recent.iter_mut().zip(&mut self.forgotten_recent) {
            forgotten.extend(paths.drain(..));
        }
        self.recent_heads.insert(id, Default::default());
    }
    pub fn flush(&self) -> Result<(), String> {
        if let Some(writer) = &self.writer {
            writer.flush()?;
        }
        for writer in self.workspaces.values() {
            writer.flush()?;
        }
        Ok(())
    }
    pub fn poll(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        for writer in self.writer.iter().chain(self.workspaces.values()) {
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
