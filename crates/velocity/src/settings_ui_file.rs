//! Snapshots and guarded writes for one open Settings page. The serial preferences
//! queue owns write ordering; the UI never waits for that queue to drain.
use crate::{workbench, workspace::Workspace};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::SystemTime,
};
use vel_config::{Value, json::Document, settings::validate_layer};
use vel_fileio::atomic::{self, Durability, Stage};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Snapshot(Option<(String, SystemTime)>);

impl Snapshot {
    pub(super) fn read(path: &Path) -> Result<Self, String> {
        if vel_fileio::regular::metadata(path, true)?.is_none() {
            return Ok(Self(None));
        }
        let mut file = vel_fileio::regular::read(path)?;
        let modified = file.metadata().and_then(|m| m.modified()).map_err(io)?;
        let mut source = String::new();
        file.read_to_string(&mut source).map_err(io)?;
        Ok(Self(Some((source, modified))))
    }

    fn source(&self) -> &str {
        self.0.as_ref().map_or("{}", |(s, _)| s)
    }

    pub(super) fn object(&self) -> Result<Document, String> {
        let doc = Document::parse(self.source())?;
        doc.value
            .is_object()
            .then_some(doc)
            .ok_or_else(|| "Expected a JSON object at line 1, column 1.".into())
    }
}

fn io(e: std::io::Error) -> String {
    e.to_string()
}

#[derive(Debug)]
pub(super) enum Issue {
    Outside,
    Io(String),
}

impl From<String> for Issue {
    fn from(message: String) -> Self {
        Self::Io(message)
    }
}

#[derive(Debug)]
pub(super) struct Failure {
    pub(super) id: Option<u64>,
    pub(super) draft: Option<(usize, String)>,
    pub(super) issue: Issue,
}

#[derive(Debug, Default)]
struct State {
    expected: Option<Snapshot>,
    outside: bool,
    pending: usize,
    failures: Vec<Failure>,
}

#[derive(Debug)]
pub(super) struct FileSession {
    opening: Option<Snapshot>,
    state: Arc<Mutex<State>>,
}

pub(super) enum Mutation {
    Key {
        key: &'static str,
        previous: Option<Value>,
        value: Option<Value>,
    },
    Restore,
}

impl FileSession {
    pub(super) fn new(snapshot: Option<Snapshot>) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                expected: snapshot.clone(),
                ..State::default()
            })),
            opening: snapshot,
        }
    }

    pub(super) fn observe(&self, snapshot: Option<&Snapshot>) {
        let mut state = self.state.lock().unwrap();
        if state.pending == 0 && state.expected.as_ref() != snapshot {
            state.outside = true;
        }
    }

    pub(super) fn pending(&self) -> bool {
        self.state.lock().unwrap().pending != 0
    }

    pub(super) fn results(&self) -> (bool, Vec<Failure>) {
        let mut state = self.state.lock().unwrap();
        (state.pending != 0, std::mem::take(&mut state.failures))
    }

    pub(super) fn queue(
        &self,
        workspace: &mut Workspace,
        path: PathBuf,
        mutation: Mutation,
        id: Option<u64>,
        draft: Option<(usize, String)>,
    ) -> Result<(), String> {
        let state = self.state.clone();
        let opening = matches!(mutation, Mutation::Restore)
            .then(|| self.opening.clone())
            .flatten();
        let overrides = match &mutation {
            Mutation::Key { key, value, .. } => vec![(key.to_string(), value.clone())],
            Mutation::Restore => vec![],
        };
        let defaults = workspace
            .settings
            .layer(vel_config::settings::Layer::Default)
            .clone();
        workspace.settings_reports.claim(&path);
        self.state.lock().unwrap().pending += 1;
        let job = crate::release::enqueue(move || {
            let result = (|| {
                let before = Snapshot::read(&path)?;
                let outside = {
                    let mut state = state.lock().unwrap();
                    state.outside |= state.expected.as_ref() != Some(&before);
                    state.outside
                };
                let source = match mutation {
                    Mutation::Key {
                        key,
                        previous,
                        value,
                    } => {
                        let mut doc = before.object()?;
                        validate_layer(doc.value.as_object_mut().unwrap(), &defaults);
                        if doc.value.get(key) != previous.as_ref() {
                            return Err(Issue::Outside);
                        }
                        Some(workbench::update_preference(
                            before.source(),
                            key,
                            value.as_ref(),
                        )?)
                    }
                    Mutation::Restore => {
                        let opening = opening.filter(|_| !outside).ok_or(Issue::Outside)?;
                        before.object()?;
                        opening.object()?;
                        opening.0.map(|(source, _)| source)
                    }
                };
                publish(&path, &before, source)
            })();
            let mut state = state.lock().unwrap();
            state.pending -= 1;
            match result {
                Ok(after) => {
                    state.expected = Some(after);
                    Ok(())
                }
                Err(issue) => {
                    let message = match &issue {
                        Issue::Outside => {
                            state.outside = true;
                            "The file was changed outside Settings".into()
                        }
                        Issue::Io(message) => message.clone(),
                    };
                    state.failures.push(Failure { id, draft, issue });
                    Err(vel_config::json::source_diagnostic(
                        &path.to_string_lossy(),
                        &message,
                    ))
                }
            }
        });
        match job {
            Ok(rx) => {
                workspace.track_settings_write(rx, &overrides);
                Ok(())
            }
            Err(error) => {
                self.state.lock().unwrap().pending -= 1;
                Err(error)
            }
        }
    }
}

fn publish(path: &Path, before: &Snapshot, source: Option<String>) -> Result<Snapshot, Issue> {
    if source.as_deref() == before.0.as_ref().map(|(s, _)| s.as_str()) {
        return Ok(before.clone());
    }
    let check = || {
        if Snapshot::read(path)? == *before {
            Ok(())
        } else {
            Err(Issue::Outside)
        }
    };
    let parent = atomic::parent(path);
    let Some(source) = source else {
        check()?;
        fs::remove_file(path).map_err(io)?;
        return Ok(Snapshot(None));
    };
    fs::create_dir_all(parent).map_err(io)?;
    let mut stage = Stage::new(parent, ".velocity-settings-", None).map_err(io)?;
    stage.file().write_all(source.as_bytes()).map_err(io)?;
    stage.sync(Durability::Barrier).map_err(io)?;
    let modified = stage
        .file()
        .metadata()
        .and_then(|m| m.modified())
        .map_err(io)?;
    // Check again after staging and syncing, as close as possible to the rename.
    check()?;
    if let Some(warning) = stage
        .publish(path, before.0.is_none())
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Issue::Outside
            } else {
                Issue::Io(io(error))
            }
        })?
        .warning
    {
        eprintln!("{warning}");
    }
    Ok(Snapshot(Some((source, modified))))
}
