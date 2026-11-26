//! Detached Hot Exit windows are immutable, individually durable records.
use std::{
    collections::BTreeMap,
    fs,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};
use vel_fileio::atomic::{Durability, Stage};
use vel_session::{Recovery, Session};

#[derive(Debug)]
pub(super) struct Journal {
    directory: Option<PathBuf>,
    next: u64,
    loaded: u64,
    adopted: u64,
    committed_on_load: u64,
}

impl Journal {
    pub(super) fn recover(data: Option<&Path>, recovery: &mut Recovery) -> Self {
        let adopted = recovery.session.adopted_departure;
        let mut journal = Self {
            directory: data.map(|p| p.join("Sessions/Detached")),
            next: adopted.saturating_add(1),
            loaded: adopted,
            adopted,
            committed_on_load: adopted,
        };
        let Some(dir) = &journal.directory else {
            return journal;
        };
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    recovery
                        .messages
                        .push(format!("{}: {error}", dir.display()));
                }
                return journal;
            }
        };
        // Only paths/counters for records not replayed this time. The session
        // format still bounds the number of windows and documents it can open.
        let mut records = BTreeMap::new();
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    recovery
                        .messages
                        .push(format!("{}: {error}", dir.display()));
                    return journal;
                }
            };
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "session")
                && let Some(id) = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.parse::<u64>().ok())
            {
                journal.next = journal.next.max(id.saturating_add(1));
                records.insert(id, path);
            }
        }
        if adopted > 0 {
            // Keep records needed by the fallback generation too. If it is
            // unreadable, leave the files alone.
            let previous = dir.with_file_name("Session.sublime-session.1");
            if let Ok(file) = fs::File::open(previous)
                && let Ok(header) =
                    serde_json::from_reader::<_, Header>(std::io::BufReader::new(file))
            {
                for (_, path) in records.range(..=adopted.min(header.adopted_departure)) {
                    let _ = fs::remove_file(path);
                }
            }
        }
        let session = &mut recovery.session;
        let mut next_id = session
            .documents
            .iter()
            .map(|d| d.id)
            .chain(
                session
                    .windows
                    .iter()
                    .flat_map(|w| w.sheets.iter().map(|s| s.id)),
            )
            .max()
            .unwrap_or_default();
        for (id, path) in records.into_iter().filter(|(id, _)| *id > adopted) {
            if session.windows.len() == 128 || session.documents.len() == 10_000 {
                recovery.messages.push("More detached Hot Exit drafts remain on disk; reopen after freeing session space.".into());
                break;
            }
            let mut record = vel_session::load(&path);
            recovery.messages.append(&mut record.messages);
            if record.source.is_none() {
                // Do not advance the committed cursor over a record that could
                // not be read, and preserve any text recovered from it.
                append(session, record.session, &mut next_id);
                recovery.messages.push(format!(
                    "Could not fully restore detached drafts from {}",
                    path.display()
                ));
                break;
            }
            if !append(session, record.session, &mut next_id) {
                recovery.messages.push("More detached Hot Exit drafts remain on disk; reopen after freeing session space.".into());
                break;
            }
            journal.loaded = id;
        }
        journal
    }

    pub(super) fn stage(&mut self, session: Session) -> Result<PathBuf, String> {
        let dir = self
            .directory
            .as_ref()
            .ok_or("No storage for detached Hot Exit drafts")?;
        let id = self.next;
        self.next = id
            .checked_add(1)
            .ok_or("Detached Hot Exit generation exhausted")?;
        let path = dir.join(format!("{id}.session"));
        // No replacement is expected; a new record never shares another
        // record's file or in-memory snapshot.
        session.validate()?;
        let error = |e| vel_fileio::regular::error(&path, e);
        if !dir.is_dir() {
            fs::create_dir_all(dir).map_err(error)?;
            #[cfg(unix)]
            fs::File::open(vel_fileio::atomic::parent(dir))
                .and_then(|parent| parent.sync_all())
                .map_err(error)?;
        }
        let mut stage = Stage::new(dir, ".velocity-detached-", None).map_err(error)?;
        {
            let mut output = BufWriter::new(stage.file());
            serde_json::to_writer(&mut output, &session)
                .map_err(|e| vel_fileio::regular::error(&path, e))?;
            output.flush().map_err(error)?;
        }
        stage.sync(Durability::Barrier).map_err(error)?;
        if let Some(warning) = stage.publish(&path, true).map_err(error)?.warning {
            return Err(warning);
        }
        Ok(path)
    }

    pub(super) fn adopt(&mut self) {
        self.adopted = self.loaded;
    }

    pub(super) fn adopted(&self) -> u64 {
        self.adopted
    }

    pub(super) fn committed_on_load(&self) -> u64 {
        self.committed_on_load
    }
}

#[derive(serde::Deserialize)]
struct Header {
    #[serde(default)]
    adopted_departure: u64,
}

fn append(into: &mut Session, mut record: Session, next: &mut u64) -> bool {
    if into.windows.len() + record.windows.len() > 128
        || into.documents.len() + record.documents.len() > 10_000
    {
        return false;
    }
    let mut new_id = || {
        *next += 1;
        *next
    };
    let docs: BTreeMap<_, _> = record
        .documents
        .iter_mut()
        .map(|doc| {
            let old = doc.id;
            doc.id = new_id();
            (old, doc.id)
        })
        .collect();
    for window in &mut record.windows {
        let sheets: BTreeMap<_, _> = window
            .sheets
            .iter_mut()
            .map(|sheet| {
                let old = sheet.id;
                sheet.id = new_id();
                sheet.document = docs[&sheet.document];
                (old, sheet.id)
            })
            .collect();
        for group in &mut window.workbench.groups {
            for id in group.sheets.iter_mut().chain(&mut group.mru) {
                *id = sheets[id];
            }
            group.selected = group.selected.iter().map(|id| sheets[id]).collect();
            group.focused = group.focused.map(|id| sheets[&id]);
            group.anchor = group.anchor.map(|id| sheets[&id]);
        }
    }
    into.windows.extend(record.windows);
    into.documents.extend(record.documents);
    true
}
