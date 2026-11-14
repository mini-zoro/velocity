//! Application-wide session coordinator shared by all native window controllers.
use std::{
    collections::BTreeMap,
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
        for (index, incoming) in recent.into_iter().enumerate() {
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
        let mut workspace =
            crate::workspace::Workspace::new(vel_text::Text::new(), None, None, false).unwrap();
        workspace
            .editor_mut()
            .run("insert", &vel_config::json!({"characters":"dirty"}))
            .unwrap();
        workspace.snapshot().unwrap()
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
